mod pubsub;
mod server;

use crate::server::Server;
use clap::Parser;
use dotenv::dotenv;
use futures::{pin_mut, select, FutureExt, StreamExt};
use pubsub::PubSub;
use std::net::{IpAddr, Ipv4Addr};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_stream::wrappers::TcpListenerStream;
use tokio_tungstenite::tungstenite::handshake::server::{
    Request as HandshakeRequest, Response as HandshakeResponse,
};

#[derive(Parser)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(parse(try_from_str), short, long)]
    port: u16,
    #[clap(long)]
    jwt_secret: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();
    env_logger::init();

    let args: Args = Args::parse();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let dispatch = PubSub::new(100);
    let server = Server::new(args.port, args.jwt_secret.as_str(), dispatch, shutdown_rx);
    Ok(server.run().await?)
}
