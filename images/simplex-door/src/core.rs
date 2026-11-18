//! SPIKE: the core's bot API (`-p`, loopback), one client: this runner.
//! Answers go to the asker by `corrId`; everything else is an event.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

type Sink = SplitSink<WebSocketStream<TcpStream>, Message>;

pub struct Core {
    sink: tokio::sync::Mutex<Sink>,
    pending: Mutex<HashMap<u64, oneshot::Sender<String>>>,
    next: AtomicU64,
}

/// What the reader hands on that no one asked for.
pub enum Event {
    Frame(String),
    Closed,
}

impl Core {
    pub async fn connect(port: u16, events: mpsc::UnboundedSender<Event>) -> Result<Arc<Core>, String> {
        let tcp = TcpStream::connect(("127.0.0.1", port)).await.map_err(|e| e.to_string())?;
        let _ = tcp.set_nodelay(true);
        let url = format!("ws://127.0.0.1:{port}");
        let (ws, _) = tokio_tungstenite::client_async_with_config(url, tcp, Some(fragment_bridge::net::ws_config())).await.map_err(|e| e.to_string())?;
        let (sink, mut stream) = ws.split();
        let core = Arc::new(Core { sink: tokio::sync::Mutex::new(sink), pending: Mutex::new(HashMap::new()), next: AtomicU64::new(1) });
        let c = core.clone();
        tokio::spawn(async move {
            while let Some(m) = stream.next().await {
                let text = match m {
                    Ok(Message::Text(t)) => t.to_string(),
                    Ok(_) => continue,
                    Err(_) => break,
                };
                let corr = serde_json::from_str::<serde_json::Value>(&text).ok().and_then(|v| v["corrId"].as_str().and_then(|s| s.parse::<u64>().ok()));
                let asker = corr.and_then(|n| c.pending.lock().expect("pending").remove(&n));
                match asker {
                    Some(tx) => {
                        let _ = tx.send(text);
                    }
                    None => {
                        let _ = events.send(Event::Frame(text));
                    }
                }
            }
            let _ = events.send(Event::Closed);
        });
        Ok(core)
    }

    /// One command, its answer's raw text, within `wait`.
    pub async fn ask(&self, cmd: &str, wait: Duration) -> Result<String, String> {
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().expect("pending").insert(n, tx);
        let frame = serde_json::json!({ "corrId": n.to_string(), "cmd": cmd }).to_string();
        let sent = self.sink.lock().await.send(Message::Text(frame.into())).await;
        if let Err(e) = sent {
            self.pending.lock().expect("pending").remove(&n);
            return Err(format!("{cmd}: {e}"));
        }
        match tokio::time::timeout(wait, rx).await {
            Ok(Ok(t)) => Ok(t),
            Ok(Err(_)) => Err(format!("{cmd}: the core closed")),
            Err(_) => {
                self.pending.lock().expect("pending").remove(&n);
                Err(format!("{cmd}: no answer in {wait:?}"))
            }
        }
    }
}
