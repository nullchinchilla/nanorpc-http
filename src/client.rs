use std::{
    net::SocketAddr,
    sync::RwLock,
    time::{Duration, Instant},
};

use async_tcpstream_hyper::HyperStream;
use async_trait::async_trait;
use bytes::Bytes;
use concurrent_queue::ConcurrentQueue;
use http_body_util::{BodyExt, Full};
use hyper::{client::conn::http1::SendRequest, Request};
use nanorpc::{JrpcRequest, JrpcResponse, RpcTransport};
use smol::future::FutureExt;

type Conn = SendRequest<Full<Bytes>>;

/// An HTTP-based [RpcTransport] for nanorpc.
pub struct HttpRpcTransport {
    exec: smol::Executor<'static>,
    remote: SocketAddr,
    pool: ConcurrentQueue<(Conn, Instant)>,
    idle_timeout: Duration,

    req_timeout: RwLock<Duration>,
}

impl HttpRpcTransport {
    /// Create a new HttpRpcTransport that goes to the given socket address over unencrypted HTTP/1.1. A default timeout is applied.
    ///
    /// Currently, custom paths, HTTPS, etc are not supported.
    pub fn new(remote: SocketAddr) -> Self {
        Self {
            exec: smol::Executor::new(),
            remote,
            pool: ConcurrentQueue::bounded(64),
            idle_timeout: Duration::from_secs(60),
            req_timeout: Duration::from_secs(30).into(),
        }
    }

    /// Sets the timeout for this transport. If `None`, disables any timeout handling.
    pub fn set_timeout(&self, timeout: Option<Duration>) {
        *self.req_timeout.write().unwrap() = timeout.unwrap_or(Duration::MAX);
    }

    /// Gets the timeout for this transport.
    pub fn timeout(&self) -> Option<Duration> {
        let to = *self.req_timeout.read().unwrap();
        if to == Duration::MAX {
            None
        } else {
            Some(to)
        }
    }

    /// Opens a new connection to the remote.
    async fn open_conn(&self) -> std::io::Result<Conn> {
        while let Ok((conn, expiry)) = self.pool.pop() {
            if expiry.elapsed() < self.idle_timeout {
                return Ok(conn);
            }
        }
        // okay there's nothing in the pool for us. create a new conn asap
        let conn = HyperStream::new_wrapped(smol::net::TcpStream::connect(self.remote).await?);
        let (conn, handle) = hyper::client::conn::http1::handshake(conn)
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e))?;

        self.exec
            .spawn(async move {
                let _ = handle.await;
            })
            .detach();
        Ok(conn)
    }
}

#[async_trait]
impl RpcTransport for HttpRpcTransport {
    type Error = std::io::Error;

    async fn call_raw(&self, req: JrpcRequest) -> Result<JrpcResponse, Self::Error> {
        let timeout = *self.req_timeout.read().unwrap();
        async {
            let mut conn = self.open_conn().await?;
            let response = conn
                .send_request(
                    Request::builder()
                        .method("POST")
                        .body(Full::new(serde_json::to_vec(&req)?.into()))
                        .expect("could not build request"),
                )
                .await
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e))?;
            let response = response
                .into_body()
                .collect()
                .await
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::BrokenPipe, e))?
                .to_bytes();
            let resp = serde_json::from_slice(&response)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            let _ = self.pool.push((conn, Instant::now()));
            Ok(resp)
        }
        .or(async {
            smol::Timer::after(timeout).await;
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "nanorpc-http request timed out",
            ))
        })
        .or(self.exec.run(std::future::pending()))
        .await
    }
}
