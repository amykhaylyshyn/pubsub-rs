use crate::PubSub;
use futures::{pin_mut, select, FutureExt, StreamExt};

async fn redis_pubsub(
    redis_addr: &str,
    dispatch: PubSub<String, String>,
) -> redis::RedisResult<()> {
    let client = redis::Client::open(redis_addr)?;
    let conn = client.get_async_connection().await?;
    let mut redis_pubsub = conn.into_pubsub();
    let subscribe_fut = async {
        let mut channel_added = dispatch.subscribe_channel_added();
        while let Ok(channel) = channel_added.recv().await {
            if let Err(err) = redis_pubsub.subscribe(channel).await {
                log::error!("subscribe error: {}", err);
            }
        }
    }
    .fuse();
    let unsubscribe_fut = async {
        let mut channel_removed = dispatch.subscribe_channel_removed();
        while let Ok(channel) = channel_removed.recv().await {
            if let Err(err) = redis_pubsub.unsubscribe(channel).await {
                log::error!("unsubscribe error: {}", err);
            }
        }
    }
    .fuse();
    let publish_fut = async {
        let messages = redis_pubsub.on_message();
        messages
            .for_each_concurrent(None, |msg| async move {
                let channel = msg.get_channel::<String>();
                let payload = msg.get_payload::<String>();
                if channel.is_err() || payload.is_err() {
                    return;
                }
                let channel = channel.unwrap();
                let payload = payload.unwrap();

                log::info!("channel: {}, payload: {}", channel, payload);
            })
            .await;
    }
    .fuse();

    pin_mut!(subscribe_fut, unsubscribe_fut, publish_fut);

    select! {
        _ = subscribe_fut => (),
        _ = unsubscribe_fut => (),
        _ = publish_fut => (),
    }

    Ok(())
}
