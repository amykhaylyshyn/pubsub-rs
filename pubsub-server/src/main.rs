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

async fn handle_connection(
    pubsub: PubSub<String, String>,
    raw_stream: TcpStream,
) -> Result<(), Box<dyn std::error::Error>> {
    let remote_addr = raw_stream.peer_addr()?;
    let (upgrade_tx, upgrade_rx) = oneshot::channel();
    let (upgrade_result_tx, upgrade_result_rx) = oneshot::channel();
    let ws_stream = tokio_tungstenite::accept_hdr_async(
        raw_stream,
        |req: &HandshakeRequest, res: HandshakeResponse| {
            log::info!("new connection from {} to {}", remote_addr, req.uri());
            upgrade_tx.send(req.uri().to_owned()).ok();
            upgrade_result_rx.blocking_recv();
            Ok(res)
        },
    )
    .await?;

    let upgrade_req_uri = upgrade_rx.await?;
    upgrade_result_tx.send(());

    let (outgoing, incoming) = ws_stream.split();

    Ok(())
}

async fn run_server(args: Args) -> std::io::Result<()> {
    let pubsub = PubSub::new(100);
    let listener = TcpListenerStream::new(
        TcpListener::bind((IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), args.port)).await?,
    );

    listener
        .for_each_concurrent(None, |stream_result| {
            let pubsub = pubsub.clone();
            async move {
                match stream_result {
                    Ok(stream) => {
                        handle_connection(pubsub, stream)
                            .await
                            .map_err(|err| log::error!("handle connection error: {}", err))
                            .ok();
                    }
                    Err(err) => log::error!("new connection error: {}", err),
                }
            }
        })
        .await;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();

    let args: Args = Args::parse();
    Ok(run_server(args).await?)
}
