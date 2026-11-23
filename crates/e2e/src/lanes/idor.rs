//! Nobody reaches another person's data by naming it (FIN-584, the CASA
//! lab's broken-access-control check; docs/security/access-control.md).
//!
//! One members-only fragment, its owner, a viewer, an editor, and a
//! stranger (a signed-in person who is none of them). The owner first
//! makes what the routes name: a file, a blob, a subscription. Every route
//! in `routes` is then asked as each: below the route's role the answer is
//! 403 exactly (never the thing), and at or above it a read answers what
//! the owner's own read answers, which is a success except where the table
//! says the fixture has none (no card, icon, run or app code: the owner's
//! 404 too). Writes are asked only of those they refuse, so the fragment
//! stays the same fragment, and the owner's view of it after is the view
//! before. Then the routes keyed by a person rather than a fragment:
//! another's invite, subscription and own-list view, and search across.
//!
//! What this section does not ask, and where it is asked: an operation's
//! call and a channel post need app code and declared channels (the `ops`
//! and `posts` sections); another person's export, ready, is the `export`
//! section's; another's computer, one that exists, is the `computers`
//! section's.
//!
//! `idor` runs on the local node with levers and fakes (a member is added
//! by the test lever, as `members` adds one; a subscription's URL is the
//! push fake's).

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

/// One route of a fragment: method, path below `/api/f/{name}/`, body
/// (raw bytes for a blob), and whether the owner's read of it, on this
/// fixture, finds nothing (`absent`: their 404 too).
struct Route {
    method: &'static str,
    path: String,
    body: Option<Value>,
    raw: Option<Vec<u8>>,
    needs: Needs,
    absent: bool,
}

fn r(method: &'static str, path: impl Into<String>, body: Option<Value>, needs: Needs) -> Route {
    Route { method, path: path.into(), body, raw: None, needs, absent: false }
}

/// A read the owner's fixture has nothing for: the owner's answer is 404.
fn none_yet(path: &str, needs: Needs) -> Route {
    Route { absent: true, ..r("GET", path, None, needs) }
}

/// What the owner made before anyone else asks.
struct Made {
    file: String,
    blob: String,
    /// Bytes another blob upload sends, and their hash.
    other: (Vec<u8>, String),
    victim: String,
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

/// The fragment routes a member's role gates (cell/src/fragment.rs `route`),
/// each with a body that parses, so a refusal is the role's and not the
/// body's. Subscriptions list and remove the caller's own, and are asked
/// apart (`by_person`); the inbox takes its token, not a role.
fn routes(m: &Made) -> Vec<Route> {
    use Needs::*;
    vec![
        r("GET", "status", None, Viewer),
        r("GET", "address", None, Viewer),
        r("GET", "manifest", None, Viewer),
        r("GET", "members", None, Viewer),
        r("GET", "storage-token?access=read", None, Viewer),
        r("GET", "files", None, Viewer),
        r("GET", format!("file?path={}", m.file), None, Viewer),
        none_yet("card", Viewer),
        none_yet("icon", Viewer),
        r("GET", "events", None, Viewer),
        r("GET", "channels", None, Viewer),
        r("GET", "channels/events", None, Viewer),
        r("GET", "channels/events/newest", None, Viewer),
        r("GET", "runs", None, Viewer),
        none_yet("runs/1", Viewer),
        r("GET", "triggers", None, Viewer),
        r("GET", "schedules", None, Viewer),
        r("GET", "search/status", None, Viewer),
        r("GET", format!("blobs/{}", m.blob), None, Viewer),
        r("GET", "secrets", None, Editor),
        r("GET", "search/sources", None, Editor),
        // no `access` is a token that writes too (`StorageAccess::parse`)
        r("GET", "storage-token", None, Editor),
        r("PUT", "secrets/IDOR_KEY", Some(json!("not yours")), Editor),
        r("DELETE", "secrets/IDOR_KEY", None, Editor),
        r("POST", "files", Some(json!({ "files": [{ "path": "site/idor.txt", "text": "not yours\n" }] })), Editor),
        Route { raw: Some(m.other.0.clone()), ..r("PUT", format!("blobs/{}", m.other.1), None, Editor) },
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
        none_yet("database", Owner),
        r("PUT", "visibility", Some(json!({ "visibility": "public" })), Owner),
        r("POST", "invites", Some(json!({ "email": "idor@e2e.test", "role": "editor" })), Owner),
        r("POST", "rotate", Some(json!({})), Owner),
        // membership changes: someone added, a role raised, a member removed
        r("PUT", format!("members/{}", m.victim), Some(json!({ "role": "editor" })), Owner),
        r("DELETE", format!("members/{}", m.victim), None, Owner),
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
/// its members, invites, visibility, files, secrets, schedules,
/// subscriptions and pins.
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
        "subscriptions": read("subscriptions")?,
    }))
}

pub fn idor(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("idor", &[crate::Need::Levers, crate::Need::Fakes]) {
        return Ok(());
    }
    let (owner, viewer, editor, stranger) = (api.person()?, api.person()?, api.person()?, api.person()?);
    // a member whom the membership routes name: no asker is them, so no
    // asker's request is their own leaving
    let victim = api.person()?;
    let name = s.named(api, &owner, "idor")?;
    s.create(api, &owner, &name)?;
    let r = api.signed(&owner, "PUT", &path(&name, "visibility"), Some(&json!({ "visibility": "members" })))?;
    anyhow::ensure!(r.status == 200, "members only: {r}");
    for (who, role) in [(&viewer, "viewer"), (&editor, "editor"), (&victim, "viewer")] {
        let r = api.share(&name, who.pubkey_hex(), role)?;
        anyhow::ensure!(r.status == 200, "a {role} added: {r}");
    }
    let made = victims(api, &owner, &name, &victim, &format!("{}/notify", s.push.url))?;
    let routes = routes(&made);
    // the owner reads each read first: what an allowed asker must get too
    let mut owners = std::collections::BTreeMap::new();
    let mut unread = vec![];
    for route in routes.iter().filter(|r| r.method == "GET") {
        let reply = api.signed(&owner, "GET", &path(&name, &route.path), None)?;
        if reply.status != if route.absent { 404 } else { 200 } {
            unread.push(format!("GET {} → {reply}", route.path));
        }
        owners.insert(route.path.clone(), reply.status);
    }
    s.ok("the owner reads every route the table asks (200), and finds nothing where the fixture has none (404)", unread.is_empty(), unread.join("\n"));
    let before = state(api, &owner, &name)?;
    for asker in [Asker::Stranger, Asker::Viewer, Asker::Editor] {
        let keys = match asker {
            Asker::Stranger => &stranger,
            Asker::Viewer => &viewer,
            Asker::Editor => &editor,
        };
        fragment_routes(s, api, keys, asker, &name, &routes, &owners)?;
    }
    let after = state(api, &owner, &name)?;
    s.ok("every write refused left the fragment as it was: its members, invites, visibility, share link, files, secrets, schedules, subscriptions and pins", before == after, json!({ "before": before, "after": after }));
    share_link(s, api, &owner, &stranger)?;
    let r = api.unsigned("GET", &path(&name, "status"), None)?;
    s.ok("unsigned, a members-only fragment's status is 401", r.status == 401, &r);
    by_person(s, api, &owner, &viewer, &stranger, &name)
}

/// A share link, real and valid, on a fragment it opens (visibility
/// `link`): on the fragment's own site it lets anyone read, signed or
/// not; on the control API it lends nothing, to a signed stranger or to
/// no one signed, whichever route it rides on.
fn share_link(s: &mut Suite, api: &Api, owner: &Keys, stranger: &Keys) -> Result<()> {
    let name = s.named(api, owner, "idor-link")?;
    s.create(api, owner, &name)?;
    let file = "site/linked.txt";
    let r = api.signed(owner, "POST", &path(&name, "files"), Some(&json!({ "files": [{ "path": file, "text": "the owner's, by link\n" }] })))?;
    anyhow::ensure!(r.status == 200, "the owner's file: {r}");
    let status = api.signed(owner, "GET", &path(&name, "status"), None)?;
    let view = status.body["viewToken"].as_str().unwrap_or_default().to_string();
    anyhow::ensure!(status.status == 200 && status.body["visibility"] == "link" && !view.is_empty(), "a link fragment with its share link: {status}");
    let site = |query: &str| api.call(crate::api::Call { method: "GET", url: api.site_url(&name, &format!("__file?path={file}{query}")), ..crate::api::Call::default() });
    let (with, without) = (site(&format!("&view={view}"))?, site("")?);
    s.ok(
        "on a link fragment's own site its share link lets an unsigned visitor read its file (200), and without it they read nothing",
        with.status == 200 && with.text.contains("the owner's, by link") && without.status != 200 && !without.text.contains("the owner's, by link"),
        format!("{with} {without}"),
    );
    let mut lent = vec![];
    for route in ["status", "members", "files", &format!("file?path={file}")] {
        let q = if route.contains('?') { "&" } else { "?" };
        let signed = api.signed(stranger, "GET", &path(&name, &format!("{route}{q}view={view}")), None)?;
        let unsigned = api.unsigned("GET", &path(&name, &format!("{route}{q}view={view}")), None)?;
        if signed.status != 403 || unsigned.status != 401 {
            lent.push(format!("{route}: signed {signed}, unsigned {unsigned}"));
        }
    }
    s.ok(
        "the same valid share link on the control API lends nothing: a signed stranger is 403 and no one signed is 401, on its status, members, files and a file",
        lent.is_empty(),
        lent.join("\n"),
    );
    Ok(())
}

/// What the owner makes for the routes to name: a file on main and a
/// `fragment.json` (so `manifest` has one to answer), a blob,
/// a subscription of theirs (to `hook`, the push fake), and the member a
/// membership change names (`victim`).
fn victims(api: &Api, owner: &Keys, name: &str, victim: &Keys, hook: &str) -> Result<Made> {
    let file = "site/owners.txt".to_string();
    let files = json!({ "files": [{ "path": file, "text": "the owner's\n" }, { "path": "fragment.json", "text": "{}\n" }] });
    let r = api.signed(owner, "POST", &path(name, "files"), Some(&files))?;
    anyhow::ensure!(r.status == 200, "the owner's file: {r}");
    let bytes = b"the owner's blob".to_vec();
    let blob = sha256(&bytes);
    let r = api.call(crate::api::Call {
        method: "PUT",
        url: format!("{}{}", api.base, path(name, &format!("blobs/{blob}"))),
        body: Some(bytes),
        content_type: Some("application/octet-stream"),
        keys: Some(owner),
        ..crate::api::Call::default()
    })?;
    anyhow::ensure!(r.status == 200, "the owner's blob: {r}");
    let r = api.signed(owner, "POST", &path(name, "subscriptions"), Some(&json!({ "channel": "events", "url": hook })))?;
    anyhow::ensure!(r.status == 200, "the owner's subscription: {r}");
    let other = b"someone else's blob".to_vec();
    let other_sha = sha256(&other);
    Ok(Made { file, blob, other: (other, other_sha), victim: victim.pubkey_hex().to_string() })
}

/// Each route as `keys` (`asker`): refused (403, exactly) below the
/// route's role; at or above it, a read answers what the owner's answered
/// (`owners`). Writes are asked only of those they refuse.
fn fragment_routes(s: &mut Suite, api: &Api, keys: &Keys, asker: Asker, name: &str, routes: &[Route], owners: &std::collections::BTreeMap<String, u16>) -> Result<()> {
    let mut wrong: Vec<String> = vec![];
    let mut asked = 0;
    for route in routes {
        let allowed = asker.may(route.needs);
        // a write is asked only of those it refuses: it would change the fragment
        if allowed && route.method != "GET" {
            continue;
        }
        asked += 1;
        let reply: Reply = match &route.raw {
            Some(bytes) => api.call(crate::api::Call {
                method: route.method,
                url: format!("{}{}", api.base, path(name, &route.path)),
                body: Some(bytes.clone()),
                content_type: Some("application/octet-stream"),
                keys: Some(keys),
                ..crate::api::Call::default()
            })?,
            None => api.signed(keys, route.method, &path(name, &route.path), route.body.as_ref())?,
        };
        let fine = match allowed {
            false => reply.status == 403,
            true => owners.get(&route.path) == Some(&reply.status),
        };
        if !fine {
            wrong.push(format!("{} {} → {}", route.method, route.path, reply));
        }
    }
    let who = match asker {
        Asker::Stranger => "a stranger (signed in, no member) is refused every route of a members-only fragment (403), reads and writes",
        Asker::Viewer => "a viewer reads what a viewer may, as the owner does, and is refused (403) every editor's and owner's route",
        Asker::Editor => "an editor reads what it may, as the owner does, and is refused (403) every owner's route",
    };
    s.ok(&format!("{who} ({asked} routes)"), wrong.is_empty(), wrong.join("\n"));
    Ok(())
}

/// Routes keyed by a person or by an id that is someone else's.
fn by_person(s: &mut Suite, api: &Api, owner: &Keys, viewer: &Keys, stranger: &Keys, name: &str) -> Result<()> {
    // an invite to someone else's email: not the stranger's to answer
    let r = api.signed(owner, "POST", &path(name, "invites"), Some(&json!({ "email": "invited-idor@e2e.test", "role": "viewer" })))?;
    anyhow::ensure!(r.status == 200, "an invite: {r}");
    let yes = api.signed(stranger, "POST", &format!("/api/invites/{name}/accept"), Some(&json!({})))?;
    let role = api.signed(stranger, "GET", &path(name, "status"), None)?;
    s.ok("an invite to another's email is no one else's to accept (404), and makes them nothing", yes.status == 404 && role.status == 403, format!("{yes} {role}"));
    // the owner's subscription: no one else lists or removes it. The list
    // and removal are scoped by the signer (theirs, or every one for the
    // owner: subscriptions.rs), so anyone signed is answered, with none
    // of the owner's
    let theirs = api.signed(owner, "GET", &path(name, "subscriptions"), None)?;
    let id = theirs.body["subscriptions"].as_array().and_then(|a| a.first()).and_then(|s| s["id"].as_i64());
    anyhow::ensure!(theirs.status == 200 && id.is_some(), "the owner lists their subscription: {theirs}");
    let id = id.unwrap_or_default();
    let listed = |keys: &Keys| api.signed(keys, "GET", &path(name, "subscriptions"), None);
    let (stranger_list, viewer_list) = (listed(stranger)?, listed(viewer)?);
    let none = |r: &Reply| r.status == 200 && r.body["subscriptions"].as_array().is_some_and(Vec::is_empty);
    s.ok("the owner's subscription is in no one else's list: a stranger's and a viewer's hold none", none(&stranger_list) && none(&viewer_list), format!("{stranger_list} {viewer_list}"));
    let gone_stranger = api.signed(stranger, "DELETE", &path(name, &format!("subscriptions/{id}")), None)?;
    let gone_viewer = api.signed(viewer, "DELETE", &path(name, &format!("subscriptions/{id}")), None)?;
    let still = api.signed(owner, "GET", &path(name, "subscriptions"), None)?;
    let kept = still.body["subscriptions"].as_array().is_some_and(|a| a.iter().any(|s| s["id"].as_i64() == Some(id)));
    s.ok(
        "the owner's subscription, named by its id, is no one else's to remove: a stranger and a viewer remove nothing, and it is still the owner's",
        [&gone_stranger, &gone_viewer].iter().all(|r| r.status == 200 && r.body["removed"] == 0) && kept,
        format!("{gone_stranger} {gone_viewer} {still}"),
    );
    // an own-list view names the asker's list alone
    let before = api.signed(owner, "GET", "/api/fragments", None)?;
    let row = |r: &Reply| r.body["fragments"].as_array().and_then(|a| a.iter().find(|f| f["name"] == name)).map(|f| f["archived"].as_bool().unwrap_or(false));
    anyhow::ensure!(row(&before) == Some(false), "the owner's list holds the fragment, not archived: {before}");
    let archived = api.signed(stranger, "PUT", &format!("/api/fragments/{name}/archived"), Some(&json!({ "archived": true })))?;
    let after = api.signed(owner, "GET", "/api/fragments", None)?;
    s.ok("archiving someone else's fragment changes nothing of theirs: the owner's row is still there, not archived", row(&after) == Some(false), format!("{archived} {after}"));
    // a search across names fragments: one not theirs is denied, its words never answered
    let r = api.signed(stranger, "POST", "/api/search", Some(&json!({ "q": "owner", "in": [name], "mode": "lexical" })))?;
    let denied = r.body["fragments"].as_array().and_then(|a| a.first()).is_some_and(|f| f["state"] == "denied");
    s.ok("a search across someone else's fragment is denied for it, with no results", r.status == 200 && denied && r.body["results"].as_array().is_some_and(Vec::is_empty), &r);
    Ok(())
}
