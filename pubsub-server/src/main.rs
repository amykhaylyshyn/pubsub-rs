mod pubsub;

use clap::Parser;
use dotenv::dotenv;
use futures::StreamExt;
use pubsub::PubSub;
use std::net::{IpAddr, Ipv4Addr};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
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
}

struct Server {
    port: u16,
    dispatch: PubSub<String, String>,
}

impl Server {
    fn new(port: u16, dispatch: PubSub<String, String>) -> Self {
        Self { port, dispatch }
    }

    async fn run(&self) -> std::io::Result<()> {
        let listener = TcpListenerStream::new(
            TcpListener::bind((IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), self.port)).await?,
        );

        listener
            .for_each_concurrent(None, |stream_result| async move {
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
    let dispatch = PubSub::new(100);
    let server = Server::new(args.port, dispatch.clone());
    Ok(server.run().await?)
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn test_new_connection() -> Result<(), Box<dyn std::error::Error>> {
        Ok(())
    }
}
