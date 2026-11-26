//! Agents acting for people (docs/cloudflare-v1.md, decision 36): sharing
//! with a person shares with their agents, never above that person, unless
//! the agent is held below its owner. A share is the person's and their
//! agents' (Paul, 2026-10-13: "giving all the pubkeys under the umbrella of
//! this email" access): no share says otherwise.
//!
//! Paul shares a fragment with Skyler as an editor; Skyler's agent edits it
//! acting for Skyler, and alone (as itself, no member) reaches nothing.
//! Held at viewer by Skyler, it reads and never edits. No one else holds
//! Skyler's agent.
//!
//! Then an agent shares for its owner (Paul, 2026-10-04): `sharing`.

use std::time::Duration;

use anyhow::Result;
use fragment_nip98::Keys;
use serde_json::{json, Value};

use super::app::ship;
use crate::api::{Api, Reply};
use crate::Suite;

const TODO_APP: &[u8] = include_bytes!("../../fixtures/todo.mjs");
const TODO_JSON: &[u8] = include_bytes!("../../fixtures/todo.json");
const CHAT_APP: &[u8] = include_bytes!("../../fixtures/chat.mjs");
const CHAT_JSON: &[u8] = include_bytes!("../../fixtures/chat.json");
const FORMAT_MJS: &[u8] = include_bytes!("../../fixtures/format.mjs");

pub fn delegation(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("delegation", &[]) {
        return Ok(());
    }
    let (paul, skyler) = (api.person()?, api.person()?);
    let skyler_id = api.identity(&skyler)?;
    // Skyler's agent, registered by Skyler
    let juniper = Keys::generate();
    let reg = "/api/identities";
    let r = api.signed(&skyler, "POST", reg, Some(&json!({ "kind": "agent", "proof": api.proof(&juniper, "POST", reg, &skyler) })))?;
    let juniper_id = r.body["id"].as_str().unwrap_or("").to_string();
    s.ok("Skyler registers an agent", r.status == 200 && r.body["owner"] == skyler_id.as_str(), &r);

    let name = s.named(api, &paul, "shared")?;
    let c = s.create(api, &paul, &name)?;
    ship(s, &c, TODO_APP, TODO_JSON);
    api.signed(&paul, "PUT", &format!("/api/f/{name}/visibility"), Some(&json!({ "visibility": "members" })))?;
    let r = api.signed(&paul, "PUT", &format!("/api/f/{name}/members/{skyler_id}"), Some(&json!({ "role": "editor", "peopleOnly": true })))?;
    s.ok("a share is the person's and their agents': one that asks otherwise is refused (400)", r.status == 400, &r);
    // an invite is to a person and their agents (decision 63): Skyler's yes
    // reaches Skyler's agent, acting for them, unnamed
    let invited = api.signed(&paul, "POST", &format!("/api/f/{name}/invites"), Some(&json!({ "email": Api::email_of(&skyler), "role": "editor" })))?;
    let r = api.signed(&skyler, "POST", &format!("/api/invites/{name}/accept"), Some(&json!({})))?;
    s.ok("Paul invites Skyler as an editor, and Skyler accepts", invited.status == 200 && r.status == 200 && r.body["role"] == "editor", format!("{invited} {r}"));
    let mut n = 0;
    let mut add = |for_skyler: bool| {
        n += 1;
        let path = match for_skyler {
            true => format!("/api/f/{name}/ops/add_todo?for={skyler_id}"),
            false => format!("/api/f/{name}/ops/add_todo"),
        };
        api.signed(&juniper, "POST", &path, Some(&json!({ "id": format!("j{n}"), "input": { "text": "from Skyler's agent" } })))
    };
    let r = add(true)?;
    s.ok("Skyler's agent edits it, acting for Skyler", r.status == 200, &r);
    // a person's membership is their agents' (decision 64: "when they
    // accept they add their agent... treat them as one entity")
    let r = add(false)?;
    let members = api.signed(&paul, "GET", &format!("/api/f/{name}/members"), None)?;
    let row_of = |members: &crate::api::Reply, id: &str| members.body["members"].as_array().and_then(|m| m.iter().find(|m| m["principal"] == id).cloned()).unwrap_or_default();
    let junipers = row_of(&members, &juniper_id);
    let their_list = api.signed(&juniper, "GET", "/api/fragments", None)?;
    let listed_there = their_list.body["fragments"].as_array().is_some_and(|l| l.iter().any(|f| f["name"] == name.as_str() && f["role"] == "editor"));
    s.ok(
        "Skyler's yes brought Skyler's agent in with them, at their role: it edits as itself, is a member (an agent, Skyler's), and its own list names the fragment",
        r.status == 200 && junipers["kind"] == "agent" && junipers["owner"] == skyler_id.as_str() && junipers["role"] == "editor" && listed_there,
        format!("{r} {junipers} {their_list}"),
    );

    let r = api.op(&skyler, &name, "add_todo", "s1", json!({ "text": "from Skyler" }))?;
    s.ok("as Skyler does", r.status == 200, &r);
    // Paul shared with Skyler and his agents, not "Skyler and his agent
    // Juniper": none of Paul's views names it (the API's members list, the
    // fragment's truth, has its row)
    let access = api.signed(&paul, "GET", "/api/access", None)?;
    let paul_session = api.sign_in(&Api::email_of(&paul))?;
    let sheet = super::signin::with_session(api, "GET", &format!("/share/{name}"), &paul_session)?;
    let row = access.body["fragments"].as_array().and_then(|l| l.iter().find(|f| f["name"] == name.as_str()).cloned()).unwrap_or_default();
    s.ok(
        "Paul sees Skyler, never Skyler's agent: the access view and the share sheet show Skyler as one entry, \"with their agents\", and neither names it",
        access.status == 200
            && sheet.status == 200
            && sheet.text.contains(&Api::email_of(&skyler))
            && sheet.text.contains("With their agents")
            && row["members"].as_array().is_some_and(|m| m.len() == 1)
            && row["sharing"]["guests"] == 1
            && [&access.text, &sheet.text].iter().all(|t| !t.contains(&juniper_id)),
        format!("{row} {}", sheet.status),
    );
    // its role is its person's: it follows theirs, and is set no other way
    let own_role = api.signed(&paul, "PUT", &format!("/api/f/{name}/members/{juniper_id}"), Some(&json!({ "role": "viewer" })))?;
    let lowered = api.signed(&paul, "PUT", &format!("/api/f/{name}/members/{skyler_id}"), Some(&json!({ "role": "viewer" })))?;
    let after = row_of(&api.signed(&paul, "GET", &format!("/api/f/{name}/members"), None)?, &juniper_id);
    let raised = api.signed(&paul, "PUT", &format!("/api/f/{name}/members/{skyler_id}"), Some(&json!({ "role": "editor" })))?;
    s.ok(
        "an agent in with its person holds their role: set on its own it is 400, saying so; Skyler lowered to viewer, it follows",
        own_role.status == 400 && own_role.message().contains("its person") && lowered.status == 200 && after["role"] == "viewer" && raised.status == 200,
        format!("{own_role} {lowered} {after}"),
    );

    let held = |by: &Keys, held: serde_json::Value| api.signed(by, "PUT", &format!("/api/identities/{juniper_id}/held"), Some(&json!({ "held": held })));
    let r = held(&paul, json!("viewer"))?;
    s.ok("no one but its owner holds an agent", r.status == 403, &r);
    let r = held(&skyler, json!("owner"))?;
    s.ok("an agent is held at viewer or editor, never owner", r.status == 400, &r);
    // a membership of its own elsewhere (one of before decision 64, no
    // person of its in there: a lever makes it), which a hold never caps
    let solo = s.named(api, &paul, "solo")?;
    s.create(api, &paul, &solo)?;
    let r = api.share(&solo, &juniper_id, "editor")?;
    anyhow::ensure!(r.status == 200, "Skyler's agent on its own in {solo}: {r}");
    let r = held(&skyler, json!("viewer"))?;
    s.ok("Skyler holds the agent at viewer", r.status == 200, &r);
    let r = add(true)?;
    let alone = add(false)?;
    s.ok("held below Skyler, it no longer edits: acting for them, nor as itself, in with them (the hold secures the whole entity)", r.status == 403 && alone.status == 403, format!("{r} {alone}"));
    let (with_person, on_its_own) = (api.status(&juniper, &name)?, api.status(&juniper, &solo)?);
    s.ok(
        "the hold caps it only where it is in with Skyler (a viewer there), never a membership of its own (an editor still, where Skyler is not)",
        with_person.body["role"] == "viewer" && on_its_own.body["role"] == "editor",
        format!("{} / {}", with_person.body["role"], on_its_own.body["role"]),
    );
    let q = api.signed(&juniper, "POST", &format!("/api/f/{name}/ops/count?for={skyler_id}"), Some(&json!({ "id": "q1", "input": {} })))?;
    s.ok("and still reads", q.status == 200, &q);
    held(&skyler, Value::Null)?;
    let r = add(true)?;
    s.ok("let go, it edits again", r.status == 200, &r);

    // an agent Skyler makes after their yes comes in beside them
    let (later, later_id) = agent_of(api, &skyler)?;
    let row = row_of(&api.signed(&paul, "GET", &format!("/api/f/{name}/members"), None)?, &later_id);
    let r = api.signed(&later, "POST", &format!("/api/f/{name}/ops/add_todo"), Some(&json!({ "id": "later1", "input": { "text": "from Skyler's newer agent" } })))?;
    s.ok(
        "an agent Skyler makes after their yes comes in beside them, at their role, and edits as itself",
        row["kind"] == "agent" && row["owner"] == skyler_id.as_str() && row["role"] == "editor" && r.status == 200,
        format!("{row} {r}"),
    );
    // leaving, and removing, are the whole entity's
    let in_now = |id: &str| -> Result<bool> { Ok(!row_of(&api.signed(&paul, "GET", &format!("/api/f/{name}/members"), None)?, id).is_null()) };
    let left = api.signed(&juniper, "DELETE", &format!("/api/f/{name}/members/me"), None)?;
    let (skyler_in, juniper_in, later_in) = (in_now(&skyler_id)?, in_now(&juniper_id)?, in_now(&later_id)?);
    let alone = add(false)?;
    let theirs = api.op(&skyler, &name, "add_todo", "s2", json!({ "text": "from Skyler, gone" }))?;
    s.ok(
        "one agent of Skyler's leaving takes the whole entity out: Skyler, it and their other agent; none edits now",
        left.status == 200 && !skyler_in && !juniper_in && !later_in && alone.status == 403 && theirs.status == 403,
        format!("{left} {alone} {theirs}"),
    );
    let invited = api.signed(&paul, "POST", &format!("/api/f/{name}/invites"), Some(&json!({ "email": Api::email_of(&skyler), "role": "viewer" })))?;
    let yes = api.signed(&skyler, "POST", &format!("/api/invites/{name}/accept"), Some(&json!({})))?;
    let back = (in_now(&skyler_id)?, in_now(&juniper_id)?, in_now(&later_id)?);
    let removed = api.signed(&paul, "DELETE", &format!("/api/f/{name}/members/{skyler_id}"), None)?;
    let out = (in_now(&skyler_id)?, in_now(&juniper_id)?, in_now(&later_id)?);
    s.ok(
        "invited again, Skyler's yes brings every agent of theirs back; Paul removing Skyler removes them all",
        invited.status == 200 && yes.status == 200 && back == (true, true, true) && removed.status == 200 && out == (false, false, false),
        format!("{yes} {removed} {back:?} {out:?}"),
    );
    sharing(s, api)?;
    own_membership(s, api)?;
    mandates(s, api)
}

/// Goal: proof of who asked (FIN-615, FIN-720; docs/api.md, "Who asked:
/// mandates"). A place gives its agent members, reading it as themselves,
/// a mandate beside each person's own words there to them; with its
/// owner's, the agent's calls act for its owner exactly as `for` does, now
/// proven; with anyone
/// else's, for them, held to their role. Method: Ana's room, where Ana's
/// agent Juniper and Ben's agent Fred are members with Ben, and Ana, Ben
/// and Juniper each say something. Valid: each agent's read gives a
/// mandate beside Ana's and Ben's records and none beside Juniper's, a
/// person's read none; with Ana's, Juniper acts for
/// Ana in Ana's private app (the app told Ana, at Juniper's editor cap) and
/// in the list, as `for` would; the same mandate again acts the same (a
/// replay). With Ben's, Juniper acts as Ben in Ana's shared app, at Ben's
/// viewer role, the same op id as Ana's another run of his own. Invalid:
/// an altered mandate, another agent carrying Juniper's, one beside `for`,
/// and one where `for` is not honored are refused; with Ben's, Juniper
/// reaches nothing of Ana's he cannot, makes no fragment, and never leaves.
fn mandates(s: &mut Suite, api: &Api) -> Result<()> {
    let (ana, ben) = (api.person()?, api.person()?);
    let (ana_id, ben_id) = (api.identity(&ana)?, api.identity(&ben)?);
    let (juniper, juniper_id) = agent_of(api, &ana)?;
    let (fred, fred_id) = agent_of(api, &ben)?;
    let room = s.named(api, &ana, "room")?;
    let c = s.create(api, &ana, &room)?;
    s.commit(&c, &[("fragment.json", Some(br#"{"channels":{"chat":{"read":"viewer","post":"viewer"}}}"#))]);
    s.deploy(&c);
    for (who, role) in [(&ben_id, "viewer"), (&juniper_id, "editor"), (&fred_id, "editor")] {
        let r = api.share(&room, who, role)?;
        anyhow::ensure!(r.status == 200, "{who} in the room: {r}");
    }
    for (keys, id, text) in [(&ben, "b1", "Fred, Juniper: hello"), (&ana, "a1", "Juniper, note this in my app"), (&juniper, "j1", "on it")] {
        let r = api.signed(keys, "POST", &format!("/api/f/{room}/channels/chat"), Some(&json!({ "id": id, "body": { "text": text } })))?;
        anyhow::ensure!(r.status == 200, "{id} in the room: {r}");
    }
    let read = |keys: &Keys| -> Result<Vec<Value>> {
        let r = api.signed(keys, "GET", &format!("/api/f/{room}/channels/chat?after=0"), None)?;
        anyhow::ensure!(r.status == 200, "reading the room: {r}");
        Ok(r.body["records"].as_array().cloned().unwrap_or_default())
    };
    // who wrote each record a reader is given a mandate beside
    let mandated = |records: &[Value]| -> Vec<String> { records.iter().filter(|r| r["mandate"].as_str().is_some_and(|m| m.starts_with("fm1."))).map(|r| r["principal"].as_str().unwrap_or_default().to_string()).collect() };
    let (juniper_reads, fred_reads, ben_reads) = (read(&juniper)?, read(&fred)?, read(&ben)?);
    let both = vec![ben_id.clone(), ana_id.clone()];
    s.ok(
        "an agent member reading its place as itself is given a mandate beside each person's own words to it, its owner's and anyone else's, and none beside an agent's; a person's read carries none",
        juniper_reads.len() == 3 && mandated(&juniper_reads) == both && mandated(&fred_reads) == both && mandated(&ben_reads).is_empty() && ben_reads.iter().all(|r| r.get("mandate").is_none()),
        json!({ "juniper": mandated(&juniper_reads), "fred": mandated(&fred_reads), "ben": mandated(&ben_reads) }),
    );
    let mandate_of = |records: &[Value], by: &str| records.iter().find(|r| r["principal"] == by).and_then(|r| r["mandate"].as_str()).unwrap_or_default().to_string();
    let (anas, bens) = (mandate_of(&juniper_reads, &ana_id), mandate_of(&juniper_reads, &ben_id));

    let private = s.named(api, &ana, "private")?;
    let c = s.create(api, &ana, &private)?;
    s.commit(&c, &[("applib/format.mjs", Some(FORMAT_MJS))]);
    ship(s, &c, CHAT_APP, CHAT_JSON);
    api.signed(&ana, "PUT", &format!("/api/f/{private}/visibility"), Some(&json!({ "visibility": "members" })))?;
    let r = api.share(&private, &juniper_id, "editor")?;
    anyhow::ensure!(r.status == 200, "Juniper in Ana's private app: {r}");
    let carrying = |keys: &Keys, mandate: &str, method: &'static str, path: &str, body: Option<Value>| {
        api.call(crate::api::Call {
            method,
            url: format!("{}{path}", api.base),
            body: body.map(|b| b.to_string().into_bytes()),
            content_type: Some("application/json"),
            keys: Some(keys),
            extra: vec![("x-fragment-mandate", mandate.to_string())],
            ..crate::api::Call::default()
        })
    };
    let whoami = |keys: &Keys, mandate: &str, id: &str| carrying(keys, mandate, "POST", &format!("/api/f/{private}/ops/whoami"), Some(json!({ "id": id, "input": {} })));
    let (who, again) = (whoami(&juniper, &anas, "w1")?, whoami(&juniper, &anas, "w1")?);
    let listed = carrying(&juniper, &anas, "GET", "/api/fragments", None)?;
    s.ok(
        "with Ana's mandate, Juniper acts for Ana as `for` would: in her private app the app is told Ana, at Juniper's editor cap, the same again the same; and it lists her fragments",
        who.status == 200 && who.body["result"] == json!({ "principal": ana_id, "role": "editor" }) && again.body["result"] == who.body["result"]
            && listed.status == 200 && listed.body["fragments"].as_array().is_some_and(|l| l.iter().any(|f| f["name"] == private.as_str())),
        format!("{who} {again} {}", listed.status),
    );

    let mut altered = anas.clone();
    let last = altered.pop().unwrap_or('0');
    altered.push(if last == '0' { '1' } else { '0' });
    let forged = whoami(&juniper, &altered, "w2")?;
    let freds = whoami(&fred, &anas, "w3")?;
    let both = carrying(&juniper, &anas, "POST", &format!("/api/f/{private}/ops/whoami?for={ana_id}"), Some(json!({ "id": "w4", "input": {} })))?;
    // where `for` is refused, a mandate is too, never dropped: a route
    // acting as its signer alone, the registry's own, and the invites
    let unhonored: Vec<Reply> = ["/api/notices", "/api/identities/me", "/api/invites"].iter().map(|path| carrying(&juniper, &anas, "GET", path, None)).collect::<Result<_>>()?;
    s.ok(
        "an altered mandate (403), another agent carrying Juniper's (403), one beside `for` (400), and one where `for` is not honored (400, the identities and invites too) are refused",
        forged.status == 403 && freds.status == 403 && freds.message().contains("only by the agent it names") && both.status == 400 && unhonored.iter().all(|r| r.status == 400),
        format!("{forged} {freds} {both} {:?}", unhonored.iter().map(|r| r.status).collect::<Vec<_>>()),
    );

    // ---- someone else's ask: Juniper for Ben
    let shared = s.named(api, &ana, "shared")?;
    let c = s.create(api, &ana, &shared)?;
    s.commit(&c, &[("applib/format.mjs", Some(FORMAT_MJS))]);
    ship(s, &c, CHAT_APP, CHAT_JSON);
    api.signed(&ana, "PUT", &format!("/api/f/{shared}/visibility"), Some(&json!({ "visibility": "members" })))?;
    for (who, role) in [(&juniper_id, "editor"), (&ben_id, "viewer")] {
        let r = api.share(&shared, who, role)?;
        anyhow::ensure!(r.status == 200, "{who} in Ana's shared app: {r}");
    }
    let asked = |mandate: &str, id: &str| carrying(&juniper, mandate, "POST", &format!("/api/f/{shared}/ops/whoami"), Some(json!({ "id": id, "input": {} })));
    let (anas_run, bens_run, bens_again) = (asked(&anas, "s1")?, asked(&bens, "s1")?, asked(&bens, "s1")?);
    s.ok(
        "with Ben's mandate, Juniper acts as Ben in Ana's shared app: the app is told Ben, at his viewer role (below Juniper's editor cap); the op id Ana's ask used is another run, his own, the same again the same",
        anas_run.body["result"] == json!({ "principal": ana_id, "role": "editor" })
            && bens_run.status == 200
            && bens_run.body["result"] == json!({ "principal": ben_id, "role": "viewer" })
            && bens_again.body["result"] == bens_run.body["result"],
        format!("{anas_run} {bens_run} {bens_again}"),
    );
    let unreached = whoami(&juniper, &bens, "w5")?;
    let made = carrying(&juniper, &bens, "POST", "/api/fragments", Some(json!({ "name": s.named(api, &ben, "made")?, "template": "blank" })))?;
    let left = carrying(&juniper, &bens, "DELETE", &format!("/api/f/{shared}/members/me"), None)?;
    let still = api.signed(&ana, "GET", &format!("/api/f/{shared}/members"), None)?;
    let stays = still.body["members"].as_array().is_some_and(|l| l.iter().any(|m| m["principal"] == juniper_id.as_str()));
    s.ok(
        "with Ben's, Juniper reaches nothing of Ana's that Ben can't (her private app, 403), makes no fragment for him (403), and never leaves on his word (403)",
        unreached.status == 403 && made.status == 403 && left.status == 403 && stays,
        format!("{unreached} {made} {left} {stays}"),
    );
    Ok(())
}

/// Goal: Paul's obvious case (2026-11-21: "if ben is talking to my agent
/// and he asks it to do something with a fragment he's shared as an editor
/// on"): where an agent's owner is not in and it is, a request it signs for
/// its owner is its own (FIN-615, `permit::acts_as_itself`): it acts with
/// its own membership, under its own name, and no further. Method: Ben's
/// chat app, Ana's agent Juniper an editor there on its own (a membership
/// from before decision 64: today an agent comes in only with its person,
/// so a lever makes it), Ana not in it. Valid: Juniper, signing for Ana as
/// its terminal does, edits it, the app told Juniper with its own role,
/// never Ana with a role she does not hold, and its record Juniper's; the
/// same call again (a replay) writes nothing more; it mints a storage token
/// that writes; held at viewer by Ana, it still edits (a hold never caps
/// its own memberships, decision 36). Invalid: Ana herself still only reads
/// it (FIN-11); Juniper for Ana never shares it, deletes it or sets its
/// cap; once Ben removes it, for Ana it reaches nothing; and once Ana is
/// in, it acts for her again, at her role (R17).
fn own_membership(s: &mut Suite, api: &Api) -> Result<()> {
    let (ana, ben) = (api.person()?, api.person()?);
    let ana_id = api.identity(&ana)?;
    let (juniper, juniper_id) = agent_of(api, &ana)?;
    let app = s.named(api, &ben, "bens")?;
    let c = s.create(api, &ben, &app)?;
    s.commit(&c, &[("applib/format.mjs", Some(FORMAT_MJS))]);
    ship(s, &c, CHAT_APP, CHAT_JSON);
    api.signed(&ben, "PUT", &format!("/api/f/{app}/visibility"), Some(&json!({ "visibility": "members" })))?;
    let r = api.share(&app, &juniper_id, "editor")?;
    anyhow::ensure!(r.status == 200, "Juniper in Ben's app on its own: {r}");
    // Juniper signs `method /api/f/<app>[/<rest>]` for Ana, as its terminal does (`FRAGMENT_FOR`)
    let for_ana = |method: &str, rest: &str, body: Option<Value>| {
        let path = match rest {
            "" => format!("/api/f/{app}?for={ana_id}"),
            rest => format!("/api/f/{app}/{rest}?for={ana_id}"),
        };
        api.signed(&juniper, method, &path, body.as_ref())
    };
    // `note` is an editor's: it posts to the editors' channel
    let note = |id: &str| for_ana("POST", "ops/note", Some(json!({ "id": id, "input": { "text": "Ben asked Juniper" } })));
    let notes = || super::jobs::records(api, &ben, &app, "staff");
    let whoami = |id: &str| for_ana("POST", "ops/whoami", Some(json!({ "id": id, "input": {} })));

    let (first, again) = (note("for-ana-1")?, note("for-ana-1")?);
    let posted = notes();
    s.ok(
        "Ana's agent, signing for Ana, edits Ben's app it is an editor of, where Ana is not: its record is its own; the same call again writes nothing more",
        first.status == 200 && again.status == 200 && posted.len() == 1 && posted[0]["principal"] == juniper_id.as_str(),
        format!("{first} {again} {posted:?}"),
    );
    let who = whoami("w1")?;
    s.ok(
        "the app is told the agent, with its own role: never Ana, with a role she does not hold there",
        who.status == 200 && who.body["result"] == json!({ "principal": juniper_id, "role": "editor" }),
        &who,
    );
    let token = for_ana("GET", "storage-token", None)?;
    s.ok("for Ana, it mints a storage token that writes: its membership is a member's", token.status == 200 && token.body["write"] == true, &token);
    let held = |held: Value| api.signed(&ana, "PUT", &format!("/api/identities/{juniper_id}/held"), Some(&json!({ "held": held })));
    let hold = held(json!("viewer"))?;
    let r = note("for-ana-2")?;
    let freed = held(Value::Null)?;
    s.ok(
        "held at viewer by Ana, it still edits there for her: a hold caps what it does with her access, never its own memberships",
        hold.status == 200 && r.status == 200 && freed.status == 200 && notes().len() == 2,
        format!("{hold} {r} {freed}"),
    );

    let hers = api.op(&ana, &app, "note", "ana-1", json!({ "text": "Ana herself" }))?;
    let reads = api.status(&ana, &app)?;
    s.ok(
        "Ana herself reads Ben's app through Juniper, and acts there never (403)",
        hers.status == 403 && hers.message().contains("acting here needs your own") && reads.status == 200 && reads.body["role"] == "viewer",
        format!("{hers} {}", reads.body["role"]),
    );
    let (_, bens_agent) = agent_of(api, &ben)?;
    let shares = for_ana("PUT", &format!("members/{bens_agent}"), Some(json!({ "role": "viewer" })))?;
    let deletes = for_ana("DELETE", "", None)?;
    let caps = for_ana("PUT", "cap", Some(json!({ "id": "c1", "micros": 1 })))?;
    s.ok(
        "for Ana it shares nothing of Ben's, and never deletes it or sets its cap (403 each)",
        shares.status == 403 && shares.message().contains("an agent shares only") && deletes.status == 403 && caps.status == 403 && api.status(&ben, &app)?.status == 200,
        format!("{shares} {deletes} {caps}"),
    );

    let removed = api.signed(&ben, "DELETE", &format!("/api/f/{app}/members/{juniper_id}"), None)?;
    let r = note("for-ana-3")?;
    s.ok("once Ben removes it, for Ana it reaches nothing (403), and nothing was written", removed.status == 200 && r.status == 403 && notes().len() == 2, format!("{removed} {r}"));
    // Ana in, at viewer: her yes brings her agents in beside her, at her role
    let invited = api.signed(&ben, "POST", &format!("/api/f/{app}/invites"), Some(&json!({ "email": Api::email_of(&ana), "role": "viewer" })))?;
    let yes = api.signed(&ana, "POST", &format!("/api/invites/{app}/accept"), Some(&json!({})))?;
    let (who, r) = (whoami("w2")?, note("for-ana-4")?);
    s.ok(
        "once Ana is in, a viewer, it acts for her again: the app is told Ana at her role, and it edits nothing (403)",
        invited.status == 200 && yes.status == 200 && who.body["result"] == json!({ "principal": ana_id, "role": "viewer" }) && r.status == 403,
        format!("{yes} {who} {r}"),
    );
    Ok(())
}

/// An agent `owner` registers, signing with a key of its own: its keys and
/// its identity.
pub(super) fn agent_of(api: &Api, owner: &Keys) -> Result<(Keys, String)> {
    let keys = Keys::generate();
    let reg = "/api/identities";
    let r = api.signed(owner, "POST", reg, Some(&json!({ "kind": "agent", "proof": api.proof(&keys, "POST", reg, owner) })))?;
    anyhow::ensure!(r.status == 200 && r.body["kind"] == "agent", "registering an agent: {r}");
    let id = r.body["id"].as_str().unwrap_or("").to_string();
    Ok((keys, id))
}

/// The role `keys`' list gives `name`, if it lists it.
fn listed(api: &Api, keys: &Keys, name: &str) -> Result<Option<String>> {
    let r = api.signed(keys, "GET", "/api/fragments", None)?;
    Ok(r.body["fragments"].as_array().and_then(|a| a.iter().find(|f| f["name"] == name)).and_then(|f| f["role"].as_str().map(str::to_string)))
}

/// Goal: Paul's decision of 2026-10-04, "your agent can share on your
/// behalf", over the API with a signed agent. Method: Hand, a key Paul
/// registered as his agent, signs `for=<Paul>` on Paul's app and does each
/// sharing action Paul could, checked as the people it reaches see it and
/// as Paul's events and members record it; then each case it must not
/// share in: for someone else, as itself, another person's agent, a
/// fragment Paul only edits, held, deleting, the cap, and a path spelled
/// so the router cannot tell what it is (the fragment decides again).
fn sharing(s: &mut Suite, api: &Api) -> Result<()> {
    let (paul, skyler, bob) = (api.person()?, api.person()?, api.person()?);
    let (paul_id, skyler_id, bob_id) = (api.identity(&paul)?, api.identity(&skyler)?, api.identity(&bob)?);
    let (hand, hand_id) = agent_of(api, &paul)?;
    let (juniper, _) = agent_of(api, &skyler)?;
    let app = s.named(api, &paul, "shares")?;
    let c = s.create(api, &paul, &app)?;
    s.commit(&c, &[("site/index.html", Some(b"<h1>Paul's app</h1>".as_slice()))]);
    s.deploy(&c);
    api.signed(&paul, "PUT", &format!("/api/f/{app}/visibility"), Some(&json!({ "visibility": "members" })))?;
    // `keys` signs `method /api/f/<name>[/<rest>]`, for `asker` when one is named
    let ask = |keys: &Keys, asker: Option<&str>, method: &str, name: &str, rest: &str, body: Option<Value>| {
        let path = match rest {
            "" => format!("/api/f/{name}"),
            rest => format!("/api/f/{name}/{rest}"),
        };
        let path = match asker {
            Some(asker) => format!("{path}?for={asker}"),
            None => path,
        };
        api.signed(keys, method, &path, body.as_ref())
    };
    let hand_for_paul = |method: &str, rest: &str, body: Option<Value>| ask(&hand, Some(&paul_id), method, &app, rest, body);
    let refused = |r: &Reply| r.status == 403;
    let viewer = json!({ "role": "viewer" });

    // valid: Paul's agent, for Paul, shares Paul's app as Paul would
    let direct = hand_for_paul("PUT", &format!("members/{bob_id}"), Some(viewer.clone()))?;
    s.ok(
        "Paul's agent, for Paul, adding Bob directly is refused (403), as Paul's own would be, saying to invite him by email",
        direct.status == 403 && direct.message().contains("invite them by email") && listed(api, &bob, &app)?.is_none(),
        &direct,
    );
    let (_, spare_id) = agent_of(api, &paul)?;
    let added = hand_for_paul("PUT", &format!("members/{spare_id}"), Some(viewer.clone()))?;
    s.ok(
        "it adds another agent of Paul's at once, and the member names the agent as who added it",
        added.status == 200 && added.body["role"] == "viewer" && added.body["addedBy"] == hand_id.as_str(),
        &added,
    );
    let again = hand_for_paul("PUT", &format!("members/{spare_id}"), Some(viewer.clone()))?;
    s.ok("the same PUT again is the same member (a replay)", again.status == 200 && again.body == added.body, json!({ "first": added.body, "again": again.body }));
    let shared = api.share(&app, &bob_id, "viewer")?;
    let bobs = listed(api, &bob, &app)?;
    anyhow::ensure!(shared.status == 200 && bobs.as_deref() == Some("viewer"), "Bob shared with by a lever: {shared}");
    let r = hand_for_paul("PUT", &format!("members/{bob_id}"), Some(json!({ "role": "editor" })))?;
    s.ok("it changes Bob's role", r.status == 200 && r.body["role"] == "editor", &r);
    let invited = format!("d-{}@e2e.test", &Keys::generate().pubkey_hex()[..12]);
    let r = hand_for_paul("POST", "invites", Some(json!({ "email": invited, "role": "viewer" })))?;
    s.ok(
        "it invites by email someone who has never signed in: the invite waits, naming the agent as its maker",
        r.status == 200 && r.body["invited"]["email"] == invited.as_str() && r.body["invited"]["createdBy"] == hand_id.as_str(),
        &r,
    );
    let r = hand_for_paul("GET", "invites", None)?;
    s.ok("lists the invites waiting", r.status == 200 && r.body["invites"].as_array().is_some_and(|a| a.iter().any(|i| i["email"] == invited.as_str())), &r);
    let r = hand_for_paul("DELETE", &format!("invites/{invited}"), None)?;
    s.ok("and revokes one", r.status == 200 && r.body["revoked"] == invited.as_str(), &r);
    // an agent's invite to someone who signs in already waits for their
    // yes too (decision 63): from Paul, as they see it; made by the agent
    let cleo = api.person()?;
    let r = hand_for_paul("POST", "invites", Some(json!({ "email": Api::email_of(&cleo), "role": "editor" })))?;
    let theirs = api.signed(&cleo, "GET", "/api/invites", None)?;
    let shown = theirs.body["invites"].as_array().and_then(|l| l.iter().find(|i| i["fragment"] == app.as_str()).cloned()).unwrap_or_default();
    let yes = api.signed(&cleo, "POST", &format!("/api/invites/{app}/accept"), Some(&json!({})))?;
    let cleo_id = api.identity(&cleo)?;
    let members = api.signed(&paul, "GET", &format!("/api/f/{app}/members"), None)?;
    let row = members.body["members"].as_array().and_then(|m| m.iter().find(|m| m["principal"] == cleo_id.as_str()).cloned()).unwrap_or_default();
    s.ok(
        "it invites someone who signs in already: the invite waits for their yes, theirs to see as from Paul; accepted, they are a member the agent added",
        r.status == 200
            && r.body["invited"]["createdBy"] == hand_id.as_str()
            && shown["from"] == Api::email_of(&paul).as_str()
            && shown["role"] == "editor"
            && yes.status == 200
            && row["role"] == "editor"
            && row["addedBy"] == hand_id.as_str(),
        json!([r.body, shown, yes.body, row]),
    );
    let r = hand_for_paul("POST", "invites", Some(json!({ "email": Api::email_of(&paul), "role": "viewer" })))?;
    s.ok("an invite to Paul's own email is 400: Paul owns it", r.status == 400 && r.message().contains("owns it"), &r);
    let closed = api.page(&app, "", None)?;
    let r = hand_for_paul("PUT", "visibility", Some(json!({ "visibility": "public" })))?;
    let mut open = None;
    // the deploy lands: the page is asked again until it has
    s.eventually(Duration::from_secs(20), || {
        open = api.page(&app, "", None).ok();
        open.as_ref().is_some_and(|p| p.status == 200 && p.text.contains("Paul's app"))
    });
    s.ok(
        "it makes the app public: an anonymous visitor it refused before opens it",
        r.status == 200 && closed.status == 401 && open.as_ref().is_some_and(|p| p.status == 200 && p.text.contains("Paul's app")),
        json!({ "set": r.body, "before": closed.status, "after": open.as_ref().map(|p| p.status) }),
    );
    let before = api.status(&paul, &app)?.body["viewToken"].as_str().unwrap_or("").to_string();
    let r = hand_for_paul("POST", "rotate", Some(json!({ "scopes": ["view"] })))?;
    let after = api.status(&paul, &app)?.body["viewToken"].as_str().unwrap_or("").to_string();
    s.ok(
        "it rotates the share link",
        r.status == 200 && r.body["rotated"] == json!(["view"]) && r.body["viewToken"] == after.as_str() && !before.is_empty() && after != before,
        &r,
    );
    let defaults = hand_for_paul("POST", "rotate", Some(json!({})))?;
    s.ok("and a rotate that names nothing renews both links", defaults.status == 200 && defaults.body["rotated"] == json!(["inbox", "view"]), &defaults);
    let r = hand_for_paul("DELETE", &format!("members/{bob_id}"), None)?;
    let bobs = listed(api, &bob, &app)?;
    s.ok("and removes Bob, whose list no longer has it", r.status == 200 && bobs.is_none(), json!({ "removed": r.body, "listed": bobs }));
    let r = api.signed(&paul, "GET", &format!("/api/f/{app}/events?tail=50"), None)?;
    let by_hand: Vec<&str> = r.body["events"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["data"]["by"] == hand_id.as_str() && e["data"]["for"] == paul_id.as_str() && e["summary"].as_str().is_some_and(|t| t.contains("(an agent, for ")))
        .filter_map(|e| e["kind"].as_str())
        .collect();
    s.ok(
        "Paul's events say his agent did each, for him",
        ["member.set", "invite.created", "invite.revoked", "visibility", "tokens.rotated", "member.removed"].iter().all(|k| by_hand.contains(k)),
        json!(by_hand),
    );
    let itself = hand_for_paul("PUT", &format!("members/{hand_id}"), Some(json!({ "role": "owner" })))?;
    let paul_row = hand_for_paul("PUT", &format!("members/{paul_id}"), Some(viewer.clone()))?;
    let paul_out = hand_for_paul("DELETE", &format!("members/{paul_id}"), None)?;
    s.ok(
        "the role rules hold: it makes no one owner, itself included, and neither changes nor removes Paul",
        itself.status == 400 && paul_row.status == 400 && paul_out.status == 400,
        json!({ "itself": itself.body, "paulsRole": paul_row.body, "paulOut": paul_out.body }),
    );
    let r = hand_for_paul("POST", "invites", Some(json!({ "email": Api::email_of(&skyler), "role": "editor" })))?;
    let yes = api.signed(&skyler, "POST", &format!("/api/invites/{app}/accept"), Some(&json!({})))?;
    hand_for_paul("PUT", "visibility", Some(json!({ "visibility": "members" })))?;
    let junipers = ask(&juniper, Some(&skyler_id), "GET", &app, "status", None)?;
    let skylers = api.status(&skyler, &app)?;
    s.ok(
        "an invite it makes is Skyler's and their agents': accepted, Skyler edits, and Skyler's agent, acting for them, reaches it too",
        r.status == 200 && yes.status == 200 && junipers.status == 200 && junipers.body["role"] == "editor" && skylers.status == 200 && skylers.body["role"] == "editor",
        json!({ "invite": r.body, "yes": yes.body, "agent": junipers.status, "skyler": skylers.body["role"] }),
    );

    // invalid: no one else's authority, and nothing that is the owner's own
    let r = ask(&hand, Some(&skyler_id), "PUT", &app, &format!("members/{bob_id}"), Some(viewer.clone()))?;
    s.ok("acting for someone else, it shares nothing of Paul's, though Paul owns it", refused(&r) && r.message().contains("its own owner"), &r);
    let r = ask(&hand, None, "PUT", &app, &format!("members/{bob_id}"), Some(viewer.clone()))?;
    s.ok("nor as itself", refused(&r), &r);
    let for_skyler = ask(&juniper, Some(&skyler_id), "PUT", &app, &format!("members/{bob_id}"), Some(viewer.clone()))?;
    let for_paul = ask(&juniper, Some(&paul_id), "PUT", &app, &format!("members/{bob_id}"), Some(viewer.clone()))?;
    s.ok(
        "another person's agent shares nothing: Skyler's, for Skyler, who edits Paul's app, nor naming Paul",
        refused(&for_skyler) && for_skyler.message().contains("owner owns") && refused(&for_paul),
        json!({ "forSkyler": for_skyler.body, "forPaul": for_paul.body }),
    );
    let theirs = s.named(api, &skyler, "shares-theirs")?;
    s.create(api, &skyler, &theirs)?;
    api.share(&theirs, &paul_id, "editor")?;
    let add = ask(&hand, Some(&paul_id), "PUT", &theirs, &format!("members/{bob_id}"), Some(viewer.clone()))?;
    let open = ask(&hand, Some(&paul_id), "PUT", &theirs, "visibility", Some(json!({ "visibility": "public" })))?;
    s.ok("on a fragment Paul only edits (Skyler's), Paul's agent shares nothing", refused(&add) && refused(&open), json!({ "add": add.body, "visibility": open.body }));
    let hold = |held: Value| api.signed(&paul, "PUT", &format!("/api/identities/{hand_id}/held"), Some(&json!({ "held": held })));
    let r = hold(json!("editor"))?;
    let add = hand_for_paul("PUT", &format!("members/{bob_id}"), Some(viewer.clone()))?;
    let rotate = hand_for_paul("POST", "rotate", Some(json!({ "scopes": ["view"] })))?;
    s.ok(
        "held below Paul, even at editor, it shares nothing",
        r.status == 200 && refused(&add) && add.message().contains("holds") && refused(&rotate),
        json!({ "add": add.body, "rotate": rotate.body }),
    );
    hold(Value::Null)?;
    let r = hand_for_paul("PUT", &format!("members/{spare_id}"), Some(json!({ "role": "editor" })))?;
    s.ok("let go, it shares again", r.status == 200, &r);

    // it reads and answers the invites waiting for Paul's yes, acting for
    // him (Paul, 2026-10-22: "skyler invited me to edit a fragment, do you
    // see it?"), and no other agent does, nor it as itself, nor held
    let (yes_to, no_to) = (s.named(api, &skyler, "asks-paul")?, s.named(api, &skyler, "asks-paul-too")?);
    for f in [&yes_to, &no_to] {
        s.create(api, &skyler, f)?;
        let r = api.signed(&skyler, "POST", &format!("/api/f/{f}/invites"), Some(&json!({ "email": Api::email_of(&paul), "role": "editor" })))?;
        anyhow::ensure!(r.status == 200, "Skyler invites Paul to {f}: {r}");
    }
    let waiting = |keys: &Keys, asker: Option<&str>| api.signed(keys, "GET", &asker.map_or("/api/invites".to_string(), |a| format!("/api/invites?for={a}")), None);
    let seen = waiting(&hand, Some(&paul_id))?;
    let names: Vec<&str> = seen.body["invites"].as_array().into_iter().flatten().filter(|i| i["from"] == Api::email_of(&skyler).as_str()).filter_map(|i| i["fragment"].as_str()).collect();
    let (alone, theirs, as_person) = (waiting(&hand, None)?, waiting(&juniper, Some(&paul_id))?, waiting(&paul, Some(&paul_id))?);
    s.ok(
        "acting for Paul, it lists the invites waiting for his yes (from Skyler, to his email); as itself it is 403, so is Skyler's agent naming Paul, and a person naming `for` is 400",
        seen.status == 200 && names.contains(&yes_to.as_str()) && names.contains(&no_to.as_str()) && alone.status == 403 && theirs.status == 403 && as_person.status == 400,
        format!("{seen} {alone} {theirs} {as_person}"),
    );
    hold(json!("editor"))?;
    let held_seen = waiting(&hand, Some(&paul_id))?;
    let held_yes = api.signed(&hand, "POST", &format!("/api/invites/{yes_to}/accept?for={paul_id}"), Some(&json!({})))?;
    hold(Value::Null)?;
    s.ok("held below Paul, it neither reads nor answers them (403)", held_seen.status == 403 && held_yes.status == 403 && listed(api, &paul, &yes_to)?.is_none(), format!("{held_seen} {held_yes}"));
    let yes = api.signed(&hand, "POST", &format!("/api/invites/{yes_to}/accept?for={paul_id}"), Some(&json!({})))?;
    let no = api.signed(&hand, "POST", &format!("/api/invites/{no_to}/decline?for={paul_id}"), Some(&json!({})))?;
    let after = waiting(&paul, None)?;
    let left = after.body["invites"].as_array().into_iter().flatten().filter(|i| i["from"] == Api::email_of(&skyler).as_str()).count();
    let joined = api.signed(&skyler, "GET", &format!("/api/f/{yes_to}/members"), None)?;
    let row_in = |id: &str| joined.body["members"].as_array().into_iter().flatten().find(|m| m["principal"] == id).cloned().unwrap_or_default();
    let (paul_row, hand_row) = (row_in(&paul_id), row_in(&hand_id));
    s.ok(
        "its yes makes Paul a member, in his list, and his agents with him (decision 64), it among them; its no takes the other back; neither waits for him now",
        yes.status == 200
            && yes.body["role"] == "editor"
            && no.status == 200
            && listed(api, &paul, &yes_to)?.as_deref() == Some("editor")
            && listed(api, &paul, &no_to)?.is_none()
            && left == 0
            && paul_row["kind"] == "person"
            && hand_row["kind"] == "agent"
            && hand_row["owner"] == paul_id.as_str(),
        format!("{yes} {no} {after} {joined}"),
    );
    let deleted = hand_for_paul("DELETE", "", None)?;
    let capped = hand_for_paul("PUT", "cap", Some(json!({ "id": "agent-cap", "micros": 1 })))?;
    let still = api.status(&paul, &app)?;
    let asks = |r: &Reply| refused(r) && r.message().contains("to confirm it in a chat");
    s.ok(
        "for Paul, it deletes his app or sets its cap only on his confirm, on a card in a chat: a key with no chat to ask in is refused, saying so",
        asks(&deleted) && asks(&capped) && still.status == 200,
        json!({ "delete": deleted.body, "cap": capped.body, "status": still.status }),
    );
    let vis = ask(&hand, Some(&skyler_id), "PUT", &app, "%76isibility", Some(json!({ "visibility": "public" })))?;
    let cap = hand_for_paul("PUT", "%63ap", Some(json!({ "id": "agent-cap-2", "micros": 1 })))?;
    let st = api.status(&paul, &app)?;
    s.ok(
        "spelled so the router cannot tell, the fragment refuses alike: visibility for someone else, the cap for Paul",
        refused(&vis) && refused(&cap) && st.body["visibility"] == "members",
        json!({ "visibility": vis.body, "cap": cap.body, "now": st.body["visibility"] }),
    );
    Ok(())
}
