use parking_lot::lock_api::RwLockUpgradableReadGuard;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::hash::Hash;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

pub struct Inner<C, T> {
    capacity: usize,
    channels: RwLock<HashMap<C, broadcast::Sender<T>>>,
    channel_added_broadcast: broadcast::Sender<C>,
    channel_removed_broadcast: broadcast::Sender<C>,
}

impl<C, T> Inner<C, T>
where
    C: Clone + Eq + Hash,
    T: 'static + Clone + Send,
{
    fn new(capacity: usize) -> Self {
        let (channel_added_broadcast, _) = broadcast::channel(32);
        let (channel_removed_broadcast, _) = broadcast::channel(32);
        Self {
            capacity,
            channels: RwLock::new(HashMap::new()),
            channel_added_broadcast,
            channel_removed_broadcast,
        }
    }
}

impl<C, T> Inner<C, T>
where
    C: Clone + Eq + Hash,
    T: 'static + Clone + Send,
{
    pub fn subscribe(self: &Arc<Self>, channel: &C) -> Receiver<C, T> {
        let channels = self.channels.upgradable_read();
        let sender = channels.get(channel);
        let inner_rx = match sender {
            Some(tx) => tx.subscribe(),
            None => {
                let (tx, rx) = broadcast::channel(self.capacity);
                let mut channels = RwLockUpgradableReadGuard::upgrade(channels);
                channels.insert(channel.clone(), tx);
                self.channel_added_broadcast.send(channel.clone()).ok();
                rx
            }
        };
        Receiver::new(self.clone(), inner_rx, channel.clone())
    }

    pub fn publish(self: &Arc<Self>, channel: &C, msg: T) {
        let channels = self.channels.upgradable_read();
        if let Some(sender) = channels.get(channel) {
            if sender.send(msg).is_err() {
                let mut channels = RwLockUpgradableReadGuard::upgrade(channels);
                channels.remove(channel);
                self.channel_removed_broadcast.send(channel.clone()).ok();
            }
        }
    }

    pub fn subscribe_channel_added(&self) -> broadcast::Receiver<C> {
        self.channel_added_broadcast.subscribe()
    }

    pub fn subscribe_channel_removed(&self) -> broadcast::Receiver<C> {
        self.channel_removed_broadcast.subscribe()
    }

    fn notify_unsubscribe(&self, channel: &C) {
        let channels = self.channels.upgradable_read();
        let subscriber_count = channels
            .get(channel)
            .map(|sender| sender.receiver_count())
            .unwrap_or(0);
        if subscriber_count == 0 {
            let mut channels = RwLockUpgradableReadGuard::upgrade(channels);
            channels.remove(channel);
            self.channel_removed_broadcast.send(channel.clone()).ok();
        }
    }
}

pub struct Receiver<C, T>
where
    C: Clone + Eq + Hash,
    T: 'static + Clone + Send,
{
    parent: Arc<Inner<C, T>>,
    inner: BroadcastStream<T>,
    channel: C,
}

impl<C, T> Receiver<C, T>
where
    C: Clone + Eq + Hash,
    T: 'static + Clone + Send,
{
    fn new(parent: Arc<Inner<C, T>>, inner: broadcast::Receiver<T>, channel: C) -> Self {
        Self {
            parent,
            inner: BroadcastStream::new(inner),
            channel,
        }
    }
}

impl<C, T> Deref for Receiver<C, T>
where
    C: Clone + Eq + Hash,
    T: 'static + Clone + Send,
{
    type Target = BroadcastStream<T>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<C, T> DerefMut for Receiver<C, T>
where
    C: Clone + Eq + Hash,
    T: 'static + Clone + Send,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<C, T> Drop for Receiver<C, T>
where
    C: Clone + Eq + Hash,
    T: 'static + Clone + Send,
{
    fn drop(&mut self) {
        self.parent.notify_unsubscribe(&self.channel)
    }
}

#[derive(Clone)]
pub struct PubSub<C, T>(Arc<Inner<C, T>>)
where
    C: Clone + Eq + Hash,
    T: 'static + Clone + Send;

impl<C, T> PubSub<C, T>
where
    C: Clone + Eq + Hash,
    T: 'static + Clone + Send,
{
    pub fn new(capacity: usize) -> Self {
        Self(Arc::new(Inner::new(capacity)))
    }
}

impl<C, T> Deref for PubSub<C, T>
where
    C: Clone + Eq + Hash,
    T: 'static + Clone + Send,
{
    type Target = Arc<Inner<C, T>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<C, T> DerefMut for PubSub<C, T>
where
    C: Clone + Eq + Hash,
    T: 'static + Clone + Send,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast::error::TryRecvError;
    use tokio_stream::StreamExt;

    #[tokio::test]
    async fn test_pubsub() -> Result<(), Box<dyn std::error::Error>> {
        let pubsub: PubSub<String, String> = PubSub::new(16);

        let mut channel_added = pubsub.subscribe_channel_added();
        let mut channel_removed = pubsub.subscribe_channel_removed();

        assert_eq!(channel_added.try_recv(), Err(TryRecvError::Empty));
        assert_eq!(channel_removed.try_recv(), Err(TryRecvError::Empty));

        let mut rx1 = pubsub.subscribe(&"channel1".to_owned());
        assert_eq!(channel_added.try_recv(), Ok("channel1".to_owned()));

        let mut rx2 = pubsub.subscribe(&"channel1".to_owned());

        pubsub.publish(&"channel1".to_owned(), "hello".to_string());
        let msg = rx1.next().await.unwrap()?;
        assert_eq!(msg, "hello".to_string());

        drop(rx1);

        let msg = rx2.next().await.unwrap()?;
        assert_eq!(msg, "hello".to_string());

        assert_eq!(channel_removed.try_recv(), Err(TryRecvError::Empty));
        drop(rx2);
        assert_eq!(channel_removed.try_recv(), Ok("channel1".to_owned()));
        assert_eq!(channel_added.try_recv(), Err(TryRecvError::Empty));

        Ok(())
    }
}
