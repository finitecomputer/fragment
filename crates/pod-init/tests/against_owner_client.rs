//! The owner's HTTP client against a real pod-init service on loopback,
//! with pod-init's fake volume and quoter. The fake quote is "QUOTE" and
//! REPORTDATA, unsigned, so this covers the wire, the binding and the
//! key request, not Intel's chain (crates/pod's tests/real_quote.rs
//! does that, on a quote that carries a plain nonce).

mod common;

use fragment_pod::owner::checks::check_binding;
use fragment_pod::owner::client::{self, Pod};
use fragment_pod::unlock::binding::State;
use fragment_pod::unlock::keys::DiskKey;
use fragment_pod::unlock::request;
use fragment_pod::unlock::seal::Kind;
use fragment_pod::unlock::sign::OwnerKey;
use std::net::TcpListener;

fn report_data(quote: &[u8]) -> [u8; 64] {
    quote[5..].try_into().unwrap()
}

#[test]
fn init_through_the_client_then_the_attested_state_is_open_and_mine() {
    let pod = common::Pod::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    std::thread::scope(|scope| {
        scope.spawn(|| pod.service.serve(&listener, Some(4)));
        let client = Pod::new(&addr).unwrap();
        let before = client.status(&[7; 32]).unwrap();
        assert_eq!(check_binding(&report_data(&before.quote), &before, [7; 32]), Ok(()));
        assert!(check_binding(&report_data(&before.quote), &before, [8; 32]).is_err());
        assert_eq!(before.state, State::Blank);

        let me = OwnerKey::generate().unwrap();
        let req = request::make(Kind::Init, &before.boot_pubkey, &me, &DiskKey::random().unwrap()).unwrap();
        assert_eq!(client.send(Kind::Init, &req).unwrap().state, State::Open);

        let after = client.status(&[9; 32]).unwrap();
        assert_eq!(check_binding(&report_data(&after.quote), &after, [9; 32]), Ok(()));
        assert_eq!((after.state, after.owner, after.boot_pubkey), (State::Open, Some(me.public()), before.boot_pubkey));

        let again = client.send(Kind::Init, &req).unwrap_err();
        assert!(client::is_refusal(&again), "{again:#}");
        assert!(format!("{again:#}").contains("wrong_state"), "{again:#}");
    });
}
