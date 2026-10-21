//! Real CLI subprocesses and real parsers/validators; configuration is
//! synthetic. No vendor, browser, credential store or network fake is used.
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Config {
    dir: PathBuf,
    path: PathBuf,
    text: String,
}

impl Config {
    fn new(text: String) -> Self {
        let dir = std::env::temp_dir().join(format!("fragment-preflight-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("config.jsonc");
        fs::write(&path, &text).unwrap();
        Self { dir, path, text }
    }

    fn run(&self) -> Output {
        let out = command().args(["preflight", "--config"]).arg(&self.path).current_dir(&self.dir).output().unwrap();
        assert_eq!(fs::read_to_string(&self.path).unwrap(), self.text, "config is never rewritten");
        assert_eq!(fs::read_dir(&self.dir).unwrap().count(), 1, "no generated files");
        out
    }
}

impl Drop for Config {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.dir).unwrap();
    }
}

fn command() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_xtask"));
    // No account credentials, HOME or executable tools are available.
    cmd.env_clear().env("PATH", "");
    cmd
}

fn config() -> Value {
    json!({
        "account_id": "synthetic-account", "zone": "example.test",
        "host_secret": "synthetic-host-name",
        "codestorage": {"org": "synthetic-org", "private_key": "synthetic-storage-name"},
        "workos": {"client_id": "synthetic-client-name", "api_key": "synthetic-api-name"},
        "dns_token_file": "/does-not-exist/preflight-dns",
        "test_secret_file": "/does-not-exist/preflight-test",
        "providers": [{
            "name": "mail", "kind": "connection", "hosts": ["api.example.test"],
            "placements": [{"header": "authorization", "format": "Bearer {}"}],
            "env": ["MAIL_TOKEN"], "key": "synthetic-oauth-name",
            "connection_oauth": {
                "authorize": "https://login.example.test/authorize?access_type=offline",
                "exchange": "https://login.example.test/token", "client_id": "synthetic-client",
                "scopes": ["mail.read"]
            }
        }]
    })
}

fn refused(value: Value, expected: &str) {
    let file = Config::new(value.to_string());
    let a = file.run();
    let b = file.run();
    assert_eq!(a.status.code(), Some(1));
    assert_eq!(b.status.code(), Some(1));
    assert!(a.stdout.is_empty());
    assert_eq!(a.stderr, b.stderr, "errors are deterministic");
    let error = String::from_utf8(a.stderr).unwrap();
    assert!(error.contains(expected), "expected {expected}: {error}");
    assert!(!error.contains("SYNTHETIC_REDACTION_MARKER"), "input must never be echoed");
}

#[test]
fn valid_jsonc_checks_without_credentials_tools_or_writes() {
    let file = Config::new(format!("// synthetic deployment\n{}", config()));
    let out = file.run();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.stderr.is_empty());
    assert_eq!(String::from_utf8(out.stdout).unwrap(),
        "preflight: OK (offline configuration only)\nproviders: 1 total, 1 browser OAuth configured, 0 intentionally without browser OAuth\nNot checked: secret existence/values, credential files, provider enablement, registered callbacks, grants or refresh. No network calls or writes.\n");
}

#[test]
fn absent_oauth_and_manually_supplied_own_keys_remain_valid() {
    for kind in ["connection", "own"] {
        for absent in [true, false] {
            let mut c = config();
            let p = &mut c["providers"][0];
            p["kind"] = json!(kind);
            p.as_object_mut().unwrap().remove("key");
            if absent { p.as_object_mut().unwrap().remove("connection_oauth"); }
            else { p["connection_oauth"] = Value::Null; }
            let out = Config::new(c.to_string()).run();
            assert_eq!(out.status.code(), Some(0));
            assert!(String::from_utf8(out.stdout).unwrap().contains("1 intentionally without browser OAuth"));
        }
    }
    let mut c = config();
    c.as_object_mut().unwrap().remove("providers");
    assert_eq!(Config::new(c.to_string()).run().status.code(), Some(0));
}

#[test]
fn partial_oauth_and_orphan_or_missing_store_bindings_fail() {
    for field in ["authorize", "exchange", "client_id", "scopes"] {
        let mut c = config();
        c["providers"][0]["connection_oauth"].as_object_mut().unwrap().remove(field);
        refused(c, "providers[0]: invalid provider shape");
    }
    for remove in ["key", "connection_oauth"] {
        let mut c = config();
        c["providers"][0].as_object_mut().unwrap().remove(remove);
        refused(c, "providers[0].key");
    }
    for key in [Value::Null, json!(17), json!(""), json!("/SYNTHETIC_REDACTION_MARKER")] {
        let mut c = config();
        c["providers"][0]["key"] = key;
        refused(c, "providers[0].key");
    }
}

#[test]
fn existing_oauth_checks_reject_bad_endpoints_client_ids_and_scopes() {
    for (field, bad) in [
        ("authorize", json!("http://login.example.test/SYNTHETIC_REDACTION_MARKER")),
        ("authorize", json!("https://SYNTHETIC_REDACTION_MARKER@login.example.test/authorize")),
        ("authorize", json!("https://login.example.test/authorize#SYNTHETIC_REDACTION_MARKER")),
        ("authorize", json!("https://login.example.test/authorize?client_secret=SYNTHETIC_REDACTION_MARKER")),
        ("exchange", json!("https://login.example.test/token?code=SYNTHETIC_REDACTION_MARKER")),
        ("client_id", json!("")), ("client_id", json!("client with spaces")),
        ("scopes", json!([])), ("scopes", json!(["scope with spaces"])),
        ("scopes", json!(vec!["scope"; 33])),
    ] {
        let mut c = config();
        c["providers"][0]["connection_oauth"][field] = bad;
        refused(c, "providers[0].oauth/connection_oauth");
    }
}

#[test]
fn provider_catalog_and_workos_errors_never_echo_input() {
    for (field, bad, expected) in [
        ("name", json!("SYNTHETIC_REDACTION_MARKER"), "providers[0].name"),
        ("hosts", json!(["SYNTHETIC_REDACTION_MARKER"]), "providers[0].hosts"),
        ("env", json!(["bad/SYNTHETIC_REDACTION_MARKER"]), "providers[0].env"),
        ("kind", json!("SYNTHETIC_REDACTION_MARKER"), "providers[0]: invalid provider shape"),
        ("SYNTHETIC_REDACTION_MARKER", json!(true), "providers[0]: invalid provider shape"),
    ] {
        let mut c = config(); c["providers"][0][field] = bad;
        refused(c, expected);
    }
    for workos in [Value::Null, json!({"client_id": "synthetic-name"}), json!({"client_id": "synthetic-name", "api_key": "SYNTHETIC_REDACTION_MARKER/value"})] {
        let mut c = config(); c["workos"] = workos;
        refused(c, "workos");
    }
    let mut c = config(); c["host_secret"] = json!("SYNTHETIC_REDACTION_MARKER/value");
    refused(c, "existing deployment checks failed");
    let mut c = config(); c["SYNTHETIC_REDACTION_MARKER"] = json!(true);
    refused(c, "invalid deployment shape");
    let mut c = config();
    let duplicate = c["providers"][0].clone();
    c["providers"].as_array_mut().unwrap().push(duplicate);
    refused(c, "each provider must have a unique name");
}

#[test]
fn own_oauth_and_operator_keys_use_the_existing_catalog_rules() {
    let mut own = config();
    let p = own["providers"][0].as_object_mut().unwrap();
    p.remove("connection_oauth");
    p.remove("key");
    p.insert("kind".into(), json!("own"));
    p.insert("oauth".into(), json!({
        "authorize": "https://login.example.test/authorize",
        "exchange": "https://login.example.test/token",
        "manage": "https://login.example.test/keys"
    }));
    assert_eq!(Config::new(own.to_string()).run().status.code(), Some(0));
    let mut bad = own.clone();
    bad["providers"][0]["oauth"]["manage"] = json!("SYNTHETIC_REDACTION_MARKER");
    refused(bad, "providers[0].oauth/connection_oauth");
    own["providers"][0]["kind"] = json!("connection");
    refused(own, "providers[0].oauth/connection_oauth");

    let mut operator = config();
    operator["providers"][0].as_object_mut().unwrap().remove("connection_oauth");
    operator["providers"][0]["kind"] = json!("operator");
    operator["providers"][0]["price"] = json!({"micros": 1, "per": 1});
    assert_eq!(Config::new(operator.to_string()).run().status.code(), Some(0));
    operator["providers"][0].as_object_mut().unwrap().remove("key");
    refused(operator, "providers[0].key");
}

#[test]
fn invalid_json_and_cli_arguments_have_stable_safe_failures() {
    let out = Config::new("{ SYNTHETIC_REDACTION_MARKER".into()).run();
    assert_eq!(out.status.code(), Some(1));
    let error = String::from_utf8(out.stderr).unwrap();
    assert!(error.contains("invalid JSONC at line"));
    assert!(!error.contains("SYNTHETIC_REDACTION_MARKER"));
    for args in [vec![], vec!["--config"], vec!["--config", "--help"], vec!["--config", "one", "--config", "two"], vec!["--token", "SYNTHETIC_REDACTION_MARKER"]] {
        let out = command().arg("preflight").args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(1));
        let error = String::from_utf8(out.stderr).unwrap();
        assert!(error.contains("usage:"));
        assert!(!error.contains("SYNTHETIC_REDACTION_MARKER"));
    }
    let out = command().args(["preflight", "--config", "/does-not-exist/SYNTHETIC_REDACTION_MARKER"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let error = String::from_utf8(out.stderr).unwrap();
    assert!(error.contains("cannot read UTF-8 file"));
    assert!(!error.contains("SYNTHETIC_REDACTION_MARKER"));
}
