use parking_lot::lock_api::RwLockUpgradableReadGuard;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;
use tokio::sync::broadcast;

pub struct Inner<C, T> {
    capacity: usize,
    channels: RwLock<HashMap<C, broadcast::Sender<T>>>,
}

impl<C, T> Inner<C, T> {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            channels: RwLock::new(HashMap::new()),
        }
    }
}

impl<C, T> Inner<C, T>
where
    C: ToOwned<Owned = C> + Eq + Hash,
    T: Clone,
{
    pub fn subscribe(&self, channel: &C) -> broadcast::Receiver<T> {
        let channels = self.channels.upgradable_read();
        let sender = channels.get(channel);
        match sender {
            Some(tx) => tx.subscribe(),
            None => {
                let (tx, rx) = broadcast::channel(self.capacity);
                let mut channels = RwLockUpgradableReadGuard::upgrade(channels);
                channels.insert(channel.to_owned(), tx);
                rx
            }
        }
    }

    pub fn publish(&self, channel: &C, msg: T) {
        let channels = self.channels.upgradable_read();
        if let Some(sender) = channels.get(channel) {
            if sender.send(msg).is_err() {
                let mut channels = RwLockUpgradableReadGuard::upgrade(channels);
                channels.remove(channel);
            }
        }
    }
}

#[derive(Clone, Shrinkwrap)]
pub struct PubSub<C, T>(Arc<Inner<C, T>>);

impl<C, T> PubSub<C, T> {
    pub fn new(capacity: usize) -> Self {
        Self(Arc::new(Inner::new(capacity)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_subscribe() -> Result<(), Box<dyn std::error::Error>> {
        let pubsub: PubSub<String, String> = PubSub::new(16);
        let mut rx: broadcast::Receiver<String> = pubsub.subscribe(&"channel1".to_owned());
        pubsub.publish(&"channel1".to_owned(), "hello".to_string());
        let msg = rx.recv().await?;
        assert_eq!(msg, "hello".to_string());
        Ok(())
    }
}
