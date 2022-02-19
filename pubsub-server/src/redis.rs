use crate::PubSub;
use futures::{pin_mut, select, FutureExt, StreamExt};
use tokio::sync::mpsc;

enum Command {
    Subscribe(String),
    Unsubscribe(String),
    Publish(String, String),
}

async fn redis_pubsub(
    redis_addr: &str,
    dispatch: PubSub<String, String>,
) -> redis::RedisResult<()> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let client = redis::Client::open(redis_addr)?;
    let conn = client.get_async_connection().await?;
    let mut redis_pubsub = conn.into_pubsub();
    let subscribe_fut = async {
        let mut channel_added = dispatch.subscribe_channel_added();
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
        let mut messages = redis_pubsub.on_message();
        loop {
            let rx_fut = rx.recv().fuse();
            let next_fut = messages.next().fuse();

            pin_mut!(rx_fut, next_fut);

            let cmd_opt: Option<Command> = select! {
                msg = rx_fut => msg,
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

            if cmd_opt.is_none() {
                continue;
            }

            let cmd = cmd_opt.unwrap();
            match cmd {
                Command::Subscribe(channel) => {
                    redis_pubsub
                        .subscribe(channel)
                        .await
                        .map_err(|err| log::error!("subscribe error: {}", err))
                        .ok();
                }
                Command::Unsubscribe(channel) => {
                    redis_pubsub
                        .unsubscribe(channel)
                        .await
                        .map_err(|err| log::error!("unsubscribe error: {}", err))
                        .ok();
                }
                Command::Publish(channel, payload) => {
                    dispatch.publish(&channel, payload);
                }
            }
        }
    }
    .fuse();

    pin_mut!(subscribe_fut, unsubscribe_fut, commands_fut);

    select! {
        _ = subscribe_fut => (),
        _ = unsubscribe_fut => (),
        _ = commands_fut => (),
    }

    Ok(())
}
