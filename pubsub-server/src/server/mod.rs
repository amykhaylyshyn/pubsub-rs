mod pubsub;

pub use pubsub::PubSub;

pub struct Config {
    port: u16,
}

pub struct Server {}

impl Server {
    pub fn new(config: Config) -> Self {
        Self {}
    }
}
