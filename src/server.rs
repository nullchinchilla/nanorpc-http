use std::{convert::Infallible, net::SocketAddr};

use async_compat::CompatExt;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{body::Incoming, service::service_fn, Request, Response};
use nanorpc::{JrpcRequest, RpcService};
use smol::net::TcpListener;

/// An HTTP-based nanorpc server.
pub struct HttpRpcServer {
    listener: TcpListener,
}

impl HttpRpcServer {
    /// Creates a new HTTP server listening at the given address.
    pub async fn bind(addr: SocketAddr) -> std::io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Self { listener })
    }

    /// Runs the server, blocking indefinitely until a fatal failure happens. This must be run for the server to make any progress!
    pub async fn run(&self, service: impl RpcService) -> std::io::Result<()> {
        let exec = smol::Executor::new();
        exec.run(async {
            loop {
                let (next, _) = self.listener.accept().await?;
                exec.spawn(async {
                    let connection = hyper::server::conn::http1::Builder::new()
                        .keep_alive(true)
                        .serve_connection(
                            next.compat(),
                            service_fn(|req: Request<Incoming>| async {
                                let response = async {
                                    let body = req.into_body().collect().await?.to_bytes();
                                    let jrpc_req: JrpcRequest = serde_json::from_slice(&body)?;
                                    let jrpc_response = service.respond_raw(jrpc_req).await;
                                    anyhow::Ok(Response::new(
                                        Full::<Bytes>::new(
                                            serde_json::to_vec(&jrpc_response)?.into(),
                                        )
                                        .boxed(),
                                    ))
                                };

                                match response.await {
                                    Ok(resp) => Ok::<_, Infallible>(resp),
                                    Err(err) => Ok(Response::builder()
                                        .status(500)
                                        .body(err.to_string().boxed())
                                        .unwrap()),
                                }
                            }),
                        );
                    let _ = connection.await;
                })
                .detach();
            }
        })
        .await
    }
}
