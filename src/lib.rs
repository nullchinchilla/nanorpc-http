pub mod client;
pub mod server;

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use nanorpc::nanorpc_derive;
    use smol::future::FutureExt;

    use crate::{
        client::{HttpRpcTransport, Proxy},
        server::HttpRpcServer,
    };

    #[nanorpc_derive]
    #[async_trait]
    pub trait AddProtocol {
        async fn add(&self, x: f64, y: f64) -> f64 {
            x + y
        }
    }

    struct AddImpl;

    impl AddProtocol for AddImpl {}

    #[test]
    fn simple_pingpong() {
        smol::future::block_on(async {
            let service = AddService(AddImpl);
            let server = HttpRpcServer::bind("127.0.0.1:12345".parse().unwrap())
                .await
                .unwrap();
            async { server.run(service).await.unwrap() }
                .race(async {
                    let client =
                        AddClient::from(HttpRpcTransport::new("127.0.0.1:12345".parse().unwrap()));
                    assert_eq!(client.add(1.0, 2.0).await.unwrap(), 3.0);
                })
                .await
        });
    }
}
