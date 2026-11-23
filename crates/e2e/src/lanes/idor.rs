//! Nobody reaches another person's data by naming it (FIN-584, the CASA
//! lab's broken-access-control check; docs/security/access-control.md).
//!
//! One members-only fragment, its owner, a viewer, an editor, and a
//! stranger (a signed-in person who is none of them): every route of a
//! fragment is asked as each, and each is refused where its role is below
//! the route's (`ROUTES`, the inventory's table). A refusal comes before
//! anything is read or written, so a read refused answers 401 or 403 and
//! never the thing, and a write refused leaves the fragment as it was.
//! Writes are asked only of those refused, so the fragment stays the same
//! fragment for every check. Then the routes keyed by a person rather than a
//! fragment: another's invite, own-list view, search across, and export,
//! each named by its id by someone it is not theirs.
//!
//! `idor` runs on the local node with levers (a member is added by the
//! test lever, as `members` adds one).

use anyhow::Result;
use fragment_nip98::Keys;
use serde_json::{json, Value};

use crate::api::{Api, Reply};
use crate::Suite;

/// The weakest member role a route needs, as the inventory names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Needs {
    Viewer,
    Editor,
    Owner,
}

/// Who asks: a member at a role, or nobody's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Asker {
    Stranger,
    Viewer,
    Editor,
}

impl Asker {
    fn may(self, needs: Needs) -> bool {
        match self {
            Asker::Stranger => false,
            Asker::Viewer => needs == Needs::Viewer,
            Asker::Editor => needs <= Needs::Editor,
        }
    }
}

/// One route of a fragment: method, path below `/api/f/{name}/`, body.
struct Route {
    method: &'static str,
    path: String,
    body: Option<Value>,
    needs: Needs,
}

/// A 64-hex hash no blob of the fragment's has.
const NO_BLOB: &str = "4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393";

fn r(method: &'static str, path: impl Into<String>, body: Option<Value>, needs: Needs) -> Route {
    Route { method, path: path.into(), body, needs }
}

/// The fragment routes a member's role gates (cell/src/fragment.rs `route`),
/// each with a body that reads, so a refusal is the role's and not the
/// body's. `/api/f/{name}/subscriptions` lists the caller's own (none for a
/// stranger) and is checked apart; the inbox takes its token, not a role.
fn routes() -> Vec<Route> {
    use Needs::*;
    vec![
        r("GET", "status", None, Viewer),
        r("GET", "address", None, Viewer),
        r("GET", "manifest", None, Viewer),
        r("GET", "members", None, Viewer),
        r("GET", "storage-token?access=read", None, Viewer),
        r("GET", "files", None, Viewer),
        r("GET", "file?path=site/index.html", None, Viewer),
        r("GET", "card", None, Viewer),
        r("GET", "icon", None, Viewer),
        r("GET", "events", None, Viewer),
        r("GET", "channels", None, Viewer),
        r("GET", "channels/events", None, Viewer),
        r("GET", "channels/events/newest", None, Viewer),
        r("GET", "runs", None, Viewer),
        r("GET", "runs/1", None, Viewer),
        r("GET", "triggers", None, Viewer),
        r("GET", "schedules", None, Viewer),
        r("GET", "search/status", None, Viewer),
        r("GET", format!("blobs/{NO_BLOB}"), None, Viewer),
        r("GET", "secrets", None, Editor),
        r("GET", "search/sources", None, Editor),
        r("GET", "storage-token?access=write", None, Editor),
        r("PUT", "secrets/IDOR_KEY", Some(json!("not yours")), Editor),
        r("DELETE", "secrets/IDOR_KEY", None, Editor),
        r("POST", "files", Some(json!({ "files": [{ "path": "site/idor.txt", "content": "not yours" }] })), Editor),
        r("POST", "deploy", Some(json!({})), Editor),
        r("POST", "refresh", Some(json!({})), Editor),
        r("POST", "card", None, Editor),
        r("DELETE", "icon", None, Editor),
        r("POST", "replay", Some(json!({ "run": 1 })), Editor),
        r("POST", "pause", Some(json!({ "op": "nothing", "paused": true })), Editor),
        r("POST", "schedules", Some(json!({ "cron": "0 * * * *", "channel": "inbox", "record": {} })), Editor),
        r("PUT", "schedules/1", Some(json!({ "paused": true })), Editor),
        r("DELETE", "schedules/1", None, Editor),
        r("GET", "invites", None, Owner),
        r("GET", "door", None, Owner),
        r("GET", "database", None, Owner),
        r("PUT", "visibility", Some(json!({ "visibility": "public" })), Owner),
        r("POST", "invites", Some(json!({ "email": "idor@e2e.test", "role": "editor" })), Owner),
        r("POST", "rotate", Some(json!({})), Owner),
        r("DELETE", "", None, Owner),
    ]
}

fn path(name: &str, route: &str) -> String {
    match route {
        "" => format!("/api/f/{name}"),
        _ => format!("/api/f/{name}/{route}"),
    }
}

/// What the owner sees of the fragment that a refused write would change:
/// its members, invites, visibility, files, secrets, schedules and pins.
fn state(api: &Api, owner: &Keys, name: &str) -> Result<Value> {
    let read = |route: &str| api.signed(owner, "GET", &path(name, route), None).map(|r| r.body);
    let status = read("status")?;
    Ok(json!({
        "members": read("members")?["members"],
        "invites": read("invites")?["invites"],
        "visibility": status["visibility"],
        "pins": status["pins"],
        "viewToken": status["viewToken"],
        "files": read("files")?,
        "secrets": read("secrets")?,
        "schedules": read("schedules")?,
    }))
}

pub fn idor(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("idor", &[crate::Need::Levers]) {
        return Ok(());
    }
    let (owner, viewer, editor, stranger) = (api.person()?, api.person()?, api.person()?, api.person()?);
    let name = s.named(api, &owner, "idor")?;
    s.create(api, &owner, &name)?;
    let r = api.signed(&owner, "PUT", &path(&name, "visibility"), Some(&json!({ "visibility": "members" })))?;
    anyhow::ensure!(r.status == 200, "members only: {r}");
    for (who, role) in [(&viewer, "viewer"), (&editor, "editor")] {
        let r = api.share(&name, who.pubkey_hex(), role)?;
        anyhow::ensure!(r.status == 200, "a {role} added: {r}");
    }
    let before = state(api, &owner, &name)?;
    for asker in [Asker::Stranger, Asker::Viewer, Asker::Editor] {
        let keys = match asker {
            Asker::Stranger => &stranger,
            Asker::Viewer => &viewer,
            Asker::Editor => &editor,
        };
        fragment_routes(s, api, keys, asker, &name)?;
    }
    let after = state(api, &owner, &name)?;
    s.ok("every write refused left the fragment as it was: its members, invites, visibility, share link, files, secrets, schedules and pins", before == after, json!({ "before": before, "after": after }));
    let r = api.signed(&stranger, "GET", &path(&name, "subscriptions"), None)?;
    s.ok("a stranger's subscriptions list is their own: none here", r.status == 200 && r.body["subscriptions"].as_array().is_some_and(Vec::is_empty), &r);
    let r = api.signed(&stranger, "GET", &path(&name, "status?view=x"), None)?;
    s.ok("a `view` on an API route lends nothing: the share link counts on the fragment's own site alone", r.status == 403, &r);
    let r = api.unsigned("GET", &path(&name, "status"), None)?;
    s.ok("unsigned, a members-only fragment's status is 401", r.status == 401, &r);
    by_person(s, api, &owner, &stranger, &name)
}

/// Each route as `keys` (`asker`): refused (403) below the route's role,
/// answered (not 401 or 403) at or above it, where reads alone are asked:
/// a read allowed may still find nothing (no card yet: 404), which says
/// nothing of who may ask.
fn fragment_routes(s: &mut Suite, api: &Api, keys: &Keys, asker: Asker, name: &str) -> Result<()> {
    let mut wrong: Vec<String> = vec![];
    let mut asked = 0;
    for route in routes() {
        let allowed = asker.may(route.needs);
        // a write is asked only of those it refuses: it would change the fragment
        if allowed && route.method != "GET" {
            continue;
        }
        asked += 1;
        let reply: Reply = api.signed(keys, route.method, &path(name, &route.path), route.body.as_ref())?;
        let refused = matches!(reply.status, 401 | 403);
        let fine = match allowed {
            false => reply.status == 403,
            true => !refused,
        };
        if !fine {
            wrong.push(format!("{} {} → {}", route.method, route.path, reply));
        }
    }
    let who = match asker {
        Asker::Stranger => "a stranger (signed in, no member) is refused every route of a members-only fragment (403), reads and writes",
        Asker::Viewer => "a viewer reads what a viewer may and is refused (403) every editor's and owner's route",
        Asker::Editor => "an editor reads what it may and is refused (403) every owner's route",
    };
    s.ok(&format!("{who} ({asked} routes)"), wrong.is_empty(), wrong.join("\n"));
    Ok(())
}

/// Routes keyed by a person or by an id that is someone else's.
fn by_person(s: &mut Suite, api: &Api, owner: &Keys, stranger: &Keys, name: &str) -> Result<()> {
    // an invite to someone else's email: not the stranger's to answer
    let r = api.signed(owner, "POST", &path(name, "invites"), Some(&json!({ "email": "invited-idor@e2e.test", "role": "viewer" })))?;
    anyhow::ensure!(r.status == 200, "an invite: {r}");
    let yes = api.signed(stranger, "POST", &format!("/api/invites/{name}/accept"), Some(&json!({})))?;
    let role = api.signed(stranger, "GET", &path(name, "status"), None)?;
    s.ok("an invite to another's email is no one else's to accept (404), and makes them nothing", yes.status == 404 && role.status == 403, format!("{yes} {role}"));
    // an own-list view names the asker's list alone
    let archived = api.signed(stranger, "PUT", &format!("/api/fragments/{name}/archived"), Some(&json!({ "archived": true })))?;
    let theirs = api.signed(owner, "GET", "/api/fragments", None)?;
    let still = theirs.body["fragments"].as_array().and_then(|a| a.iter().find(|f| f["name"] == name)).map(|f| f["archived"].clone());
    s.ok(
        "archiving someone else's fragment changes nothing of theirs: their list is as it was",
        archived.status != 200 || still.as_ref().is_some_and(|a| a.is_null() || *a == json!(false)),
        format!("{archived} {still:?}"),
    );
    // a search across names fragments: one not theirs is denied, its words never answered
    let r = api.signed(stranger, "POST", "/api/search", Some(&json!({ "q": "anything", "in": [name], "mode": "lexical" })))?;
    let denied = r.body["fragments"].as_array().and_then(|a| a.first()).is_some_and(|f| f["state"] == "denied");
    s.ok("a search across someone else's fragment is denied for it, with no results", r.status == 200 && denied && r.body["results"].as_array().is_some_and(Vec::is_empty), &r);
    // an export is its person's: another's id is none of theirs
    let pressed = api.signed(owner, "POST", "/api/export", None)?;
    match pressed.body["id"].as_str().filter(|id| fragment_proto::export::valid_export_id(id)) {
        Some(id) => {
            let r = api.signed(stranger, "GET", &format!("/api/export/{id}/download"), None)?;
            s.ok("another person's export, named by its id, is not found (404)", r.status == 404, &r);
        }
        None => s.skip("another person's export, named by its id, is not found (404)", &format!("no export was started here: {pressed}")),
    }
    // a computer is found by its owner's identity; another's id is no one's
    let theirs = fragment_core::computer::default_computer_of(&api.identity(owner)?);
    for (method, route) in [("GET", ""), ("POST", "/wake"), ("POST", "/sleep"), ("PUT", "/image")] {
        let body = (method != "GET").then(|| json!({ "image": "stub" }));
        let r = api.signed(stranger, method, &format!("/api/computers/{theirs}{route}"), body.as_ref())?;
        s.ok(&format!("{method} someone else's computer{route} is not found (404), as one never made"), r.status == 404, &r);
    }
    Ok(())
}
