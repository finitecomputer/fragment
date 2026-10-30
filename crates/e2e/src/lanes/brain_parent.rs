//! Scripted curator fixtures: exercise the guide using real viewer reads and
//! editor writes. This is test code, not a curator runtime or model test.
use anyhow::{ensure, Result};
use fragment_core::npub;
use fragment_nip98::Keys;
use serde_json::{json, Value};

use crate::api::{Api, Reply};
use crate::Suite;

const SOURCES: &str = "executive/datasets/sources.json";
const INCORPORATED: &str = "executive/datasets/incorporated.json";
const OUTPUT: &str = "executive/wiki/launch.md";
const PAGE: &str = "team/wiki/launch.md";

fn read(api: &Api, keys: &Keys, name: &str, path: &str) -> Result<Reply> {
    api.signed(keys, "GET", &format!("/api/f/{name}/file?path={}", crate::api::url_enc(path)), None)
}

fn write(api: &Api, keys: &Keys, name: &str, files: Value) -> Result<Value> {
    let r = api.signed(keys, "POST", &format!("/api/f/{name}/files"), Some(&json!({"files": files})))?;
    ensure!(r.status == 200, "writing fixture: {r}");
    Ok(r.body["commit"].clone())
}

// No in-memory watermarks: recover inputs and publication from parent files.
fn refresh(api: &Api, curator: &Keys, parent: &str) -> Result<Value> {
    let sources: Vec<Value> = serde_json::from_str(&read(api, curator, parent, SOURCES)?.text)?;
    let mut incorporated = json!({});
    let mut output = "---\nstatus: current\n---\n# Launch\n".to_string();
    for source in sources {
        let child = source["fragment"].as_str().unwrap();
        let path = source["paths"][0].as_str().unwrap();
        let mut verified = None;
        for _ in 0..3 {
            let before = api.status(curator, child)?;
            let page = read(api, curator, child, path)?;
            let after = api.status(curator, child)?;
            if before.status != 200 || page.status != 200 || after.status != 200 {
                // Withdraw claims, not just their label. Never use a cached page.
                return write(api, curator, parent, json!([
                    {"path": OUTPUT, "text": "---\nstatus: withdrawn\n---\n# Launch\nSource unavailable; refresh incomplete.\n"}
                ]));
            }
            let pin = &before.body["pins"]["main"];
            if pin.as_str().is_some_and(|p| !p.is_empty()) && *pin == after.body["pins"]["main"] {
                verified = Some((pin.clone(), page.text));
                break;
            }
        }
        let Some((revision, text)) = verified else {
            return write(api, curator, parent, json!([
                {"path": OUTPUT, "text": "---\nstatus: stale\n---\n# Launch\nSource moved repeatedly; refresh incomplete.\n"}
            ]));
        };
        // Synthetic sources are single facts; interpretation is deliberately fixed.
        output.push_str(&format!("\n{} Source: [{}]({}/#/{path}), main {}.\n", text.trim(), child, api.site_url(child, "").trim_end_matches('/'), revision.as_str().unwrap()));
        incorporated[child] = json!({"revision": revision, "pages": [path], "outputs": [OUTPUT]});
    }
    write(api, curator, parent, json!([
        {"path": OUTPUT, "text": output},
        {"path": INCORPORATED, "text": serde_json::to_string_pretty(&incorporated)?}
    ]))
}

pub(super) fn run(s: &mut Suite, api: &Api, owner: &Keys) -> Result<()> {
    let curator = api.person()?;
    let mut children = Vec::new();
    for (label, fact) in [("research", "November research approved."), ("delivery", "November delivery planned.")] {
        let made = api.create_with(owner, json!({"label": s.name(label), "template": "brain", "visibility": "members"}))?;
        ensure!(made.status == 200, "child: {made}");
        let name = made.body["name"].as_str().unwrap().to_string();
        write(api, owner, &name, json!([{"path": PAGE, "text": fact}]))?;
        ensure!(api.share(&name, &npub::encode(curator.pubkey_hex()), "viewer")?.status == 200, "share child");
        children.push(name);
    }
    let made = api.create_with(&curator, json!({"label": s.name("executive"), "template": "brain", "visibility": "members"}))?;
    ensure!(made.status == 200, "parent: {made}");
    let parent = made.body["name"].as_str().unwrap();
    let sources: Vec<Value> = children.iter().map(|name| json!({"fragment": name, "paths": [PAGE]})).collect();
    write(api, &curator, parent, json!([{"path": SOURCES, "text": serde_json::to_string(&sources)?}]))?;
    let commit = refresh(api, &curator, parent)?;
    let output = read(api, &curator, parent, OUTPUT)?;
    let record: Value = serde_json::from_str(&read(api, &curator, parent, INCORPORATED)?.text)?;
    let mut cited = true;
    for child in &children {
        let pin = api.status(&curator, child)?.body["pins"]["main"].clone();
        cited &= record[child]["revision"] == pin && output.text.contains(pin.as_str().unwrap()) && output.text.contains(child);
        let search = super::search(api, &curator, child, json!({"q": "November"}))?;
        cited &= search.status == 200 && !super::results(&search).is_empty();
    }
    let stranger = api.person()?;
    s.ok("restricted_parent_cites_children", cited && api.status(&curator, parent)?.body["visibility"] == "members" && read(api, &stranger, parent, OUTPUT)?.status == 403, &output);
    s.ok("refresh_does_not_duplicate", refresh(api, &curator, parent)? == commit, "same parent commit on repeat refresh");

    // A committed publication with a lost acknowledgement, then a missed update
    // and deletion while the curator is down. Recover solely from fragment files.
    write(api, owner, &children[0], json!([{"path": PAGE, "text": "December research approved."}]))?;
    write(api, owner, &children[1], json!([{"path": PAGE, "delete": true}]))?;
    s.stop()?;
    let api = &s.start(false)?;
    refresh(api, &curator, parent)?;
    let withdrawn = read(api, &curator, parent, OUTPUT)?;
    write(api, owner, &children[1], json!([{"path": PAGE, "text": "December delivery planned."}]))?;
    let commit = refresh(api, &curator, parent)?;
    let output = read(api, &curator, parent, OUTPUT)?;
    s.ok("refresh_after_restart_reconciles", withdrawn.text.contains("status: withdrawn") && !withdrawn.text.contains("November") && output.text.contains("December") && !output.text.contains("November") && refresh(api, &curator, parent)? == commit, &output);

    let revoked = api.signed(owner, "DELETE", &format!("/api/f/{}/members/{}", children[1], npub::encode(curator.pubkey_hex())), None)?;
    ensure!(revoked.status == 200, "revoke: {revoked}");
    // A newer private fact must never appear in the parent, even with an old
    // incorporated revision and a formerly readable synthesis still present.
    write(api, owner, &children[1], json!([{"path": PAGE, "text": "Classified merger."}]))?;
    refresh(api, &curator, parent)?;
    let output = read(api, &curator, parent, OUTPUT)?;
    let denied = super::search(api, &curator, &children[1], json!({"q": "merger"}))?;
    s.ok("denied_child_not_disclosed", denied.status == 403 && output.text.contains("status: withdrawn") && !output.text.contains("Classified") && !output.text.contains("December"), &output);
    Ok(())
}
