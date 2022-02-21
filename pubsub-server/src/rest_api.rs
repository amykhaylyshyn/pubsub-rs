use actix_web::dev::ServiceRequest;
use actix_web::{web, App, HttpServer, ResponseError};
use actix_web_httpauth::extractors::bearer::BearerAuth;
use actix_web_httpauth::middleware::HttpAuthentication;
use futures::{pin_mut, select, FutureExt};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Display, Formatter};
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

fn check_api_key(
    api_key: String,
    req: ServiceRequest,
    credential: BearerAuth,
) -> Result<ServiceRequest, actix_web::Error> {
    if credential.token() != api_key {
        Err(actix_web::Error::from(io::Error::from(
            io::ErrorKind::PermissionDenied,
        )))
    } else {
        Ok(req)
    }
}

pub async fn run_api(
    listen_address: &str,
    redis_url: &str,
    api_key: &str,
    mut shutdown_signal: watch::Receiver<bool>,
) -> io::Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let api_key = api_key.to_owned();

    let server_fut = async move {
        let api_key = api_key.clone();
        HttpServer::new(move || {
            let api_key = api_key.clone();
            let auth = HttpAuthentication::bearer(move |req, credentials| {
                let api_key = api_key.clone();
                async move { check_api_key(api_key, req, credentials) }
            });
            App::new()
                .app_data(web::Data::new(AppState { tx: tx.clone() }))
                .wrap(auth)
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
            let msg_json = serde_json::to_string(&msg)
                .map_err(|err| log::error!("cannot serialize message: {}", err))
                .ok();
            if msg_json.is_none() {
                continue;
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
