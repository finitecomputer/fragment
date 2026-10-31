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

/// A port of the test's own, bound for as long as it is held: each life of
/// its bridge listens on a clone (`relay`), so no other socket takes the
/// port before the first life or between two, as one picked and let go may.
fn bound_port() -> std::net::TcpListener {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap()
}

fn relay(port: &std::net::TcpListener, dir: &std::path::Path) -> Box<Relay> {
    let listen = port.local_addr().unwrap();
    Box::new(Relay { config: RelayConfig { listen, gateway_id: "activity-test".into(), secret: SECRET.into(), media_dir: dir.join("media"), media_local: true }, listener: port.try_clone().unwrap() })
}

fn start(fake: &Fake, name: &str) -> (std::net::SocketAddr, support::Running) {
    let dir = support::dir(name);
    let port = bound_port();
    (port.local_addr().unwrap(), support::start(support::config(&fake.url(), &dir, support::settings()), relay(&port, &dir)))
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

/// Goal (phase 4): a call the model asked for, said on the socket before
/// it runs, is answered `called` and recorded as its turn's `turn.call`,
/// before its end's `turn.step` (the same `tool_call_id`). Invalid: a call
/// with no tool, or an id that is no post id, is refused and recorded
/// nowhere. Replay: said twice, it is one record.
#[tokio::test]
async fn a_call_asked_for_is_recorded_before_its_end() {
    let fake = Fake::start("127.0.0.1:0", &["juniper"]).await;
    let (listen, bridge) = start(&fake, "asked-calls");
    let hermes = Hermes::spawn(listen, "activity-test", SECRET);
    let chat = fake.chat("talk", &["juniper"]);
    fake.until(10_000, "chat followed", |w| w.live_sockets() >= 2).await;
    let mut observer = dial(listen, SECRET).await;
    observer.next().await.unwrap().unwrap();
    let asked = fake.say(&chat, &person("paul"), json!({ "text": "use a tool slow without observations" }));
    let turn = fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", asked["seq"].as_u64().unwrap());
    support::until(10_000, "the turn reached Hermes", || hermes.with(|s| !s.heard.is_empty())).await;
    let call = json!({ "turn": turn, "response": "f00d-1", "call": { "position": 0, "tool_call_id": "call-deploy", "tool": "terminal", "args": "make deploy" } });
    assert_eq!(said(&mut observer, &call.to_string()).await, "called");
    assert_eq!(said(&mut observer, &call.to_string()).await, "called", "said again");
    for bad in [
        json!({ "turn": turn, "response": "f00d-1", "call": { "position": 1, "tool_call_id": "call-x", "tool": "" } }),
        json!({ "turn": turn, "response": "f00d-1", "call": { "position": 1, "tool_call_id": "has space", "tool": "terminal" } }),
        json!({ "turn": turn, "call": { "position": 1, "tool_call_id": "call-x", "tool": "terminal" } }),
    ] {
        assert_eq!(said(&mut observer, &bad.to_string()).await, "refused", "{bad}");
    }
    let end = json!({ "turn": turn, "tool": "terminal", "args": "make deploy", "excerpt": "Exit code: 0", "tool_call_id": "call-deploy", "status": "completed", "duration_ms": 9, "response": "f00d-1" });
    assert_eq!(said(&mut observer, &end.to_string()).await, "observed");
    fake.until(10_000, "turn ended", |w| !w.bodies(&chat, "work", "turn.end").is_empty()).await;
    let work = fake.with(|w| w.records(&chat, "work"));
    let kinds: Vec<&str> = work.iter().filter_map(|r| r["body"]["kind"].as_str()).filter(|k| *k == "turn.call" || *k == "turn.step").collect();
    assert_eq!(kinds, vec!["turn.call", "turn.step"], "one intent, then its end");
    let calls = fake.with(|w| w.bodies(&chat, "work", "turn.call"));
    assert_eq!(calls, vec![json!({ "kind": "turn.call", "turn": turn, "response": "f00d-1", "tool_call_id": "call-deploy", "tool": "terminal", "category": "shell", "args": "make deploy" })]);
    assert_eq!(fake.with(|w| w.bodies(&chat, "work", "turn.step"))[0]["response"], "f00d-1", "its end names its response");
    drop(observer);
    drop(hermes);
    bridge.stop().await;
}

/// Goal (phase 4; Alex on #823): a provider's call id reused by a later
/// response of the same turn (`call_0`, every response) is another call.
/// The first response's call ends; the second's two reuse its ids, one
/// with another preview and one with the same, and a crash cuts them
/// before either ends. Replay: a frame said again is one record. The next
/// life's next turn is told the second's two calls may or may not have
/// run, and the first's as its step alone.
#[tokio::test]
async fn a_reused_call_id_is_another_call_across_a_crash() {
    let fake = Fake::start("127.0.0.1:0", &["juniper"]).await;
    let chat = fake.chat("talk", &["juniper"]);
    let dir = support::dir("asked-reused");
    let port = bound_port();
    let listen = port.local_addr().unwrap();
    let cfg = support::config(&fake.url(), &dir, support::settings());
    let bridge = support::start_killable(cfg.clone(), relay(&port, &dir));
    let hermes = Hermes::spawn(listen, "activity-test", SECRET);
    fake.until(10_000, "chat followed", |w| w.live_sockets() >= 2).await;
    let mut observer = dial(listen, SECRET).await;
    observer.next().await.unwrap().unwrap();
    // a turn that runs until its card is answered, so a crash cuts it
    let asked = fake.say(&chat, &person("paul"), json!({ "text": "ship it, something risky" }));
    let turn = fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", asked["seq"].as_u64().unwrap());
    support::until(10_000, "the turn reached Hermes", || hermes.with(|s| !s.heard.is_empty())).await;
    let call = |response: &str, position: u32, id: &str, args: &str| json!({ "turn": turn, "response": response, "call": { "position": position, "tool_call_id": id, "tool": "terminal", "args": args } });
    assert_eq!(said(&mut observer, &call("f00d-1", 0, "call_0", "ls").to_string()).await, "called");
    let end = json!({ "turn": turn, "tool": "terminal", "args": "ls", "excerpt": "Exit code: 0", "tool_call_id": "call_0", "status": "completed", "duration_ms": 3, "response": "f00d-1" });
    assert_eq!(said(&mut observer, &end.to_string()).await, "observed");
    for frame in [call("f00d-2", 0, "call_0", "make deploy"), call("f00d-2", 1, "call_1", "ls"), call("f00d-2", 0, "call_0", "make deploy")] {
        assert_eq!(said(&mut observer, &frame.to_string()).await, "called");
    }
    fake.until(10_000, "the three intents recorded", |w| w.bodies(&chat, "work", "turn.call").len() >= 3).await;
    // cut while it waits on its card: never continued (its answer is its
    // next turn), so the next message is told what was cut
    fake.until(10_000, "its card", |w| !w.bodies(&chat, "work", "turn.prompt").is_empty()).await;
    bridge.kill().await;
    drop(observer);
    fake.until(10_000, "its sockets closed", |w| w.live_sockets() == 0).await;
    let bridge = support::start_killable(cfg, relay(&port, &dir));
    let woken = hermes.after_boot(listen, "activity-test", SECRET);
    let next = fake.say(&chat, &person("paul"), json!({ "text": "good morning" }));
    let next = fragment_bridge::records::turn_id("juniper--k3x9", &chat, "chat", next["seq"].as_u64().unwrap());
    fake.until(10_000, "the next message's turn to end", |w| w.bodies(&chat, "work", "turn.end").iter().any(|e| e["turn"] == next)).await;
    let calls: Vec<(String, String, String)> = fake.with(|w| w.bodies(&chat, "work", "turn.call")).iter().map(|c| (c["response"].as_str().unwrap().into(), c["tool_call_id"].as_str().unwrap().into(), c["args"].as_str().unwrap().into())).collect();
    let want = [("f00d-1", "call_0", "ls"), ("f00d-2", "call_0", "make deploy"), ("f00d-2", "call_1", "ls")].map(|(r, i, a)| (r.to_string(), i.to_string(), a.to_string()));
    assert_eq!(calls, want, "one record a call, the replay none");
    woken.with(|s| {
        let heard = s.heard.iter().find(|e| e["message_id"] == next).unwrap_or_else(|| panic!("Hermes heard the next message: {:?}", s.heard));
        let note = heard["context"][0]["text"].as_str().unwrap_or_else(|| panic!("the note: {heard}"));
        for said in ["It had started these and never heard back, so each may or may not have run, perhaps partly; check before running any again: terminal make deploy; terminal ls\n", "Its steps, as recorded: terminal ls (ok)"] {
            assert!(note.contains(said), "{said:?} in {note}");
        }
    });
    bridge.stop().await;
}
