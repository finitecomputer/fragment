//! The platform objects' versioned schemas (fragment_core::migrate; the
//! cell's schema.rs) on the real runtime: a new object of each class
//! lands at its newest step; a replay runs nothing; an object from before
//! versions is adopted when its tables are step 1's (a named variant
//! fixed first), and refused, naming what differs, when they are not; a
//! step that fails rolls back; a step that adds a column keeps the rows;
//! code that does not know an object's version refuses it. Locally, after
//! a crash, each object's start says the same: the adopted answers with
//! nothing run again, the refused answer `schema_refused`.

use anyhow::Result;
use fragment_core::migrate::{self, Class};
use serde_json::{json, Value};

use crate::api::{Api, Reply};
use crate::{Need, Suite};

/// A fragment's schema lever (`/api/test/fragment`; schema.rs
/// `fragment_lever`), with its `shape` or `step` when it takes one.
fn lever(api: &Api, name: &str, op: &str, with: Option<(&str, &str)>) -> Result<Reply> {
    let mut body = json!({ "fragment": name, "op": op });
    if let Some((key, value)) = with {
        body[key] = json!(value);
    }
    api.unsigned("POST", "/api/test/fragment", Some(&body))
}

/// Whether a lever's view holds every step of `class` as it declares them,
/// each taken `how` (step 1 `first`).
fn holds_all(view: &Value, class: &Class, first: &str) -> bool {
    let steps = view["steps"].as_array().cloned().unwrap_or_default();
    view["class"] == class.name
        && view["known"] == class.steps.len()
        && steps.len() == class.steps.len()
        && steps.iter().zip(class.steps).enumerate().all(|(i, (held, step))| {
            let how = if i == 0 { first } else { "ran" };
            held["version"] == i + 1 && held["name"] == step.name && held["sha"] == migrate::sha(step).as_str() && held["how"] == how
        })
}

/// A fragment's members, by principal.
fn members(api: &Api, owner: &fragment_nip98::Keys, name: &str) -> Vec<String> {
    let r = api.signed(owner, "GET", &format!("/api/f/{name}/members"), None);
    let list = r.ok().and_then(|r| r.body["members"].as_array().cloned()).unwrap_or_default();
    list.iter().filter_map(|m| m["principal"].as_str().map(str::to_string)).collect()
}

pub fn migrations(s: &mut Suite, _: &Api) -> Result<()> {
    if !s.section("migrations", &[Need::Levers]) {
        return Ok(());
    }
    let api = s.api();
    let known = migrate::FRAGMENT.steps.len();
    let owner = api.person()?;
    let owner_id = api.identity(&owner)?;
    let member = api.person()?;
    let member_id = api.identity(&member)?;

    // ---- new objects: every step, as declared
    let adopted = s.named(&api, &owner, "schema-adopted")?;
    s.create(&api, &owner, &adopted)?;
    let r = lever(&api, &adopted, "schema", None)?;
    s.ok("a new fragment holds every step of its class's, each as it declares it", r.status == 200 && holds_all(&r.body, &migrate::FRAGMENT, "ran") && r.body["runs"].as_array().is_some_and(|c| c.contains(&json!("code"))), &r);
    let first = r.body.clone();
    let r = api.unsigned("POST", "/api/test/list", Some(&json!({ "identity": owner_id, "op": "schema" })))?;
    s.ok("and so does its owner's list", r.status == 200 && holds_all(&r.body, &migrate::PRINCIPAL, "ran"), &r);
    let r = api.unsigned("POST", "/api/test/ledger", Some(&json!({ "identity": owner_id, "op": "schema" })))?;
    s.ok("and their ledger", r.status == 200 && holds_all(&r.body, &migrate::LEDGER, "ran"), &r);
    if s.hosted() {
        s.skip("and the registry", "a preview's registry is everyone's: its levers are no route");
        s.skip("and their computer", "a preview's computer is its image's: the local lane reads the stub's");
    } else {
        let r = api.unsigned("POST", "/api/test/registry", Some(&json!("schema")))?;
        s.ok("and the registry", r.status == 200 && holds_all(&r.body, &migrate::REGISTRY, "ran"), &r);
        let made = api.signed(&owner, "POST", "/api/computers", Some(&json!({})))?;
        let id = made.body["computer"].as_str().unwrap_or_default().to_string();
        let r = api.unsigned("POST", "/api/test/computer", Some(&json!({ "computer": id, "op": "schema" })))?;
        s.ok("and their computer", made.status == 200 && r.status == 200 && holds_all(&r.body, &migrate::COMPUTER, "ran"), format!("{made} / {r}"));
    }
    let r = lever(&api, &adopted, "schema-rerun", None)?;
    let again = lever(&api, &adopted, "schema", None)?;
    s.ok(
        "its migration run again runs nothing (a replay): its steps as they were",
        r.body["ran"] == format!("nothing: at {known}").as_str() && again.body == first,
        format!("{r} / {again}"),
    );

    // ---- objects from before versions
    api.signed(&owner, "PUT", &format!("/api/f/{adopted}/members/{}", member.pubkey_hex()), Some(&json!({ "role": "viewer" })))?;
    let r = lever(&api, &adopted, "schema-unversion", None)?;
    let rerun = lever(&api, &adopted, "schema-rerun", None)?;
    let view = lever(&api, &adopted, "schema", None)?;
    s.ok(
        "a fragment from before versions whose tables are step 1's is adopted at 1, and the steps after run",
        r.status == 200 && rerun.body["ran"].as_str().is_some_and(|ran| ran.starts_with("adopted at 1 (fixed: [])")) && holds_all(&view.body, &migrate::FRAGMENT, "adopted"),
        format!("{rerun} / {view}"),
    );
    s.ok("and keeps its rows: its members as they were", members(&api, &owner, &adopted) == [owner_id.clone(), member_id.clone()], json!(members(&api, &owner, &adopted)));

    let variant = s.named(&api, &owner, "schema-variant")?;
    s.create(&api, &owner, &variant)?;
    api.signed(&owner, "PUT", &format!("/api/f/{variant}/members/{}", member.pubkey_hex()), Some(&json!({ "role": "editor" })))?;
    let r = lever(&api, &variant, "schema-unversion", Some(("shape", "people-only")))?;
    let planted = r.body["members"].as_array().is_some_and(|c| c.contains(&json!("people_only")));
    let rerun = lever(&api, &variant, "schema-rerun", None)?;
    let after = lever(&api, &variant, "schema", None)?;
    s.ok(
        "one from the days of people-only shares has its members table made step 1's (the variant), then is adopted",
        planted
            && rerun.body["ran"].as_str().is_some_and(|ran| ran.starts_with("adopted at 1 (fixed: [\"members\"])"))
            && after.body["members"].as_array().is_some_and(|c| !c.is_empty() && !c.contains(&json!("people_only")))
            && holds_all(&after.body, &migrate::FRAGMENT, "adopted"),
        format!("{r} / {rerun} / {after}"),
    );
    s.ok("and its members are kept", members(&api, &owner, &variant) == [owner_id.clone(), member_id.clone()], json!(members(&api, &owner, &variant)));

    let reset = s.named(&api, &owner, "schema-reset")?;
    s.create(&api, &owner, &reset)?;
    let r = lever(&api, &reset, "schema-unversion", Some(("shape", "before-code")))?;
    let rerun = lever(&api, &reset, "schema-rerun", None)?;
    let view = lever(&api, &reset, "schema", None)?;
    let refused = rerun.body["refused"].as_str().unwrap_or_default();
    s.ok(
        "one whose tables are not step 1's (from before runs.code, #178) requires a reset: refused, naming its object and table",
        r.status == 200 && refused.starts_with("Fragment ") && refused.contains("reset required") && refused.contains("its table runs is"),
        &rerun,
    );
    s.ok(
        "and nothing of it changed: no step kept, runs as it was",
        view.status == 200 && view.body["steps"] == json!([]) && view.body["runs"].as_array().is_some_and(|c| !c.is_empty() && !c.contains(&json!("code"))),
        &view,
    );

    // ---- steps after the newest: one that fails, one that adds a column
    let stepped = s.named(&api, &owner, "schema-stepped")?;
    s.create(&api, &owner, &stepped)?;
    api.signed(&owner, "PUT", &format!("/api/f/{stepped}/members/{}", member.pubkey_hex()), Some(&json!({ "role": "viewer" })))?;
    let r = lever(&api, &stepped, "schema-try", Some(("step", "fails")))?;
    let view = lever(&api, &stepped, "schema", None)?;
    let refused = r.body["refused"].as_str().unwrap_or_default();
    s.ok(
        "a step that fails rolls every step back: the table it made is gone, the version as it was",
        refused.contains(&format!("step {} (e2e_fails) failed", known + 1)) && refused.contains("e2e_no_such_table") && r.body["probeTable"] == 0 && holds_all(&view.body, &migrate::FRAGMENT, "ran"),
        format!("{r} / {view}"),
    );
    let r = lever(&api, &stepped, "schema-try", Some(("step", "adds")))?;
    let view = lever(&api, &stepped, "schema", None)?;
    let steps = view.body["steps"].as_array().cloned().unwrap_or_default();
    s.ok(
        "a step that adds a column to members migrates it, one version on",
        r.body["ran"] == format!("migrated from {known} to {}", known + 1).as_str() && steps.len() == known + 1 && steps.last().is_some_and(|l| l["name"] == "e2e_probe"),
        format!("{r} / {view}"),
    );
    s.ok("and its rows are kept", members(&api, &owner, &stepped) == [owner_id.clone(), member_id.clone()], json!(members(&api, &owner, &stepped)));
    let r = lever(&api, &stepped, "schema-rerun", None)?;
    s.ok(
        "code that does not know that step refuses it (a rollback's), naming why",
        r.body["refused"].as_str().is_some_and(|m| m.contains(&format!("holds schema version {}, newer than this code's {known}", known + 1))),
        &r,
    );

    // ---- each object's start says the same (locally: a preview's node
    // never crashes, and its objects stay as this start left them)
    let api = if s.hosted() {
        for check in ["after a crash the adopted fragment answers, nothing run again", "and the one at a version its code does not know refuses every call, naming itself and why", "and the one that requires a reset too"] {
            s.skip(check, "the hosted lane crashes no node");
        }
        api
    } else {
        s.crash()?;
        let api = s.start(false)?;
        let r = api.status(&owner, &adopted)?;
        let view = lever(&api, &adopted, "schema", None)?;
        s.ok(
            "after a crash the adopted fragment answers, nothing run again",
            r.status == 200 && holds_all(&view.body, &migrate::FRAGMENT, "adopted") && members(&api, &owner, &adopted).len() == 2,
            format!("{r} / {view}"),
        );
        let r = api.status(&owner, &stepped)?;
        s.ok(
            "and the one at a version its code does not know refuses every call, naming itself and why",
            r.status == 500 && r.body["error"] == "schema_refused" && r.body["message"].as_str().is_some_and(|m| m.starts_with("Fragment ") && m.contains("newer than this code's")),
            &r,
        );
        let r = api.status(&owner, &reset)?;
        s.ok(
            "and the one that requires a reset too",
            r.status == 500 && r.body["error"] == "schema_refused" && r.body["message"].as_str().is_some_and(|m| m.starts_with("Fragment ") && m.contains("reset required")),
            &r,
        );
        api
    };

    // ---- the planted ones end: a delete removes everything, refused or
    // not (locally they refuse by now), and its storage's release makes
    // the name's object current; none is left on a preview to refuse its
    // sweep
    for (name, was) in [(&stepped, "at a version its code does not know"), (&reset, "that requires a reset")] {
        let r = api.signed(&owner, "DELETE", &format!("/api/f/{name}"), None)?;
        let (cleaned, last) = s.ended_cleaned(&api, name);
        let view = lever(&api, name, "schema", None)?;
        s.ok(
            &format!("the fragment {was} is deleted, its cleanup finishes, and its object takes every step again"),
            r.status == 200 && cleaned && holds_all(&view.body, &migrate::FRAGMENT, "ran"),
            format!("{r} / {last} / {view}"),
        );
    }
    let made = s.create(&api, &owner, &reset);
    let r = api.status(&owner, &reset)?;
    s.ok("and the name is made again, current", made.is_ok() && r.status == 200, &r);
    Ok(())
}
