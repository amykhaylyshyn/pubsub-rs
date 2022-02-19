mod pubsub;
mod server;

use crate::server::Server;
use clap::Parser;
use dotenv::dotenv;
use futures::{join, FutureExt};
use pubsub::PubSub;
use std::io;
use tokio::{signal, sync::watch};

#[derive(Parser)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(parse(try_from_str), short, long)]
    port: u16,
    #[clap(long)]
    jwt_secret: String,
}

#[cfg(target_family = "windows")]
async fn wait_exit_signal() -> io::Result<()> {
    Ok(signal::ctrl_c().await?)
}

#[cfg(target_family = "unix")]
async fn wait_exit_signal() -> io::Result<()> {
    unimplemented!();
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();
    env_logger::init();

    let args: Args = Args::parse();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let dispatch = PubSub::new(100);
    let server =
        Server::bind_to_port(args.port, args.jwt_secret.as_str(), dispatch, shutdown_rx).await?;
    let wait_exit_fut = async move {
        wait_exit_signal().await.expect("exit signal");
        shutdown_tx.send(true).expect("shutdown signal sent");
        log::info!("received shutdown signal");
    }
    .fuse();
    let server_run_fut = async move {
        server.run().await.expect("server running");
    }
    .fuse();

    join!(wait_exit_fut, server_run_fut);

    Ok(())
}
