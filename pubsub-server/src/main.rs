mod pubsub;

use clap::Parser;
use dotenv::dotenv;
use futures::StreamExt;
use pubsub::PubSub;
use std::net::{IpAddr, Ipv4Addr};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, watch};
use tokio_stream::wrappers::TcpListenerStream;
use tokio_tungstenite::tungstenite::handshake::server::{
    Request as HandshakeRequest, Response as HandshakeResponse,
};
use tokio_tungstenite::tungstenite::http::uri::Uri;

#[derive(Parser)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(parse(try_from_str), short, long)]
    port: u16,
    #[clap(long)]
    jwt_secret: String,
}

struct ServerHandle {
    shutdown_sender: watch::Sender<bool>,
}

impl ServerHandle {
    fn new(shutdown: watch::Sender<bool>) -> Self {
        Self {
            shutdown_sender: shutdown,
        }
    }

    fn shutdown(&self) {
        self.shutdown_sender
            .send(true)
            .map_err(|err| log::error!("server shutdown error"))
            .ok();
    }
}

struct Server {
    port: u16,
    jwt_secret: String,
    dispatch: PubSub<String, String>,
    shutdown_signal: watch::Receiver<bool>,
}

impl Server {
    fn new(
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

    async fn run(self) -> std::io::Result<()> {
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

        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();

    let args: Args = Args::parse();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let dispatch = PubSub::new(100);
    let server = Server::new(args.port, args.jwt_secret.as_str(), dispatch, shutdown_rx);
    Ok(server.run().await?)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
