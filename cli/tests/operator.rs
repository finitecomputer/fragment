//! `fragment operator wake` as an operator runs it: the binary, signed
//! with an operator key's file, against a host that wakes some objects
//! and refuses others. Its stdout is one line in either JSON selector
//! (`--json`, `FRAGMENT_OUTPUT=json`), whatever the objects answered,
//! and it exits nonzero when any refused.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;

use fragment_fakes::http::{Request, Response, Server};
use serde_json::{json, Value};

const READY: &str = "aa";
const REFUSED: &str = "bb";

/// A host whose Fragment objects with ids starting `aa` wake ready, and
/// with `bb` refuse, as one that requires a reset does.
fn host() -> Server {
    Server::start(
        0,
        Arc::new(|r: &Request| {
            let id = serde_json::from_slice::<Value>(&r.body).ok().and_then(|b| b["id"].as_str().map(str::to_string)).unwrap_or_default();
            match (r.method.as_str(), r.path.as_str()) {
                ("POST", "/api/admin/objects/Fragment/wake") if id.starts_with(READY) => Response::json(200, &json!({ "class": "Fragment", "id": id, "ready": true })),
                ("POST", "/api/admin/objects/Fragment/wake") => Response::json(500, &json!({ "error": "schema_refused", "message": format!("Fragment {id}: reset required") })),
                _ => Response::json(404, &json!({ "error": "not_found", "message": "nothing here" })),
            }
        }),
    )
    .expect("start the fake host")
}

/// A directory of its own: an operator key's file, and a file of `ids`.
fn files(tag: &str, ids: &[String]) -> (PathBuf, PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("fragment-wake-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (key, list) = (dir.join("operator-key"), dir.join("ids"));
    std::fs::write(&key, "07".repeat(32)).unwrap();
    std::fs::write(&list, ids.join("\n") + "\n").unwrap();
    (dir, key, list)
}

/// How the run asks for JSON, if it does.
#[derive(Clone, Copy, Debug)]
enum Mode {
    Human,
    Flag,
    Env,
}

fn wake(host: &str, key: &Path, list: &Path, mode: Mode) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_fragment"));
    c.env("HOME", list.parent().unwrap()).env_remove("XDG_CONFIG_HOME").env_remove("FRAGMENT_OPERATOR_KEY_FILE").env_remove("FRAGMENT_OUTPUT");
    c.args(["operator", "wake", "Fragment"]).arg(list).arg("--key-file").arg(key).args(["--host", host]);
    match mode {
        Mode::Human => {}
        Mode::Flag => {
            c.arg("--json");
        }
        Mode::Env => {
            c.env("FRAGMENT_OUTPUT", "json");
        }
    }
    c.output().unwrap()
}

/// Goal: a script reads a wake's outcome from stdout, refused or not
/// (review of #857: a refusal printed the people's summary above the
/// failure envelope, two lines no parser reads). Valid: every object
/// ready, one line `{"ok":true,"data":{class, asked, ready}}`, exit 0.
/// Invalid: all refused, or some, one line `{"ok":false,"error":…}`
/// naming how many and which, exit nonzero; each refusal on stderr. For
/// people, the summary line, and the same exits. Method: the binary,
/// each batch in each mode, against a fake host.
#[test]
fn a_wake_answers_one_line_of_json_whatever_the_objects_answer() {
    let server = host();
    let ready = |n: usize| (0..n).map(|i| format!("{READY}{i:062x}")).collect::<Vec<_>>();
    let refused = |n: usize| (0..n).map(|i| format!("{REFUSED}{i:062x}")).collect::<Vec<_>>();
    let batches = [("ready", ready(2), 0), ("refused", refused(2), 2), ("mixed", [ready(2), refused(1)].concat(), 1)];
    for (tag, ids, refusals) in batches {
        let (dir, key, list) = files(tag, &ids);
        for mode in [Mode::Flag, Mode::Env, Mode::Human] {
            let out = wake(&server.url, &key, &list, mode);
            let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout).to_string(), String::from_utf8_lossy(&out.stderr).to_string());
            let said = format!("{tag} {mode:?}: exit {:?}\nstdout: {stdout}\nstderr: {stderr}", out.status.code());
            assert_eq!(out.status.success(), refusals == 0, "{said}");
            assert_eq!(stdout.lines().count(), 1, "one line on stdout: {said}");
            assert_eq!(stderr.lines().filter(|l| l.starts_with(&format!("Fragment {REFUSED}")) && l.ends_with("reset required")).count(), refusals, "each refusal on stderr: {said}");
            let summary = format!("{} of {} Fragment objects ready", ids.len() - refusals, ids.len());
            match mode {
                Mode::Human => assert_eq!(stdout.trim_end(), summary, "{said}"),
                Mode::Flag | Mode::Env => {
                    let v: Value = serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{e}: {said}"));
                    if refusals == 0 {
                        assert_eq!(v, json!({ "ok": true, "data": { "class": "Fragment", "asked": ids.len(), "ready": ids.len() } }), "{said}");
                    } else {
                        let message = v["error"]["message"].as_str().unwrap_or_default();
                        assert_eq!(v["ok"], false, "{said}");
                        assert!(message.starts_with(&format!("{refusals} of {} Fragment objects did not wake", ids.len())), "{said}");
                        assert!(ids.iter().filter(|id| id.starts_with(REFUSED)).all(|id| message.contains(id.as_str())), "naming which: {said}");
                    }
                }
            }
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
