mod pubsub;
mod server;

use clap::Parser;
use dotenv::dotenv;
use std::net::{IpAddr, Ipv4Addr};
use tokio::net::TcpListener;

#[derive(Parser)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(parse(try_from_str), short, long)]
    port: u16,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();

    let args: Args = Args::parse();

    let listener = TcpListener::bind((IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), args.port)).await?;

    while let Ok((stream, addr)) = listener.accept().await {
        log::info!("new connection from {:?}", addr);
    }

    Ok(())
}
