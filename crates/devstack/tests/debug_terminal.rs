//! A node started from VS Code's JavaScript Debug Terminal. Its
//! `VSCODE_INSPECTOR_OPTIONS` turns wrangler's inspector off, and with it
//! the only lines that name the runtime's address (front.rs). The node
//! keeps the variable from wrangler, so the node starts, and its front door
//! follows a reload to the runtime's new address.
//!
//! The variable is set for the whole process, so this is a test binary of
//! its own. It boots wrangler dev on the pinned Node, so it runs by name:
//! `cargo test -p fragment-devstack --test debug_terminal -- --ignored`.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::thread;
use std::time::{Duration, Instant};

use fragment_devstack as devstack;

/// How long an edit may take to answer through the front door: wrangler
/// sees the file change, rebuilds and restarts its runtime.
const RELOAD_WAIT: Duration = Duration::from_secs(60);
/// Between asks while the reload is on its way.
const ASK_EVERY: Duration = Duration::from_millis(100);

fn worker(says: &str) -> String {
    format!("export default {{ fetch() {{ return new Response({says:?}); }} }};\n")
}

/// One GET through `port`, read to the connection's close: its status and
/// what followed its head; none when nothing answered.
fn get(port: u16) -> Option<(u16, String)> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(20))).ok()?;
    s.write_all(format!("GET / HTTP/1.1\r\nhost: 127.0.0.1:{port}\r\nconnection: close\r\n\r\n").as_bytes()).ok()?;
    let mut text = String::new();
    s.read_to_string(&mut text).ok()?;
    let status = text.split_whitespace().nth(1)?.parse().ok()?;
    Some((status, text.split_once("\r\n\r\n")?.1.to_string()))
}

fn says(answer: &Option<(u16, String)>, code: &str) -> bool {
    answer.as_ref().is_some_and(|(status, body)| *status == 200 && body.contains(code))
}

#[test]
#[ignore = "boots wrangler dev on the pinned Node: run by name with --ignored"]
fn a_node_starts_and_reloads_from_vs_codes_debug_terminal() {
    // as VS Code's JavaScript Debug Terminal leaves it for every process
    std::env::set_var("VSCODE_INSPECTOR_OPTIONS", "{}");
    let tools = devstack::Tools::locate().expect("the pinned Node and wrangler");
    let project = devstack::repo_root().join(format!("target/debug-terminal/{}", devstack::random_hex(4)));
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("worker.js"), worker("first code")).unwrap();
    std::fs::write(devstack::local_config(&project), r#"{ "name": "debug-terminal", "main": "worker.js", "compatibility_date": "2026-01-01" }"#).unwrap();
    let opts = devstack::NodeOptions { project: project.clone(), listener: devstack::hold_port(0).unwrap(), log_dir: project.clone(), own_group: true };
    let port = opts.port();
    let (node, _) = devstack::Node::start(&tools, &opts).expect("a node starts with VSCODE_INSPECTOR_OPTIONS set");
    let first = get(port);
    assert!(says(&first, "first code"), "the first code answers through the front door: {first:?}");

    std::fs::write(project.join("worker.js"), worker("second code")).unwrap();
    let t0 = Instant::now();
    let mut answer = get(port);
    while !says(&answer, "second code") {
        let log = std::fs::read_to_string(&node.log).unwrap_or_default();
        assert!(t0.elapsed() < RELOAD_WAIT, "the reloaded code did not answer through the front door in {RELOAD_WAIT:?}: {answer:?}\n{log}");
        thread::sleep(ASK_EVERY);
        answer = get(port);
    }
    let log = std::fs::read_to_string(&node.log).unwrap();
    assert!(log.matches("\"type\":\"reloadComplete\"").count() >= 2, "wrangler named its runtime again after the reload:\n{log}");
    node.stop().expect("the node stops");
    std::fs::remove_dir_all(&project).ok();
}
