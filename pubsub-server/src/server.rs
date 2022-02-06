use crate::PubSub;
use clap::Parser;
use dotenv::dotenv;
use futures::{pin_mut, select, FutureExt, StreamExt};
use std::net::{IpAddr, Ipv4Addr};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_stream::wrappers::TcpListenerStream;
use tokio_tungstenite::tungstenite::handshake::server::{
    Request as HandshakeRequest, Response as HandshakeResponse,
};

pub struct ServerHandle {
    shutdown_sender: watch::Sender<bool>,
}

impl ServerHandle {
    fn new(shutdown: watch::Sender<bool>) -> Self {
        Self {
            shutdown_sender: shutdown,
        }
    }

    pub fn shutdown(&self) {
        self.shutdown_sender
            .send(true)
            .map_err(|err| log::error!("server shutdown error"))
            .ok();
    }
}

pub struct Server {
    port: u16,
    jwt_secret: String,
    dispatch: PubSub<String, String>,
    shutdown_signal: watch::Receiver<bool>,
}

impl Server {
    pub fn new(
        port: u16,
        jwt_secret: &str,
        dispatch: PubSub<String, String>,
        shutdown_signal: watch::Receiver<bool>,
    ) -> Self {
        Self {
            port,
            jwt_secret: jwt_secret.to_string(),
            dispatch,
            shutdown_signal,
        }
    }

    pub async fn run(self) -> std::io::Result<()> {
        let listener = TcpListenerStream::new(
            TcpListener::bind((IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), self.port)).await?,
        );

        let mut shutdown_rx = self.shutdown_signal.clone();
        listener
            .take_until(async {
                shutdown_rx
                    .changed()
                    .await
                    .map_err(|err| log::error!("shutdown server error"))
                    .ok();
                true
            })
            .for_each_concurrent(None, |stream_result| async {
                match stream_result {
                    Ok(raw_stream) => {
                        self.handle_connection(raw_stream)
                            .await
                            .map_err(|err| log::error!("handle connection error: {}", err))
                            .ok();
                    }
                    Err(err) => log::error!("new connection error: {}", err),
                }
            })
            .await;

        Ok(())
    }

    async fn handle_connection(
        &self,
        raw_stream: TcpStream,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let remote_addr = raw_stream.peer_addr()?;
        let ws_stream = tokio_tungstenite::accept_hdr_async(
            raw_stream,
            |req: &HandshakeRequest, res: HandshakeResponse| {
                log::info!("new connection from {} to {}", remote_addr, req.uri());
                Ok(res)
            },
        )
        .await?;

        let (outgoing, incoming) = ws_stream.split();
        let rx_fut = async move {
            incoming.for_each(|msg| async {}).await;
        }
        .fuse();
        let mut shutdown_rx = self.shutdown_signal.clone();
        let shutdown_fut = async move {
            shutdown_rx.changed().await.ok();
        }
        .fuse();

        pin_mut!(rx_fut, shutdown_fut);

        select! {
            _ = rx_fut => (),
            _ = shutdown_fut => (),
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::connect_async;

    #[tokio::test]
    async fn test_shutdown() -> Result<(), Box<dyn std::error::Error>> {
        let dispatch = PubSub::new(10);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let server = Server::new(4001, "secret", dispatch, shutdown_rx);
        let server_task = tokio::spawn(async move {
            server.run().await.unwrap();
        });
        shutdown_tx.send(true)?;
        server_task.await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_connection() -> Result<(), Box<dyn std::error::Error>> {
        dotenv::dotenv().ok();
        env_logger::init();

        let dispatch = PubSub::new(10);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let server = Server::new(4001, "secret", dispatch, shutdown_rx);
        let server_task = tokio::spawn(async move {
            server.run().await.unwrap();
        });

        let (ws_stream, _) = connect_async("ws://127.0.0.1:4001").await?;
        let (sink, stream) = ws_stream.split();

        shutdown_tx.send(true)?;
        server_task.await?;
        Ok(())
    }
}