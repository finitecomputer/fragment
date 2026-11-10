//! The finite.computer move (docs/migrations.md), driven through its
//! routes with the fake mover (no page yet): the roster, binding a person
//! by their WorkOS id, their answers (the levers', until their routes
//! land), the mover's mailbox and reports, each deadline by the object's
//! clock lever, held moves and staff's acts on them, and phase 2. Valid,
//! invalid, replay and restart for each.

use anyhow::{Context, Result};
use fragment_fakes::mover::{event, fixture_zip, staged, Mover, Script};
use fragment_proto::migrations::{Has, Roster, RosterAgent, RosterPerson, Sizes};
use serde_json::{json, Value};

use crate::api::Api;
use crate::{Need, Suite};

const HOUR_MS: i64 = 60 * 60 * 1000;
const DAY_MS: i64 = 24 * HOUR_MS;

fn agent(old: &str) -> RosterAgent {
    RosterAgent {
        old: old.into(),
        name: format!("Old {old}"),
        picture_url: None,
        last_active_ms: Some(1_800_000_000_000),
        sizes: Sizes { cache: 1, history: 2, work: 3, staged: 5, total: 6 },
        brains: vec![],
        sites: vec![],
        has: Has { telegram: false, openrouter: false, nostr: true },
    }
}

fn person(workos_id: &str, olds: &[&str]) -> RosterPerson {
    RosterPerson { workos_id: workos_id.into(), email: format!("{workos_id}@old.test"), agents: olds.iter().map(|o| agent(o)).collect() }
}

pub fn moves(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("move", &[Need::Fakes, Need::Deployment, Need::Levers]) {
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
    let (_, strangers_agent) = api.own_agent(&stranger)?;
    let theirs = answer(&a1, "yes", Some(&strangers_agent))?;
    let yes = answer(&a1, "yes", Some(&alice_agent))?;
    let yes_again = answer(&a1, "yes", Some(&alice_agent))?;
    let cmd = mover.commands()?.into_iter().find(|c| c.old == a1).context("a1's command")?;
    s.ok(
        "Yes with another person's agent is refused (403); with their own, the mover's mailbox holds stop_and_stage of the stage <old>.p1.1, and Yes again changes nothing",
        theirs.status == 403
            && yes.body["state"] == "asked"
            && yes_again.status == 200
            && serde_json::to_value(&cmd)? == json!({ "id": format!("{a1}:1:stop_and_stage"), "old": a1, "attempt": 1, "kind": "stop_and_stage", "phase": "p1", "stage": format!("{a1}.p1.1") }),
        format!("{theirs} {cmd:?}"),
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
    let r = act(&a1, "retry")?;
    let stale = mover.report_as(&a1, &cmd.id, 9, event("stopped"))?;
    s.ok("staff's retry is a new attempt (2), and a report on attempt 1's command is stale (409), changing nothing", r.status == 200 && r.body["state"] == "asked" && r.body["attempt"] == 2 && stale.status == 409 && state(&a1) == "asked", format!("{r} {}", stale.body));

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
    let r = act(&a1, "restart-old")?;
    let restart = mover.commands()?.into_iter().find(|c| c.old == a1);
    mover.run_once()?;
    let m = moved(&a1)?;
    s.ok(
        "restart-old offers it again, unbound, and the mover restarts the old agent: its command answered, then gone",
        r.status == 200 && r.body["state"] == "open" && restart.is_some_and(|c| c.id == format!("{a1}:3:restart")) && m["command"].is_null() && m["agent"].is_null(),
        &m,
    );

    // ---- a stage that fails at the last attempt is held; staff retire it
    let yes = answer(&a1, "yes", Some(&alice_agent))?;
    let attempt = yes.body["attempt"].clone();
    mover.script(Script::Fails);
    mover.run_once()?;
    let m = moved(&a1)?;
    s.ok("Yes again is attempt 4; a failed stage past the attempts is held (stage_failed)", attempt == 4 && m["state"] == "held" && m["reason"] == "stage_failed", &m);
    let r = act(&a1, "retire")?;
    mover.script(Script::Done);
    mover.run_once()?;
    let bad = act(&a1, "withdraw")?;
    s.ok("staff retire it: the mover retires the old agent and the move is done (moved); a done move is not withdrawn (409)", r.body["state"] == "retiring" && state(&a1) == "moved" && bad.status == 409, format!("{} {}", r.body, bad));

    // ---- a roster withdraws the unasked; staff's withdrawal holds
    let r = mover.push(&roster("s3", vec![person(&alice_id, &[&a1, &a2]), person(&dave_id, &[&d1, &d2])]))?;
    let gone = state(&b1);
    let r2 = mover.push(&roster("s4", vec![person(&alice_id, &[&a1, &a2]), person(&dave_id, &[&d1, &d2]), person(&format!("{run}-nobody"), &[&b1])]))?;
    let back = state(&b1);
    let w = act(&b1, "withdraw")?;
    mover.push(&roster("s5", vec![person(&alice_id, &[&a1, &a2]), person(&dave_id, &[&d1, &d2]), person(&format!("{run}-nobody"), &[&b1])]))?;
    s.ok(
        "a roster without an unasked move withdraws it, the next that lists it offers it again, and staff's withdrawal stays whatever a roster says; a moved one is untouched",
        r.body["withdrawn"] == 1 && gone == "withdrawn" && r2.status == 200 && back == "listed" && w.body["state"] == "withdrawn" && state(&b1) == "withdrawn" && state(&a1) == "moved",
        format!("{} {} {gone} {back}", r.body, r2.body),
    );

    // ---- phase 2: past the cutoff, the unasked are staged and retired
    // unasked (archived); a late Yes imports the kept stage; 90 days on, expired
    // carol is on the old platform alone: she signs up only after the cutoff
    let carol = crate::Keys::generate();
    let c_id = s.workos.user(&Api::email_of(&carol)).id;
    mover.push(&roster("s6", vec![person(&alice_id, &[&a1, &a2]), person(&dave_id, &[&d1, &d2]), person(&c_id, &[&c1])]))?;
    let cutoff = crate::MIGRATIONS_CUTOFF_MS;
    let now = lever(json!({ "op": "clock", "offsetMs": offset.get() }))?["now"].as_i64().context("the object's now")?;
    later_by(cutoff - now + 1)?;
    let p2 = mover.commands()?.into_iter().filter(|c| [&a2, &c1, &d1].contains(&&c.old)).map(|c| (c.old.clone(), serde_json::to_value(&c).unwrap_or_default())).collect::<Vec<_>>();
    s.ok(
        "at the cutoff every unasked move is archiving: stop_and_stage of its stage <old>.p2.<attempt>",
        p2.len() == 3 && p2.iter().all(|(o, c)| c["kind"] == "stop_and_stage" && c["phase"] == "p2" && c["stage"].as_str().is_some_and(|p| p.starts_with(&format!("{o}.p2.")))),
        format!("{p2:?}"),
    );
    mover.run_once()?;
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
    let (_, carol_agent) = api.own_agent(&carol)?;
    for (o, who, agent) in [(&c1, "a late sign-up", &carol_agent), (&a2, "someone who never answered", &alice_agent)] {
        let late = answer(o, "yes", Some(agent))?;
        let late_move = moved(o)?;
        s.ok(
            &format!("{who}'s late Yes imports the kept phase 2 stage: importing, its phase 2 stage, into the agent they name"),
            late.body["state"] == "importing" && late_move["state"] == "importing" && late_move["phase"] == "p2" && late_move["agent"] == agent.as_str(),
            &late_move,
        );
    }
    later_by(90 * DAY_MS)?;
    let e = moved(&d1)?;
    s.ok("90 days past the cutoff an unclaimed archive expires (its stage's deletion is the alarm's: F7)", e["state"] == "expired", &e);
    lever(json!({ "op": "clock", "offsetMs": 0 }))?;
    Ok(())
}
