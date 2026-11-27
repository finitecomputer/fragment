//! The finite.computer move (docs/migrations.md), driven through its
//! routes with the fake mover (no page yet): the roster, binding a person
//! by their WorkOS id, their answers (the levers', until their routes
//! land), the mover's mailbox and reports, each deadline by the object's
//! clock lever, held moves and staff's acts on them, and phase 2. Valid,
//! invalid, replay and restart for each.

use anyhow::{Context, Result};
use base64::Engine as _;
use fragment_fakes::mover::{event, fixture_zip, staged, Mover, Script};
use fragment_proto::migrations::{HandoffItem, Has, Roster, SecretKind, RosterAgent, RosterPerson, Sizes, STAGE_FORMAT, WHY_MAX_BYTES};
use std::time::Duration;
use serde_json::{json, Value};

use crate::api::Api;
use crate::{Need, Suite};

const HOUR_MS: i64 = 60 * 60 * 1000;
const DAY_MS: i64 = 24 * HOUR_MS;
/// The offer's words under its title, as the server says them
/// (`fragment_core::migrations::offer`).
const OFFER_TEXT: &str = "Its memories, chats, and supported files and skills come with it, to a more secure home with a fresh key.";
/// Whether a person's words promise a free month, which the move no
/// longer gives: its seat has no end (Paul, 2026-11-19).
fn promises_a_month(words: &str) -> bool {
    let w = words.to_lowercase();
    w.contains("month") || w.contains("free")
}

fn agent(old: &str) -> RosterAgent {
    RosterAgent {
        old: old.into(),
        name: format!("Old {old}"),
        picture_url: None,
        last_active_ms: Some(1_800_000_000_000),
        sizes: Sizes { cache: 1, history: 2, work: 3, staged: 5, total: 6 },
        brains: vec![],
        sites: vec![],
        // none, as the mover's fleet says of every old agent (FIN-498)
        has: Has { telegram: false, openrouter: false, nostr: false },
    }
}

/// `fragment operator <args> --key-file <key> --json`: its `data`.
fn op_cli(s: &Suite, api: &Api, home: &std::path::Path, key: &str, args: &[&str]) -> Result<Value> {
    s.cli_json(api, home, &[&["operator"], args, &["--key-file", key, "--json"]].concat())
}

fn person(workos_id: &str, olds: &[&str]) -> RosterPerson {
    RosterPerson { workos_id: workos_id.into(), email: format!("{workos_id}@old.test"), agents: olds.iter().map(|o| agent(o)).collect() }
}

pub fn moves(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("move", &[Need::Fakes, Need::Deployment, Need::Levers, Need::Move]) {
        return Ok(());
    }
    let mover = Mover::new(&api.base, s.mover.clone());
    let run = s.name("mv");
    let old = |n: &str| format!("{run}-{n}");
    let (a1, a2, b1, c1, d1) = (old("a1"), old("a2"), old("b1"), old("c1"), old("d1"));
    let lever = |body: Value| -> Result<Value> {
        let r = api.unsigned("POST", "/api/test/migrations", Some(&body))?;
        anyhow::ensure!(r.status == 200, "the migrations lever {body}: {r}");
        Ok(r.body)
    };
    let op_session = api.sign_in("operator@e2e.test")?;
    let _ = api.approve(&op_session, &s.operator);
    let operator = s.operator.clone();
    let admin = |method: &str, path: &str| api.signed(&operator, method, &format!("/api/admin/migrations{path}"), None);
    let act = |o: &str, op: &str| api.signed(&operator, "POST", &format!("/api/admin/migrations/{o}"), Some(&json!({ "op": op })));
    let withdraw = |o: &str, why: &str| api.signed(&operator, "POST", &format!("/api/admin/migrations/{o}"), Some(&json!({ "op": "withdraw", "why": why })));
    const WHY: &str = "a staff test agent";
    // staff's CLI, with the operator key's file
    let file = s.dir("move-operator").join("operator.key");
    std::fs::write(&file, format!("{}\n", operator.secret_hex()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600))?;
    }
    let (home, key) = (s.dir("move-cli"), file.to_string_lossy().to_string());
    // a move as staff read it: the operators' list, page by page
    let moved = |o: &str| -> Result<Value> {
        let mut after = String::new();
        loop {
            let r = admin("GET", &format!("?after={after}"))?;
            anyhow::ensure!(r.status == 200, "the moves: {r}");
            if let Some(m) = r.body["moves"].as_array().and_then(|ms| ms.iter().find(|m| m["old"] == o)) {
                return Ok(m.clone());
            }
            match r.body["next"].as_str() {
                Some(next) => after = next.to_string(),
                None => anyhow::bail!("no move {o}"),
            }
        }
    };
    let state = |o: &str| -> String { moved(o).map(|m| m["state"].as_str().unwrap_or("").to_string()).unwrap_or_default() };
    // the object's clock, moved forward; then its supervisor's pass
    let offset = std::cell::Cell::new(0i64);
    let later_by = |ms: i64| -> Result<()> {
        offset.set(offset.get() + ms);
        lever(json!({ "op": "clock", "offsetMs": offset.get() }))?;
        Ok(())
    };

    // ---- who may call: the mover's key alone; the operators alone
    let stranger = api.person()?;
    let roster = |snapshot: &str, people: Vec<RosterPerson>| Roster { snapshot: format!("{run}-{snapshot}"), observed_ms: 1_800_000_000_000, people };
    let r = api.signed(&stranger, "PUT", "/api/migrations/mover/roster", Some(&serde_json::to_value(roster("s0", vec![]))?))?;
    let unsigned = api.unsigned("GET", "/api/migrations/mover/mailbox", None)?;
    let r2 = api.signed(&stranger, "GET", "/api/admin/migrations", None)?;
    s.ok("a key that is not the mover's is refused (403), unsigned is 401, and someone who is no operator lists nothing (403)", r.status == 403 && unsigned.status == 401 && r2.status == 403, format!("{r} {unsigned} {r2}"));

    // ---- the roster lists; a person who opens the move is bound then
    let visit = |who: &str| api.unsigned("POST", "/api/test/migrations", Some(&json!({ "op": "visit", "who": who })));
    let alice = api.person()?;
    let alice_id = s.workos.user(&Api::email_of(&alice)).id;
    let dave_email = format!("{run}-dave@e2e.test");
    let dave_id = s.workos.user(&dave_email).id;
    let first = roster("s1", vec![person(&alice_id, &[&a1, &a2]), person(&dave_id, &[&d1]), person(&format!("{run}-nobody"), &[&b1])]);
    let r = mover.push(&first)?;
    let listed = [&a1, &a2, &d1, &b1].iter().all(|o| state(o) == "listed");
    let opened = visit(&Api::email_of(&alice))?;
    s.ok(
        "the mover's roster lists every old agent; the person who opens the move is bound then, both their old agents offered",
        r.status == 200 && r.body["bound"] == 0 && r.body["withdrawn"] == 0 && r.body["replay"] == false && listed && opened.status == 200 && opened.body["bound"] == 2 && state(&a1) == "open" && state(&a2) == "open" && state(&d1) == "listed",
        format!("{} {} {opened}", r.status, r.body),
    );
    let again = mover.push(&first)?;
    s.ok("the same snapshot again changes nothing (replay)", again.status == 200 && again.body["replay"] == true && again.body["bound"] == 0, format!("{} {}", again.status, again.body));
    let twice = roster("bad", vec![person(&alice_id, &[&a1]), person(&format!("{run}-x"), &[&a1])]);
    let bad_id = roster("bad2", vec![person(&alice_id, &["not/an id"])]);
    let (r1, r2) = (mover.push(&twice)?, mover.push(&bad_id)?);
    s.ok("a roster naming an old agent twice, or an id that is not one, is refused (400), changing nothing", r1.status == 400 && r2.status == 400 && state(&a1) == "open", format!("{} {}", r1.body, r2.body));
    // dave signs in later, and opens the move; a roster after names his new agent
    let unknown = visit(&dave_email)?;
    api.sign_in(&dave_email)?;
    let (late, again) = (visit(&dave_email)?, visit(&dave_email)?);
    let d2 = old("d2");
    let r = mover.push(&roster("s2", vec![person(&alice_id, &[&a1, &a2]), person(&dave_id, &[&d1, &d2]), person(&format!("{run}-nobody"), &[&b1])]))?;
    s.ok(
        "someone not signed in here opens nothing; a person who signs in later is bound when they open the move (again: nothing more), and a roster after offers their new old agent at once",
        unknown.status != 200 && late.body["bound"] == 1 && again.body["bound"] == 0 && state(&d1) == "open" && r.body["bound"] == 1 && state(&d2) == "open",
        format!("{unknown} {late} {} {}", r.status, r.body),
    );
    let mb = mover.commands()?;
    s.ok("nothing is asked yet: the mailbox holds no command of theirs", mb.iter().all(|c| !c.old.starts_with(&run)), format!("{mb:?}"));

    // ---- Not now, a day later; Never, then Ask again
    let answer = |o: &str, a: &str, agent: Option<&str>| api.unsigned("POST", "/api/test/migrations", Some(&json!({ "op": "answer", "old": o, "answer": a, "agent": agent })));
    let r = answer(&a2, "later", None)?;
    let replay = answer(&a2, "later", None)?;
    later_by(DAY_MS - HOUR_MS)?;
    let waiting = state(&a2);
    later_by(HOUR_MS)?;
    s.ok("\"Not now\" waits a day, then offers it again", r.status == 200 && replay.status == 200 && waiting == "later" && state(&a2) == "open", format!("{} {waiting}", r.body));
    let n = answer(&a2, "never", None)?;
    let back = answer(&a2, "ask_again", None)?;
    let listed = answer(&b1, "later", None)?;
    s.ok("\"Never\" holds until \"Ask again\"; a move whose person is not known here takes no answer (409)", n.body["state"] == "never" && back.body["state"] == "open" && listed.status == 409, format!("{} {} {}", n.body, back.body, listed));

    // ---- Yes: the mover is told to stop and stage it
    let (_, alice_agent) = api.own_agent(&alice)?;
    let (strangers_agent_keys, strangers_agent) = api.own_agent(&stranger)?;
    let theirs = answer(&a1, "yes", Some(&strangers_agent))?;
    let yes = answer(&a1, "yes", Some(&alice_agent))?;
    let yes_again = answer(&a1, "yes", Some(&alice_agent))?;
    let cmd = mover.commands()?.into_iter().find(|c| c.old == a1).context("a1's command")?;
    s.ok(
        "Yes with another person's agent is refused (403); with their own, the mover's mailbox holds stop_and_stage of the stage <old>.p1.1, asking for every kind of secret whatever the roster's has says, and Yes again changes nothing",
        theirs.status == 403
            && yes.body["state"] == "asked"
            && yes_again.status == 200
            && serde_json::to_value(&cmd)? == json!({ "id": format!("{a1}:1:stop_and_stage"), "old": a1, "attempt": 1, "kind": "stop_and_stage", "phase": "p1", "stage": format!("{a1}.p1.1"), "secrets": ["telegram_bot_token", "openrouter_key", "nostr_nsec"] }),
        format!("{theirs} {cmd:?}"),
    );
    // an agent takes in one move, ever (Paul 11-19): a2's Yes into the
    // agent a1 is bound to is refused, a2 left as it was
    let taken = answer(&a2, "yes", Some(&alice_agent))?;
    s.ok(
        "a Yes into an agent another of the person's moves is bound to is refused (409, in the server's words), changing nothing",
        taken.status == 409 && taken.to_string().contains("that agent took in another upgrade already") && state(&a2) == "open" && state(&a1) == "asked",
        &taken,
    );

    // ---- the hand-off of its secrets (F11): the mover's alone, for its
    // command; a value is never echoed (move-import puts them)
    let hand = |keys: &crate::Keys, o: &str, body: Value| api.signed(keys, "POST", &format!("/api/migrations/mover/agents/{o}/secrets"), Some(&body));
    let mover_keys = s.mover.clone();
    let nsec = json!({ "kind": "nostr_nsec", "value": "nsec1vl029mgpspedva04g90vltkh6fvh240zqtv9k0t9af8935ke9laqsnlfe5" });
    let stranger_hand = hand(&stranger, &a1, json!({ "command": cmd.id, "items": [nsec] }))?;
    let twice = hand(&mover_keys, &a1, json!({ "command": cmd.id, "items": [nsec, { "kind": "nostr_nsec", "value": "handoff-sentinel" }] }))?;
    let unread = hand(&mover_keys, &a1, json!({ "command": cmd.id, "items": [{ "kind": "nostr_nsec", "value": 987_654_321_012u64 }] }))?;
    let stale = hand(&mover_keys, &a1, json!({ "command": format!("{a1}:9:stop_and_stage"), "items": [nsec] }))?;
    let none = hand(&mover_keys, &d1, json!({ "command": format!("{d1}:1:stop_and_stage"), "items": [nsec] }))?;
    let quiet = [&twice, &unread].iter().all(|r| !r.text.contains("handoff-sentinel") && !r.text.contains("987654321012"));
    s.ok(
        "a hand-off is the mover's alone (403); a kind twice, or a body that does not read is refused (400) in words that never quote a value; another command, or an old agent with none, is 409",
        stranger_hand.status == 403 && twice.status == 400 && unread.status == 400 && quiet && stale.status == 409 && none.status == 409,
        format!("{} {} {} {} {}", stranger_hand.status, twice, unread, stale.status, none.status),
    );

    // ---- the mover down: held, then staff try again
    mover.script(Script::Down);
    mover.run_once()?;
    later_by(HOUR_MS)?;
    let m = moved(&a1)?;
    s.ok("a mover that never says it stopped the old agent holds the move (mover_down) within the hour", m["state"] == "held" && m["reason"] == "mover_down", &m);
    let list = admin("GET", "")?;
    let row = list.body["moves"].as_array().and_then(|ms| ms.iter().find(|m| m["old"] == a1.as_str())).cloned().unwrap_or_default();
    s.ok("staff see it held, with its reason and no command, and no name or email", list.status == 200 && row["state"] == "held" && row["reason"] == "mover_down" && row["command"].is_null() && !list.text.contains("@old.test") && !list.text.contains("Old "), &row);
    let r = op_cli(s, api, &home, &key, &["retry", &a1]);
    let stale = mover.report_as(&a1, &cmd.id, 9, event("stopped"))?;
    s.ok(
        "staff's retry (`fragment operator retry`) is a new attempt (2), and a report on attempt 1's command is stale (409), changing nothing",
        r.as_ref().is_ok_and(|v| v["state"] == "asked" && v["attempt"] == 2) && stale.status == 409 && state(&a1) == "asked",
        format!("{r:?} {}", stale.body),
    );

    // ---- staging: stopped, progress, a part, then a stall, then staged
    mover.script(Script::Stalls);
    let ran = mover.run_once()?;
    let objects = |prefix: &str| -> Vec<String> { lever(json!({ "op": "objects", "prefix": prefix })).ok().and_then(|v| serde_json::from_value(v["keys"].clone()).ok()).unwrap_or_default() };
    let two = format!("{a1}.p1.2");
    let taken = mover.parts(&two)?;
    s.ok(
        "the mover's part is taken while it stages: its number, size, hash and etag listed for a resume, never its bytes",
        taken.status == 200 && taken.body["parts"].as_array().is_some_and(|p| p.len() == 1 && p[0]["n"] == 1 && p[0]["bytes"] == 65_536 && p[0]["sha256"].as_str().is_some_and(|h| h.len() == 64) && p[0]["etag"].is_string() && p[0].as_object().is_some_and(|o| o.len() == 4)),
        &taken.body,
    );
    // what it was not told to upload, or a hash its signature does not cover: refused, nothing taken
    let piece = fixture_zip(1024);
    let (other_attempt, other_agent, other_phase) = (mover.put_part(&format!("{a1}.p1.3"), 1, &piece)?, mover.put_part(&format!("{d1}.p1.1"), 1, &piece)?, mover.put_part(&format!("{a1}.p2.2"), 1, &piece)?);
    let lying = mover.put_part_claiming(&two, 2, &piece, &"0".repeat(64))?;
    let after_lie = mover.parts(&two)?;
    let bare = api.unsigned("GET", &format!("/api/migrations/mover/stages/{two}/parts"), None)?;
    let theirs = api.signed(&stranger, "GET", &format!("/api/migrations/mover/stages/{two}/parts"), None)?;
    s.ok(
        "the mover may not upload a stage it was not told to (another attempt's, another old agent's, another phase's: 403), nor a part whose signature is not over the hash its URL names (401, nothing taken); no one else lists its parts (401, 403)",
        other_attempt.status == 403 && other_agent.status == 403 && other_phase.status == 403 && lying.status == 401 && after_lie.body["parts"].as_array().map(Vec::len) == Some(1) && bare.status == 401 && theirs.status == 403,
        format!("{} {} {} {} {}", other_attempt.status, other_agent.status, other_phase.status, lying.status, lying.body),
    );
    let mut short = mover.manifest(&mover.commands()?.into_iter().find(|c| c.old == a1).context("a1's command")?)?;
    short.zip_bytes += 1;
    let early = mover.complete(&two, &short)?;
    s.ok("a complete whose parts are not its manifest's zip is refused (400), nothing written", early.status == 400 && objects(&format!("p1/{a1}/2/")).is_empty(), format!("{} {}", early.status, early.body));
    let cmd2 = mover.commands()?.into_iter().find(|c| c.old == a1).context("a1's command")?;
    let replay = mover.report_as(&a1, &cmd2.id, 1, event("stopped"))?;
    s.ok(
        "stopped and progress move it to staging; a report's seq taken already is answered taken: false, changing nothing",
        ran.iter().any(|(c, a)| c.old == a1 && a.iter().all(|x| x.status == 200)) && state(&a1) == "staging" && replay.status == 200 && replay.body["taken"] == false,
        format!("{} {}", replay.status, replay.body),
    );
    later_by(31 * 60 * 1000)?;
    let m = moved(&a1)?;
    s.ok("a stage with no word for 30 minutes is tried again as a new attempt (3), the stalled one's prefix queued for deletion", m["state"] == "asked" && m["attempt"] == 3, &m);
    mover.script(Script::Done);
    // not stopped yet: its stage is not the mover's to upload
    let three = format!("{a1}.p1.3");
    let unstopped = mover.put_part(&three, 1, &piece)?;
    s.ok("before the mover says it stopped the old agent, its stage takes no part (409)", unstopped.status == 409, format!("{} {}", unstopped.status, unstopped.body));
    // staged without its stage complete: refused, nothing moves
    let cmd3 = mover.commands()?.into_iter().find(|c| c.old == a1).context("a1's command")?;
    mover.report(&cmd3, event("stopped"))?;
    let wrong = mover.report(&cmd3, staged(&"00".repeat(32)))?;
    s.ok("staged before its stage is complete is refused (409) and the move waits", wrong.status == 409 && state(&a1) == "staging", format!("{} {}", wrong.status, wrong.body));
    // a zip of two parts, the first sent before (a resume): the second alone is sent
    let zip = fixture_zip(5 * 1024 * 1024 + 65_536);
    mover.zip(zip.clone(), 5 * 1024 * 1024);
    let first = mover.put_part(&three, 1, &zip[..5 * 1024 * 1024])?;
    let sha = mover.stage(&cmd3)?;
    let again = mover.complete(&three, &mover.manifest(&cmd3)?)?;
    let held = mover.parts(&three)?;
    s.ok(
        "a zip of two parts, resumed after the first: made whole, its manifest written last, the same complete again answering the same",
        first.status == 200 && held.body["parts"].as_array().map(Vec::len) == Some(2) && again.status == 200 && again.body["manifest_sha256"] == sha.as_str() && again.body["zip_bytes"] == zip.len() && objects(&format!("p1/{a1}/3/")) == [format!("p1/{a1}/3/manifest.json"), format!("p1/{a1}/3/stage.zip")],
        format!("{} {}", again.body, held.body),
    );
    mover.report(&cmd3, staged(&sha))?;
    mover.zip(fixture_zip(65_536), 65_536);
    let after = moved(&a1)?;
    s.ok(
        "staged: importing attempt 3 into the agent bound at Yes, its command done (its routine is the alarm's: F7)",
        after["state"] == "importing" && after["attempt"] == 3 && after["command"].is_null() && after["agent"] == alice_agent.as_str(),
        &after,
    );

    // ---- an import never opened: its routine once more, then held
    later_by(2 * HOUR_MS)?;
    let once_more = state(&a1) == "importing";
    later_by(2 * HOUR_MS)?;
    let m = moved(&a1)?;
    s.ok("a stage not opened within two hours gets its routine once more (still importing), then the move is held (not_opened)", once_more && m["state"] == "held" && m["reason"] == "not_opened", &m);

    // ---- staff restart the old agent: offered again, the mover told to restart it
    let staged_three = objects(&format!("p1/{a1}/3/"));
    let r = op_cli(s, api, &home, &key, &["restart-old", &a1]);
    let restart = mover.commands()?.into_iter().find(|c| c.old == a1);
    mover.run_once()?;
    let m = moved(&a1)?;
    s.ok(
        "restart-old (`fragment operator restart-old`) offers it again, unbound, and the mover restarts the old agent: its command answered, then gone",
        r.as_ref().is_ok_and(|v| v["state"] == "open") && restart.is_some_and(|c| c.id == format!("{a1}:3:restart")) && m["command"].is_null() && m["agent"].is_null(),
        &m,
    );

    // what the alarm deletes, it deletes after the call that queued it
    let deleted = |s: &Suite, prefix: &str| s.eventually(Duration::from_secs(20), || objects(prefix).is_empty());
    let gave_up = deleted(s, &format!("p1/{a1}/3/"));
    s.ok("the stage it gave up on is deleted from the bucket by the alarm", staged_three.len() == 2 && gave_up, format!("{staged_three:?}"));

    // ---- a stage that fails at the last attempt is held; staff retire it
    let yes = answer(&a1, "yes", Some(&alice_agent))?;
    let attempt = yes.body["attempt"].clone();
    mover.script(Script::Fails);
    mover.run_once()?;
    let m = moved(&a1)?;
    s.ok("Yes again is attempt 4; a failed stage past the attempts is held (stage_failed)", attempt == 4 && m["state"] == "held" && m["reason"] == "stage_failed", &m);
    let r = op_cli(s, api, &home, &key, &["retire", &a1]);
    mover.script(Script::Done);
    mover.run_once()?;
    let bad = withdraw(&a1, WHY)?;
    let again = op_cli(s, api, &home, &key, &["retire", &a1]);
    s.ok(
        "staff retire it (`fragment operator retire`): the mover retires the old agent and the move is done (moved); a done move is not withdrawn (409), nor retired again (the CLI fails)",
        r.as_ref().is_ok_and(|v| v["state"] == "retiring") && state(&a1) == "moved" && bad.status == 409 && again.is_err(),
        format!("{r:?} {bad} {again:?}"),
    );

    // ---- the one person's bound agent reads its own stage, while it
    // imports; no one else, ever
    let e1 = old("e1");
    mover.push(&roster("s2b", vec![person(&alice_id, &[&a1, &a2, &e1]), person(&dave_id, &[&d1, &d2]), person(&format!("{run}-nobody"), &[&b1])]))?;
    // an agent takes in one move, ever: alice's first took in a1 (moved)
    let into_again = answer(&e1, "yes", Some(&alice_agent))?;
    let (e1_keys, e1_agent) = api.own_agent(&alice)?;
    let yes_e1 = answer(&e1, "yes", Some(&e1_agent))?;
    s.ok(
        "an agent that took in a move takes in no other, even once that move is done (409, e1 left as it was); another agent of theirs is taken",
        into_again.status == 409 && into_again.to_string().contains("that agent took in another upgrade already") && yes_e1.status == 200 && yes_e1.body["state"] == "asked",
        format!("{into_again} {yes_e1}"),
    );
    let stage = format!("{e1}.p1.1");
    let at = |what: &str| format!("/api/migrations/stages/{stage}/{what}");
    let before = (api.signed(&e1_keys, "POST", &at("open"), None)?, api.signed(&e1_keys, "GET", &at("zip"), None)?);
    s.ok("its bound agent reads nothing before the move imports (409)", before.0.status == 409 && before.1.status == 409, format!("{} {}", before.0, before.1));
    mover.run_once()?;
    let (alice_other_keys, _) = api.own_agent(&alice)?;
    let mover_reads = (mover.call("GET", &at("zip"), None)?.status, mover.call("POST", &at("open"), None)?.status);
    let refused = [
        ("a stranger", api.signed(&stranger, "GET", &at("zip"), None)?.status),
        ("another person's agent", api.signed(&strangers_agent_keys, "GET", &at("zip"), None)?.status),
        ("the same person's other agent", api.signed(&alice_other_keys, "GET", &at("zip"), None)?.status),
        ("its person, as themselves", api.signed(&alice, "GET", &at("zip"), None)?.status),
        ("an operator", api.signed(&operator, "GET", &at("zip"), None)?.status),
        ("another person's agent opening it", api.signed(&strangers_agent_keys, "POST", &at("open"), None)?.status),
    ];
    s.ok(
        "while it imports, its stage is refused to a stranger, another person's agent, the same person's other agent, its person, and an operator (403), and to the mover (401: no route reads it a stage)",
        refused.iter().all(|(_, status)| *status == 403) && mover_reads.0 == 401 && mover_reads.1 == 401,
        format!("{refused:?} {mover_reads:?}"),
    );
    let row = moved(&e1)?;
    let fields = ["old", "workos_id", "person", "agent", "state", "reason", "phase", "attempt", "command", "since_ms", "deadline_ms", "staged_ms", "why"];
    s.ok("staff's list says its state and when, never a stage's bytes or manifest", row["state"] == "importing" && row.as_object().is_some_and(|o| o.keys().all(|k| fields.contains(&k.as_str()))), &row);
    let other = api.signed(&e1_keys, "POST", &format!("/api/migrations/stages/{e1}.p1.9/open"), None)?;
    let opened = api.signed(&e1_keys, "POST", &at("open"), None)?;
    let m = &opened.body["manifest"];
    s.ok(
        "its bound agent opens it: the manifest as completed; another attempt's stage is none (404)",
        other.status == 404 && opened.status == 200 && opened.body["stage"] == stage.as_str() && m["format"] == STAGE_FORMAT && m["old"] == e1.as_str() && m["attempt"] == 1 && m["zip_bytes"] == 65_536 && m["left"][0]["why"] == "secret",
        &opened.body,
    );
    let whole = api.signed(&e1_keys, "GET", &at("zip"), None)?;
    let range = api.call(crate::api::Call { method: "GET", url: format!("{}{}", api.base, at("zip")), keys: Some(&e1_keys), extra: vec![("range", "bytes=100-199".to_string())], ..Default::default() })?;
    let zip = mover.zip_bytes();
    s.ok(
        "it reads the zip whole (200), and by a range (206, that range alone, content-range of the whole)",
        whole.status == 200 && whole.bytes == zip && range.status == 206 && range.bytes == zip[100..200] && range.header("content-range") == "bytes 100-199/65536" && range.header("accept-ranges") == "bytes",
        format!("{} {} {}", whole.status, range.status, range.header("content-range")),
    );
    let bad = api.signed(&e1_keys, "POST", &at("done"), Some(&json!({ "ok": "yes" })))?;
    let done = api.signed(&e1_keys, "POST", &at("done"), Some(&json!({ "ok": true, "report_sha256": "cd".repeat(32) })))?;
    let again = api.signed(&e1_keys, "POST", &at("done"), Some(&json!({ "ok": true })))?;
    let after = (api.signed(&e1_keys, "GET", &at("zip"), None)?, api.signed(&e1_keys, "POST", &at("open"), None)?);
    s.ok(
        "done well: retiring; the same done again changes nothing; its stage is refused to its own agent at once (409); a done that is not one is 400",
        bad.status == 400 && done.status == 200 && done.body["state"] == "retiring" && again.status == 200 && again.body["state"] == "retiring" && after.0.status == 409 && after.1.status == 409,
        format!("{bad} {} {} {}", done.body, after.0, after.1),
    );
    // done is harmless: nothing for the mover, the old agent stays stopped
    // and its stage kept, until staff's word
    let asked = mover.commands()?.into_iter().any(|c| c.old == e1);
    mover.run_once()?;
    let row = moved(&e1)?;
    let kept = objects(&format!("p1/{e1}/1/"));
    s.ok(
        "a good import waits for staff: retiring with no command, nothing in the mailbox, no deadline but its stage's keep, and its stage kept in the bucket",
        !asked && row["state"] == "retiring" && row["command"].is_null() && row["deadline_ms"] == row["staged_ms"].as_i64().map(|at| at + 14 * DAY_MS).unwrap_or_default() && kept.len() == 2,
        format!("{row} {kept:?}"),
    );
    let r = act(&e1, "retire")?;
    let retire = mover.commands()?.into_iter().find(|c| c.old == e1).map(|c| serde_json::to_value(&c).unwrap_or_default());
    mover.run_once()?;
    s.ok(
        "on staff's retire the old agent is retired as migrated (moved), its stage still kept to its keep",
        r.status == 200 && r.body["state"] == "retiring" && retire.as_ref().is_some_and(|c| c["kind"] == "retire" && c["as"] == "migrated") && state(&e1) == "moved" && objects(&format!("p1/{e1}/1/")).len() == 2,
        format!("{r} {retire:?} {}", state(&e1)),
    );

    // ---- a roster withdraws the unasked; staff's withdrawal holds
    let r = mover.push(&roster("s3", vec![person(&alice_id, &[&a1, &a2]), person(&dave_id, &[&d1, &d2])]))?;
    let gone = state(&b1);
    let r2 = mover.push(&roster("s4", vec![person(&alice_id, &[&a1, &a2]), person(&dave_id, &[&d1, &d2]), person(&format!("{run}-nobody"), &[&b1])]))?;
    let back = state(&b1);
    let w = withdraw(&b1, WHY)?;
    mover.push(&roster("s5", vec![person(&alice_id, &[&a1, &a2]), person(&dave_id, &[&d1, &d2]), person(&format!("{run}-nobody"), &[&b1])]))?;
    s.ok(
        "a roster without an unasked move withdraws it, the next that lists it offers it again, and staff's withdrawal stays whatever a roster says; a moved one is untouched",
        r.body["withdrawn"] == 1 && gone == "withdrawn" && r2.status == 200 && back == "listed" && w.body["state"] == "withdrawn" && state(&b1) == "withdrawn" && state(&a1) == "moved",
        format!("{} {} {gone} {back}", r.body, r2.body),
    );

    // ---- the will-not-move list: staff withdraw an old agent ahead of the
    // roster, saying why; `include` takes a withdrawal back
    let (w1, w2) = (old("w1"), old("w2"));
    let nobody = format!("{run}-nobody");
    let (empty, long) = (withdraw(&w1, " ")?, withdraw(&w1, &"x".repeat(WHY_MAX_BYTES + 1))?);
    let ahead = withdraw(&w1, WHY)?;
    let again = withdraw(&w1, WHY)?;
    s.ok(
        "staff withdraw an old agent no roster has listed yet, saying why (none, or too long: 400); the same again changes nothing",
        empty.status == 400 && long.status == 400 && ahead.status == 200 && ahead.body["state"] == "withdrawn" && ahead.body["why"] == WHY && ahead.body["workos_id"] == "" && again.status == 200 && again.body == ahead.body,
        format!("{empty} {long} {ahead} {again}"),
    );
    let r = mover.push(&roster("s5b", vec![person(&alice_id, &[&a1, &a2, &w1]), person(&dave_id, &[&d1, &d2]), person(&nobody, &[&b1])]))?;
    let f = moved(&w1)?;
    let notices = api.signed(&alice, "GET", "/api/notices", None)?;
    let offers = api.signed(&alice, "GET", "/api/migrations/offers", None)?;
    let told = mover.commands()?.into_iter().any(|c| c.old == w1);
    s.ok(
        "a roster that lists it names whose it is and leaves it withdrawn, with its why: its person is not asked, and the mover told nothing",
        r.status == 200 && f["state"] == "withdrawn" && f["workos_id"] == alice_id.as_str() && f["why"] == WHY && notices.status == 200 && !notices.text.contains(&format!("move:{w1}")) && offers.status == 200 && !offers.text.contains(&w1) && !told,
        format!("{} {f} {notices} {offers} {told}", r.body),
    );
    let list = admin("GET", "")?;
    let listed: Vec<String> = list.body["moves"].as_array().into_iter().flatten().filter(|m| m["why"].is_string()).filter_map(|m| m["old"].as_str().map(str::to_string)).collect();
    s.ok("the operators' list gives staff's why with each move they withdrew, and none with any other", list.status == 200 && listed.contains(&b1) && listed.contains(&w1) && !listed.contains(&a2), json!(listed));
    // include: b1, listed by the roster, its person not here: listed again
    let inc = act(&b1, "include")?;
    let inc_again = act(&b1, "include")?;
    // w1, dropped by the roster: left withdrawn, the roster's, until a
    // roster lists it, which offers it to its person (bound: open)
    mover.push(&roster("s5c", vec![person(&alice_id, &[&a1, &a2]), person(&dave_id, &[&d1, &d2]), person(&nobody, &[&b1])]))?;
    let dropped = act(&w1, "include")?;
    mover.push(&roster("s5d", vec![person(&alice_id, &[&a1, &a2, &w1]), person(&dave_id, &[&d1, &d2]), person(&nobody, &[&b1])]))?;
    let f_back = state(&w1);
    // w2, withdrawn ahead of any roster: forgotten
    let g = withdraw(&w2, WHY)?;
    let forgot = act(&w2, "include")?;
    let forgot_again = act(&w2, "include")?;
    s.ok(
        "include takes staff's withdrawal back: listed again (again: nothing more); one the roster dropped stays withdrawn until a roster lists it (then offered); one withdrawn ahead of any roster is forgotten (then 404)",
        inc.status == 200 && inc.body["state"] == "listed" && inc.body["why"].is_null() && inc_again.body == inc.body
            && dropped.status == 200 && dropped.body["state"] == "withdrawn" && dropped.body["why"].is_null() && f_back == "open"
            && g.status == 200 && forgot.status == 200 && forgot.body.is_null() && moved(&w2).is_err() && forgot_again.status == 404,
        format!("{inc} {dropped} {f_back} {forgot} {forgot_again}"),
    );
    // the same from the CLI, with the operator key's file
    let w3 = old("w3");
    let cli = |args: &[&str]| op_cli(s, api, &home, &key, args);
    let out = cli(&["withdraw", &w3, "--why", WHY]);
    let list = cli(&["moves", "--withdrawn"]);
    let back = cli(&["include", &w3]);
    let after = cli(&["moves", "--withdrawn"]);
    let olds = |v: &Result<Value>| v.as_ref().ok().and_then(|v| v["moves"].as_array().cloned()).unwrap_or_default().iter().filter_map(|m| m["old"].as_str().map(str::to_string)).collect::<Vec<_>>();
    s.ok(
        "`fragment operator withdraw`, `moves --withdrawn` and `include` with the operator key's file: an old id no roster listed is withdrawn with its why, on the list, then forgotten",
        out.as_ref().is_ok_and(|v| v["state"] == "withdrawn" && v["why"] == WHY) && olds(&list).contains(&w3) && back.as_ref().is_ok_and(Value::is_null) && !olds(&after).contains(&w3) && olds(&after).iter().all(|o| o != &a2),
        format!("{out:?} {list:?} {back:?}"),
    );
    // w1 out again, for the cutoff below
    withdraw(&w1, WHY)?;

    // ---- phase 2: past the cutoff, the unasked are staged and retired
    // unasked (archived); a late Yes imports the kept stage; 90 days on, expired
    // carol is on the old platform alone: she signs up only after the cutoff
    let carol = crate::Keys::generate();
    let c_id = s.workos.user(&Api::email_of(&carol)).id;
    mover.push(&roster("s6", vec![person(&alice_id, &[&a1, &a2, &w1]), person(&dave_id, &[&d1, &d2]), person(&c_id, &[&c1])]))?;
    // dave is wiped before the cutoff, his move still bound to him (a wipe
    // is an operator key's that no person holds)
    let wiped = match s.runs("move", &[Need::Operator]) {
        true => {
            let dave = moved(&d1)?["person"].as_str().context("dave's move names him")?.to_string();
            let wiper = s.wiper.clone().context("Need::Operator lends an operator key")?;
            Some(super::billing::wipe_until_done(api, &wiper, &dave)?.0)
        }
        false => None,
    };
    let cutoff = crate::MIGRATIONS_CUTOFF_MS;
    let now = lever(json!({ "op": "clock", "offsetMs": offset.get() }))?["now"].as_i64().context("the object's now")?;
    later_by(cutoff - now + 1)?;
    let p2 = mover.commands()?.into_iter().filter(|c| [&a2, &c1, &d1].contains(&&c.old)).map(|c| (c.old.clone(), serde_json::to_value(&c).unwrap_or_default())).collect::<Vec<_>>();
    let f_cut = mover.commands()?.into_iter().any(|c| c.old == w1);
    s.ok(
        "at the cutoff every unasked move is archiving: stop_and_stage of its stage <old>.p2.<attempt>, handing off no secret (no one said Yes); staff's withdrawn, the roster listing it, is passed by",
        p2.len() == 3 && p2.iter().all(|(o, c)| c["kind"] == "stop_and_stage" && c["phase"] == "p2" && c["stage"].as_str().is_some_and(|p| p.starts_with(&format!("{o}.p2."))) && c["secrets"].is_null()) && !f_cut && state(&w1) == "withdrawn",
        format!("{p2:?} {f_cut} {}", state(&w1)),
    );
    // each archive carries its owner's SimpleX chat
    mover.simplex(true);
    mover.run_once()?;
    mover.simplex(false);
    s.ok(
        "an archive that carries a SimpleX chat lands for someone wiped since the move bound them: they hold no seat to keep on, so its complete goes on",
        wiped.as_ref().is_none_or(|w| w.status == 200 && w.body["state"] == "wiped") && moved(&d1)?["staged_ms"].is_i64() && objects(&format!("p2/{d1}/")).len() == 2,
        format!("{} {} {:?}", wiped.as_ref().map(|w| w.to_string()).unwrap_or_default(), moved(&d1)?, objects(&format!("p2/{d1}/"))),
    );
    let retire = mover.commands()?.into_iter().find(|c| c.old == c1).map(|c| serde_json::to_value(&c).unwrap_or_default());
    mover.run_once()?;
    s.ok("staged, then retired as archived: archived", retire.as_ref().is_some_and(|c| c["kind"] == "retire" && c["as"] == "archived" && c.get("stage").is_none()) && state(&c1) == "archived" && state(&a2) == "archived", format!("{retire:?}"));
    // archived agents are marked moved on the old platform, so the roster
    // names them no more; carol signs up late, and opens the move
    let before = moved(&c1)?["person"].clone();
    let r = mover.push(&roster("s7", vec![]))?;
    let session = api.sign_in(&Api::email_of(&carol))?;
    api.approve(&session, &carol)?;
    let opened = visit(&Api::email_of(&carol))?;
    let after = moved(&c1)?;
    s.ok(
        "a person whose archived agent the roster no longer names, signing up late, is bound when they open the move; the archive stays archived",
        before.is_null() && r.status == 200 && opened.status == 200 && after["person"].is_string() && after["state"] == "archived",
        format!("{before} {opened} {after}"),
    );
    let (carol_agent_keys, carol_agent) = api.own_agent(&carol)?;
    // archived, the stage is no one's to read until its person says Yes
    let unread = api.signed(&carol_agent_keys, "GET", &format!("/api/migrations/stages/{c1}.p2.{}/zip", moved(&c1)?["attempt"]), None)?;
    s.ok("an archived stage is read by no one, its person's own agent included, until their Yes binds it (403)", unread.status == 403, &unread);
    let (_, a2_agent) = api.own_agent(&alice)?;
    for (o, who, agent) in [(&c1, "a late sign-up", &carol_agent), (&a2, "someone who never answered", &a2_agent)] {
        let late = answer(o, "yes", Some(agent))?;
        let late_move = moved(o)?;
        s.ok(
            &format!("{who}'s late Yes imports the kept phase 2 stage: importing, its phase 2 stage, into the agent they name"),
            late.body["state"] == "importing" && late_move["state"] == "importing" && late_move["phase"] == "p2" && late_move["agent"] == agent.as_str(),
            &late_move,
        );
    }
    // the late sign-up's import, to its end: on staff's word, the old agent
    // retired as migrated, marked again from archived (what finite.computer
    // redirects by)
    let late_stage = format!("{c1}.p2.{}", moved(&c1)?["attempt"]);
    let at = |what: &str| format!("/api/migrations/stages/{late_stage}/{what}");
    let opened = api.signed(&carol_agent_keys, "POST", &at("open"), None)?;
    let zip = api.signed(&carol_agent_keys, "GET", &at("zip"), None)?;
    let done = api.signed(&carol_agent_keys, "POST", &at("done"), Some(&json!({ "ok": true })))?;
    let asked = mover.commands()?.into_iter().any(|c| c.old == c1);
    let r = act(&c1, "retire")?;
    let retire = mover.commands()?.into_iter().find(|c| c.old == c1).map(|c| serde_json::to_value(&c).unwrap_or_default());
    mover.run_once()?;
    let late_kept = objects(&format!("p2/{c1}/"));
    s.ok(
        "the late sign-up reads the kept stage and says it is done; on staff's retire, the old agent is retired as migrated, the move done (moved), its stage kept to its expiry",
        opened.status == 200 && opened.body["manifest"]["phase"] == "p2" && zip.status == 200 && zip.bytes == mover.zip_bytes() && done.body["state"] == "retiring" && !asked && r.status == 200 && retire.as_ref().is_some_and(|c| c["kind"] == "retire" && c["as"] == "migrated") && state(&c1) == "moved" && late_kept.len() == 2,
        format!("{} {} {asked} {retire:?} {} {late_kept:?}", opened.status, done.body, state(&c1)),
    );
    let kept = objects(&format!("p2/{d1}/"));
    later_by(90 * DAY_MS)?;
    let e = moved(&d1)?;
    let gone = deleted(s, &format!("p2/{d1}/")) && deleted(s, &format!("p2/{c1}/"));
    s.ok(
        "90 days past the cutoff an unclaimed archive expires, and every kept phase 2 stage is deleted from the bucket, the imported one's too",
        kept.len() == 2 && e["state"] == "expired" && state(&c1) == "moved" && gone,
        format!("{kept:?} {}", e["state"]),
    );
    lever(json!({ "op": "clock", "offsetMs": 0 }))?;
    offset.set(0);
    offer(s, api, &mover, &run, &later_by, &state)
}

/// F4: the offer, told as a notice that must be answered (`GET
/// /api/notices`), its person bound by their first read, and their own
/// answers through their routes: valid, invalid (another's, an agent's),
/// replay, and the watch told of each change, the alarm's too.
fn offer(s: &mut Suite, api: &Api, mover: &Mover, run: &str, later_by: &dyn Fn(i64) -> Result<()>, state: &dyn Fn(&str) -> String) -> Result<()> {
    let erin = api.person()?;
    let erin_id = s.workos.user(&Api::email_of(&erin)).id;
    let (f1, f2) = (format!("{run}-f1"), format!("{run}-f2"));
    let mut pictured = agent(&f1);
    pictured.picture_url = Some(format!("{}/__shell/agent.png", api.base));
    let roster = Roster { snapshot: format!("{run}-s8"), observed_ms: 1_800_000_000_000, people: vec![RosterPerson { workos_id: erin_id, email: format!("{run}-erin@old.test"), agents: vec![pictured, agent(&f2)] }] };
    anyhow::ensure!(mover.push(&roster)?.status == 200, "erin's roster");
    let offers = |who: &fragment_nip98::Keys| -> Result<Vec<Value>> {
        let r = api.signed(who, "GET", "/api/notices", None)?;
        anyhow::ensure!(r.status == 200, "notices: {r}");
        Ok(r.body["notices"].as_array().cloned().unwrap_or_default().into_iter().filter(|n| n["kind"] == "move_offer").collect())
    };
    let ids = |n: &[Value]| n.iter().map(|n| n["id"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>();
    let unbound = (state(&f1), state(&f2));
    let first = offers(&erin)?;
    let o = first.first().cloned().unwrap_or_default();
    let labels: Vec<&str> = o["actions"].as_array().map(|a| a.iter().filter_map(|x| x["label"].as_str()).collect()).unwrap_or_default();
    s.ok(
        "a person's first notices read binds them by their WorkOS user: each old agent of theirs is offered, oldest first, a notice that must be answered in the server's words, as its upgrade: Upgrade opening /move, Not now and Never their own routes",
        unbound == ("listed".into(), "listed".into())
            && ids(&first) == [format!("move:{f1}"), format!("move:{f2}")]
            && state(&f1) == "open"
            && o["mustAnswer"] == true
            && o["title"] == format!("Old {f1}'s upgrade is ready")
            && o["text"] == OFFER_TEXT
            && labels == [format!("Upgrade Old {f1}").as_str(), "Not now", "Never"]
            && o["actions"][0]["open"].as_str().is_some_and(|u| u.ends_with("/move"))
            && o["actions"][1]["post"] == format!("/api/migrations/offers/{f1}/later")
            && o["image"] == format!("/api/migrations/offers/{f1}/picture")
            && first[1].get("image").is_none(),
        json!([unbound, first]),
    );

    // its picture, read for them alone
    let pic = api.signed(&erin, "GET", &format!("/api/migrations/offers/{f1}/picture"), None)?;
    let shell_png = api.unsigned("GET", "/__shell/agent.png", None)?;
    let stranger = api.person()?;
    let (none, theirs) = (api.signed(&erin, "GET", &format!("/api/migrations/offers/{f2}/picture"), None)?, api.signed(&stranger, "GET", &format!("/api/migrations/offers/{f1}/picture"), None)?);
    s.ok(
        "its picture is read for its person (the roster's, as an image), kept by no cache (its URL is the same for whoever signs in next); one with none, or another person's, is none (404)",
        pic.status == 200 && pic.header("content-type") == "image/png" && pic.header("cache-control") == "no-store" && !pic.bytes.is_empty() && pic.bytes == shell_png.bytes && none.status == 404 && theirs.status == 404,
        format!("{} {} {:?} {} {}", pic.status, pic.header("content-type"), pic.header("cache-control"), none.status, theirs.status),
    );

    // another's answers: none of theirs
    let at = |o: &str, verb: &str| format!("/api/migrations/offers/{o}/{verb}");
    let (_, stranger_agent) = api.own_agent(&stranger)?;
    let (erin_agent_keys, erin_agent) = api.own_agent(&erin)?;
    let refused = (
        api.signed(&stranger, "POST", &at(&f2, "later"), None)?.status,
        api.signed(&erin_agent_keys, "POST", &at(&f2, "later"), None)?.status,
        api.unsigned("POST", &at(&f2, "later"), None)?.status,
        api.signed(&erin, "POST", &at(&f2, "maybe"), None)?.status,
    );
    s.ok(
        "an offer is its person's alone to answer: another person's answer is none (404), an agent's is refused (403), no one's is 401, and an answer that is none is no route (404); nothing changes",
        refused == (404, 403, 401, 404) && state(&f2) == "open" && offers(&stranger)?.is_empty(),
        format!("{refused:?}"),
    );

    // Not now: gone, told, back a day later (the alarm's change told too)
    let url = format!("{}/api/fragments/watch", api.base);
    let mut tab = crate::lanes::shell::watching(crate::api::Socket::connect(api, &url, Some(&erin), None, None).map(|(socket, _)| socket));
    let later = api.signed(&erin, "POST", &at(&f2, "later"), None)?;
    let told_later = crate::lanes::shell::told(&mut tab);
    let replay = api.signed(&erin, "POST", &at(&f2, "later"), None)?;
    let waiting = ids(&offers(&erin)?);
    later_by(DAY_MS)?;
    let told_back = crate::lanes::shell::told(&mut tab);
    let back = ids(&offers(&erin)?);
    s.ok(
        "Not now: the offer goes, the person's pages are told, the same again changes nothing; a day later it is asked again, and they are told",
        later.status == 200 && later.body == json!({ "state": "later" }) && told_later == json!({ "type": "changed" }) && replay.status == 200 && replay.body["state"] == "later"
            && waiting == [format!("move:{f1}")] && told_back == json!({ "type": "changed" }) && back.contains(&format!("move:{f2}")),
        json!([later.body, told_later, waiting, told_back, back]),
    );

    // a roster after names a new old agent of theirs: offered at once, and told
    let f3 = format!("{run}-f3");
    let mut more = roster.clone();
    more.snapshot = format!("{run}-s9");
    more.people[0].agents.push(agent(&f3));
    let pushed = mover.push(&more)?;
    let told_new = crate::lanes::shell::told(&mut tab);
    let with_new = ids(&offers(&erin)?);
    s.ok(
        "a roster after, naming a new old agent of a person bound already, offers it at once, and their pages are told",
        pushed.status == 200 && pushed.body["bound"] == 1 && told_new == json!({ "type": "changed" }) && with_new.contains(&format!("move:{f3}")),
        json!([pushed.body, told_new, with_new]),
    );

    // Never, then Ask again
    let never = api.signed(&erin, "POST", &at(&f2, "never"), None)?;
    let gone = ids(&offers(&erin)?);
    let again = api.signed(&erin, "POST", &at(&f2, "ask-again"), None)?;
    let asked_again = ids(&offers(&erin)?);
    let open_again = api.signed(&erin, "POST", &at(&f2, "ask-again"), None)?;
    let too_late = api.signed(&erin, "POST", &at(&f1, "ask-again"), None)?;
    s.ok(
        "Never: asked no more, until Ask again, which offers it at once; Ask again of an offer open already changes nothing",
        never.body["state"] == "never" && gone == [format!("move:{f1}"), format!("move:{f3}")] && again.body["state"] == "open" && asked_again.contains(&format!("move:{f2}")) && open_again.status == 200 && too_late.status == 200,
        json!([never.body, gone, again.body, asked_again]),
    );

    // Yes: an agent of the person's own
    let theirs = api.signed(&erin, "POST", &at(&f1, "yes"), Some(&json!({ "agent": stranger_agent })))?;
    let bad = api.signed(&erin, "POST", &at(&f1, "yes"), Some(&json!({ "agent": 7 })))?;
    let yes = api.signed(&erin, "POST", &at(&f1, "yes"), Some(&json!({ "agent": erin_agent })))?;
    let yes_again = api.signed(&erin, "POST", &at(&f1, "yes"), Some(&json!({ "agent": erin_agent })))?;
    let left = ids(&offers(&erin)?);
    let cmd = mover.commands()?.into_iter().find(|c| c.old == f1);
    let later_now = api.signed(&erin, "POST", &at(&f1, "later"), None)?;
    s.ok(
        "Yes with another person's agent is refused (403), a body that is none (400); with their own, the old agent is told to stop and its offer goes; Yes again changes nothing, and Not now after it is refused in the server's words (409)",
        theirs.status == 403 && bad.status == 400 && yes.body["state"] == "asked" && yes_again.status == 200 && left == [format!("move:{f3}"), format!("move:{f2}")] && cmd.is_some_and(|c| c.stage.as_deref() == Some(&format!("{f1}.p1.1"))) && later_now.status == 409 && !later_now.message().is_empty(),
        json!([theirs.status, bad.status, yes.body, left, later_now.body]),
    );
    if let Ok(w) = tab {
        w.close();
    }
    // the object's clock back as it was, for the sections after
    later_by(-DAY_MS)?;
    Ok(())
}

/// The move into a real agent on a computer (the stub image): its routine
/// lands on the agent fragment's own `tasks`, naming its stage, and the
/// agent reads its stage through its computer's egress, signed as itself,
/// then says it is done. What the importer then does with the bytes is
/// the importer's (F8, F9).
pub fn move_import(s: &mut Suite, api: &Api) -> Result<()> {
    use super::computers::{agent_replies, phase, turn_of, AGENT_JSON, CHAT_JSON};
    use super::jobs::records;
    if !s.section("move-import", &[Need::Fakes, Need::Deployment, Need::Levers, Need::Computers, Need::Move]) {
        return Ok(());
    }
    const WAKE: Duration = Duration::from_secs(90);
    let mover = Mover::new(&api.base, s.mover.clone());
    let old = format!("{}-o1", s.name("mi"));
    let owner = api.person()?;
    let workos_id = s.workos.user(&Api::email_of(&owner)).id;
    // a computer, its agent fragment, and a chat the agent is in (as the computers lane makes them)
    let r = api.signed(&owner, "POST", "/api/computers", Some(&json!({})))?;
    let id = r.body["computer"].as_str().unwrap_or("").to_string();
    let agent_name = s.named(api, &owner, "kestrel")?;
    let agent = s.create(api, &owner, &agent_name)?;
    s.commit(&agent, &[("fragment.json", Some(AGENT_JSON))]);
    s.deploy(&agent);
    super::computers::members_only(api, &owner, &agent_name)?;
    let r = api.signed(&owner, "PUT", &format!("/api/computers/{id}/agents/{agent_name}"), Some(&json!({})))?;
    let identity = r.body["agents"][0]["identity"].as_str().unwrap_or("").to_string();
    anyhow::ensure!(r.status == 200 && !identity.is_empty(), "the agent assigned: {r}");
    let chat_name = s.named(api, &owner, "chat")?;
    let chat = s.create(api, &owner, &chat_name)?;
    s.commit(&chat, &[("fragment.json", Some(CHAT_JSON))]);
    s.deploy(&chat);
    let r = api.signed(&owner, "PUT", &format!("/api/f/{chat_name}/members/{identity}"), Some(&json!({ "role": "editor" })))?;
    anyhow::ensure!(r.status == 200, "the agent joins its chat: {r}");
    let woke = s.eventually(WAKE, || phase(api, &owner, &id) == "awake");
    anyhow::ensure!(woke, "the computer wakes: {}", phase(api, &owner, &id));

    // the move: Yes with this agent, staged
    // an old agent with all three secrets, though its roster says none: Yes hands each to its owner's Vault
    let people = vec![person(&workos_id, &[&old])];
    let r = mover.push(&Roster { snapshot: format!("{old}-s1"), observed_ms: 1_800_000_000_000, people })?;
    anyhow::ensure!(r.status == 200, "the roster: {} {}", r.status, r.body);
    let visit = api.unsigned("POST", "/api/test/migrations", Some(&json!({ "op": "visit", "who": Api::email_of(&owner) })))?;
    anyhow::ensure!(visit.status == 200 && visit.body["bound"] == 1, "its owner opens the move, and is bound: {visit}");
    let seat = || api.signed(&owner, "GET", "/api/seat", None).map(|r| r.body["seat"].clone()).unwrap_or(Value::Null);
    let comped = seat();
    let yes = api.unsigned("POST", "/api/test/migrations", Some(&json!({ "op": "answer", "old": old, "answer": "yes", "agent": identity })))?;
    anyhow::ensure!(yes.status == 200, "Yes: {yes}");
    let tag = crate::Keys::generate().pubkey_hex()[..16].to_string();
    let (token, key) = (format!("123456:tg-sentinel-{tag}"), format!("sk-or-v1-sentinel-{tag}"));
    const NSEC: &str = "nsec1vl029mgpspedva04g90vltkh6fvh240zqtv9k0t9af8935ke9laqsnlfe5";
    let item = |kind, value: &str| HandoffItem { kind, value: value.into() };
    let all = vec![item(SecretKind::TelegramBotToken, &token), item(SecretKind::OpenrouterKey, &key), item(SecretKind::NostrNsec, NSEC)];
    let cmd = mover.commands()?.into_iter().find(|c| c.old == old).context("its stop_and_stage")?;
    let stopped = mover.report(&cmd, event("stopped"))?;
    anyhow::ensure!(stopped.status == 200, "stopped: {} {}", stopped.status, stopped.body);
    let vault = || -> Result<Value> { Ok(api.signed(&owner, "GET", "/api/vault", None)?.body) };
    let entry = |v: &Value, name: &str| v["secrets"].as_array().and_then(|l| l.iter().find(|e| e["name"] == name)).cloned().unwrap_or_default();
    let name_of = |kind| fragment_core::migrations::vault_secret(kind, &agent_name, NSEC).map(|(n, _)| n).unwrap_or_default();
    let (tg_name, or_name, ns_name) = (name_of(SecretKind::TelegramBotToken), name_of(SecretKind::OpenrouterKey), name_of(SecretKind::NostrNsec));
    let mine = json!([agent_name]);

    // a hand-off cut short after its first secret: that one is in the Vault
    let first = mover.hand_off(&old, &cmd.id, vec![item(SecretKind::TelegramBotToken, &token)])?;
    let v = vault()?;
    let tg = entry(&v, &tg_name);
    s.ok(
        "Yes hands the old agent's Telegram token to its person's Vault once it is stopped: toward api.telegram.org, bound to the new agent alone, its value listed nowhere",
        cmd.secrets == SecretKind::ALL && first.status == 200 && first.body == json!({ "handed": ["telegram_bot_token"] }) && tg["hosts"] == json!(["api.telegram.org"]) && tg["agents"] == mine && !v.to_string().contains(&token),
        format!("{} {} | {v}", first.status, first.body),
    );
    // its person removes it, and keeps a secret of another agent's under the
    // name OpenRouter's would take; then the mover tries the whole hand-off again
    let removed = api.signed(&owner, "DELETE", &format!("/api/vault/{tg_name}"), None)?;
    let theirs = api.signed(&owner, "PUT", &format!("/api/vault/{or_name}"), Some(&json!({ "value": "their-own-key", "hosts": ["example.com"], "agents": ["other--x7k2"] })))?;
    anyhow::ensure!(removed.status == 200 && theirs.status == 200, "the owner's own edits: {removed} {theirs}");
    let again = mover.hand_off(&old, &cmd.id, all.clone())?;
    let v = vault()?;
    let (ns, or) = (entry(&v, &ns_name), entry(&v, &or_name));
    s.ok(
        "tried again whole, it puts what it had not (the Nostr key, to sign with toward finite.site), never puts back the token its person removed, and never takes the name of another agent's secret (OpenRouter's left out, theirs untouched)",
        again.status == 200
            && again.body == json!({ "handed": ["telegram_bot_token", "nostr_nsec"] })
            && entry(&v, &tg_name).is_null()
            && ns["hosts"] == json!([fragment_core::migrations::NOSTR_KEY_HOST]) && ns["place"] == json!({ "sign": "nip98" }) && ns["agents"] == mine
            && or["agents"] == json!(["other--x7k2"]) && or["hosts"] == json!(["example.com"])
            && ![token.as_str(), key.as_str(), NSEC].iter().any(|x| v.to_string().contains(x)),
        format!("{} {} | {v}", again.status, again.body),
    );
    let replay = mover.hand_off(&old, &cmd.id, all.clone())?;
    let after = vault()?;
    s.ok("the same hand-off again answers the same and changes nothing", replay.status == 200 && replay.body == again.body && after == v, format!("{} {}", replay.status, replay.body));
    // its stage carries its owner's SimpleX chat
    mover.simplex(true);
    mover.run_once()?;
    mover.simplex(false);
    let on = s.eventually(crate::lanes::billing::PUSHED, || seat()["kind"] == "seat_always_on");
    s.ok(
        "its stage carries its SimpleX chat: its owner's comped `seat`, which sleeps, is made always-on as it completes, which SimpleX needs",
        comped["kind"] == "seat" && comped["comped"] == true && on && seat()["comped"] == true,
        json!([comped, seat()]),
    );
    let late = mover.hand_off(&old, &cmd.id, all)?;
    s.ok("once its stage is staged, its command is done: a hand-off is stale (409)", late.status == 409, format!("{} {}", late.status, late.body));
    let logged = super::vault::logs(s);
    s.ok("no value handed off is in any node's log", ![token.as_str(), key.as_str(), NSEC].iter().any(|v| logged.contains(v)), "a value was logged");
    let stage = format!("{old}.p1.1");
    let agent_npub = agent["npub"].as_str().unwrap_or("").to_string();
    let routine = || records(api, &owner, &agent_name, "tasks").into_iter().find(|r| r["body"]["kind"] == "routine" && r["body"]["text"].as_str().is_some_and(|t| t.contains(&stage)));
    let landed = s.eventually(Duration::from_secs(30), || routine().is_some());
    let r = routine().unwrap_or_default();
    s.ok(
        "staged: the import's routine lands on the agent's own tasks, as the agent fragment itself, with no chat, naming its stage and the skill",
        landed && r["principal"] == agent_npub.as_str() && r["body"]["chat"].is_null() && r["body"]["text"].as_str().is_some_and(|t| t.contains("import-agent-home")),
        &r,
    );

    // the agent reads its stage through its computer's egress, signed as itself
    let say = |n: u32, text: &str| api.signed(&owner, "POST", &format!("/api/f/{chat_name}/channels/chat"), Some(&json!({ "id": format!("mv{n}"), "body": { "text": text } })));
    let fetched = |s: &Suite, n: u32, text: &str| -> Result<String> {
        let r = say(n, text)?;
        let turn = turn_of(&agent_name, &chat_name, "chat", r.body["record"]["seq"].as_i64().unwrap_or(0));
        let reply = || agent_replies(&records(api, &owner, &chat_name, "chat"), &identity).into_iter().find(|r| r["body"]["turn"] == turn.as_str());
        s.eventually(WAKE, || reply().is_some());
        Ok(reply().and_then(|r| r["body"]["text"].as_str().map(str::to_string)).unwrap_or_default())
    };
    let base = format!("http://api.fragment.internal/api/migrations/stages/{stage}");
    let opened = fetched(s, 1, &format!("fetch {base}/open with {agent_name} in x-fragment-agent posting {{}}"))?;
    let zip = fetched(s, 2, &format!("fetch {base}/zip with {agent_name} in x-fragment-agent"))?;
    s.ok("the agent opens its stage and reads its zip through its own egress (NIP-98 as itself)", opened.starts_with("fetched 200:") && opened.contains(STAGE_FORMAT) && zip.starts_with("fetched 200:"), format!("{opened} | {}", zip.chars().take(40).collect::<String>()));

    // the old agent's Nostr key, handed off, signs its sites' calls at finite.site (the agent brings them over itself)
    let site = fragment_core::migrations::NOSTR_KEY_HOST;
    let tag = fragment_core::swap::TagKey::derive(&s.host_secret).tag(&id, &agent_name, &ns_name);
    let signer = fragment_core::swap::Placeholder::new(fragment_core::catalog::Kind::Own, &ns_name, &tag).text();
    let given = fetched(s, 5, "credentials")?;
    let before = s.upstream.seen().len();
    let listed = fetched(s, 6, &format!("fetch http://{site}/api/v1/sites with {signer}"))?;
    let seen = s.upstream.seen().get(before..).and_then(|l| l.last().cloned()).unwrap_or_default();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64;
    let old_key = fragment_nip98::Keys::from_secret_hex(&fragment_core::npub::secret_hex(NSEC).context("NIP-19's example key")?).context("a key")?;
    let header = seen["auth"]["authorization"].as_str().unwrap_or_default().to_string();
    let by = fragment_nip98::verify(Some(&header), "GET", &format!("https://{site}/api/v1/sites"), b"", now, 120);
    s.ok(
        "the new agent's view lists the handed-off Nostr key as its own placeholder, and its egress catches finite.site: a call there reaches it signed (NIP-98) by the old agent's key, never the key",
        given.contains(&signer) && listed.starts_with("fetched 200") && seen["host"] == site && by.as_deref() == Ok(old_key.pubkey_hex()) && !listed.contains(&old_key.secret_hex()) && !header.contains(&old_key.secret_hex()),
        json!({ "given": given, "said": listed, "seen": seen, "signer": format!("{by:?}") }),
    );
    // a git remote's own basic auth, as finite.site mints it, carries no placeholder: the Vault signs nothing over it
    let minted = format!("fs-minted-{tag}");
    let before = s.upstream.seen().len();
    let cloned = fetched(s, 7, &format!("fetch http://{site}/a/b.git/info/refs?service=git-upload-pack with {minted} as basic git"))?;
    let seen = s.upstream.seen().get(before..).and_then(|l| l.last().cloned()).unwrap_or_default();
    let basic = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("git:{minted}")));
    s.ok(
        "a request to finite.site that carries no placeholder (a git clone's minted basic auth) goes on untouched: its own Authorization, no signature",
        cloned.starts_with("fetched 200") && seen["host"] == site && seen["auth"]["authorization"] == basic.as_str() && seen["target"] == "/a/b.git/info/refs?service=git-upload-pack",
        json!({ "said": cloned, "seen": seen }),
    );
    let done = fetched(s, 3, &format!("fetch {base}/done with {agent_name} in x-fragment-agent posting {{\"ok\":true}}"))?;
    let after = fetched(s, 4, &format!("fetch {base}/zip with {agent_name} in x-fragment-agent"))?;
    s.ok("it says it is done: the move waits for staff's retire, and its stage's routes refuse at once", done.starts_with("fetched 200:") && done.contains("retiring") && after.starts_with("fetched 409:"), format!("{done} | {after}"));
    mover.run_once()?;
    Ok(())
}

/// The offer in the shell (F4), in Chrome: a person on the roster signs in
/// to a page that asks them first, over everything (a guest's welcome too),
/// the old agent's name, picture and words, Yes the main answer opening
/// `/move` here; Not now answered there, the page is theirs. Its pictures,
/// at 1280 and 390 px, light and dark.
pub fn move_ui(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("move-ui", &[Need::Fakes, Need::Deployment, Need::Levers, Need::Chrome, Need::LocalDocker, Need::Computers, Need::Move]) {
        return Ok(());
    }
    let Some(mut b) = s.browser()? else {
        s.ok("Chrome is installed for the move's page (set CHROME_BIN)", false, "no Chrome found");
        return Ok(());
    };
    let shots = s.dir("move-ui");
    let wait = Duration::from_secs(30);
    let mover = Mover::new(&api.base, s.mover.clone());
    let run = s.name("mu");
    let email = format!("{run}-juniper@e2e.test");
    let session = api.sign_in(&email)?;
    let workos_id = s.workos.user(&email).id;
    let old = format!("{run}-a");
    // a guest, as someone from finite.computer arrives (this fleet's default
    // plan is a seat, whose shell would make a first agent of its own)
    let id = super::signin::who(api, &session)?["id"].as_str().context("their identity")?.to_string();
    let made_guest = api.signed(&s.operator, "POST", &format!("/api/ledger/{id}/plan"), Some(&json!({ "id": format!("{old}-guest"), "plan": "guest" })))?;
    anyhow::ensure!(made_guest.status == 200, "a guest: {made_guest}");
    let mut a = agent(&old);
    a.name = "Juniper".into();
    a.picture_url = Some(format!("{}/__shell/agent.png", api.base));
    let roster = Roster { snapshot: format!("{run}-ui"), observed_ms: 1_800_000_000_000, people: vec![RosterPerson { workos_id, email: format!("{run}@old.test"), agents: vec![a] }] };
    anyhow::ensure!(mover.push(&roster)?.status == 200, "the roster");
    b.set_cookie(&format!("{}/", api.base), "fragment_session", &session)?;
    let id = format!("move:{old}");
    let shown = format!("document.getElementById('must-answer').dataset.id === {}", serde_json::to_string(&id)?);
    for (width, height, phone) in [(1280, 800, false), (390, 844, true)] {
        for scheme in ["light", "dark"] {
            let page = b.open(&format!("{}/", api.base))?;
            b.viewport(&page, width, height, phone)?;
            b.color_scheme(&page, scheme)?;
            let asked = b.until(&page, &shown, wait) && b.until(&page, "document.querySelector('#must-answer img')?.naturalWidth > 0", wait);
            std::thread::sleep(Duration::from_millis(300));
            let _ = b.screenshot(&page, &shots.join(format!("offer-{width}-{scheme}.png")));
            if width == 1280 && scheme == "light" {
                let seen = b.eval(&page, crate::lanes::shell::MUST_ANSWER_SEEN)?;
                let yes = b.eval(&page, "(() => { const a = document.querySelector('#must-answer a'); return { text: a?.textContent, href: a?.getAttribute('href'), target: a?.target ?? '', main: a?.classList.contains('primary') }; })()")?;
                s.ok(
                    "a person on the roster is asked as their page opens, over everything: the old agent's name, picture and words, its upgrade (no free month), Upgrade Juniper the main answer opening /move here, then Not now and Never",
                    asked && seen["covers"] == true && seen["words"].as_str().is_some_and(|w| w.contains("Juniper's upgrade is ready") && w.contains(OFFER_TEXT) && w.contains("Not now") && w.contains("Never") && !promises_a_month(w))
                        && yes["text"] == "Upgrade Juniper" && yes["href"].as_str().is_some_and(|h| h.ends_with("/move")) && yes["target"] == "" && yes["main"] == true,
                    json!([seen, yes]),
                );
            }
            if width == 390 && scheme == "dark" {
                b.eval(&page, "([...document.querySelectorAll('#must-answer button')].find((x) => x.textContent === 'Not now').click(), true)")?;
                let gone = b.until(&page, "!document.getElementById('must-answer').open", wait);
                s.ok("Not now: the offer goes, and the page is theirs", gone, b.eval(&page, crate::lanes::shell::MUST_ANSWER_SEEN)?);
            }
            b.close(page)?;
        }
    }
    move_page_ui(s, api, &mut b, &mover, &session, &old, &shots)?;
    move_into_theirs(s, api, &mut b, &mover, &roster.people, &shots)?;
    println!("      (screenshots: {})", shots.display());
    Ok(())
}

/// F5, the move's comp: a person on the roster who opens the move is given
/// a comped seat with no end, once, ever: opened again, after the node
/// restarts, nothing more. No credit is granted: the seat brings its
/// month's included credit. One who holds a seat already keeps theirs.
/// Staff end a move's comp as any comp (`DELETE /api/admin/seats/{id}`,
/// `fragment operator uncomp`); an operator's comp with an end (`untilMs`)
/// ends by itself then.
pub fn move_comp(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("move-comp", &[Need::Fakes, Need::Deployment, Need::Levers, Need::Node, Need::Move]) {
        return Ok(());
    }
    let mover = Mover::new(&api.base, s.mover.clone());
    let run = s.name("mc");
    let visit = |api: &Api, who: &fragment_nip98::Keys| api.unsigned("POST", "/api/test/migrations", Some(&json!({ "op": "visit", "who": Api::email_of(who) })));
    let ledger = |api: &Api, k: &fragment_nip98::Keys| api.signed(k, "GET", "/api/ledger", None).map(|r| r.body).unwrap_or(Value::Null);
    let purchased = |api: &Api, k: &fragment_nip98::Keys| ledger(api, k)["purchasedMicros"].as_i64().unwrap_or(-1);
    let seat = |api: &Api, k: &fragment_nip98::Keys| api.signed(k, "GET", "/api/seat", None).map(|r| r.body).unwrap_or(Value::Null);
    // when each comped seat of theirs ends by itself (null: never), which no route shows
    let ends = |api: &Api, k: &fragment_nip98::Keys| api.unsigned("POST", "/api/test/registry", Some(&json!({ "compEnds": Api::email_of(k) }))).map(|r| r.body["ends"].clone()).unwrap_or(Value::Null);
    let gina = api.person()?;
    // a guest, as someone from finite.computer arrives (this fleet's default plan is a seat)
    let made_guest = api.signed(&s.operator, "POST", &format!("/api/ledger/{}/plan", api.identity(&gina)?), Some(&json!({ "id": format!("{run}-guest"), "plan": "guest" })))?;
    anyhow::ensure!(made_guest.status == 200, "gina a guest: {made_guest}");
    let hank = api.person()?;
    let comped = api.signed(&s.operator, "POST", "/api/admin/seats", Some(&json!({ "email": Api::email_of(&hank), "kind": "seat_always_on" })))?;
    anyhow::ensure!(comped.status == 200, "hank's own seat: {comped}");
    let people = [(&gina, "g1"), (&hank, "h1")].map(|(k, o)| RosterPerson { workos_id: s.workos.user(&Api::email_of(k)).id, email: format!("{run}-{o}@old.test"), agents: vec![agent(&format!("{run}-{o}"))] });
    anyhow::ensure!(mover.push(&Roster { snapshot: format!("{run}-s1"), observed_ms: 1_800_000_000_000, people: people.to_vec() })?.status == 200, "the roster");
    let before = (purchased(api, &gina), seat(api, &gina)["guest"].clone());
    let first = visit(api, &gina)?;
    let given = s.eventually(crate::lanes::billing::PUSHED, || seat(api, &gina)["guest"] == false && ledger(api, &gina)["includedGrantedMicros"] == 50_000_000);
    let mine = seat(api, &gina);
    s.ok(
        "a guest on the roster who opens the move is given its comp: a comped `seat` with no end, its own month's credit ($50 included) and no credit granted",
        before == (0, json!(true)) && first.status == 200 && first.body["comp"] == true && first.body.get("untilMs").is_none() && given
            && mine["seat"]["kind"] == "seat" && mine["seat"]["comped"] == true && ends(api, &gina) == json!([null]) && purchased(api, &gina) == 0,
        json!([before, first.body, mine, ends(api, &gina), ledger(api, &gina)]),
    );
    let again = visit(api, &gina)?;
    s.ok("opened again: nothing more (no comp due)", again.status == 200 && again.body["comp"] == false && purchased(api, &gina) == 0 && ends(api, &gina) == json!([null]), &again.body);
    let hanks = visit(api, &hank)?;
    let kept = seat(api, &hank);
    s.ok(
        "one who holds a seat already keeps theirs, as it was (its kind, no end), and is given nothing more",
        hanks.status == 200 && hanks.body["comp"] == true && kept["seat"]["kind"] == "seat_always_on" && kept["seat"]["comped"] == true && ends(api, &hank) == json!([null])
            && purchased(api, &hank) == 0 && visit(api, &hank)?.body["comp"] == false,
        json!([hanks.body, kept, ends(api, &hank)]),
    );
    // restarted: what was given is remembered
    s.stop()?;
    let api = &s.start(false)?;
    let after = (visit(api, &gina)?, visit(api, &hank)?);
    s.ok(
        "the node restarted: opened again, nothing more is given, to either, and the move's seat still has no end",
        after.0.status == 200 && after.0.body["comp"] == false && after.1.body["comp"] == false && purchased(api, &gina) == 0 && purchased(api, &hank) == 0
            && seat(api, &gina)["seat"]["comped"] == true && ends(api, &gina) == json!([null]),
        json!([after.0.body, after.1.body, ends(api, &gina)]),
    );
    // a visit cut off at its seat: the next gives it, with no end
    let joy = api.person()?;
    let joy_people = vec![RosterPerson { workos_id: s.workos.user(&Api::email_of(&joy)).id, email: format!("{run}-j1@old.test"), agents: vec![agent(&format!("{run}-j1"))] }];
    anyhow::ensure!(mover.push(&Roster { snapshot: format!("{run}-s2"), observed_ms: 1_800_000_000_000, people: [people.to_vec(), joy_people].concat() })?.status == 200, "the roster");
    let down = api.unsigned("POST", "/api/test/registry", Some(&json!({ "downFor": { "path": "/orgs/comp", "times": 1 } })))?;
    anyhow::ensure!(down.status == 200, "the registry's next comp refused: {down}");
    let cut = visit(api, &joy)?;
    let retried = visit(api, &joy)?;
    let joys = seat(api, &joy);
    s.ok(
        "a visit cut off at its seat is tried again by the next, which gives it with no end; after it, nothing more",
        cut.status != 200 && retried.status == 200 && retried.body["comp"] == true && joys["seat"]["comped"] == true && ends(api, &joy) == json!([null])
            && purchased(api, &joy) == 0 && visit(api, &joy)?.body["comp"] == false,
        json!([cut.status, retried.body, joys, ends(api, &joy)]),
    );
    // staff end a move's comp as any other (`fragment operator uncomp`)
    let joy_seat = joys["seat"]["id"].as_str().unwrap_or_default().to_string();
    let uncomped = api.signed(&s.operator, "DELETE", &format!("/api/admin/seats/{joy_seat}"), None)?;
    let ended = s.eventually(crate::lanes::billing::PUSHED, || seat(api, &joy)["seat"].is_null());
    s.ok("staff end a move's comp at once, as any comp", uncomped.status == 200 && ended && ends(api, &joy) == json!([]) && visit(api, &joy)?.body["comp"] == false, json!([uncomped.body, seat(api, &joy)]));
    // an operator's comp with an end ends then, by itself (the registry's alarm)
    let ivy = api.person()?;
    let soon = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis() as i64 + 3_000;
    let r = api.signed(&s.operator, "POST", "/api/admin/seats", Some(&json!({ "email": Api::email_of(&ivy), "kind": "seat", "untilMs": soon })))?;
    let held = ends(api, &ivy) == json!([soon]) && seat(api, &ivy)["seat"]["kind"] == "seat";
    let ended = s.eventually(Duration::from_secs(30), || seat(api, &ivy)["seat"].is_null());
    s.ok("an operator's comp with an end holds until then, and then ends by itself", r.status == 200 && held && ended, seat(api, &ivy));
    Ok(())
}

/// F6, the move's page, in Chrome: the offer's Yes opens `/move` in place,
/// the cover standing aside there; the one question (which, by what
/// name); Bring over makes the guest's comp, then a new agent (its chat not
/// opened), then posts its Yes; then the move as the server says it goes:
/// stopping, then (the fake mover staging it) its import begun, which takes
/// the person to the new agent's own chat (FIN-463: the agent tells its
/// move there, and asks there). The page opened again then says it is
/// being brought over, a tap from that chat. Pictures at 1280 and 390 px,
/// light and dark.
fn move_page_ui(s: &mut Suite, api: &Api, b: &mut crate::browser::Browser, mover: &Mover, session: &str, old: &str, shots: &std::path::Path) -> Result<()> {
    let wait = Duration::from_secs(30);
    let agent_wait = Duration::from_secs(180);
    // answered Not now above: Never, then Ask again, through their own routes, offers it again now
    for verb in ["never", "ask-again"] {
        let r = super::shell::shell(api, session, "POST", &format!("/api/migrations/offers/{old}/{verb}"), Some(&json!({})), &[])?;
        anyhow::ensure!(r.status == 200, "{verb}: {r}");
    }
    let row = format!("document.querySelector('#move-page .move-row[data-old={}]')", serde_json::to_string(old)?);
    let said = format!("({row}?.querySelector('p')?.textContent ?? '')");
    let page = b.open(&format!("{}/", api.base))?;
    b.viewport(&page, 1280, 800, false)?;
    b.color_scheme(&page, "light")?;
    anyhow::ensure!(b.until(&page, "document.getElementById('must-answer').open && !!document.querySelector('#must-answer a')", wait), "the offer, over the page");
    b.eval(&page, "(document.querySelector('#must-answer a').click(), true)")?;
    let opened = b.until(&page, &format!("location.pathname === '/move' && !!{row} && !document.getElementById('must-answer').open"), wait);
    let seen = b.eval(&page, &format!("(() => {{ const r = {row}; return {{ words: r?.innerText, page: document.getElementById('move-page')?.innerText, picked: r?.querySelector('input[type=checkbox]')?.checked, bring: !!document.querySelector('#move-page button.primary'), cover: document.getElementById('must-answer').open, asked: !!document.querySelector('#move-page .move-into') }}; }})()"))?;
    s.ok(
        "Upgrade Juniper opens /move here: the old agent, its words, and whether to upgrade it (chosen), no free month; the offer's cover stands aside on the page that answers it; with no agent here, nothing more is asked",
        opened && seen["words"].as_str().is_some_and(|w| w.contains("Juniper's upgrade is ready.")) && seen["page"].as_str().is_some_and(|w| !w.is_empty() && !promises_a_month(w)) && seen["picked"] == true && seen["bring"] == true && seen["cover"] == false && seen["asked"] == false,
        &seen,
    );
    let _ = b.screenshot(&page, &shots.join("move-1280-light.png"));
    let mut picked = vec![check_by_keys(b, &page, shots, 1280, "light")?];
    for (width, height, phone, scheme) in [(1280, 800, false, "dark"), (390, 844, true, "light"), (390, 844, true, "dark")] {
        let p = b.open(&format!("{}/move", api.base))?;
        b.viewport(&p, width, height, phone)?;
        b.color_scheme(&p, scheme)?;
        let _ = b.until(&p, &format!("!!{row}?.querySelector('img')?.naturalWidth"), wait);
        std::thread::sleep(Duration::from_millis(300));
        let _ = b.screenshot(&p, &shots.join(format!("move-{width}-{scheme}.png")));
        picked.push(check_by_keys(b, &p, shots, width, scheme)?);
        b.close(p)?;
    }
    let held = |c: &Value| {
        c["drawn"] == "none" && c["off"] == json!({ "checked": false, "focused": true, "ring": true, "bring": false }) && c["on"] == json!({ "checked": true, "focused": true, "ring": true, "bring": true }) && c["fill"] != c["empty"]
    };
    s.ok(
        "at 1280 and 390 px, light and dark, the box is the platform's check, not the browser's: reached by Tab, Space unchecks it and checks it again, its focus ring kept, Upgrade following each",
        picked.len() == 4 && picked.iter().all(held),
        json!(picked),
    );

    // the routines on the person's one agent's `tasks` (its hello is one),
    // and the names of their agents and chats
    let mine = || -> Result<(Vec<Value>, Vec<String>, Vec<String>)> {
        let list = super::shell::shell(api, session, "GET", "/api/fragments", None, &[])?;
        let named = |kind: &str| -> Vec<String> { list.body["fragments"].as_array().into_iter().flatten().filter(|f| f["kind"] == kind).filter_map(|f| f["name"].as_str().map(str::to_string)).collect() };
        let (agents, chats) = (named("agent"), named("chat"));
        let routines = match agents.as_slice() {
            [agent] => super::shell::shell(api, session, "GET", &format!("/api/f/{agent}/channels/tasks"), None, &[])?.body["records"].as_array().cloned().unwrap_or_default().into_iter().filter(|r| r["body"]["kind"] == "routine").collect(),
            _ => vec![],
        };
        Ok((routines, agents, chats))
    };
    let under_way = |r: &Value| r["body"]["text"].as_str().is_some_and(|t| t.contains("your upgrade is under way"));

    // Upgrade, its Yes never landing (the page's own call for it refused,
    // so the server never hears it; FIN-817): its new agent is made, in its
    // chat, and nothing tells its person their upgrade is under way
    b.eval(&page, "(() => { let n = 1; const f = window.fetch; window.fetch = (u, o) => (String(u).endsWith('/yes') && n-- > 0 ? Promise.reject(new TypeError('Failed to fetch')) : f(u, o)); return true; })()")?;
    b.eval(&page, "(document.querySelector('#move-page button.primary').click(), true)")?;
    let making = b.until(&page, &format!("/^Preparing Juniper's upgrade/.test({said})"), agent_wait);
    let _ = b.screenshot(&page, &shots.join("move-making-1280-light.png"));
    let refused = b.until(&page, &format!("!!document.querySelector('#move-page .form-error') && !document.querySelector('#move-page button.primary').disabled && {row}?.dataset.state === 'open'"), agent_wait);
    let (routines, agents, chats) = mine()?;
    s.ok(
        "Upgrade whose Yes never lands: its new agent is made, in its one chat, the page says so, to try again; the move still open, and its chat says nothing of an upgrade under way (no hello)",
        refused && agents.len() == 1 && chats.len() == 1 && routines.is_empty(),
        json!({ "agents": agents, "chats": chats, "routines": routines, "error": b.eval(&page, "document.querySelector('#move-page .form-error')?.textContent")? }),
    );

    // Upgrade again: the same agent, its Yes, then its hello
    b.eval(&page, "(document.querySelector('#move-page button.primary').click(), true)")?;
    let asked = b.until(&page, &format!("{row}?.dataset.state === 'asked'"), agent_wait);
    let (routines, again, _) = mine()?;
    s.ok(
        "Upgrade again goes on with that agent, never a second: its Yes lands, and only then its hello, the move's, said once",
        asked && again == agents && routines.len() == 1 && routines.iter().all(under_way),
        json!({ "agents": again, "routines": routines }),
    );
    let moves = api.signed(&s.operator, "GET", "/api/admin/migrations", None)?;
    let m = moves.body["moves"].as_array().and_then(|ms| ms.iter().find(|m| m["old"] == old)).cloned().unwrap_or_default();
    let ledger = super::shell::shell(api, session, "GET", "/api/ledger", None, &[])?;
    let seat = super::shell::shell(api, session, "GET", "/api/seat", None, &[])?;
    let still = b.eval(&page, "location.pathname")?;
    let list = super::shell::shell(api, session, "GET", "/api/fragments", None, &[])?;
    let kinds: Vec<String> = list.body["fragments"].as_array().map(|l| l.iter().filter_map(|f| f["kind"].as_str().map(str::to_string)).collect()).unwrap_or_default();
    let one_each = kinds.iter().filter(|k| *k == "agent").count() == 1 && kinds.iter().filter(|k| *k == "chat").count() == 1;
    let its_chat = list.body["fragments"].as_array().and_then(|l| l.iter().find(|f| f["kind"] == "chat")).and_then(|f| f["name"].as_str()).unwrap_or_default().to_string();
    let shown_chat = format!(
        "(location.pathname === '/' && document.getElementById('move-page').hidden && new URLSearchParams((document.querySelector('#frames iframe:not([hidden])')?.dataset.src ?? '?').split('?')[1]).get('name') === {})",
        serde_json::to_string(&its_chat)?
    );
    let _ = b.screenshot(&page, &shots.join("move-asked-1280-light.png"));
    s.ok(
        "Upgrade: the guest is given the comp (a comped seat, no credit granted), one new agent is made, with its one chat, not opened (the page stays), and its Yes starts the move: pausing it, in the server's words",
        making && asked && still == "/move" && one_each && b.eval(&page, &said)?.as_str().is_some_and(|w| w == "Pausing Juniper for its upgrade…") && m["state"] == "asked" && m["agent"].is_string()
            && ledger.body["purchasedMicros"] == 0 && seat.body["seat"]["comped"] == true,
        json!({ "move": m, "ledger": ledger.body["purchasedMicros"], "seat": seat.body["seat"], "path": still, "kinds": kinds }),
    );
    // the mover stops it and stages it: the page follows, read again, and
    // once its import begins the person is in the new agent's own chat
    mover.run_once()?;
    let arrived = b.until(&page, &shown_chat, Duration::from_secs(60));
    std::thread::sleep(Duration::from_millis(300));
    let _ = b.screenshot(&page, &shots.join("move-arrived-1280-light.png"));
    s.ok(
        "its import begun while the page shows, the person is taken to the new agent's own chat, where it says how the move goes and asks what it needs",
        arrived,
        json!({ "path": b.eval(&page, "location.pathname")?, "chat": its_chat }),
    );
    b.close(page)?;
    let open_chat = format!("[...({row}?.querySelectorAll('button') ?? [])].map((b) => b.textContent)");
    for (width, height, phone, scheme) in [(1280, 800, false, "light"), (1280, 800, false, "dark"), (390, 844, true, "light"), (390, 844, true, "dark")] {
        let p = b.open(&format!("{}/move", api.base))?;
        b.viewport(&p, width, height, phone)?;
        b.color_scheme(&p, scheme)?;
        let importing = b.until(&p, &format!("{said} === 'Upgrading Juniper…' && {open_chat}.includes('Open chat')"), wait);
        std::thread::sleep(Duration::from_millis(300));
        let _ = b.screenshot(&p, &shots.join(format!("move-importing-{width}-{scheme}.png")));
        if (width, scheme) == (1280, "light") {
            s.ok(
                "come back to the page as it imports: it stays, says it is being upgraded, and its row opens the new agent's chat",
                importing && b.eval(&p, "location.pathname")? == "/move",
                json!({ "said": b.eval(&p, &said)?, "buttons": b.eval(&p, &open_chat)? }),
            );
            // FIN-463: where its SimpleX chat is kept, said once, calmly
            let simplex = b.eval(&p, "[...document.querySelectorAll('#move-page .move-simplex')].map((p) => p.textContent)")?;
            s.ok("as it imports, one line says its SimpleX chat, if it had one, is kept in Settings", simplex == json!(["If you chatted with it on SimpleX, keep that chat in Settings."]), &simplex);
        }
        b.eval(&p, &format!("([...{row}.querySelectorAll('button')].find((b) => b.textContent === 'Open chat').click(), true)"))?;
        let tapped = b.until(&p, &shown_chat, wait);
        std::thread::sleep(Duration::from_millis(500));
        let _ = b.screenshot(&p, &shots.join(format!("move-chat-{width}-{scheme}.png")));
        if (width, scheme) == (1280, "light") {
            s.ok("its Open chat is the new agent's own chat", tapped, b.eval(&p, "location.pathname")?);
        }
        b.close(p)?;
    }
    Ok(())
}

/// Paul 11-19: someone who moved by hand has an agent here already, and
/// Upgrade made them a second. On `/move` they are asked, for each old
/// agent, a new one or theirs, nothing chosen for them (pictures at 1280
/// and 390 px, light and dark). A Yes that never landed: a reload asks
/// again, and no agent is made. Theirs chosen, its Yes alone binds it:
/// nothing made, nothing of its self changed; while it may still import,
/// it is no one else's choice, and the server refuses it; once the move
/// imports, the routine is on its own tasks and the person in its own chat.
fn move_into_theirs(s: &mut Suite, api: &Api, b: &mut crate::browser::Browser, mover: &Mover, before: &[RosterPerson], shots: &std::path::Path) -> Result<()> {
    let wait = Duration::from_secs(30);
    let run = s.name("mt");
    let email = format!("{run}-rook@e2e.test");
    let session = api.sign_in(&email)?;
    let workos_id = s.workos.user(&email).id;
    let call = |method: &'static str, path: &str, body: Option<Value>| super::shell::shell(api, &session, method, path, body.as_ref(), &[]);
    // their agent, Pip, as the shell's Add makes one: its fragment and its
    // self, on a computer, with its own chat
    let made = call("POST", "/api/fragments", Some(json!({ "label": "pip", "template": "agent", "title": "Pip" })))?;
    let pip = made.body["name"].as_str().context("Pip's fragment")?.to_string();
    let pip_npub = made.body["npub"].as_str().unwrap_or_default().to_string();
    let own: [(&str, &str); 4] = [
        ("SOUL.md", "You are Rook's assistant. Keep answers short.\n"),
        ("memories/MEMORY.md", "Rook takes tea at four.\n"),
        ("skills/home/brew/SKILL.md", "---\nname: brew\ndescription: How Rook likes tea.\n---\n\nSteep it four minutes.\n"),
        ("agent.json", "{\n  \"tier\": \"cheap\"\n}\n"),
    ];
    let w = call("POST", &format!("/api/f/{pip}/files"), Some(json!({ "key": "pip-self", "message": "its self", "files": own.iter().map(|(p, t)| json!({ "path": p, "text": t })).collect::<Vec<_>>() })))?;
    anyhow::ensure!(w.status == 200, "Pip's self: {w}");
    let computer = call("POST", "/api/computers", Some(json!({})))?.body["computer"].as_str().context("their computer")?.to_string();
    let placed = call("PUT", &format!("/api/computers/{computer}/agents/{pip}"), Some(json!({})))?;
    let identity = placed.body["agents"].as_array().and_then(|l| l.iter().find(|a| a["fragment"] == pip.as_str())).and_then(|a| a["identity"].as_str()).context("Pip on its computer")?.to_string();
    anyhow::ensure!(call("POST", &format!("/api/f/{pip}/deploy"), Some(json!({})))?.status == 200, "Pip deployed");
    let chat = call("POST", "/api/fragments", Some(json!({ "label": "pip-chat", "template": "chat", "title": "Chat" })))?.body["name"].as_str().context("Pip's chat")?.to_string();
    anyhow::ensure!(call("PUT", &format!("/api/f/{chat}/members/{identity}"), Some(json!({ "role": "editor" })))?.status == 200, "Pip in its chat");
    // what Pip is, as its person reads it: each file of its self, its name, and their agents and chats
    let pip_is = || -> Result<Value> {
        let mut is = serde_json::Map::new();
        for (path, _) in own {
            is.insert(path.into(), json!(call("GET", &format!("/api/f/{pip}/file?path={}", crate::api::url_enc(path)), None)?.text));
        }
        let list = call("GET", "/api/fragments", None)?.body["fragments"].as_array().cloned().unwrap_or_default();
        is.insert("title".into(), list.iter().find(|f| f["name"] == pip.as_str()).map(|f| f["title"].clone()).unwrap_or_default());
        let mut kinds: Vec<&str> = list.iter().filter_map(|f| f["kind"].as_str()).collect();
        kinds.sort_unstable();
        is.insert("kinds".into(), json!(kinds));
        Ok(Value::Object(is))
    };
    let was = pip_is()?;
    let whole = own.iter().all(|(p, t)| was[*p] == *t) && was["title"] == "Pip" && was["kinds"] == json!(["agent", "chat"]);
    anyhow::ensure!(whole, "Pip as made: {was}");

    // their old agent, Wren, on the roster beside the first person's
    let wren = format!("{run}-b");
    let mut a = agent(&wren);
    a.name = "Wren".into();
    a.picture_url = Some(format!("{}/__shell/agent.png", api.base));
    let people = [before.to_vec(), vec![RosterPerson { workos_id: workos_id.clone(), email: format!("{run}@old.test"), agents: vec![a.clone()] }]].concat();
    anyhow::ensure!(mover.push(&Roster { snapshot: format!("{run}-into"), observed_ms: 1_800_000_000_000, people: people.clone() })?.status == 200, "the roster");
    b.set_cookie(&format!("{}/", api.base), "fragment_session", &session)?;
    let row = format!("document.querySelector('#move-page .move-row[data-old={}]')", serde_json::to_string(&wren)?);
    let radio = |value: &str| format!("#move-page .move-row[data-old=\"{wren}\"] input[type=radio][value=\"{value}\"]");
    let asked = format!(
        "(() => {{ const q = {row}?.querySelector('.move-into'); return q ? {{ said: q.querySelector('p').textContent, answers: [...q.querySelectorAll('label')].map((l) => [l.textContent, l.querySelector('input').checked]), shown: !q.hidden, role: q.getAttribute('role'), go: !document.querySelector('#move-page button.primary').disabled }} : null; }})()"
    );
    const SAID: &str = "You already have an agent here. Make a new one for this upgrade, or use Pip?";
    let unanswered = json!({ "said": SAID, "answers": [["Make a new one", false], ["Use Pip", false]], "shown": true, "role": "radiogroup", "go": false });
    for (width, height, phone, scheme) in [(1280, 800, false, "light"), (1280, 800, false, "dark"), (390, 844, true, "light"), (390, 844, true, "dark")] {
        let p = b.open(&format!("{}/move", api.base))?;
        b.viewport(&p, width, height, phone)?;
        b.color_scheme(&p, scheme)?;
        let drawn = b.until(&p, &format!("!!{row}?.querySelector('.move-into') && !!{row}?.querySelector('img')?.naturalWidth"), wait);
        std::thread::sleep(Duration::from_millis(300));
        let _ = b.screenshot(&p, &shots.join(format!("into-{width}-{scheme}.png")));
        let q = b.eval(&p, &asked)?;
        b.click(&p, &radio(&pip))?;
        std::thread::sleep(Duration::from_millis(200));
        let _ = b.screenshot(&p, &shots.join(format!("into-pip-{width}-{scheme}.png")));
        let chose = b.eval(&p, &asked)?;
        if (width, scheme) == (1280, "light") {
            s.ok(
                "someone with an agent here is asked on /move, an old agent at a time: a new one, or Pip by its name; nothing chosen for them, Upgrade waiting for the answer; Pip chosen, Upgrade offered",
                drawn && q == unanswered && chose["answers"] == json!([["Make a new one", false], ["Use Pip", true]]) && chose["go"] == true,
                json!([q, chose]),
            );
        }
        b.close(p)?;
    }

    // a Yes that never lands (the page's own call for it refused, so the
    // server never hears it): nothing made; a reload asks again
    let page = b.open(&format!("{}/move", api.base))?;
    b.viewport(&page, 1280, 800, false)?;
    b.color_scheme(&page, "light")?;
    anyhow::ensure!(b.until(&page, &format!("!!{row}?.querySelector('.move-into')"), wait), "the question");
    b.eval(&page, "(() => { let n = 1; const f = window.fetch; window.fetch = (u, o) => (String(u).endsWith('/yes') && n-- > 0 ? Promise.reject(new TypeError('Failed to fetch')) : f(u, o)); return true; })()")?;
    b.click(&page, &radio(&pip))?;
    b.click(&page, "#move-page button.primary")?;
    let failed = b.until(&page, &format!("!!document.querySelector('#move-page .form-error') && !document.querySelector('#move-page button.primary').disabled && {row}?.dataset.state === 'open'"), Duration::from_secs(60));
    let kept = b.eval(&page, &asked)?;
    let after_fail = pip_is()?;
    // a move as staff read it, page by page
    let operator = s.operator.clone();
    let state = |o: &str| -> Value {
        let mut after = String::new();
        while let Ok(r) = api.signed(&operator, "GET", &format!("/api/admin/migrations?after={after}"), None) {
            if let Some(m) = r.body["moves"].as_array().and_then(|ms| ms.iter().find(|m| m["old"] == o)) {
                return m.clone();
            }
            match r.body["next"].as_str() {
                Some(next) => after = next.to_string(),
                None => break,
            }
        }
        Value::Null
    };
    b.reload(&page)?;
    let again = b.until(&page, &format!("!!{row}?.querySelector('.move-into')"), wait) && b.eval(&page, &asked)? == unanswered;
    s.ok(
        "a Yes into Pip that never landed: the page says so, Pip still chosen, to try again; nothing made, the move still open; a reload asks again, nothing chosen",
        failed && kept["answers"] == json!([["Make a new one", false], ["Use Pip", true]]) && after_fail == was && state(&wren)["state"] == "open" && again,
        json!({ "kept": kept, "pip": after_fail, "move": state(&wren) }),
    );

    // Pip chosen: its Yes alone, nothing made, nothing of it changed
    b.click(&page, &radio(&pip))?;
    b.click(&page, "#move-page button.primary")?;
    let bound = b.until(&page, &format!("{row}?.dataset.state === 'asked'"), Duration::from_secs(60));
    let _ = b.screenshot(&page, &shots.join("into-asked-1280-light.png"));
    let m = state(&wren);
    let now = pip_is()?;
    let gone = b.eval(&page, &format!("!{row}?.querySelector('.move-into') && !document.querySelector('#move-page button.primary')"))?;
    s.ok(
        "Pip chosen, Upgrade: Wren's move is bound to Pip (asked), no agent or chat made, Pip's SOUL, memories, skill, agent.json and name as they were; nothing left to ask",
        bound && m["agent"] == identity.as_str() && m["state"] == "asked" && now == was && gone == true,
        json!({ "move": m, "pip": now }),
    );

    // Pip took in Wren: it is no one else's, ever; another old agent of
    // theirs is not asked about it, and its Yes into Pip is refused
    let finch = format!("{run}-c");
    let people = [before.to_vec(), vec![RosterPerson { workos_id, email: format!("{run}@old.test"), agents: vec![a, agent(&finch)] }]].concat();
    anyhow::ensure!(mover.push(&Roster { snapshot: format!("{run}-into2"), observed_ms: 1_800_000_000_000, people })?.status == 200, "the roster again");
    let finch_row = format!("document.querySelector('#move-page .move-row[data-old={}]')", serde_json::to_string(&finch)?);
    let shown = b.until(&page, &format!("!!{finch_row}"), wait);
    let refused = call("POST", &format!("/api/migrations/offers/{finch}/yes"), Some(json!({ "agent": identity })))?;
    let finch_asked = b.eval(&page, &format!("!!{finch_row}?.querySelector('.move-into')"))?;
    s.ok(
        "Pip took in Wren, so it is no one else's: their other old agent is asked nothing about it, and a Yes into Pip for it is refused (409), changing nothing",
        shown && finch_asked == false && refused.status == 409 && refused.to_string().contains("that agent took in another upgrade already") && state(&finch)["state"] == "open",
        json!({ "refused": refused.to_string(), "finch": state(&finch) }),
    );

    // the mover stages Wren: its import's routine on Pip's own tasks, and
    // the person, on the page, taken to Pip's own chat
    mover.run_once()?;
    let shown_chat = format!(
        "(location.pathname === '/' && document.getElementById('move-page').hidden && new URLSearchParams((document.querySelector('#frames iframe:not([hidden])')?.dataset.src ?? '?').split('?')[1]).get('name') === {})",
        serde_json::to_string(&chat)?
    );
    let arrived = b.until(&page, &shown_chat, Duration::from_secs(60));
    std::thread::sleep(Duration::from_millis(300));
    let _ = b.screenshot(&page, &shots.join("into-arrived-1280-light.png"));
    let stage = format!("{wren}.p1.1");
    let routine = || call("GET", &format!("/api/f/{pip}/channels/tasks"), None).ok().and_then(|r| r.body["records"].as_array().cloned()).unwrap_or_default().into_iter().find(|r| r["body"]["kind"] == "routine" && r["body"]["text"].as_str().is_some_and(|t| t.contains(&stage)));
    let landed = s.eventually(Duration::from_secs(30), || routine().is_some());
    let r = routine().unwrap_or_default();
    let end = pip_is()?;
    s.ok(
        "staged: Wren's import begins in Pip, its routine on Pip's own tasks as Pip, naming its stage and the skill; the person is taken to Pip's own chat; Pip's self still as it was, and still no other agent",
        arrived && landed && r["principal"] == pip_npub.as_str() && r["body"]["text"].as_str().is_some_and(|t| t.contains("import-agent-home")) && end == was && state(&wren)["state"] == "importing",
        json!({ "routine": r, "pip": end, "move": state(&wren) }),
    );
    b.close(page)?;
    Ok(())
}

/// The move page's check, by keys alone: Tab to it, Space to uncheck it (a
/// picture, its focus ring on), Space to check it again (a picture). What
/// each left: checked, focused, its ring, Bring over enabled; and how it is
/// drawn (`appearance`), its fill on and off.
fn check_by_keys(b: &mut crate::browser::Browser, page: &crate::browser::Page, shots: &std::path::Path, width: u32, scheme: &str) -> Result<Value> {
    let at = json!(format!("{width} {scheme}"));
    for _ in 0..40 {
        if b.eval(page, "document.activeElement?.matches('#move-page input.check') === true")? == json!(true) {
            break;
        }
        b.press(page, "Tab", false)?;
    }
    const STATE: &str = "(() => { const c = document.querySelector('#move-page input.check'); return { checked: c.checked, focused: document.activeElement === c, ring: c.matches(':focus-visible'), bring: !document.querySelector('#move-page button.primary').disabled }; })()";
    b.press(page, " ", false)?;
    std::thread::sleep(Duration::from_millis(200));
    let off = b.eval(page, STATE)?;
    let empty = b.eval(page, "getComputedStyle(document.querySelector('#move-page input.check')).backgroundColor")?;
    let _ = b.screenshot(page, &shots.join(format!("move-unchecked-focus-{width}-{scheme}.png")));
    b.press(page, " ", false)?;
    std::thread::sleep(Duration::from_millis(200));
    let on = b.eval(page, STATE)?;
    let fill = b.eval(page, "getComputedStyle(document.querySelector('#move-page input.check')).backgroundColor")?;
    let _ = b.screenshot(page, &shots.join(format!("move-checked-focus-{width}-{scheme}.png")));
    let drawn = b.eval(page, "getComputedStyle(document.querySelector('#move-page input.check')).appearance")?;
    Ok(json!({ "at": at, "drawn": drawn, "off": off, "on": on, "empty": empty, "fill": fill }))
}

/// The room a move-big computer has: less than its stage, more than what
/// its import keeps on the disk.
const BIG_ROOM: u64 = 24 * 1024 * 1024;

/// A move bigger than its new computer can hold (2026-11-18: an old
/// agent's 47 GB import failed on a computer with about 10 GB free). The
/// bound agent's CLI runs as a computer whose disk has less room than the
/// stage (`Suite::cli_in_room`: no file may grow past it), as the agent
/// (a key of its own, as its computer's egress signs). The old way,
/// downloading the stage first, finds no room; the import reads the stage
/// where the platform keeps it, and keeps on the disk only what it keeps.
/// The reader's windows and refusals, and the files' rules, are
/// host-tested (cli/src/home/).
pub fn move_big(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("move-big", &[Need::Fakes, Need::Levers, Need::Move]) {
        return Ok(());
    }
    const MIB: usize = 1024 * 1024;
    let mover = Mover::new(&api.base, s.mover.clone());
    let old = format!("{}-o1", s.name("mb"));
    let owner = api.person()?;
    let workos_id = s.workos.user(&Api::email_of(&owner)).id;
    let (agent_keys, agent) = super::delegation::agent_of(api, &owner)?;
    let home = s.dir("move-big-agent");
    std::fs::create_dir_all(home.join(".config/fragment"))?;
    std::fs::write(home.join(".config/fragment/config.json"), json!({ "secret_key": agent_keys.secret_hex() }).to_string())?;

    // the old home: the fixture, and work larger than the computer's room
    let pattern = |n: usize, seed: u32| -> Vec<u8> { (0..n as u32).map(|i| (i.wrapping_mul(2_654_435_761) ^ seed).rotate_right(11) as u8).collect() };
    let zip_at = s.dir("move-big-stage").join("stage.zip");
    // a file too deep for its own place in storage (FIN-755): an old
    // export's run logs, past a repo path's 300 bytes where it was; one
    // whose name is too long even for the short place (250 bytes); one
    // named as a note is (FIN-757); and the old agent's own note, kept in
    // its folder
    let deep_dir = format!("workspace/finite-export-v2/run-1/stage/{}", (0..6).map(|i| format!("{i}{}", "d".repeat(49))).collect::<Vec<_>>().join("/"));
    let (deep, deep_long, deep_noted) = (format!("{deep_dir}/trace.bin"), format!("{deep_dir}/{}", "n".repeat(250)), format!("{deep_dir}/log.offloaded.md"));
    let extra = [
        (deep.as_str(), pattern(3 * MIB, 6)),
        (deep_long.as_str(), pattern(4096, 7)),
        (deep_noted.as_str(), b"# a run's log, named as a note is\n".to_vec()),
        ("agent/hermes-home/old.offloaded.md", b"# Moved to storage\n\nthe old agent's own note\n".to_vec()),
        ("workspace/videos/trip.mp4", pattern(40 * MIB, 1)),
        ("workspace/photos/garden.jpg", pattern(3 * MIB, 2)),
        ("workspace/datasets/readings.csv", pattern(6 * MIB, 3)),
        // what the files' pick leaves: weights larger than the computer's
        // room, a project's history (a hidden folder), a key file
        ("workspace/models/llama.gguf", pattern(30 * MIB, 4)),
        ("workspace/projects/app/.git/HEAD", b"ref: refs/heads/main\n".to_vec()),
        ("workspace/projects/app/.git/objects/ab/cdef", pattern(4096, 5)),
        ("workspace/projects/app/.env", b"TOKEN=not-read".to_vec()),
    ];
    let files = fragment_fakes::old_home::stage(&crate::devstack::repo_root(), &zip_at, &extra)?;
    let zip = std::fs::read(&zip_at)?;
    let bytes: u64 = files.iter().map(|(_, b)| b.len() as u64).sum();
    anyhow::ensure!(zip.len() as u64 > 2 * BIG_ROOM, "the stage is larger than the computer's room");
    mover.home(zip.clone(), 16 * MIB, files.len() as u64, bytes);
    let layout = fragment_fakes::old_home::layout(&crate::devstack::repo_root()).to_str().context("a utf-8 path")?.to_string();

    // Yes with this agent, and staged
    let people = vec![person(&workos_id, &[&old])];
    let r = mover.push(&Roster { snapshot: format!("{old}-s1"), observed_ms: 1_800_000_000_000, people })?;
    anyhow::ensure!(r.status == 200, "the roster: {} {}", r.status, r.body);
    let visit = api.unsigned("POST", "/api/test/migrations", Some(&json!({ "op": "visit", "who": Api::email_of(&owner) })))?;
    anyhow::ensure!(visit.status == 200, "its owner opens the move: {visit}");
    let yes = api.unsigned("POST", "/api/test/migrations", Some(&json!({ "op": "answer", "old": old, "answer": "yes", "agent": agent })))?;
    anyhow::ensure!(yes.status == 200, "Yes: {yes}");
    mover.run_once()?;
    let stage = format!("{old}.p1.1");
    let base = format!("/api/migrations/stages/{stage}");
    let opened = api.signed(&agent_keys, "POST", &format!("{base}/open"), Some(&json!({})))?;
    anyhow::ensure!(opened.status == 200 && opened.body["manifest"]["zip_bytes"] == zip.len(), "the agent opens its stage: {opened}");

    // the old way: the stage downloaded onto the computer first, which has no room for it
    let url = format!("{}{base}/zip", api.base);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64;
    let mut curl = std::process::Command::new("curl");
    curl.args(["-fsS", "-o"]).arg(home.join("home.zip")).args(["-H", &format!("authorization: {}", agent_keys.header("GET", &url, b"", now)), &url]);
    crate::with_room(&mut curl, BIG_ROOM);
    let downloaded = curl.output()?;
    let held = std::fs::metadata(home.join("home.zip")).map(|m| m.len()).unwrap_or(0);
    s.ok(
        "the old way, downloading the stage first, finds no room on a computer smaller than it: the download fails partway",
        !downloaded.status.success() && held <= BIG_ROOM,
        format!("{:?} {held} bytes: {}", downloaded.status.code(), String::from_utf8_lossy(&downloaded.stderr)),
    );
    std::fs::remove_file(home.join("home.zip")).ok();

    // the import reads it where it is
    let source = format!("stage:{stage}");
    let run = |s: &Suite, args: &[&str]| -> Result<Value> { crate::cli_data(args, &s.cli_in_room(api, &home, BIG_ROOM, args)) };
    let survey = run(s, &["home", "survey", &source, "--layout", &layout, "--json"])?;
    s.ok(
        "the survey reads the stage in place: its files, the cap's pick of its work, its skills",
        survey["source"]["kind"] == "stage"
            && survey["source"]["files"].as_u64().unwrap_or(0) + survey["source"]["skipped"].as_array().map_or(0, Vec::len) as u64 == files.len() as u64
            && survey["files"]["brought"]["files"].as_u64().is_some_and(|n| n >= 3),
        json!({ "source": survey["source"], "files": survey["files"]["brought"] }),
    );
    let made = s.create(api, &owner, "old-files")?;
    let name = made["name"].as_str().unwrap_or_default().to_string();
    let r = api.signed(&owner, "PUT", &format!("/api/f/{name}/visibility"), Some(&json!({ "visibility": "members" })))?;
    anyhow::ensure!(r.status == 200, "its files' fragment made members-only: {r}");
    let r = api.signed(&owner, "PUT", &format!("/api/f/{name}/members/{agent}"), Some(&json!({ "role": "editor" })))?;
    anyhow::ensure!(r.status == 200, "the agent edits it: {r}");
    let brought = run(s, &["home", "files", &source, "--layout", &layout, "--into", &name, "--json"])?;
    let file = |p: &str| api.signed(&owner, "GET", &format!("/api/f/{name}/file?path={}", crate::api::url_enc(p)), None);
    // every file it says it brought reads back whole where it went
    let (mut whole, mut bad) = (vec![], vec![]);
    for (path, want) in &files {
        let Some(rel) = path.strip_prefix("workspace/") else { continue };
        // past a repo path in the files too: left for storage, never asked for here
        if path.starts_with(&deep_dir) {
            continue;
        }
        match file(&format!("files/{rel}"))? {
            r if r.status == 200 && r.bytes == *want => whole.push(rel),
            r if r.status == 404 => {}
            r => bad.push(format!("{rel}: {} ({} bytes)", r.status, r.bytes.len())),
        }
    }
    s.ok(
        "its files come from the stage into a fragment of files, each it says it brought read back whole, the video larger than the computer's room among them",
        brought["files"].as_u64() == Some(whole.len() as u64) && whole.len() >= 3 && bad.is_empty() && whole.contains(&"videos/trip.mp4"),
        json!({ "brought": brought, "whole": whole, "bad": bad }),
    );
    let kept = disk_used(&home);
    s.ok("what the import keeps on the computer is within its room: the stage never came onto it", kept <= BIG_ROOM, format!("{kept} bytes"));

    // nothing left behind: the rest of the home kept in storage, a note where each was
    let storage = s.create(api, &owner, "storage")?["name"].as_str().unwrap_or_default().to_string();
    let r = api.signed(&owner, "PUT", &format!("/api/f/{storage}/visibility"), Some(&json!({ "visibility": "members" })))?;
    anyhow::ensure!(r.status == 200, "its storage made members-only: {r}");
    let r = api.signed(&owner, "PUT", &format!("/api/f/{storage}/members/{agent}"), Some(&json!({ "role": "editor" })))?;
    anyhow::ensure!(r.status == 200, "the agent edits its storage: {r}");
    let notes = home.join("old-agent/home");
    let notes_arg = notes.to_str().context("a utf-8 path")?.to_string();
    let store = |s: &Suite| run(s, &["home", "files", &source, "--layout", &layout, "--into", &name, "--store", &storage, "--notes", &notes_arg, "--json"]);
    let all = store(s)?;
    let stored = &all["stored"];
    let note_of = |p: &str| notes.join(format!("{p}.offloaded.md"));
    let record = |note: &std::path::Path| -> Option<Value> {
        let text = std::fs::read_to_string(note).ok()?;
        let start = text.rfind("\n```json\n")? + "\n```json\n".len();
        let end = text[start..].find("\n```\n")? + start;
        serde_json::from_str(&text[start..end]).ok()
    };
    // a file's own note, where it was, found by its record (a name too
    // long for `<name>.offloaded.md` has one named short)
    let note_for = |p: &str| -> Option<std::path::PathBuf> {
        let from = format!("/{p}");
        let dir = notes.join(p.rsplit_once('/').map_or("", |(d, _)| d));
        std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|n| n.to_string_lossy().ends_with(".offloaded.md") && record(n).is_some_and(|r| r["from"].as_str().is_some_and(|f| f.ends_with(&from))))
    };
    // where a note says its first file is in storage: its record's `at`, and the file's path from it
    let kept_at = |p: &str| -> Option<String> {
        let r = record(&note_for(p)?)?;
        Some(format!("{}/{}", r["at"].as_str()?, r["files"][0]["path"].as_str()?))
    };
    let deeps = [deep.as_str(), deep_long.as_str(), deep_noted.as_str()];
    let deep_at: std::collections::BTreeMap<&str, String> = deeps.iter().map(|p| (*p, kept_at(p).unwrap_or_default())).collect();
    let deep_notes: Vec<Option<std::path::PathBuf>> = deeps.iter().map(|p| note_for(p)).collect();
    // count and hash every file of the stage: each in one place, whole, but those never read
    let read = |fragment: &str, path: &str| api.signed(&owner, "GET", &format!("/api/f/{fragment}/file?path={}", crate::api::url_enc(path)), None);
    let (mut here, mut there, mut unread, mut wrong) = (0, 0, 0, vec![]);
    for (path, want) in &files {
        let in_files = match path.strip_prefix("workspace/") {
            Some(rel) => Some(read(&name, &format!("files/{rel}"))?),
            None => None,
        };
        let in_storage = read(&storage, &deep_at.get(path.as_str()).cloned().unwrap_or_else(|| format!("home/{path}")))?;
        let files_status = in_files.as_ref().map_or(404, |r| r.status);
        match (in_files.is_some_and(|r| r.status == 200 && r.bytes == *want), in_storage.status == 200 && in_storage.bytes == *want) {
            (true, false) => here += 1,
            (false, true) => there += 1,
            (false, false) if path.ends_with("/.env") && files_status == 404 && in_storage.status == 404 => unread += 1,
            _ => wrong.push(format!("{path}: files {files_status} storage {}", in_storage.status)),
        }
    }
    let manifest = opened.body["manifest"]["files"].as_u64().unwrap_or(0);
    s.ok(
        "nothing is left behind: every file of the stage, counted against its manifest and hashed, is in the files or kept whole in storage, but a key file, never read",
        wrong.is_empty() && here + there + unread == manifest as usize && unread == 1 && there >= 10 && stored["files"].as_u64() == Some(there as u64) && all["unread"] == 1,
        json!({ "here": here, "there": there, "unread": unread, "manifest": manifest, "wrong": wrong, "stored": stored }),
    );
    // the work kept is one note (within a note's bounds), larger than the room
    let weights = note_of("workspace");
    let left = String::from_utf8_lossy(&read(&name, "left-behind.md")?.bytes).to_string();
    s.ok(
        "a note where each kept folder or file was, laid out as the old home, and the files' list says where the rest is",
        weights.is_file() && std::fs::read_to_string(&weights).unwrap_or_default().contains(&format!("`{storage}`")) && note_of("agent").is_file() && left.starts_with("# Kept in storage") && left.contains(&storage),
        json!({ "notes": stored["notes_written"], "left": left.lines().take(3).collect::<Vec<_>>() }),
    );
    let kept = disk_used(&home);
    s.ok("the rest of the home never came onto the computer: what it keeps there is still within its room", kept <= BIG_ROOM, format!("{kept} bytes"));
    // back, as the agent brings back what it offloads: within its room, and not past it
    let back = |s: &Suite, note: &std::path::Path| s.cli_in_room(api, &home, BIG_ROOM, &["offload", "--back", note.to_str().unwrap_or_default(), "--json"]);
    let agent_back = back(s, &note_of("agent"));
    let soul = files.iter().find(|(p, _)| p == "agent/hermes-home/SOUL.md").map(|(_, b)| b.clone()).unwrap_or_default();
    let too_big = back(s, &weights);
    s.ok(
        "a note comes back as `fragment offload` brings one back, each file checked against the old one; one larger than the computer's room does not, and its note stays",
        agent_back.status.success() && std::fs::read(notes.join("agent/hermes-home/SOUL.md")).ok() == Some(soul) && !note_of("agent").exists() && !too_big.status.success() && weights.is_file(),
        format!("{} | {}", String::from_utf8_lossy(&agent_back.stdout), String::from_utf8_lossy(&too_big.stdout)),
    );
    // each back: trace.bin by its note, the others by where they were
    let mut deep_backs = Vec::new();
    for (p, note) in deeps.iter().zip(&deep_notes) {
        let want = files.iter().find(|(f, _)| f.as_str() == *p).map(|(_, b)| b.clone());
        let out = back(s, &if *p == deep { note.clone().unwrap_or_default() } else { notes.join(p) });
        let ok = out.status.success() && want.is_some() && std::fs::read(notes.join(p)).ok() == want && note.as_ref().is_some_and(|n| !n.exists());
        deep_backs.push(json!({ "ok": ok, "note": note.as_ref().map(|n| n.display().to_string()), "out": String::from_utf8_lossy(&out.stdout), "err": String::from_utf8_lossy(&out.stderr) }));
    }
    let short = |p: &str, under: &str, most: usize| deep_at[p].strip_prefix(under).is_some_and(|rest| rest.len() <= most + 1 + p.rsplit('/').next().unwrap_or_default().len());
    s.ok(
        "a file too deep for its own place is kept whole under a short one (a shorter one for a long name, its note named short), its note where it was, and comes back there by its own name (one named as a note is too), checked against the old one",
        short(&deep, "home/.long/", 24) && short(&deep_noted, "home/.long/", 24) && short(&deep_long, ".long/", 16) && deep_backs.iter().all(|b| b["ok"] == true),
        json!({ "at": deep_at, "backs": deep_backs }),
    );
    let again = store(s)?;
    s.ok(
        "again, nothing more is kept: what its notes name, and what came back, is kept already",
        again["stored"]["files"] == 0 && again["stored"]["already"] == stored["files"] && again["files"] == 0,
        &again["stored"],
    );
    // what its owner sees: the files, and their storage, each a page of files
    for f in [&name, &storage] {
        let deployed = s.cli_in_room(api, &home, BIG_ROOM, &["deploy", f]);
        anyhow::ensure!(deployed.status.success(), "the agent deploys {f}: {}", String::from_utf8_lossy(&deployed.stderr));
    }
    owner_sees(s, api, &owner, &[(&name, "files", "files"), (&storage, "storage", "home")])?;

    // the copy, stage to blobs on the platform: the bound agent's alone,
    // into a fragment it (or its owner, `for`) edits, within the zip and
    // its upload limit
    let (at, len) = (zip.windows(4).position(|w| w == b"PK\x03\x04").unwrap_or(0) as u64, 2 * MIB as u64);
    let copy = |k: &crate::Keys, into: &str, q: &str| api.signed(k, "POST", &format!("{base}/copy?into={into}&{q}"), None);
    let range = format!("offset={at}&bytes={len}");
    let want = fragment_core::blob::sha256_hex(&zip[at as usize..(at + len) as usize]);
    let mine = copy(&agent_keys, &name, &range)?;
    let held = api.signed(&owner, "GET", &format!("/api/f/{name}/blobs/{want}"), None)?;
    s.ok(
        "the bound agent copies a range of its stage into a fragment it edits, on the platform: the blob of those bytes",
        mine.status == 200 && mine.body["sha256"] == want.as_str() && mine.body["bytes"] == len && held.status == 200 && held.bytes == zip[at as usize..(at + len) as usize],
        format!("{mine} | {}", held.status),
    );
    let owners = s.create(api, &owner, "owners")?["name"].as_str().unwrap_or_default().to_string();
    let me = api.signed(&owner, "GET", "/api/identities/me", None)?.body["id"].as_str().unwrap_or_default().to_string();
    let for_owner = copy(&agent_keys, &owners, &format!("{range}&for={me}"))?;
    let as_itself = copy(&agent_keys, &owners, &range)?;
    s.ok(
        "acting for its owner, it copies into its owner's fragment it is no member of, as its computer's egress signs; as itself, the fragment refuses it (403)",
        for_owner.status == 200 && for_owner.body["sha256"] == want.as_str() && api.signed(&owner, "GET", &format!("/api/f/{owners}/blobs/{want}"), None)?.status == 200 && as_itself.status == 403,
        format!("{for_owner} | {as_itself}"),
    );
    let stranger = api.person()?;
    let theirs = copy(&stranger, &name, &range)?;
    let past = copy(&agent_keys, &name, &format!("offset={}&bytes={len}", zip.len() as u64 - len + 1))?;
    let over = copy(&agent_keys, &name, &format!("offset=0&bytes={}", fragment_proto::limits::BLOB_MAX_BYTES + 1))?;
    let none = copy(&agent_keys, &name, "offset=0&bytes=0")?;
    s.ok(
        "a copy is refused to anyone but the bound agent (403), past the zip's end (400), past its upload limit (413), and of nothing (400)",
        theirs.status == 403 && past.status == 400 && over.status == 413 && none.status == 400,
        format!("{} {} {} {}", theirs.status, past.status, over.status, none.status),
    );

    let done = api.signed(&agent_keys, "POST", &format!("{base}/done"), Some(&json!({ "ok": true })))?;
    let after = copy(&agent_keys, &name, &range)?;
    s.ok("it says it is done: the move waits for staff, and its stage copies no more (409)", done.status == 200 && done.body["state"] == "retiring" && after.status == 409, format!("{done} | {}", after.status));
    Ok(())
}

/// Each of `pages` (a fragment, what to call its picture, and a folder its
/// page of files lists) as `owner` opens it from their apps, in Chrome, at
/// 1280 and 390 px, light and dark: its picture taken once the page in its
/// frame lists that folder (no Chrome, none).
fn owner_sees(s: &mut Suite, api: &Api, owner: &crate::Keys, pages: &[(&str, &str, &str)]) -> Result<()> {
    let Some(mut b) = s.browser()? else {
        println!("      (no Chrome: no pictures of the owner's pages)");
        return Ok(());
    };
    let shots = s.dir("move-big");
    let session = api.sign_in(&Api::email_of(owner))?;
    b.set_cookie(&format!("{}/", api.base), "fragment_session", &session)?;
    let wait = Duration::from_secs(30);
    for (name, called, folder) in pages {
        let row = format!("document.querySelector({})", serde_json::to_string(&format!("#apps [data-key=\"app:{name}\"]"))?);
        let lists = format!("(document.body?.innerText ?? '').split(/\\s+/).includes({})", serde_json::to_string(folder)?);
        let mut seen = Vec::new();
        for (width, height, phone) in [(1280, 800, false), (390, 844, true)] {
            for scheme in ["light", "dark"] {
                let page = b.open(&format!("{}/", api.base))?;
                b.viewport(&page, width, height, phone)?;
                b.color_scheme(&page, scheme)?;
                let opened = b.until(&page, &format!("!!{row}"), wait) && {
                    b.eval(&page, &format!("({row}.click(), true)"))?;
                    b.until(&page, "!!document.querySelector('.viewer iframe')", wait)
                };
                let shown = opened && s.eventually(wait, || b.eval_in_frame(&page, name, &lists).ok() == Some(json!(true)));
                std::thread::sleep(Duration::from_millis(500));
                let _ = b.screenshot(&page, &shots.join(format!("{called}-{width}-{scheme}.png")));
                seen.push((format!("{width}-{scheme}"), shown));
                b.close(page)?;
            }
        }
        s.ok(&format!("its owner opens its {called} from their apps: a page of its files, listing {folder}/, at 1280 and 390 px, light and dark"), seen.iter().all(|(_, shown)| *shown), json!(seen));
    }
    println!("      (screenshots: {})", shots.display());
    Ok(())
}

/// The bytes the files under `dir` hold.
fn disk_used(dir: &std::path::Path) -> u64 {
    let Ok(listed) = std::fs::read_dir(dir) else { return 0 };
    listed.flatten().map(|e| match e.file_type() {
        Ok(t) if t.is_dir() => disk_used(&e.path()),
        Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
        _ => 0,
    }).sum()
}
