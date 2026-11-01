//! `fake-claude`: Claude Code's stream-json wire, recorded from the real
//! `claude` and replayed, for the bridge's tests of its `claude` runtime
//! (tests/claude.rs). Test-only: no image carries it.
//!
//! It reads what to do from `fake-claude.json` beside its own executable
//! (each test copies it into a directory of its own, so tests in parallel
//! never share one):
//!
//! - `{"mode": "record", "real": <claude>, "out": <dir>}`: runs the real
//!   `claude` with its own arguments, isolated from its user's settings
//!   and MCP servers (`--setting-sources "" --strict-mcp-config`) and with
//!   no auto memory (a test's directory is inside this repo, whose memory
//!   is its user's), passes every line both ways unchanged, and writes them
//!   in the order they came to `<dir>/<n>.jsonl` (`n` the next free), as
//!   `{"args"}`, then `{"in"}` and `{"out"}` lines, then `{"exit"}`,
//!   scrubbed of the machine (`scrub`). A rate-limit line (the account's
//!   plan) is passed on and never written.
//! - `{"mode": "replay", "script": <dir>}`: plays `<dir>/<n>.jsonl`, the
//!   next one no process played yet, back: each `in` waits for the next
//!   line of its input, which must be of the same kind (else it exits 3,
//!   saying what it expected); each `out` is written; `sleep_ms` (a
//!   composed script's, never recorded) pauses; `exit` exits. A
//!   session is as Claude Code keeps one: `--session-id` of one whose
//!   transcript is there, and `--resume` of one that is not, fail as the
//!   real one does, and its first message makes its transcript.
//!
//! In either mode each line it is given is appended to `heard.jsonl` beside
//! it, as it came: what a test reads of what Claude Code was told.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // as the real one says it, which an install reads (FIN-258)
    if args == ["--version"] {
        println!("2.1.316 (Claude Code)");
        return;
    }
    // through its launcher's link, as an install lays it out (macOS's
    // current_exe names the link itself)
    let exe = std::env::current_exe().and_then(std::fs::canonicalize).expect("its own path");
    let dir = exe.parent().expect("a directory").to_path_buf();
    let cfg: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("fake-claude.json")).expect("fake-claude.json beside it")).expect("JSON");
    match cfg["mode"].as_str() {
        Some("record") => record(&args, Path::new(cfg["real"].as_str().expect("real")), Path::new(cfg["out"].as_str().expect("out"))),
        Some("replay") => replay(&args, Path::new(cfg["script"].as_str().expect("script"))),
        Some("echo") => echo(&args),
        other => panic!("fake-claude: no mode {other:?}"),
    }
}

/// A scripted Claude Code (the stub image's: images/stub/Dockerfile): each
/// message answered `echo: <its words>` on Claude Code's own stream-json
/// wire, its session's transcript kept as the real one keeps it (so a
/// later process resumes it), an interrupt answered.
fn echo(args: &[String]) {
    let id = flag(args, "--session-id").or_else(|| flag(args, "--resume")).unwrap_or("echo").to_string();
    let config = PathBuf::from(std::env::var("CLAUDE_CONFIG_DIR").unwrap_or_else(|_| ".claude".into()));
    let cwd = std::env::current_dir().expect("a working directory");
    let project: String = cwd.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let transcript = config.join("projects").join(project).join(format!("{id}.jsonl"));
    let mut stdout = std::io::stdout();
    let mut inited = false;
    // bounded by its input: one line per pass
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { return };
        let got: serde_json::Value = serde_json::from_str(&line).unwrap_or(serde_json::Value::Null);
        let out = match got["type"].as_str() {
            Some("user") => {
                if !transcript.is_file() {
                    let _ = std::fs::create_dir_all(transcript.parent().expect("a project"));
                    let _ = std::fs::write(&transcript, b"");
                }
                let content = &got["message"]["content"];
                let said = content.as_str().map(str::to_string).unwrap_or_else(|| content.as_array().into_iter().flatten().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join(" "));
                let text = format!("echo: {said}");
                let mut out = Vec::new();
                if !inited {
                    inited = true;
                    out.push(serde_json::json!({ "type": "system", "subtype": "init", "session_id": id }));
                }
                out.push(serde_json::json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": text }] }, "parent_tool_use_id": null }));
                out.push(serde_json::json!({ "type": "result", "subtype": "success", "is_error": false, "result": text }));
                out
            }
            Some("control_request") => vec![serde_json::json!({ "type": "control_response", "response": { "subtype": "success", "request_id": got["request_id"], "response": {} } })],
            _ => Vec::new(),
        };
        for v in out {
            writeln!(stdout, "{v}").expect("written");
        }
        stdout.flush().expect("flushed");
    }
}

/// A line it was given, appended to `heard.jsonl` beside it.
fn heard(line: &str) {
    let path = std::env::current_exe().expect("its own path").with_file_name("heard.jsonl");
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).expect("heard.jsonl");
    writeln!(f, "{line}").expect("written");
}

/// The next `<n>.jsonl` in `dir` no process has taken (made empty, so the
/// next process takes the one after).
fn next_file(dir: &Path, taken: &str) -> (usize, PathBuf) {
    std::fs::create_dir_all(dir).expect("its directory");
    // bounded by the recordings a test makes: a handful
    for n in 0.. {
        let marker = dir.join(format!("{n}.{taken}"));
        if std::fs::OpenOptions::new().write(true).create_new(true).open(&marker).is_ok() {
            return (n, dir.join(format!("{n}.jsonl")));
        }
    }
    unreachable!()
}

fn record(args: &[String], real: &Path, out: &Path) {
    // recording spends its user's own plan: Haiku 5.5 alone (Paul, 2026-10-30)
    assert_eq!(flag(args, "--model"), Some("claude-haiku-5-5"), "fake-claude records only with --model claude-haiku-5-5");
    let (_, path) = next_file(out, "recorded");
    let log = Arc::new(Mutex::new(std::fs::File::create(&path).expect("its recording")));
    let put = |log: &Arc<Mutex<std::fs::File>>, v: serde_json::Value| {
        let mut f = log.lock().expect("log");
        writeln!(f, "{v}").expect("written");
    };
    put(&log, serde_json::json!({ "args": args }));
    let mut cmd = Command::new(real);
    // the real claude dies with its recorder, as a computer's processes do
    // all at once (the crash scenario kills the recorder)
    // (Linux's: elsewhere a recording's crash leaves the real one to exit
    // on its closed input.)
    // SAFETY: prctl is async-signal-safe, and touches only this child.
    #[cfg(target_os = "linux")]
    unsafe {
        std::os::unix::process::CommandExt::pre_exec(&mut cmd, || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
    let mut child = cmd
        .args(args)
        .args(["--setting-sources", "", "--strict-mcp-config"])
        .env("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("the real claude starts");
    let mut to = child.stdin.take().expect("piped");
    let from = child.stdout.take().expect("piped");
    let (log_in, log_out) = (log.clone(), log.clone());
    let input = std::thread::spawn(move || {
        // bounded by its input: ends when the bridge closes it
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            heard(&line);
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                put(&log_in, serde_json::json!({ "in": scrub(v) }));
            }
            if writeln!(to, "{line}").and_then(|_| to.flush()).is_err() {
                return;
            }
        }
    });
    let output = std::thread::spawn(move || {
        let mut stdout = std::io::stdout();
        // bounded by the real claude's output
        for line in std::io::BufReader::new(from).lines().map_while(Result::ok) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                if v["type"] != "rate_limit_event" {
                    put(&log_out, serde_json::json!({ "out": scrub(v) }));
                }
            }
            if writeln!(stdout, "{line}").and_then(|_| stdout.flush()).is_err() {
                return;
            }
        }
    });
    let status = child.wait().expect("the real claude ends");
    let _ = output.join();
    put(&log, serde_json::json!({ "exit": status.code().unwrap_or(1) }));
    drop(input);
    std::process::exit(status.code().unwrap_or(1));
}

/// A recorded line, without the machine it was recorded on: its init's
/// directories and sockets dropped; a tool input's streamed pieces emptied
/// (the bridge reads the whole input from its `assistant` line, and a piece
/// can split a path past scrubbing); its working directory (where its tools
/// ran) the image's, `/data/work/<agent>`; its home directory `~`; and its
/// user's name, as a word (a listing's owner), `user`.
fn scrub(mut v: serde_json::Value) -> serde_json::Value {
    if v["type"] == "system" && v["subtype"] == "init" {
        if let Some(o) = v.as_object_mut() {
            for k in ["cwd", "original_cwd", "memory_paths", "messaging_socket_path", "additional_directories"] {
                o.remove(k);
            }
        }
    }
    if v["type"] == "stream_event" && v["event"]["delta"]["type"] == "input_json_delta" {
        v["event"]["delta"]["partial_json"] = serde_json::Value::String(String::new());
    }
    let mut text = v.to_string();
    if let Ok(cwd) = std::env::current_dir() {
        let name = cwd.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        text = text.replace(&cwd.to_string_lossy().to_string(), &format!("/data/work/{name}"));
    }
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() {
        text = text.replace(&home, "~");
    }
    let user = std::path::Path::new(&home).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if user.len() >= 3 {
        text = whole_word(&text, &user, "user");
    }
    serde_json::from_str(&text).unwrap_or(v)
}

/// `text` with each `word` that stands alone (no letter, digit, `_` or `-`
/// either side) as `with`.
fn whole_word(text: &str, word: &str, with: &str) -> String {
    let part = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-');
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    // bounded by `text`: each pass moves past one match
    while let Some(i) = rest.find(word) {
        let (before, after) = (rest[..i].chars().next_back(), rest[i + word.len()..].chars().next());
        out.push_str(&rest[..i]);
        out.push_str(if part(before) || part(after) { word } else { with });
        rest = &rest[i + word.len()..];
    }
    out.push_str(rest);
    out
}

/// The value of `--<flag>` among `args`.
fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

/// What kind of line a wire message is, as the replay matches them.
fn kind(v: &serde_json::Value) -> String {
    match v["type"].as_str().unwrap_or("") {
        "control_request" => format!("control_request {}", v["request"]["subtype"].as_str().unwrap_or("")),
        "control_response" => format!("control_response {}", v["response"]["response"]["behavior"].as_str().unwrap_or(v["response"]["subtype"].as_str().unwrap_or(""))),
        t => t.to_string(),
    }
}

fn replay(args: &[String], script: &Path) {
    let (n, path) = next_file(script, "played");
    let lines: Vec<serde_json::Value> = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("fake-claude: {}: {e}", path.display())).lines().map(|l| serde_json::from_str(l).expect("a recorded line")).collect();
    // a session as Claude Code keeps one, by its transcript's file
    let (id, resume) = match (flag(args, "--session-id"), flag(args, "--resume")) {
        (Some(id), _) => (id.to_string(), false),
        (_, Some(id)) => (id.to_string(), true),
        _ => panic!("fake-claude: neither --session-id nor --resume"),
    };
    let config = PathBuf::from(std::env::var("CLAUDE_CONFIG_DIR").expect("CLAUDE_CONFIG_DIR"));
    let cwd = std::env::current_dir().expect("a working directory");
    let project: String = cwd.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let transcript = config.join("projects").join(project).join(format!("{id}.jsonl"));
    if resume && !transcript.is_file() {
        eprintln!("No conversation found with session ID: {id}");
        println!("{}", serde_json::json!({ "type": "result", "subtype": "error_during_execution", "is_error": true, "session_id": id }));
        std::process::exit(0);
    }
    if !resume && transcript.is_file() {
        eprintln!("Error: Session ID {id} is already in use.");
        std::process::exit(1);
    }
    let recorded_resume = lines.first().and_then(|l| l["args"].as_array()).is_some_and(|a| a.iter().any(|x| x == "--resume"));
    if recorded_resume != resume {
        eprintln!("fake-claude: recording {n} was made {}, this process {}", if recorded_resume { "resuming" } else { "new" }, if resume { "resumes" } else { "is new" });
        std::process::exit(3);
    }
    let stdin = std::io::stdin();
    let mut input = stdin.lock().lines();
    let mut stdout = std::io::stdout();
    for l in &lines {
        if let Some(expected) = l.get("in") {
            let Some(Ok(line)) = input.next() else { std::process::exit(0) };
            heard(&line);
            let got: serde_json::Value = serde_json::from_str(&line).unwrap_or(serde_json::Value::Null);
            if kind(&got) != kind(expected) {
                eprintln!("fake-claude: recording {n} expected `{}`, got `{}`", kind(expected), kind(&got));
                std::process::exit(3);
            }
            if got["type"] == "user" && !transcript.is_file() {
                std::fs::create_dir_all(transcript.parent().expect("a project")).expect("its project");
                std::fs::write(&transcript, b"").expect("its transcript");
            }
        } else if let Some(v) = l.get("out") {
            writeln!(stdout, "{v}").expect("written");
            stdout.flush().expect("flushed");
        } else if let Some(ms) = l.get("sleep_ms").and_then(serde_json::Value::as_u64) {
            // a composed script's pause: Claude Code slow to answer
            std::thread::sleep(std::time::Duration::from_millis(ms));
        } else if let Some(code) = l.get("exit") {
            std::process::exit(i32::try_from(code.as_i64().unwrap_or(0)).unwrap_or(1));
        }
    }
    // as the real one: on until its input closes
    // bounded by its input
    while let Some(Ok(_)) = input.next() {}
}
