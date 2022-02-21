use crate::PubSub;
use futures::{pin_mut, select, FutureExt, SinkExt, Stream, StreamExt};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use std::io;
use std::net::Ipv4Addr;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, ToSocketAddrs};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_stream::wrappers::TcpListenerStream;
use tokio_tungstenite::tungstenite::{
    handshake::server::{
        ErrorResponse, Request as HandshakeRequest, Response as HandshakeResponse,
    },
    Message,
};

#[derive(Debug, Deserialize, Serialize)]
struct Claims {
    subs: Vec<String>,
    exp: u64,
}

#[derive(Debug, Deserialize, Serialize)]
struct WsConnectionQuery {
    token: String,
}

#[derive(Debug, Serialize)]
struct NotifyMessage<'a> {
    channel: &'a str,
    data: &'a str,
}

pub struct Server<T> {
    listener: Option<T>,
    jwt_secret: String,
    dispatch: PubSub<String, String>,
    shutdown_signal: watch::Receiver<bool>,
}

impl<T> Server<T> {
    pub fn new(
        listener: T,
        jwt_secret: &str,
        dispatch: PubSub<String, String>,
        shutdown_signal: watch::Receiver<bool>,
    ) -> Self {
        Self {
            listener: Some(listener),
            jwt_secret: jwt_secret.to_string(),
            dispatch,
            shutdown_signal,
        }
    }

    pub fn listener(&self) -> &T {
        self.listener.as_ref().unwrap()
    }
}

impl Server<TcpListenerStream> {
    pub async fn bind<A: ToSocketAddrs>(
        address: A,
        jwt_secret: &str,
        dispatch: PubSub<String, String>,
        shutdown_signal: watch::Receiver<bool>,
    ) -> io::Result<Self> {
        Ok(Self::new(
            TcpListenerStream::new(TcpListener::bind(address).await?),
            jwt_secret,
            dispatch,
            shutdown_signal,
        ))
    }

    pub async fn bind_to_port(
        port: u16,
        jwt_secret: &str,
        dispatch: PubSub<String, String>,
        shutdown_signal: watch::Receiver<bool>,
    ) -> io::Result<Self> {
        Ok(Self::bind(
            (Ipv4Addr::LOCALHOST, port),
            jwt_secret,
            dispatch,
            shutdown_signal,
        )
        .await?)
    }
}

impl<T, S> Server<T>
where
    T: Stream<Item = io::Result<S>>,
    S: AsyncRead + AsyncWrite + Unpin,
{
    pub async fn run(mut self) -> std::io::Result<()> {
        let mut shutdown_rx = self.shutdown_signal.clone();
        let mut listener = None;
        std::mem::swap(&mut self.listener, &mut listener);

        listener
            .unwrap()
            .take_until(async {
                shutdown_rx
                    .changed()
                    .await
                    .map_err(|_| log::error!("shutdown server error"))
                    .ok();
                true
            })
            .for_each_concurrent(None, |stream_result| async {
                match stream_result {
                    Ok(raw_stream) => {
                        self.handle_connection(raw_stream)
                            .await
                            .map_err(|err| log::error!("handle connection error: {}", err))
                            .ok();
                    }
                    Err(err) => {
                        log::error!("handle connection error: {}", err);
                    }
                }
            })
            .await;

        Ok(())
    }

    async fn handle_connection(&self, raw_stream: S) -> Result<(), Box<dyn std::error::Error>> {
        let (upgrade_tx, upgrade_rx) = oneshot::channel();
        let ws_stream = tokio_tungstenite::accept_hdr_async(
            raw_stream,
            |req: &HandshakeRequest, res: HandshakeResponse| {
                log::info!("new connection to {}", req.uri());
                match req.uri().query() {
                    None => Err(ErrorResponse::new(None)),
                    Some(query_str) => match serde_qs::from_str::<WsConnectionQuery>(query_str) {
                        Ok(query) => match jsonwebtoken::decode::<Claims>(
                            query.token.as_str(),
                            &DecodingKey::from_secret(self.jwt_secret.as_bytes()),
                            &Validation::new(Algorithm::HS256),
                        ) {
                            Ok(token_data) => {
                                let _ = upgrade_tx.send(token_data.claims.subs);
                                Ok(res)
                            }
                            Err(_) => Err(ErrorResponse::new(Some("Unauthorized".to_string()))),
                        },
                        Err(_) => Err(ErrorResponse::new(Some("Unauthorized".to_string()))),
                    },
                }
            },
        )
        .await?;

        let channels = upgrade_rx.await?;
        let (tx, mut rx) = mpsc::channel(16);
        let read_subscriptions = channels.iter().map(|channel| {
            let receiver = self.dispatch.subscribe(channel);
            let tx = tx.clone();
            Box::pin(async move {
                receiver
                    .for_each(|msg_result| {
                        let tx = tx.clone();
                        async move {
                            match msg_result {
                                Ok(msg) => {
                                    // TODO: fork tungstenite to support bytes::Bytes and publish data without copying
                                    // See this issue for details: https://github.com/snapview/tungstenite-rs/pull/104
                                    tx.send(Message::Text(msg))
                                        .await
                                        .map_err(|_| log::error!("send error"))
                                        .ok();
                                }
                                Err(err) => log::error!("connection is lagging: {:?}", err),
                            }
                        }
                    })
                    .await
            })
        });
        let forward_publish_fut = async {
            futures::future::select_all(read_subscriptions).await;
        }
        .fuse();

        let (outgoing, incoming) = ws_stream.split();

        let recv_fut = async move {
            incoming.for_each(|_| async {}).await;
        }
        .fuse();
        let send_fut = async move {
            let mut outgoing = outgoing;
            while let Some(msg) = rx.recv().await {
                let _ = outgoing.send(msg).await;
            }
        }
        .fuse();
        let mut shutdown_rx = self.shutdown_signal.clone();
        let shutdown_fut = async move {
            shutdown_rx.changed().await.ok();
        }
        .fuse();

        pin_mut!(recv_fut, send_fut, forward_publish_fut, shutdown_fut);

        select! {
            _ = recv_fut => (),
            _ = send_fut => (),
            _ = forward_publish_fut => (),
            _ = shutdown_fut => (),
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use jsonwebtoken::{EncodingKey, Header};
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio_tungstenite::connect_async;

    #[tokio::test]
    async fn test_shutdown() -> Result<(), Box<dyn std::error::Error>> {
        let dispatch = PubSub::new(10);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let server = Server::bind_to_port(0, "secret", dispatch, shutdown_rx).await?;
        let server_task = tokio::spawn(async move {
            server.run().await.unwrap();
        });
        shutdown_tx.send(true)?;
        server_task.await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_connection() -> Result<(), Box<dyn std::error::Error>> {
        let dispatch = PubSub::new(10);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let server = Server::bind_to_port(0, "secret", dispatch.clone(), shutdown_rx).await?;
        let port = server.listener().as_ref().local_addr()?.port();
        let server_task = tokio::spawn(async move {
            server.run().await.unwrap();
        });

        let token = jsonwebtoken::encode(
            &Header::default(),
            &Claims {
                exp: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
                subs: vec!["channel1".to_string(), "channel2".to_string()],
            },
            &EncodingKey::from_secret("secret".as_ref()),
        )?;

        let query = serde_qs::to_string(&WsConnectionQuery { token })?;
        let url = url::Url::parse(format!("ws://127.0.0.1:{}/pubsub?{}", port, query).as_str())?;
        let (ws_stream, _) = connect_async(url).await?;
        let (_, stream) = ws_stream.split();

        dispatch.publish(&"channel3".to_string(), "message3".to_string());
        dispatch.publish(&"channel1".to_string(), "message1".to_string());
        dispatch.publish(&"channel2".to_string(), "message2".to_string());

        let messages: Vec<String> = stream
            .take(2)
            .map(|msg| msg.unwrap().to_string())
            .collect()
            .await;

        assert_eq!(
            messages,
            vec!["message1".to_string(), "message2".to_string()]
        );

        shutdown_tx.send(true)?;
        server_task.await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_unauthorized_connection() -> Result<(), Box<dyn std::error::Error>> {
        let dispatch = PubSub::new(10);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let server = Server::bind_to_port(0, "secret", dispatch, shutdown_rx).await?;
        let port = server.listener().as_ref().local_addr()?.port();
        let server_task = tokio::spawn(async move {
            server.run().await.unwrap();
        });

        let url = url::Url::parse(format!("ws://127.0.0.1:{}/pubsub", port).as_str())?;
        let connect_result = connect_async(url).await;
        assert!(connect_result.is_err());

        shutdown_tx.send(true)?;
        server_task.await?;
        Ok(())
    }
}
