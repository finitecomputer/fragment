//! The node's front door (src/front.rs) against wrangler's own proxy, on a
//! Worker of a few lines. A burst of requests, each on a connection of its
//! own, has wrangler's proxy pool a connection to the runtime for each; a
//! second burst, sent as the runtime closes those idle 5 s, finds the proxy
//! writing onto connections that are closing, and the proxy answers 500
//! ("Network connection lost"), which no client asks again. Through the
//! front door each connection is the client's own to the runtime, nothing
//! pooled: no request is lost.
//!
//! The second burst is sent at an instant (its connections opened first):
//! `GAPS_MS` after the first burst's last answer, about the runtime's 5 s.
//! The front door must lose nothing there. What wrangler's proxy loses is
//! printed, not held to: its window is a few milliseconds wide and moves
//! with the machine. A node started from a shell lost 25 to 60 of 192
//! PUTs in each such sweep (2026-10-30), and CI's nodes logged the loss
//! ("Error inside ProxyWorker … Network connection lost", runs 40341674686
//! and 40404853530); one this test starts has lost a few in some runs,
//! none in others.
//!
//! It boots wrangler dev on the pinned Node, so it runs by name:
//! `cargo test -p fragment-devstack --test front_door -- --ignored`.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use fragment_devstack as devstack;

/// Requests in a burst, each on a connection of its own.
const BURST: usize = 32;
/// The second burst's instant after the first burst's last answer, in
/// milliseconds: wrangler's proxy loses requests sent just before the
/// runtime's 5 s, before it has let the closing connections go.
const GAPS_MS: [u64; 9] = [4_980, 4_985, 4_990, 4_995, 5_000, 5_005, 5_010, 5_015, 5_020];

/// A Worker answering every request with a page's worth of body, a moment
/// on (an `async` handler's answer, as a Worker's usually is).
const WORKER: &str = "export default { async fetch(req) { await null; return new Response('x'.repeat(4096)); } };\n";

/// What one request came to.
#[derive(Debug, PartialEq)]
enum Answer {
    Status(u16),
    /// The connection closed with no answer: the client's to ask again.
    Closed,
}

/// One request on a connection of its own, written at `at` (at once when
/// none) once `go` says every request of its burst has its connection:
/// kept alive, as a browser's (a `connection: close` would reach the
/// runtime through wrangler's proxy too, which then pools nothing), and
/// read to its length. Its answer, and when it came.
fn ask(port: u16, method: &str, go: &Barrier, at: Option<Instant>) -> (Answer, Instant) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(20))).expect("a read timeout");
    go.wait();
    if let Some(at) = at {
        thread::sleep(at.saturating_duration_since(Instant::now()));
    }
    let closed = || (Answer::Closed, Instant::now());
    let req = format!("{method} /x HTTP/1.1\r\nhost: 127.0.0.1:{port}\r\naccept-encoding: identity\r\ncontent-length: 1\r\n\r\nx");
    if s.write_all(req.as_bytes()).is_err() {
        return closed();
    }
    let mut r = BufReader::new(s);
    let mut status = String::new();
    if r.read_line(&mut status).unwrap_or(0) == 0 {
        return closed();
    }
    let mut length = 0usize;
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).unwrap_or(0) == 0 {
            return closed();
        }
        if h == "\r\n" {
            break;
        }
        if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
            length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; length];
    if r.read_exact(&mut body).is_err() {
        return closed();
    }
    let answer = status.split_whitespace().nth(1).and_then(|c| c.parse().ok()).map_or(Answer::Closed, Answer::Status);
    (answer, Instant::now())
}

/// A burst of `method` requests through `port`, written at `at`: their
/// answers, and when the last came.
fn burst(port: u16, method: &'static str, at: Option<Instant>) -> (Vec<Answer>, Instant) {
    let answers = Arc::new(Mutex::new(Vec::new()));
    let go = Arc::new(Barrier::new(BURST));
    let threads: Vec<_> = (0..BURST)
        .map(|_| {
            let (answers, go) = (answers.clone(), go.clone());
            thread::spawn(move || {
                let a = ask(port, method, &go, at);
                answers.lock().unwrap().push(a);
            })
        })
        .collect();
    for t in threads {
        t.join().expect("a request's thread");
    }
    let answers = Arc::try_unwrap(answers).unwrap().into_inner().unwrap();
    let last = answers.iter().map(|(_, t)| *t).max().expect("a burst answers");
    (answers.into_iter().map(|(a, _)| a).collect(), last)
}

/// Answers, counted by what they were.
fn said(answers: &[Answer]) -> String {
    let mut counts = BTreeMap::new();
    for a in answers {
        *counts.entry(format!("{a:?}")).or_insert(0) += 1;
    }
    format!("{counts:?}")
}

fn lost(answers: &[Answer]) -> usize {
    answers.iter().filter(|a| **a == Answer::Status(500)).count()
}

#[test]
#[ignore = "boots wrangler dev on the pinned Node: run by name with --ignored"]
fn the_front_door_loses_no_request_wranglers_proxy_does() {
    let tools = devstack::Tools::locate().expect("the pinned Node and wrangler");
    let project = devstack::repo_root().join(format!("target/front-door/{}", devstack::random_hex(4)));
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("worker.js"), WORKER).unwrap();
    std::fs::write(devstack::local_config(&project), r#"{ "name": "front-door", "main": "worker.js", "compatibility_date": "2026-01-01" }"#).unwrap();
    let port = devstack::free_port().unwrap();
    let opts = devstack::NodeOptions { project: project.clone(), port, log_dir: project.clone(), own_group: true };
    let (node, _) = devstack::Node::start(&tools, &opts).expect("a node");
    // wrangler's own port, its proxy's, as it says it is ready
    let text = std::fs::read_to_string(&node.log).unwrap();
    let proxied: u16 = text.split("Ready on http://127.0.0.1:").nth(1).and_then(|t| t.split(|c: char| !c.is_ascii_digit()).next()).and_then(|p| p.parse().ok()).expect("wrangler's port in its log");
    assert_ne!(proxied, port);

    let mut through_proxy = 0;
    for gap in GAPS_MS.map(Duration::from_millis) {
        // each path in turn, so neither's burst lands in the other's gap
        for (through, port) in [("front door", port), ("wrangler's proxy", proxied)] {
            let (warmed, last) = burst(port, "GET", None);
            assert!(warmed.iter().all(|a| *a == Answer::Status(200)), "{through}'s first burst: {}", said(&warmed));
            let (answers, _) = burst(port, "PUT", Some(last + gap));
            println!("{gap:?} on, {through}: {}", said(&answers));
            match through {
                "front door" => assert_eq!(lost(&answers), 0, "the front door lost a PUT: {}", said(&answers)),
                _ => through_proxy += lost(&answers),
            }
        }
    }
    node.stop().expect("the node stops");
    std::fs::remove_dir_all(&project).ok();
    println!("wrangler's proxy lost {through_proxy} PUTs in {} sweeps; the front door none", GAPS_MS.len());
}
