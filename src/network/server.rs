use crate::config::SharedConfig;
use crate::{models::message::Message, network::message};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::{
    net::TcpListener,
    sync::{mpsc, oneshot},
};
use tokio_websockets::{MaybeTlsStream, Message as WsMessage, ServerBuilder, WebSocketStream};

pub struct WebSocketServer {
    config: SharedConfig,
    user_tx: mpsc::Sender<Message>,
    server_ready_tx: Option<oneshot::Sender<()>>,
}

impl WebSocketServer {
    pub fn new(
        config: SharedConfig,
        user_tx: mpsc::Sender<Message>,
        server_ready_tx: oneshot::Sender<()>,
    ) -> Self {
        Self {
            config,
            user_tx,
            server_ready_tx: Some(server_ready_tx),
        }
    }

    pub async fn run(&mut self) {
        let (addr, valid_token) = (
            self.config.server_addr().to_string(),
            self.config.token().to_string(),
        );
        let addr = addr.as_str();
        let valid_token = valid_token.to_string();

        let listener = match TcpListener::bind(addr).await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("Ошибка привязки адреса {}: {}", addr, e);
                return;
            }
        };
        if let Some(tx) = self.server_ready_tx.take() {
            let _ = tx.send(());
        }

        while let Ok((stream, peer_addr)) = listener.accept().await {
            // println!("Новое подключение от {}", peer_addr);
            let user_tx = self.user_tx.clone();
            let valid_token = valid_token.clone();

            tokio::spawn(async move {
                let ws_stream: WebSocketStream<MaybeTlsStream<TcpStream>> =
                    match ServerBuilder::new()
                        .accept(MaybeTlsStream::Plain(stream))
                        .await
                    {
                        Ok((_request, ws_stream)) => ws_stream,
                        Err(e) => {
                            eprintln!("Ошибка установления WebSocket: {}", e);
                            return;
                        }
                    };
                let (mut sink, mut stream): (SplitSink<_, WsMessage>, SplitStream<_>) =
                    ws_stream.split();

                let auth_message = match stream.next().await {
                    Some(Ok(msg)) => msg,
                    _ => {
                        eprintln!("Клиент {} не отправил токен", peer_addr);
                        return;
                    }
                };

                let message: Message = match auth_message
                    .as_text()
                    .and_then(|text| serde_json::from_str(text).ok())
                {
                    Some(msg) => msg,
                    None => {
                        eprintln!("Некорректное сообщение авторизации от {}", peer_addr);
                        return;
                    }
                };

                if message.token != valid_token {
                    eprintln!("Неверный токен от {}: {}", peer_addr, message.token);
                    let _ = sink.send(WsMessage::text("Неверный токен")).await;
                    return;
                }
                // println!("Клиент {} авторизован", peer_addr);

                tokio::spawn(async move { message::receive_messages(stream, sink, user_tx).await });
            });
        }
    }
}
