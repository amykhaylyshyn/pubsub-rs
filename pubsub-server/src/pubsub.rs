use futures::Stream;
use parking_lot::lock_api::RwLockUpgradableReadGuard;
use parking_lot::RwLock;
use pin_project::{pin_project, pinned_drop};
use std::collections::HashMap;
use std::hash::Hash;
use std::mem::ManuallyDrop;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::broadcast;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;

pub struct Inner<C, T> {
    buffer_size: usize,
    channels: RwLock<HashMap<C, broadcast::Sender<T>>>,
    channel_added_broadcast: broadcast::Sender<C>,
    channel_removed_broadcast: broadcast::Sender<C>,
}

impl<C, T> Inner<C, T>
where
    C: Clone + Eq + Hash,
    T: 'static + Clone + Send,
{
    fn new(buffer_size: usize) -> Self {
        let (channel_added_broadcast, _) = broadcast::channel(32);
        let (channel_removed_broadcast, _) = broadcast::channel(32);
        Self {
            buffer_size,
            channels: RwLock::new(HashMap::new()),
            channel_added_broadcast,
            channel_removed_broadcast,
        }
    }
}

impl<C, T> Inner<C, T>
where
    C: Clone + Eq + Hash,
    T: Clone + Send,
{
    pub fn subscribe(self: &Arc<Self>, channel: &C) -> Receiver<C, T> {
        let channels = self.channels.upgradable_read();
        let sender = channels.get(channel);
        let inner_rx = match sender {
            Some(tx) => tx.subscribe(),
            None => {
                let (tx, rx) = broadcast::channel(self.buffer_size);
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
                self.unsubscribe(
                    RwLockUpgradableReadGuard::upgrade(channels).deref_mut(),
                    channel,
                );
            }
        }
    }

    pub fn channels(&self) -> Vec<C> {
        let channels = self.channels.read();
        channels.keys().map(|channel| channel.to_owned()).collect()
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
            self.unsubscribe(
                RwLockUpgradableReadGuard::upgrade(channels).deref_mut(),
                channel,
            );
        }
    }

    fn unsubscribe(&self, channels: &mut HashMap<C, broadcast::Sender<T>>, channel: &C) {
        channels.remove(channel);
        self.channel_removed_broadcast.send(channel.clone()).ok();
    }
}

#[pin_project(PinnedDrop, project = ReceiverProj)]
pub struct Receiver<C, T>
where
    C: Clone + Eq + Hash,
    T: 'static + Clone + Send,
{
    parent: Arc<Inner<C, T>>,
    #[pin]
    inner: ManuallyDrop<BroadcastStream<T>>,
    channel: C,
}

#[pinned_drop]
impl<C, T> PinnedDrop for Receiver<C, T>
where
    C: Clone + Eq + Hash,
    T: 'static + Clone + Send,
{
    fn drop(mut self: Pin<&mut Self>) {
        let ReceiverProj {
            channel,
            parent,
            mut inner,
        } = self.as_mut().project();
        unsafe { ManuallyDrop::drop(&mut *inner) }
        parent.notify_unsubscribe(channel);
    }
}

impl<C, T> Receiver<C, T>
where
    C: Clone + Eq + Hash,
    T: 'static + Clone + Send,
{
    fn new(parent: Arc<Inner<C, T>>, inner: broadcast::Receiver<T>, channel: C) -> Self {
        Self {
            parent,
            inner: ManuallyDrop::new(BroadcastStream::new(inner)),
            channel,
        }
    }
}

impl<C, T> Stream for Receiver<C, T>
where
    C: Clone + Eq + Hash,
    T: 'static + Clone + Send,
{
    type Item = Result<T, BroadcastStreamRecvError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        unsafe {
            self.project()
                .inner
                .map_unchecked_mut(|x| x.deref_mut())
                .poll_next(cx)
        }
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
    pub fn new(buffer_size: usize) -> Self {
        Self(Arc::new(Inner::new(buffer_size)))
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
