//! `fragment pod` and the key it owns pods with, as a person runs them:
//! the binary, with a HOME of its own. A key that owns a pod is never
//! replaced (`keys rotate`, `login --force` refuse), and `fragment pod`'s
//! refusals keep the CLI's envelope and exit codes. The commands against
//! a pod are cli/src/pod/tests.rs's, in process.
#![cfg(feature = "pod")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

/// A HOME of its own, with a config holding a key; its config folder.
fn home(tag: &str) -> (PathBuf, PathBuf) {
    let home = std::env::temp_dir().join(format!("fragment-pod-cli-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let config = home.join(if cfg!(target_os = "macos") { "Library/Application Support" } else { ".config" }).join("fragment");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("config.json"), format!(r#"{{"secret_key":"{}","host":"http://127.0.0.1:1"}}"#, "05".repeat(32))).unwrap();
    (home, config)
}

fn fragment(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_fragment"))
        .env("HOME", home)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("FRAGMENT_OUTPUT")
        .env_remove("FRAGMENT_HOST")
        .env_remove("FRAGMENT_AS_AGENT")
        .env_remove("FRAGMENT_FOR")
        .args(args)
        .output()
        .expect("run fragment")
}

fn envelope(o: &Output) -> Value {
    let text = String::from_utf8_lossy(&o.stdout);
    assert_eq!(text.lines().count(), 1, "one line: {text}");
    serde_json::from_str(&text).unwrap()
}

/// Valid: with no pod claimed, rotate goes on (to the host, which is not
/// there). Invalid: with one claimed, `keys rotate` and `login --force`
/// refuse before any request, naming it, and the key stays.
#[test]
fn a_key_that_owns_a_pod_is_never_replaced() {
    let (home, config) = home("rotate");
    let before = std::fs::read_to_string(config.join("config.json")).unwrap();
    let free = fragment(&home, &["keys", "rotate", "--json"]);
    assert_eq!(envelope(&free)["error"]["code"], "unavailable", "no pod: it goes on to the host");

    let pod = config.join("pods/pod-a.example.com");
    std::fs::create_dir_all(&pod).unwrap();
    std::fs::write(pod.join("disk.key"), format!("{}\n", "ab".repeat(64))).unwrap();
    for args in [&["keys", "rotate", "--json"][..], &["login", "--force", "--no-wait", "--no-browser", "--json"]] {
        let o = fragment(&home, args);
        assert_eq!(o.status.code(), Some(2), "{args:?}");
        let e = envelope(&o);
        assert_eq!(e["error"]["code"], "invalid_usage", "{e}");
        assert!(e["error"]["message"].as_str().unwrap().contains("owns the pod pod-a.example.com"), "{e}");
    }
    assert_eq!(std::fs::read_to_string(config.join("config.json")).unwrap(), before, "the key stays");
    let _ = std::fs::remove_dir_all(&home);
}

/// A pod with no policy is a usage mistake naming the file to write
/// (exit 2), and one that does not answer is unavailable (exit 1); both
/// in the envelope, and without `--json` on stderr.
#[test]
fn pod_refusals_keep_the_envelope_and_exit_codes() {
    let (home, config) = home("envelope");
    let o = fragment(&home, &["pod", "status", "127.0.0.1:1", "--json"]);
    assert_eq!(o.status.code(), Some(2));
    let e = envelope(&o);
    assert_eq!(e["error"]["code"], "invalid_usage", "{e}");
    assert!(e["error"]["message"].as_str().unwrap().contains(&config.join("pods/127.0.0.1/policy.json").display().to_string()), "{e}");

    let pod = config.join("pods/127.0.0.1");
    std::fs::create_dir_all(&pod).unwrap();
    let policy = format!(r#"{{"releases":[{{"seq":1,"name":"a","mrtd":"{}"}}],"gce_instance":"projects/1/zones/us-central1-b/instances/2"}}"#, "aa".repeat(48));
    std::fs::write(pod.join("policy.json"), policy).unwrap();
    let o = fragment(&home, &["pod", "unlock", "127.0.0.1:1", "--json"]);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(envelope(&o)["error"]["code"], "unavailable");
    let o = fragment(&home, &["pod", "unlock", "127.0.0.1:1"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(o.stdout.is_empty() && String::from_utf8_lossy(&o.stderr).starts_with("error: "), "{o:?}");
    assert!(!pod.join("disk.key").exists());
    let _ = std::fs::remove_dir_all(&home);
}
