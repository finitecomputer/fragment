//! The operators' admin (docs/api.md, Operators; decision 59): people and
//! orgs a page at a time, searched by email; a person's page with their
//! ledger and computer; their computer pinned to another image and back
//! (docs/computers.md, "Preview and production"); a computer that won't
//! wake, its newest save too big for its disk, started from an older save
//! (docs/computers.md, "Saves and what a wake restores"); billing's health;
//! a trial mailed; and the log of what operators did. An operator key no person holds reaches it, as a
//! wipe's does; no one else. Its page, `/admin`, is an operator's session's
//! alone: to anyone else it is the 404 of a path that is not there. Valid
//! and invalid.

use std::io::Read;
use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use fragment_nip98::Keys;
use serde_json::{json, Value};

use super::computers::{lever, lever_with, notice, QUEUE_DRAIN};

use crate::api::{Api, Call, Reply};
use crate::{Need, Suite};

pub fn admin(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("admin", &[Need::Deployment, Need::Operator]) {
        return Ok(());
    }
    let op_session = api.sign_in("operator@e2e.test")?;
    let _ = api.approve(&op_session, &s.operator);
    let operator = s.operator.clone();
    let get = |keys: &Keys, path: &str| api.signed(keys, "GET", &format!("/api/admin{path}"), None);

    // ---- people, found by email
    let ivy = api.person()?;
    let ivy_id = api.identity(&ivy)?;
    let ivy_email = Api::email_of(&ivy);
    let r = api.signed(&operator, "POST", "/api/admin/seats", Some(&json!({ "email": ivy_email, "kind": "seat" })))?;
    let org = r.body["org"]["id"].as_str().unwrap_or("").to_string();
    let found = get(&operator, &format!("/people?q={}", ivy_email[..10].to_uppercase()))?;
    let row = found.body["people"].as_array().into_iter().flatten().find(|p| p["npub"] == ivy_id.as_str()).cloned().unwrap_or(Value::Null);
    s.ok(
        "an operator finds a person by the start of their email (any case): their npub, email, org, and comped seat",
        found.status == 200 && row["email"] == ivy_email.as_str() && row["org"]["id"] == org.as_str() && row["seat"]["comped"] == true && row["seat"]["good"] == true && row["admin"] == true,
        &found,
    );
    let none = get(&operator, "/people?q=nobody-signs-in-as-this")?;
    let wild = get(&operator, "/people?q=%25")?;
    s.ok("a search no one matches finds none; its wildcards are only themselves", none.body["people"] == json!([]) && wild.body["people"] == json!([]), json!([none.body, wild.body]));
    let page = get(&operator, "/people")?;
    let listed = page.body["people"].as_array().map(Vec::len).unwrap_or(0);
    s.ok("the list is a page at a time, by npub", page.status == 200 && (1..=100).contains(&listed) && (listed < 100) == page.body["next"].is_null(), format!("{listed} people, next {}", page.body["next"]));
    let theirs = get(&operator, &format!("/people/{ivy_id}"))?;
    s.ok(
        "a person's page: as listed, with their ledger, and no computer",
        theirs.status == 200 && theirs.body["person"] == row && theirs.body["ledger"]["plan"] == "seat" && theirs.body["computer"].is_null() && theirs.body["references"].is_null(),
        &theirs,
    );
    let missing = get(&operator, "/people/npub1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq")?;
    s.ok("no one by that npub is 404", missing.status == 404, &missing);
    glance(s, api, &operator, &ivy, &ivy_id, &ivy_email)?;

    // ---- a person's computer pinned by an operator (docs/computers.md,
    // "Preview and production"): onto another image, and back
    let pin = |keys: &Keys, image: &str| api.signed(keys, "PUT", &format!("/api/admin/people/{ivy_id}/computer/image"), Some(&json!({ "image": image })));
    let none_yet = pin(&operator, "stub")?;
    s.ok("an operator's pin of a person with no computer yet is 404", none_yet.status == 404, &none_yet);
    const PINNED: &str = "an operator pins a person's computer to another image: its view names it, its next start runs another version, asleep it is told nothing, and the person's page gives both images' whole references";
    const PINNED_BACK: &str = "`fragment operator pin` with the operator key's file pins it back, and the log holds the pin";
    let computers = s.runs("admin", &[Need::Computers]);
    // locally, the stub's two builds; hosted, its own image again (a
    // deployment's names are its config's)
    let mut first = String::new();
    if !computers {
        for label in [PINNED, "an image the deployment lacks is refused (400)", PINNED_BACK] {
            s.skip(label, "this deployment makes no computers");
        }
    } else {
        let made = api.signed(&ivy, "POST", "/api/computers", Some(&json!({})))?;
        first = made.body["image"].as_str().unwrap_or("").to_string();
        let other = if s.hosted() { first.clone() } else { "stub-next".to_string() };
        let r = pin(&operator, &other)?;
        let page = get(&operator, &format!("/people/{ivy_id}"))?;
        let next = page.body["references"]["next"].as_str().unwrap_or("").to_string();
        s.ok(
            PINNED,
            made.status == 200
                && r.status == 200
                && r.body["image"] == other.as_str()
                && (s.hosted() || r.body["latest"] != made.body["latest"])
                && r.body["notices"].as_array().is_none_or(|n| n.is_empty())
                && page.body["computer"]["image"] == other.as_str()
                && page.body["references"]["running"].is_null()
                && !next.is_empty()
                && r.body["latest"] == fragment_core::computer::version_of(&next).as_str(),
            json!({ "made": made.body, "pinned": r.body, "references": page.body["references"] }),
        );
        let unknown = pin(&operator, "no-such-image")?;
        s.ok("an image the deployment lacks is refused (400)", unknown.status == 400, &unknown);
    }

    // ---- orgs and billing's health
    let orgs = get(&operator, "/orgs")?;
    let listed = orgs.body["orgs"].as_array().into_iter().flatten().find(|o| o["id"] == org.as_str()).cloned().unwrap_or(Value::Null);
    s.ok("an operator lists orgs: each one's seats, admins and Stripe status", orgs.status == 200 && listed["comped_seats"].is_null() && listed["compedSeats"] == 1 && listed["admins"] == 1 && listed["status"].is_null(), &listed);
    let health = get(&operator, "/health")?;
    s.ok(
        "billing's health: orgs paying and seats, the queues, and nothing failing",
        health.status == 200 && health.body["seatsComped"].as_u64().is_some_and(|n| n >= 1) && health.body["failing"] == json!([]),
        &health,
    );

    // ---- a trial mailed
    let code = api.signed(&operator, "POST", "/api/admin/trials", Some(&json!({ "name": "Admin's", "days": 14, "capacity": 5 })))?;
    let code_id = code.body["id"].as_str().unwrap_or("").to_string();
    let to = format!("admin-trial-{}@example.com", &Keys::generate().pubkey_hex()[..8]);
    let sent = api.signed(&operator, "POST", &format!("/api/admin/trials/{code_id}/send"), Some(&json!({ "email": to })))?;
    if !s.hosted() {
        let mail = s.mail.sent_to(&to);
        s.ok(
            "an operator mails a trial: its days of the always-on seat (the one sold, a code naming no kind), its code, and where to start it, the welcome with its code",
            sent.status == 200
                && code.body["kind"] == "seat_always_on"
                && mail.len() == 1
                && mail[0].text.contains("14 days of a seat with an always-on computer")
                && mail[0].text.contains(code.body["code"].as_str().unwrap_or("?"))
                && mail[0].text.contains(&format!("/welcome?trial={}\n", code.body["code"].as_str().unwrap_or("?"))),
            format!("{sent} {mail:?}"),
        );
    }
    let ended = api.signed(&operator, "PATCH", &format!("/api/admin/trials/{code_id}"), Some(&json!({ "revision": 1, "active": false })))?;
    let not_sent = api.signed(&operator, "POST", &format!("/api/admin/trials/{code_id}/send"), Some(&json!({ "email": to })))?;
    s.ok("an ended code is mailed to no one (400)", ended.status == 200 && not_sent.status == 400, &not_sent);

    // ---- the log
    let grant = api.signed(&operator, "POST", &format!("/api/ledger/{ivy_id}/grant"), Some(&json!({ "id": "admin-log-1", "micros": 1_000_000, "by": api.identity(&operator)?, "why": "a thank-you" })))?;
    let log = get(&operator, "/log")?;
    let actions: Vec<String> = log.body["entries"].as_array().into_iter().flatten().filter_map(|e| e["action"].as_str().map(str::to_string)).collect();
    let target_of = |action: &str| log.body["entries"].as_array().into_iter().flatten().find(|e| e["action"] == action).map(|e| e["target"].clone()).unwrap_or(Value::Null);
    s.ok(
        "the log holds what operators did, newest first: the grant, the trial's end and mailing, its making, the comp",
        grant.status == 200
            && actions.iter().position(|a| a == "ledger-grant") < actions.iter().position(|a| a == "trial-change")
            && ["ledger-grant", "trial-change", "trial-send", "trial-new", "comp"].iter().all(|a| actions.iter().any(|x| x == a))
            && target_of("ledger-grant") == ivy_id.as_str()
            && target_of("trial-new") == code_id.as_str(),
        format!("{actions:?}"),
    );

    // ---- who reaches it
    let wiper = s.wiper.clone().unwrap_or_else(Keys::generate);
    let by_key = get(&wiper, "/health")?;
    s.ok("an operator key no person holds reaches it (no registry asked of it)", by_key.status == 200, &by_key);
    // the CLI, with that key's file
    let dir = s.dir("admin-operator");
    let file = dir.join("operator.key");
    std::fs::write(&file, format!("{}\n", wiper.secret_hex()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600))?;
    }
    let home = s.dir("admin-cli");
    let path = file.to_string_lossy().to_string();
    let listed = s.cli_json(api, &home, &["operator", "people", "--q", &ivy_email, "--key-file", &path, "--json"]);
    s.ok(
        "`fragment operator people --q` with the operator key's file finds them too, their seat saying whether they let its computer sleep",
        listed.as_ref().is_ok_and(|v| v["people"].as_array().is_some_and(|p| p.iter().any(|p| p["npub"] == ivy_id.as_str() && p["seat"]["sleeps"] == false))),
        format!("{listed:?}"),
    );
    let made = s.cli_json(api, &home, &["operator", "trials", "--name", "From the CLI", "--days", "30", "--capacity", "1", "--key-file", &path, "--json"]);
    s.ok(
        "`fragment operator trials --name` makes a code of the always-on seat, the one sold, up to 30 days",
        made.as_ref().is_ok_and(|v| v["kind"] == "seat_always_on" && v["days"] == 30 && v["capacity"] == 1),
        format!("{made:?}"),
    );
    if computers {
        let back = s.cli_json(api, &home, &["operator", "pin", &ivy_id, &first, "--key-file", &path, "--json"]);
        let log = get(&operator, "/log")?;
        let pins: Vec<&Value> = log.body["entries"].as_array().into_iter().flatten().filter(|e| e["action"] == "pin" && e["target"] == ivy_id.as_str()).collect();
        s.ok(
            PINNED_BACK,
            back.as_ref().is_ok_and(|v| v["image"] == first.as_str()) && pins.len() == 2 && pins[0]["detail"] == first.as_str() && pins[0]["operator"] == fragment_core::npub::encode(wiper.pubkey_hex()).as_str(),
            json!({ "cli": format!("{back:?}"), "pins": pins }),
        );
    }
    if s.runs("admin", &[Need::Computers, Need::Levers]) {
        start_from_checks(s, api, &operator, &home, &path)?;
    } else {
        for label in START_FROM_CHECKS {
            s.skip(label, "this deployment makes no computers, or pulls no levers");
        }
    }
    let stranger = api.person()?;
    let refused = [
        get(&stranger, "/people")?,
        get(&stranger, "/health")?,
        api.signed(&stranger, "POST", "/api/admin/trials", Some(&json!({ "name": "x", "kind": "seat", "days": 1, "capacity": 1 })))?,
        pin(&stranger, "stub")?,
    ];
    let unsigned = api.unsigned("GET", "/api/admin/people", None)?;
    s.ok(
        "anyone else is refused (403), unsigned is 401",
        refused.iter().all(|r| r.status == 403) && unsigned.status == 401,
        json!([refused.iter().map(|r| r.status).collect::<Vec<_>>(), unsigned.status]),
    );

    // ---- its page, an operator's session's alone (shell.rs `admin_page`)
    let page = |path: &str, session: Option<&str>| api.call(Call { method: "GET", url: format!("{}{path}", api.base), cookie: session.map(|t| format!("fragment_session={t}")), ..Call::default() });
    let missing = page("/no-such-page", None)?;
    // the router's 404 names the path it has no route for
    let as_missing = |r: &Reply| r.status == 404 && r.header("content-type") == missing.header("content-type") && r.text == missing.text.replace("/no-such-page", "/admin");
    let stranger_session = api.sign_in(&Api::email_of(&stranger))?;
    let not_live = "0".repeat(64);
    let others = [page("/admin", None)?, page("/admin", Some(&stranger_session))?, page("/admin", Some(&not_live))?];
    s.ok(
        "to anyone but an operator, `/admin` is the 404 a path that is not there is, word for word: signed out, signed in, or with a session that is not live",
        missing.status == 404 && missing.text.contains("/no-such-page") && others.iter().all(as_missing),
        format!("{} / {missing}", others.iter().map(Reply::to_string).collect::<Vec<_>>().join(" / ")),
    );
    let theirs = page("/admin", Some(&op_session))?;
    s.ok(
        "an operator's session gets the page, never framed",
        theirs.status == 200
            && theirs.header("content-type").starts_with("text/html")
            && theirs.text.contains("<title>fragment admin</title>")
            && theirs.header("content-security-policy").contains("frame-ancestors 'none'")
            && theirs.header("x-frame-options") == "DENY",
        &theirs,
    );
    Ok(())
}

const START_FROM_CHECKS: [&str; 7] = [
    "an operator's start from a save is refused while its computer runs (409)",
    "a computer whose starts keep failing, not for room (the lever's fail-starts), fails each start, a strike each, and won't wake",
    "an operator starts it from the older save it keeps: asleep, its strikes counted anew, its newer save set aside, and its owner told nothing more of a computer that won't wake",
    "a save it does not keep, or a body naming anything else, is refused (400); anyone else, 403",
    "`fragment operator start-from` with the operator key's file, again, changes nothing (its time set aside kept), and the log holds both, with the save set aside",
    "its owner's export copies the save its next start restores and, beside it, the one set aside, each record as stored",
    "its next wake comes up from the save chosen, a rollback, and its owner is told where it went back to",
];

/// A computer that won't wake, started again from an older save by an
/// operator (`POST /api/admin/people/{npub}/computer/start-from`;
/// docs/computers.md, "Saves and what a wake restores"): the lever's
/// `fail-starts` fails its starts, a strike each, so it ends `wont_wake`
/// (a save too big for its disk falls back by itself: `computer-room`,
/// computers.rs); the choice sets the newer
/// save aside (kept, exported, never restored), and the next wake comes up
/// from the save chosen. Valid, invalid (running, a save not kept, a field
/// it does not know, anyone else), replay (the CLI's same choice).
fn start_from_checks(s: &mut Suite, api: &Api, operator: &Keys, home: &Path, key_file: &str) -> Result<()> {
    const WAKE: Duration = Duration::from_secs(90);
    let una = api.person()?;
    let una_id = api.identity(&una)?;
    let made = api.signed(&una, "POST", "/api/computers", Some(&json!({})))?;
    let id = made.body["computer"].as_str().unwrap_or("").to_string();
    let view = || api.signed(&una, "GET", &format!("/api/computers/{id}"), None).map(|r| r.body).unwrap_or(Value::Null);
    let choose = |keys: &Keys, body: Value| api.signed(keys, "POST", &format!("/api/admin/people/{una_id}/computer/start-from"), Some(&body));
    let wake = || api.signed(&una, "POST", &format!("/api/computers/{id}/wake"), Some(&json!({})));
    let awake = |s: &Suite| s.eventually(WAKE, || view()["phase"] == "awake");
    let asleep = |s: &Suite| {
        std::thread::sleep(QUEUE_DRAIN);
        s.eventually(WAKE, || api.signed(&una, "POST", &format!("/api/computers/{id}/sleep"), Some(&json!({}))).is_ok_and(|r| r.body["phase"] == "asleep"))
    };
    // two lives, each saved as it slept: save 1, and save 2 of the life
    // that came up from it
    let _ = wake();
    let up = made.status == 200 && awake(s);
    let running = choose(operator, json!({ "save": 1 }))?;
    s.ok(START_FROM_CHECKS[0], up && running.status == 409, &running);
    let mut saved = asleep(s);
    let _ = wake();
    saved &= awake(s) && view()["restored"]["save"].is_string() && asleep(s);
    let saves = lever(api, &id, "saves")?.body["saves"].clone();
    let (newest, older) = (saves[0].clone(), saves[1].clone());
    anyhow::ensure!(saved && newest["number"] == 2 && older["number"] == 1, "two saves, 2 of the life that came up from 1: {saves}");
    // its starts keep failing (not for room, which falls back by itself)
    let failing = lever_with(api, &id, json!({ "op": "fail-starts", "times": fragment_core::computer::STARTS_FAILED_MAX }))?;
    let _ = wake();
    let mut gave_up = Value::Null;
    let stuck = s.eventually(Duration::from_secs(180), || {
        gave_up = view();
        gave_up["phase"] == "wont_wake"
    });
    let why = notice(&gave_up, "wont_wake")["why"].as_str().unwrap_or("").to_string();
    s.ok(START_FROM_CHECKS[1], failing.status == 200 && stuck && why.contains("the test lever"), &gave_up);
    // the operator chooses save 1
    let chosen = choose(operator, json!({ "save": 1 }))?;
    let levered = lever(api, &id, "saves")?.body;
    let set_aside = chosen.body["saves"][0]["setAside"].clone();
    s.ok(
        START_FROM_CHECKS[2],
        chosen.status == 200
            && chosen.body["phase"] == "asleep"
            && notice(&chosen.body, "wont_wake").is_null()
            && levered["failures"] == 0
            && chosen.body["saves"][0]["number"] == 2
            && set_aside.is_i64()
            && chosen.body["saves"][1]["number"] == 1
            && chosen.body["saves"][1].get("setAside").is_none(),
        json!({ "chosen": chosen.body, "failures": levered["failures"] }),
    );
    let not_kept = choose(operator, json!({ "save": 9 }))?;
    let unknown = choose(operator, json!({ "save": 1, "image": "stub" }))?;
    let stranger = api.person()?;
    let theirs = choose(&stranger, json!({ "save": 1 }))?;
    s.ok(START_FROM_CHECKS[3], not_kept.status == 400 && unknown.status == 400 && theirs.status == 403, format!("{not_kept} / {unknown} / {theirs}"));
    let again = s.cli_json(api, home, &["operator", "start-from", &una_id, "1", "--key-file", key_file, "--json"]);
    let log = api.signed(operator, "GET", "/api/admin/log", None)?;
    let logged: Vec<&Value> = log.body["entries"].as_array().into_iter().flatten().filter(|e| e["action"] == "start-from" && e["target"] == una_id.as_str()).collect();
    s.ok(
        START_FROM_CHECKS[4],
        again.as_ref().is_ok_and(|v| v["phase"] == "asleep" && v["saves"] == chosen.body["saves"]) && logged.len() == 2 && logged.iter().all(|e| e["detail"] == "save 1; set aside 2"),
        json!({ "cli": format!("{again:?}"), "logged": logged }),
    );
    // its owner's export: the newest save, the one set aside
    let exported = exported_save(s, api, &una, &id, &older, &newest)?;
    s.ok(START_FROM_CHECKS[5], exported["ok"] == true, &exported);
    // its next wake: from save 1, a rollback, told
    let _ = wake();
    let back = awake(s);
    let v = view();
    let told = notice(&v, "went_back");
    s.ok(
        START_FROM_CHECKS[6],
        back && v["restored"]["save"] == older["id"] && v["restored"]["rollback"] == true && told["pending"] == false && told["save"] == 1 && told["savedAt"] == older["atMs"],
        &v,
    );
    let _ = lever_with(api, &id, json!({ "op": "fail-starts", "times": 0 }));
    Ok(())
}

/// `keys`' export, made and read: whether its computer `id`'s save is
/// `save`, and the one set aside beside it `set_aside` (each as the `saves`
/// lever lists it), each of their records as stored (`{ok, export, save,
/// setAside, records}`).
pub(super) fn exported_save(s: &Suite, api: &Api, keys: &Keys, id: &str, save: &Value, set_aside: &Value) -> Result<Value> {
    let pressed = api.signed(keys, "POST", "/api/export", None)?;
    let mut export = Value::Null;
    let built = s.eventually(Duration::from_secs(120), || {
        export = api.signed(keys, "GET", "/api/export", None).map(|r| r.body["export"].clone()).unwrap_or(Value::Null);
        export["state"] != "building"
    });
    let download = api.signed(keys, "GET", &format!("/api/export/{}/download", export["id"].as_str().unwrap_or("")), None)?;
    let mut files = std::collections::BTreeMap::new();
    for entry in tar::Archive::new(download.bytes.as_slice()).entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().to_string();
        let mut bytes = vec![];
        entry.read_to_end(&mut bytes)?;
        files.insert(path, bytes);
    }
    let root = fragment_core::export::ROOT;
    let hex = id.strip_prefix("computer:").unwrap_or("");
    let meta: Value = files.get(&format!("{root}/computers/{hex}/meta.json")).and_then(|b| serde_json::from_slice(b).ok()).unwrap_or(Value::Null);
    let stored = |s: &Value, path: fn(&str, &str) -> String| {
        let records = s["records"].as_array().cloned().unwrap_or_default();
        !records.is_empty() && records.iter().all(|r| files.get(&path(id, r["dir"].as_str().unwrap_or("/data"))).is_some_and(|b| fragment_core::blob::sha256_hex(b) == r["sha256"].as_str().unwrap_or("-")))
    };
    let both = stored(save, fragment_core::export::save_path) && stored(set_aside, fragment_core::export::set_aside_path);
    let ok = pressed.status == 200 && built && export["state"] == "ready" && meta["save"]["id"] == save["id"] && meta["setAside"]["id"] == set_aside["id"] && both;
    Ok(json!({ "ok": ok, "export": export, "save": meta["save"], "setAside": meta["setAside"], "stored": both }))
}

/// The admin's page (`/admin`) in Chrome, with an operator's own session:
/// a person who holds a key the deployment lists (decision 59). It lists
/// people and comps a seat, makes a trial code, shows the orgs, billing's
/// health and the log; to anyone else `/admin` is a 404, and nothing of it.
pub fn admin_page(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("admin-page", &[Need::Chrome, Need::Deployment]) {
        return Ok(());
    }
    let Some(mut b) = s.browser()? else {
        s.ok("Chrome is installed for the admin's page (set CHROME_BIN)", false, "no Chrome found");
        return Ok(());
    };
    let wait = std::time::Duration::from_secs(15);
    // the operator: a person who holds the deployment's operator key
    let session = api.sign_in("operator@e2e.test")?;
    let _ = api.approve(&session, &s.operator);
    b.set_cookie(&format!("{}/", api.base), "fragment_session", &session)?;
    // the glance first, its default tab: a person whose computer the logs
    // say is awake and won't wake (lines planted in the fake, a local run's)
    let glancer = api.person()?;
    let glancer_id = api.identity(&glancer)?;
    let glancer_email = Api::email_of(&glancer);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis() as i64;
    if !s.hosted() {
        let computer = fragment_core::computer::default_computer_of(&glancer_id);
        let script = crate::OBSERVABILITY_SCRIPT;
        s.observability.log(now - 30_000, json!({ "$workers.scriptName": script, "computer": computer, "phase": { "phase": "awake" } }));
        s.observability.log(now - 20_000, json!({ "$workers.scriptName": script, "computer": computer, "phase": { "phase": "failed" } }));
        s.observability.log(now - 10_000, json!({ "$workers.scriptName": script, "computer": computer, "event": { "event": "save_failed", "generation": 1, "seq": 1, "why": "backup '/data': No space left on device (os error 28)" } }));
    }
    let page = b.open(&format!("{}/admin", api.base))?;
    let tiles = b.until(&page, "document.querySelectorAll('#tiles .tile').length === 5 && /Updated/.test(document.getElementById('glance-at').textContent)", wait);
    let shown = b.eval(&page, "[...document.querySelectorAll('#tiles .tile')].map(t => t.querySelector('.label').textContent + ': ' + t.querySelector('.big').textContent)")?;
    let me = b.eval(&page, "document.getElementById('me').textContent")?;
    s.ok("an operator's session opens the admin's page on its glance: five tiles, and says who they are", tiles && me == "operator@e2e.test", &shown);
    // the glance as an operator sees it, for review (`dir` clears it: once)
    let shots = s.dir("admin-page");
    let _ = b.screenshot(&page, &shots.join("glance.png"));
    if !s.hosted() {
        let day = b.eval(&page, "(() => { const d = new Date(); d.setHours(0, 0, 0, 0); return d.getTime(); })()")?;
        let read = api.signed(&s.operator, "GET", &format!("/api/admin/glance?dayStart={day}"), None)?;
        let awake = read.body["logs"]["computersAwakeNow"].as_u64().unwrap_or(0);
        let rows = b.until(
            &page,
            &format!(
                "[...document.querySelectorAll('#trouble-rows tr')].some(r => r.textContent.includes({glancer_email:?}) && r.textContent.includes(\"won't wake\")) && [...document.querySelectorAll('#new-rows tr')].some(r => r.textContent.includes({glancer_email:?}) && r.textContent.endsWith('yes'))"
            ),
            wait,
        );
        let tile = |b: &mut crate::browser::Browser| b.eval(&page, "[...document.querySelectorAll('#tiles .tile')].find(t => t.querySelector('.label').textContent === 'Computers awake now').querySelector('.big').textContent");
        let before = tile(&mut b)?;
        s.ok(
            "its tiles are the glance's numbers, its trouble names the person whose computer won't wake, and its new people say whose computer came up",
            rows && before == json!(awake.to_string()),
            json!({ "page": shown, "glance": read.body["logs"] }),
        );
        let said = b.until(
            &page,
            &format!("[...document.querySelectorAll('#trouble-rows tr')].some(r => r.textContent.includes({glancer_email:?}) && r.textContent.includes('a save failed') && /^disk full · \\d{{4}}-\\d\\d-\\d\\d \\d\\d:\\d\\d · backup '\\/data': No space left on device \\(os error 28\\)$/.test(r.querySelector('.why')?.textContent))"),
            wait,
        );
        s.ok("its trouble says why a save failed: the kind's short label, when, and the reason its newest line gave", said, b.eval(&page, "document.getElementById('trouble-rows').innerText")?);
        // the glance says how old the logs' numbers are (a read is kept
        // two minutes: `fragment_core::glance::LOGS_FRESH_MS`)
        let aged = b.until(&page, "/^Updated .* · logs read just now; again each minute\\.$/.test(document.getElementById('glance-at').textContent)", wait);
        s.ok("it says when the logs' numbers were read", aged, b.eval(&page, "document.getElementById('glance-at').textContent")?);
        // the trouble rows as an operator sees them, wide and narrow, light
        // and dark, for review
        for scheme in ["light", "dark"] {
            b.color_scheme(&page, scheme)?;
            for (width, height, mobile) in [(1280, 900, false), (390, 844, true)] {
                b.viewport(&page, width, height, mobile)?;
                let _ = b.screenshot(&page, &shots.join(format!("glance-trouble-{width}-{scheme}.png")));
            }
        }
        b.color_scheme(&page, "light")?;
        b.viewport(&page, 1280, 900, false)?;
        // a computer awake since, and the glance read again within the two
        // minutes: the read kept, its numbers as they were, saying when
        s.observability.log(now, json!({ "$workers.scriptName": crate::OBSERVABILITY_SCRIPT, "computer": "computer:000000000000000000000a11", "phase": { "phase": "awake" } }));
        let asked = s.observability.asked().len();
        // cleared, so its line said again is this read's
        b.eval(&page, "document.getElementById('glance-at').textContent = ''")?;
        b.click(&page, "#tabs button[data-tab=glance]")?;
        let again = b.until(&page, "/^Updated .* · logs read (just now|a minute ago); again each minute\\.$/.test(document.getElementById('glance-at').textContent)", wait);
        s.ok(
            "read again within two minutes, the glance asks the logs nothing and shows the read it kept, saying when it was read: a computer awake since counts at the next read",
            again && tile(&mut b)? == before && s.observability.asked().len() == asked,
            json!({ "tile": tile(&mut b)?, "asked": s.observability.asked().len() - asked, "at": b.eval(&page, "document.getElementById('glance-at').textContent")? }),
        );
        b.eval(&page, &format!("[...document.querySelectorAll('#trouble-rows tr')].find(r => r.textContent.includes({glancer_email:?})).click()"))?;
        let opened = b.until(&page, &format!("!document.getElementById('people').hidden && !document.getElementById('person').hidden && document.getElementById('person').textContent.includes({glancer_email:?})"), wait);
        s.ok("a person in trouble opens their page", opened, b.eval(&page, "document.getElementById('person').textContent")?);
    }
    b.click(&page, "#tabs button[data-tab=people]")?;
    let listed = b.until(&page, "document.querySelectorAll('#people-rows tr').length > 0 && /people/.test(document.getElementById('status').textContent)", wait);
    s.ok("its People tab lists people", listed, b.eval(&page, "document.getElementById('status').textContent")?);

    let email = format!("admin-page-{}@e2e.test", &Keys::generate().pubkey_hex()[..10]);
    b.eval(&page, &format!("(() => {{ const f = document.getElementById('comp'); f.email.value = {email:?}; f.kind.value = 'seat_always_on'; f.requestSubmit(); }})()"))?;
    let comped = b.until(&page, &format!("document.getElementById('status').textContent.startsWith('comped: {email}')"), wait);
    s.ok("it comps a seat for an email", comped, b.eval(&page, "document.getElementById('status').textContent")?);
    b.eval(&page, "(() => { const f = document.getElementById('search'); f.q.value = 'operator@'; f.requestSubmit(); })()")?;
    let found = b.until(&page, "[...document.querySelectorAll('#people-rows tr')].length === 1 && document.querySelector('#people-rows tr').textContent.includes('operator@e2e.test')", wait);
    s.ok("it finds a person by the start of their email", found, b.eval(&page, "document.getElementById('people-rows').textContent")?);
    // an always-on seat's holder who lets its computer sleep: the list and
    // their page say so, why their computer is not always on
    let sleeper = api.person()?;
    let sleeper_email = Api::email_of(&sleeper);
    let r = api.signed(&s.operator, "POST", "/api/admin/seats", Some(&json!({ "email": sleeper_email, "kind": "seat_always_on" })))?;
    let chose = api.signed(&sleeper, "PUT", "/api/seat", Some(&json!({ "sleeps": true })))?;
    anyhow::ensure!(r.status == 200 && chose.status == 200, "an always-on seat, let sleep: {r} {chose}");
    b.eval(&page, &format!("(() => {{ const f = document.getElementById('search'); f.q.value = {sleeper_email:?}; f.requestSubmit(); }})()"))?;
    let listed = b.until(&page, "[...document.querySelectorAll('#people-rows tr')].length === 1 && document.querySelector('#people-rows tr').textContent.includes('always-on comped, lets it sleep')", wait);
    if listed {
        b.click(&page, "#people-rows tr")?;
    }
    let shown = listed && b.until(&page, "!document.getElementById('person').hidden && document.getElementById('person').textContent.includes('always-on comped, lets it sleep')", wait);
    s.ok("a seat whose holder lets its computer sleep says so, listed and on their page", shown, b.eval(&page, "[document.getElementById('people-rows').textContent, document.getElementById('person').textContent]")?);

    b.click(&page, "#tabs button[data-tab=trials]")?;
    b.eval(&page, "(() => { const f = document.getElementById('trial-new'); f.elements.name.value = 'From the page'; f.elements.days.value = '9'; f.requestSubmit(); })()")?;
    let made = b.until(&page, "document.getElementById('status').textContent.startsWith('made ') && [...document.querySelectorAll('#trial-rows tr')].some(r => r.textContent.includes('From the page') && r.textContent.includes('9 days, always-on'))", wait);
    s.ok("it makes a trial code of the always-on seat (no kind to choose), and lists it", made, b.eval(&page, "document.getElementById('status').textContent")?);

    b.click(&page, "#tabs button[data-tab=orgs]")?;
    let orgs = b.until(&page, &format!("[...document.querySelectorAll('#org-rows tr')].some(r => r.textContent.includes({email:?}))"), wait);
    s.ok("its orgs list holds the comp's new org of one", orgs, "");
    b.click(&page, "#tabs button[data-tab=health]")?;
    let health = b.until(&page, "document.querySelectorAll('#health-facts dt').length >= 8", wait);
    s.ok("it shows billing's health", health, "");
    b.click(&page, "#tabs button[data-tab=log]")?;
    let log = b.until(&page, &format!("[...document.querySelectorAll('#log-rows tr')].some(r => r.textContent.includes('comp') && r.textContent.includes({email:?}))"), wait);
    s.ok("its log holds the comp, by this operator", log, "");

    // anyone else
    let other = api.sign_in(&format!("admin-page-stranger-{}@e2e.test", crate::api::now_s()))?;
    b.set_cookie(&format!("{}/", api.base), "fragment_session", &other)?;
    let theirs = b.open(&format!("{}/admin", api.base))?;
    let missing = b.until(&theirs, "document.readyState === 'complete' && !document.getElementById('status') && document.body.innerText.includes('no route /admin')", wait);
    s.ok("to anyone else it is the 404 of a path that is not there: nothing of the page", missing, b.eval(&theirs, "document.body.innerText")?);
    Ok(())
}

/// The operators' glance (`GET /api/admin/glance`), the admin Worker's:
/// the registry's people, and the logs' numbers from lines planted in the
/// Workers Observability fake (a local run's), kept a short while and
/// answered with when they were read; an operator's alone. Valid, replay
/// (read again within the while: nothing asked, the same read and its
/// time), invalid (another caller, a day that is not today), and the
/// logs' read failing (the registry's numbers stay). Each case after the
/// first names a day of its own (a millisecond apart), which no kept read
/// answers. Hosted, it answers its shape.
fn glance(s: &mut Suite, api: &Api, operator: &Keys, ivy: &Keys, ivy_id: &str, ivy_email: &str) -> Result<()> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis() as i64;
    let day = now - 3_600_000;
    let ask = |keys: &Keys, day: i64| api.signed(keys, "GET", &format!("/api/admin/glance?dayStart={day}"), None);
    if s.hosted() {
        let r = ask(operator, day)?;
        s.ok("the glance answers an operator: the registry's people, and the logs' numbers or why not", r.status == 200 && r.body["signups"]["total"].as_u64().is_some() && r.body["logs"]["state"].is_string(), &r);
        return Ok(());
    }
    let theirs = fragment_core::computer::default_computer_of(ivy_id);
    let stranger = "computer:00000000000000000000beef".to_string();
    let saver = "computer:0000000000000000000005a7".to_string();
    // the node's Worker's lines, and another deployment's in the same
    // account (a branch's), which name the same person's computer (a hash
    // of them) and are never counted
    let of = |script: &str, mut line: Value| {
        line["$workers.scriptName"] = json!(script);
        line
    };
    let (ours, foreign) = (crate::OBSERVABILITY_SCRIPT, "fragment-elsewhere");
    s.observability.log(now - 60_000, of(ours, json!({ "computer": theirs, "event": { "event": "alarm" }, "phase": { "phase": "awake" } })));
    s.observability.log(now - 2 * 3_600_000, of(ours, json!({ "computer": stranger, "phase": { "phase": "awake" } })));
    for (id, ago) in [("frag-a", 1_000), ("frag-a", 2_000), ("frag-b", 3_000)] {
        s.observability.log(now - ago, of(ours, json!({ "$workers.entrypoint": "Fragment", "$workers.durableObjectId": id })));
    }
    for ago in [10_000, 20_000] {
        s.observability.log(now - ago, of(ours, json!({ "computer": theirs, "phase": { "phase": "failed", "why": "its container kept stopping" } })));
    }
    s.observability.log(now - 30_000, of(ours, json!({ "computer": stranger, "disk": { "total": 20_000_000_000_u64, "free": 1_000_000_000_u64, "room": 731_564_544_u64 }, "generation": 3 })));
    // its saves failed twice: the newest says why, naming a person's file
    // (its phase left out of the lines, so it counts as awake nowhere)
    let failed = |seq: u64, why: &str| json!({ "computer": saver, "event": { "event": "save_failed", "generation": 2, "seq": seq, "why": why } });
    s.observability.log(now - 40_000, of(ours, failed(1, "its container is not running")));
    s.observability.log(now - 15_000, of(ours, failed(2, "backup '/data': open /data/home/Tax Return.pdf: Permission denied (os error 13)")));
    s.observability.log(now - 5_000, of(foreign, json!({ "computer": "computer:0000000000000000000f0e1d", "phase": { "phase": "awake" } })));
    s.observability.log(now - 5_000, of(foreign, json!({ "computer": theirs, "phase": { "phase": "failed" } })));
    s.observability.log(now - 5_000, of(foreign, json!({ "computer": theirs, "event": { "event": "save_failed", "why": "another deployment's" } })));
    s.observability.log(now - 1_000, of(foreign, json!({ "computer": saver, "event": { "event": "save_failed", "why": "another deployment's" } })));
    s.observability.log(now - 5_000, of(foreign, json!({ "$workers.entrypoint": "Fragment", "$workers.durableObjectId": "frag-elsewhere" })));
    let asked_at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis() as i64;
    let r = ask(operator, day)?;
    let b = &r.body;
    let listed = b["newPeople"].as_array().into_iter().flatten().find(|p| p["npub"] == ivy_id).cloned().unwrap_or(Value::Null);
    s.ok(
        "an operator's glance: signups in the last hour, today and the week, and the newest people, each with their email, seat, and computer, which the logs say came up",
        r.status == 200
            && b["signups"]["hour"].as_u64() >= Some(1)
            && b["signups"]["today"].as_u64() >= Some(1)
            && b["signups"]["total"].as_u64() >= b["signups"]["week"].as_u64()
            && listed["email"] == ivy_email
            && listed["seat"]["comped"] == true
            && listed["computer"] == theirs.as_str()
            && listed["computerUp"] == true,
        b,
    );
    let read = &b["logs"];
    let trouble = read["trouble"].as_array().cloned().unwrap_or_default();
    s.ok(
        "its logs' numbers, of this deployment's Worker's lines alone: computers awake now (a line in ten minutes), today and this week, fragments that ran, and computers in trouble, each named with its person when it is one's, a failed save with why its newest did, a person's file left out (another deployment's lines about the same computers count for nothing)",
        read["state"] == "read"
            && read["computersAwakeNow"] == 1
            && read["computersAwakeToday"] == 1
            && read["computersAwakeWeek"] == 2
            && read["fragmentsAwakeNow"] == 2
            && trouble.first() == Some(&json!({ "computer": theirs, "kind": "wont_wake", "lines": 2, "npub": ivy_id, "email": ivy_email }))
            && trouble.get(1) == Some(&json!({ "computer": saver, "kind": "saves_failing", "lines": 2, "npub": null, "email": null, "why": { "at": now - 15_000, "text": "backup …: Permission denied (os error 13)" } }))
            && trouble.get(2) == Some(&json!({ "computer": stranger, "kind": "disk_filling", "lines": 1, "npub": null, "email": null }))
            && trouble.len() == 3
            && read.get("whyUnread").is_none(),
        read,
    );
    let asked = s.observability.asked();
    let questions = fragment_core::glance::Question::ALL.len();
    s.ok(
        "its read asks one query a question, with the account's read-only key, each of this deployment's Worker, and says when it read them",
        asked.len() == questions
            && asked.iter().all(|q| q["queryId"] == fragment_core::glance::QUERY_ID && q["parameters"]["filters"][0]["value"] == crate::OBSERVABILITY_SCRIPT)
            && read["at"].as_i64().is_some_and(|at| (asked_at..=b["at"].as_i64().unwrap_or(0)).contains(&at))
            && read.get("whyKept").is_none(),
        json!({ "asked": asked.len(), "at": read["at"], "asked at": asked_at }),
    );
    // replay: within the while the read is kept, a load asks nothing and
    // answers the same read, with when it was read; the registry's numbers,
    // read every load, count a person who joined since
    api.person()?;
    let again = ask(operator, day)?;
    let total = |r: &Value| r["signups"]["total"].as_u64().unwrap_or(0);
    s.ok(
        "read again at once, the glance asks the logs nothing: the same numbers and trouble, read when they were, and the registry's read anew (a person who joined since counted)",
        again.status == 200 && s.observability.asked().len() == questions && again.body["logs"] == *read && again.body["at"].as_i64() >= b["at"].as_i64() && total(&again.body) == total(b) + 1,
        json!({ "asked": s.observability.asked().len(), "logs": again.body["logs"], "signups": [&b["signups"], &again.body["signups"]] }),
    );
    let person = ask(ivy, day)?;
    let tomorrow = ask(operator, now + 3_600_000)?;
    let stale = ask(operator, now - 2 * 24 * 3_600_000)?;
    // the integers' ends: refused, never an overflow past the bound
    let least = ask(operator, i64::MIN)?;
    let most = ask(operator, i64::MAX)?;
    s.ok(
        "anyone else's glance is refused (403), and a day that is not today is invalid (400), the integers' ends too",
        person.status == 403 && [&tomorrow, &stale, &least, &most].iter().all(|r| r.status == 400),
        json!([person.status, tomorrow.status, stale.status, least.status, most.status]),
    );
    // why saves failed is the lines themselves, which may run past the
    // cell's bound on an answer: its read failing alone takes nothing else
    s.observability.fail_next_events(400, "too many events to answer");
    let partial = ask(operator, day - 1)?;
    let logs = &partial.body["logs"];
    let rows = logs["trouble"].as_array().cloned().unwrap_or_default();
    s.ok(
        "when only why saves failed can't be read, the logs' numbers and trouble stand, no row says why, and the glance says why not",
        partial.status == 200
            && logs["state"] == "read"
            && logs["computersAwakeWeek"] == read["computersAwakeWeek"]
            && rows.iter().map(|t| (&t["computer"], &t["kind"])).eq(trouble.iter().map(|t| (&t["computer"], &t["kind"])))
            && rows.iter().all(|t| t.get("why").is_none())
            && logs["whyUnread"].as_str().is_some_and(|w| w.contains("SaveWhys") && w.contains("too many events to answer"))
            && s.observability.asked().len() == 2 * questions,
        logs,
    );
    s.observability.fail_next(403, "Authentication error");
    let before = s.observability.asked().len();
    let failed = ask(operator, day - 2)?;
    // the read stops at the refusal: the questions in flight then, and
    // no more (the fake records the refused one in none of them, so a read
    // that went on records the other eight)
    let asked = s.observability.asked().len() - before;
    s.ok(
        "when the logs refuse their key, the glance says why and keeps the registry's numbers; whether a computer came up is unknown; and the read stops at the refusal, never asking every question",
        failed.status == 200
            && failed.body["logs"]["state"] == "failed"
            && failed.body["logs"]["why"].as_str().is_some_and(|w| w.contains("Authentication error"))
            && failed.body["signups"] == again.body["signups"]
            && failed.body["newPeople"].as_array().into_iter().flatten().all(|p| p["computerUp"].is_null())
            && asked + 1 < questions,
        json!({ "asked": asked, "glance": failed.body }),
    );
    Ok(())
}
