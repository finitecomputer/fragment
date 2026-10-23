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
    let r = add(false)?;
    s.ok("but alone, as itself, it reaches nothing (it is no member)", r.status == 403, &r);

    let r = api.op(&skyler, &name, "add_todo", "s1", json!({ "text": "from Skyler" }))?;
    s.ok("as Skyler does", r.status == 200, &r);
    // Paul shared with Skyler and his agents, not "Skyler and his agent
    // Juniper": none of Paul's views names it
    let members = api.signed(&paul, "GET", &format!("/api/f/{name}/members"), None)?;
    let access = api.signed(&paul, "GET", "/api/access", None)?;
    let paul_session = api.sign_in(&Api::email_of(&paul))?;
    let sheet = super::signin::with_session(api, "GET", &format!("/share/{name}"), &paul_session)?;
    s.ok(
        "Paul sees Skyler, never Skyler's agent: not in the members, the access view, nor the share sheet",
        members.status == 200
            && access.status == 200
            && sheet.status == 200
            && sheet.text.contains(&Api::email_of(&skyler))
            && [&members.text, &access.text, &sheet.text].iter().all(|t| !t.contains(&juniper_id) && !t.contains("With their agent")),
        format!("{members} {access}"),
    );

    let held = |by: &Keys, held: serde_json::Value| api.signed(by, "PUT", &format!("/api/identities/{juniper_id}/held"), Some(&json!({ "held": held })));
    let r = held(&paul, json!("viewer"))?;
    s.ok("no one but its owner holds an agent", r.status == 403, &r);
    let r = held(&skyler, json!("owner"))?;
    s.ok("an agent is held at viewer or editor, never owner", r.status == 400, &r);
    let r = held(&skyler, json!("viewer"))?;
    s.ok("Skyler holds the agent at viewer", r.status == 200, &r);
    let r = add(true)?;
    s.ok("held below Skyler, it no longer edits", r.status == 403, &r);
    let q = api.signed(&juniper, "POST", &format!("/api/f/{name}/ops/count?for={skyler_id}"), Some(&json!({ "id": "q1", "input": {} })))?;
    s.ok("and still reads", q.status == 200, &q);
    held(&skyler, Value::Null)?;
    let r = add(true)?;
    s.ok("let go, it edits again", r.status == 200, &r);
    sharing(s, api)
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
    let paul_row = joined.body["members"].as_array().into_iter().flatten().find(|m| m["principal"] == paul_id.as_str()).cloned().unwrap_or_default();
    s.ok(
        "its yes makes Paul a member (Paul, not it), in his list; its no takes the other back; neither waits for him now",
        yes.status == 200
            && yes.body["role"] == "editor"
            && no.status == 200
            && listed(api, &paul, &yes_to)?.as_deref() == Some("editor")
            && listed(api, &paul, &no_to)?.is_none()
            && left == 0
            && paul_row["kind"] == "person"
            && !joined.text.contains(&hand_id),
        format!("{yes} {no} {after} {joined}"),
    );
    let deleted = hand_for_paul("DELETE", "", None)?;
    let capped = hand_for_paul("PUT", "cap", Some(json!({ "id": "agent-cap", "micros": 1 })))?;
    let still = api.status(&paul, &app)?;
    s.ok(
        "for Paul, it never deletes his app nor sets its cap",
        refused(&deleted) && refused(&capped) && still.status == 200,
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
