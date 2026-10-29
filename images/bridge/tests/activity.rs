//! The authenticated runtime activity sockets: several hook instances
//! hold one keepalive; a close or a crashed peer lets only its own go.
mod support;

use fragment_bridge::runtime::relay::{wire, Relay, RelayConfig};
use futures_util::StreamExt;
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
    fake.until(10_000, "runtime work holds", |w| w.keepalive_open == 1).await;
    let (mut second, _) = connect_async(request(SECRET)).await.unwrap();
    assert_eq!(second.next().await.unwrap().unwrap().into_text().unwrap(), "ready");
    first.close(None).await.unwrap();
    // No gateway chat connection is needed for runtime activity. Give the
    // first socket's close time to reach the engine before checking it.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(fake.with(|w| w.keepalive_log == [true] && w.keepalive_open == 1));
    drop(second); // a gateway process can disappear without a close frame
    fake.until(10_000, "the last runtime activity went away", |w| w.keepalive_log == [true, false] && w.keepalive_open == 0).await;
    bridge.stop().await;
}
