//! The authenticated runtime activity sockets: several hook instances
//! hold one keepalive; a close or a crashed peer lets only its own go.
mod support;

use fragment_bridge::runtime::relay::{wire, Relay, RelayConfig};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, tungstenite::client::IntoClientRequest};

use support::fake::Fake;

const SECRET: &str = "0123456789abcdef0123456789abcdef";

#[tokio::test]
async fn activity_is_authenticated_and_every_live_socket_holds() {
    let fake = Fake::start("127.0.0.1:0", &["juniper"]).await;
    let dir = support::dir("activity-sockets");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let listen = listener.local_addr().unwrap();
    drop(listener);
    let runtime = Box::new(Relay { config: RelayConfig { listen, gateway_id: "activity-test".into(), secret: SECRET.into(), media_dir: dir.join("media"), media_local: true } });
    let bridge = support::start(support::config(&fake.url(), &dir, support::settings()), runtime);
    fake.until(10_000, "the bridge to start", |w| w.live_sockets() >= 1).await;
    let request = |secret| {
        let mut req = format!("ws://{listen}/relay/activity").into_client_request().unwrap();
        req.headers_mut().insert("authorization", format!("Bearer {}", wire::token("activity-test", secret, 0)).parse().unwrap());
        req
    };
    let (mut refused, _) = connect_async(request("wrong")).await.unwrap();
    let frame = refused.next().await.unwrap().unwrap();
    assert!(frame.is_close());
    assert!(fake.with(|w| w.keepalive_log.is_empty()), "an unauthenticated socket holds nothing");
    let (mut first, _) = connect_async(request(SECRET)).await.unwrap();
    assert_eq!(first.next().await.unwrap().unwrap().into_text().unwrap(), "ready");
    first.send(tokio_tungstenite::tungstenite::Message::text(r#"{"working":true}"#)).await.unwrap();
    assert_eq!(first.next().await.unwrap().unwrap().into_text().unwrap(), "working");
    fake.until(10_000, "runtime work holds", |w| w.keepalive_open == 1).await;
    let (mut second, _) = connect_async(request(SECRET)).await.unwrap();
    assert_eq!(second.next().await.unwrap().unwrap().into_text().unwrap(), "ready");
    let mut event_req = format!("ws://{listen}/relay/activity").into_client_request().unwrap();
    event_req.headers_mut().insert("authorization", format!("Bearer {}", wire::token("activity-test", SECRET, 0)).parse().unwrap());
    let (mut observer, _) = connect_async(event_req).await.unwrap();
    assert_eq!(observer.next().await.unwrap().unwrap().into_text().unwrap(), "ready");
    observer.send(tokio_tungstenite::tungstenite::Message::text(r#"{"working":false}"#)).await.unwrap();
    observer.close(None).await.unwrap();
    second.send(tokio_tungstenite::tungstenite::Message::text(r#"{"working":true}"#)).await.unwrap();
    assert_eq!(second.next().await.unwrap().unwrap().into_text().unwrap(), "working");
    first.close(None).await.unwrap();
    // No gateway chat connection is needed for runtime activity. Give the
    // first socket's close time to reach the engine before checking it.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(fake.with(|w| w.keepalive_log == [true] && w.keepalive_open == 1));
    drop(second); // a gateway process can disappear without a close frame
    fake.until(10_000, "the last runtime activity went away", |w| w.keepalive_log == [true, false] && w.keepalive_open == 0).await;
    bridge.stop().await;
}

#[tokio::test]
async fn structured_disconnect_keeps_other_work_and_never_invents_completion() {
    use serde_json::json;
    use support::{fake::person, hermes::Hermes};
    let fake = Fake::start("127.0.0.1:0", &["juniper"]).await;
    let dir = support::dir("structured-disconnect");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let listen = listener.local_addr().unwrap();
    drop(listener);
    let runtime = Box::new(Relay { config: RelayConfig { listen, gateway_id: "activity-test".into(), secret: SECRET.into(), media_dir: dir.join("media"), media_local: true } });
    let bridge = support::start(support::config(&fake.url(), &dir, support::settings()), runtime);
    let hermes = Hermes::spawn(listen, "activity-test", SECRET);
    let chat = fake.chat("talk", &["juniper"]);
    fake.until(10_000, "chat followed", |w| w.live_sockets() >= 2).await;
    let request = |suffix: &str| {
        let mut req = format!("ws://{listen}/relay/activity{suffix}").into_client_request().unwrap();
        req.headers_mut().insert("authorization", format!("Bearer {}", wire::token("activity-test", SECRET, 0)).parse().unwrap());
        req
    };
    let (mut children, _) = connect_async(request("")).await.unwrap();
    children.next().await.unwrap().unwrap();
    children.send(tokio_tungstenite::tungstenite::Message::text(r#"{"working":true}"#)).await.unwrap();
    assert_eq!(children.next().await.unwrap().unwrap().into_text().unwrap(), "working");
    let (mut observer, _) = connect_async(request("")).await.unwrap();
    observer.next().await.unwrap().unwrap();
    let said = fake.say(&chat, &person("paul"), json!({"text":"use a tool slow without observations"}));
    let turn = fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", said["seq"].as_u64().unwrap());
    fake.until(10_000, "turn started", |w| !w.bodies(&chat, "work", "turn.start").is_empty()).await;
    let frame = json!({"turn":turn,"tool":"terminal","excerpt":"Exit code: 7", "evidence":{
        "tool_call_id":"call-failed","status":"failed","duration_ms":2,"error_type":"tool_error",
        "error_message":"Exit code: 7","result_ref":"hermes:session:call-failed"}}).to_string();
    for _ in 0..2 {
        observer.send(tokio_tungstenite::tungstenite::Message::text(frame.clone())).await.unwrap();
        assert_eq!(observer.next().await.unwrap().unwrap().into_text().unwrap(), "observed");
    }
    drop(observer);
    fake.until(10_000, "turn ended", |w| !w.bodies(&chat, "work", "turn.end").is_empty()).await;
    let steps = fake.with(|w| w.bodies(&chat, "work", "turn.step"));
    assert_eq!(steps.len(), 1, "neither duplicate nor progress fabricates another result");
    assert_eq!(steps[0]["ok"], false);
    assert!(fake.with(|w| w.keepalive_open == 1), "the child socket still holds after parent and observer end");
    children.close(None).await.unwrap();
    fake.until(10_000, "children ended", |w| w.keepalive_open == 0).await;
    drop(hermes);
    bridge.stop().await;
}
