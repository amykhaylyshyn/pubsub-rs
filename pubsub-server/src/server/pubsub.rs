use std::collections::HashMap;
use std::hash::Hash;
use tokio::sync::broadcast;

pub struct PubSub<C, T> {
    capacity: usize,
    channels: HashMap<C, broadcast::Sender<T>>,
}

impl<C, T> PubSub<C, T> where C: ToOwned + Eq + Hash {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            channels: HashMap::new(),
        }
    }

    pub fn subscribe(&mut self, channel: &C) -> broadcast::Receiver<T> {
        match self.channels.get(channel) {
            Some(tx) => tx.subscribe(),
            None => {
                let (tx, rx) = broadcast::channel(self.capacity);
                self.channels.insert(channel.to_owned(), tx);
                rx
            }
        }
    }

    pub fn publish(&mut self, channel: &C, msg: T) {
        if let Some(sender) = self.channels.get(channel) {
            if sender.send(msg).is_err() {
                self.channels.remove(channel);
            }
        }
    }
}