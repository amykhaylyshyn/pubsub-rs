use crate::PubSub;
use futures::{pin_mut, select, FutureExt, StreamExt};
use tokio::sync::{mpsc, watch};

enum Command {
    Subscribe(String),
    Unsubscribe(String),
    Publish(String, String),
    Quit,
}

pub async fn redis_pubsub(
    redis_url: &str,
    dispatch: PubSub<String, String>,
    mut shutdown_signal: watch::Receiver<bool>,
) -> redis::RedisResult<()> {
    let (tx, mut rx) = mpsc::unbounded_channel();

    let subscribe_fut = async {
        let mut channel_added = dispatch.subscribe_channel_added();
        for channel in dispatch.channels().into_iter() {
            tx.send(Command::Subscribe(channel.to_owned()))
                .map_err(|_| log::error!("send subscribe command error"))
                .ok();
        }

        while let Ok(channel) = channel_added.recv().await {
            tx.send(Command::Subscribe(channel))
                .map_err(|_| log::error!("send subscribe command error"))
                .ok();
        }
    }
    .fuse();
    let unsubscribe_fut = async {
        let mut channel_removed = dispatch.subscribe_channel_removed();
        while let Ok(channel) = channel_removed.recv().await {
            tx.send(Command::Unsubscribe(channel))
                .map_err(|_| log::error!("send unsubscribe command error"))
                .ok();
        }
    }
    .fuse();
    let commands_fut = async {
        log::info!("connecting to {}", redis_url);
        let client = redis::Client::open(redis_url)?;
        let conn = client.get_async_connection().await?;
        let mut pubsub = conn.into_pubsub();
        log::info!("connected to {}", redis_url);

        loop {
            let mut messages = pubsub.on_message();
            let rx_fut = rx.recv().fuse();
            let next_fut = messages.next().fuse();

            pin_mut!(rx_fut, next_fut);

            let cmd_opt: Option<Command> = select! {
                msg = rx_fut => match msg {
                    Some(msg) => Some(msg),
                    None => Some(Command::Quit),
                },
                msg = next_fut => {
                    match msg {
                        Some(msg) => {
                            let channel = msg.get_channel::<String>();
                            let payload = msg.get_payload::<String>();
                            if channel.is_err() || payload.is_err() {
                                None
                            } else {
                                let channel = channel.unwrap();
                                let payload = payload.unwrap();

                                Some(Command::Publish(channel, payload))
                            }
                        },
                        _ => None,
                    }
                }
            };

            drop(messages);

            if cmd_opt.is_none() {
                continue;
            }

            let cmd = cmd_opt.unwrap();
            match cmd {
                Command::Subscribe(channel) => {
                    log::info!("redis subscribe: {}", channel);
                    pubsub
                        .subscribe(channel)
                        .await
                        .map_err(|err| log::error!("subscribe error: {}", err))
                        .ok();
                }
                Command::Unsubscribe(channel) => {
                    log::info!("redis unsubscribe: {}", channel);
                    pubsub
                        .unsubscribe(channel)
                        .await
                        .map_err(|err| log::error!("unsubscribe error: {}", err))
                        .ok();
                }
                Command::Publish(channel, payload) => {
                    log::info!("redis message: {} {}", channel, payload);
                    dispatch.publish(&channel, payload);
                }
                Command::Quit => break,
            }
        }

        redis::RedisResult::Ok(())
    }
    .fuse();
    let exit_fut = shutdown_signal.changed().fuse();

    pin_mut!(subscribe_fut, unsubscribe_fut, commands_fut, exit_fut);

    select! {
        _ = subscribe_fut => (),
        _ = unsubscribe_fut => (),
        _ = commands_fut => (),
        _ = exit_fut => (),
    }

    Ok(())
}
