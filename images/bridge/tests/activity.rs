//! The authenticated runtime activity sockets: a socket holds only while it
//! says its children run, several hook instances hold one keepalive, and a
//! close or a crashed peer lets only its own go; a tool call's end said on
//! one is a step of its turn, and every frame is answered.
mod support;

use fragment_bridge::runtime::relay::{wire, Relay, RelayConfig};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, tungstenite::client::IntoClientRequest};

use support::fake::{person, Fake};
use support::hermes::Hermes;

const SECRET: &str = "0123456789abcdef0123456789abcdef";

fn start(fake: &Fake, name: &str) -> (std::net::SocketAddr, support::Running) {
    let dir = support::dir(name);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let listen = listener.local_addr().unwrap();
    drop(listener);
    let runtime = Box::new(Relay { config: RelayConfig { listen, gateway_id: "activity-test".into(), secret: SECRET.into(), media_dir: dir.join("media"), media_local: true } });
    (listen, support::start(support::config(&fake.url(), &dir, support::settings()), runtime))
}

async fn dial(listen: std::net::SocketAddr, secret: &str) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let mut req = format!("ws://{listen}/relay/activity").into_client_request().unwrap();
    req.headers_mut().insert("authorization", format!("Bearer {}", wire::token("activity-test", secret, 0)).parse().unwrap());
    connect_async(req).await.unwrap().0
}

/// A frame, and the bridge's answer to it.
async fn said(socket: &mut tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, frame: &str) -> String {
    socket.send(Message::text(frame)).await.unwrap();
    socket.next().await.unwrap().unwrap().into_text().unwrap().to_string()
}

#[tokio::test]
async fn activity_is_authenticated_and_every_working_socket_holds() {
    let fake = Fake::start("127.0.0.1:0", &["juniper"]).await;
    let (listen, bridge) = start(&fake, "activity-sockets");
    fake.until(10_000, "the bridge to start", |w| w.live_sockets() >= 1).await;
    let mut refused = dial(listen, "wrong").await;
    assert!(refused.next().await.unwrap().unwrap().is_close());
    let mut first = dial(listen, SECRET).await;
    assert_eq!(first.next().await.unwrap().unwrap().into_text().unwrap(), "ready");
    // every frame is answered, so no hook waits out its timeout
    assert_eq!(said(&mut first, r#"{"nothing":"known"}"#).await, "refused");
    assert_eq!(said(&mut first, &format!(r#"{{"working":true,"pad":"{}"}}"#, "x".repeat(4096))).await, "refused");
    assert!(fake.with(|w| w.keepalive_log.is_empty()), "neither a socket alone nor an unauthenticated one holds");
    assert_eq!(said(&mut first, r#"{"working":true}"#).await, "working");
    fake.until(10_000, "runtime work holds", |w| w.keepalive_open == 1).await;
    let mut second = dial(listen, SECRET).await;
    assert_eq!(second.next().await.unwrap().unwrap().into_text().unwrap(), "ready");
    assert_eq!(said(&mut second, r#"{"working":true}"#).await, "working");
    assert_eq!(said(&mut first, r#"{"working":false}"#).await, "working");
    // No gateway chat connection is needed for runtime activity. Give the
    // first socket's word time to reach the engine before checking it.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(fake.with(|w| w.keepalive_log == [true] && w.keepalive_open == 1));
    first.close(None).await.unwrap();
    drop(second); // a gateway process can disappear without a close frame
    fake.until(10_000, "the last runtime activity went away", |w| w.keepalive_log == [true, false] && w.keepalive_open == 0).await;
    bridge.stop().await;
}

/// Goal: a tool call's end is a step of its turn, as observed (a failed
/// call is no success); its socket's end invents none and lets go of no
/// other socket's hold.
#[tokio::test]
async fn an_observed_end_is_a_step_and_a_disconnect_invents_none() {
    let fake = Fake::start("127.0.0.1:0", &["juniper"]).await;
    let (listen, bridge) = start(&fake, "structured-disconnect");
    let hermes = Hermes::spawn(listen, "activity-test", SECRET);
    let chat = fake.chat("talk", &["juniper"]);
    fake.until(10_000, "chat followed", |w| w.live_sockets() >= 2).await;
    let mut children = dial(listen, SECRET).await;
    children.next().await.unwrap().unwrap();
    assert_eq!(said(&mut children, r#"{"working":true}"#).await, "working");
    let mut observer = dial(listen, SECRET).await;
    observer.next().await.unwrap().unwrap();
    let asked = fake.say(&chat, &person("paul"), json!({ "text": "use a tool slow without observations" }));
    let turn = fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", asked["seq"].as_u64().unwrap());
    // a call ends in a turn Hermes holds, so only once the turn reached it
    // (its `turn.start` is posted before the relay hands it on)
    support::until(10_000, "the turn reached Hermes", || hermes.with(|s| !s.heard.is_empty())).await;
    let end = json!({ "turn": turn, "tool": "terminal", "args": "make", "excerpt": "Exit code: 7", "tool_call_id": "call-failed", "status": "failed", "duration_ms": 2 });
    assert_eq!(said(&mut observer, &end.to_string()).await, "observed");
    drop(observer);
    fake.until(10_000, "turn ended", |w| !w.bodies(&chat, "work", "turn.end").is_empty()).await;
    let steps = fake.with(|w| w.bodies(&chat, "work", "turn.step"));
    assert_eq!(steps, vec![json!({ "kind": "turn.step", "turn": turn, "step": 1, "tool": "terminal", "category": "shell", "args": "make", "ok": false, "excerpt": "Exit code: 7", "tool_call_id": "call-failed", "status": "failed", "duration_ms": 2 })], "its progress text and its socket's end are no steps");
    assert!(fake.with(|w| w.keepalive_open == 1), "the child socket still holds after parent and observer end");
    children.close(None).await.unwrap();
    fake.until(10_000, "children ended", |w| w.keepalive_open == 0).await;
    drop(hermes);
    bridge.stop().await;
}
