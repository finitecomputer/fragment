//! The shell's platform (docs/cloudflare-v1.md, phase 5): what its page
//! asks of the API and the fragments it frames.
//!
//! The shell is the platform's own page, so it calls the API with the
//! person's platform session rather than a key: only a same-origin request
//! that carries the shell's header (and, writing, the platform's Origin)
//! is taken that way; a fragment's page on the same site, or any other,
//! is not. A chat or an agent is a fragment on a blessed template
//! (decision 40), which the platform's release serves: its repo names the
//! template and holds only its face and data. A person's list says what
//! each fragment is (its kind) and its title.

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::api::{Api, Call, Reply, Socket};
use crate::browser::{Browser, Page};
use crate::Suite;

/// A request as the shell's page sends it: the session cookie, the
/// shell's header, `Sec-Fetch-Site: same-origin`, and its Origin.
pub(super) fn shell(api: &Api, session: &str, method: &'static str, path: &str, body: Option<&Value>, extra: &[(&'static str, String)]) -> Result<Reply> {
    let mut headers: Vec<(&str, String)> = vec![("x-fragment-shell", "1".into()), ("sec-fetch-site", "same-origin".into()), ("origin", api.base.clone())];
    for (k, v) in extra {
        headers.retain(|(h, _)| h != k);
        headers.push((k, v.clone()));
    }
    api.call(Call {
        method,
        url: format!("{}{path}", api.base),
        body: body.map(|b| b.to_string().into_bytes()),
        content_type: body.map(|_| "application/json"),
        cookie: Some(format!("fragment_session={session}")),
        extra: headers.into_iter().filter(|(_, v)| !v.is_empty()).collect(),
        ..Call::default()
    })
}

/// A fragment's label: its name before its random suffix.
fn label_of(name: &str) -> &str {
    fragment_proto::split_fragment_name(name).map_or("", |(label, _)| label)
}

/// The person's own fragment labelled `label` (the shell names what it
/// makes from a label, with a random suffix), from their list.
fn own_named(api: &Api, session: &str, label: &str) -> Result<String> {
    let r = shell(api, session, "GET", "/api/fragments", None, &[])?;
    let list = r.body["fragments"].as_array().cloned().unwrap_or_default();
    let found = list.iter().find_map(|f| f["name"].as_str().filter(|n| f["role"] == "owner" && label_of(n) == label));
    found.map(str::to_string).with_context(|| format!("no fragment of theirs labelled {label}: {r}"))
}

pub fn shell_platform(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("shell", &[crate::Need::Fakes]) {
        return Ok(());
    }
    let email = format!("shell-{}@e2e.test", crate::api::now_s());
    let session = api.sign_in(&email)?;

    // the shell's own requests act as the signed-in person
    let r = shell(api, &session, "GET", "/api/identities/me", None, &[])?;
    let id = r.body["id"].as_str().unwrap_or("").to_string();
    s.ok("the shell's page reads the API as its signed-in person, with no key", r.status == 200 && r.body["kind"] == "person", &r);
    s.ok("someone new is someone at once: their own npub, and their email", fragment_core::npub::is_identity(&id) && r.body["email"] == email.as_str(), &r);
    let r = shell(api, &session, "GET", "/api/identities/me", None, &[("x-fragment-shell", String::new())])?;
    s.ok("a request without the shell's header is no one's (401)", r.status == 401, &r);
    let r = shell(api, &session, "GET", "/api/identities/me", None, &[("sec-fetch-site", "same-site".into())])?;
    s.ok("nor one from another origin of the site, a fragment's page (401)", r.status == 401, &r);
    let r = shell(api, &session, "POST", "/api/fragments", Some(&json!({ "label": "nope" })), &[("origin", String::new())])?;
    s.ok("nor a write without the platform's Origin (401)", r.status == 401, &r);
    let r = shell(api, &session, "POST", "/api/fragments", Some(&json!({ "label": "nope" })), &[("origin", api.site_origin("x--k3x9"))])?;
    s.ok("nor a write from a fragment's origin (401)", r.status == 401, &r);
    let r = shell(api, "f".repeat(64).as_str(), "GET", "/api/identities/me", None, &[])?;
    s.ok("a session that is not one is refused (401)", r.status == 401, &r);
    let r = shell(api, &session, "POST", &format!("/api/identities/{id}/keys"), Some(&json!({ "proof": "x" })), &[])?;
    s.ok("a key is added only by a key you hold, never the shell's session", r.status == 401, &r);

    // a chat and an agent, on blessed templates
    let r = shell(api, &session, "POST", "/api/fragments", Some(&json!({ "label": "juniper", "template": "agent", "title": "Juniper" })), &[])?;
    let agent = r.body["name"].as_str().unwrap_or("").to_string();
    s.ok("the shell makes an agent fragment on the agent template, named from its label", r.status == 200 && label_of(&agent) == "juniper", &r);
    let landed = s.eventually(std::time::Duration::from_secs(30), || {
        shell(api, &session, "GET", "/api/fragments", None, &[]).is_ok_and(|r| {
            r.body["fragments"].as_array().is_some_and(|l| l.iter().any(|f| f["name"] == agent.as_str() && f["kind"] == "agent" && f["title"] == "Juniper"))
        })
    });
    let list = shell(api, &session, "GET", "/api/fragments", None, &[])?;
    s.ok("the person's list says it is an agent, and its title", landed, &list);
    let manifest = shell(api, &session, "GET", &format!("/api/f/{agent}/manifest"), None, &[])?;
    s.ok("its repo names the template and its face, nothing else", manifest.body == json!({ "template": "agent", "meta": { "title": "Juniper" } }), &manifest);
    let page = api.call(Call { method: "GET", url: api.site_url(&agent, ""), cookie: None, ..Call::default() })?;
    let owner_page = shell_site(s, api, &session, &agent, "")?;
    s.ok(
        "its page is the release's template, with its own title",
        owner_page.status == 200 && owner_page.text.contains("agent.js") && owner_page.text.contains(r#"og:title" content="Juniper""#),
        format!("{} / anonymous {}", owner_page.status, page.status),
    );
    let script = shell_site(s, api, &session, &agent, "agent.js")?;
    s.ok("and its script, from the release", script.status == 200 && script.text.contains("SOUL.md"), script.status);
    release_assets(s, api, &session, &agent)?;
    let channels = shell(api, &session, "GET", &format!("/api/f/{agent}/channels"), None, &[])?;
    s.ok(
        "the template's channels run on it",
        channels.body["channels"].as_array().is_some_and(|c| c.iter().any(|c| c["name"] == "tasks")),
        &channels,
    );
    // its data is its own: a job, read from its repo by the release's page
    let r = shell(
        api,
        &session,
        "POST",
        &format!("/api/f/{agent}/files"),
        Some(&json!({ "files": [{ "path": "SOUL.md", "text": "Water the tomatoes.\n" }, { "path": "agent.json", "text": "{\"tier\":\"cheap\"}" }] })),
        &[],
    )?;
    let deploy = shell(api, &session, "POST", &format!("/api/f/{agent}/deploy"), Some(&json!({})), &[])?;
    s.ok("its job and settings are files of its own, deployed", r.status == 200 && deploy.status == 200, format!("{r} {deploy}"));
    let soul = shell_site(s, api, &session, &agent, "__file?path=SOUL.md")?;
    s.ok("which its page reads", soul.status == 200 && soul.text.contains("tomatoes"), soul.status);
    // an agent is no app: its deploys get no preview card (decision 31)
    let events = || shell(api, &session, "GET", &format!("/api/f/{agent}/events?tail=200"), None, &[]).map(|r| r.body).unwrap_or(Value::Null);
    let skipped = s.eventually(super::site::CARD_WAIT, || events()["events"].as_array().is_some_and(|l| l.iter().any(|e| e["kind"] == "card.skipped" && e["data"]["why"] == "not_an_app")));
    let card = shell(api, &session, "GET", &format!("/api/f/{agent}/card"), None, &[])?;
    s.ok("an agent fragment is not shot for a card (it is no app): 404", skipped && card.status == 404, format!("{card} | {}", events()));

    // code of its own is a fork's, refused while it names the template
    let r = shell(
        api,
        &session,
        "POST",
        &format!("/api/f/{agent}/files"),
        Some(&json!({ "files": [{ "path": "fragment.json", "text": "{\"template\":\"agent\",\"channels\":{\"x\":{\"read\":\"viewer\",\"post\":\"editor\"}}}" }] })),
        &[],
    )?;
    shell(api, &session, "POST", &format!("/api/f/{agent}/deploy"), Some(&json!({})), &[])?;
    let refused = s.eventually(std::time::Duration::from_secs(30), || {
        shell(api, &session, "GET", &format!("/api/f/{agent}/status"), None, &[]).is_ok_and(|r| r.body["code"]["error"].as_str().is_some_and(|e| e.contains("fork it")))
    });
    s.ok("a fragment on a template that declares code of its own is refused, saying to fork", r.status == 200 && refused, "");

    // connections (decisions 22 and 37): every provider the deployment
    // offers, its kind and the person's state there
    let status = |r: &Reply| r.body["providers"].as_array().and_then(|l| l.iter().find(|c| c["provider"] == crate::SWAP_CONNECTION)).map(|c| c["state"].clone());
    let r = shell(api, &session, "GET", "/api/connections", None, &[])?;
    let rows = r.body["providers"].as_array().cloned().unwrap_or_default();
    let row = |p: &str| rows.iter().find(|x| x["provider"] == p).cloned().unwrap_or(Value::Null);
    let keys = crate::SWAP_KEYS.iter().all(|(name, _, env)| {
        let k = row(name);
        k["kind"] == "operator" && k["state"] == "offered" && k["env"] == json!([env]) && k["price"]["micros"].as_i64().is_some_and(|m| m > 0)
    });
    s.ok(
        "the shell lists every provider the deployment offers: the connection not connected yet, the operator's keys offered at their prices, own keys not set (one signed in to, with its models)",
        status(&r) == Some(json!("not_connected"))
            && row(crate::SWAP_CONNECTION)["kind"] == "connection"
            && keys
            && row(crate::SWAP_OWN)["state"] == "not_set"
            && row(crate::SWAP_ROUTER)["state"] == "not_set"
            && row(crate::SWAP_ROUTER)["signIn"]["manage"].is_string()
            && row(crate::SWAP_ROUTER)["models"].as_array().is_some_and(|m| !m.is_empty())
            && rows.len() == crate::SWAP_KEYS.len() + 4,
        &r,
    );
    let r = shell(api, &session, "POST", "/api/connections/perplexity/authorize", Some(&json!({})), &[])?;
    s.ok("an operator key is no connection to authorize (400)", r.status == 400, &r);
    let r = shell(api, &session, "POST", &format!("/api/connections/{}/authorize", crate::SWAP_CONNECTION), Some(&json!({})), &[])?;
    s.ok("a connection without browser-bound OAuth configured fails closed", r.status >= 400 && r.body["url"].is_null(), &r);
    // The connections lane exercises the browser-bound import. This lane
    // tests the shell's read of an existing account at its vendor boundary.
    s.workos.connect(&email, crate::SWAP_CONNECTION, true);
    let r = shell(api, &session, "GET", "/api/connections", None, &[])?;
    s.ok("the shell reads the connected account without issuing consent", status(&r) == Some(json!("connected")), &r);
    let r = shell(api, &session, "POST", "/api/connections/notion/authorize", Some(&json!({})), &[])?;
    s.ok("a provider the deployment does not offer is none to connect (404)", r.status == 404, &r);

    // a template that is not blessed is copied, and the title its maker
    // gives it is its fragment.json's, over the template's: the face every
    // member's list shows
    let r = shell(api, &session, "POST", "/api/fragments", Some(&json!({ "label": "orchard", "template": "todo", "title": "Orchard rota" })), &[])?;
    let todo = r.body["name"].as_str().unwrap_or("").to_string();
    let titled = s.eventually(std::time::Duration::from_secs(30), || {
        shell(api, &session, "GET", "/api/fragments", None, &[]).is_ok_and(|r| r.body["fragments"].as_array().is_some_and(|l| l.iter().any(|f| f["name"] == todo.as_str() && f["title"] == "Orchard rota")))
    });
    let manifest = shell(api, &session, "GET", &format!("/api/f/{todo}/manifest"), None, &[])?;
    s.ok(
        "a copied template's fragment takes the title its maker gives it, over the template's, and its list says so",
        r.status == 200 && titled && manifest.body["meta"]["title"] == "Orchard rota" && manifest.body["meta"]["description"].as_str().is_some_and(|d| d.contains("todo")) && manifest.body["operations"]["add"].is_object(),
        &manifest,
    );
    let r = shell(api, &session, "POST", "/api/fragments", Some(&json!({ "label": "bare", "title": "Bare" })), &[])?;
    s.ok("a title needs a template: a fragment made bare says its own in fragment.json (400)", r.status == 400, &r);
    let r = shell(api, &session, "POST", "/api/fragments", Some(&json!({ "label": "lab", "template": "nope" })), &[])?;
    s.ok("a template that is none is refused, naming the blessed ones", r.status == 400 && r.text.contains("agent"), &r);
    search_and_archive(s, api, &session)?;
    search_follows_reading(s, api)?;
    unread_and_seen(s, api, &session)?;
    list_watch(s, api, &session, &id)
}

/// A chat's `unread` in a person's list (docs/api.md, Control API): the
/// messages their search holds after the last they marked seen (`PUT
/// /api/fragments/{name}/seen`), each person's own. Valid: a chat counts
/// every message before any mark (the person's own from elsewhere too),
/// a mark clears it and tells their open shell, a new message counts one;
/// another member's count is theirs alone. Replay: a mark again answers
/// the same. Invalid: a body, a name that is none, a bare label, a
/// fragment not theirs, no one. Leaving the chat drops the mark: back in,
/// everything counts again.
fn unread_and_seen(s: &mut Suite, api: &Api, session: &str) -> Result<()> {
    let r = shell(api, session, "POST", "/api/fragments", Some(&json!({ "label": "unread-talk", "template": "chat", "title": "Unread talk" })), &[])?;
    let chat = r.body["name"].as_str().unwrap_or("").to_string();
    anyhow::ensure!(r.status == 200, "making the chat: {r}");
    let post = |id: &str, text: &str| shell(api, session, "POST", &format!("/api/f/{chat}/channels/chat"), Some(&json!({ "id": id, "body": { "text": text } })), &[]);
    let took = s.eventually(SEARCH_WAIT, || post("u1", "first of three").is_ok_and(|r| r.status == 200));
    anyhow::ensure!(took && post("u2", "second of three")?.status == 200, "posting to the chat");
    let row_in = |r: &Reply| r.body["fragments"].as_array().and_then(|l| l.iter().find(|f| f["name"] == chat.as_str()).cloned()).unwrap_or(Value::Null);
    let mine = || shell(api, session, "GET", "/api/fragments", None, &[]).map(|r| row_in(&r)).unwrap_or(Value::Null);
    let counted = s.eventually(SEARCH_WAIT, || mine()["unread"] == 2);
    s.ok("a chat counts the messages its person's search holds, before any mark: two (their own, from elsewhere, too)", counted, mine());

    // marked seen: nothing unread, and their open shell is told
    let url = format!("{}/api/fragments/watch", api.base);
    let mut tab = watching(Socket::connect(api, &url, None, Some(&format!("fragment_session={session}")), Some(&api.base)).map(|(socket, _)| socket));
    let seen = |extra: Option<&Value>| shell(api, session, "PUT", &format!("/api/fragments/{chat}/seen"), extra, &[]);
    let r = seen(None)?;
    let told_it = told(&mut tab);
    s.ok(
        "PUT …/seen marks it: {name, unread: 0}, its row counts nothing (left out), and their open shell is told",
        r.status == 200 && r.body == json!({ "name": chat, "unread": 0 }) && mine()["unread"].is_null() && told_it == json!({ "type": "changed" }),
        json!([r.body, mine(), told_it]),
    );
    let again = seen(None)?;
    s.ok("marking it again answers the same", again.status == 200 && again.body == r.body, &again);
    if let Ok(w) = tab {
        w.close();
    }
    anyhow::ensure!(post("u3", "third of three")?.status == 200, "posting again");
    let one = s.eventually(SEARCH_WAIT, || mine()["unread"] == 1);
    s.ok("a message after the mark counts one", one, mine());

    // each person's own: a member never marked counts all three; theirs moves alone
    let member = api.person()?;
    let member_id = api.identity(&member)?;
    let r = shell(api, session, "PUT", &format!("/api/f/{chat}/members/{member_id}"), Some(&json!({ "role": "viewer" })), &[])?;
    anyhow::ensure!(r.status == 200, "adding the member: {r}");
    let theirs = || api.signed(&member, "GET", "/api/fragments", None).map(|r| row_in(&r)).unwrap_or(Value::Null);
    let three = s.eventually(SEARCH_WAIT, || theirs()["unread"] == 3);
    let r = api.signed(&member, "PUT", &format!("/api/fragments/{chat}/seen"), None)?;
    s.ok(
        "a member's count is theirs (all three, never marked); their mark (signed, with their key) clears theirs, and the person's stays one",
        three && r.status == 200 && theirs()["unread"].is_null() && mine()["unread"] == 1,
        json!({ "theirs": theirs(), "mine": mine(), "mark": r.body }),
    );
    // leaving drops their mark: back in, all three count again
    let out = shell(api, session, "DELETE", &format!("/api/f/{chat}/members/{member_id}"), None, &[])?;
    let back = shell(api, session, "PUT", &format!("/api/f/{chat}/members/{member_id}"), Some(&json!({ "role": "viewer" })), &[])?;
    let again = s.eventually(SEARCH_WAIT, || theirs()["unread"] == 3);
    s.ok("removed and added again, the member counts all three again (their mark went with them)", out.status == 200 && back.status == 200 && again, theirs());

    // refused: a body, no name, a bare label, not theirs, no one
    let body = seen(Some(&json!({ "n": 1 })))?;
    s.ok("a mark takes no body (400)", body.status == 400, &body);
    let r = shell(api, session, "PUT", "/api/fragments/Not%20A%20Name/seen", None, &[])?;
    s.ok("a name that is none is refused (400)", r.status == 400, &r);
    let r = shell(api, session, "PUT", "/api/fragments/unread-talk/seen", None, &[])?;
    s.ok("a bare label names nothing here (404)", r.status == 404 && r.message().contains("in full"), &r);
    let outsider = api.person()?;
    let r = api.signed(&outsider, "PUT", &format!("/api/fragments/{chat}/seen"), None)?;
    s.ok("a fragment they are not in is none of theirs to mark (404)", r.status == 404, &r);
    let r = api.call(Call { method: "PUT", url: format!("{}/api/fragments/{chat}/seen", api.base), ..Call::default() })?;
    s.ok("and no one unsigned marks anything (401)", r.status == 401, &r);
    Ok(())
}

/// A watch socket of a person's list, past its `hello`; or why not (the
/// refusal's status is in it).
fn watching(socket: Result<Socket>) -> Result<Socket, String> {
    socket.and_then(|mut w| w.expect("hello").map(|_| w)).map_err(|e| format!("{e:#}"))
}

/// The next frame a watch socket was sent, or why none came (its read
/// waits 5 s: a change reaches it in the request that made it).
fn told(w: &mut Result<Socket, String>) -> Value {
    match w {
        Ok(w) => w.next().unwrap_or_else(|e| json!(format!("{e:#}"))),
        Err(why) => json!(why.clone()),
    }
}

/// The person's list, watched (`GET /api/fragments/watch`; Paul on p5,
/// 2026-10-05: an app his agent made did not show in his sidebar until he
/// reloaded). Their Principal tells each socket only that the list
/// changed, and the page reads it again. Valid: the shell's socket (its
/// session, from the platform's own page) is told of a fragment made, one
/// shared with them by someone else, an archive, a removal and a delete;
/// a second tab's is told too, and a key's (the CLI's) of its own list.
/// Invalid: the session from any other page (no Origin, a fragment's), no
/// one, or no upgrade. Its frame names nothing. Past the most sockets a
/// list holds, one more is refused (429).
fn list_watch(s: &mut Suite, api: &Api, session: &str, id: &str) -> Result<()> {
    let url = format!("{}/api/fragments/watch", api.base);
    let cookie = format!("fragment_session={session}");
    let page = |origin: Option<&str>| Socket::connect(api, &url, None, Some(&cookie), origin).map(|(socket, _)| socket);
    let changed = json!({ "type": "changed" });
    let mut tab = watching(page(Some(&api.base)));
    let mut second = watching(page(Some(&api.base)));
    s.ok("the shell's page watches the person's list: a socket with its session, from the platform's own page", tab.is_ok() && second.is_ok(), json!([tab.as_ref().err(), second.as_ref().err()]));
    let r = shell(api, session, "POST", "/api/fragments", Some(&json!({ "label": "watched" })), &[])?;
    let made = r.body["name"].as_str().unwrap_or("").to_string();
    let (one, two) = (told(&mut tab), told(&mut second));
    s.ok("a fragment made is told to it at once, and to a second tab's, naming nothing", r.status == 200 && one == changed && two == changed, json!([r.status, one, two]));

    // someone else's fragment, shared with them; the CLI's socket is a key's
    let other = api.person()?;
    let mut cli = watching(Socket::connect(api, &url, Some(&other), None, None).map(|(socket, _)| socket));
    let theirs = s.named(api, &other, "lent")?;
    let r = api.create(&other, &theirs)?;
    let own = told(&mut cli);
    s.ok("a key's socket (the CLI's, naming no page) watches its own list: told of a fragment it made", r.status == 200 && own == changed, json!([r.status, own]));
    let r = api.signed(&other, "PUT", &format!("/api/f/{theirs}/members/{id}"), Some(&json!({ "role": "viewer" })))?;
    let shared = told(&mut tab);
    let list = shell(api, session, "GET", "/api/fragments", None, &[])?;
    let listed = list.body["fragments"].as_array().is_some_and(|l| l.iter().any(|f| f["name"] == theirs.as_str() && f["role"] == "viewer"));
    s.ok("one someone else shares with them is told, and their list then has it", r.status == 200 && shared == changed && listed, json!([r.status, shared, list.body]));
    let r = shell(api, session, "PUT", &format!("/api/fragments/{made}/archived"), Some(&json!({ "archived": true })), &[])?;
    let (one, two) = (told(&mut tab), told(&mut second));
    s.ok("an archive in one tab is told to every tab", r.status == 200 && one == changed && two == changed, json!([r.status, one, two]));
    let r = api.signed(&other, "DELETE", &format!("/api/f/{theirs}/members/{id}"), None)?;
    let removed = told(&mut tab);
    s.ok("their removal from someone else's is told", r.status == 200 && removed == changed, json!([r.status, removed]));
    let r = shell(api, session, "DELETE", &format!("/api/f/{made}"), None, &[])?;
    let deleted = told(&mut tab);
    s.ok("and a delete of their own", r.status == 200 && deleted == changed, json!([r.status, deleted]));

    // a page that is not the platform's own, or no one, watches nothing
    let refused = |socket: Result<Socket>| watching(socket).err().unwrap_or_else(|| "opened".into());
    let unnamed = refused(page(None));
    s.ok("the session from an upgrade that names no page is no one's (401)", unnamed.contains("401"), &unnamed);
    let fragment = refused(page(Some(&api.site_origin("x--k3x9"))));
    s.ok("nor from a fragment's page, one site with the platform (401)", fragment.contains("401"), &fragment);
    let nobody = refused(Socket::connect(api, &url, None, None, Some(&api.base)).map(|(socket, _)| socket));
    s.ok("and no one watches no list (401)", nobody.contains("401"), &nobody);
    let r = shell(api, session, "GET", "/api/fragments/watch", None, &[])?;
    s.ok("it is a socket: a request without the upgrade is refused (400)", r.status == 400, &r);

    // a list holds at most LIST_WATCHERS_MAX sockets (two are open)
    let mut more = vec![];
    for _ in 2..fragment_proto::limits::LIST_WATCHERS_MAX {
        more.push(watching(page(Some(&api.base))));
    }
    let past = refused(page(Some(&api.base)));
    s.ok(
        &format!("a list holds {} sockets at most: one more is refused (429)", fragment_proto::limits::LIST_WATCHERS_MAX),
        more.iter().all(Result::is_ok) && past.contains("429"),
        json!({ "opened": more.iter().filter(|w| w.is_ok()).count(), "past": past }),
    );
    for w in more.into_iter().chain([tab, second, cli]).flatten() {
        w.close();
    }
    Ok(())
}

/// How long a message takes to reach a person's search: its fragment's
/// alarm sends it, and a backoff waits on a list that holds no role yet.
const SEARCH_WAIT: std::time::Duration = std::time::Duration::from_secs(30);
/// The first chat's title (cell/shell/shell.js, `FIRST_CHAT`): its own,
/// never its agent's name.
const FIRST_CHAT: &str = "Your first chat";

/// A search's message hits in `fragment`.
fn hits_in(r: &Reply, fragment: &str) -> Vec<Value> {
    r.body["messages"].as_array().map(|l| l.iter().filter(|m| m["fragment"] == fragment).cloned().collect()).unwrap_or_default()
}

/// `q`, as a URL's query value.
fn query(q: &str) -> String {
    crate::api::url_enc(q)
}

/// Search (decision 9) and archiving, the person's own view (docs/api.md,
/// Search): a message's words find it, for the chat's people only, in
/// fragments they are in now; a query is words, never FTS5 syntax; a
/// person archives a fragment for themselves alone, twice the same as
/// once, and only one of theirs; a chat's row names its agents and shows
/// its newest message from each person's own search, so the shell's
/// sidebar is one read, told when a new message comes. Goal: the person's list is a fenced
/// projection (lesson 12). Method: three people (the shell's person, a
/// member who is removed and comes back, and an outsider with a chat of
/// their own), each asking their own list through the API.
fn search_and_archive(s: &mut Suite, api: &Api, session: &str) -> Result<()> {
    let r = shell(api, session, "POST", "/api/fragments", Some(&json!({ "label": "garden-talk", "template": "chat", "title": "Garden talk" })), &[])?;
    let chat = r.body["name"].as_str().unwrap_or("").to_string();
    anyhow::ensure!(r.status == 200 && label_of(&chat) == "garden-talk", "making the chat: {r}");
    let said = "Our tomatoes need water every Tuesday";
    let post = |id: &str, channel: &str, body: Value| shell(api, session, "POST", &format!("/api/f/{chat}/channels/{channel}"), Some(&json!({ "id": id, "body": body })), &[]);
    // the template's channels are the chat's as soon as it installs
    let took = s.eventually(SEARCH_WAIT, || post("m1", "chat", json!({ "text": said })).is_ok_and(|r| r.status == 200));
    let posted = post("m1", "chat", json!({ "text": said }))?;
    anyhow::ensure!(took && posted.body["replayed"] == true, "posting to the chat: {posted}");
    let seq = posted.body["record"]["seq"].as_i64().unwrap_or(0);
    // a record that is no message (an agent's step, a page's own kind) is
    // never searched. A step is no person's to post (`work` is its agents':
    // 403), so an agent of another person's, in the chat a moment, posts it.
    let step = post("w1", "work", json!({ "kind": "turn.step", "turn": "t1", "step": 1, "text": "zucchini plans" }))?;
    anyhow::ensure!(step.status == 403, "a person's step is refused: {step}");
    // (in a chat of their own: the first agent a chat takes is its own, for good)
    let r = shell(api, session, "POST", "/api/fragments", Some(&json!({ "label": "garden-steps", "template": "chat", "title": "Garden steps" })), &[])?;
    let steps = r.body["name"].as_str().unwrap_or("").to_string();
    anyhow::ensure!(r.status == 200, "making the steps' chat: {r}");
    let gardener = api.person()?;
    let (hand, hand_id) = super::delegation::agent_of(api, &gardener)?;
    let joined = shell(api, session, "PUT", &format!("/api/f/{steps}/members/{hand_id}"), Some(&json!({ "role": "editor" })), &[])?;
    anyhow::ensure!(joined.status == 200, "an agent joins the chat: {joined}");
    let step = api.signed(&hand, "POST", &format!("/api/f/{steps}/channels/work"), Some(&json!({ "id": "w1", "body": { "kind": "turn.step", "turn": "t1", "step": 1, "text": "zucchini plans" } })))?;
    anyhow::ensure!(step.status == 200, "the agent posts its step: {step}");
    let own = post("w2", "chat", json!({ "kind": "garden.note", "text": "zucchini cuttings" }))?;
    anyhow::ensure!(own.status == 200, "posting a page's own record: {own}");
    let search = |q: &str| shell(api, session, "GET", &format!("/api/search?q={}", query(q)), None, &[]);

    // a message's words find it, newest first, with where it is and a snippet
    let found = s.eventually(SEARCH_WAIT, || search("tomatoes").is_ok_and(|r| !hits_in(&r, &chat).is_empty()));
    let r = search("tomatoes")?;
    let hit = hits_in(&r, &chat).first().cloned().unwrap_or_default();
    s.ok(
        "search finds a message by its words: its chat, channel, record and a snippet",
        found && r.status == 200 && hit["channel"] == "chat" && hit["seq"] == seq && hit["at"].is_i64() && hit["snippet"].as_str().is_some_and(|t| t.contains("tomatoes")),
        &r,
    );
    let r = search("TOMAT tues")?;
    s.ok("every word, ignoring case, each a prefix", hits_in(&r, &chat).len() == 1, &r);

    // the chat's row in the person's list shows its newest message, from
    // their search, and their open shell is told when a new one arrives
    let row = |r: &Reply| r.body["fragments"].as_array().and_then(|l| l.iter().find(|f| f["name"] == chat.as_str()).cloned()).unwrap_or(Value::Null);
    let mine = || shell(api, session, "GET", "/api/fragments", None, &[]);
    let r = mine()?;
    s.ok(
        "the chat's row shows its newest message (the agent's step after it is none), and when it was said (previewAt: its record's time)",
        row(&r)["preview"] == said && row(&r)["previewAt"] == posted.body["record"]["at"] && posted.body["record"]["at"].is_i64(),
        &r,
    );
    let url = format!("{}/api/fragments/watch", api.base);
    let mut tab = watching(Socket::connect(api, &url, None, Some(&format!("fragment_session={session}")), Some(&api.base)).map(|(socket, _)| socket));
    if let Ok(w) = tab.as_mut() {
        w.patience(SEARCH_WAIT)?;
    }
    let basil = post("m2", "chat", json!({ "text": "Basil wants water too" }))?;
    anyhow::ensure!(basil.status == 200, "posting again: {basil}");
    let told_new = told(&mut tab);
    let r = mine()?;
    s.ok(
        "a new message is told to the person's open shell, and their list's row then shows it",
        told_new == json!({ "type": "changed" }) && row(&r)["preview"] == "Basil wants water too" && row(&r)["previewAt"] == basil.body["record"]["at"],
        json!([told_new, row(&r)]),
    );
    if let Ok(w) = tab {
        w.close();
    }
    let both = s.eventually(SEARCH_WAIT, || search("water").is_ok_and(|r| hits_in(&r, &chat).len() == 2));
    let r = search("water")?;
    let snippets: Vec<Value> = hits_in(&r, &chat).iter().map(|h| h["snippet"].clone()).collect();
    s.ok("two messages are two hits, the newest first, each with its own snippet", both && snippets == [json!("Basil wants water too"), json!(said)], &r);
    let r = search("tomatoes zucchini")?;
    s.ok("a word the message lacks finds nothing (an agent's step is not searched)", r.status == 200 && hits_in(&r, &chat).is_empty(), &r);
    let r = search("zucchini")?;
    s.ok("nor is a record that is no message (a step, a page's own record)", r.status == 200 && hits_in(&r, &chat).is_empty() && hits_in(&r, &steps).is_empty(), &r);
    let gone = shell(api, session, "DELETE", &format!("/api/f/{steps}"), None, &[])?;
    anyhow::ensure!(gone.status == 200, "deleting the steps' chat: {gone}");
    let r = search("cuttings")?;
    s.ok("a page's own record's words find nothing either", r.status == 200 && hits_in(&r, &chat).is_empty(), &r);
    let r = search("garden")?;
    let titled = r.body["fragments"].as_array().is_some_and(|l| l.iter().any(|f| f["name"] == chat.as_str() && f["kind"] == "chat" && f["title"] == "Garden talk"));
    s.ok("a fragment's title finds it, before any message", titled, &r);

    // FTS5's syntax in a query is words, never operators
    let r = search("tomatoes OR zucchini")?;
    s.ok("OR in a query is a word: no message has it, so nothing matches", r.status == 200 && hits_in(&r, &chat).is_empty(), &r);
    let r = search("\"tomat")?;
    s.ok("an unbalanced quote is text", r.status == 200 && hits_in(&r, &chat).len() == 1, &r);
    for q in ["NOT tomatoes", "text:tomatoes", "NEAR(tomatoes water)", "tomatoes AND", "*", "^water", "(", "{text}: water"] {
        let r = search(q)?;
        s.ok(&format!("{q:?} is a query like any other (200, no FTS5 error)"), r.status == 200 && r.body["messages"].is_array(), &r);
    }
    let r = shell(api, session, "GET", &format!("/api/search?q={}", "x".repeat(fragment_proto::limits::SEARCH_QUERY_MAX_BYTES + 1)), None, &[])?;
    s.ok("a query past its length is refused (400)", r.status == 400, &r);
    let r = shell(api, session, "GET", "/api/search", None, &[])?;
    s.ok("a search names its query (400)", r.status == 400, &r);
    let r = shell(api, session, "GET", "/api/search?q=a&q=b", None, &[])?;
    s.ok("once (400)", r.status == 400, &r);
    let r = api.call(Call { method: "GET", url: format!("{}/api/search?q=tomatoes", api.base), ..Call::default() })?;
    s.ok("and no one unsigned searches (401)", r.status == 401, &r);

    // a member finds what was said before they joined; an outsider never does
    let member = api.person()?;
    let outsider = api.person()?;
    let member_id = api.identity(&member)?;
    let r = shell(api, session, "PUT", &format!("/api/f/{chat}/members/{member_id}"), Some(&json!({ "role": "viewer" })), &[])?;
    anyhow::ensure!(r.status == 200, "adding the member: {r}");
    let theirs = |keys: &fragment_nip98::Keys, q: &str| api.signed(keys, "GET", &format!("/api/search?q={}", query(q)), None);
    let found = s.eventually(SEARCH_WAIT, || theirs(&member, "tomatoes").is_ok_and(|r| !hits_in(&r, &chat).is_empty()));
    s.ok("a new member's search finds what the chat said before they joined (signed, with their key)", found, theirs(&member, "tomatoes")?);

    // the chat's agents, the first added first, are named in every member's row
    let rows = || -> Result<(Value, Value)> { Ok((row(&mine()?), row(&api.signed(&member, "GET", "/api/fragments", None)?))) };
    let (_, lead) = super::delegation::agent_of(api, &member)?;
    let (_, second) = super::delegation::agent_of(api, &member)?;
    let added = [&lead, &second].map(|a| shell(api, session, "PUT", &format!("/api/f/{chat}/members/{a}"), Some(&json!({ "role": "viewer" })), &[]));
    let (my_row, their_row) = rows()?;
    s.ok(
        "agents added are named in every member's row, the first added first; the member's row shows the newest message from their own search",
        added.iter().all(|r| r.as_ref().is_ok_and(|r| r.status == 200))
            && my_row["agents"] == json!([lead, second])
            && their_row["agents"] == json!([lead, second])
            && their_row["preview"] == "Basil wants water too",
        json!([my_row, their_row]),
    );
    let removed = shell(api, session, "DELETE", &format!("/api/f/{chat}/members/{second}"), None, &[])?;
    let (my_row, their_row) = rows()?;
    s.ok("the second removed, every row names the lead left", removed.status == 200 && my_row["agents"] == json!([lead]) && their_row["agents"] == json!([lead]), json!([my_row, their_row]));
    // the lead is the chat's own agent: its owner keeps it (members.rs `chat_agent`)
    let refused = shell(api, session, "DELETE", &format!("/api/f/{chat}/members/{lead}"), None, &[])?;
    let (my_row, their_row) = rows()?;
    s.ok(
        "the lead, the chat's own agent, is not removed (400, saying why), and every row still names it",
        refused.status == 400 && refused.message().contains("this chat's own agent") && my_row["agents"] == json!([lead]) && their_row["agents"] == json!([lead]),
        json!([refused.body, my_row, their_row]),
    );
    let own = s.named(api, &outsider, "plot")?;
    let r = api.create_with(&outsider, json!({ "name": own, "template": "chat" }))?;
    anyhow::ensure!(r.status == 200, "the outsider's chat: {r}");
    let mine = api.signed(&outsider, "POST", &format!("/api/f/{own}/channels/chat"), Some(&json!({ "id": "o1", "body": { "text": "my tomatoes are fine" } })));
    let theirs_found = s.eventually(SEARCH_WAIT, || theirs(&outsider, "tomatoes").is_ok_and(|r| !hits_in(&r, &own).is_empty()));
    let r = theirs(&outsider, "tomatoes")?;
    s.ok(
        "someone not in the chat finds their own message, never the chat's",
        mine.is_ok_and(|m| m.status == 200) && theirs_found && hits_in(&r, &chat).is_empty(),
        &r,
    );
    let r = search("tomatoes")?;
    s.ok("nor does the chat's person find the outsider's", hits_in(&r, &own).is_empty() && hits_in(&r, &chat).len() == 1, &r);

    // archiving: the person's own view, and only of what they hold a role on
    let archived = |r: &Reply, name: &str| r.body["fragments"].as_array().and_then(|l| l.iter().find(|f| f["name"] == name).map(|f| f["archived"] == true));
    let list = |keys: Option<&fragment_nip98::Keys>| match keys {
        Some(k) => api.signed(k, "GET", "/api/fragments", None),
        None => shell(api, session, "GET", "/api/fragments", None, &[]),
    };
    let archive = |name: &str, body: Value| shell(api, session, "PUT", &format!("/api/fragments/{name}/archived"), Some(&body), &[]);
    let r = archive(&chat, json!({ "archived": true }))?;
    s.ok("the person archives a chat of theirs", r.status == 200 && r.body == json!({ "name": chat, "archived": true }), &r);
    let again = archive(&chat, json!({ "archived": true }))?;
    s.ok("archiving it again is the same", again.status == 200 && again.body == r.body, &again);
    let mine = list(None)?;
    s.ok("their list says it is archived", archived(&mine, &chat) == Some(true), &mine);
    let member_list = list(Some(&member))?;
    s.ok("a member's list does not", archived(&member_list, &chat) == Some(false), &member_list);
    let r = search("tomatoes")?;
    let r2 = search("garden")?;
    let titled = r2.body["fragments"].as_array().is_some_and(|l| l.iter().any(|f| f["name"] == chat.as_str() && f["archived"] == true));
    s.ok("search still finds it, and its messages, saying it is archived", hits_in(&r, &chat).len() == 1 && titled, &r2);
    let r = api.signed(&member, "PUT", &format!("/api/fragments/{chat}/archived"), Some(&json!({ "archived": true })))?;
    let unarchived = archive(&chat, json!({ "archived": false }))?;
    let (mine, member_list) = (list(None)?, list(Some(&member))?);
    s.ok(
        "the member archives it for themselves: the person unarchives it, and each list keeps its own",
        r.status == 200 && unarchived.status == 200 && archived(&mine, &chat) == Some(false) && archived(&member_list, &chat) == Some(true),
        json!({ "mine": archived(&mine, &chat), "member's": archived(&member_list, &chat) }),
    );
    let r = archive("garden-talk", json!({ "archived": true }))?;
    s.ok("a bare label names nothing here (404): a fragment is named in full", r.status == 404 && r.message().contains("in full"), &r);
    let r = archive(&own, json!({ "archived": true }))?;
    s.ok("a fragment they are not in is none of theirs to archive (404)", r.status == 404, &r);
    let r = archive("nothing-here--k3x9", json!({ "archived": true }))?;
    s.ok("nor one that does not exist (404)", r.status == 404, &r);
    let r = archive("Not A Name", json!({ "archived": true }))?;
    s.ok("a name that is none is refused (400)", r.status == 400, &r);
    let r = archive(&chat, json!({}))?;
    let r2 = archive(&chat, json!({ "archived": "yes" }))?;
    s.ok("a body without `archived`, or not a boolean, is refused (400)", r.status == 400 && r2.status == 400, format!("{r} {r2}"));

    // removed, a member's results go, and so does their archiving
    let r = shell(api, session, "DELETE", &format!("/api/f/{chat}/members/{member_id}"), None, &[])?;
    anyhow::ensure!(r.status == 200, "removing the member: {r}");
    let gone = s.eventually(SEARCH_WAIT, || theirs(&member, "tomatoes").is_ok_and(|r| r.status == 200 && hits_in(&r, &chat).is_empty()));
    let r = theirs(&member, "garden")?;
    let titled = r.body["fragments"].as_array().is_some_and(|l| l.iter().any(|f| f["name"] == chat.as_str()));
    s.ok("a removed member's search finds nothing of the chat, by its words or its title", gone && !titled, &r);
    let r = api.signed(&member, "PUT", &format!("/api/fragments/{chat}/archived"), Some(&json!({ "archived": false })))?;
    s.ok("nor may they archive it now (404)", r.status == 404, &r);
    let r = shell(api, session, "PUT", &format!("/api/f/{chat}/members/{member_id}"), Some(&json!({ "role": "viewer" })), &[])?;
    anyhow::ensure!(r.status == 200, "adding the member again: {r}");
    let back = s.eventually(SEARCH_WAIT, || theirs(&member, "tomatoes").is_ok_and(|r| hits_in(&r, &chat).len() == 1));
    let member_list = list(Some(&member))?;
    s.ok("back in, they find it again, once, and it is not archived for them", back && archived(&member_list, &chat) == Some(false), &member_list);

    // a deleted fragment's messages go from its people's search
    let r = api.signed(&outsider, "DELETE", &format!("/api/f/{own}"), None)?;
    let gone = s.eventually(SEARCH_WAIT, || theirs(&outsider, "tomatoes").is_ok_and(|r| r.status == 200 && hits_in(&r, &own).is_empty()));
    s.ok("a deleted fragment's messages leave its people's search", r.status == 200 && gone, theirs(&outsider, "tomatoes")?);
    Ok(())
}

/// Search follows who may read (#156, problem 5): a deploy that makes a
/// channel the editors' takes its messages from every list (only channels
/// every member reads are searched), and a member who joins after never
/// gets them. Method: an app with two postable channels, `talk` tightened
/// by a second deploy and `notes` left as it was; each wait ends on what
/// does arrive, never on an absence.
fn search_follows_reading(s: &mut Suite, api: &Api) -> Result<()> {
    const OPEN: &[u8] = br#"{ "channels": { "talk": { "read": "viewer", "post": "viewer" }, "notes": { "read": "viewer", "post": "viewer" } } }"#;
    const CLOSED: &[u8] = br#"{ "channels": { "talk": { "read": "editor", "post": "editor" }, "notes": { "read": "viewer", "post": "viewer" } } }"#;
    let (owner, viewer, late) = (api.person()?, api.person()?, api.person()?);
    let name = s.named(api, &owner, "readers")?;
    let c = s.create(api, &owner, &name)?;
    s.commit(&c, &[("fragment.json", Some(OPEN))]);
    s.deploy(&c);
    let add = |keys: &fragment_nip98::Keys| api.signed(&owner, "PUT", &format!("/api/f/{name}/members/{}", keys.pubkey_hex()), Some(&json!({ "role": "viewer" })));
    anyhow::ensure!(add(&viewer)?.status == 200, "adding the viewer");
    let post = |channel: &str, id: &str, text: &str| api.signed(&owner, "POST", &format!("/api/f/{name}/channels/{channel}"), Some(&json!({ "id": id, "body": { "text": text } })));
    let search = |keys: &fragment_nip98::Keys| api.signed(keys, "GET", "/api/search?q=rhubarb", None);
    let took = s.eventually(SEARCH_WAIT, || post("talk", "t1", "the rhubarb is ready").is_ok_and(|r| r.status == 200));
    let found = s.eventually(SEARCH_WAIT, || search(&viewer).is_ok_and(|r| hits_in(&r, &name).len() == 1));
    s.ok("a viewer finds a message on a channel every member reads", took && found, search(&viewer)?);

    s.commit(&c, &[("fragment.json", Some(CLOSED))]);
    s.deploy(&c);
    let gone = s.eventually(SEARCH_WAIT, || search(&viewer).is_ok_and(|r| r.status == 200 && hits_in(&r, &name).is_empty()));
    let read = api.signed(&viewer, "GET", &format!("/api/f/{name}/channels/talk"), None)?;
    s.ok("a deploy that makes the channel the editors' takes its message from the viewer's search, as from their reading (403)", gone && read.status == 403, &read);
    let gone = s.eventually(SEARCH_WAIT, || search(&owner).is_ok_and(|r| r.status == 200 && hits_in(&r, &name).is_empty()));
    s.ok("and from the owner's: a channel only some members read is never searched", gone, search(&owner)?);

    anyhow::ensure!(add(&late)?.status == 200, "adding the late viewer");
    let posted = post("notes", "n1", "the rhubarb went to market")?;
    let caught_up = s.eventually(SEARCH_WAIT, || search(&late).is_ok_and(|r| !hits_in(&r, &name).is_empty()));
    let r = search(&late)?;
    s.ok(
        "a member who joins after finds the channels searched now, never the tightened one's old message",
        posted.status == 200 && caught_up && hits_in(&r, &name).iter().all(|h| h["channel"] == "notes"),
        &r,
    );
    Ok(())
}

/// A fragment's page, as the shell's person sees it once signed in there.
/// The release's files (cell/src/assets.rs), read from the cell's Static
/// Assets and served by the router alone, each as its route answered it
/// when the cell held its bytes: the shell's with their type, validator and
/// policy, a blessed template's site file as the release's bytes, and none
/// of the assets at a path of its own (`run_worker_first`).
fn release_assets(s: &mut Suite, api: &Api, session: &str, agent: &str) -> Result<()> {
    let js = fragment_templates::shell("shell.js").context("the release's shell.js")?;
    let r = api.unsigned("GET", "/__shell/shell.js", None)?;
    let tag = r.header("etag");
    s.ok(
        "the shell's script is the release's bytes, typed, revalidated by their hash, never sniffed",
        r.status == 200
            && r.bytes == js.bytes
            && r.header("content-type") == "text/javascript; charset=utf-8"
            && tag == format!("\"s-{}\"", &js.sha256[..20])
            && r.header("cache-control") == "no-cache"
            && r.header("x-content-type-options") == "nosniff",
        format!("{} {} bytes, {} {tag}", r.status, r.bytes.len(), r.header("content-type")),
    );
    let again = api.call(Call { method: "GET", url: format!("{}/__shell/shell.js", api.base), extra: vec![("if-none-match", tag)], ..Call::default() })?;
    let wallpaper = fragment_templates::shell("wallpaper.jpg").context("the release's wallpaper")?;
    let head = api.call(Call { method: "HEAD", url: format!("{}/__shell/wallpaper.jpg", api.base), ..Call::default() })?;
    s.ok(
        "asked again it is 304, and a HEAD names a file's size and type alone",
        again.status == 304 && again.bytes.is_empty() && head.status == 200 && head.header("content-length") == wallpaper.size.to_string() && head.header("content-type") == "image/jpeg" && head.bytes.is_empty(),
        format!("{} / HEAD {} {} {}", again.status, head.status, head.header("content-length"), head.header("content-type")),
    );
    let index = fragment_templates::shell("index.html").context("the release's shell page")?;
    let page = api.unsigned("GET", "/", None)?;
    // the release that answers: every answer of the platform's host names
    // it, and the page's meta, which the shell's script compares with them
    let release = api.unsigned("HEAD", "/healthz", None)?.header("x-fragment-deploy");
    let filled = fragment_templates::shell_page(std::str::from_utf8(index.bytes)?, &release);
    s.ok(
        "the shell's page is the release's, its meta naming the release that answered, under the platform's policy",
        page.status == 200
            && page.text == filled
            && page.text.contains(&format!(r#"<meta name="fragment-release" content="{release}">"#))
            && page.header("content-security-policy").contains("frame-ancestors 'none'")
            && page.header("x-frame-options") == "DENY"
            && page.header("cache-control") == "no-store",
        format!("{} {} bytes (the release's filled: {}), release {release}; {}", page.status, page.bytes.len(), page.text == filled, page.header("content-security-policy")),
    );
    let me = shell(api, session, "GET", "/api/identities/me", None, &[])?;
    let refused = shell(api, session, "GET", "/api/identities/me", None, &[("x-fragment-shell", String::new())])?;
    s.ok(
        "every answer of the platform's host names its release (x-fragment-deploy): its page, its files, the API's, a refusal's",
        fragment_core::release::valid(&release) && [&page, &r, &me, &refused].iter().all(|a| a.header("x-fragment-deploy") == release) && refused.status == 401,
        json!({ "release": release, "page": page.header("x-fragment-deploy"), "file": r.header("x-fragment-deploy"), "api": me.header("x-fragment-deploy"), "refused": refused.header("x-fragment-deploy") }),
    );
    let paths = [
        "/__shell/CREDITS.md".to_string(),
        "/__shell/index.html".into(),
        "/__shell/vendor/lucide-LICENSE".into(),
        format!("/{}", js.sha256),
        format!("/__shell/{}", js.sha256),
        format!("/assets/{}", js.sha256),
        format!("/build/assets/{}", js.sha256),
    ];
    let mut served = vec![];
    for path in &paths {
        let r = api.unsigned("GET", path, None)?;
        if r.status != 404 || r.bytes == js.bytes {
            served.push(format!("{path}: {}", r.status));
        }
    }
    s.ok("nothing else of cell/shell/, and no asset by its name, is served (404)", served.is_empty(), served.join(", "));
    let f = fragment_templates::blessed::site_file("agent", "site/agent.js").context("the release's agent.js")?;
    let r = shell_site(s, api, session, agent, "agent.js")?;
    let by_name = shell_site(s, api, session, agent, f.sha256)?;
    s.ok(
        "a blessed template's file is the release's bytes, and its fragment's host serves no asset by its name",
        r.status == 200 && r.bytes == f.bytes && by_name.status == 404 && by_name.bytes != f.bytes,
        format!("{} {} bytes / by its name {}", r.status, r.bytes.len(), by_name.status),
    );
    Ok(())
}

fn shell_site(_s: &Suite, api: &Api, session: &str, name: &str, path: &str) -> Result<Reply> {
    let token = super::signin::site_cookie(api, session, name)?;
    api.call(Call { method: "GET", url: api.site_url(name, path), cookie: Some(format!("fragment_site={token}")), ..Call::default() })
}

/// Sets a field of the page's and fires its input, as typing would.
fn fill(selector: &str, value: &str) -> String {
    format!(
        "(() => {{ const e = document.querySelector({selector:?}); if (!e) return false; e.value = {value:?}; e.dispatchEvent(new Event('input', {{ bubbles: true }})); return true; }})()"
    )
}

/// The shell in a browser (phase 5's exit, at desktop and phone sizes):
/// first run (the first agent), the agent's chat framed and
/// signed in on its own origin with the agent's answer in it, a second
/// agent, an app's window, settings (at `/settings`, which the address
/// keeps), the phone's layout, a read of the computer that fails from the
/// page's start (`refresh_failed_start_ui`), and last a guest told who is
/// signed in, who uses another account to sign in to this seat
/// (`guest_ui`). The agents
/// run on the stub image (Docker), as the computers section's do.
pub fn shell_ui(s: &mut Suite, api: &Api) -> Result<()> {
    // (the fakes and the deployment: a guest the operator's key makes,
    // signed out of the WorkOS fake's held session: `guest_ui`)
    if !s.section("shell-ui", &[crate::Need::Chrome, crate::Need::LocalDocker, crate::Need::Fakes, crate::Need::Deployment]) {
        return Ok(());
    }
    let Some(mut b) = s.browser()? else {
        s.ok("Chrome is installed for the shell's lane (set CHROME_BIN)", false, "no Chrome found");
        return Ok(());
    };
    let shots = s.dir("shell-ui");
    let wait = std::time::Duration::from_secs(30);
    let agent_wait = std::time::Duration::from_secs(120);
    // signed out, settings is the shell asking them to sign in, and back to settings
    let page = b.open(&format!("{}/settings", api.base))?;
    let asked = b.until(&page, "document.querySelector('#first-run-card a.primary')?.getAttribute('href') === '/auth/login?screen_hint=sign-up&return=%2Fsettings'", wait);
    s.ok("signed out, /settings is the shell asking them to sign in, and to come back to settings", asked, b.eval(&page, "document.body.innerText.slice(0, 200)")?);
    let other = b.eval(&page, "(() => { const a = document.querySelector('#first-run-card a.splash-secondary'); return { href: a?.getAttribute('href'), says: a?.textContent }; })()")?;
    s.ok(
        "and offers Sign In: a sign-in on AuthKit's sign-in screen, back to settings too",
        other == json!({ "href": "/auth/login?screen_hint=sign-in&return=%2Fsettings", "says": "Sign In" }),
        &other,
    );
    let logo = b.eval(
        &page,
        "(async () => { const i = document.querySelector('#first-run .first-run-logo'); if (!i) return null; const drawn = await i.decode().then(() => true, () => false); const r = await fetch(i.src); return { src: i.getAttribute('src'), drawn, status: r.status, type: r.headers.get('content-type') }; })()",
    )?;
    s.ok(
        "above it, Finite's logo, drawn: one of the shell's published files",
        logo == json!({ "src": "/__shell/finite-logo.svg", "drawn": true, "status": 200, "type": "image/svg+xml" }),
        &logo,
    );
    // evidence for a person: the signed-out page, on a desktop and a phone,
    // in its own fonts
    let _ = b.eval(&page, "document.fonts.ready.then(() => true)");
    let _ = b.screenshot(&page, &shots.join("signed-out.png"));
    b.viewport(&page, 375, 812, true)?;
    let _ = b.screenshot(&page, &shots.join("phone-375-signed-out.png"));
    b.close(page)?;

    let email = format!("shell-ui-{}@e2e.test", crate::api::now_s());
    let session = api.sign_in(&email)?;
    b.set_cookie(&format!("{}/", api.base), "fragment_session", &session)?;
    // The first run, held twice in its first page: its computer's making
    // (then failed once), and its agent's joining the chat (never let go:
    // the page is reloaded there). The chat is there to write in from the
    // start; what it shows of the making is what the platform said.
    let page = b.open_with_script(&format!("{}/", api.base), r#"
      if (window === window.top && !sessionStorage.getItem('e2e-reloaded')) {
        const fetch = window.fetch.bind(window);
        let heldComputer = false, heldJoin = false;
        window.fetch = async (url, options) => {
          if (url === '/api/computers' && options?.method === 'POST' && !heldComputer) {
            heldComputer = true;
            await new Promise(resolve => { window.releaseSetup = resolve; });
            return new Response(JSON.stringify({message: 'Setup temporarily unavailable'}), {status: 503});
          }
          if (/^\/api\/f\/[^/]+\/members\/npub1/.test(url) && options?.method === 'PUT' && !heldJoin) {
            heldJoin = true;
            window.joinHeld = true;
            await new Promise(() => {});
          }
          return fetch(url, options);
        };
      }
    "#)?;
    b.viewport(&page, 1280, 800, false)?;

    // first run: the default agent, made with no question asked (Paul,
    // 2026-10-03), its chat first and in the open (Skyler, 2026-10-12),
    // and nothing to choose before it: a seat (the e2e's people are the
    // deployment's plan's) skips the welcome
    let creating = b.until(&page, "!document.getElementById('layout').hidden && document.getElementById('first-run').hidden", wait);
    let first = b.eval(&page, "({ path: location.pathname, seats: document.querySelectorAll('#first-run-card [data-plan]').length, steps: [...document.querySelectorAll('#first-run-card .creating-steps li')].map((li) => li.textContent) })")?;
    s.ok(
        "signed in, a seat's shell makes their default agent at once, asking nothing, in the chat's column: no welcome, no seat to choose or wait for, at home's address",
        creating && first["path"] == "/" && first["seats"] == 0 && first["steps"] == json!([]),
        &first,
    );
    let opened = b.until(&page, "document.querySelectorAll('#chats .chat-row').length === 1 && !!document.querySelector('#frames iframe')", agent_wait);
    if !opened {
        let _ = b.screenshot(&page, &shots.join("creating-failed.png"));
    }
    let said = b.eval(&page, "({ rows: [...document.querySelectorAll('#chats .chat-row')].map((r) => r.textContent), notice: document.getElementById('notice').innerText.slice(0, 200) })")?;
    s.ok("its chat is made first and opens at once, the one row of their chats", opened, &said);
    // the chat's name as the sidebar holds it: a title like "Starfire 40K"
    // is labelled starfire-40k, so it is read, never guessed
    let key = b.eval(&page, "document.querySelector('#chats .chat-row')?.dataset.key ?? ''")?;
    let chat = key.as_str().and_then(|k| k.strip_prefix("chat:")).unwrap_or("").to_string();
    let first_label = label_of(&chat).trim_end_matches("-chat").to_string();
    let host = chat.clone();
    // while its computer is held: a card of what is being made, typing, no Send, no message
    let mut held = Value::Null;
    let drafting = s.eventually(wait, || {
        held = b
            .eval_in_frame(
                &page,
                &host,
                "(() => { const t = document.getElementById('text'); const c = document.querySelector('.setup-card'); if (!t || t.disabled || !c || !c.querySelector('.setup-steps li')) return null; \
                 t.value = 'my first draft'; t.dispatchEvent(new Event('input', { bubbles: true })); \
                 return { send: document.getElementById('send').disabled, steps: [...c.querySelectorAll('.setup-steps li')].map((l) => [l.className, l.textContent]), messages: document.querySelectorAll('#messages .msg').length }; })()",
            )
            .unwrap_or(Value::Null);
        held["steps"].as_array().is_some_and(|l| l.len() == 3)
    });
    let making = held["steps"][0][0] == "doing" && held["steps"][0][1].as_str().is_some_and(|t| t.starts_with("Making ")) && held["steps"][1][0] == "todo" && held["steps"][2][0] == "todo";
    s.ok(
        "while its making waits, the chat shows a card of it (no one's message, nothing in the agent's name), takes typing, and sends nothing",
        drafting && making && held["send"] == true && held["messages"] == 0,
        &held,
    );
    let _ = b.screenshot(&page, &shots.join("creating.png"));
    b.until(&page, "typeof window.releaseSetup === 'function'", wait);
    b.eval(&page, "(window.releaseSetup(), true)")?;
    let failed = s.eventually(wait, || {
        b.eval_in_frame(&page, &host, "document.querySelector('.setup-error')?.textContent.includes('Setup temporarily unavailable') && !!document.querySelector('.setup-steps li.failed') && !!document.querySelector('.setup-error button')").ok() == Some(Value::Bool(true))
    });
    s.ok("a step that fails says so in the chat, the platform's words, with Try again", failed, "");
    b.eval_in_frame(&page, &host, "(document.querySelector('.setup-error button').click(), true)")?;
    // tried again, it is made, and its joining held: still no Send
    let joining = b.until(&page, "window.joinHeld === true", agent_wait);
    let mut steps = Value::Null;
    let made = s.eventually(wait, || {
        steps = b
            .eval_in_frame(&page, &host, "({ steps: [...document.querySelectorAll('.setup-steps li')].map((l) => [l.className, l.textContent]), send: document.getElementById('send').disabled, draft: document.getElementById('text').value, error: !!document.querySelector('.setup-error') })")
            .unwrap_or(Value::Null);
        steps["steps"][0][0] == "done" && steps["steps"][1][0] == "doing"
    });
    let agents_in = |api: &Api| -> Vec<Value> {
        shell(api, &session, "GET", &format!("/api/f/{chat}/members"), None, &[])
            .map(|r| r.body["members"].as_array().into_iter().flatten().filter(|m| m["kind"] == "agent").map(|m| m["principal"].clone()).collect())
            .unwrap_or_default()
    };
    s.ok(
        "tried again, it says its agent is made (and is: theirs, on their computer) while it joins the chat, keeps what was typed, and Send stays off: the agent is not in the chat",
        joining && made && steps["send"] == true && steps["draft"] == "my first draft" && steps["error"] == false && agents_in(api).is_empty() && agent_ids(api, &session, &[&first_label]).is_ok(),
        &steps,
    );
    let _ = b.screenshot(&page, &shots.join("creating-joining.png"));
    // a reload there goes on from what was made: nothing is kept in the browser
    b.eval(&page, "(sessionStorage.setItem('e2e-reloaded', '1'), true)")?;
    b.reload(&page)?;
    let resumed = b.until(&page, &format!("document.querySelectorAll('#chats .chat-row').length === 1 && document.querySelector('#chats .chat-row')?.dataset.key === {}", js(&format!("chat:{chat}"))), agent_wait);
    let sendable = s.eventually(agent_wait, || {
        b.eval_in_frame(&page, &host, "(() => { const t = document.getElementById('text'); if (!t) return false; t.value = 'my first draft'; t.dispatchEvent(new Event('input', { bubbles: true })); return !document.getElementById('send').disabled; })()").ok() == Some(Value::Bool(true))
    });
    let joined = agents_in(api);
    s.ok("reloaded mid-run, the shell goes on from what was made (the same chat, no other), and Send turns on once the platform has the agent in it", resumed && sendable && joined.len() == 1, json!({ "agents": joined }));
    let ready = s.eventually(agent_wait, || b.eval_in_frame(&page, &host, "!document.querySelector('.setup-card') && document.getElementById('note').textContent === ''").ok() == Some(Value::Bool(true)));
    s.ok("once the agent follows its chat, the card goes", ready, "");
    let row = b.eval(&page, "document.querySelector('#sidebar-agent .title')?.textContent ?? null")?;
    let list = shell(api, &session, "GET", "/api/fragments", None, &[])?;
    let owned = |kind: &str| list.body["fragments"].as_array().into_iter().flatten().filter(|f| f["kind"] == kind && f["role"] == "owner").count();
    s.ok(
        "the sidebar shows their one agent, named, and the run made one agent and one chat, though it was tried twice and reloaded",
        row.as_str().is_some_and(|t| !t.is_empty()) && owned("agent") == 1 && owned("chat") == 1,
        json!({ "agent": row, "agents": owned("agent"), "chats": owned("chat") }),
    );

    // its chat's title is its own (Paul, 2026-10-16), and its job never
    // names it: its name is its agent fragment's title alone
    let first_name = row.as_str().unwrap_or("").to_string();
    let chat_row = js(&format!("#chats [data-key={}] .label", js(&format!("chat:{chat}"))));
    let shown = b.eval(&page, &format!("({{ row: document.querySelector({chat_row})?.textContent ?? null, header: document.getElementById('chat-title').textContent }})"))?;
    let listed = own_row_of(api, &session, &chat);
    s.ok(
        "their first chat is titled \"Your first chat\", never its agent's name: its row, the shell's header and its own title",
        listed["title"] == FIRST_CHAT && shown["row"] == FIRST_CHAT && shown["header"] == FIRST_CHAT && first_name != FIRST_CHAT,
        json!({ "listed": listed, "shown": shown, "agent": first_name }),
    );
    let soul = shell(api, &session, "GET", &format!("/api/f/{}/file?path=SOUL.md", own_named(api, &session, &first_label)?), None, &[])?;
    s.ok(
        "its first agent's job (its SOUL.md) says what it does and never its name",
        soul.status == 200 && soul.text.contains("default agent") && !first_name.is_empty() && !soul.text.contains(&first_name),
        json!({ "name": first_name, "soul": soul.text }),
    );
    let settings = shell(api, &session, "GET", &format!("/api/f/{}/file?path=agent.json", own_named(api, &session, &first_label)?), None, &[])?;
    s.ok(
        "the shell's first agent names the cheap tier (GLM-5.3 Flash), and no colour: its colour is its identity's",
        settings.status == 200 && settings.body["tier"] == "cheap" && settings.body.get("color").is_none(),
        &settings,
    );
    // ready is ready: its computer awake, and the agent following its chat
    let computers = shell(api, &session, "GET", "/api/computers", None, &[])?;
    let subs = shell(api, &session, "GET", &format!("/api/f/{chat}/subscriptions"), None, &[])?;
    let awake = computers.body["computers"][0]["phase"] == "awake";
    let follows = subs.body["subscriptions"].as_array().is_some_and(|l| l.iter().any(|x| x["wake"] == true && x["channel"] == "chat"));
    s.ok("as the card goes, its computer is awake and the agent follows the chat", awake && follows, json!({ "computer": computers.body["computers"][0]["phase"], "subscriptions": subs.body["subscriptions"] }));
    let first_id = agent_ids(api, &session, &[&first_label])?.remove(0);
    let (hello, seen) = said_hello(s, api, &session, &chat, &first_id, agent_wait);
    s.ok("the agent speaks first: its hello once, and no message of the person's before it", hello && seen.as_array().is_some_and(|l| l.len() == 1), &seen);
    // a reload once it is made shows no card and makes nothing
    b.reload(&page)?;
    let again = b.until(&page, "!document.getElementById('layout').hidden && document.querySelectorAll('#chats .chat-row').length === 1 && !!document.querySelector('#frames iframe')", wait);
    std::thread::sleep(std::time::Duration::from_secs(2));
    let quiet = b.eval_in_frame(&page, &host, "!document.querySelector('.setup-card')").ok() == Some(Value::Bool(true));
    let list = shell(api, &session, "GET", "/api/fragments", None, &[])?;
    let chats_now = list.body["fragments"].as_array().into_iter().flatten().filter(|f| f["kind"] == "chat").count();
    s.ok("reloaded once it is made, the chat shows no card, and nothing is made again", again && quiet && chats_now == 1, chats_now);
    let signed = b.until(&page, "[...document.querySelectorAll('#frames iframe')].some(f => !f.dataset.blocked)", wait);
    let _ = s.eventually(wait, || b.eval_in_frame(&page, &host, "!!document.querySelector('.msg.agent:not(.streaming)')").ok() == Some(Value::Bool(true)));
    let _ = b.screenshot(&page, &shots.join("first-hello.png"));
    // the person's first message, answered by an agent already up
    let t0 = std::time::Instant::now();
    let mut typed = false;
    while t0.elapsed() < wait && !typed {
        let sent = b.eval_in_frame(&page, &host, "(() => { const t = document.getElementById('text'); if (!t || t.disabled) return false; t.value = 'hello there, please help me water the garden'; t.dispatchEvent(new Event('input', { bubbles: true })); document.getElementById('say').requestSubmit(); return true; })()");
        typed = sent.ok() == Some(Value::Bool(true));
        if !typed {
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }
    let t1 = std::time::Instant::now();
    let mut answered = false;
    while typed && t1.elapsed() < wait && !answered {
        answered = b.eval_in_frame(&page, &host, "document.body.innerText").ok().and_then(|v| v.as_str().map(|t| t.contains("water the garden") && t.contains("echo:"))).unwrap_or(false);
        if !answered {
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }
    s.ok(
        "its chat is framed, signed in on the chat's own origin, and the agent answers the first message at once",
        signed && typed && answered,
        json!({ "host": host, "typed": typed, "answeredInMs": t1.elapsed().as_millis() as u64 }),
    );
    let sidebar = b.until(&page, "document.getElementById('layout').classList.contains('left-open') && document.getElementById('sidebar').getBoundingClientRect().width > 0", wait);
    s.ok("at a desktop's width the sidebar shows beside the chat", sidebar, "");
    s.ok("the chat frame's accessible name cannot show a tooltip over its contents; the chat's title carries it", b.eval(&page, "(() => { const f = [...document.querySelectorAll('#frames iframe')].find(f => !f.hidden); const n = document.getElementById('chat-title'); return !!f && !f.hasAttribute('title') && f.getAttribute('aria-label') === n.textContent && n.title === n.textContent; })()")? == true, "");
    let _ = b.screenshot(&page, &shots.join("desktop-chat.png"));
    doing_ui(s, api, &mut b, &page, &session, &chat, &shots)?;

    // another agent of theirs, made elsewhere (the CLI, or an agent): the
    // shell has no list of agents (one agent: Paul, 2026-10-13), so it
    // stays out of the sidebar, while its chat is a chat like any other
    let first_title = row.as_str().unwrap_or("").to_string();
    let (_, reader_chat, reader_id) = extra_agent(api, &session, "reader", "Reader", "keep my reading list")?;
    let reader_row = format!("document.querySelector({})", js(&format!("#chats [data-key={}]", js(&format!("chat:{reader_chat}")))));
    let one = b.until(
        &page,
        &format!("document.querySelectorAll('#primary-agent .sidebar-agent').length === 1 && document.querySelector('#sidebar-agent .title')?.textContent === {} && !!{reader_row}", js(&first_title)),
        agent_wait,
    );
    s.ok(
        "another agent of theirs stays out of the sidebar, which shows their one agent (the first), and its chat lists among their chats",
        one,
        b.eval(&page, "({ agent: document.getElementById('primary-agent').innerText, chats: document.getElementById('chats').innerText })")?,
    );
    let (hello, seen) = said_hello(s, api, &session, &reader_chat, &reader_id, agent_wait);
    anyhow::ensure!(hello, "the Reader's hello: {seen:?}");

    // what came to a chat not open, and another chat with the Reader (its
    // hello shown once it is open: no frame of it before)
    let first_title = row.as_str().unwrap_or("").to_string();
    let me = Person { session: &session, first: &first_label, first_title: &first_title };
    b.click(&page, &format!("#chats [data-key={}]", js(&format!("chat:{reader_chat}"))))?;
    b.until(&page, "document.getElementById('chat-title').textContent === 'Reader'", wait);
    let _ = s.eventually(wait, || b.eval_in_frame(&page, &reader_chat, "!!document.querySelector('.msg.agent:not(.streaming)')").ok() == Some(Value::Bool(true)));
    unread_ui(s, api, &mut b, &page, &me, &reader_chat, &shots)?;
    new_chat_ui(s, api, &mut b, &page, &me, &reader_chat, &shots)?;

    // both agents in one chat, search, and archiving, as the person uses them
    groups_ui(s, api, &mut b, &page, &me, &shots)?;
    // the person's agents in any chat's @, and one asking another
    roster_ui(s, api, &mut b, &page, &me, &shots)?;
    ask_cli(s, api, &me)?;

    // an app's window: the desktop's (its dock and its windows)
    let desktop = b.until(&page, "document.getElementById('layout').classList.contains('right-open') && document.getElementById('viewer').getBoundingClientRect().width > 0 && !!document.querySelector('#apps #add-app')", wait);
    // its windows (the agents' screens) closed: the desktop itself
    b.eval(&page, "([...document.querySelectorAll('#stack .pane button[aria-label=Close]')].forEach((b) => b.click()), true)")?;
    let bare = b.until(&page, "document.getElementById('viewer').classList.contains('is-empty') && document.getElementById('desktop-empty').getBoundingClientRect().height > 0", wait);
    let empty = b.eval(&page, "({ title: document.querySelector('#desktop-empty .desktop-empty-title')?.textContent ?? null, asks: [...document.querySelectorAll('#desktop-empty .desktop-suggestion')].map((b) => b.textContent), die: document.querySelector('#desktop-empty .prompt-dice')?.getAttribute('aria-label') ?? null, apps: document.querySelectorAll('#apps [data-key^=\"app:\"]').length })")?;
    s.ok(
        "at a desktop's width the desktop shows beside the chat: no apps yet, it says where they land and offers three ways to ask their agent for one, the first three of the twelve, with a die for three more, and its dock offers Add",
        desktop && bare && empty["apps"] == 0 && empty["title"] == "Your apps land here" && strings(&empty["asks"]) == APP_PROMPTS[..3] && empty["die"] == DIE_LABEL,
        &empty,
    );
    let _ = b.screenshot(&page, &shots.join("desktop-empty.png"));
    let asked_in = app_prompts_ui(s, api, &mut b, &page, &session, &empty["asks"], &shots)?;
    // an app made elsewhere, as the CLI makes one from a template (the
    // shell offers none): in the dock with no reload, its window on a click
    let home = cli_home(s, api, &session, "desktop-cli-home")?;
    let made = s.cli_json(api, &home, &["create", "groceries", "--template", "todo", "--title", "groceries", "--json"])?;
    let groceries = made["name"].as_str().unwrap_or("").to_string();
    let docked = b.until(&page, &format!("!!document.querySelector({})", js(&format!("#apps [data-key=\"app:{groceries}\"]"))), wait);
    b.eval(&page, &format!("(document.querySelector({})?.click(), true)", js(&format!("#apps [data-key=\"app:{groceries}\"]"))))?;
    let window = b.until(&page, "document.querySelectorAll('#apps [data-key^=\"app:\"]').length === 1 && document.querySelector('.viewer iframe') && document.getElementById('desktop-empty').hidden", wait);
    s.ok("an app the CLI makes from a template (`fragment create --template todo`) lands in the dock with no reload, and opens in a window on the desktop", label_of(&groceries) == "groceries" && docked && window, &made);
    // its card, once the platform has shot it: read with the shell's session,
    // shown as a blob URL (the shell's CSP allows `img-src blob:`), larger
    // over its icon while a pointer is on it
    let icon = "document.querySelector('#apps [data-key^=\"app:\"]')";
    let carded = b.until(
        &page,
        &format!("(() => {{ {icon}?.dispatchEvent(new PointerEvent('pointerenter', {{ pointerType: 'mouse' }})); const p = document.querySelector('.card-peek'); const i = p?.querySelector('img'); return !!p && !p.hidden && !!i && i.src.startsWith('blob:') && i.complete && i.naturalWidth === 1280 && i.naturalHeight === 800 && p.getBoundingClientRect().width === 320; }})()"),
        super::site::CARD_WAIT,
    );
    s.ok("a pointer over its dock icon shows its card, a blob URL of the 1280×800 shot, larger above it", carded, b.eval(&page, "document.querySelector('.card-peek')?.outerHTML.slice(0, 200) ?? null")?);
    // deployed (its card is shot after), its face is the title it was given, not its template's
    let named = b.eval(&page, &format!("{icon}?.querySelector('.label')?.textContent ?? null"))?;
    s.ok("and its icon is titled with the title its maker gave it, not its template's", named == "groceries", &named);
    let _ = b.screenshot(&page, &shots.join("desktop-app-card.png"));
    b.eval(&page, &format!("({icon}?.dispatchEvent(new PointerEvent('pointerleave')), true)"))?;
    let _ = b.screenshot(&page, &shots.join("desktop-app.png"));
    compose_elsewhere_ui(s, api, &mut b, &page, &session, &asked_in, &groceries)?;
    open_beside_ui(s, api, &mut b, &page, &me, &groceries, &shots)?;
    sidebar_live(s, api, &mut b, &page, &session, &shots)?;

    // settings, at /settings: what a person needs of their account, its
    // Loading… where the loaded page's content goes
    let id = super::signin::who(api, &session)?["id"].as_str().unwrap_or("").to_string();
    let loading = settings_loading(&mut b, &page, "document.getElementById('settings').click()", wait)?;
    s.ok(
        "settings load in the loaded page's own box: its layout, padding, content and heading where they stay once loaded (no jump), on a desktop",
        loading["ok"] == true,
        &loading,
    );
    let settings = b.until(&page, "location.pathname === '/settings' && ['Account', 'Credit', 'Computer', 'Sharing', 'Connections', 'Build with Claude Code or Codex'].every(h => document.getElementById('settings-page').innerText.toUpperCase().includes(h.toUpperCase()))", wait);
    let text = b.eval(&page, "document.getElementById('settings-page').innerText")?;
    let text = text.as_str().unwrap_or("");
    let mut has = |q: &str| b.eval(&page, &format!("!!document.querySelector({q:?})")).ok() == Some(serde_json::json!(true));
    s.ok(
        "settings, at /settings: the account (who they are, their identity, signing out; no Add another sign-in, which WorkOS answered as the same email), credit, computer, connections, and building with a coding agent",
        settings
            && text.contains(&email)
            && text.contains(&id)
            && !text.contains("Add another sign-in")
            && !text.contains("Sign-ins")
            && !has("#settings-page a[href^='/auth/link']")
            && has("#settings-page form[method=post][action='/auth/logout'] button"),
        &text[..text.len().min(600)],
    );
    let account = b.eval(&page, ACCOUNT_SEEN)?;
    s.ok(
        "settings' Account says who is signed in, the email in bold, beside Sign out and Use another account (back to settings); a seat's holder is told nothing a guest is",
        account_shows(&account, &email) && account["noSeat"] == json!([]) && !text.contains("You are a guest"),
        &account,
    );
    // a prompt for Claude Code or Codex naming this platform's own llms.txt,
    // copied whole; the CLI's install folded below it
    let prompt = format!("Read {}/llms.txt and help me build: ", api.base);
    let shown = b.eval(&page, "(() => { const p = document.getElementById('build-prompt'); const d = document.querySelector('#settings-page details.settings-install'); return { prompt: p?.value, size: p && getComputedStyle(p).fontSize, folded: d && !d.open, install: d?.querySelector('pre.command')?.textContent }; })()")?;
    s.ok(
        "settings' Build with Claude Code or Codex: a prompt naming this platform's llms.txt, its text 16px, and the install folded behind Install it yourself",
        shown["prompt"] == prompt.as_str()
            && shown["size"] == "16px"
            && shown["folded"] == true
            && shown["install"].as_str().is_some_and(|i| {
                i.contains("releases/latest/download/fragment-$(uname -s)-$(uname -m).tar.gz") && i.contains(&format!("fragment login --host {}", api.base))
            }),
        &shown,
    );
    b.eval(&page, "(() => { window.__copied = null; navigator.clipboard.writeText = async (text) => { window.__copied = text; }; const p = document.getElementById('build-prompt'); p.value += 'a chore wheel'; document.querySelector('#settings-page [data-action=copy-prompt]').click(); return true; })()")?;
    let copied = b.until(&page, &format!("window.__copied === {:?} && document.querySelector('#settings-page [data-action=copy-prompt]').textContent === 'Copied'", format!("{prompt}a chore wheel")), wait);
    s.ok("its Copy copies the prompt as the person finished it, and says so", copied, b.eval(&page, "window.__copied")?);
    // Sharing: what they own, each with who can open it (GET /api/access)
    let theirs = format!("#settings-access .access-row[data-fragment={groceries:?}]");
    let listed = b.until(&page, &format!("!!document.querySelector({theirs:?}) && document.querySelector({theirs:?}).textContent.includes('Anyone with the link')"), wait);
    s.ok(
        "settings' Sharing lists what they own, with who can open each",
        listed,
        b.eval(&page, "document.getElementById('settings-access')?.innerText ?? 'no Sharing section'")?,
    );
    add_skills_ui(s, api, &mut b, &page, &session)?;
    skills_ui(s, api, &mut b, &page, &session)?;
    connections_ui(s, api, &mut b, &page, &session, &email, &chat)?;
    models_ui(s, api, &mut b, &page, &session, &shots)?;
    computer_ui(s, api, &mut b, &page, &session, &shots)?;
    update_ui(s, api, &mut b, &page, &session, &shots)?;
    b.color_scheme(&page, "dark")?;
    let _ = b.screenshot(&page, &shots.join("desktop-settings-dark.png"));
    b.color_scheme(&page, "light")?;
    b.eval(&page, "(document.getElementById('settings-page').scrollTop = 1e6, true)")?;
    let _ = b.screenshot(&page, &shots.join("desktop-settings-end.png"));
    b.reload(&page)?;
    let kept = b.until(&page, "location.pathname === '/settings' && !document.getElementById('settings-page').hidden && document.getElementById('frames').hidden && [...document.querySelectorAll('#settings-page h2')].some((h) => h.textContent === 'Account')", wait);
    s.ok("a reload stays on settings", kept, b.eval(&page, "location.pathname")?);
    b.click(&page, "#chats .chat-row")?;
    let home = b.until(&page, "location.pathname === '/' && document.getElementById('settings-page').hidden && !document.getElementById('frames').hidden", wait);
    b.eval(&page, "history.back(), true")?;
    let back = b.until(&page, "location.pathname === '/settings' && !document.getElementById('settings-page').hidden", wait);
    b.eval(&page, "history.forward(), true")?;
    let forward = b.until(&page, "location.pathname === '/' && document.getElementById('settings-page').hidden", wait);
    s.ok("a chat opened from settings is at /, and back and forward walk between the two", home && back && forward, b.eval(&page, "location.pathname")?);
    // settings fill the work area: a window opened there (an agent's Open)
    // goes back to the open chat, the window beside it, seen
    b.click(&page, "#settings")?;
    let listed = b.until(&page, "location.pathname === '/settings' && [...document.querySelectorAll('#settings-page .settings-section button.row')].some((b) => b.textContent === 'Open')", wait);
    b.eval(&page, "([...document.querySelectorAll('#settings-page .settings-section button.row')].find((b) => b.textContent === 'Open')?.click(), true)")?;
    let seen = b.until(
        &page,
        "location.pathname === '/' && !document.getElementById('layout').classList.contains('settings-view') && document.getElementById('viewer').getBoundingClientRect().width > 0 && !!document.querySelector('#stack .pane iframe')",
        wait,
    );
    s.ok("an agent's Open in settings goes back to the chat, its window beside it", listed && seen, b.eval(&page, "({ path: location.pathname, view: document.getElementById('layout').className, panes: [...document.querySelectorAll('#stack .pane')].map((p) => p.dataset.key) })")?);

    release_ui(s, api, &mut b, &page, &session, &chat, &reader_chat, &groceries, &shots)?;

    // the phone: the list, then a chat
    b.color_scheme(&page, "light")?;
    b.viewport(&page, 375, 812, true)?;
    b.reload(&page)?;
    let phone = b.until(&page, "!document.getElementById('layout').hidden && document.querySelector('#frames iframe')", wait);
    let fits = b.eval(&page, "document.documentElement.scrollWidth <= innerWidth + 1")?;
    s.ok("on a phone it fits its width, a chat open", phone && fits == true, &fits);
    // the open chat names its agent once `__people` answers, in its
    // composer's placeholder ("Message Reader", which no innerText holds),
    // and the picture waits for it; the frame the shell shows names its
    // fragment (`data-fragment`), whose flat host its page is on
    let open = b.eval(&page, "[...document.querySelectorAll('#frames iframe')].find((f) => !f.hidden)?.dataset.fragment ?? null")?;
    let host = open.as_str().unwrap_or_default().to_string();
    let names = |p: &Value| p.as_str().is_some_and(|p| p.len() > "Message ".len() && p.starts_with("Message "));
    let t0 = std::time::Instant::now();
    let mut placeholder = Value::Null;
    // bounded: the wait, a look every 250 ms
    while !host.is_empty() && t0.elapsed() < wait {
        placeholder = b.eval_in_frame(&page, &host, "document.getElementById('text')?.placeholder ?? null").unwrap_or(Value::Null);
        if names(&placeholder) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    s.ok("and the open chat names its agent", names(&placeholder), json!({ "fragment": open, "placeholder": placeholder }));
    let _ = b.screenshot(&page, &shots.join("phone-chat.png"));
    // the list: the sidebar over the chat, its + the launcher, bottom right
    b.click(&page, "#toggle-left")?;
    // (once the list has slid in: its + is fixed within it while it moves)
    let fab_expr = "(() => { const r = document.getElementById('sidebar-new-chat').getBoundingClientRect(); return r.width >= 44 && r.right > innerWidth - 40 && r.bottom > innerHeight - 60; })()";
    let list = b.until(&page, &format!("document.getElementById('layout').classList.contains('left-open') && document.querySelector('.sidebar-shell').getBoundingClientRect().width >= innerWidth - 2 && {fab_expr}"), wait);
    let fab = b.eval(&page, &format!("({{ fab: {fab_expr}, at: (() => {{ const r = document.getElementById('sidebar-new-chat').getBoundingClientRect(); return [r.left, r.top, r.width, innerWidth, innerHeight]; }})() }})"))?;
    let _ = b.screenshot(&page, &shots.join("phone-list.png"));
    s.ok("on a phone the list covers the chat, its + the launcher at its bottom right, a thumb's size", list && fab["fab"] == true, &fab);
    b.eval(&page, "(document.getElementById('scrim').click(), true)")?;
    b.until(&page, "!document.getElementById('layout').classList.contains('left-open')", wait);
    // an app's window on a phone: the desktop is a sheet from the bottom, the
    // chat's top above it; its dock in it (a script's click: under the
    // phone's emulation a second pointer press right after the first
    // reaches no element of the page)
    b.eval(&page, "(document.getElementById('toggle-right').click(), true)")?;
    b.until(&page, "document.getElementById('layout').classList.contains('right-open') && !!document.querySelector('#apps [data-key^=\"app:\"]')", wait);
    b.eval(&page, "(document.querySelector('#apps [data-key^=\"app:\"]')?.click(), true)")?;
    let sheet = b.until(
        &page,
        "(() => { const v = document.getElementById('viewer').getBoundingClientRect(); return document.getElementById('layout').classList.contains('right-open') \
         && Math.abs(v.bottom - innerHeight) < 2 && Math.abs(v.width - innerWidth) < 2 && v.top > 24 && !!document.querySelector('#stack .pane iframe'); })()",
        wait,
    );
    let shown = b.eval(&page, "document.querySelector('#stack .pane')?.dataset.key?.replace(/^app:/, '') ?? ''")?;
    let shown = shown.as_str().unwrap_or_default().to_string();
    let _ = s.eventually(wait, || !shown.is_empty() && b.eval_in_frame(&page, &shown, "document.readyState").ok() == Some(json!("complete")));
    let _ = b.screenshot(&page, &shots.join("phone-sheet.png"));
    s.ok(
        "on a phone an app's window opens as a sheet from the bottom, the chat's top showing above it",
        sheet,
        b.eval(&page, "(() => { const v = document.getElementById('viewer').getBoundingClientRect(); return { top: v.top, bottom: v.bottom, width: v.width, innerWidth, innerHeight }; })()")?,
    );
    phone_ui(s, api, &mut b, &page, &session, &shots)?;
    // last: it renames the person's first agent, which the checks above name
    faces_ui(s, api, &mut b, &page, &me, &chat, &shots)?;
    b.close(page)?;
    // their computer unread from the page's start: theirs, then a new person's
    refresh_failed_start_ui(s, api, &mut b, &shots)?;
    // a guest, in this browser, who then signs in to this seat
    guest_ui(s, api, &mut b, &email, &shots)?;
    println!("      (screenshots: {})", shots.display());
    Ok(())
}

/// The page's visibility as `v` (`hidden` or `visible`), as a phone's app
/// switcher or another tab makes it (headless Chrome's page is always on
/// screen, so its document says so in place of the browser).
fn visibility(v: &str) -> String {
    format!(
        "(() => {{ Object.defineProperty(document, 'visibilityState', {{ configurable: true, get: () => {v:?} }}); \
         Object.defineProperty(document, 'hidden', {{ configurable: true, get: () => {v:?} === 'hidden' }}); document.dispatchEvent(new Event('visibilitychange')); return true; }})()"
    )
}

/// The chat's composer, ready for the person: its page has its hello.
const COMPOSER_READY: &str = "(() => { const t = document.getElementById('text'); return !!t && !t.disabled && document.getElementById('say')?.dataset.ready === '1'; })()";

/// A file chosen in a chat's composer, as its picker hands it over: how
/// many its composer holds then.
const PICK_FILE: &str = "(() => { const p = document.getElementById('attachment-picker'); const d = new DataTransfer(); d.items.add(new File(['kept through the deploy'], 'note.txt', { type: 'text/plain' })); \
    p.files = d.files; p.dispatchEvent(new Event('change')); return document.querySelectorAll('#attachments .attachment-chip').length; })()";

/// The chat's composer's files taken out again.
const REMOVE_FILE: &str = "(() => { for (const b of [...document.querySelectorAll('#attachments .attachment-remove')]) b.click(); return document.querySelectorAll('#attachments .attachment-chip').length === 0; })()";

/// A chat page's next post to its `chat` channel held (`window.__holdPosts`),
/// until `window.__postHeld()`, then answered as it would be, or refused
/// (`window.__failPosts`: a 400, which no retry of the library's takes back),
/// as a slow or failing network's are.
const HOLD_POSTS: &str = "(() => { if (!window.__realFetch) window.__realFetch = window.fetch; window.__postHeld = null; \
    window.fetch = async (url, o) => { if (String(url).includes('__op/channels/chat') && window.__holdPosts) { window.__holdPosts = false; const fail = window.__failPosts; \
      await new Promise((r) => { window.__postHeld = r; }); \
      if (fail) return new Response(JSON.stringify({ error: 'invalid', message: 'refused for the test' }), { status: 400, headers: { 'content-type': 'application/json' } }); } \
      return window.__realFetch(url, o); }; return true; })()";

/// The draft a chat's page keeps in its own origin's storage (chat.js `DRAFT_KEY`).
const DRAFT: &str = "JSON.parse(localStorage.getItem('fragment.chat.draft.v1'))";

/// The cookie a local fleet's answers to a request name its release by
/// (docs/api.md, Test levers): this browser meets a deploy.
const RELEASE_COOKIE: &str = "fragment_e2e_release";

/// A new release under an open shell (docs/api.md, The shell, "A new
/// release"): the release cookie makes every answer of the node to this
/// browser, and the shell's page, name another release, as a deploy does.
/// Valid: hidden, the page reloads at once onto it, where it was (its chat,
/// its app's window); on screen it waits, and reloads at the person's next
/// move unless a chat's page is busy with what no storage keeps: a file
/// chosen, a message that failed while newer words were typed, a retry of
/// it under way, a quote chosen, a file picked the moment the page hides
/// (heard before the reload is decided), and a file picked while a reload
/// is under way (its answer slow), which stops it. Words typed in a chat
/// are its page's to keep: typed a moment
/// before the reload (nothing waited for), they are in its composer after
/// it, and after a reload by hand too. Replay: the release it runs said
/// again, and the release it just left answering again (a deploy's old
/// side), reload nothing: no loop. The words are their person's: another
/// account signed in to the tab opens the same chat to an empty composer
/// (the first one's draft gone), and signing out or Use another account
/// has each chat's page let its draft go before the form is sent.
/// Invalid: a cookie naming no release, or none, is answered as the node's
/// own. The browser goes back to the node's own, and to its person,
/// whatever happened.
#[allow(clippy::too_many_arguments)]
fn release_ui(s: &mut Suite, api: &Api, b: &mut Browser, page: &Page, session: &str, chat: &str, other: &str, app: &str, shots: &std::path::Path) -> Result<()> {
    let checked = release_steps(s, api, b, page, session, chat, other, app, shots);
    b.set_cookie(&format!("{}/", api.base), "fragment_session", session)?;
    b.set_cookie(&format!("{}/", api.base), RELEASE_COOKIE, "")?;
    // the page as the node's own release answers it, its composer emptied (its draft with it), at home
    let _ = b.eval_in_frame(page, chat, &fill("#text", ""));
    std::thread::sleep(std::time::Duration::from_millis(500));
    b.eval(page, "(history.replaceState(null, '', '/'), true)")?;
    reload_shell(b, page)?;
    checked
}

/// The shell reloaded by hand, and up again: its new page's layout shown
/// and a chat's frame in it (the old page, marked, is not taken for it).
fn reload_shell(b: &mut Browser, page: &Page) -> Result<bool> {
    b.eval(page, "(window.__before_reload = true, true)")?;
    b.reload(page)?;
    Ok(b.until(page, "!window.__before_reload && !document.getElementById('layout').hidden && !!document.querySelector('#frames iframe')", std::time::Duration::from_secs(30)))
}

#[allow(clippy::too_many_arguments)]
fn release_steps(s: &mut Suite, api: &Api, b: &mut Browser, page: &Page, session: &str, chat: &str, other: &str, app: &str, shots: &std::path::Path) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let platform = format!("{}/", api.base);
    // a request as this browser's would be, the release cookie naming `id`
    let named = |method: &'static str, path: &str, id: &str| {
        let cookie = Some(format!("fragment_session={session}; {RELEASE_COOKIE}={id}"));
        let extra = vec![("x-fragment-shell", "1".to_string()), ("sec-fetch-site", "same-origin".to_string())];
        api.call(Call { method, url: format!("{}{path}", api.base), cookie, extra, ..Call::default() })
    };
    // a person's press on a row or a button, as a script's click (its
    // handler, wherever a pointer would be): whether it was there
    let press = |b: &mut Browser, selector: &str| -> Result<bool> {
        Ok(b.eval(page, &format!("(() => {{ const e = document.querySelector({}); e?.click(); return !!e; }})()", js(selector)))? == true)
    };
    // a negative check's look: past the shell's own wait after a press (0.4 s)
    let quiet = std::time::Duration::from_secs(3);
    let run = crate::api::now_s();
    let (next, after) = (format!("e2e-next-{run}"), format!("e2e-after-{run}"));
    let meta = |id: &str| format!("document.querySelector('meta[name=fragment-release]')?.content === {}", js(id));
    let shown = |name: &str| format!("[...document.querySelectorAll('#frames iframe')].find((f) => !f.hidden)?.dataset.fragment === {}", js(name));
    let window = format!("!!document.querySelector({})", js(&format!("#stack .pane[data-key=\"app:{app}\"] iframe")));
    let row = |name: &str| format!("#chats [data-key={}]", js(&format!("chat:{name}")));
    let composer = |b: &mut Browser| b.eval_in_frame(page, chat, "({ text: document.getElementById('text')?.value ?? null, files: document.querySelectorAll('#attachments .attachment-chip').length })").unwrap_or(Value::Null);
    let ready = |s: &Suite, b: &mut Browser| s.eventually(wait, || b.eval_in_frame(page, chat, COMPOSER_READY).ok() == Some(Value::Bool(true)));
    let me_id = super::signin::who(api, session)?["id"].as_str().unwrap_or("").to_string();

    // the cookie: one naming no release, or none at all, is answered as the node's own
    let own = api.unsigned("HEAD", "/healthz", None)?.header("x-fragment-deploy");
    let bad = named("HEAD", "/healthz", "not*a*release")?;
    let empty = named("HEAD", "/healthz", "")?;
    s.ok(
        "a release cookie naming no release, empty, or none at all, is answered as the node's own",
        fragment_core::release::valid(&own) && own != next && bad.header("x-fragment-deploy") == own && empty.header("x-fragment-deploy") == own,
        json!({ "own": own, "bad": bad.header("x-fragment-deploy"), "empty": empty.header("x-fragment-deploy") }),
    );

    // the shell, fresh on the node's own release: its first chat open, empty, and an app's window
    b.set_cookie(&platform, RELEASE_COOKIE, "")?;
    let fresh = reload_shell(b, page)?;
    anyhow::ensure!(fresh && b.until(page, &format!("!!document.querySelector({})", js(&row(chat))), wait), "the shell reloaded, its first chat listed");
    press(b, &row(chat))?;
    b.eval(page, &format!("(document.querySelector({})?.click(), true)", js(&format!("#apps [data-key=\"app:{app}\"]"))))?;
    let set = b.until(page, &format!("{} && {window}", shown(chat)), wait) && ready(s, b);
    let open = "({ title: document.getElementById('chat-title')?.textContent, shown: [...document.querySelectorAll('#frames iframe')].find((f) => !f.hidden)?.dataset.fragment, panes: [...document.querySelectorAll('#stack .pane')].map((p) => p.dataset.key), path: location.pathname })";
    anyhow::ensure!(set, "the shell's first chat ({chat}) and an app's window (app:{app}) open: {}", b.eval(page, open)?);
    b.eval_in_frame(page, chat, &fill("#text", ""))?;
    b.eval(page, "(window.__release = 1, true)")?;

    // a deploy, to this browser: every answer names another release, the page's too
    b.set_cookie(&platform, RELEASE_COOKIE, &next)?;
    let health = named("HEAD", "/healthz", &next)?;
    let me = named("GET", "/api/identities/me", &next)?;
    let fresh = named("GET", "/", &next)?;
    let others = shell(api, session, "GET", "/api/identities/me", None, &[])?;
    s.ok(
        "with the cookie, every answer of the node names that release (its health, the API's, the shell's page and its meta), calls inside it still work, and a request without it is answered as before",
        health.header("x-fragment-deploy") == next
            && me.status == 200
            && me.header("x-fragment-deploy") == next
            && fresh.text.contains(&format!(r#"<meta name="fragment-release" content="{next}">"#))
            && others.status == 200
            && others.header("x-fragment-deploy") == own,
        json!({ "health": health.header("x-fragment-deploy"), "api": [me.status, me.header("x-fragment-deploy")], "without": others.header("x-fragment-deploy") }),
    );
    // hidden, it asks which release runs now and reloads at once
    b.eval(page, &visibility("hidden"))?;
    let reloaded = b.until(page, &format!("!window.__release && {} && {} && {window}", meta(&next), shown(chat)), wait);
    s.ok(
        "a new release, and the page hidden (a phone's app switched away from): it reloads at once onto it, its chat open again and its app's window too",
        reloaded,
        b.eval(page, "({ mark: window.__release ?? null, release: document.querySelector('meta[name=fragment-release]')?.content, panes: [...document.querySelectorAll('#stack .pane')].map((p) => p.dataset.key) })")?,
    );

    // on screen, another release is waited on: nothing reloads under the person
    let ready_now = ready(s, b);
    b.eval(page, "(window.__release = 2, true)")?;
    b.set_cookie(&platform, RELEASE_COOKIE, &after)?;
    b.eval(page, &visibility("visible"))?;
    let seen = b.until(page, &format!("document.documentElement.dataset.releaseNext === {}", js(&after)), wait);
    std::thread::sleep(quiet);
    let stayed = b.eval(page, &format!("window.__release === 2 && {}", meta(&next)))? == true;
    s.ok("another release, seen as the page is shown again: on screen, it waits, the page as it was", ready_now && seen && stayed, json!({ "seen": seen, "stayed": stayed }));
    // a file chosen in a chat (which no storage keeps): its page says it is
    // busy, and the person's next move, another chat, reloads nothing
    let attached = b.eval_in_frame(page, chat, PICK_FILE)?;
    press(b, &row(other))?;
    let moved = b.until(page, &shown(other), wait);
    std::thread::sleep(quiet);
    let held = composer(b);
    let stayed = b.eval(page, &format!("window.__release === 2 && {}", meta(&next)))? == true;
    s.ok(
        "a chat's page busy with what no storage keeps (a file chosen) holds the reload: the person's next move, another chat, opens it and reloads nothing",
        attached == 1 && moved && stayed && held["files"] == 1,
        json!({ "composer": held, "stayed": stayed }),
    );
    // back in the first chat, the file gone: what else no storage keeps
    // holds the page hidden (Alex's review of #406)
    press(b, &row(chat))?;
    b.until(page, &shown(chat), wait);
    b.eval_in_frame(page, chat, REMOVE_FILE)?;
    let stays_hidden = |b: &mut Browser| -> Result<bool> {
        b.eval(page, &visibility("hidden"))?;
        std::thread::sleep(quiet);
        let stayed = b.eval(page, &format!("window.__release === 2 && {}", meta(&next)))? == true;
        b.eval(page, &visibility("visible"))?;
        Ok(stayed)
    };
    let in_chat = |s: &Suite, b: &mut Browser, expr: &str| s.eventually(wait, || b.eval_in_frame(page, chat, &format!("!!({expr})")).ok() == Some(Value::Bool(true)));
    // a message that fails while newer words are typed: its words are in its bubble alone
    b.eval_in_frame(page, chat, HOLD_POSTS)?;
    b.eval_in_frame(page, chat, "(window.__holdPosts = true, window.__failPosts = true, true)")?;
    b.eval_in_frame(page, chat, &format!("(() => {{ {}; document.getElementById('say').requestSubmit(); return true; }})()", fill("#text", "the message that fails")))?;
    let pending = in_chat(s, b, "typeof window.__postHeld === 'function'");
    b.eval_in_frame(page, chat, &fill("#text", "newer words"))?;
    b.eval_in_frame(page, chat, "(window.__postHeld(), true)")?;
    let failed = in_chat(s, b, "!!document.querySelector('.message-error .message-retry')");
    let stayed = stays_hidden(b)?;
    let held = composer(b);
    s.ok(
        "a message that failed while newer words were typed (its words in its bubble alone, with Retry) holds the reload: hidden, the page stays",
        pending && failed && stayed && held["text"] == "newer words",
        json!({ "pending": pending, "failed": failed, "stayed": stayed, "composer": held }),
    );
    // its retry under way holds it too; sent, the page is busy no more
    b.eval_in_frame(page, chat, "(window.__holdPosts = true, window.__failPosts = false, window.__postHeld = null, document.querySelector('.message-error .message-retry').click(), true)")?;
    let retrying = in_chat(s, b, "typeof window.__postHeld === 'function'");
    let stayed = stays_hidden(b)?;
    b.eval_in_frame(page, chat, "(window.__postHeld(), true)")?;
    let sent = in_chat(s, b, "!document.querySelector('.message-error') && !document.querySelector('.msg.user.mine.sending')");
    s.ok("a retry of it under way holds the reload too, and it is sent once let go", retrying && stayed && sent, json!({ "retrying": retrying, "stayed": stayed, "sent": sent }));
    // a quote chosen holds it, and unquoted, holds it no more
    b.eval_in_frame(page, chat, "([...document.querySelectorAll('.msg .reply')].pop()?.click(), true)")?;
    let quoting = in_chat(s, b, "!document.getElementById('quoting').hidden");
    let stayed = stays_hidden(b)?;
    let quoted = b.eval_in_frame(page, chat, "!document.getElementById('quoting').hidden")? == true;
    b.eval_in_frame(page, chat, "(document.getElementById('unquote').click(), true)")?;
    s.ok("a quote chosen holds the reload: hidden, the page stays, and the quote with it", quoting && stayed && quoted, json!({ "quoting": quoting, "stayed": stayed }));
    // a file picked and the page hidden at once: the page's word of it, sent
    // just before, is heard before the reload is decided
    b.eval_in_frame(page, chat, PICK_FILE)?;
    let stayed = stays_hidden(b)?;
    let held = composer(b);
    b.eval_in_frame(page, chat, REMOVE_FILE)?;
    s.ok("a file picked the moment the page is hidden is heard before the reload is decided: the page stays, the file kept", stayed && held["files"] == 1, json!({ "stayed": stayed, "composer": held }));
    // a reload under way (its answer slow, a phone's), a file picked before
    // the new page comes: the reload is stopped, the page and the file kept.
    // That it is under way (its `beforeunload`) is seen from another page
    // of the platform's origin, through their storage: the page's own
    // session is held from the moment its navigation starts until it ends,
    // so a look at the page itself can miss the moment and see only the new
    // page come in (Alex's, from #430)
    std::thread::sleep(std::time::Duration::from_millis(300));
    let leaving_key = js(&format!("e2e-leaving-{run}"));
    let witness = b.open(&format!("{platform}healthz"))?;
    let cleared = format!("location.pathname === '/healthz' && (localStorage.removeItem({leaving_key}), true)");
    anyhow::ensure!(b.until(&witness, &cleared, wait), "a page of the platform's origin opens, to see the shell's reload begin");
    b.eval(page, &format!("(addEventListener('beforeunload', () => localStorage.setItem({leaving_key}, '1'), {{ once: true }}), true)"))?;
    b.latency(page, 4_000)?;
    b.eval(page, &visibility("hidden"))?;
    let leaving = b.until(&witness, &format!("localStorage.getItem({leaving_key}) === '1'"), wait);
    // (the frame's own session: the page's waits on its navigation)
    let picked = b.eval_in_isolated_frame(chat, PICK_FILE)?;
    std::thread::sleep(std::time::Duration::from_secs(6));
    b.latency(page, 0)?;
    b.eval(&witness, &cleared)?;
    b.close(witness)?;
    let stayed = b.eval(page, &format!("window.__release === 2 && {}", meta(&next)))? == true;
    b.eval(page, &visibility("visible"))?;
    let held = composer(b);
    s.ok(
        "a file picked while a reload is under way (its answer slow) stops it: the page stays as it was, the file kept",
        leaving && picked == 1 && stayed && held["files"] == 1,
        json!({ "leaving": leaving, "stayed": stayed, "composer": held }),
    );
    b.eval_in_frame(page, chat, REMOVE_FILE)?;
    // the file gone, words typed and the page hidden at once (nothing waited
    // for): it reloads, and the words are in their chat's composer after it
    let words = "a thought half typed before the deploy";
    let removed = b.eval_in_frame(page, chat, "document.querySelectorAll('#attachments .attachment-chip').length === 0")?;
    // (its page tells the shell it is busy no more: a message, a moment)
    std::thread::sleep(std::time::Duration::from_millis(300));
    b.eval_in_frame(page, chat, &fill("#text", words))?;
    b.eval(page, &visibility("hidden"))?;
    let reloaded = b.until(page, &format!("!window.__release && {} && {}", meta(&after), shown(chat)), wait);
    let back = s.eventually(wait, || b.eval_in_frame(page, chat, &format!("document.getElementById('text')?.value === {}", js(words))).ok() == Some(Value::Bool(true)));
    let kept = b.eval_in_frame(page, chat, DRAFT)?;
    let _ = b.screenshot(page, &shots.join("release-words-back.png"));
    s.ok(
        "the file gone, words typed and the page hidden the same moment: it reloads onto the new release, and the words are in their chat's composer again, kept by its own page as its person's",
        removed == true && reloaded && back && kept["who"] == me_id.as_str() && kept["text"] == words,
        json!({ "composer": composer(b), "kept": kept }),
    );
    // any reload: by hand, the words are there again
    let again = reload_shell(b, page)? && b.until(page, &shown(chat), wait) && s.eventually(wait, || b.eval_in_frame(page, chat, &format!("document.getElementById('text')?.value === {}", js(words))).ok() == Some(Value::Bool(true)));
    s.ok("and after a reload by hand too: a draft is its chat page's, whatever reloads it", again, composer(b));

    // no loop: the release it runs, said again, and the one it just left
    // (a deploy's old side, answering again), reload nothing
    b.eval(page, "(window.__release = 3, true)")?;
    b.set_cookie(&platform, RELEASE_COOKIE, &next)?;
    // the launcher's search: an answer the shell reads, naming the old release
    press(b, "#sidebar-new-chat")?;
    b.until(page, "document.getElementById('search-dialog').open", wait);
    b.eval(page, &fill("#workspace-search", "thought"))?;
    let read = b.until(page, "document.getElementById('search-results').children.length > 0 || document.getElementById('search-status').textContent !== ''", wait);
    b.press(page, "Escape", false)?;
    b.until(page, "!document.getElementById('search-dialog').open", wait);
    press(b, &row(other))?;
    let moved = b.until(page, &shown(other), wait);
    b.eval(page, &visibility("hidden"))?;
    std::thread::sleep(quiet);
    b.eval(page, &visibility("visible"))?;
    let still = b.eval(page, &format!("({{ mark: window.__release ?? null, waits: document.documentElement.dataset.releaseNext ?? null, release: {} }})", meta(&after)))?;
    s.ok(
        "no loop: the release it runs, said again, and the one its reload left, answering again, reload nothing, at its next move or hidden",
        read && moved && still == json!({ "mark": 3, "waits": null, "release": true }),
        &still,
    );

    // another account in this tab, with the first one's draft kept in the
    // chat (shared with them): it is none of theirs
    let other_email = format!("shell-ui-release-{run}@e2e.test");
    let other_session = api.sign_in(&other_email)?;
    let other_id = super::signin::who(api, &other_session)?["id"].as_str().unwrap_or("").to_string();
    let shared = shell(api, session, "PUT", &format!("/api/f/{chat}/members/{other_id}"), Some(&json!({ "role": "editor" })), &[])?;
    anyhow::ensure!(shared.status == 200, "the first chat shared with another account: {shared}");
    b.set_cookie(&platform, RELEASE_COOKIE, "")?;
    b.set_cookie(&platform, "fragment_session", &other_session)?;
    b.eval(page, "(history.replaceState(null, '', '/'), true)")?;
    let theirs = reload_shell(b, page)? && b.until(page, &shown(chat), wait) && ready(s, b);
    std::thread::sleep(quiet);
    let given = composer(b);
    let kept = b.eval_in_frame(page, chat, DRAFT)?;
    s.ok(
        "another account signed in to the tab opens the same chat (shared with them): its composer is empty, and the first account's draft is gone from the page's storage",
        theirs && given["text"] == "" && kept.is_null(),
        json!({ "composer": given, "kept": kept }),
    );
    // signing out, and Use another account, have each chat's page let its
    // draft go before the form is sent (its sending stood in for here)
    let mut forgot = vec![];
    for form in ["form[action='/auth/logout']", "form[action^='/auth/switch']"] {
        let typed = ready(s, b) && b.eval_in_frame(page, chat, &fill("#text", "their own words"))? == true;
        let saved = s.eventually(wait, || b.eval_in_frame(page, chat, DRAFT).ok().is_some_and(|d| d["who"] == other_id.as_str() && d["text"] == "their own words"));
        press(b, "#settings")?;
        b.until(page, &format!("!!document.querySelector({})", js(&format!("#settings-page {form}"))), wait);
        let sent = format!(
            "(() => {{ window.__sent = null; HTMLFormElement.prototype.submit = function () {{ window.__sent = this.getAttribute('action'); }}; document.querySelector({}).requestSubmit(); return true; }})()",
            js(&format!("#settings-page {form}"))
        );
        b.eval(page, &sent)?;
        let went = b.until(page, "!!window.__sent", wait);
        let gone = b.eval_in_frame(page, chat, DRAFT)?;
        forgot.push(json!({ "typed": typed, "saved": saved, "went": went, "draft": gone }));
        // a fresh page for the next form: the one asked keeps no draft more
        press(b, &row(chat))?;
        b.until(page, "location.pathname === '/'", wait);
        reload_shell(b, page)?;
        b.until(page, &shown(chat), wait);
    }
    s.ok(
        "signing out and Use another account have the chat's page let its person's draft go, and only then is the form sent",
        forgot.iter().all(|f| f["typed"] == true && f["saved"] == true && f["went"] == true && f["draft"].is_null()),
        json!(forgot),
    );
    // back to the first account, at home, the chat theirs alone again
    b.set_cookie(&platform, "fragment_session", session)?;
    anyhow::ensure!(reload_shell(b, page)?, "the first account's shell again, at home: {}", b.eval(page, "location.pathname")?);
    let unshared = shell(api, session, "DELETE", &format!("/api/f/{chat}/members/{other_id}"), None, &[])?;
    anyhow::ensure!(unshared.status == 200, "the other account out of the first chat again: {unshared}");
    Ok(())
}

/// What a guest is told of who is signed in (shell.js `noSeatSaid`), for `email`.
pub(super) fn no_seat_said(email: &str) -> String {
    format!("You're signed in as {email}. This account has no seat. If your team gave you a seat under another email, use another account.")
}

/// What the page shows of who is signed in to someone with no seat: each
/// line of it shown, its bold parts, and each "Use another account" shown
/// (its action, method and words).
pub(super) const NO_SEAT_SEEN: &str = "({ said: [...document.querySelectorAll('.no-seat')].filter((p) => p.getClientRects().length).map((p) => p.textContent), \
    bold: [...document.querySelectorAll('.no-seat strong')].filter((p) => p.getClientRects().length).map((b) => b.textContent), \
    switches: [...document.querySelectorAll('form[action^=\"/auth/switch\"]')].filter((f) => f.getClientRects().length).map((f) => [f.getAttribute('action'), f.getAttribute('method'), f.textContent]) })";

/// Settings' Account's line of who is signed in: its label, the email in
/// bold, its forms (action, method, words), and any no-seat line on the page.
const ACCOUNT_SEEN: &str = "(() => { const w = document.querySelector('#settings-page .settings-who'); \
    return { label: w?.querySelector('.settings-key')?.textContent ?? null, email: w?.querySelector('.settings-value strong')?.textContent ?? null, \
    actions: [...(w?.querySelectorAll('form') ?? [])].map((f) => [f.getAttribute('action'), f.getAttribute('method'), f.textContent]), \
    noSeat: [...document.querySelectorAll('.no-seat')].map((p) => p.textContent) }; })()";

/// `ACCOUNT_SEEN` says `email` is signed in, beside Sign out and Use another account.
fn account_shows(seen: &Value, email: &str) -> bool {
    seen["label"] == "Signed in as"
        && seen["email"] == email
        && seen["actions"] == json!([["/auth/logout", "post", "Sign out"], ["/auth/switch?return=%2Fsettings", "post", "Use another account"]])
}

/// Who is signed in, and another account, inside a phone's screen: the
/// page no wider than it, and no line of who is signed in, nor Use another
/// account (nor its words), past its edges.
const NO_SEAT_FITS: &str = "(() => { const d = document.documentElement; const p = document.getElementById('settings-page'); \
    const parts = [...document.querySelectorAll('.no-seat, .no-seat *, form[action^=\"/auth/switch\"], form[action^=\"/auth/switch\"] *, .settings-who, .settings-who *')].filter((e) => e.getClientRects().length); \
    const out = parts.filter((e) => { const r = e.getBoundingClientRect(); return r.left < -1 || r.right > innerWidth + 1 || e.scrollWidth > e.clientWidth + 1; }).map((e) => `${e.tagName} ${e.className} ${e.textContent.slice(0, 40)}`); \
    const page = p && !p.hidden ? [p.scrollWidth, p.clientWidth] : null; \
    return { ok: parts.length > 0 && out.length === 0 && d.scrollWidth <= innerWidth && (!page || page[0] <= page[1]), parts: parts.length, out, wide: d.scrollWidth, width: innerWidth, page }; })()";

/// Who is signed in, to a guest (2026-10-20, in production: a browser's
/// autofill signed someone in by email code as an address their seat was
/// not under, a new guest, and they thought sign-in broken). The welcome,
/// settings' Billing and its Account say which email is signed in, that it
/// has no seat, and offer Use another account (a form to `/auth/switch`),
/// at a desktop's width and on a phone (320 to 430 px: nothing past the
/// screen's edges). Pressed while WorkOS holds the guest's session in the
/// browser, it ends that session and this one, as the signin section's
/// switch does; Sign in then asks who is signing in, and `seat_email`
/// signs in to their seat, whose home and settings say nothing a guest is
/// told.
fn guest_ui(s: &mut Suite, api: &Api, b: &mut Browser, seat_email: &str, shots: &std::path::Path) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    // a guest: the e2e's people are seats, so the operator makes one a guest
    let guest = format!("shell-ui-guest-{}@e2e.test", crate::api::now_s());
    let session = api.sign_in(&guest)?;
    let id = super::signin::who(api, &session)?["id"].as_str().context("a signed-in guest")?.to_string();
    let op = api.sign_in("operator@e2e.test")?;
    let _ = api.approve(&op, &s.operator);
    let r = api.signed(&s.operator, "POST", &format!("/api/ledger/{id}/plan"), Some(&json!({ "id": guest, "plan": "guest" })))?;
    anyhow::ensure!(r.status == 200, "making a guest: {r}");
    b.set_cookie(&format!("{}/", api.base), "fragment_session", &session)?;
    let page = b.open(&format!("{}/", api.base))?;
    let said = no_seat_said(&guest);
    let welcome = "location.pathname === '/welcome' && !!document.querySelector('#first-run-card .no-seat')";
    let settings = "location.pathname === '/settings' && /You are a guest/.test(document.getElementById('settings-page').innerText) && !!document.querySelector('#settings-page .settings-who')";
    // on a phone, from 320 px to 430 px
    let mut bad: Vec<Value> = Vec::new();
    for w in [320u32, 375, 430] {
        b.viewport(&page, w, 812, true)?;
        for (place, path, ready) in [("welcome", "/welcome", welcome), ("settings", "/settings", settings)] {
            b.eval(&page, &format!("(location.assign({path:?}), true)"))?;
            let shown = b.until(&page, ready, wait);
            if place == "settings" {
                b.eval(&page, "(document.querySelector('#settings-page .settings-who')?.scrollIntoView({ block: 'center' }), true)")?;
            }
            let fits = b.eval(&page, NO_SEAT_FITS)?;
            if !shown || fits["ok"] != true {
                bad.push(json!({ "width": w, "place": place, "shown": shown, "seen": fits }));
            }
            if w == 320 {
                let _ = b.screenshot(&page, &shots.join(format!("phone-{w}-guest-{place}.png")));
            }
        }
    }
    s.ok(
        "at 320, 375 and 430 px on a phone, the welcome and settings keep who is signed in and Use another account inside the screen, the page no wider than it",
        bad.is_empty(),
        json!(bad),
    );
    // at a desktop's width: the welcome
    b.viewport(&page, 1280, 800, false)?;
    b.eval(&page, "(location.assign('/'), true)")?;
    let welcomed = b.until(&page, welcome, wait);
    let seen = b.eval(&page, NO_SEAT_SEEN)?;
    s.ok(
        "a guest's welcome says which email is signed in, in bold, that it has no seat, and offers Use another account: a form posting to /auth/switch, back to the welcome",
        welcomed && seen["said"] == json!([said]) && seen["bold"] == json!([guest]) && seen["switches"] == json!([["/auth/switch?return=%2Fwelcome", "post", "Use another account"]]),
        &seen,
    );
    let _ = b.screenshot(&page, &shots.join("guest-welcome.png"));
    // settings: Billing's guest state, then Account
    b.eval(&page, "(location.assign('/settings'), true)")?;
    let still = "document.getAnimations().every((a) => a.playState !== 'running' || a.effect?.getComputedTiming().iterations === Infinity)";
    let settled = b.until(&page, &format!("{settings} && {still}"), wait);
    let seen = b.eval(&page, NO_SEAT_SEEN)?;
    let account = b.eval(&page, ACCOUNT_SEEN)?;
    let get = b.eval(&page, "[...document.querySelectorAll('#settings-page a')].filter((a) => a.textContent === 'Get a seat').map((a) => a.getAttribute('href'))")?;
    let back = json!(["/auth/switch?return=%2Fsettings", "post", "Use another account"]);
    s.ok(
        "a guest's settings: Billing says which email is signed in, that it has no seat, beside Get a seat and Use another account; Account says who is signed in, in bold, beside Sign out and Use another account",
        settled && seen["said"] == json!([said]) && seen["switches"] == json!([back, back]) && get == json!(["/welcome"]) && account_shows(&account, &guest),
        json!({ "seen": seen, "account": account, "get": get }),
    );
    let _ = b.screenshot(&page, &shots.join("guest-settings.png"));
    // settings' Use another account, with WorkOS holding the guest's
    // session in the browser (as their autofilled sign-in left it)
    let pressed = super::signin::switched_out(
        s,
        b,
        &page,
        "#settings-page .settings-who form[action^='/auth/switch'] button",
        &guest,
        "settings' Use another account, pressed while WorkOS holds the guest's session, reaches WorkOS under the shell's CSP and ends it: the browser comes back signed out",
    )?;
    s.ok("and the guest's session here ended with it", super::signin::who(api, &session)?.is_null(), "");
    if !pressed {
        b.close(page)?;
        return Ok(());
    }
    // Sign in asks who is signing in now: the seat's email, to its seat.
    // Each press is a script's click on the element, and the fake's form
    // is submitted: a mouse's press, at a point measured on a page just
    // loaded, never reached Settings on CI's runners (2026-10-20: it never
    // opened, and made none of its calls). What is checked is what the
    // shell then shows.
    b.eval(&page, "(document.querySelector('#first-run-card a.splash-secondary').click(), true)")?;
    let asked = b.until(&page, "!!document.querySelector('input[name=login_hint]')", wait);
    b.eval(&page, &format!("(document.querySelector('input[name=login_hint]').value = {}, document.querySelector('input[name=login_hint]').form.submit(), true)", js(seat_email)))?;
    let home = b.until(&page, "location.pathname === '/' && !document.getElementById('layout').hidden && !!document.querySelector('#frames iframe')", wait);
    let told = b.eval(&page, "({ noSeat: document.querySelectorAll('.no-seat').length, notice: document.getElementById('notice').hidden ? null : document.getElementById('notice').innerText })")?;
    b.eval(&page, "(document.getElementById('settings').click(), true)")?;
    // settled once their email is rendered in Account (it waits on the
    // page's calls), not at a moment after the press
    let settled = b.until(
        &page,
        &format!("location.pathname === '/settings' && document.querySelector('#settings-page .settings-who .settings-value strong')?.textContent === {}", js(seat_email)),
        wait,
    );
    let account = b.eval(&page, ACCOUNT_SEEN)?;
    let text = b.eval(&page, "document.getElementById('settings-page').innerText")?;
    s.ok(
        "then Sign in asks who is signing in, and the seat's email lands home on their seat, told nothing a guest is; settings say that email is signed in",
        asked && home && settled && told == json!({ "noSeat": 0, "notice": null }) && account_shows(&account, seat_email) && account["noSeat"] == json!([]) && !text.as_str().unwrap_or("").contains("You are a guest"),
        json!({ "home": told, "account": account }),
    );
    let _ = b.screenshot(&page, &shots.join("guest-switched-to-seat.png"));
    b.close(page)?;
    Ok(())
}

/// An agent's name and picture in the shell (docs/api.md, "An agent's name
/// and picture"): Settings' Agents renames the person's agent and gives it
/// a picture from the row's own controls, and both reach the sidebar, its
/// settings row, the chat's page (its header, and its replies' byline and
/// face) and the share sheet, with no reload, while its first chat keeps
/// its own title. Renamed elsewhere (as `fragment agent` does), the shell
/// and its page follow, and nothing on its page says an old name (its job
/// names none). Rename asks for the chat's name in the chat's ⋯, the
/// agent's in its menu. Remove takes the picture away everywhere. On a
/// phone the name's field is 16px (no zoom on focus) and the row's
/// controls stay on screen.
fn faces_ui(s: &mut Suite, api: &Api, b: &mut Browser, page: &Page, me: &Person, chat: &str, shots: &std::path::Path) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    b.viewport(page, 1280, 900, false)?;
    let r = shell(api, me.session, "GET", "/api/computers", None, &[])?;
    let first = r.body["computers"][0]["agents"].as_array().into_iter().flatten().find(|a| a["fragment"].as_str().map(label_of) == Some(me.first)).cloned().unwrap_or_default();
    let (agent, id) = (first["fragment"].as_str().unwrap_or("").to_string(), first["identity"].as_str().unwrap_or("").to_string());
    anyhow::ensure!(!agent.is_empty() && !id.is_empty(), "the first agent: {r}");
    let face = js(&format!("[data-agent-face={}]", js(&agent)));
    let own_row = js(&format!("#chats [data-key={}] .label", js(&format!("chat:{chat}"))));
    // Rename, in Settings' Agents
    b.click(page, "#settings")?;
    let rename = js(&format!("[data-rename-agent={}]", js(&agent)));
    b.until(page, &format!("!!document.querySelector({rename})"), wait);
    b.eval(page, &format!("(document.querySelector({rename}).click(), true)"))?;
    let asked = b.until(page, "document.getElementById('rename-agent').open && document.getElementById('rename-title').textContent === 'Name your agent'", wait);
    b.eval(page, &fill("#agent-name", "Frank"))?;
    b.eval(page, "(document.getElementById('rename-form').requestSubmit(), true)")?;
    let screen = js(&format!("[data-screen-for={}]", js(&agent)));
    let renamed = b.until(
        page,
        &format!(
            "document.querySelector('#sidebar-agent .title')?.textContent === 'Frank' && document.querySelector({face})?.textContent === 'Frank' && document.querySelector({own_row})?.textContent === {first_chat} && document.querySelector({screen})?.textContent === \"Open Frank's screen\"",
            first_chat = js(FIRST_CHAT)
        ),
        wait,
    );
    s.ok(
        "Settings' Agents renames the person's agent: its name shows in the sidebar, its settings row and its screen's button, with no reload, and its first chat's row keeps the chat's own title",
        asked && renamed,
        b.eval(page, &format!("({{ pill: document.querySelector('#sidebar-agent .title')?.textContent, row: document.querySelector({face})?.textContent, chat: document.querySelector({own_row})?.textContent, screen: document.querySelector({screen})?.textContent }})"))?,
    );
    // a picture, from a file
    let orange = super::faces::orange();
    let file = shots.join("agent-picture.png");
    std::fs::write(&file, &orange)?;
    b.choose_files(page, &format!("[data-picture-for={}]", js(&agent)), &[&file])?;
    let path = super::faces::picture_path(&id, &orange);
    let loaded = |sel: &str| format!("(() => {{ const i = document.querySelector({}); return !!i && i.getAttribute('src') === {} && i.complete && i.naturalWidth === 16; }})()", js(sel), js(&path));
    let pictured = b.until(page, &format!("{} && {}", loaded("#sidebar-agent img"), loaded(&format!("[data-agent-face={}] img", js(&agent)))), wait);
    let offers = b.eval(page, &format!("({{ label: document.querySelector('[data-picture-label={a}]')?.textContent, remove: !document.querySelector('[data-picture-remove={a}]')?.hidden }})", a = js(&agent)))?;
    s.ok(
        "and gives it a picture from a file: the sidebar and its row show it in its image's place, loaded, and the row offers Change and Remove",
        pictured && offers["label"] == "Change picture" && offers["remove"] == true,
        json!({ "offers": offers, "row": own_row_of(api, me.session, &agent) }),
    );
    b.eval(page, &format!("(document.querySelector({face})?.scrollIntoView({{ block: 'center' }}), true)"))?;
    let _ = b.screenshot(page, &shots.join("settings-agent-face.png"));
    // the chat: its header in the shell (the chat's own title), and its page
    open_row(b, page, chat, wait)?;
    let headed = b.until(page, &format!("document.getElementById('chat-title').textContent === {}", js(FIRST_CHAT)), wait);
    let in_page = format!(
        "(() => {{ const p = {}; const m = document.querySelector('#head-marks img'); const by = [...document.querySelectorAll('.msg.agent .who')].pop(); \
         return document.getElementById('head-label')?.textContent === 'Frank' && !!m && m.src.endsWith(p) && m.naturalWidth === 16 && !!by && by.textContent.includes('Frank') && !!by.querySelector('img') && by.querySelector('img').src.endsWith(p); }})()",
        js(&path)
    );
    let framed = s.eventually(wait, || b.eval_in_frame(page, chat, &in_page).ok() == Some(Value::Bool(true)));
    s.ok(
        "its first chat's header in the shell is still the chat's own title, and the chat's page names the agent anew and shows its picture in its header and its replies' byline",
        headed && framed,
        b.eval_in_frame(page, chat, "({ head: document.getElementById('head-label')?.textContent, marks: document.getElementById('head-marks')?.innerHTML, by: [...document.querySelectorAll('.msg.agent .who')].pop()?.innerHTML })")?,
    );
    let _ = b.screenshot(page, &shots.join("chat-agent-face.png"));
    let sheet = super::signin::with_session(api, "GET", &format!("/share/{chat}"), me.session)?;
    s.ok("its chat's share sheet names it so, with its picture", sheet.status == 200 && sheet.text.contains("Frank") && sheet.text.contains(&format!("src=\"{path}\"")), &sheet);
    // its screen (its menu's Its screen), by its name and never its label
    // (Paul, 2026-10-20: "Watching pulsar-360's screen" of his Waffle): the
    // window's title, and the screen's page, which the shell tells (the
    // stub's page titles itself by it, as our Hermes image's says it)
    let screen_at = format!("/p/6080/?agent={agent}");
    let screen_pane = js(&format!("#stack .pane[data-key={}] .pane-title", js(&format!("screen:{agent}"))));
    let screen_named = |s: &Suite, b: &mut Browser, name: &str| {
        let pane = format!("document.querySelector({screen_pane})?.textContent === {}", js(name));
        b.until(page, &pane, wait) && s.eventually(wait, || b.eval_in_frame(page, &screen_at, "document.title").ok() == Some(json!(format!("{name}'s screen"))))
    };
    let screen_said = |b: &mut Browser| json!({ "window": b.eval(page, &format!("document.querySelector({screen_pane})?.textContent ?? null")).ok(), "page": b.eval_in_frame(page, &screen_at, "document.title").ok() });
    b.click(page, "#sidebar-agent")?;
    let opened = menu_item(b, page, "Its screen")?;
    let named = opened && screen_named(s, b, "Frank");
    s.ok("its screen's window and page name it by its name, never its label", named, json!({ "label": me.first, "shown": screen_said(b) }));
    // renamed elsewhere, as `fragment agent --name` does (its files, then a
    // deploy): the sidebar and the chat's page follow; the chat's own title
    // is the chat's, and stays
    let mut manifest = shell(api, me.session, "GET", &format!("/api/f/{agent}/manifest"), None, &[])?.body;
    manifest["meta"]["title"] = json!("Frankie");
    let files = json!({ "message": "rename", "files": [{ "path": "fragment.json", "text": format!("{}\n", serde_json::to_string_pretty(&manifest)?) }] });
    let wrote = shell(api, me.session, "POST", &format!("/api/f/{agent}/files"), Some(&files), &[])?;
    let deployed = shell(api, me.session, "POST", &format!("/api/f/{agent}/deploy"), Some(&json!({})), &[])?;
    let followed = b.until(page, "document.querySelector('#sidebar-agent .title')?.textContent === 'Frankie'", wait);
    let in_frame = s.eventually(wait, || b.eval_in_frame(page, chat, "document.getElementById('head-label')?.textContent === 'Frankie'").ok() == Some(Value::Bool(true)));
    let kept = b.eval(page, &format!("({{ row: document.querySelector({own_row})?.textContent ?? null, header: document.getElementById('chat-title').textContent }})"))?;
    let listed = own_row_of(api, me.session, chat);
    s.ok(
        "renamed elsewhere (its files and a deploy, as the CLI does), the sidebar and the chat's page follow with no reload, and its first chat keeps its own title: its row, its header and its fragment's",
        wrote.status == 200 && deployed.status == 200 && followed && in_frame && kept["row"] == FIRST_CHAT && kept["header"] == FIRST_CHAT && listed["title"] == FIRST_CHAT,
        json!({ "shown": kept, "chat": listed }),
    );
    let renamed = screen_named(s, b, "Frankie");
    let _ = b.screenshot(page, &shots.join("screen-renamed.png"));
    s.ok("its screen, open, follows the rename: its window and its page name it anew, with no reload", renamed, screen_said(b));
    // closed (its address names the agent too, as its page's frame does)
    b.eval(page, &format!("(document.querySelector({screen_pane})?.closest('.pane')?.querySelector('button[aria-label=Close]')?.click(), true)"))?;
    b.until(page, &format!("!document.querySelector({screen_pane})"), wait);
    // its page (its menu's Its profile): its new name, and no old one
    // anywhere on it (its job names none); the model it runs, its tier's
    // (agent.json's `cheap`), by its id as their computer's view names it
    b.click(page, "#sidebar-agent")?;
    let profile = menu_item(b, page, "Its profile")?;
    let mut seen = Value::Null;
    // its own origin's frame (a screen's names it in its address too)
    let its_page = api.site_origin(&agent);
    let cheap = r.body["computers"][0]["models"]["cheap"].as_str().unwrap_or("").to_string();
    let named = profile
        && s.eventually(wait, || {
            seen = b.eval_in_frame(page, &its_page, "({ name: document.getElementById('name')?.textContent ?? null, title: document.title, model: document.getElementById('tier')?.textContent ?? null, soul: document.getElementById('soul')?.textContent ?? '', text: document.body.innerText })").unwrap_or(Value::Null);
            seen["name"] == "Frankie" && seen["soul"].as_str().is_some_and(|t| t.contains("default agent")) && seen["model"] == cheap.as_str()
        });
    let text = seen["text"].as_str().unwrap_or("");
    let _ = b.screenshot(page, &shots.join("agent-page-renamed.png"));
    s.ok(
        "its page shows its new name, and none of the names it had (its job says what it does, never who it is), and the model it runs by its id (its tier's, as their computer's view names it), never a tier's name",
        named && seen["title"] == "Frankie" && !text.contains(me.first_title) && !seen["soul"].as_str().unwrap_or("").contains("Frank") && !cheap.is_empty() && cheap != "cheap" && !text.contains("cheap model"),
        json!({ "was": me.first_title, "seen": seen, "cheap": cheap }),
    );
    // Rename asks for what it renames: the chat's name in the chat's ⋯, the
    // agent's in its menu, each alone
    let asked_for = |b: &mut Browser, open: &str| -> Result<Value> {
        b.click(page, open)?;
        menu_item(b, page, "Rename…")?;
        b.until(page, "document.getElementById('rename-agent').open", wait);
        let said = b.eval(page, "({ title: document.getElementById('rename-title').textContent, value: document.getElementById('agent-name').value })")?;
        b.eval(page, "(document.getElementById('rename-cancel').click(), true)")?;
        b.until(page, "!document.getElementById('rename-agent').open", wait);
        Ok(said)
    };
    let chats_own = asked_for(b, "#chat-options")?;
    let agents_own = asked_for(b, "#sidebar-agent")?;
    s.ok(
        "Rename… in the first chat's ⋯ names the chat (\"Name this chat\", its own title), and in the agent's menu the agent (\"Name your agent\", its name)",
        chats_own == json!({ "title": "Name this chat", "value": FIRST_CHAT }) && agents_own == json!({ "title": "Name your agent", "value": "Frankie" }),
        json!({ "chat": chats_own, "agent": agents_own }),
    );
    // Remove: gone everywhere
    b.click(page, "#settings")?;
    let remove = js(&format!("[data-picture-remove={}]", js(&agent)));
    b.until(page, &format!("!!document.querySelector({remove}) && !document.querySelector({remove}).hidden"), wait);
    b.eval(page, &format!("(document.querySelector({remove}).click(), true)"))?;
    let gone = b.until(page, &format!("!document.querySelector('#sidebar-agent img') && !document.querySelector('[data-agent-face={a}] img') && document.querySelector('[data-picture-label={a}]')?.textContent === 'Add a picture'", a = js(&agent)), wait);
    let none = super::faces::fetched(api, &format!("/api/identities/{id}/picture"))?;
    s.ok("Remove takes its picture away: no picture in the sidebar or its row, which offers Add again, and none at its address", gone && none.status == 404, none.status);
    // its colour, by its identity (the agent template's agent-color.js, which
    // the shell and the chat link to), is one colour on its page, in the
    // sidebar and on its chat's page; agent.json names none
    let tint = "(e) => e?.style.getPropertyValue('--agent-color') ?? null";
    let want = b.eval(page, &format!("import('/__shell/agent-color.js').then((m) => m.colorOf({}))", js(&id)))?;
    let sidebar = b.eval(page, &format!("({tint})(document.querySelector('#sidebar-agent .agent-avatar'))"))?;
    let in_chat = s.eventually(wait, || b.eval_in_frame(page, chat, &format!("({tint})(document.querySelector('#head-marks .agent-avatar'))")).ok() == Some(want.clone()));
    b.click(page, "#sidebar-agent")?;
    let profile = menu_item(b, page, "Its profile")?;
    let on_page = profile && s.eventually(wait, || b.eval_in_frame(page, &agent, &format!("({tint})(document.getElementById('avatar'))")).ok() == Some(want.clone()));
    let settings = shell(api, me.session, "GET", &format!("/api/f/{agent}/file?path=agent.json"), None, &[])?;
    s.ok(
        "its colour is its identity's, one on its own page, in the sidebar and on its chat's page; agent.json names none",
        want.as_str().is_some_and(|c| c.starts_with('#')) && sidebar == want && in_chat && on_page && settings.status == 200 && settings.body.get("color").is_none(),
        json!({ "identity's": want, "sidebar": sidebar, "agentPage": b.eval_in_frame(page, &agent, &format!("({tint})(document.getElementById('avatar'))")).ok(), "agentJson": settings.body }),
    );
    // back to Settings' Agents, which its profile's window left
    b.click(page, "#settings")?;
    b.until(page, &format!("location.pathname === '/settings' && !!document.querySelector({rename})"), wait);
    // on a phone: its name's field is 16px, and its row's controls stay on screen
    b.viewport(page, 390, 844, true)?;
    b.reload(page)?;
    b.until(page, &format!("!!document.querySelector({rename})"), wait);
    let fits = b.eval(page, &format!("(() => {{ const r = document.querySelector({rename}); r.scrollIntoView({{ block: 'center' }}); return [...r.parentElement.querySelectorAll('button, label')].every((e) => e.getBoundingClientRect().right <= innerWidth + 1); }})()"))?;
    b.eval(page, &format!("(document.querySelector({rename}).click(), true)"))?;
    b.until(page, "document.getElementById('rename-agent').open", wait);
    let size = b.eval(page, "parseFloat(getComputedStyle(document.getElementById('agent-name')).fontSize)")?;
    let _ = b.screenshot(page, &shots.join("phone-rename-agent.png"));
    b.eval(page, "(document.getElementById('rename-cancel').click(), true)")?;
    s.ok("on a phone the agent's controls stay on screen, and its name's field is 16px (no zoom as it takes focus)", fits == true && size.as_f64().is_some_and(|px| px >= 16.0), json!({ "fits": fits, "px": size }));
    b.viewport(page, 1280, 900, false)?;
    Ok(())
}

/// `name`'s row in the person's list, as the shell reads it.
fn own_row_of(api: &Api, session: &str, name: &str) -> Value {
    let r = shell(api, session, "GET", "/api/fragments", None, &[]).ok();
    r.and_then(|r| r.body["fragments"].as_array().and_then(|l| l.iter().find(|f| f["name"] == name).cloned())).unwrap_or(Value::Null)
}

/// Whether a page stays in its viewport: nothing wider than it, and the
/// page itself does not scroll (asked to, it stays put), with its own
/// overscroll held (no bounce).
const PAGE_FITS: &str = "(() => { const d = document.documentElement; const wide = d.scrollWidth; scrollTo(0, 400); document.body.scrollTop = 400; \
    const moved = scrollY + d.scrollTop + document.body.scrollTop; scrollTo(0, 0); document.body.scrollTop = 0; \
    return { ok: wide <= innerWidth && moved === 0 && getComputedStyle(d).overscrollBehaviorY === 'none', wide, width: innerWidth, moved }; })()";

/// Settings' boxes that must not move as they load (left, top, width,
/// rounded): its layout's, its content's and its heading's, with the
/// layout's padding, and whether it is still loading.
const SETTINGS_BOX: &str = "(() => { const p = document.getElementById('settings-page'); const r = (e) => { const b = e?.getBoundingClientRect(); return b ? [b.left, b.top, b.width].map(Math.round) : null; }; \
    const l = p.querySelector('.settings-layout'); const c = p.querySelector('.settings-content'); \
    return { layout: r(l), padding: l ? getComputedStyle(l).padding : null, content: r(c), heading: r(c?.querySelector('h1')), loading: !!p.querySelector('.settings-loading'), sections: p.querySelectorAll('.settings-section').length }; })()";

/// The shell's next read of the ledger held until `window.__ledgerHeld()`:
/// settings, which read it first, stay loading.
const HOLD_LEDGER: &str = "(() => { const real = window.fetch; window.__ledgerHeld = null; window.fetch = async (url, o) => { \
    if (String(url) === '/api/ledger') { window.fetch = real; await new Promise((r) => { window.__ledgerHeld = r; }); } return real(url, o); }; return true; })()";

/// Opens settings by `open` (a script's expression) with their read of
/// the ledger held (`HOLD_LEDGER`), so they stay loading: their boxes then
/// (`SETTINGS_BOX`), and once the read is let go and they load. `ok` when
/// they were seen loading and then loaded, and nothing moved.
fn settings_loading(b: &mut Browser, page: &Page, open: &str, wait: std::time::Duration) -> Result<Value> {
    b.eval(page, HOLD_LEDGER)?;
    b.eval(page, &format!("({open}, true)"))?;
    let held = b.until(page, "location.pathname === '/settings' && !!window.__ledgerHeld && !!document.querySelector('#settings-page .settings-loading')", wait);
    let loading = b.eval(page, SETTINGS_BOX)?;
    b.eval(page, "(window.__ledgerHeld?.(), true)")?;
    let loaded = b.until(page, "!document.querySelector('#settings-page .settings-loading') && document.querySelectorAll('#settings-page .settings-section').length > 0", wait);
    let after = b.eval(page, SETTINGS_BOX)?;
    let same = ["layout", "padding", "content", "heading"].iter().all(|k| !loading[k].is_null() && loading[k] == after[k]);
    Ok(json!({ "ok": held && loaded && loading["loading"] == true && after["loading"] == false && same, "loading": loading, "loaded": after }))
}

/// Opens `chat` from its row in the sidebar, as a person picks it (a
/// reload, or /, opens the newest chat: maybe another).
fn open_row(b: &mut Browser, page: &Page, chat: &str, wait: std::time::Duration) -> Result<()> {
    let row = format!("document.querySelector({})", js(&format!("#chats [data-key={}]", js(&format!("chat:{chat}")))));
    let open = format!("location.pathname === '/' && [...document.querySelectorAll('#frames iframe')].find((f) => !f.hidden)?.dataset.fragment === {}", js(chat));
    if b.eval(page, &format!("!!({open})"))? != true {
        b.eval(page, &format!("({row}?.click(), true)"))?;
    }
    anyhow::ensure!(b.until(page, &open, wait), "{chat} opened from its row");
    Ok(())
}

/// Whether settings' Your data stays in a phone's width: none of its rows
/// past the screen's edge, nor the page or the section wider than it shows.
const YOUR_DATA_FITS: &str = "(() => { const d = document.getElementById('settings-data'); const p = document.getElementById('settings-page'); if (!d || !p) return { ok: false, missing: true }; \
    d.scrollIntoView({ block: 'center' }); const out = [...d.querySelectorAll('*')].filter((e) => e.getBoundingClientRect().right > innerWidth + 1).map((e) => e.dataset.action || e.className || e.tagName); \
    return { ok: out.length === 0 && p.scrollWidth <= p.clientWidth && d.scrollWidth <= d.clientWidth, out, page: [p.scrollWidth, p.clientWidth], section: [d.scrollWidth, d.clientWidth] }; })()";

/// The fields a phone would zoom into on focus (text under 16px): none.
const SMALL_FIELDS: &str = "[...document.querySelectorAll('input:not([type=checkbox]):not([type=radio]):not([type=hidden]):not([type=file]):not([type=range]), textarea, select')] \
    .map((e) => [e.id || e.name || e.getAttribute('aria-label') || e.placeholder || e.tagName, parseFloat(getComputedStyle(e).fontSize)]).filter(([, px]) => px < 16)";

/// A message the page must hold without widening: a word that never
/// breaks, a long line of code, and a wide table.
const WIDE_MESSAGE: &str = "A word that never breaks: supercalifragilisticexpialidociousandthensomemoreletterswithoutanyspaceatallforthephone\n\n```js\nconst line = 'a very long line of code that must scroll inside its own block and never widen the page at all';\n```\n\n| column one | column two | column three | column four | column five | column six |\n|---|---|---|---|---|---|\n| a wide table cell | another wide cell | and another one | four four four | five five five | six six six |";

/// The shell on a phone, as an app (Paul, 2026-10-13: "staying strictly in
/// the viewport (shouldn't feel overflow scrolly, shouldn't zoom on text
/// input, should feel like a native app)"), at 320, 375, 390 and 430 px in
/// Chrome's phone emulation: the page never wider than the screen nor
/// scrolling, with its sidebar open and closed, a chat holding a long
/// word, code and a wide table (the table scrolls in its own box), the
/// launcher, settings (its Your data with an export ready, its widest
/// rows), the share sheet, an app's window and the ways to ask for one
/// (three, then three more from its die, a target of 44 px or more that a
/// press reaches); no field a phone zooms into on focus (16px or more), in
/// the shell, the chat, the launcher, settings and the sheet; and, at 375,
/// a way to ask the die dealt, pressed, goes into the chat's composer, the
/// desktop's sheet closed.
/// Safari's keyboard, bounce and zoom are not Chrome's: a phone tests those.
fn phone_ui(s: &mut Suite, api: &Api, b: &mut Browser, page: &Page, session: &str, shots: &std::path::Path) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let agent_wait = std::time::Duration::from_secs(120);
    b.color_scheme(page, "light")?;
    let home = "!document.getElementById('layout').hidden && !!document.querySelector('#frames iframe:not([hidden])')";
    b.viewport(page, 375, 812, true)?;
    b.reload(page)?;
    b.until(page, home, wait);
    // a chat of theirs (theirs to invite to), their newest
    let listed = shell(api, session, "GET", "/api/fragments", None, &[])?;
    let chat = listed.body["fragments"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["kind"] == "chat" && f["role"] == "owner" && f["archived"] != true)
        .max_by_key(|f| f["previewAt"].as_i64().unwrap_or(0))
        .and_then(|f| f["name"].as_str())
        .unwrap_or("")
        .to_string();
    anyhow::ensure!(!chat.is_empty(), "a chat of theirs: {listed}");
    open_row(b, page, &chat, wait)?;
    // the wide message, and its echo, in that chat
    let r = shell(api, session, "POST", &format!("/api/f/{chat}/channels/chat"), Some(&json!({ "id": "phone-wide-1", "body": { "text": WIDE_MESSAGE } })), &[])?;
    anyhow::ensure!(r.status == 200, "the wide message: {r}");
    let echoed = s.eventually(agent_wait, || b.eval_in_frame(page, &chat, "!!document.querySelector('.msg.agent .md-table table')").ok() == Some(Value::Bool(true)));
    // an export of theirs, built while the first width's chat is looked at
    let r = shell(api, session, "POST", "/api/export", None, &[])?;
    anyhow::ensure!(r.status == 200, "the export: {r}");
    let mut small: Vec<Value> = Vec::new();
    let mut phone_prompt: Option<(bool, Value)> = None;
    let mut phone_loading: Option<Value> = None;
    for w in [320u32, 375, 390, 430] {
        b.viewport(page, w, 812, true)?;
        b.reload(page)?;
        b.until(page, home, wait);
        open_row(b, page, &chat, wait)?;
        let mut places: Vec<(&str, Value)> = Vec::new();
        // the chat open, the sidebar closed; its page in its frame, the table in its own box
        places.push(("chat", b.eval(page, PAGE_FITS)?));
        let _ = s.eventually(wait, || b.eval_in_frame(page, &chat, "!!document.querySelector('.msg.agent .md-table table')").ok() == Some(Value::Bool(true)));
        b.eval_in_frame(page, &chat, "(() => { const s = document.getElementById('scroll'); s.scrollTop = s.scrollHeight; return true; })()")?;
        let framed = b.eval_in_frame(
            page,
            &chat,
            &format!("(() => {{ const p = {PAGE_FITS}; const t = [...document.querySelectorAll('.msg.agent .md-table')].pop(); const c = [...document.querySelectorAll('.msg.agent pre')].pop(); \
              return {{ ...p, ok: p.ok && !!t && t.getBoundingClientRect().right <= innerWidth + 1 && (!c || c.getBoundingClientRect().right <= innerWidth + 1), table: t ? [t.scrollWidth, t.clientWidth] : null }}; }})()"),
        )?;
        places.push(("its chat's page, a long word, code and a wide table", framed));
        if let Ok(Value::Array(l)) = b.eval_in_frame(page, &chat, SMALL_FIELDS) { small.extend(l.into_iter().map(|f| json!({ "width": w, "in": "chat", "field": f }))); }
        if w == 320 || w == 430 {
            let _ = b.screenshot(page, &shots.join(format!("phone-{w}-chat.png")));
        }
        // the list, over the chat
        b.eval(page, "(document.getElementById('toggle-left').click(), true)")?;
        b.until(page, "document.getElementById('layout').classList.contains('left-open') && document.getAnimations().every((a) => a.playState !== 'running' || a.effect?.getComputedTiming().iterations === Infinity)", wait);
        places.push(("the list", b.eval(page, PAGE_FITS)?));
        // the launcher, words typed
        b.eval(page, "(document.getElementById('sidebar-new-chat').click(), true)")?;
        b.until(page, "document.getElementById('search-dialog').open", wait);
        b.eval(page, &fill("#workspace-search", "garden"))?;
        b.until(page, "!!document.querySelector('#search-results .search-result')", wait);
        places.push(("the launcher", b.eval(page, PAGE_FITS)?));
        if let Value::Array(l) = b.eval(page, SMALL_FIELDS)? { small.extend(l.into_iter().map(|f| json!({ "width": w, "in": "launcher", "field": f }))); }
        b.eval(page, "(document.getElementById('search-dialog').dispatchEvent(new Event('cancel')), true)")?;
        b.until(page, "!document.getElementById('search-dialog').open", wait);
        // settings (at 375 px, held loading a moment: where the loaded page's content goes)
        let open = "document.getElementById('scrim').click(), document.getElementById('settings').click()";
        if w == 375 {
            phone_loading = Some(settings_loading(b, page, open, wait)?);
        } else {
            b.eval(page, &format!("({open}, true)"))?;
        }
        b.until(page, "location.pathname === '/settings' && !!document.querySelector('#settings-access')", wait);
        places.push(("settings", b.eval(page, PAGE_FITS)?));
        if let Value::Array(l) = b.eval(page, SMALL_FIELDS)? { small.extend(l.into_iter().map(|f| json!({ "width": w, "in": "settings", "field": f }))); }
        if w == 320 {
            let _ = b.screenshot(page, &shots.join(format!("phone-{w}-settings.png")));
        }
        // its Your data, the export ready: its download, size, until when, and again
        let ready = b.until(page, "document.querySelector('#settings-data .settings-export')?.dataset.state === 'ready' && !!document.querySelector('#settings-data [data-action=export-download]')", agent_wait);
        let data = b.eval(page, YOUR_DATA_FITS)?;
        places.push(("settings' Your data, an export ready", json!({ "ok": ready && data["ok"] == true, "seen": data })));
        if w == 320 {
            let _ = b.screenshot(page, &shots.join(format!("phone-{w}-your-data.png")));
        }
        // the share sheet, over the chat
        // (the chat of theirs, from its row: at / the newest chat opens, maybe one shared with them)
        open_row(b, page, &chat, wait)?;
        let invite = "(() => { const i = [...document.querySelectorAll('#menu button')].find((b) => b.textContent === 'Invite…'); if (i) { i.click(); return true; } \
            if (!document.querySelector('#menu button')) document.getElementById('chat-options')?.click(); return false; })()";
        let sheet = b.until(page, invite, wait) && b.until(page, "document.getElementById('sheet').open", wait) && s.eventually(wait, || b.eval_in_frame(page, "/share/", "!!document.querySelector('input[type=email]')").ok() == Some(Value::Bool(true)));
        places.push(("the share sheet", b.eval(page, PAGE_FITS)?));
        let inside = b.eval_in_frame(page, "/share/", "({ ok: document.documentElement.scrollWidth <= innerWidth, wide: document.documentElement.scrollWidth, width: innerWidth })").unwrap_or(Value::Null);
        places.push(("the share sheet's page", json!({ "ok": sheet && inside["ok"] == true, "inside": inside })));
        if let Ok(Value::Array(l)) = b.eval_in_frame(page, "/share/", SMALL_FIELDS) { small.extend(l.into_iter().map(|f| json!({ "width": w, "in": "share sheet", "field": f }))); }
        b.eval(page, "(document.getElementById('sheet').close(), true)")?;
        // an app's window: the desktop's sheet
        b.eval(page, "(document.getElementById('toggle-right').click(), true)")?;
        b.until(page, "document.getElementById('layout').classList.contains('right-open') && !!document.querySelector('#apps [data-key^=\"app:\"]')", wait);
        b.eval(page, "(document.querySelector('#apps [data-key^=\"app:\"]')?.click(), true)")?;
        b.until(page, "!!document.querySelector('#stack .pane iframe')", wait);
        places.push(("an app's window", b.eval(page, PAGE_FITS)?));
        // ways to ask for an app: the dock's Add, in the desktop's sheet, three
        let prompts = "#stack .pane[data-key=prompts] .app-prompt";
        let die = "#stack .pane[data-key=prompts] .prompt-dice";
        let settled = "document.getAnimations().every((a) => a.playState !== 'running' || a.effect?.getComputedTiming().iterations === Infinity)";
        b.eval(page, "(document.getElementById('add-app')?.click(), true)")?;
        b.until(page, &format!("document.querySelectorAll('{prompts}').length === 3"), wait);
        places.push(("ways to ask for an app", b.eval(page, PAGE_FITS)?));
        // and three more from its die: a thumb's target in the sheet, the
        // one a press there reaches once its window scrolls to it, every
        // prompt inside the screen's width
        b.until(page, settled, wait);
        let first = b.eval(page, &format!("[...document.querySelectorAll('{prompts}')].map((b) => b.textContent)"))?;
        b.eval(page, &format!("(document.querySelector('{die}')?.click(), true)"))?;
        let dealt = b.until(page, &format!("(() => {{ const h = [...document.querySelectorAll('{prompts}')].map((b) => b.textContent); return h.length === 3 && h.every((p) => !{first}.includes(p)); }})()"), wait);
        b.until(page, settled, wait);
        let target = b.eval(
            page,
            &format!("(() => {{ const d = document.querySelector('{die}'); d?.scrollIntoView({{ block: 'nearest' }}); const r = d?.getBoundingClientRect(); const hit = !!r && document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2) === d; \
              const inside = [...document.querySelectorAll('{prompts}')].every((b) => b.getBoundingClientRect().right <= innerWidth + 1); \
              return {{ ok: !!r && r.width >= 44 && r.height >= 44 && r.left >= 0 && r.right <= innerWidth && hit && inside, die: r && [r.left, r.top, r.width, r.height], hit, inside, width: innerWidth, height: innerHeight }}; }})()"),
        )?;
        let fits = b.eval(page, PAGE_FITS)?;
        places.push(("ways to ask, dealt by its die", json!({ "ok": dealt && target["ok"] == true && fits["ok"] == true, "first": first, "target": target, "page": fits })));
        if w == 375 {
            let _ = b.screenshot(page, &shots.join(format!("phone-{w}-prompts.png")));
            let text = b.eval(page, &format!("document.querySelector('{prompts}')?.textContent ?? ''"))?.as_str().unwrap_or("").to_string();
            let before = chat_count(api, session, &chat);
            b.eval(page, &format!("(document.querySelector('{prompts}')?.click(), true)"))?;
            let filled = s.eventually(wait, || b.eval_in_frame(page, &chat, COMPOSER).ok().is_some_and(|v| v["value"] == text.as_str() && v["focused"] == true));
            let uncovered = b.until(page, "!document.getElementById('layout').classList.contains('right-open') && !document.getElementById('layout').classList.contains('left-open') && document.getAnimations().every((a) => a.playState !== 'running' || a.effect?.getComputedTiming().iterations === Infinity)", wait);
            let seen = b.eval_in_frame(page, &chat, "(() => { const r = document.getElementById('text').getBoundingClientRect(); return { top: r.top, bottom: r.bottom, innerHeight }; })()")?;
            let shows = seen["top"].as_f64().is_some_and(|t| t >= 0.0) && seen["bottom"].as_f64().zip(seen["innerHeight"].as_f64()).is_some_and(|(b, h)| b <= h);
            std::thread::sleep(std::time::Duration::from_secs(2));
            let after = chat_count(api, session, &chat);
            let _ = b.screenshot(page, &shots.join(format!("phone-{w}-prompt-composed.png")));
            phone_prompt = Some((
                !text.is_empty() && filled && uncovered && shows && after == before,
                json!({ "composer": b.eval_in_frame(page, &chat, COMPOSER).unwrap_or(Value::Null), "box": seen, "records": [before, after], "uncovered": uncovered }),
            ));
            b.eval_in_frame(page, &chat, "(() => { const t = document.getElementById('text'); t.value = ''; t.dispatchEvent(new Event('input', { bubbles: true })); t.blur(); return true; })()")?;
        }
        let bad: Vec<Value> = places.iter().filter(|(_, v)| v["ok"] != true).map(|(p, v)| json!({ "place": p, "seen": v })).collect();
        s.ok(
            &format!("at {w} px on a phone, the page stays in its viewport, never scrolling or wider: the chat (its sidebar closed and open), a long word, code and a wide table, the launcher, settings and its Your data, the share sheet, an app's window, and the ways to ask for one, three and three more from its die (a 44 px target)"),
            echoed && bad.is_empty(),
            json!({ "bad": bad, "places": places.iter().map(|(p, _)| p).collect::<Vec<_>>() }),
        );
    }
    s.ok("on a phone no field is under 16px (Safari zooms in on one as it takes focus): the chat's composer, the launcher, settings and the share sheet", small.is_empty(), json!(small));
    let (composed, seen) = phone_prompt.unwrap_or((false, Value::Null));
    s.ok("on a phone, a way to ask from the dock's Add goes into the chat's composer: the desktop's sheet closes, the composer shows, focused, and nothing is sent", composed, seen);
    let loading = phone_loading.unwrap_or(Value::Null);
    s.ok(
        "settings load in the loaded page's own box: its layout, padding, content and heading where they stay once loaded (no jump), on a phone (375 px)",
        loading["ok"] == true,
        &loading,
    );
    Ok(())
}

/// What the person is told of their computer, and the way back to working
/// (docs/computers.md, "What its owner is told"), as they see it on
/// settings: a sleep whose save fails (the lever's) shows at once, with no
/// reload (the computer tells the page's list socket); its Restart restarts
/// it once, the restart's own save failing too, and the notice then says
/// what it went back to; OK tells it no more, here or after a reload; and
/// settings' Restart computer asks once more, then restarts it, saved.
fn computer_ui(s: &mut Suite, api: &Api, b: &mut Browser, page: &Page, session: &str, shots: &std::path::Path) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let computers = shell(api, session, "GET", "/api/computers", None, &[])?;
    let id = computers.body["computers"][0]["computer"].as_str().unwrap_or("").to_string();
    let view = || shell(api, session, "GET", &format!("/api/computers/{id}"), None, &[]).map(|r| r.body).unwrap_or(Value::Null);
    let shown = |kind: &str| format!("document.querySelector('#computer-notices:not([hidden]) .computer-notice[data-kind={kind}]')");
    // its saves fail; its owner's sleep keeps it awake, and tells them
    std::thread::sleep(super::computers::QUEUE_DRAIN);
    let failing = super::computers::lever_with(api, &id, json!({ "op": "fail-saves", "times": 50 }))?;
    let r = shell(api, session, "POST", &format!("/api/computers/{id}/sleep"), Some(&json!({})), &[])?;
    let generation = r.body["generation"].as_u64().unwrap_or(0);
    let warned = b.until(page, &format!("{n}?.innerText.includes(\"can't save\") && !!{n}.querySelector('button[data-action=restart]')", n = shown("unsaved")), wait);
    let said = b.eval(page, "document.getElementById('computer-notices').innerText")?;
    let _ = b.screenshot(page, &shots.join("computer-unsaved.png"));
    s.ok(
        "a sleep whose save fails is shown at once, with no reload: since when its work is unsaved, when it stops, and Restart",
        failing.status == 200 && r.body["phase"] == "awake" && warned && said.as_str().is_some_and(|t| t.contains("hasn't saved since") && t.contains("If it still can't by")),
        &said,
    );
    // Restart, in the notice: its save fails too, so it goes back, and says so
    b.eval(page, &format!("({}.querySelector('button[data-action=restart]').click(), true)", shown("unsaved")))?;
    let back = b.until(page, &format!("{n}?.innerText.includes(\"Your restart couldn't save first\") && !!{n}.querySelector('button[data-action=seen]')", n = shown("went_back")), std::time::Duration::from_secs(90));
    let v = view();
    let said = b.eval(page, "document.getElementById('computer-notices').innerText")?;
    let _ = b.screenshot(page, &shots.join("computer-went-back.png"));
    s.ok(
        "Restart in the notice restarts it once, back to its last save, and the notice says which save, with an OK",
        back && v["generation"].as_u64() == Some(generation + 1) && v["restored"]["rollback"] == true && said.as_str().is_some_and(|t| t.contains("It went back to its save of")),
        json!({ "said": said, "view": v }),
    );
    super::computers::lever_with(api, &id, json!({ "op": "fail-saves", "times": 0 }))?;
    b.eval(page, &format!("({}.querySelector('button[data-action=seen]').click(), true)", shown("went_back")))?;
    let gone = b.until(page, "document.getElementById('computer-notices').hidden", wait);
    b.reload(page)?;
    let loaded = b.until(page, "!document.getElementById('settings-page').hidden && document.getElementById('settings-page').innerText.toUpperCase().includes('COMPUTER')", wait);
    let still = b.eval(page, "document.getElementById('computer-notices').hidden")?;
    s.ok("OK: it is told no more, here or after a reload", gone && loaded && still == true && view()["notices"].as_array().is_none_or(|l| l.is_empty()), view());
    // settings' Restart computer asks once more, then restarts it, saved
    let asked = b.until(page, "!!document.querySelector('#settings-page [data-action=restart-ask]')", wait);
    b.click(page, "#settings-page [data-action=restart-ask]")?;
    let confirm = b.until(page, "!!document.querySelector('#settings-page button[data-action=restart]')", wait);
    let before = view()["generation"].as_u64().unwrap_or(0);
    b.click(page, "#settings-page button[data-action=restart]")?;
    let restarted = s.eventually(std::time::Duration::from_secs(90), || view()["generation"].as_u64() == Some(before + 1) && view()["phase"] == "awake");
    let settled = b.until(page, "!!document.querySelector('#settings-page [data-action=restart-ask]') && document.getElementById('computer-notices').hidden", wait);
    let v = view();
    s.ok(
        "settings' Restart computer asks once more, then restarts it, saved first: nothing to tell",
        asked && confirm && restarted && settled && v["restored"]["rollback"] == false && v["notices"].as_array().is_none_or(|l| l.is_empty()),
        &v,
    );
    Ok(())
}

/// An update, as the person sees it (docs/computers.md, "What its owner is
/// told"; Paul, 2026-10-09: "new update, click to restart"): their computer
/// awake, between turns, on another image than its next start runs (a pin
/// here, which is what a deploy that changed its image is to it) is shown
/// at once, with no reload, over settings, with Restart and Later; settings
/// says the version it runs and the latest. Restart takes it, and nothing
/// more is told; Later puts a version off, here and after a reload, while
/// settings still says the latest.
fn update_ui(s: &mut Suite, api: &Api, b: &mut Browser, page: &Page, session: &str, shots: &std::path::Path) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let computers = shell(api, session, "GET", "/api/computers", None, &[])?;
    let id = computers.body["computers"][0]["computer"].as_str().unwrap_or("").to_string();
    let view = || shell(api, session, "GET", &format!("/api/computers/{id}"), None, &[]).map(|r| r.body).unwrap_or(Value::Null);
    let shown = "document.querySelector('#computer-notices:not([hidden]) .computer-notice[data-kind=update]')";
    let settings_text = |b: &mut Browser| b.eval(page, "document.getElementById('settings-page').innerText").ok().and_then(|t| t.as_str().map(str::to_string)).unwrap_or_default();
    let idle = |s: &Suite| s.eventually(std::time::Duration::from_secs(15), || super::computers::lever(api, &id, "saves").is_ok_and(|r| r.body["keepalives"] == 0));
    // settings' Computer section, in view for its screenshot
    let to_computer = "([...document.querySelectorAll('#settings-page h2')].find((h) => h.textContent === 'Computer')?.scrollIntoView({ block: 'start' }), true)";
    let first = view();
    let running = first["version"].as_str().unwrap_or("").to_string();
    idle(s);
    let r = shell(api, session, "PUT", &format!("/api/computers/{id}/image"), Some(&json!({ "image": "stub-next" })), &[])?;
    let latest = r.body["latest"].as_str().unwrap_or("").to_string();
    let told = b.until(page, &format!("{shown}?.innerText.includes('An update is ready for your computer') && !!{shown}.querySelector('button[data-action=restart]') && !!{shown}.querySelector('button[data-action=later]')"), wait);
    let said = b.eval(page, "document.getElementById('computer-notices').innerText")?;
    let _ = b.screenshot(page, &shots.join("computer-update.png"));
    s.ok(
        "an update is shown at once, with no reload: an update is ready, Restart (about a minute, chats and work kept; or by itself at its next sleep), and Later",
        r.status == 200 && told && !running.is_empty() && latest != running && said.as_str().is_some_and(|t| t.contains("about a minute; your chats and work are kept") && t.contains("the next time your computer sleeps")),
        json!({ "said": said, "view": r.body }),
    );
    // a read of it failing meanwhile (#437): what the last one said stays
    refresh_failed_ui(s, api, b, page, session, first["agents"][0]["fragment"].as_str().unwrap_or(""), shots)?;
    // settings, read again, says the version it runs and the latest
    b.reload(page)?;
    let versions = b.until(page, "document.getElementById('settings-page').innerText.includes('Latest version')", wait);
    let text = settings_text(b);
    b.eval(page, to_computer)?;
    let _ = b.screenshot(page, &shots.join("settings-update.png"));
    s.ok("settings says the version it runs, and the latest, ready", versions && text.contains(&running) && text.contains(&latest), &text[..text.len().min(800)]);
    // Restart, in the notice: it takes the update, and nothing more is told
    b.eval(page, &format!("({shown}.querySelector('button[data-action=restart]').click(), true)"))?;
    let restarted = s.eventually(std::time::Duration::from_secs(90), || {
        let v = view();
        v["phase"] == "awake" && v["version"] == latest.as_str()
    });
    let settled = b.until(page, "document.getElementById('computer-notices').hidden && !!document.querySelector('#settings-page [data-action=restart-ask]') && !document.getElementById('settings-page').innerText.includes('Latest version')", wait);
    let v = view();
    s.ok("Restart in the notice takes it: it runs the latest, and nothing more is told", restarted && settled && v["latest"] == latest.as_str() && v["notices"].as_array().is_none_or(|l| l.is_empty()), &v);
    // pinned back, another version is ready; Later puts it off, here and after a reload
    idle(s);
    shell(api, session, "PUT", &format!("/api/computers/{id}/image"), Some(&json!({ "image": "stub" })), &[])?;
    let again = b.until(page, &format!("!!{shown}?.querySelector('button[data-action=later]')"), wait);
    b.eval(page, &format!("({shown}.querySelector('button[data-action=later]').click(), true)"))?;
    let hidden = b.until(page, "document.getElementById('computer-notices').hidden", wait);
    b.reload(page)?;
    let loaded = b.until(page, "!document.getElementById('settings-page').hidden && document.getElementById('settings-page').innerText.includes('Latest version')", wait);
    let still = b.eval(page, "document.getElementById('computer-notices').hidden")?;
    b.eval(page, to_computer)?;
    let _ = b.screenshot(page, &shots.join("settings-update-later.png"));
    let v = view();
    s.ok(
        "Later: that version is told no more, here or after a reload, and settings still says the latest",
        again && hidden && loaded && still == true && v["notices"].as_array().is_none_or(|l| l.is_empty()) && v["latest"] == running.as_str() && v["version"] == latest.as_str(),
        &v,
    );
    Ok(())
}

/// The shell's page with its reads of its person's computer (`GET
/// /api/computers`) failing as a blip's do while `window.__failComputers`
/// holds (counted: `__computerReads`); with `__holdMakes` set before it, a
/// first run's makes (`POST /api/fragments`, `POST /api/computers`) held
/// for good (counted: `__makes`), so nothing is made; and whether it ever
/// said "Making your agent…" (`__makingSaid`). Its top window's alone.
const FAIL_COMPUTERS: &str = "(() => { if (window !== window.top || window.__computerReads !== undefined) return true; \
    const real = window.fetch.bind(window); Object.assign(window, { __failComputers: true, __computerReads: 0, __makes: 0, __makingSaid: false }); \
    window.fetch = async (url, o) => { \
      if (url === '/api/computers' && o?.method === 'GET') { window.__computerReads++; if (window.__failComputers) throw new TypeError('Failed to fetch'); } \
      if (window.__holdMakes && o?.method === 'POST' && ['/api/fragments', '/api/computers'].includes(url)) { window.__makes++; await new Promise(() => {}); } \
      return real(url, o); }; \
    new MutationObserver(() => { if (document.getElementById('notice')?.textContent.includes('Making your agent')) window.__makingSaid = true; }).observe(document, { subtree: true, childList: true, characterData: true }); \
    return true; })()";
/// The band's line for a read of the computer that failed, its Try again ready.
const REFRESH_LINE: &str = "(() => { const r = document.querySelector('#computer-notices:not([hidden]) .computer-notice[data-kind=refresh]'); \
    return !!r && r.innerText.includes(\"Couldn't refresh your computer\") && !!r.querySelector('button[data-action=refresh]:enabled'); })()";
/// The band says nothing of a failed read.
const NO_REFRESH_LINE: &str = "!document.querySelector('#computer-notices .computer-notice[data-kind=refresh]')";
/// Try again pressed, the computer's reads let through.
const READ_AGAIN: &str = "(window.__failComputers = false, document.querySelector('#computer-notices [data-action=refresh]')?.click(), true)";
/// What the page made and said, under `FAIL_COMPUTERS`.
const MADE: &str = "({ notice: document.getElementById('notice').hidden ? null : document.getElementById('notice').innerText, band: document.getElementById('computer-notices').innerText, \
    makingSaid: window.__makingSaid, makes: window.__makes, reads: window.__computerReads })";

/// What their agent's menu offers, opened from its pill and closed again.
fn agent_menu(b: &mut Browser, page: &Page) -> Result<Vec<String>> {
    b.click(page, "#sidebar-agent")?;
    b.until(page, "!document.getElementById('menu').hidden", std::time::Duration::from_secs(5));
    let items = b.eval(page, "[...document.querySelectorAll('#menu button')].map((b) => b.textContent)")?;
    b.press(page, "Escape", false)?;
    Ok(strings(&items))
}

/// Whether an agent's menu knows the agent and its computer: a new chat
/// with it (its identity, from the computer's view) and its screen.
fn knows_agent(menu: &[String]) -> bool {
    ["New chat", "Its screen"].iter().all(|item| menu.iter().any(|m| m == item))
}

/// A read of the computer that fails, the shell open (#437: a blip or a
/// 5xx was taken for "no computer", and its notices and agents went). A
/// change their list's socket tells the page (an archive that changes
/// nothing) has it read again, and its read of the computer fails: the
/// band keeps the update it told and adds "Couldn't refresh your computer"
/// with Try again, and their agent's menu still knows it. Try again, the
/// reads let through: the line goes, the update stays.
fn refresh_failed_ui(s: &mut Suite, api: &Api, b: &mut Browser, page: &Page, session: &str, fragment: &str, shots: &std::path::Path) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let update = "!!document.querySelector('#computer-notices:not([hidden]) .computer-notice[data-kind=update] button[data-action=restart]')";
    b.eval(page, FAIL_COMPUTERS)?;
    let r = shell(api, session, "PUT", &format!("/api/fragments/{fragment}/archived"), Some(&json!({ "archived": false })), &[])?;
    let failed = b.until(page, &format!("window.__computerReads > 0 && {REFRESH_LINE} && {update}"), wait);
    let said = b.eval(page, "document.getElementById('computer-notices').innerText")?;
    let menu = agent_menu(b, page)?;
    let _ = b.screenshot(page, &shots.join("computer-refresh-failed.png"));
    s.ok(
        "a read of the computer that fails is not \"no computer\": the update it told stays, with \"Couldn't refresh your computer\" and Try again, and their agent's menu still offers a new chat with it and its screen",
        r.status == 200 && failed && knows_agent(&menu),
        json!({ "said": said, "menu": menu, "reads": b.eval(page, "window.__computerReads")? }),
    );
    b.eval(page, READ_AGAIN)?;
    let back = b.until(page, &format!("{NO_REFRESH_LINE} && {update}"), wait);
    s.ok("Try again reads it again: the line goes, the update stays", back, b.eval(page, "document.getElementById('computer-notices').innerText")?);
    Ok(())
}

/// A read of the computer that fails from the page's start (#437: someone
/// with an agent was sent to "Making your agent…", their agents unknown
/// taken for none), every read of it failing and a first run's makes held
/// (`FAIL_COMPUTERS`). Someone with an agent sees their chat and the line,
/// and no first run; Try again knows their agent and makes nothing. A new
/// person, with no chats, has their first run all the same, and Try again
/// during it starts no other. Someone with only the chat an earlier try
/// made (no agent yet) sees that chat and the line, and no first run,
/// until Try again reads their computer: then the first run goes on there.
fn refresh_failed_start_ui(s: &mut Suite, api: &Api, b: &mut Browser, shots: &std::path::Path) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let settle = std::time::Duration::from_secs(2);
    let script = format!("window.__holdMakes = true; {FAIL_COMPUTERS}");
    let open = |b: &mut Browser| -> Result<Page> {
        let page = b.open_with_script(&format!("{}/", api.base), &script)?;
        b.viewport(&page, 1280, 800, false)?;
        Ok(page)
    };
    let chat_open = format!("!document.getElementById('layout').hidden && !!document.querySelector('#frames iframe:not([hidden])') && {REFRESH_LINE}");

    // someone with an agent (the browser's person)
    let page = open(b)?;
    let shown = b.until(&page, &chat_open, wait);
    std::thread::sleep(settle);
    let made = b.eval(&page, MADE)?;
    let _ = b.screenshot(&page, &shots.join("start-refresh-failed.png"));
    s.ok(
        "their computer unread from the page's start, someone with an agent sees their chat and \"Couldn't refresh your computer\" with Try again, never \"Making your agent…\", and nothing is made",
        shown && made["makingSaid"] == false && made["makes"] == 0,
        &made,
    );
    b.eval(&page, READ_AGAIN)?;
    let read = b.until(&page, NO_REFRESH_LINE, wait);
    let menu = agent_menu(b, &page)?;
    std::thread::sleep(settle);
    let made = b.eval(&page, MADE)?;
    s.ok(
        "Try again reads it: the line goes, their agent's menu offers a new chat with it and its screen, and still nothing is made",
        read && knows_agent(&menu) && made["makingSaid"] == false && made["makes"] == 0,
        json!({ "menu": menu, "made": made }),
    );
    b.close(page)?;

    // a new person: no chats, so their first run, the line beside it
    let session = api.sign_in(&format!("shell-ui-unread-{}@e2e.test", crate::api::now_s()))?;
    b.set_cookie(&format!("{}/", api.base), "fragment_session", &session)?;
    let page = open(b)?;
    let first = b.until(&page, &format!("window.__makingSaid === true && window.__makes === 1 && {REFRESH_LINE}"), wait);
    let made = b.eval(&page, MADE)?;
    let _ = b.screenshot(&page, &shots.join("start-refresh-failed-new.png"));
    s.ok("someone with no chats has their first run all the same (\"Making your agent…\"), the line beside it", first, &made);
    b.eval(&page, READ_AGAIN)?;
    let read = b.until(&page, NO_REFRESH_LINE, wait);
    std::thread::sleep(settle);
    let made = b.eval(&page, MADE)?;
    s.ok("Try again during it reads the computer, and starts no other run", read && made["makes"] == 1, &made);
    b.close(page)?;

    // the chat an earlier try made, and no agent yet
    let r = shell(api, &session, "POST", "/api/fragments", Some(&json!({ "label": "basil-chat", "template": "chat", "title": FIRST_CHAT })), &[])?;
    anyhow::ensure!(r.status == 200, "making an earlier try's chat: {r}");
    let chat = r.body["name"].as_str().unwrap_or("").to_string();
    let page = open(b)?;
    let shown = b.until(&page, &chat_open, wait);
    std::thread::sleep(settle);
    let made = b.eval(&page, MADE)?;
    s.ok(
        "someone with only the chat an earlier try made, their computer unread, sees that chat and the line, and no first run yet",
        shown && made["makingSaid"] == false && made["makes"] == 0,
        &made,
    );
    b.eval(&page, READ_AGAIN)?;
    let on = b.until(&page, &format!("{NO_REFRESH_LINE} && window.__makingSaid === true && window.__makes > 0"), wait);
    let card = s.eventually(wait, || b.eval_in_frame(&page, &chat, "!!document.querySelector('.setup-card')").ok() == Some(Value::Bool(true)));
    let _ = b.screenshot(&page, &shots.join("start-refresh-failed-earlier-try.png"));
    let framed = b.eval_in_frame(&page, &chat, "({ text: document.body.innerText.slice(0, 300), hidden: document.hidden })").unwrap_or_else(|e| json!(e.to_string()));
    s.ok("Try again reads it, finds no agent, and the first run goes on in that chat (its card)", on && card, json!({ "made": b.eval(&page, MADE)?, "chat": framed }));
    b.close(page)?;
    Ok(())
}

/// The Connections page in settings (decisions 22, 37 and 44): every
/// provider the deployment offers, one row each, with its kind, the
/// person's state there, which agents may use it (and a press that narrows
/// one), and this month's calls by agent with an operator key's cost. The
/// person's first agent calls a connection and an operator key through its
/// computer (the stub's `fetch`), so the page has uses to show.
fn connections_ui(s: &mut Suite, api: &Api, b: &mut Browser, page: &Page, session: &str, email: &str, chat: &str) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let computers = shell(api, session, "GET", "/api/computers", None, &[])?;
    let computer = computers.body["computers"][0].clone();
    let agents: Vec<(String, String)> = computer["agents"].as_array().map(|l| l.iter().map(|a| (a["fragment"].as_str().unwrap_or("").to_string(), a["identity"].as_str().unwrap_or("").to_string())).collect()).unwrap_or_default();
    let Some((lead, lead_id)) = agents.first().cloned() else {
        s.ok("the Connections page has agents to show", false, &computers);
        return Ok(());
    };
    // the person connects Google, and reading their connections tells their computer
    s.workos.connect(email, crate::SWAP_CONNECTION, true);
    shell(api, session, "GET", "/api/connections", None, &[])?;
    let replies = || {
        shell(api, session, "GET", &format!("/api/f/{chat}/channels/chat"), None, &[])
            .ok()
            .and_then(|r| r.body["records"].as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r["principal"] == lead_id.as_str() && r["body"]["text"].as_str().is_some_and(|t| t.starts_with("fetched")))
            .count()
    };
    let calls = [format!("fetch http://{}/gmail/v1/users/me/profile with ${}", crate::SWAP_CONNECTION_HOST, crate::SWAP_CONNECTION_ENV), "fetch http://api.perplexity.ai/search with $PERPLEXITY_API_KEY".to_string()];
    for (n, text) in calls.iter().enumerate() {
        shell(api, session, "POST", &format!("/api/f/{chat}/channels/chat"), Some(&json!({ "id": format!("conn-{n}"), "body": { "text": text } })), &[])?;
        s.eventually(std::time::Duration::from_secs(60), || replies() > n);
    }
    let id = computer["computer"].as_str().unwrap_or("").to_string();
    let counted = s.eventually(wait, || {
        shell(api, session, "GET", &format!("/api/computers/{id}/uses"), None, &[]).is_ok_and(|r| r.body["uses"].as_array().is_some_and(|l| l.len() >= 2))
    });
    b.reload(page)?;
    let row = |p: &str| format!("#settings-connections [data-provider={p:?}]");
    // each row: its provider, kind, state, agents allowed, and uses
    let rows = "[...document.querySelectorAll('#settings-connections [data-provider]')].map((r) => [r.dataset.provider, r.dataset.kind, r.dataset.state, \
                r.querySelectorAll('[data-agent][aria-pressed=true]').length, [...r.querySelectorAll('[data-use]')].map((u) => [u.dataset.use, Number(u.dataset.calls), Number(u.dataset.micros)])])";
    let shown = b.until(page, &format!("document.querySelectorAll('#settings-connections [data-provider]').length === {} && !!document.querySelector('{} [data-use]')", crate::SWAP_KEYS.len() + 4, row("perplexity")), wait);
    let got = b.eval(page, rows)?;
    let of = |p: &str| got.as_array().and_then(|l| l.iter().find(|r| r[0] == p)).cloned().unwrap_or(Value::Null);
    let perplexity_charge = 7_500; // $0.005 a call at list, and the margin
    s.ok(
        "settings' Connections lists every provider, one row each, its kind and the person's state: Google connected, the operator's keys offered, own keys not set",
        shown
            && of(crate::SWAP_CONNECTION)[1] == "connection"
            && of(crate::SWAP_CONNECTION)[2] == "connected"
            && crate::SWAP_KEYS.iter().all(|(k, _, _)| of(k)[1] == "operator" && of(k)[2] == "offered")
            && of(crate::SWAP_OWN)[1] == "own"
            && of(crate::SWAP_OWN)[2] == "not_set"
            && of(crate::SWAP_ROUTER)[1] == "own"
            && of(crate::SWAP_ROUTER)[2] == "not_set",
        &got,
    );
    s.ok("each row names which agents may use it: all of them, by default", got.as_array().is_some_and(|l| l.iter().all(|r| r[3] == agents.len())), &got);
    s.ok(
        "and this month's use by agent: a connection's calls counted, an operator key's with its cost",
        counted && of(crate::SWAP_CONNECTION)[4] == json!([[lead, 1, 0]]) && of("perplexity")[4] == json!([[lead, 1, perplexity_charge]]),
        &got,
    );
    b.eval(page, "(document.getElementById('settings-connections').scrollIntoView(), true)")?;
    // the lane's pictures so far are there: `dir` would start it afresh
    let _ = b.screenshot(page, &s.scratch.join("shell-ui/desktop-connections.png"));
    // a press narrows that agent from that provider, and the page says so
    b.eval(page, &format!("document.querySelector('{} [data-agent={lead:?}]').click(), true", row("perplexity")))?;
    let narrowed = b.until(page, &format!("document.querySelector('{} [data-agent={lead:?}]')?.getAttribute('aria-pressed') === 'false'", row("perplexity")), wait);
    let view = shell(api, session, "GET", &format!("/api/computers/{id}"), None, &[])?;
    let list = view.body["agents"].as_array().and_then(|l| l.iter().find(|a| a["fragment"] == lead.as_str())).map(|a| a["connections"].clone()).unwrap_or(Value::Null);
    let others: Vec<&str> = std::iter::once(crate::SWAP_CONNECTION).chain(crate::SWAP_KEYS.iter().map(|(k, _, _)| *k).filter(|k| *k != "perplexity")).chain([crate::SWAP_OWN, crate::SWAP_ROUTER, "e2e-oauth"]).collect();
    s.ok(
        "pressing an agent takes that provider from it (a narrowing of the rest), and the page says so",
        narrowed && list.as_array().is_some_and(|l| l.len() == others.len() && others.iter().all(|p| l.iter().any(|x| x == p))),
        &list,
    );
    b.eval(page, &format!("document.querySelector('{} [data-agent={lead:?}]').click(), true", row("perplexity")))?;
    let again = b.until(page, &format!("document.querySelector('{} [data-agent={lead:?}]')?.getAttribute('aria-pressed') === 'true'", row("perplexity")), wait);
    let view = shell(api, session, "GET", &format!("/api/computers/{id}"), None, &[])?;
    let back = view.body["agents"].as_array().and_then(|l| l.iter().find(|a| a["fragment"] == lead.as_str())).is_some_and(|a| a["connections"].is_null());
    s.ok("pressed again, it may use every provider again (null)", again && back, &view);
    Ok(())
}

/// Settings' own models (Paul, 2026-10-08: "they should be able to connect
/// them from settings ideally"): OpenRouter's row is connected with a
/// press, through its own page (the fake's, which approves at once), with
/// no key pasted and no CLI; then each agent offers the provider's models
/// beside its tier's (named by the tier, never a vendor), and the one
/// picked is its agent.json's `model`, its tier kept, which its computer
/// runs its next turn on. Its tier again takes the model away;
/// disconnecting takes the offer.
fn models_ui(s: &mut Suite, api: &Api, b: &mut Browser, page: &Page, session: &str, shots: &std::path::Path) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let row = format!("#settings-connections [data-provider={:?}]", crate::SWAP_ROUTER);
    b.reload(page)?;
    let offered = b.until(page, &format!("document.querySelector('{row}')?.dataset.state === 'not_set' && !!document.querySelector('{row} [data-action=sign-in]')"), wait);
    let pasted = b.eval(page, &format!("!!document.querySelector('{row} input')"))?;
    s.ok(
        "an own key with a sign-in is connected from its row with a press, never pasted: no key field, and no agent offers its models yet",
        offered && pasted == false && b.eval(page, "document.querySelectorAll('[data-model-for]').length")? == 0,
        b.eval(page, &format!("document.querySelector('{row}')?.innerText"))?,
    );
    // the press opens the provider's page, which sends the browser back to the platform
    let keys_before = s.openrouter.keys().len();
    b.click(page, &format!("{row} [data-action=sign-in]"))?;
    let callback = format!("/api/connections/{}/callback", crate::SWAP_ROUTER);
    let mut popup = None;
    let came_back = s.eventually(wait, || {
        popup = b.pages().ok().and_then(|l| l.into_iter().find(|(_, url)| url.contains(&callback)).map(|(id, _)| id));
        popup.is_some()
    });
    let said = match &popup {
        Some(id) => {
            let p = b.attach(id)?;
            let connected = b.until(&p, "document.body?.innerText.includes('is connected')", wait);
            let text = b.eval(&p, "document.body?.innerText ?? ''")?;
            b.close(p)?;
            (connected, text)
        }
        None => (false, Value::Null),
    };
    b.reload(page)?;
    let set = b.until(page, &format!("document.querySelector('{row}')?.dataset.state === 'set' && !!document.querySelector('{row} [data-action=disconnect]')"), wait);
    let manage = b.eval(page, &format!("document.querySelector('{row} a[target=_blank][rel=noopener]')?.href ?? null"))?;
    s.ok(
        "pressed, its provider's page sends the browser back to the platform, which says it is connected; settings shows it connected, a Disconnect, and the provider's page to revoke the key",
        came_back && said.0 && set && s.openrouter.keys().len() == keys_before + 1 && manage == "https://openrouter.ai/settings/keys",
        json!({ "popup": said.1, "manage": manage }),
    );
    // each agent offers the provider's models beside its tier's, by the
    // tier's name and its model's id, as their computer's view names it
    let lead = b.eval(page, "document.querySelector('[data-model-for]')?.dataset.modelFor ?? ''")?.as_str().unwrap_or("").to_string();
    let read = b.until(page, &format!("document.querySelector('[data-model-for={lead:?}]')?.dataset.read === '1'"), wait);
    let options = b.eval(page, &format!("[...(document.querySelector('[data-model-for={lead:?}]')?.options ?? [])].map((o) => [o.value, o.textContent])"))?;
    let options = options.as_array().cloned().unwrap_or_default();
    let agent_json = || shell(api, session, "GET", &format!("/api/f/{lead}/file?path=agent.json"), None, &[]).map(|r| r.body).unwrap_or(Value::Null);
    let before = agent_json();
    let view = || shell(api, session, "GET", "/api/computers", None, &[]).map(|r| r.body["computers"][0].clone()).unwrap_or(Value::Null);
    let runs_on = |v: &Value| v["agents"].as_array().and_then(|l| l.iter().find(|a| a["fragment"] == lead.as_str())).map(|a| a["modelProviders"].clone()).unwrap_or(Value::Null);
    let connected = view();
    let cheap = connected["models"]["cheap"].as_str().unwrap_or("").to_string();
    let vendor = b.eval(page, "document.getElementById('settings-page').innerText.includes('Workers AI') || [...document.querySelectorAll('[data-model-for] option')].some((o) => o.textContent.includes('Workers AI'))")?;
    s.ok(
        "then each agent offers the provider's models, after its tier's, which it runs on now, named by its tier and its model's id as their computer's view names it (Cheap (<id>), never \"cheap\") and no vendor: no Workers AI anywhere in settings; the view names it among the providers whose models the agent may run",
        read && !lead.is_empty()
            && before["tier"] == "cheap"
            && !cheap.is_empty() && cheap != "cheap"
            && ["medium", "fallback", "vision"].iter().all(|n| connected["models"][n].as_str().is_some_and(|id| !id.is_empty()))
            && options.first().is_some_and(|o| o[0] == "" && o[1] == format!("Cheap ({cheap})").as_str())
            && options.iter().any(|o| o[0] == "openrouter anthropic/claude-sonnet-5.5" && o[1] == "Claude Sonnet 5.5")
            && vendor == false
            && runs_on(&connected) == json!([crate::SWAP_ROUTER]),
        json!({ "options": options, "agentJson": before, "workersAi": vendor, "view": connected }),
    );
    let pick = |b: &mut Browser, value: &str| -> Result<bool> {
        b.eval(page, &format!("(() => {{ const p = document.querySelector('[data-model-for={lead:?}]'); p.value = {value:?}; p.dispatchEvent(new Event('change')); return true; }})()"))?;
        Ok(b.until(page, &format!("document.querySelector('[data-model-for={lead:?}]')?.dataset.saved === {value:?}"), wait))
    };
    let saved = pick(b, "openrouter anthropic/claude-sonnet-5.5")?;
    let after = agent_json();
    b.eval(page, &format!("(document.querySelector('[data-model-for={lead:?}]')?.scrollIntoView({{ block: 'center' }}), true)"))?;
    let _ = b.screenshot(page, &shots.join("desktop-own-models.png"));
    b.eval(page, &format!("(document.querySelector('{row}')?.scrollIntoView({{ block: 'center' }}), true)"))?;
    let _ = b.screenshot(page, &shots.join("desktop-own-models-connected.png"));
    s.ok(
        "picked, it is the agent's agent.json model ({provider, id}), its tier kept, and no colour (its identity's)",
        saved && after["model"] == json!({ "provider": "openrouter", "id": "anthropic/claude-sonnet-5.5" }) && after["tier"] == before["tier"] && after.get("color").is_none(),
        json!({ "before": before, "after": after }),
    );
    // its page (its row's Open) names the model it runs: that one, while its
    // computer gives it the provider's models; narrowed from the provider,
    // its tier's, by its id (Paul, 2026-10-20: it said "cheap model"), and
    // back again, each with no reload (its owner's view tells the shell)
    b.eval(page, &format!("(document.querySelector('.agent-setting[data-agent={lead:?}] button.row')?.click(), true)"))?;
    let its_page = api.site_origin(&lead);
    let shows = |b: &mut Browser, want: &str| s.eventually(wait, || b.eval_in_frame(page, &its_page, "document.getElementById('tier')?.textContent ?? null").ok() == Some(json!(want)));
    let own_shown = shows(b, "anthropic/claude-sonnet-5.5 (openrouter)");
    let narrow = |list: Value| shell(api, session, "PUT", &format!("/api/computers/{}/agents/{lead}/connections", connected["computer"].as_str().unwrap_or("")), Some(&json!({ "connections": list })), &[]);
    let narrowed = narrow(json!([]))?;
    let tier_shown = narrowed.status == 200 && shows(b, &cheap);
    let unused = runs_on(&view());
    let _ = b.screenshot(page, &shots.join("agent-page-model-unused.png"));
    let widened = narrow(Value::Null)?;
    let own_again = widened.status == 200 && shows(b, "anthropic/claude-sonnet-5.5 (openrouter)");
    s.ok(
        "its page names the model it runs by its id: its own while its computer gives it the provider's models; narrowed from the provider, its tier's (the view names none it may run), never the one agent.json names; widened, its own again",
        own_shown && tier_shown && unused.is_null() && own_again,
        json!({ "page": b.eval_in_frame(page, &its_page, "document.getElementById('tier')?.textContent ?? null").ok(), "tier": cheap, "narrowed": unused }),
    );
    b.click(page, "#settings")?;
    b.until(page, &format!("location.pathname === '/settings' && !!document.querySelector('[data-model-for={lead:?}]')"), wait);
    b.reload(page)?;
    let shown = b.until(page, &format!("document.querySelector('[data-model-for={lead:?}]')?.dataset.read === '1' && document.querySelector('[data-model-for={lead:?}]').value === 'openrouter anthropic/claude-sonnet-5.5'"), wait);
    let back = pick(b, "")?;
    let gone = agent_json();
    s.ok("settings shows it again as picked; its tier again takes the model away, its tier kept", shown && back && gone.get("model").is_none() && gone["tier"] == before["tier"], &gone);
    // disconnected: the key goes, and no agent offers its models
    b.click(page, &format!("{row} [data-action=disconnect]"))?;
    let off = b.until(page, &format!("document.querySelector('{row}')?.dataset.state === 'not_set' && document.querySelectorAll('[data-model-for]').length === 0"), wait);
    let left = runs_on(&view());
    s.ok("Disconnect takes the key away, and no agent offers its models, nor may run them", off && left.is_null(), json!({ "row": b.eval(page, &format!("document.querySelector('{row}')?.innerText"))?, "modelProviders": left }));
    Ok(())
}

/// How soon an open shell shows a change to its person's list, made
/// anywhere: "within a few seconds", with no reload.
const SIDEBAR_LIVE_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// The sidebar is live (Paul on p5, 2026-10-05: "Fred published a
/// calories app but it didn't show up in my apps list on the left until I
/// reloaded"). Method: the shell open in two tabs; an agent of the
/// person's, acting for them (`?for=<owner>`, as their computer's egress
/// signs a `fragment` CLI's requests in agent mode), makes an app and
/// deploys it through the API. Valid: both tabs list it within a few
/// seconds, with no reload, and its card once the platform shoots it; the
/// first tab's open chat is not reloaded, and the rows it showed are the
/// same elements (patched in place). Then the same for an app someone
/// else shares with them, one deleted elsewhere, and one archived in the
/// other tab.
fn sidebar_live(s: &mut Suite, api: &Api, b: &mut Browser, page: &Page, session: &str, shots: &std::path::Path) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let me = super::signin::who(api, session)?["id"].as_str().unwrap_or("").to_string();
    // their agent: a key of theirs (`fragment login`'s) registers it
    let keys = fragment_nip98::Keys::generate();
    api.approve(session, &keys)?;
    let (agent, _) = super::delegation::agent_of(api, &keys)?;
    let second = b.open(&format!("{}/", api.base))?;
    let ready = b.until(&second, "!document.getElementById('layout').hidden && document.querySelectorAll('#apps [data-key^=\"app:\"]').length === 1", wait);
    // what the first tab shows: its open chat (counting its frame's loads) and its rows, marked
    let marked = b.eval(
        page,
        "(() => { const f = [...document.querySelectorAll('#frames iframe')].find((f) => !f.hidden); window.__loads = 0; f?.addEventListener('load', () => window.__loads++); \
         window.__open = f?.dataset.fragment ?? null; const rows = [...document.querySelectorAll('#chats .row[data-key], #apps [data-key]')]; rows.forEach((r) => { r.__kept = true; }); return rows.length; })()",
    )?;
    let row = |name: &str| format!("document.querySelector({})", js(&format!("#apps [data-key={}]", js(&format!("app:{name}")))));
    let both = |b: &mut Browser, expr: &str| b.until(page, expr, SIDEBAR_LIVE_WAIT) && b.until(&second, expr, SIDEBAR_LIVE_WAIT);

    // the agent makes an app for its owner, writes its page, and deploys it
    let as_agent = |method: &str, path: &str, body: Value| api.signed(&agent, method, &format!("{path}?for={me}"), Some(&body));
    // the first tab is the one the person looks at
    b.front(page)?;
    let made = as_agent("POST", "/api/fragments", json!({ "label": "meals", "template": "blank" }))?;
    let t0 = std::time::Instant::now();
    let name = made.body["name"].as_str().unwrap_or("").to_string();
    let first = b.until(page, &row(&name), SIDEBAR_LIVE_WAIT);
    let first_ms = t0.elapsed().as_millis() as u64;
    let other_tab = b.until(&second, &row(&name), SIDEBAR_LIVE_WAIT);
    let second_ms = t0.elapsed().as_millis() as u64;
    let shown = first && other_tab;
    println!("      (the agent's app showed in the open tab in {first_ms} ms, in the second by {second_ms} ms)");
    let page_html = "<!doctype html><title>Meals</title><h1>Meals</h1><p>What we ate today.</p>";
    let wrote = as_agent("POST", &format!("/api/f/{name}/files"), json!({ "message": "the page", "files": [{ "path": "site/index.html", "text": page_html }] }))?;
    let deployed = as_agent("POST", &format!("/api/f/{name}/deploy"), json!({}))?;
    s.ok(
        "an app the person's agent makes for them shows in the open shell's dock within a few seconds, with no reload, and in a second tab",
        ready && made.status == 200 && shown,
        json!({ "made": made.status, "firstInMs": first_ms, "secondInMs": second_ms, "first": b.eval(page, "[...document.querySelectorAll('#apps [data-key]')].map((r) => r.dataset.key)")?, "second": b.eval(&second, "[...document.querySelectorAll('#apps [data-key]')].map((r) => r.dataset.key)")? }),
    );
    let carded = b.until(
        page,
        &format!("(() => {{ {}?.dispatchEvent(new PointerEvent('pointerenter', {{ pointerType: 'mouse' }})); const i = document.querySelector('.card-peek:not([hidden]) img'); return !!i && i.src.startsWith('blob:'); }})()", row(&name)),
        super::site::CARD_WAIT,
    );
    b.eval(page, &format!("({}?.dispatchEvent(new PointerEvent('pointerleave')), true)", row(&name)))?;
    s.ok("its icon shows its card once the platform shoots its deploy", wrote.status == 200 && deployed.status == 200 && carded, json!([wrote.status, deployed.status]));
    let kept = b.eval(
        page,
        "(() => { const f = [...document.querySelectorAll('#frames iframe')].find((f) => !f.hidden); return { loads: window.__loads, open: f?.dataset.fragment ?? null, was: window.__open, \
         kept: [...document.querySelectorAll('#chats .row[data-key], #apps [data-key]')].filter((r) => r.__kept).length }; })()",
    )?;
    s.ok(
        "the open chat stays open, its frame not reloaded, and the rows shown before are the same elements (patched in place)",
        kept["loads"] == 0 && kept["open"] == kept["was"] && !kept["open"].is_null() && kept["kept"] == marked && marked.as_i64().is_some_and(|n| n > 0),
        json!({ "kept": kept, "marked": marked }),
    );
    let _ = b.screenshot(page, &shots.join("desktop-sidebar-live.png"));

    // shared by someone else, deleted elsewhere, archived in the other tab
    let other = api.person()?;
    let theirs = api.qualified(&other, "potluck")?;
    let r = api.create(&other, &theirs)?;
    anyhow::ensure!(r.status == 200, "the other person's app: {r}");
    let r = api.signed(&other, "PUT", &format!("/api/f/{theirs}/members/{me}"), Some(&json!({ "role": "viewer" })))?;
    let lent = both(b, &format!("{}?.dataset.role === 'viewer' && {}.title.endsWith('(viewer)')", row(&theirs), row(&theirs)));
    s.ok("an app someone else shares with them shows in both tabs' docks, as a viewer's", r.status == 200 && lent, &r);
    let there = b.eval(page, &format!("!!{}", row(&name)))? == true;
    let r = shell(api, session, "DELETE", &format!("/api/f/{name}"), None, &[])?;
    let gone = both(b, &format!("!{}", row(&name)));
    s.ok("an app deleted elsewhere leaves both tabs' docks", there && r.status == 200 && gone, &r);
    // the other tab archives it as its page does (the app's menu's call), and says nothing to the first
    b.eval(&second, &format!("(fetch('/api/fragments/{theirs}/archived', {{ method: 'PUT', headers: {{ 'x-fragment-shell': '1', 'content-type': 'application/json' }}, body: '{{\"archived\":true}}' }}), true)"))?;
    let archived = b.until(page, &format!("!{}", row(&theirs)), SIDEBAR_LIVE_WAIT);
    shell(api, session, "PUT", &format!("/api/fragments/{theirs}/archived"), Some(&json!({ "archived": false })), &[])?;
    let back = b.until(page, &row(&theirs), SIDEBAR_LIVE_WAIT);
    s.ok("one archived in the other tab leaves the first's, and comes back unarchived", lent && archived && back, "");
    b.close(second)?;
    Ok(())
}

/// The person's own skills fragments (kind `skills`, theirs), as their list says.
fn own_skills(api: &Api, session: &str) -> Result<Vec<String>> {
    let list = shell(api, session, "GET", "/api/fragments", None, &[])?;
    Ok(list.body["fragments"].as_array().into_iter().flatten().filter(|f| f["kind"] == "skills" && f["role"] == "owner").filter_map(|f| f["name"].as_str().map(str::to_string)).collect())
}

/// The managed skills added from settings (decision 17): a person with an
/// agent and no skills fragment (theirs deleted here; people set up before
/// 2026-10-03 have none). Valid: their settings offer the managed skills,
/// and the button makes the fragment from the blessed template and lists
/// it. Replay: loaded again, there is no second. Loading makes none unasked.
fn add_skills_ui(s: &mut Suite, api: &Api, b: &mut Browser, page: &Page, session: &str) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let before = own_skills(api, session)?;
    let Some(old) = before.first().cloned() else {
        s.ok("the shell made the person's skills fragment at setup", false, json!(before));
        return Ok(());
    };
    let r = shell(api, session, "DELETE", &format!("/api/f/{old}"), None, &[])?;
    b.reload(page)?;
    let add = "[...document.querySelectorAll('#settings-skills button')].find((b) => b.textContent === 'Add the managed skills')";
    let offered = b.until(page, &format!("!!{add}"), wait);
    let none = own_skills(api, session)?;
    s.ok("with it deleted, their settings offer the managed skills, and loading made none", r.status == 200 && offered && none.is_empty(), json!({ "deleted": r.status, "skills": none }));
    b.eval(page, &format!("({add}.click(), true)"))?;
    let made = s.eventually(wait, || own_skills(api, session).is_ok_and(|l| l.len() == 1));
    let now = own_skills(api, session)?;
    let name = now.first().cloned().unwrap_or_default();
    // listed as it is made; its template's first commit lands a moment later
    let manifest_of = || shell(api, session, "GET", &format!("/api/f/{name}/manifest"), None, &[]);
    s.eventually(wait, || manifest_of().is_ok_and(|m| m.body["template"] == "skills"));
    let manifest = manifest_of()?;
    let listed = b.until(page, &format!("document.getElementById('settings-skills')?.dataset.fragment === {}", js(&name)), wait);
    s.ok(
        "the button makes them one from the blessed template, and their settings list it",
        made && manifest.body["template"] == "skills" && listed,
        json!({ "skills": now, "manifest": manifest.body, "settings": b.eval(page, "document.getElementById('settings-skills')?.innerText.slice(0, 200)")? }),
    );
    b.reload(page)?;
    b.until(page, "!!document.getElementById('settings-skills')", wait);
    let again = own_skills(api, session)?;
    s.ok("loaded again, there is no second", again == now, json!(again));
    Ok(())
}

/// The skills in settings (decision 17): the shell made the person's skills
/// fragment at setup, and its Skills section lists exactly that fragment's
/// managed set by category, then each agent's own skills (its fragment's
/// `skills/`), as the agents' computers read them.
fn skills_ui(s: &mut Suite, api: &Api, b: &mut Browser, page: &Page, session: &str) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let list = shell(api, session, "GET", "/api/fragments", None, &[])?;
    let mine = |kind: &str| list.body["fragments"].as_array().and_then(|l| l.iter().find(|f| f["kind"] == kind && f["role"] == "owner")).and_then(|f| f["name"].as_str()).map(str::to_string);
    let (Some(skills), Some(agent)) = (mine("skills"), mine("agent")) else {
        s.ok("the shell made the person's skills fragment at setup, beside their agent", false, &list);
        return Ok(());
    };
    s.ok("the shell made the person's skills fragment at setup, beside their agent", true, &skills);
    // what the fragment's files say: skills/<category>/<name>/SKILL.md, or skills/<name>/SKILL.md
    let skills_of = |paths: &[String]| -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = paths
            .iter()
            .filter_map(|p| {
                let parts: Vec<&str> = p.split('/').collect();
                match parts.as_slice() {
                    ["skills", c, n, "SKILL.md"] => Some((c.to_string(), n.to_string())),
                    ["skills", n, "SKILL.md"] => Some(("general".to_string(), n.to_string())),
                    _ => None,
                }
            })
            .collect();
        out.sort();
        out
    };
    let paths = |name: &str| -> Result<Vec<String>> {
        let r = shell(api, session, "GET", &format!("/api/f/{name}/files"), None, &[])?;
        Ok(r.body["files"].as_array().map(|l| l.iter().filter_map(|f| f["path"].as_str().map(str::to_string)).collect()).unwrap_or_default())
    };
    let want = skills_of(&paths(&skills)?);
    let shown = "JSON.stringify([...document.querySelectorAll('#settings-skills [data-category] [data-skill]')].map((e) => [e.closest('[data-category]').dataset.category, e.dataset.skill]).sort())";
    let listed = b.until(page, &format!("{shown} !== '[]'"), wait);
    let got: Vec<(String, String)> = serde_json::from_str(b.eval(page, shown)?.as_str().unwrap_or("[]")).unwrap_or_default();
    s.ok(
        &format!("settings' Skills lists the managed set by category ({} skills), exactly the skills fragment's files", want.len()),
        listed && want.len() == 12 && got == want && got.iter().any(|(c, n)| c == "software-development" && n == "apps-finite"),
        json!({ "shown": got.len(), "files": want.len(), "missing": want.iter().filter(|w| !got.contains(w)).collect::<Vec<_>>(), "extra": got.iter().filter(|g| !want.contains(g)).collect::<Vec<_>>() }),
    );
    // an agent's own skill, from its fragment, shows beside it
    let own = "---\nname: garden-notes\ndescription: What this garden needs.\n---\n";
    let w = shell(api, session, "POST", &format!("/api/f/{agent}/files"), Some(&json!({ "files": [{ "path": "skills/garden-notes/SKILL.md", "text": own }], "key": "own-skill" })), &[])?;
    b.reload(page)?;
    let beside = format!("!!document.querySelector('#settings-skills [data-agent={}] [data-skill=\"garden-notes\"]')", js(&agent));
    let shows = b.until(page, &beside, wait);
    let own_files = skills_of(&paths(&agent)?);
    s.ok(
        "and an agent's own skill shows beside it, as its fragment's files say",
        w.status == 200 && shows && own_files.contains(&("general".to_string(), "garden-notes".to_string())),
        json!({ "write": w.status, "own": own_files }),
    );
    Ok(())
}

/// The shell's person in its browser lane: their platform session, and
/// their first agent's label (its fragment's, as the sidebar holds it).
struct Person<'a> {
    session: &'a str,
    first: &'a str,
    /// Its title, as the shell shows it ("Novatron DX").
    first_title: &'a str,
}

/// A JS string literal.
fn js(s: &str) -> String {
    serde_json::to_string(s).expect("a string encodes")
}

/// Whether `agent` speaks first in `chat` within `wait`: a reply of the
/// turn the shell's routine on its `tasks` starts (the stub echoes it as
/// `[your routine]`), and no one else's record there. With what the chat
/// holds, for the check's note.
fn said_hello(s: &Suite, api: &Api, session: &str, chat: &str, agent: &str, wait: std::time::Duration) -> (bool, Value) {
    let mut seen = Vec::new();
    let hello = s.eventually(wait, || {
        seen = shell(api, session, "GET", &format!("/api/f/{chat}/channels/chat?after=0&limit=1000"), None, &[])
            .ok()
            .and_then(|r| r.body["records"].as_array().cloned())
            .unwrap_or_default();
        !seen.is_empty() && seen.iter().all(|r| r["principal"] == agent) && seen.iter().any(|r| r["body"]["text"].as_str().is_some_and(|t| t.starts_with("echo: [your routine] ")))
    });
    (hello, seen.iter().map(|r| json!({ "principal": r["principal"], "body": r["body"] })).collect())
}

/// What their agent is doing, from its chats' rows in the list (docs/api.md,
/// "Listed records"; cell/shell/shell.js `doingChats`). Goal: the agent's
/// row and its menu say it live, with no reload and no read of `work`, and
/// say nothing once it is done. Method: the stub's agent in the first chat,
/// asked for a card (`approve`) and then a question in words (`ask-me`),
/// each answered as the person, the row watched in Chrome throughout.
fn doing_ui(s: &mut Suite, api: &Api, b: &mut Browser, page: &Page, session: &str, chat: &str, shots: &std::path::Path) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let agent_wait = std::time::Duration::from_secs(120);
    let doing = "(document.querySelector('#sidebar-agent .agent-doing')?.textContent ?? null)";
    let say = |id: &str, body: Value| shell(api, session, "POST", &format!("/api/f/{chat}/channels/chat"), Some(&json!({ "id": id, "body": body })), &[]);
    let quiet = b.until(page, &format!("{doing} === null"), agent_wait);
    anyhow::ensure!(quiet, "the agent's row says it is doing something before it is asked anything: {}", b.eval(page, doing)?);

    // a card waits for the person: its owner answers it
    let r = say("doing-card", json!({ "text": "approve this, please" }))?;
    anyhow::ensure!(r.status == 200, "asking for a card: {r}");
    let waiting = b.until(page, &format!("{doing} === 'Waiting for you'"), agent_wait);
    let _ = b.screenshot(page, &shots.join("desktop-agent-waiting.png"));
    let label = b.eval(page, "document.getElementById('sidebar-agent')?.getAttribute('aria-label') ?? null")?;
    b.click(page, "#sidebar-agent")?;
    let menu = format!("[...document.querySelectorAll('#menu button')].some((b) => b.textContent === {})", js(&format!("Waiting for you in {FIRST_CHAT}")));
    let listed = b.until(page, &menu, wait);
    b.press(page, "Escape", false)?;
    let work = shell(api, session, "GET", &format!("/api/f/{chat}/channels/work?after=0&limit=1000"), None, &[])?;
    let card = work.body["records"].as_array().into_iter().flatten().rev().find(|r| r["body"]["kind"] == "turn.prompt").cloned().unwrap_or_default();
    let prompt = card["body"]["prompt"].as_str().unwrap_or("").to_string();
    let answer = json!({ "kind": "prompt_response", "prompt": prompt, "option": "once", "agent": card["principal"], "turn": card["body"]["turn"] });
    let r = say(&format!("pr:{prompt}"), answer)?;
    let cleared = r.status == 200 && b.until(page, &format!("{doing} === null"), agent_wait);
    s.ok(
        "while its card waits, their agent's row says \"Waiting for you\" with no reload (and says so to a screen reader), its menu names the chat, and once it is answered the row says nothing",
        waiting && label.as_str().is_some_and(|l| l.contains("waiting for you")) && listed && cleared,
        json!({ "waiting": waiting, "label": label, "menu": listed, "prompt": prompt, "cleared": cleared, "row": b.eval(page, doing)? }),
    );

    // a question in words waits on whoever asked it: the person
    let r = say("doing-ask", json!({ "text": "ask-me a name" }))?;
    anyhow::ensure!(r.status == 200, "asking for a question: {r}");
    let asked = b.until(page, &format!("{doing} === 'Waiting for you'"), agent_wait);
    let r = say("doing-told", json!({ "text": "Basil" }))?;
    let told = r.status == 200 && b.until(page, &format!("{doing} === null"), agent_wait);
    s.ok(
        "a question it asks the person says \"Waiting for you\" too, until their next message answers it",
        asked && told,
        json!({ "asked": asked, "told": told, "row": b.eval(page, doing)? }),
    );
    Ok(())
}

/// The replies on a chat's `chat` from `agent` whose text holds `said`.
fn replies(api: &Api, session: &str, chat: &str, agent: &str, said: &str) -> usize {
    let r = shell(api, session, "GET", &format!("/api/f/{chat}/channels/chat?after=0&limit=1000"), None, &[]);
    r.ok()
        .and_then(|r| r.body["records"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter(|x| x["principal"] == agent && x["body"]["text"].as_str().is_some_and(|t| t.starts_with("echo:") && t.contains(said)))
        .count()
}

/// Whether a frame of the shell's landed on `agent`'s screen: the
/// computer's screen port at `?agent=<agent>`, its page there (the stub's,
/// which shows no screen: what it reads of the query is the image's).
fn screen_landed(s: &mut Suite, b: &mut Browser, page: &Page, agent: &str) -> bool {
    let landed = format!("/p/6080/?agent={agent}");
    s.eventually(std::time::Duration::from_secs(30), || b.eval_in_frame(page, &landed, "location.search").ok() == Some(json!(format!("?agent={agent}"))))
}

/// Clicks the open menu's item named `text` (a script's click: a menu's
/// button needs no gesture). Answers whether there was one.
fn menu_item(b: &mut Browser, page: &Page, text: &str) -> Result<bool> {
    let clicked = b.eval(page, &format!("(() => {{ const i = [...document.querySelectorAll('#menu button')].find((b) => b.textContent === {}); i?.click(); return !!i; }})()", js(text)))?;
    Ok(clicked == true)
}

/// A group chat of the person's two agents, made through the API (the
/// first added leads; the shell makes none: one agent) is a chat like any
/// other: listed, its agents' colours stacked where search shows it; each
/// answers when @mentioned, and the lead when no one is; its ⋯ offers each
/// one's screen. Then search: a message's words find it, and clicking it
/// opens its chat. Then archiving from the chat's ⋯: the chat leaves the
/// list, search still finds it, and Unarchive brings it back.
fn groups_ui(s: &mut Suite, api: &Api, b: &mut Browser, page: &Page, me: &Person, shots: &std::path::Path) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let agent_wait = std::time::Duration::from_secs(120);
    let r = shell(api, me.session, "GET", "/api/computers", None, &[])?;
    let agents = r.body["computers"][0]["agents"].as_array().cloned().unwrap_or_default();
    let id_of = |label: &str| agents.iter().find(|a| a["fragment"].as_str().map(label_of) == Some(label)).and_then(|a| a["identity"].as_str()).unwrap_or("").to_string();
    let first_label = me.first;
    let (lead, other) = (id_of("reader"), id_of(first_label));
    anyhow::ensure!(fragment_core::npub::is_identity(&lead) && fragment_core::npub::is_identity(&other), "the two agents' identities: {r}");

    // made as the CLI or a page's add makes one: a chat, its agents added in order
    let made = shell(api, me.session, "POST", "/api/fragments", Some(&json!({ "label": "group", "template": "chat", "title": format!("Reader, {}", me.first_title) })), &[])?;
    anyhow::ensure!(made.status == 200, "the group: {made}");
    let group = made.body["name"].as_str().unwrap_or("").to_string();
    for id in [&lead, &other] {
        let r = shell(api, me.session, "PUT", &format!("/api/f/{group}/members/{id}"), Some(&json!({ "role": "editor" })), &[])?;
        anyhow::ensure!(r.status == 200, "a member of the group: {r}");
    }
    let row = format!("#chats [data-key={}]", js(&format!("chat:{group}")));
    let listed = b.until(page, &format!("document.querySelector({})?.textContent === {}", js(&row), js(&format!("Reader, {}", me.first_title))), wait);
    s.ok("a group chat made elsewhere lists among their chats, by its title", listed, b.eval(page, "document.getElementById('chats').innerText")?);
    let r = shell(api, me.session, "GET", &format!("/api/f/{group}/members"), None, &[])?;
    let members: Vec<&str> = r.body["members"].as_array().into_iter().flatten().filter(|m| m["kind"] == "agent").filter_map(|m| m["principal"].as_str()).collect();
    s.ok("its members are the two agents, the lead added first", members == [lead.as_str(), other.as_str()], &r);
    b.click(page, &row)?;
    b.until(page, &format!("document.getElementById('chat-title').textContent === {}", js(&format!("Reader, {}", me.first_title))), wait);

    // each answers its @mention; with none, the lead answers
    let say = |id: &str, text: &str| shell(api, me.session, "POST", &format!("/api/f/{group}/channels/chat"), Some(&json!({ "id": id, "body": { "text": text } })), &[]);
    let to_other = format!("@{first_label} are you there");
    say("g1", &to_other)?;
    let answered = s.eventually(agent_wait, || replies(api, me.session, &group, &other, &to_other) == 1);
    s.ok(&format!("@{first_label} answers its @mention in the group"), answered, "");
    say("g2", "@reader and you")?;
    let answered = s.eventually(agent_wait, || replies(api, me.session, &group, &lead, "@reader and you") == 1);
    s.ok("and @reader its own", answered, "");
    say("g3", "hello both of you")?;
    let answered = s.eventually(agent_wait, || replies(api, me.session, &group, &lead, "hello both of you") == 1);
    s.ok("a message that @mentions no one, the lead answers", answered && replies(api, me.session, &group, &other, "hello both of you") == 0, "");
    let host = group.clone();
    let shown = s.eventually(wait, || {
        b.eval_in_frame(page, &host, "document.body.innerText").ok().and_then(|v| v.as_str().map(|t| t.matches("echo:").count() >= 3 && t.contains("are you there") && t.contains("and you"))).unwrap_or(false)
    });
    s.ok("the group's chat, framed in the shell, shows both agents' answers", shown, &host);
    let _ = b.screenshot(page, &shots.join("desktop-group.png"));

    // each agent's own screen (its own desktop): from the group's ⋯ the
    // person picks whose, and its frame lands on the computer's screen port
    // at `?agent=<that agent>`, the image's to read (the platform carries it)
    let fragment_of = |label: &str| agents.iter().find_map(|a| a["fragment"].as_str().filter(|f| label_of(f) == label)).unwrap_or("").to_string();
    let (reader, first) = (fragment_of("reader"), fragment_of(first_label));
    b.click(page, "#chat-options")?;
    let items = b.eval(page, "[...document.querySelectorAll('#menu button')].map((b) => b.textContent)")?;
    let mine = format!("{}'s screen", me.first_title);
    let offered = items.as_array().is_some_and(|l| l.iter().any(|t| t == "Reader's screen") && l.iter().any(|t| t.as_str() == Some(mine.as_str())) && !l.iter().any(|t| t == "New chat"));
    s.ok("a group chat's ⋯ offers each of its agents' screens (and no New chat: new chats are with their one agent)", offered, &items);
    let picked = menu_item(b, page, "Reader's screen")?;
    let landed = picked && screen_landed(s, b, page, &reader);
    let frames = b.eval(page, "[...document.querySelectorAll('#stack iframe')].map((f) => f.dataset.src)")?;
    s.ok("picking one opens that agent's screen: its frame lands on the computer's screen port at ?agent=<that agent>", landed, &frames);

    // search: a message's words, and clicking one opens its chat; a group's agents stacked
    let first_chat = own_named(api, me.session, &format!("{first_label}-chat"))?;
    b.click(page, "#sidebar-new-chat")?;
    b.eval(page, &fill("#workspace-search", "water garden"))?;
    let hit = format!("#search-results .search-message[data-fragment={}]", js(&first_chat));
    let found = b.until(page, &format!("document.querySelector({})", js(&hit)), wait);
    s.ok("searching a message's words lists it under Messages, in its chat", found, b.eval(page, "document.getElementById('search-results').innerText")?);
    let _ = b.screenshot(page, &shots.join("desktop-search.png"));
    b.click(page, &hit)?;
    let opened = b.until(page, &format!("!document.getElementById('search-dialog').open && document.getElementById('chat-title').textContent === {}", js(FIRST_CHAT)), wait);
    s.ok("clicking it opens its chat", opened, b.eval(page, "document.getElementById('chat-title').textContent")?);
    b.click(page, "#sidebar-new-chat")?;
    b.eval(page, &fill("#workspace-search", "Reader"))?;
    let stacked = format!("#search-results .search-result:not(.search-message)[data-fragment={}] .avatar-stack .agent-avatar", js(&group));
    let colors = b.until(page, &format!("document.querySelectorAll({}).length === 2", js(&stacked)), wait);
    let found_row = b.eval(page, &format!("document.querySelector({})?.outerHTML ?? null", js(&format!("#search-results .search-result[data-fragment={}]", js(&group)))))?;
    let listed_row = shell(api, me.session, "GET", "/api/fragments", None, &[])?.body["fragments"].as_array().and_then(|l| l.iter().find(|f| f["name"] == group.as_str()).cloned()).unwrap_or(Value::Null);
    s.ok("found by its title, a group shows its agents' colours stacked, the lead's first", colors, json!({ "row": found_row, "listed": listed_row }));
    b.eval(page, "(document.getElementById('search-dialog').dispatchEvent(new Event('cancel')), true)")?;
    b.until(page, "!document.getElementById('search-dialog').open", wait);
    // a direct chat's ⋯ opens its own agent's screen
    b.click(page, "#chat-options")?;
    let picked = menu_item(b, page, "Its screen")?;
    let landed = picked && screen_landed(s, b, page, &first);
    let frames = b.eval(page, "[...document.querySelectorAll('#stack iframe')].map((f) => f.dataset.src)")?;
    s.ok("a direct chat's ⋯ opens its own agent's screen (Its screen: ?agent=<its agent>)", landed, &frames);

    // archiving, from the chat's ⋯: the person's own view
    let row_of = |name: &str| format!("document.querySelector({})", js(&format!("#chats [data-key={}]", js(&format!("chat:{name}")))));
    b.click(page, "#chat-options")?;
    let archived = menu_item(b, page, "Archive")?;
    let left = b.until(page, &format!("!{} && {} && document.getElementById('chat-title').textContent !== {}", row_of(&first_chat), row_of(&group), js(FIRST_CHAT)), wait);
    s.ok("Archive, from its ⋯: the chat leaves the list, and another opens", archived && left, b.eval(page, "document.getElementById('chats').innerText")?);
    b.click(page, "#sidebar-new-chat")?;
    b.eval(page, &fill("#workspace-search", "water garden"))?;
    let rows = js(&format!("#search-results [data-fragment={}]", js(&first_chat)));
    let marked = b.until(page, &format!("[...document.querySelectorAll({rows})].some((r) => r.querySelector('.meta')?.textContent === 'Archived')"), wait);
    s.ok("search still finds it, marked archived", marked, b.eval(page, "document.getElementById('search-results').innerText")?);
    b.click(page, &hit)?;
    // the launcher gone (it fades out over the page), its chat open
    b.until(page, &format!("!document.getElementById('search-dialog').open && document.getElementById('chat-title').textContent === {}", js(FIRST_CHAT)), wait);
    b.click(page, "#chat-options")?;
    let unarchived = menu_item(b, page, "Unarchive")?;
    let back = b.until(page, &row_of(&first_chat), wait);
    s.ok("Unarchive, from its ⋯, brings it back", unarchived && back, "");

    // the phone's picture below is of the Reader's chat
    b.click(page, &format!("#chats [data-key={}]", js(&format!("chat:{}", own_named(api, me.session, "reader-chat")?))))?;
    Ok(())
}

/// A chat's `unread` in the person's list (0 when its row leaves it out).
fn unread_of(api: &Api, session: &str, chat: &str) -> i64 {
    let r = shell(api, session, "GET", "/api/fragments", None, &[]);
    r.ok().and_then(|r| r.body["fragments"].as_array().and_then(|l| l.iter().find(|f| f["name"] == chat).map(|f| f["unread"].as_i64().unwrap_or(0)))).unwrap_or(-1)
}

/// The fragment of the chat the shell shows (its frame not hidden).
fn open_chat(b: &mut Browser, page: &Page) -> String {
    b.eval(page, "[...document.querySelectorAll('#frames iframe')].find((f) => !f.hidden)?.dataset.fragment ?? ''").ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

/// Types `text` into the open chat's composer, in its frame (`host`), and
/// sends it, once its composer is ready. Whether it was sent.
fn say_in(s: &Suite, b: &mut Browser, page: &Page, host: &str, text: &str) -> bool {
    let typed = format!(
        "(() => {{ const t = document.getElementById('text'); if (!t || t.disabled || document.getElementById('say')?.dataset.ready !== '1') return false; t.value = {}; t.dispatchEvent(new Event('input', {{ bubbles: true }})); document.getElementById('say').requestSubmit(); return true; }})()",
        js(text)
    );
    s.eventually(std::time::Duration::from_secs(30), || b.eval_in_frame(page, host, &typed).ok() == Some(Value::Bool(true)))
}

/// Unread (docs/api.md, The shell): a chat not open counts on its row what
/// came since its person saw it, as their list does; opening it marks it
/// seen, here and in their list; and the open chat counts nothing as its
/// messages come. The first agent's chat, while the Reader's is open.
fn unread_ui(s: &mut Suite, api: &Api, b: &mut Browser, page: &Page, me: &Person, reader_chat: &str, shots: &std::path::Path) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let agent_wait = std::time::Duration::from_secs(120);
    let first = own_named(api, me.session, &format!("{}-chat", me.first))?;
    let ids = agent_ids(api, me.session, &[me.first])?;
    let row = format!("document.querySelector({})", js(&format!("#chats [data-key={}]", js(&format!("chat:{first}")))));
    let open = open_chat(b, page);
    let before = unread_of(api, me.session, &first);
    // a message to it from elsewhere (as the CLI or another device sends one), and its answer
    let said = "count these for me please";
    let r = shell(api, me.session, "POST", &format!("/api/f/{first}/channels/chat"), Some(&json!({ "id": "unread-ui-1", "body": { "text": said } })), &[])?;
    let answered = s.eventually(agent_wait, || replies(api, me.session, &first, &ids[0], said) == 1);
    let want = before + 2;
    let counted = s.eventually(wait, || unread_of(api, me.session, &first) == want);
    let badge = b.until(page, &format!("{row}?.querySelector('.unread')?.textContent === {}", js(&want.to_string())), wait);
    let _ = b.screenshot(page, &shots.join("desktop-unread.png"));
    s.ok(
        "a chat not open counts what came since its person saw it (their own message from elsewhere, and the answer): in their list, and as a badge on its row",
        r.status == 200 && open == reader_chat && answered && counted && badge,
        json!({ "before": before, "now": unread_of(api, me.session, &first), "badge": b.eval(page, &format!("{row}?.querySelector('.unread')?.textContent ?? null"))? }),
    );
    // opened: seen, here and in their list
    b.click(page, &format!("#chats [data-key={}]", js(&format!("chat:{first}"))))?;
    let gone = b.until(page, &format!("{row} && !{row}.querySelector('.unread')"), wait);
    let cleared = s.eventually(wait, || unread_of(api, me.session, &first) == 0);
    s.ok("opening it marks it seen: its badge goes, and their list counts nothing on it", gone && cleared, json!({ "unread": unread_of(api, me.session, &first) }));
    // open, its messages come and count nothing
    let more = "and one more while I look";
    let sent = say_in(s, b, page, &first, more);
    let answered = s.eventually(agent_wait, || replies(api, me.session, &first, &ids[0], more) == 1);
    let shown = s.eventually(wait, || b.eval_in_frame(page, &first, "document.body.innerText").ok().and_then(|v| v.as_str().map(|t| t.matches(more).count() >= 2)).unwrap_or(false));
    let none = s.eventually(wait, || unread_of(api, me.session, &first) == 0) && b.eval(page, &format!("!{row}.querySelector('.unread')"))? == true;
    s.ok("the chat open, on screen, counts nothing as its messages come (marked seen again)", sent && answered && shown && none, json!({ "unread": unread_of(api, me.session, &first) }));
    b.click(page, &format!("#chats [data-key={}]", js(&format!("chat:{reader_chat}"))))?;
    b.until(page, &format!("document.getElementById('chat-title').textContent === 'Reader' && {}", js(reader_chat) + " === [...document.querySelectorAll('#frames iframe')].find((f) => !f.hidden)?.dataset.fragment"), wait);
    Ok(())
}

/// A new chat (docs/api.md, The shell, the launcher): from the Reader's
/// chat, the launcher's Enter makes a chat with the person's one agent
/// (not the open chat's), labelled beside its own (`<first>-chat-2`), the
/// words posted whole as its first message: one record though the first
/// answer was lost and it was sent again (its id, kept); its first message
/// titles it (48 characters at most) and the agent answers there. Nothing
/// typed starts nothing. New chat, from the agent's menu, opens an empty
/// chat with it; asked again before anything is said there, that one, not
/// another. The agent's own chat keeps its label (`fragment ask`, after,
/// finds it).
fn new_chat_ui(s: &mut Suite, api: &Api, b: &mut Browser, page: &Page, me: &Person, reader_chat: &str, shots: &std::path::Path) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let agent_wait = std::time::Duration::from_secs(120);
    let first_id = agent_ids(api, me.session, &[me.first])?.remove(0);
    let chats_with = |prefix: &str| -> Vec<String> {
        shell(api, me.session, "GET", "/api/fragments", None, &[])
            .map(|l| l.body["fragments"].as_array().into_iter().flatten().filter_map(|f| f["name"].as_str().map(label_of)).filter(|l| l.starts_with(prefix)).map(str::to_string).collect())
            .unwrap_or_default()
    };
    let own_chat = format!("{}-chat", me.first);
    let before = chats_with(&own_chat);
    b.click(page, "#sidebar-new-chat")?;
    let open = b.until(page, "document.getElementById('search-dialog').open && document.activeElement === document.getElementById('workspace-search')", wait);
    // nothing typed: Enter starts nothing
    b.eval(page, "(document.getElementById('search-form').requestSubmit(), true)")?;
    std::thread::sleep(std::time::Duration::from_secs(1));
    let nothing = chats_with(&own_chat) == before && b.eval(page, "document.getElementById('search-dialog').open")? == true;
    s.ok("the launcher opens on its input, and with nothing typed, Enter starts nothing", open && nothing, json!({ "before": before, "now": chats_with(&own_chat) }));
    // words: the first row starts a chat with their agent
    let said = "Plan my reading for October: three novels and one history of maps";
    b.eval(page, &fill("#workspace-search", said))?;
    let offered = b.until(page, &format!("document.querySelector('#search-results .search-start')?.textContent.includes({})", js(&format!("Start a new chat with {}", me.first_title))), wait);
    let _ = b.screenshot(page, &shots.join("desktop-launcher.png"));
    // its first answer lost: the platform has the record, the page is told it failed
    b.eval(
        page,
        "(() => { const real = window.fetch; window.fetch = async (url, o) => { if (/\\/channels\\/chat$/.test(url) && o?.method === 'POST' && !window.__lost) { window.__lost = true; await real(url, o); throw new TypeError('the answer was lost'); } return real(url, o); }; return true; })()",
    )?;
    b.eval(page, "(document.getElementById('search-form').requestSubmit(), true)")?;
    let lost = b.until(page, "window.__lost === true && document.getElementById('search-status').textContent.includes(\"wasn't sent\") && document.getElementById('search-dialog').open", wait);
    let status = b.eval(page, "document.getElementById('search-status').textContent")?;
    // sent again: the same chat, the same record
    b.eval(page, "(document.getElementById('search-form').requestSubmit(), true)")?;
    let opened = b.until(page, "!document.getElementById('search-dialog').open", wait);
    let fresh = own_named(api, me.session, &format!("{own_chat}-2"))?;
    let shows = open_chat(b, page);
    let records = shell(api, me.session, "GET", &format!("/api/f/{fresh}/channels/chat?after=0&limit=100"), None, &[])?;
    let sent = records.body["records"].as_array().into_iter().flatten().filter(|r| r["body"]["text"] == said).count();
    let members = shell(api, me.session, "GET", &format!("/api/f/{fresh}/members"), None, &[])?;
    let agents: Vec<(Value, Value)> = members.body["members"].as_array().into_iter().flatten().filter(|m| m["kind"] == "agent").map(|m| (m["principal"].clone(), m["role"].clone())).collect();
    s.ok(
        "from the Reader's chat, Enter makes a chat with their agent (not the open chat's), labelled beside its own, and posts the words once, though the first answer was lost and it was sent again",
        offered && lost && opened && shows == fresh && agents == vec![(json!(first_id), json!("editor"))] && sent == 1,
        json!({ "status": status, "shows": shows, "fresh": fresh, "agents": agents, "sent": sent }),
    );
    let title: String = format!("{}…", said.chars().take(47).collect::<String>().trim_end());
    let answered = s.eventually(agent_wait, || replies(api, me.session, &fresh, &first_id, said) == 1);
    let titled = b.until(page, &format!("document.getElementById('chat-title').textContent === {} && document.querySelector({})?.querySelector('.label')?.textContent === {}", js(&title), js(&format!("#chats [data-key={}]", js(&format!("chat:{fresh}")))), js(&title)), wait);
    let manifest = shell(api, me.session, "GET", &format!("/api/f/{fresh}/manifest"), None, &[])?;
    let first_row = b.eval(page, "document.querySelector('#chats .chat-row')?.dataset.key ?? null")?;
    let _ = b.screenshot(page, &shots.join("desktop-new-chat-titled.png"));
    s.ok(
        "its first message titles it (48 characters at most), the agent answers there, and it heads their chats, the newest",
        answered && titled && manifest.body["meta"]["title"] == title.as_str() && first_row == format!("chat:{fresh}").as_str(),
        json!({ "title": manifest.body["meta"]["title"], "want": title, "first": first_row }),
    );
    // New chat, from their agent's menu: an empty chat with it; asked again before anything is said, that one
    b.click(page, "#sidebar-agent")?;
    let picked = menu_item(b, page, "New chat")?;
    let blank = b.until(page, "document.getElementById('chat-title').textContent === 'New chat'", wait);
    let third = own_named(api, me.session, &format!("{own_chat}-3"))?;
    let empty = s.eventually(wait, || b.eval_in_frame(page, &third, "!!document.querySelector('#messages .empty h2') && !document.querySelector('#messages .msg')").ok() == Some(Value::Bool(true)));
    let _ = b.screenshot(page, &shots.join("desktop-new-chat.png"));
    b.click(page, &format!("#chats [data-key={}]", js(&format!("chat:{reader_chat}"))))?;
    b.until(page, "document.getElementById('chat-title').textContent === 'Reader'", wait);
    b.click(page, "#sidebar-agent")?;
    let again = menu_item(b, page, "New chat")? && b.until(page, "document.getElementById('chat-title').textContent === 'New chat'", wait);
    let made = chats_with(&own_chat);
    s.ok(
        "New chat, from their agent's menu, opens an empty chat with it; asked again before anything is said there, it opens that one, not another",
        picked && blank && empty && again && open_chat(b, page) == third && made.len() == before.len() + 2,
        json!({ "chats": made }),
    );
    let own = own_named(api, me.session, &own_chat)?;
    s.ok("the agent's own chat keeps its label, apart from the new ones", label_of(&own) == own_chat && label_of(&fresh) == format!("{own_chat}-2"), json!([own, fresh]));
    b.click(page, &format!("#chats [data-key={}]", js(&format!("chat:{reader_chat}"))))?;
    b.until(page, "document.getElementById('chat-title').textContent === 'Reader'", wait);
    Ok(())
}

/// An app beside the chat (docs/api.md, The shell; docs/chat-records.md,
/// "The page"): a link in an agent's reply to another fragment's page gets
/// "Open", which opens that page in the viewer beside the chat, at its
/// path, signed in as its row's window is; a link to anything else gets
/// none, and a page that asks the shell to open a URL that is no
/// fragment's opens nothing.
fn open_beside_ui(s: &mut Suite, api: &Api, b: &mut Browser, page: &Page, me: &Person, app: &str, shots: &std::path::Path) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let agent_wait = std::time::Duration::from_secs(120);
    let host = open_chat(b, page);
    let ids = agent_ids(api, me.session, &["reader"])?;
    let pane = format!("document.querySelector({})", js(&format!("#stack .pane[data-key={}]", js(&format!("app:{app}")))));
    // its window closed first, so Open is what opens it
    b.eval(page, &format!("({pane}?.querySelector('button[aria-label=Close]')?.click(), true)"))?;
    let closed = b.until(page, &format!("!{pane}"), wait);
    let link = api.site_url(app, "?from=chat");
    let said = format!("Here it is: {link}, or [open the app]({link}) and the platform: {}/settings", api.base);
    let sent = say_in(s, b, page, &host, &said);
    let answered = s.eventually(agent_wait, || replies(api, me.session, &host, &ids[0], &link) == 1);
    let offers = format!(
        "(() => {{ const m = [...document.querySelectorAll('.msg.agent')].filter((m) => m.innerText.includes({l})).pop(); if (!m) return null; \
         return [...m.querySelectorAll('.md a')].filter((a) => ['http:', 'https:'].includes(a.protocol)).map((a) => ({{ href: a.href, open: a.nextElementSibling?.classList.contains('open-beside') ?? false }})); }})()",
        l = js(&link)
    );
    let mut links = Value::Null;
    let _ = s.eventually(wait, || {
        links = b.eval_in_frame(page, &host, &offers).unwrap_or(Value::Null);
        links.as_array().is_some_and(|l| l.len() == 3)
    });
    let want = json!([{ "href": link, "open": true }, { "href": link, "open": true }, { "href": format!("{}/settings", api.base), "open": false }]);
    s.ok("autolinks and markdown links in an agent's reply are offered Open for fragment pages; the platform is not", closed && sent && answered && links == want, json!({ "links": links, "want": want }));
    let _ = b.screenshot(page, &shots.join("desktop-open-offered.png"));
    let clicked = b.eval_in_frame(
        page,
        &host,
        &format!("(() => {{ const m = [...document.querySelectorAll('.msg.agent')].filter((m) => m.innerText.includes({})).pop(); const o = m?.querySelector('.open-beside'); o?.click(); return !!o; }})()", js(&link)),
    )?;
    let src = js(&format!("/auth/frame?name={app}&return=%2F%3Ffrom%3Dchat"));
    let opened = b.until(page, &format!("{pane}?.querySelector('iframe')?.dataset.src === {src} && document.getElementById('layout').classList.contains('right-open')"), wait);
    let landed = s.eventually(wait, || b.eval_in_frame(page, app, "location.search").ok() == Some(json!("?from=chat")));
    let _ = b.screenshot(page, &shots.join("desktop-open-beside.png"));
    s.ok(
        "Open opens that page beside the chat, in the viewer, at its path, signed in by the mint",
        clicked == true && opened && landed,
        b.eval(page, "[...document.querySelectorAll('#stack .pane')].map((p) => [p.dataset.key, p.querySelector('iframe')?.dataset.src])")?,
    );
    // a page that names no fragment's URL opens nothing
    let panes = "document.querySelectorAll('#stack .pane').length";
    let count = b.eval(page, panes)?;
    for url in ["https://example.com/", &format!("{}/settings", api.base), &api.site_url("not-a-name", "")] {
        b.eval_in_frame(page, &host, &format!("(parent.postMessage({{ fragment: 'open', url: {} }}, '*'), true)", js(url)))?;
    }
    std::thread::sleep(std::time::Duration::from_secs(1));
    let after = b.eval(page, panes)?;
    s.ok("asked to open a URL that is no fragment's page (another site, the platform, a host that is no name), the shell opens nothing", after == count, json!([count, after]));
    Ok(())
}

/// Another agent of the person's, made through the public API as the CLI
/// or an agent makes one (the shell makes only the first): its fragment,
/// its job, assigned to their computer, its own chat with it in it, and
/// its hello. Its fragment, its chat and its identity.
fn extra_agent(api: &Api, session: &str, label: &str, title: &str, job: &str) -> Result<(String, String, String)> {
    let call = |method: &'static str, path: &str, body: Value| shell(api, session, method, path, Some(&body), &[]);
    let name = |r: &Reply| r.body["name"].as_str().unwrap_or("").to_string();
    let made = call("POST", "/api/fragments", json!({ "label": label, "template": "agent", "title": title }))?;
    anyhow::ensure!(made.status == 200, "the agent: {made}");
    let agent = name(&made);
    let computers = shell(api, session, "GET", "/api/computers", None, &[])?;
    let computer = computers.body["computers"][0]["computer"].as_str().unwrap_or("").to_string();
    let assigned = call("PUT", &format!("/api/computers/{computer}/agents/{agent}"), json!({}))?;
    let id = assigned.body["agents"].as_array().into_iter().flatten().find(|a| a["fragment"] == agent.as_str()).and_then(|a| a["identity"].as_str()).unwrap_or("").to_string();
    anyhow::ensure!(fragment_core::npub::is_identity(&id), "its computer took it: {assigned}");
    let files = json!([{ "path": "SOUL.md", "text": format!("{job}\n") }, { "path": "agent.json", "text": "{\n  \"tier\": \"cheap\"\n}\n" }]);
    let wrote = call("POST", &format!("/api/f/{agent}/files"), json!({ "key": "agent-job", "message": "its job", "files": files }))?;
    let deployed = call("POST", &format!("/api/f/{agent}/deploy"), json!({}))?;
    anyhow::ensure!(wrote.status == 200 && deployed.status == 200, "its job: {wrote} {deployed}");
    let made = call("POST", "/api/fragments", json!({ "label": format!("{label}-chat"), "template": "chat", "title": title }))?;
    anyhow::ensure!(made.status == 200, "its chat: {made}");
    let chat = name(&made);
    let joined = call("PUT", &format!("/api/f/{chat}/members/{id}"), json!({ "role": "editor" }))?;
    let hello = call("POST", &format!("/api/f/{agent}/channels/tasks"), json!({ "id": "hello", "body": { "kind": "routine", "chat": chat, "text": "You were just made: say hello." } }))?;
    anyhow::ensure!(joined.status == 200 && hello.status == 200, "it joins its chat: {joined} {hello}");
    Ok((agent, chat, id))
}

/// The person's agents (the computer's), by fragment label: their identities.
fn agent_ids(api: &Api, session: &str, labels: &[&str]) -> Result<Vec<String>> {
    let r = shell(api, session, "GET", "/api/computers", None, &[])?;
    let agents = r.body["computers"][0]["agents"].as_array().cloned().unwrap_or_default();
    let ids: Vec<String> = labels
        .iter()
        .map(|l| agents.iter().find(|a| a["fragment"].as_str().map(label_of) == Some(*l)).and_then(|a| a["identity"].as_str()).unwrap_or("").to_string())
        .collect();
    anyhow::ensure!(ids.iter().all(|i| fragment_core::npub::is_identity(i)), "the agents' identities: {r}");
    Ok(ids)
}

/// What `@` lists in a framed chat's composer once its word is `typed`:
/// each option's agent, and whether it is one the message adds (not in
/// the chat yet). None while its composer is not ready.
fn mention_list(b: &mut Browser, page: &Page, host: &str, typed: &str) -> Value {
    let expr = format!(
        "(() => {{ const t = document.getElementById('text'); if (!t || t.disabled) return null; t.focus(); t.value = {}; t.setSelectionRange(t.value.length, t.value.length); \
         t.dispatchEvent(new Event('input', {{ bubbles: true }})); \
         return [...document.querySelectorAll('#mentions:not([hidden]) button')].map((b) => ({{ agent: b.dataset.agent, outside: b.classList.contains('outside') }})); }})()",
        js(typed)
    );
    b.eval_in_frame(page, host, &expr).unwrap_or(Value::Null)
}

/// Polls `mention_list` until it is `want`, at most `wait`: what it last listed.
fn mentions_until(s: &Suite, b: &mut Browser, page: &Page, host: &str, typed: &str, want: &Value, wait: std::time::Duration) -> Value {
    let mut last = Value::Null;
    let _ = s.eventually(wait, || {
        last = mention_list(b, page, host, typed);
        &last == want
    });
    last
}

/// A person's agents in any chat's `@` (decision 8; docs/chat-records.md,
/// "The page"): in a chat of their own, `@` lists its agent, then their
/// other agent, marked as one the message adds; picking it and sending
/// adds it to the chat (the shell's add, as an editor, after the lead), and
/// it answers there while the lead does not. In a chat someone else owns,
/// `@` lists that chat's own agents only: the shell hands a person's agents
/// only to the pages of their own fragments.
fn roster_ui(s: &mut Suite, api: &Api, b: &mut Browser, page: &Page, me: &Person, shots: &std::path::Path) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let agent_wait = std::time::Duration::from_secs(120);
    let ids = agent_ids(api, me.session, &["reader", me.first])?;
    let (reader, first) = (ids[0].clone(), ids[1].clone());
    let chat = own_named(api, me.session, "reader-chat")?;
    let host = chat.clone();
    b.click(page, &format!("#chats [data-key={}]", js(&format!("chat:{chat}"))))?;
    let want = json!([{ "agent": reader, "outside": false }, { "agent": first, "outside": true }]);
    let listed = mentions_until(s, b, page, &host, "@", &want, wait);
    let placeholder = b.eval_in_frame(page, &host, "document.getElementById('text')?.placeholder ?? null").unwrap_or(Value::Null);
    s.ok(
        "in a chat of their own, @ lists its agent, then their other agent, marked as one the message adds",
        listed == want && placeholder.as_str().is_some_and(|p| p.ends_with(", or @ someone else")),
        json!({ "listed": listed, "placeholder": placeholder }),
    );
    let _ = b.screenshot(page, &shots.join("desktop-roster.png"));
    // the roster says each agent's computer's phase, as the platform last said it
    let phases = b.eval_in_frame(
        page,
        &host,
        "new Promise((done) => { addEventListener('message', (e) => { if (e.data?.fragment === 'agents') done(e.data.agents.map((a) => [a.identity, a.phase ?? null])); }); \
         parent.postMessage({ fragment: 'agents?' }, '*'); setTimeout(() => done(null), 5000); })",
    )?;
    let phase = shell(api, me.session, "GET", "/api/computers", None, &[])?.body["computers"][0]["phase"].clone();
    let mut want_phases = vec![json!([reader, phase]), json!([first, phase])];
    let mut got_phases = phases.as_array().cloned().unwrap_or_default();
    want_phases.sort_by_key(Value::to_string);
    got_phases.sort_by_key(Value::to_string);
    s.ok("the shell's roster names each agent with its computer's phase (awake)", phase == "awake" && got_phases == want_phases, json!({ "roster": phases, "computer": phase }));

    // a page's own add (no click in the shell) asks its person in the
    // shell's dialog, and adds no one while it waits, nor on Cancel
    let agents_in = |api: &Api| -> Vec<Value> {
        shell(api, me.session, "GET", &format!("/api/f/{chat}/members"), None, &[])
            .map(|r| r.body["members"].as_array().into_iter().flatten().filter(|m| m["kind"] == "agent").map(|m| m["principal"].clone()).collect())
            .unwrap_or_default()
    };
    let posted = b.eval_in_frame(
        page,
        &host,
        &format!(
            "(() => {{ addEventListener('message', (e) => {{ if (e.data?.fragment === 'agent-added' && e.data.nonce === 'e2e-no-click') document.documentElement.dataset.e2eAdded = JSON.stringify(e.data); }}); \
             parent.postMessage({{ fragment: 'add-agent', identity: {}, nonce: 'e2e-no-click' }}, '*'); return true; }})()",
            js(&first)
        ),
    )?;
    let dialog = "document.getElementById('add-agent-dialog')";
    let asked = b.until(page, &format!("{dialog}.open && {dialog}.dataset.agent === {}", js(&first)), wait);
    let text = b.eval(page, "document.getElementById('add-agent-text').textContent")?;
    let armed_late = b.eval(page, "document.getElementById('add-agent-go').disabled")? == true;
    std::thread::sleep(std::time::Duration::from_secs(2));
    let while_open = agents_in(api);
    b.click(page, "#add-agent-cancel")?;
    let mut answer = Value::Null;
    let _ = s.eventually(wait, || {
        answer = b.eval_in_frame(page, &host, "document.documentElement.dataset.e2eAdded ?? null").unwrap_or(Value::Null);
        !answer.is_null()
    });
    let after = agents_in(api);
    s.ok(
        "a page's add asks the person in the shell's own dialog (its Add armed only after a moment), adds no one while left open, and Cancel answers declined",
        posted == true
            && asked
            && text.as_str().is_some_and(|t| t.contains(me.first_title) && t.contains("Reader"))
            && armed_late
            && while_open == vec![json!(reader)]
            && after == vec![json!(reader)]
            && answer.as_str().and_then(|a| serde_json::from_str::<Value>(a).ok()).is_some_and(|a| a["ok"] == false && a["error"] == "declined"),
        json!({ "text": text, "whileOpen": while_open, "after": after, "answer": answer }),
    );

    // picked with the keyboard, then sent
    let partial: String = me.first.chars().take(3).collect();
    let only = json!([{ "agent": first, "outside": true }]);
    let narrowed = mentions_until(s, b, page, &host, &format!("@{partial}"), &only, wait);
    let picked = b.eval_in_frame(
        page,
        &host,
        "(() => { const t = document.getElementById('text'); t.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true })); return t.value; })()",
    )?;
    let said = "hello from the roster";
    let sent = b.eval_in_frame(
        page,
        &host,
        &format!("(() => {{ const t = document.getElementById('text'); t.value += {}; t.dispatchEvent(new Event('input', {{ bubbles: true }})); document.getElementById('say').requestSubmit(); return true; }})()", js(said)),
    )?;
    s.ok("typing its name's start narrows @ to it, and Enter picks it", narrowed == only && picked == format!("@{} ", me.first).as_str() && sent == true, json!({ "narrowed": narrowed, "picked": picked }));
    // sending asks the person first, in the shell: they press Add
    let asked = b.until(page, &format!("{dialog}.open && {dialog}.dataset.agent === {}", js(&first)), wait);
    let unsent = agents_in(api) == vec![json!(reader)] && replies(api, me.session, &chat, &first, said) == 0;
    let _ = b.screenshot(page, &shots.join("desktop-roster-confirm.png"));
    let armed = b.until(page, "!document.getElementById('add-agent-go').disabled", wait);
    b.click(page, "#add-agent-go")?;
    let closed = b.until(page, &format!("!{dialog}.open"), wait);
    s.ok("sending asks the person in the shell's dialog first, and nothing is added or sent until they press Add", asked && unsent && armed && closed, json!({ "asked": asked, "unsent": unsent }));
    let mut members = Value::Null;
    let added = s.eventually(wait, || {
        members = shell(api, me.session, "GET", &format!("/api/f/{chat}/members"), None, &[]).map(|r| r.body).unwrap_or_default();
        let agents: Vec<(Value, Value)> = members["members"].as_array().into_iter().flatten().filter(|m| m["kind"] == "agent").map(|m| (m["principal"].clone(), m["role"].clone())).collect();
        agents == vec![(json!(reader), json!("editor")), (json!(first), json!("editor"))]
    });
    s.ok("sending adds it to the chat first, an editor after the lead (the shell's add)", added, &members);
    let answered = s.eventually(agent_wait, || replies(api, me.session, &chat, &first, said) == 1);
    let shown = s.eventually(wait, || b.eval_in_frame(page, &host, "document.body.innerText").ok().and_then(|v| v.as_str().map(|t| t.matches(said).count() >= 2)).unwrap_or(false));
    s.ok(
        "and it answers there, framed in the shell, while the chat's lead does not",
        answered && shown && replies(api, me.session, &chat, &reader, said) == 0,
        json!({ "answered": answered, "shown": shown }),
    );
    let _ = b.screenshot(page, &shots.join("desktop-roster-added.png"));

    // a chat someone else owns, with the person's first agent in it
    let me_id = super::signin::who(api, me.session)?["id"].as_str().unwrap_or("").to_string();
    let other = api.person()?;
    let theirs = api.qualified(&other, "their-chat")?;
    let r = api.create_with(&other, json!({ "name": theirs, "template": "chat", "title": "Their chat" }))?;
    anyhow::ensure!(r.status == 200, "their chat: {r}");
    for (who, role) in [(first.as_str(), "editor"), (me_id.as_str(), "editor")] {
        let r = api.signed(&other, "PUT", &format!("/api/f/{theirs}/members/{who}"), Some(&json!({ "role": role })))?;
        anyhow::ensure!(r.status == 200, "a member of their chat: {r}");
    }
    let row = format!("#chats [data-key={}]", js(&format!("chat:{theirs}")));
    let there = b.until(page, &format!("document.querySelector({})", js(&row)), wait);
    b.click(page, &row)?;
    let host = theirs.clone();
    let want = json!([{ "agent": first, "outside": false }]);
    let listed = mentions_until(s, b, page, &host, "@", &want, wait);
    // the roster would have come by now (it is asked as the page mounts)
    std::thread::sleep(std::time::Duration::from_secs(2));
    let still = mention_list(b, page, &host, "@");
    s.ok(
        "in a chat someone else owns, @ lists that chat's agents only: the person's other agent is not offered",
        there && listed == want && still == want,
        json!({ "listed": listed, "after": still }),
    );
    b.click(page, &format!("#chats [data-key={}]", js(&format!("chat:{chat}"))))?;
    Ok(())
}

/// A CLI home in the run's scratch (`name`), its key approved for the
/// person `session` signs in, as `fragment login` leaves one.
fn cli_home(s: &mut Suite, api: &Api, session: &str, name: &str) -> Result<std::path::PathBuf> {
    let home = s.dir(name);
    let keys = fragment_nip98::Keys::generate();
    api.approve(session, &keys)?;
    let config = home.join(if cfg!(target_os = "macos") { "Library/Application Support" } else { ".config" }).join("fragment");
    std::fs::create_dir_all(&config)?;
    std::fs::write(config.join("config.json"), json!({ "secret_key": keys.secret_hex() }).to_string())?;
    Ok(home)
}

/// The composer of `chat`'s page, framed: what it holds, whether it has
/// its page's focus, what of it is selected, whether it quotes a message.
const COMPOSER: &str = "(() => { const t = document.getElementById('text'); return { value: t?.value ?? null, focused: document.activeElement === t, selected: t ? t.value.slice(t.selectionStart, t.selectionEnd) : null, quoting: !document.getElementById('quoting')?.hidden }; })()";

/// How many records `chat`'s chat channel holds.
fn chat_count(api: &Api, session: &str, chat: &str) -> usize {
    shell(api, session, "GET", &format!("/api/f/{chat}/channels/chat?after=0&limit=1000"), None, &[]).ok().and_then(|r| r.body["records"].as_array().map(Vec::len)).unwrap_or(usize::MAX)
}

/// The ways to ask for an app the shell offers, three at a time (Paul's
/// twelve, 2026-10-16, in his order: `APP_PROMPTS` in cell/shell/shell.js),
/// and the label of the die that deals three more.
const APP_PROMPTS: [&str; 12] = [
    "Make a shared grocery list my family can add to from their phones",
    "Tell me on my phone as soon as … comes back in stock or drops in price",
    "Every morning, read these sites and send me a short summary: …",
    "Make a page for our trip to … where we log who paid for what and it shows who owes whom",
    "Build a habit tracker that shows my weekly streak for: …",
    "Make an RSVP page for my dinner on Saturday that tells me who's coming",
    "Make a reading list where I paste a link and it keeps the title and a one-line summary",
    "Build a chore chart for our house that rotates the jobs each week between: …",
    "Make a page where my team votes on … and everyone sees the results live",
    "Make a page where friends pick which evenings work for …, and it shows the best night",
    "Make a status board that pings me when a build or deploy fails for …",
    "Make a trip planner for … that my friends can add ideas and dates to",
];
const DIE_LABEL: &str = "Show three more ideas";

/// A JSON array's strings.
fn strings(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(|p| p.as_str().map(str::to_string)).collect()
}

/// Asking for an app (Paul, 2026-10-14: "rather than spawning generic apps
/// we teach the user how to prompt for new apps"), on a desktop's width:
/// the empty desktop's three ways to ask (`asks`) and the dock's Add's,
/// three at a time (Paul, 2026-10-16), the same three, no templates; one
/// pressed goes into the open chat's composer (one of theirs), focused, in
/// place of what was typed and of a quote, its `…` selected, and nothing
/// is sent (the chat's records stay as they were). The die deals three
/// more: never one showing, all twelve in four hands before any again,
/// the empty desktop's the same hand; one it dealt goes into the composer
/// as the first did. The chat it went to.
fn app_prompts_ui(s: &mut Suite, api: &Api, b: &mut Browser, page: &Page, session: &str, asks: &Value, shots: &std::path::Path) -> Result<String> {
    let wait = std::time::Duration::from_secs(30);
    let open = open_chat(b, page);
    anyhow::ensure!(own_named(api, session, label_of(&open)).is_ok_and(|n| n == open), "the open chat is theirs: {open}");
    let chat = open.as_str();
    let _ = s.eventually(wait, || b.eval_in_frame(page, chat, "!!document.getElementById('text') && !document.getElementById('text').disabled").ok() == Some(Value::Bool(true)));
    let before = chat_count(api, session, chat);
    let first = asks[0].as_str().unwrap_or("").to_string();
    b.eval(page, "(document.querySelector('#desktop-empty .desktop-suggestion')?.click(), true)")?;
    let filled = s.eventually(wait, || b.eval_in_frame(page, chat, COMPOSER).ok().is_some_and(|v| v["value"] == first.as_str() && v["focused"] == true));
    std::thread::sleep(std::time::Duration::from_secs(2));
    let after = chat_count(api, session, chat);
    s.ok(
        "a way to ask on the empty desktop goes into the open chat's composer, focused, and nothing is sent",
        !first.is_empty() && filled && after == before,
        json!({ "composer": b.eval_in_frame(page, chat, COMPOSER).unwrap_or(Value::Null), "records": [before, after] }),
    );

    // the dock's Add
    b.click(page, "#add-app")?;
    let pane = "#stack .pane[data-key=prompts]";
    let hand = format!("[...document.querySelectorAll('{pane} .app-prompt')].map((b) => b.textContent)");
    let three = format!("document.querySelectorAll('{pane} .app-prompt').length === 3");
    let listed = b.until(page, &three, wait);
    let shown = b.eval(
        page,
        &format!("({{ title: document.querySelector('{pane} .pane-title')?.textContent ?? null, prompts: {hand}, die: document.querySelector('{pane} .prompt-dice')?.getAttribute('aria-label') ?? null, templates: !!document.querySelector('{pane} form, {pane} input, .catalog') }})"),
    )?;
    let prompts = strings(&shown["prompts"]);
    s.ok(
        "the dock's Add opens ways to ask their agent for an app, in a window on the desktop, and no templates: three at a time, the first three of the twelve (the empty desktop's), some with a … to make theirs, and a die for three more",
        listed
            && shown["title"] == "Ask for an app"
            && shown["templates"] == false
            && prompts == APP_PROMPTS[..3]
            && strings(asks) == prompts
            && prompts.iter().any(|p| p.contains('…'))
            && shown["die"] == DIE_LABEL,
        &shown,
    );
    b.until(page, "document.getAnimations().every((a) => a.playState !== 'running' || a.effect?.getComputedTiming().iterations === Infinity)", wait);
    let _ = b.screenshot(page, &shots.join("desktop-prompts.png"));
    // a reply's quote in the composer: an app's request is no reply, so the prompt clears it
    b.eval_in_frame(page, chat, "([...document.querySelectorAll('.msg.agent .actions .reply')].pop()?.click(), true)")?;
    let quoted = s.eventually(wait, || b.eval_in_frame(page, chat, "!document.getElementById('quoting').hidden").ok() == Some(Value::Bool(true)));
    let pick = prompts.iter().position(|p| p.contains('…')).unwrap_or(0);
    let text = prompts.get(pick).cloned().unwrap_or_default();
    let before = chat_count(api, session, chat);
    b.eval(page, &format!("(document.querySelectorAll('{pane} .app-prompt')[{pick}]?.click(), true)"))?;
    let composed = s.eventually(wait, || {
        b.eval_in_frame(page, chat, COMPOSER).ok().is_some_and(|v| v["value"] == text.as_str() && v["focused"] == true && v["selected"] == "…" && v["quoting"] == false)
    });
    let closed = b.until(page, &format!("!document.querySelector('{pane}')"), wait);
    // the shell's focus is the chat's frame, so typing goes there
    let framed = b.eval(page, &format!("document.activeElement?.dataset?.fragment === {}", js(chat)))? == true;
    std::thread::sleep(std::time::Duration::from_secs(2));
    let after = chat_count(api, session, chat);
    let _ = b.screenshot(page, &shots.join("desktop-prompt-composed.png"));
    s.ok(
        "one pressed goes into the open chat's composer in place of what was typed, and clears the reply's quote set just before (a request is no reply), focused, its … selected to type over; the window closes, and nothing is sent",
        quoted && composed && closed && framed && after == before,
        json!({ "composer": b.eval_in_frame(page, chat, COMPOSER).unwrap_or(Value::Null), "records": [before, after], "quoted": quoted, "framed": framed }),
    );

    // its die, the window open again on the hand it showed: four deals
    b.click(page, "#add-app")?;
    let reopened = b.until(page, &three, wait);
    let mut hands = vec![strings(&b.eval(page, &hand)?)];
    for _ in 0..4 {
        let last = js(&serde_json::to_string(&hands[hands.len() - 1])?);
        b.click(page, &format!("{pane} .prompt-dice"))?;
        b.until(page, &format!("(() => {{ const h = {hand}; return h.length === 3 && JSON.stringify(h) !== {last}; }})()"), wait);
        hands.push(strings(&b.eval(page, &hand)?));
    }
    let focused = b.eval(page, &format!("document.activeElement === document.querySelector('{pane} .prompt-dice')"))? == true;
    let desktop = strings(&b.eval(page, "[...document.querySelectorAll('#desktop-empty .desktop-suggestion')].map((b) => b.textContent)")?);
    let fresh = hands.windows(2).all(|w| {
        let dealt: std::collections::BTreeSet<&String> = w[1].iter().collect();
        dealt.len() == 3 && w[1].iter().all(|p| APP_PROMPTS.contains(&p.as_str()) && !w[0].contains(p))
    });
    let cycled: std::collections::BTreeSet<&String> = hands[..4].iter().flatten().collect();
    s.ok(
        "its die deals three more each time, none of the three showing: the first hand and three deals show all twelve, none twice, and the next deal none of the last; the empty desktop's is the same hand, and the die keeps the focus",
        reopened && hands[0] == APP_PROMPTS[..3] && fresh && cycled.len() == 12 && desktop == hands[4] && focused,
        json!({ "hands": hands, "desktop": desktop, "focused": focused }),
    );
    b.until(page, "document.getAnimations().every((a) => a.playState !== 'running' || a.effect?.getComputedTiming().iterations === Infinity)", wait);
    let _ = b.screenshot(page, &shots.join("desktop-prompts-dealt.png"));
    // one it dealt, pressed
    let text = hands[4][0].clone();
    let slot = if text.contains('…') { "…" } else { "" };
    let before = chat_count(api, session, chat);
    b.eval(page, &format!("(document.querySelector('{pane} .app-prompt')?.click(), true)"))?;
    let composed = s.eventually(wait, || b.eval_in_frame(page, chat, COMPOSER).ok().is_some_and(|v| v["value"] == text.as_str() && v["focused"] == true && v["selected"] == slot));
    let closed = b.until(page, &format!("!document.querySelector('{pane}')"), wait);
    std::thread::sleep(std::time::Duration::from_secs(2));
    let after = chat_count(api, session, chat);
    s.ok(
        "one the die dealt, pressed, goes into the composer as the first did, focused, its … (if it has one) selected; the window closes, and nothing is sent",
        composed && closed && after == before,
        json!({ "composer": b.eval_in_frame(page, chat, COMPOSER).unwrap_or(Value::Null), "records": [before, after] }),
    );
    Ok(open)
}

/// A `compose` from any page but the shell's does nothing: from an app's
/// page (another origin, to every frame of the shell) and from the chat's
/// own page to itself, the composer keeps what it held. Then it is emptied
/// for the checks after.
fn compose_elsewhere_ui(s: &mut Suite, api: &Api, b: &mut Browser, page: &Page, session: &str, chat: &str, app: &str) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let held = b.eval_in_frame(page, chat, COMPOSER)?;
    let before = chat_count(api, session, chat);
    let from_app = s.eventually(wait, || {
        b.eval_in_frame(page, app, "(() => { for (let i = 0; i < parent.frames.length; i++) parent.frames[i].postMessage({ fragment: 'compose', text: 'from an app of theirs' }, '*'); return true; })()").ok() == Some(Value::Bool(true))
    });
    b.eval_in_frame(page, chat, "(window.postMessage({ fragment: 'compose', text: 'from its own page' }, '*'), true)")?;
    std::thread::sleep(std::time::Duration::from_secs(2));
    let now = b.eval_in_frame(page, chat, COMPOSER)?;
    let after = chat_count(api, session, chat);
    s.ok(
        "a compose from an app's page (another origin) or from the chat's own page does nothing: the composer keeps what it held, and nothing is sent",
        from_app && held["value"].is_string() && now["value"] == held["value"] && after == before,
        json!({ "held": held, "now": now, "records": [before, after] }),
    );
    b.eval_in_frame(page, chat, "(() => { const t = document.getElementById('text'); t.value = ''; t.dispatchEvent(new Event('input', { bubbles: true })); t.blur(); return true; })()")?;
    Ok(())
}

/// `fragment ask` as a person (an agent's runs in its computer: the hosted
/// lane's, and our Hermes image's): their direct chat with the agent, its
/// answer waited for and printed; the same `--id` again posts nothing and
/// finds the same answer; `--chat` adds the agent to a chat it is not in
/// first. Invalid: an agent that is none of theirs, an empty question.
fn ask_cli(s: &mut Suite, api: &Api, me: &Person) -> Result<()> {
    let home = cli_home(s, api, me.session, "ask-cli-home")?;
    let ids = agent_ids(api, me.session, &["reader"])?;
    let reader = ids[0].clone();
    let said = "hello from the cli";
    let args = ["ask", "reader", said, "--wait", "60", "--id", "ask-e2e-1", "--json"];
    let r = s.cli_json(api, &home, &args)?;
    let answer = |r: &Value| r["answer"]["replies"].as_array().into_iter().flatten().filter_map(|x| x["text"].as_str()).collect::<Vec<_>>().join("\n");
    s.ok(
        "fragment ask, as a person: their direct chat with the agent, the question to it, its answer waited for",
        r["chat"] == own_named(api, me.session, "reader-chat")?.as_str() && r["asked"]["identity"] == reader.as_str() && r["record"]["body"]["to"] == json!([reader]) && r["answer"]["outcome"] == "idle" && answer(&r).contains(said) && answer(&r).starts_with("echo:"),
        &r,
    );
    let again = s.cli_json(api, &home, &args)?;
    s.ok(
        "the same --id again posts nothing, and finds the same answer at once",
        again["replayed"] == true && again["record"]["seq"] == r["record"]["seq"] && again["answer"]["turn"] == r["answer"]["turn"] && !again["answer"]["turn"].is_null(),
        &again,
    );
    let made = shell(api, me.session, "POST", "/api/fragments", Some(&json!({ "label": "ask-here", "template": "chat", "title": "Ask here" })), &[])?;
    let here = made.body["name"].as_str().unwrap_or("").to_string();
    let r = s.cli_json(api, &home, &["ask", "reader", "and here?", "--chat", &here, "--wait", "60", "--json"])?;
    s.ok(
        "--chat: an agent not in that chat is added first, and answers there",
        made.status == 200 && r["chat"] == here.as_str() && r["added"] == json!([reader]) && r["answer"]["outcome"] == "idle" && answer(&r).contains("and here?"),
        &r,
    );
    let refused = |s: &Suite, args: &[&str]| {
        let out = s.cli(api, &home, args);
        (out.status.code(), serde_json::from_slice::<Value>(&out.stdout).unwrap_or_default())
    };
    let (none_exit, none) = refused(s, &["ask", "nobody", "hi", "--json"]);
    let (empty_exit, empty) = refused(s, &["ask", "reader", "  ", "--json"]);
    s.ok(
        "an agent that is none of theirs is not_found, and an empty question invalid_usage (exit 2)",
        none_exit == Some(1) && none["error"]["code"] == "not_found" && empty_exit == Some(2) && empty["error"]["code"] == "invalid_usage",
        json!({ "none": none, "empty": empty }),
    );
    Ok(())
}
