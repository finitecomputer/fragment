//! Real CLI processes: current local copies, then a per-source access refusal.
use fragment_core::blob::sha256_hex;
use fragment_fakes::http::{Request, Response, Server};
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

struct Workspace(PathBuf);
impl Workspace {
    fn new(tag: &str, host: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("fragment-collection-{tag}-{}", std::process::id()));
        fs::create_dir_all(root.join("config/fragment")).unwrap();
        fs::write(
            root.join("config/fragment/config.json"),
            json!({"secret_key":"07".repeat(32)}).to_string(),
        )
        .unwrap();
        let w = Self(root);
        for (alias, name) in [("personal", "garden--k3x9"), ("team", "work--k3x9")] {
            fs::create_dir_all(w.0.join(alias).join(".fragment")).unwrap();
            let text = format!("# {alias}\norchard {alias}");
            fs::write(w.0.join(alias).join("page.md"), &text).unwrap();
            fs::write(w.0.join(alias).join(".fragment/state.json"), json!({"schemaVersion":3,"name":name,"host":host,"repo":format!("repo:{name}"),"files":{"page.md":{"sha256":sha256_hex(text.as_bytes()),"size":text.len(),"mtime_ns":0,"commit":"a"}}}).to_string()).unwrap();
        }
        fs::write(w.0.join("collection.json"), json!({"version":1,"host":host,"sources":[{"alias":"personal","fragment":"garden--k3x9","dir":"personal"},{"alias":"team","fragment":"work--k3x9","dir":"team"}]}).to_string()).unwrap();
        w
    }
    fn command(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_fragment"));
        c.args(["--json", "collection", "--manifest"])
            .arg(self.0.join("collection.json"))
            .env("HOME", &self.0)
            .env("XDG_CONFIG_HOME", self.0.join("config"))
            .env_remove("FRAGMENT_AS_AGENT")
            .env_remove("FRAGMENT_FOR")
            .env_remove("FRAGMENT_API")
            .env("FRAGMENT_HOST", "http://127.0.0.1:1");
        c
    }
    fn run(&self, args: &[&str]) -> Value {
        let out = self.command().args(args).output().unwrap();
        assert!(
            out.status.success(),
            "{} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["data"].clone()
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn offline_current_workspace_and_unavailable_sources() {
    let w = Workspace::new("offline", "http://127.0.0.1:1");
    let v = w.run(&["--offline", "search", "orchard"]);
    assert_eq!(v["results"].as_array().unwrap().len(), 2);
    assert_eq!(v["results"][0]["alias"], "personal");
    assert_eq!(v["results"][1]["fragment"], "work--k3x9");
    assert_eq!(v["results"][0]["state"], "synced");
    assert_eq!(v["sources"][0]["access"], "unchecked_offline");
    let limited = w.run(&["--offline", "search", "orchard", "--limit", "1"]);
    assert_eq!(limited["results"].as_array().unwrap().len(), 1);
    assert_eq!(limited["sources"].as_array().unwrap().len(), 2);
    fs::write(
        w.0.join("personal/page.md"),
        "# Edited\norchard currentword",
    )
    .unwrap();
    let v = w.run(&["--offline", "search", "currentword"]);
    assert_eq!(v["results"][0]["state"], "local_only");
    fs::remove_file(w.0.join("personal/page.md")).unwrap();
    assert!(w.run(&["--offline", "search", "currentword"])["results"]
        .as_array()
        .unwrap()
        .is_empty());
    fs::rename(w.0.join("team"), w.0.join("missing")).unwrap();
    let v = w.run(&["--offline", "list"]);
    assert_eq!(v["sources"][1]["local"], "unavailable");
    assert!(v["sources"][1]["reason"]
        .as_str()
        .unwrap()
        .contains("directory unavailable"));
    fs::write(w.0.join("personal/.fragment/state.json"), "broken").unwrap();
    assert_eq!(
        w.run(&["--offline", "list"])["sources"][0]["local"],
        "unavailable"
    );
}

#[test]
fn online_revocation_never_falls_back_to_retained_content() {
    let revoked = Arc::new(AtomicBool::new(false));
    let deny = revoked.clone();
    let server = Server::start(0, Arc::new(move |r: &Request| {
        assert_eq!(r.method, "GET");
        assert!(r.header("authorization").unwrap().starts_with("Nostr "));
        let name = r.path.strip_prefix("/api/f/").unwrap().strip_suffix("/status").unwrap();
        if name == "work--k3x9" && deny.load(Ordering::SeqCst) {
            return Response::json(403, &json!({"error":"forbidden","message":"private response must not leak"}));
        }
        Response::json(200, &json!({"name":name,"npub":"n","owner":"id:owner","role":"viewer","visibility":"members","repo":format!("repo:{name}"),"pins":{"main":"a","live":"a"},"counts":{"files":1,"events":0,"members":2},"code":{"sha":"a","id":"app:x","error":null,"operations":{}},"viewToken":null,"inboxToken":null,"urls":{"canonical":"https://example.test/","platform":"https://example.test"}}))
    })).unwrap();
    let host = format!("http://127.0.0.1:{}", server.port);
    let w = Workspace::new("revoked", &host);
    let v = w.run(&["--host", &host, "search", "orchard"]);
    assert_eq!(v["results"].as_array().unwrap().len(), 2);
    assert_eq!(v["sources"][1]["access"], "checked");
    revoked.store(true, Ordering::SeqCst);
    let v = w.run(&["--host", &host, "search", "orchard"]);
    assert_eq!(v["results"].as_array().unwrap().len(), 1);
    assert_eq!(v["sources"][1]["access"], "denied");
    assert_eq!(v["sources"][1]["local"], "unavailable");
    assert!(!v.to_string().contains("private response"));
    // Local bytes remain; offline makes no claim about current membership.
    assert_eq!(
        w.run(&["--offline", "search", "orchard"])["results"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn invalid_manifests_and_host_mismatch() {
    let w = Workspace::new("invalid", "https://other.test");
    let out = w.command().args(["list"]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("host differs"));
    let path = w.0.join("collection.json");
    let original: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for edit in [
        json!({"version":2}),
        json!({"sources":[]}),
        json!({"extra":true}),
        json!({"sources":[original["sources"][0].clone(),original["sources"][0].clone()]}),
    ] {
        let mut bad = original.clone();
        for (k, v) in edit.as_object().unwrap() {
            bad[k] = v.clone();
        }
        fs::write(&path, bad.to_string()).unwrap();
        let out = w.command().args(["--offline", "list"]).output().unwrap();
        assert_eq!(out.status.code(), Some(2));
    }
}
