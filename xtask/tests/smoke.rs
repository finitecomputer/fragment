//! Real CLI + HTTP transport, mocked platform responses, loopback only.
//! No workerd, Docker, browsers, provider calls, or proxy overrides.
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant};

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xtask")).arg("smoke").args(args).output().unwrap()
}

fn json(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

fn fixture(status: &str, headers: &str, delay: Duration, args: &[&str]) -> Output {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let response = format!("HTTP/1.1 {status}\r\n{headers}Content-Length: 0\r\nConnection: close\r\n\r\n");
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream: TcpStream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
                Err(e) => panic!("loopback fixture did not receive a request: {e}"),
            }
        };
        stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            assert!(request.len() < 8192);
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
        }
        let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
        assert!(request.starts_with("head /healthz http/1.1\r\n"));
        assert!(request.contains("cache-control: no-cache\r\n"));
        assert!(!request.contains("authorization:"));
        assert!(!request.contains("cookie:"));
        thread::sleep(delay);
        // A timed-out client can already have closed its socket.
        let _ = stream.write_all(response.as_bytes());
        listener
    });
    let mut command_args = vec!["--url", &url, "--json"];
    command_args.extend_from_slice(args);
    let output = cli(&command_args);
    let listener = server.join().unwrap();
    assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock), "no redirect or retry request");
    output
}

#[test]
fn smoke_cli_passes_only_the_expected_release() {
    let output = fixture("200 OK", "X-Fragment-Deploy: fixture-v1\r\n", Duration::ZERO, &["--expected-release", "fixture-v1"]);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(json(&output)["code"], "passed");
    let output = fixture("200 OK", "X-Fragment-Deploy: fixture-v1\r\n", Duration::ZERO, &["--expected-release", "fixture-v2"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(json(&output)["code"], "release_mismatch");
}

#[test]
fn smoke_cli_reports_missing_identity_and_does_not_follow_redirects() {
    let output = fixture("200 OK", "", Duration::ZERO, &[]);
    assert!(!output.status.success());
    assert_eq!(json(&output)["code"], "missing_release");
    let output = fixture("302 Found", "Location: /must-not-follow\r\nX-Fragment-Deploy: fixture-v1\r\n", Duration::ZERO, &[]);
    assert!(!output.status.success());
    assert_eq!(json(&output)["code"], "http_error");
    assert_eq!(json(&output)["http_status"], 302);
}

#[test]
fn smoke_cli_bounds_a_stalled_response() {
    let started = Instant::now();
    let output = fixture("200 OK", "X-Fragment-Deploy: fixture-v1\r\n", Duration::from_millis(600), &["--timeout-ms", "300"]);
    assert!(!output.status.success());
    assert_eq!(json(&output)["code"], "request_timeout");
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn smoke_cli_reports_transport_failure_without_raw_errors() {
    let output = fixture("not HTTP", "", Duration::ZERO, &[]);
    assert!(!output.status.success());
    assert_eq!(json(&output)["code"], "request_failed");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("http://"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("http://"));
}

#[test]
fn smoke_cli_invalid_input_is_machine_readable_and_never_echoed() {
    for args in [vec!["--json"], vec!["--json", "--url", "http://localhost/?fixture-private-value"], vec!["--json", "--unexpected-fixture-value"]] {
        let output = cli(&args);
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(json(&output)["passed"], false);
        for bytes in [&output.stdout, &output.stderr] {
            assert!(!String::from_utf8_lossy(bytes).contains("fixture-private-value"));
            assert!(!String::from_utf8_lossy(bytes).contains("unexpected-fixture-value"));
        }
    }
    let output = cli(&["--help"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("not a product journey"));
}
