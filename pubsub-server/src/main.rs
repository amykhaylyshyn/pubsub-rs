mod pubsub;
mod redis_pubsub;
mod rest_api;
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
    #[clap(long)]
    redis_address: String,
    #[clap(parse(try_from_str), long)]
    rest_api_port: u16,
    #[clap(long)]
    rest_api_host: Option<String>,
}

#[cfg(target_family = "windows")]
async fn wait_exit_signal() -> io::Result<()> {
    Ok(signal::ctrl_c().await?)
}

#[cfg(target_family = "unix")]
async fn wait_exit_signal() -> io::Result<()> {
    unimplemented!();
}

#[actix_web::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();
    env_logger::init();

    let args: Args = Args::parse();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let dispatch = PubSub::new(100);
    let server = Server::bind_to_port(
        args.port,
        args.jwt_secret.as_str(),
        dispatch.clone(),
        shutdown_rx.clone(),
    )
    .await?;
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
    let redis_url = format!("redis://{}/", args.redis_address);
    let redis_url_clone = redis_url.clone();
    let shutdown_rx_clone = shutdown_rx.clone();
    let redis_fut = async move {
        redis_pubsub::redis_pubsub(redis_url_clone.as_str(), dispatch, shutdown_rx_clone)
            .await
            .map_err(|err| log::error!("redis error: {}", err))
            .ok();
    }
    .fuse();
    let rest_api_host = format!(
        "{}:{}",
        args.rest_api_host.unwrap_or("127.0.0.1".to_owned()),
        args.rest_api_port
    );
    let rest_api_fut = async move {
        rest_api::run_api(rest_api_host.as_str(), redis_url.as_str(), shutdown_rx)
            .await
            .map_err(|err| log::error!("rest api error: {}", err))
            .ok();
    }
    .fuse();

    join!(wait_exit_fut, server_run_fut, redis_fut, rest_api_fut);

    Ok(())
}
