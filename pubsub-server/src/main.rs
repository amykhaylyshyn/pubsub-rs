mod pubsub;
mod redis_pubsub;
mod rest_api;
mod server;

use crate::server::Server;
use clap::Parser;
use dotenv::dotenv;
use futures::{join, stream, FutureExt, StreamExt};
use pubsub::PubSub;
use std::io;
use tokio::{signal, sync::watch};

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// WebSocket server listen address. 0.0.0.0 for all network interfaces
    #[clap(long)]
    address: Option<String>,
    #[clap(parse(try_from_str), short, long)]
    /// WebSocket server listen port
    port: u16,
    /// JWT secret used to encode/decode client tokens
    #[clap(long)]
    jwt_secret: String,
    /// Redis address
    #[clap(long)]
    redis_address: String,
    /// REST API listen port
    #[clap(parse(try_from_str), long)]
    rest_api_port: u16,
    /// REST API listen address
    #[clap(long)]
    rest_api_address: Option<String>,
    /// REST API API Key
    #[clap(long)]
    rest_api_key: String,
}

#[cfg(target_family = "windows")]
async fn wait_exit_signal() -> io::Result<()> {
    Ok(signal::ctrl_c().await?)
}

#[cfg(target_family = "unix")]
async fn wait_exit_signal() -> io::Result<()> {
    use tokio::signal::unix::SignalKind;
    use tokio_stream::wrappers::SignalStream;

    let signals = vec![
        SignalStream::new(signal::unix::signal(SignalKind::terminate())?),
        SignalStream::new(signal::unix::signal(SignalKind::interrupt())?),
    ];
    stream::select_all(signals.into_iter()).take(1).next().await;
    Ok(())
}

#[actix_web::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();
    env_logger::init();

    let args: Args = Args::parse();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let dispatch = PubSub::new(100);
    let server_address = format!(
        "{}:{}",
        args.address.unwrap_or("127.0.0.1".to_string()),
        args.port
    );
    let server = Server::bind(
        server_address.as_str(),
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
        args.rest_api_address.unwrap_or("127.0.0.1".to_owned()),
        args.rest_api_port
    );
    let rest_api_fut = async move {
        rest_api::run_api(
            rest_api_host.as_str(),
            redis_url.as_str(),
            args.rest_api_key.as_str(),
            shutdown_rx,
        )
        .await
        .map_err(|err| log::error!("rest api error: {}", err))
        .ok();
    }
    .fuse();

    join!(wait_exit_fut, server_run_fut, redis_fut, rest_api_fut);

    Ok(())
}
