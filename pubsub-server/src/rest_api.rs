use actix_web::{web, App, HttpServer};
use futures::{pin_mut, select, FutureExt};
use redis::AsyncCommands;
use serde::{Serialize, Deserialize};
use std::io;
use tokio::sync::{mpsc, watch};

struct AppState {
    tx: mpsc::UnboundedSender<PublishRequest>,
}

#[derive(Deserialize, Serialize)]
struct PublishRequest {
    channel: String,
    data: String,
}

async fn publish(
    app_data: web::Data<AppState>,
    request: web::Json<PublishRequest>,
) -> &'static str {
    app_data
        .tx
        .send(request.into_inner())
        .map_err(|_| log::error!("publish error"))
        .ok();
    ""
}

pub async fn run_api(
    listen_address: &str,
    redis_url: &str,
    mut shutdown_signal: watch::Receiver<bool>,
) -> io::Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel();

    let server_fut = async move {
        HttpServer::new(move || {
            App::new()
                .app_data(web::Data::new(AppState { tx: tx.clone() }))
                .service(web::resource("/publish").to(publish))
        })
        .bind(listen_address)?
        .run()
        .await?;
        io::Result::Ok(())
    }
    .fuse();
    let publish_fut = async move {
        let client = redis::Client::open(redis_url)?;
        let mut conn = client.get_async_connection().await?;

        while let Some(msg) = rx.recv().await {
            log::info!("publish to redis: {} {}", msg.channel, msg.data);
            let msg_json = serde_json::to_string(&msg).map_err(|err| log::error!("cannot serialize message: {}", err)).ok();
            if msg_json.is_none() {
                continue
            }

            conn.publish(msg.channel, msg_json.unwrap()).await?;
        }

        redis::RedisResult::Ok(())
    }
    .fuse();
    let exit_fut = shutdown_signal.changed().fuse();

    pin_mut!(server_fut, publish_fut, exit_fut);

    select! {
        _ = server_fut => (),
        _ = publish_fut => (),
        _ = exit_fut => (),
    };

    Ok(())
}
