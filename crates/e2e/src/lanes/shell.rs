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
    // Finite's about page, privacy policy and terms: pages for anyone,
    // signed out too, static (no script), never framed; each its title and
    // its heading
    let legal: Vec<(Reply, &str, &str)> = vec![
        (api.unsigned("GET", "/about", None)?, "About Finite", "About Finite"),
        (api.unsigned("GET", "/privacy", None)?, "Privacy · Finite", "Privacy"),
        (api.unsigned("GET", "/terms", None)?, "Terms of service · Finite", "Terms of service"),
    ];
    let shown = legal.iter().all(|(r, title, h1)| {
        let header = |name: &str| r.headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        r.status == 200
            && header("content-type").starts_with("text/html")
            && r.text.contains(&format!("<title>{title}</title>"))
            && r.text.contains(&format!("<h1>{h1}</h1>"))
            && r.text.contains("/__shell/platform-pages.css")
            && r.text.contains("/__shell/legal.css")
            && !r.text.contains("<script")
            && header("content-security-policy").contains("frame-ancestors 'none'")
            && header("content-security-policy").contains("font-src 'self'")
    });
    // their stylesheets, their paper's edge, and the type they and the shell
    // share (platform-pages.css), the release's own; and the cell's pages'
    // base look and their card's script, which they link too
    let published = ["legal.css", "platform-pages.css", "funnel-sans-400.ttf", "funnel-sans-500.ttf", "funnel-sans-600.ttf", "funnel-sans-700.ttf", "paper-edge.svg", "sheet.css", "platform-pages.js"]
        .iter()
        .map(|f| api.unsigned("GET", &format!("/__shell/{f}"), None).map(|r| (r.status, r.headers.get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string())))
        .collect::<Result<Vec<_>>>()?;
    let typed = published.iter().zip(["text/css", "text/css", "font/ttf", "font/ttf", "font/ttf", "font/ttf", "image/svg+xml", "text/css", "text/javascript"]).all(|((status, got), want)| *status == 200 && got.starts_with(want));
    s.ok(
        "Finite's about page (/about), privacy policy (/privacy) and terms (/terms) answer anyone, signed out: static pages with their titles, in the release's own type (their CSP allows its fonts), their stylesheets and paper published, never framed",
        shown && typed,
        json!({ "pages": legal.iter().map(|(r, title, _)| json!({ "title": title, "status": r.status, "head": r.text.chars().take(300).collect::<String>() })).collect::<Vec<_>>(), "published": published }),
    );

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
            && rows.len() == crate::SWAP_KEYS.len() + 5,
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
    dock_choice(s, api, &session)?;
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
    let r = api.share(&chat, &member_id, "viewer")?;
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
    let back = api.share(&chat, &member_id, "viewer")?;
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
/// shared with them by someone else, an invite to their email (as
/// `invited`) and their yes to it, an archive, a removal and a delete;
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
    let r = api.share(&theirs, id, "viewer")?;
    let shared = told(&mut tab);
    let list = shell(api, session, "GET", "/api/fragments", None, &[])?;
    let listed = list.body["fragments"].as_array().is_some_and(|l| l.iter().any(|f| f["name"] == theirs.as_str() && f["role"] == "viewer"));
    s.ok("one someone else shares with them is told, and their list then has it", r.status == 200 && shared == changed && listed, json!([r.status, shared, list.body]));

    // an invite to their email waits for their yes (decision 63): it is
    // told as `invited`, naming nothing, to every tab, and the shell reads
    // the invites then; their yes, the shell's with its session, is a share
    // like any, told as a change of their list
    let me = shell(api, session, "GET", "/api/identities/me", None, &[])?;
    let email = me.body["email"].as_str().unwrap_or("").to_string();
    // (the second tab was told of the share above too: its frame read here)
    let shared_too = told(&mut second);
    let asks = s.named(api, &other, "asks")?;
    let r = api.create(&other, &asks)?;
    let invited = json!({ "type": "invited" });
    let r2 = api.signed(&other, "POST", &format!("/api/f/{asks}/invites"), Some(&json!({ "email": email, "role": "viewer" })))?;
    let (one, two) = (told(&mut tab), told(&mut second));
    let seen = shell(api, session, "GET", "/api/invites", None, &[])?;
    let waits = seen.body["invites"].as_array().is_some_and(|l| l.iter().any(|i| i["fragment"] == asks.as_str() && i["email"] == email.as_str()));
    s.ok(
        "an invite to their email is told to every tab as `invited`, naming nothing, and the shell then reads it with its session (GET /api/invites)",
        shared_too == changed && r.status == 200 && r2.status == 200 && !r2.body["invited"].is_null() && one == invited && two == invited && waits,
        json!([shared_too, r2.body, one, two, seen.body]),
    );
    let yes = shell(api, session, "POST", &format!("/api/invites/{asks}/accept"), Some(&json!({})), &[])?;
    let (one, two) = (told(&mut tab), told(&mut second));
    let (one_more, two_more) = (told(&mut tab), told(&mut second));
    let list = shell(api, session, "GET", "/api/fragments", None, &[])?;
    let listed = list.body["fragments"].as_array().is_some_and(|l| l.iter().any(|f| f["name"] == asks.as_str() && f["role"] == "viewer"));
    s.ok(
        "their yes, the shell's with its session, is a share like any: told as a change of their list, which has it; and as `invited`, so another tab's sidebar drops it",
        yes.status == 200 && one == changed && two == changed && one_more == invited && two_more == invited && listed,
        json!([yes.body, one, two, one_more, two_more]),
    );
    // their no, in one tab: every tab is told, and it waits no more
    let declined = s.named(api, &other, "asks-again")?;
    let r = api.create(&other, &declined)?;
    let r2 = api.signed(&other, "POST", &format!("/api/f/{declined}/invites"), Some(&json!({ "email": email, "role": "editor" })))?;
    let (_, _) = (told(&mut tab), told(&mut second));
    let no = shell(api, session, "POST", &format!("/api/invites/{declined}/decline"), Some(&json!({})), &[])?;
    let (one, two) = (told(&mut tab), told(&mut second));
    let seen = shell(api, session, "GET", "/api/invites", None, &[])?;
    let gone = seen.body["invites"].as_array().is_some_and(|l| !l.iter().any(|i| i["fragment"] == declined.as_str()));
    s.ok(
        "their no, in one tab, is told to every tab as `invited` at once, and the invite waits no more",
        r.status == 200 && r2.status == 200 && no.status == 200 && one == invited && two == invited && gone,
        json!([no.body, one, two, seen.body]),
    );
    // an invite to a fragment deleted meanwhile goes from every tab too
    let r2 = api.signed(&other, "POST", &format!("/api/f/{declined}/invites"), Some(&json!({ "email": email, "role": "viewer" })))?;
    let (_, _) = (told(&mut tab), told(&mut second));
    let deleted = api.signed(&other, "DELETE", &format!("/api/f/{declined}"), None)?;
    let (one, two) = (told(&mut tab), told(&mut second));
    let seen = shell(api, session, "GET", "/api/invites", None, &[])?;
    let gone = seen.body["invites"].as_array().is_some_and(|l| !l.iter().any(|i| i["fragment"] == declined.as_str()));
    s.ok(
        "an invite to a fragment its owner deletes is told to every tab as `invited`, and waits no more",
        r2.status == 200 && deleted.status == 200 && one == invited && two == invited && gone,
        json!([deleted.body, one, two, seen.body]),
    );
    let r = shell(api, session, "PUT", &format!("/api/fragments/{made}/archived"), Some(&json!({ "archived": true })), &[])?;
    let (one, two) = (told(&mut tab), told(&mut second));
    s.ok("an archive in one tab is told to every tab", r.status == 200 && one == changed && two == changed, json!([r.status, one, two]));
    let r = shell(api, session, "PUT", &format!("/api/fragments/{made}/dock"), Some(&json!({ "dock": false })), &[])?;
    let (one, two) = (told(&mut tab), told(&mut second));
    s.ok("and an app taken off the dock in one tab", r.status == 200 && one == changed && two == changed, json!([r.status, one, two]));
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
    let joined = api.share(&steps, &hand_id, "editor")?;
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
    let r = api.share(&chat, &member_id, "viewer")?;
    anyhow::ensure!(r.status == 200, "adding the member: {r}");
    let theirs = |keys: &fragment_nip98::Keys, q: &str| api.signed(keys, "GET", &format!("/api/search?q={}", query(q)), None);
    let found = s.eventually(SEARCH_WAIT, || theirs(&member, "tomatoes").is_ok_and(|r| !hits_in(&r, &chat).is_empty()));
    s.ok("a new member's search finds what the chat said before they joined (signed, with their key)", found, theirs(&member, "tomatoes")?);

    // the chat's agents, the first added first, are named in every member's
    // row (agents of someone in no part of it, by a lever: a member's own
    // come in with them, decision 64, and leave with them)
    let rows = || -> Result<(Value, Value)> { Ok((row(&mine()?), row(&api.signed(&member, "GET", "/api/fragments", None)?))) };
    let (_, lead) = super::delegation::agent_of(api, &outsider)?;
    let (_, second) = super::delegation::agent_of(api, &outsider)?;
    let added = [&lead, &second].map(|a| api.share(&chat, a, "viewer"));
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
    let r = api.share(&chat, &member_id, "viewer")?;
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

/// The dock, the person's own choice (#587; docs/api.md, `PUT
/// /api/fragments/{name}/dock`): every fragment is on it until they take
/// it off, for themselves alone (a member's list keeps its own), twice the
/// same as once, and only one of theirs; archiving is a choice of its own;
/// it goes when they leave (back in, it is on it). Valid, invalid and
/// replay here; a restart keeps it (the restart section).
fn dock_choice(s: &mut Suite, api: &Api, session: &str) -> Result<()> {
    let r = shell(api, session, "POST", "/api/fragments", Some(&json!({ "label": "docked" })), &[])?;
    let app = r.body["name"].as_str().unwrap_or("").to_string();
    anyhow::ensure!(r.status == 200 && label_of(&app) == "docked", "making the app: {r}");
    let member = api.person()?;
    let member_id = api.identity(&member)?;
    let r = api.share(&app, &member_id, "viewer")?;
    anyhow::ensure!(r.status == 200, "adding the member: {r}");
    let row = |r: &Reply| r.body["fragments"].as_array().and_then(|l| l.iter().find(|f| f["name"] == app.as_str()).cloned());
    // on the dock, a row leaves `dock` out; off it, it says `false`
    let on = |r: &Reply| row(r).is_some_and(|f| f.get("dock").is_none());
    let off = |r: &Reply| row(r).is_some_and(|f| f["dock"] == false);
    let mine = || shell(api, session, "GET", "/api/fragments", None, &[]);
    let theirs = || api.signed(&member, "GET", "/api/fragments", None);
    let dock = |name: &str, body: Value| shell(api, session, "PUT", &format!("/api/fragments/{name}/dock"), Some(&body), &[]);
    let listed = s.eventually(SEARCH_WAIT, || theirs().is_ok_and(|r| row(&r).is_some()));
    let (m, t) = (mine()?, theirs()?);
    s.ok("a new app is on its owner's dock and on a member's: their rows leave `dock` out", listed && on(&m) && on(&t), json!([row(&m), row(&t)]));

    let r = dock(&app, json!({ "dock": false }))?;
    s.ok("the person takes an app of theirs off their dock", r.status == 200 && r.body == json!({ "name": app, "dock": false }), &r);
    let again = dock(&app, json!({ "dock": false }))?;
    s.ok("taking it off again is the same", again.status == 200 && again.body == r.body, &again);
    let (m, t) = (mine()?, theirs()?);
    s.ok("their list says it is off their dock, and not archived; a member's has it on theirs still", off(&m) && row(&m).is_some_and(|f| f.get("archived").is_none()) && on(&t), json!([row(&m), row(&t)]));
    let r = api.signed(&member, "PUT", &format!("/api/fragments/{app}/dock"), Some(&json!({ "dock": false })))?;
    let back = dock(&app, json!({ "dock": true }))?;
    let (m, t) = (mine()?, theirs()?);
    s.ok(
        "the member, a viewer, takes it off their own dock (with their key); the person puts it back on theirs, and each list keeps its own",
        r.status == 200 && back.status == 200 && back.body == json!({ "name": app, "dock": true }) && on(&m) && off(&t),
        json!([row(&m), row(&t)]),
    );
    // archiving is a choice of its own: neither changes the other
    let archive = |on: bool| shell(api, session, "PUT", &format!("/api/fragments/{app}/archived"), Some(&json!({ "archived": on })), &[]);
    let (a, d, u) = (archive(true)?, dock(&app, json!({ "dock": false }))?, archive(false)?);
    let m = mine()?;
    s.ok("archived, taken off, then unarchived, it is off their dock still", a.status == 200 && d.status == 200 && u.status == 200 && off(&m), json!(row(&m)));

    let other = api.person()?;
    let theirs_only = s.named(api, &other, "not-docked")?;
    let r = api.create(&other, &theirs_only)?;
    anyhow::ensure!(r.status == 200, "someone else's app: {r}");
    let r = dock(&theirs_only, json!({ "dock": false }))?;
    s.ok("a fragment they are not in is none of theirs to take off (404)", r.status == 404, &r);
    let r = dock("nothing-here--k3x9", json!({ "dock": false }))?;
    s.ok("nor one that does not exist (404)", r.status == 404, &r);
    let r = dock("docked", json!({ "dock": false }))?;
    s.ok("a bare label names nothing here (404): a fragment is named in full", r.status == 404 && r.message().contains("in full"), &r);
    let r = dock("Not A Name", json!({ "dock": false }))?;
    s.ok("a name that is none is refused (400)", r.status == 400, &r);
    let (r, r2) = (dock(&app, json!({}))?, dock(&app, json!({ "dock": "no" }))?);
    s.ok("a body without `dock`, or not a boolean, is refused (400)", r.status == 400 && r2.status == 400, format!("{r} {r2}"));
    let r = api.unsigned("PUT", &format!("/api/fragments/{app}/dock"), Some(&json!({ "dock": true })))?;
    let m = mine()?;
    s.ok("no one chooses anyone's dock (401), and theirs is as it was", r.status == 401 && off(&m), &r);

    // removed, their choice goes with their row; back in, it is on their dock
    let r = shell(api, session, "DELETE", &format!("/api/f/{app}/members/{member_id}"), None, &[])?;
    let gone = s.eventually(SEARCH_WAIT, || theirs().is_ok_and(|r| r.status == 200 && row(&r).is_none()));
    let r2 = api.signed(&member, "PUT", &format!("/api/fragments/{app}/dock"), Some(&json!({ "dock": false })))?;
    s.ok("a removed member's list has it no more, nor may they choose its place (404)", r.status == 200 && gone && r2.status == 404, &r2);
    let r = api.share(&app, &member_id, "viewer")?;
    let back = s.eventually(SEARCH_WAIT, || theirs().is_ok_and(|r| row(&r).is_some()));
    let t = theirs()?;
    s.ok("back in, it is on their dock again", r.status == 200 && back && on(&t), json!(row(&t)));
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
    let add = |keys: &fragment_nip98::Keys| api.share(&name, keys.pubkey_hex(), "viewer");
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
    // and when it was deployed (`FRAGMENT_DEPLOYED_AT`: the node's start
    // here, the deploy's second hosted), which Settings shows beside it
    let deployed = page.text.split_once(r#"<meta name="fragment-release-deployed" content=""#).and_then(|(_, rest)| rest.split_once('"')).map(|(v, _)| v.to_string());
    let at = deployed.as_deref().and_then(fragment_core::release::deployed);
    let filled = fragment_templates::shell_page(std::str::from_utf8(index.bytes)?, &release, at);
    s.ok(
        "the shell's page is the release's, its metas naming the release that answered and when it was deployed, under the platform's policy",
        page.status == 200
            && page.text == filled
            && page.text.contains(&format!(r#"<meta name="fragment-release" content="{release}">"#))
            && at.is_some_and(|t| t > 0 && t as i64 <= crate::api::now_s() + 60)
            && page.header("content-security-policy").contains("frame-ancestors 'none'")
            && page.header("x-frame-options") == "DENY"
            && page.header("cache-control") == "no-store",
        format!("{} {} bytes (the release's filled: {}), release {release}, deployed {deployed:?}; {}", page.status, page.bytes.len(), page.text == filled, page.header("content-security-policy")),
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
/// page's start (`refresh_failed_start_ui`, `first_run_put_off_ui`), and
/// last a guest told who is signed in, who uses another account to sign
/// in to this seat (`guest_ui`). The agents run on the stub image
/// (Docker), as the computers section's do.
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
    // under the card, Finite's footer: who makes it, this year, and About,
    // Contact, Privacy Policy and Terms, the pages each in a tab of its own
    let links = b.eval(
        &page,
        "(() => { const f = document.querySelector('#first-run .page-footer'); return f && { shown: f.getClientRects().length > 0, made: f.querySelector(':scope > span')?.textContent, links: [...f.querySelectorAll('nav a')].map((a) => [a.textContent, a.getAttribute('href'), a.target]) }; })()",
    )?;
    let year = b.eval(&page, "String(new Date().getFullYear())")?;
    s.ok(
        "and under the card, Finite's footer: this year's \"© Finite Computer LLC\", and About, Contact, Privacy Policy and Terms, pointing at /about, its mail, /privacy and /terms",
        links
            == json!({ "shown": true, "made": format!("{} © Finite Computer LLC", year.as_str().unwrap_or("")), "links": [["About", "/about", "_blank"], ["Contact", "mailto:support@finite.computer", ""], ["Privacy Policy", "/privacy", "_blank"], ["Terms", "/terms", "_blank"]] }),
        &links,
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
    // its type is the release's own Funnel Sans (platform-pages.css), loaded under
    // the shell's CSP: the card's title (500) and its words (400)
    let faces = b.eval(
        &page,
        "document.fonts.ready.then(() => [...document.fonts].filter((f) => f.family.replace(/\"/g, '') === 'Funnel Sans' && f.status === 'loaded').map((f) => f.weight).sort())",
    )?;
    s.ok(
        "drawn in the release's own Funnel Sans, loaded under the shell's CSP: the card's title and its words",
        faces.as_array().is_some_and(|w| w.contains(&json!("400")) && w.contains(&json!("500"))),
        &faces,
    );
    // evidence for a person: the signed-out page, on a desktop and a phone,
    // in its own fonts
    let _ = b.eval(&page, "document.fonts.ready.then(() => true)");
    let _ = b.screenshot(&page, &shots.join("signed-out.png"));
    b.viewport(&page, 375, 812, true)?;
    let _ = b.screenshot(&page, &shots.join("phone-375-signed-out.png"));
    b.close(page)?;
    still_loading_ui(s, api, &mut b, &shots)?;

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
    app_policy_ui(s, api, &mut b, &page, &groceries)?;
    sidebar_live(s, api, &mut b, &page, &session, &shots)?;
    held_list_socket(s, api, &mut b, &page, &session)?;

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
    // SimpleX only on an image that does it: the stub's bridge never says so
    let computers = shell(api, &session, "GET", "/api/computers", None, &[])?;
    let said = computers.body["computers"][0].get("simplex").cloned();
    s.ok(
        "settings offer no SimpleX on a computer whose image does not do it (the stub's, as production's before SimpleX): no section, no Set up, and its view says nothing of it",
        computers.status == 200 && said.is_none() && !has("#settings-simplex") && !text.contains("SimpleX"),
        json!({ "simplex": said, "settings": &text[..text.len().min(600)] }),
    );
    let account = b.eval(&page, ACCOUNT_SEEN)?;
    s.ok(
        "settings' Account says who is signed in, the email in bold, beside Sign out and Use another account (back to settings); a seat's holder is told nothing a guest is",
        account_shows(&account, &email) && account["noSeat"] == json!([]) && !text.contains("You are a guest"),
        &account,
    );
    let legal = b.eval(&page, LEGAL_LINKS_IN_ACCOUNT)?;
    s.ok(
        "settings' Account ends with Finite's Privacy and Terms, each in a tab of its own",
        legal == json!({ "last": true, "links": [["Privacy", "/privacy", "_blank"], ["Terms", "/terms", "_blank"]], "text": "Privacy · Terms" }),
        &legal,
    );
    release_line_ui(s, api, &mut b, &page)?;
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
    // a clipboard that refuses (#438): a write rejected, then none at all
    let copy = "document.querySelector('#settings-page [data-action=copy-prompt]')";
    let unselect = "(() => { const p = document.getElementById('build-prompt'); p.setSelectionRange(0, 0); p.blur(); return true; })()";
    let refused = format!(
        "(() => {{ const v = {COPY_SEEN}; return v.said?.startsWith(\"Couldn't copy it.\") && !/permission/i.test(v.said) && v.button === 'Copy' && v.focused && v.selected; }})()"
    );
    b.until(&page, &format!("{copy}.textContent === 'Copy'"), wait);
    b.eval(&page, unselect)?;
    b.eval(&page, &format!("(navigator.clipboard.writeText = () => Promise.reject(new DOMException('Write permission denied.', 'NotAllowedError')), {copy}.click(), true)"))?;
    let rejected = b.until(&page, &refused, wait);
    let seen = b.eval(&page, COPY_SEEN)?;
    s.ok(
        "a Copy the clipboard refuses says so under the prompt, plainly, and selects the prompt to copy by hand; its button still says Copy",
        rejected,
        &seen,
    );
    b.eval(&page, unselect)?;
    b.eval(&page, &format!("(Object.defineProperty(navigator, 'clipboard', {{ configurable: true, get: () => undefined }}), {copy}.click(), true)"))?;
    let missing = b.until(&page, &refused, wait);
    let none = b.eval(&page, COPY_SEEN)?;
    b.eval(&page, &format!("(delete navigator.clipboard, navigator.clipboard.writeText = async (text) => {{ window.__copied = text; }}, window.__copied = null, {copy}.click(), true)"))?;
    let works = b.until(&page, &format!("window.__copied === {} && {copy}.textContent === 'Copied' && document.getElementById('build-prompt-said').hidden", js(&format!("{prompt}a chore wheel"))), wait);
    s.ok(
        "with no clipboard at all, the same; one that works again copies it, says Copied, and the line goes",
        missing && works,
        json!({ "none": none, "again": b.eval(&page, COPY_SEEN)? }),
    );
    // a press held, then another refused; the first then copies: the later
    // press's word stands (#464 review)
    b.until(&page, &format!("{copy}.textContent === 'Copy'"), wait);
    b.eval(&page, unselect)?;
    b.eval(&page, &format!("(window.__copy = null, navigator.clipboard.writeText = () => new Promise((done) => {{ window.__copy = done; }}), {copy}.click(), true)"))?;
    let held = b.until(&page, "typeof window.__copy === 'function'", wait);
    b.eval(&page, &format!("(window.__first = window.__copy, navigator.clipboard.writeText = () => Promise.reject(new DOMException('Write permission denied.', 'NotAllowedError')), {copy}.click(), true)"))?;
    let later = b.until(&page, &refused, wait);
    // (the first's answer lands within a task of its copy)
    b.eval(&page, "(async () => { window.__first(); await new Promise((r) => setTimeout(r, 0)); return true; })()")?;
    let stands = b.eval(&page, &refused)?;
    b.eval(&page, "(navigator.clipboard.writeText = async (text) => { window.__copied = text; }, true)")?;
    s.ok(
        "a Copy that answers after a newer one was refused changes nothing: the line stays, and the button still says Copy",
        held && later && stands == true,
        b.eval(&page, COPY_SEEN)?,
    );
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
    settings_race_ui(s, api, &mut b, &page, &session)?;
    notice_ui(s, &mut b, &page, &chat, &shots)?;
    reasoning_ui(s, api, &mut b, &page, &session, &shots)?;
    one_route_ui(s, api, &mut b, &session)?;
    models_ui(s, api, &mut b, &page, &session, &chat, &shots)?;
    left_and_back_ui(s, api, &mut b, &page, &session, &chat)?;
    computer_ui(s, api, &mut b, &page, &session, &chat, &shots)?;
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
    first_run_put_off_ui(s, api, &mut b, &shots)?;
    // a guest, in this browser, who then signs in to this seat
    guest_ui(s, api, &mut b, &email, &shots)?;
    println!("      (screenshots: {})", shots.display());
    Ok(())
}

/// The shell drawn nothing for a while (#470's run: one of shell.js's
/// imports never came, and the page stayed blank with nothing to press;
/// its first answer never coming leaves it the same): it says it is still
/// loading, with a reload of the same page, and only then. Valid: a page
/// that loads never says it (drawn, it takes the line away). One of
/// shell.js's imports refused: nothing for a while (a slow load is no
/// failure), then "Still loading… Reload", its words and link on a phone's
/// screen too; its Reload, the import let through, loads the same page,
/// and the shell starts. Its scripts come but who is signed in is never
/// answered: the same, and its Reload draws the shell.
fn still_loading_ui(s: &mut Suite, api: &Api, b: &mut Browser, shots: &std::path::Path) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let line = "document.getElementById('still-loading')";
    let started = "(!document.getElementById('first-run').hidden || !document.getElementById('layout').hidden)";
    let home = format!("{}/", api.base);
    let page = b.open(&home)?;
    let up = b.until(&page, started, wait);
    let gone = b.eval(&page, &format!("!{line}"))?;
    s.ok("a shell page that loads starts, and never says it is still loading (the line is taken away)", up && gone == true, &gone);
    b.close(page)?;

    let page = b.open_blocking(&home, &["*/__shell/billing.js"])?;
    b.viewport(&page, 1280, 800, false)?;
    let t0 = std::time::Instant::now();
    let quiet = b.until(&page, &format!("document.readyState === 'complete' && !!{line} && getComputedStyle({line}).visibility === 'hidden'"), wait);
    let shown = b.until(&page, &format!("!!{line} && getComputedStyle({line}).visibility === 'visible'"), wait);
    let after = t0.elapsed();
    let seen = b.eval(&page, &format!("({{ said: {line}?.textContent ?? null, link: {line}?.querySelector('a')?.href ?? null, started: {started} }})"))?;
    let _ = b.screenshot(&page, &shots.join("still-loading.png"));
    s.ok(
        "one of shell.js's imports refused: nothing at first, then \"Still loading… Reload\", the link to this same page, the shell not started",
        quiet && shown && after >= std::time::Duration::from_secs(8) && seen["said"] == "Still loading… Reload" && seen["link"] == home.as_str() && seen["started"] == false,
        json!({ "seen": seen, "afterMs": after.as_millis() as u64 }),
    );
    b.viewport(&page, 375, 812, true)?;
    let on_screen = format!(
        "innerWidth === 375 && document.documentElement.scrollWidth <= innerWidth && [{line}.firstChild, {line}.querySelector('a')].every((n) => {{ const r = document.createRange(); r.selectNodeContents(n); const b = r.getBoundingClientRect(); return b.width > 0 && b.left >= 0 && b.right <= innerWidth && b.top >= 0 && b.bottom <= innerHeight; }})"
    );
    let fits = b.until(&page, &on_screen, wait);
    let _ = b.screenshot(&page, &shots.join("phone-375-still-loading.png"));
    s.ok("on a phone, its words and its Reload are on screen", fits, b.eval(&page, &format!("{line}?.getBoundingClientRect().toJSON() ?? null"))?);
    b.unblock(&page)?;
    b.viewport(&page, 1280, 800, false)?;
    b.click(&page, "#still-loading a")?;
    let reloaded = b.until(&page, &format!("{started} && !{line}"), wait);
    s.ok("its Reload, the import let through now, loads the same page, and the shell starts", reloaded, b.eval(&page, "location.href")?);
    b.close(page)?;

    // its scripts came, and the first thing it asks (who is signed in) is
    // never answered: the first document's ask alone, so its Reload is not
    let hold_me = "if (window === window.top && !sessionStorage.getItem('e2e-held-me')) { sessionStorage.setItem('e2e-held-me', '1'); \
        const real = window.fetch.bind(window); window.__heldMe = false; \
        window.fetch = (url, o) => { if (String(url) === '/api/identities/me') { window.__heldMe = true; return new Promise(() => {}); } return real(url, o); }; }";
    let page = b.open_with_script(&home, hold_me)?;
    b.viewport(&page, 1280, 800, false)?;
    let t0 = std::time::Instant::now();
    let quiet = b.until(&page, &format!("window.__heldMe === true && !!{line} && getComputedStyle({line}).visibility === 'hidden'"), wait);
    let shown = b.until(&page, &format!("!!{line} && getComputedStyle({line}).visibility === 'visible'"), wait);
    let after = t0.elapsed();
    let seen = b.eval(&page, &format!("({{ said: {line}?.textContent ?? null, started: {started}, held: window.__heldMe }})"))?;
    s.ok(
        "its scripts come but who is signed in is never answered: nothing at first, then \"Still loading… Reload\", nothing of the shell drawn",
        quiet && shown && after >= std::time::Duration::from_secs(8) && seen["said"] == "Still loading… Reload" && seen["started"] == false,
        json!({ "seen": seen, "afterMs": after.as_millis() as u64 }),
    );
    // (a press, as a script's: with no line, a FAIL here, the checks after it made)
    b.eval(&page, "(document.querySelector('#still-loading a')?.click(), true)")?;
    let reloaded = b.until(&page, &format!("{started} && !{line}"), wait);
    s.ok("its Reload, answered this time, draws the shell, and the line is gone", reloaded, b.eval(&page, "location.href")?);
    b.close(page)?;
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

/// Settings' build prompt after a Copy: the line under it, when it shows
/// (`said`), its button's label, and whether the prompt has the focus and
/// is selected whole.
const COPY_SEEN: &str = "(() => { const p = document.getElementById('build-prompt'); const said = document.getElementById('build-prompt-said'); \
    return { said: said && !said.hidden && said.checkVisibility() ? said.textContent : null, button: document.querySelector('#settings-page [data-action=copy-prompt]')?.textContent ?? null, \
      focused: !!p && document.activeElement === p, selected: !!p && p.value.length > 0 && p.selectionStart === 0 && p.selectionEnd === p.value.length }; })()";

/// Settings' Account section's Privacy and Terms: whether they end it, and
/// their links (text, href, target) and words.
const LEGAL_LINKS_IN_ACCOUNT: &str = "(() => { const s = [...document.querySelectorAll('#settings-page .settings-section')].find((s) => s.querySelector(':scope > h2')?.textContent === 'Account'); \
    const p = s?.querySelector('.legal-links'); return { last: !!p && s.lastElementChild === p, links: [...(p?.querySelectorAll('a') ?? [])].map((a) => [a.textContent, a.getAttribute('href'), a.target]), text: p?.textContent ?? null }; })()";

/// Settings' last line (shell.js `releaseLine`): the release it names, its
/// words, its link (href, target), its time (seconds, from `datetime`) and
/// the page's deployed meta, whether it ends Settings, and the Computer
/// section's Version.
const RELEASE_LINE: &str = "(() => { const p = document.querySelector('#settings-page .settings-release'); const a = p?.querySelector('a'); const t = p?.querySelector('time'); \
    const v = [...document.querySelectorAll('#settings-page .settings-line')].find((l) => l.querySelector('.settings-key')?.textContent === 'Version'); \
    return p && { release: p.dataset.release, text: p.textContent, link: a && [a.textContent, a.getAttribute('href'), a.target], at: t && Date.parse(t.dateTime) / 1000, said: t?.textContent ?? null, \
      meta: document.querySelector('meta[name=fragment-release-deployed]')?.content ?? null, last: p.parentElement.lastElementChild === p, version: v?.querySelector('.settings-value')?.textContent ?? null }; })()";

/// Settings end with the release this page is, as the node reports it
/// (`x-fragment-deploy`), when it was deployed and the version its
/// computer runs (shell.js `releaseLine`), small and muted. Valid: the
/// node's own release (`dev` here, no commit: no link) and its time;
/// a commit's id (the release cookie naming one, as a deploy would) links
/// to that commit on GitHub, in a tab of its own. The browser goes back to
/// the node's own release, settings loaded, whatever happened.
fn release_line_ui(s: &mut Suite, api: &Api, b: &mut Browser, page: &Page) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let own = api.unsigned("HEAD", "/healthz", None)?.header("x-fragment-deploy");
    let checked = release_line_steps(s, api, b, page, &own, wait);
    b.set_cookie(&format!("{}/", api.base), RELEASE_COOKIE, "")?;
    b.reload(page)?;
    let back = b.until(page, &format!("document.querySelector('#settings-page .settings-release')?.dataset.release === {}", js(&own)), wait);
    anyhow::ensure!(back, "settings, back on the node's own release ({own}): {}", b.eval(page, RELEASE_LINE)?);
    checked
}

fn release_line_steps(s: &mut Suite, api: &Api, b: &mut Browser, page: &Page, own: &str, wait: std::time::Duration) -> Result<()> {
    let shown = |b: &mut Browser, id: &str| -> Result<Value> {
        b.until(page, &format!("document.querySelector('#settings-page .settings-release')?.dataset.release === {}", js(id)), wait);
        b.eval(page, RELEASE_LINE)
    };
    let line = shown(b, own)?;
    let is_commit = (7..=40).contains(&own.len()) && own.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c));
    let link = if is_commit { json!([own, format!("https://github.com/finitecomputer/fragment/commit/{own}"), "_blank"]) } else { Value::Null };
    let version = line["version"].as_str().unwrap_or("");
    let said = line["said"].as_str().unwrap_or("");
    s.ok(
        "Settings end with the release the node reports, when it was deployed, and the version its computer runs: one small line, its commit (none here) a link",
        line["release"] == own
            && line["link"] == link
            && line["last"] == true
            && line["meta"].as_str().and_then(|m| m.parse::<f64>().ok()).is_some_and(|m| line["at"].as_f64() == Some(m))
            && !said.is_empty()
            && !version.is_empty()
            && line["text"] == format!("Release {own} · {said} · Computer {version}").as_str(),
        &line,
    );
    // a deploy's commit, to this browser: its id links to it
    let commit = "0377c876abcd";
    b.set_cookie(&format!("{}/", api.base), RELEASE_COOKIE, commit)?;
    b.reload(page)?;
    let line = shown(b, commit)?;
    s.ok(
        "a release that is a commit links to it on GitHub, in a tab of its own",
        line["release"] == commit
            && line["link"] == json!([commit, format!("https://github.com/finitecomputer/fragment/commit/{commit}"), "_blank"])
            && line["text"].as_str().is_some_and(|t| t.starts_with(&format!("Release {commit} · "))),
        &line,
    );
    Ok(())
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
    let shared = api.share(chat, &other_id, "editor")?;
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
    // A look at a page between documents finds no context to run in (a
    // page just opened, or one navigating), and a look at the old document
    // can pass for the new one (the welcome, loaded again): the page opened
    // is waited for, and each load marks the document it leaves and waits
    // for a new one, ready
    anyhow::ensure!(b.until(&page, welcome, wait), "a guest's / opens on the welcome");
    let go = |b: &mut Browser, path: &str, ready: &str| -> Result<bool> {
        b.eval(&page, &format!("(window.__left = true, location.assign({}), true)", js(path)))?;
        Ok(b.until(&page, &format!("!window.__left && {ready}"), wait))
    };
    // on a phone, from 320 px to 430 px
    let mut bad: Vec<Value> = Vec::new();
    for w in [320u32, 375, 430] {
        b.viewport(&page, w, 812, true)?;
        for (place, path, ready) in [("welcome", "/welcome", welcome), ("settings", "/settings", settings)] {
            let shown = go(b, path, ready)?;
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
    let welcomed = go(b, "/", welcome)?;
    let seen = b.eval(&page, NO_SEAT_SEEN)?;
    s.ok(
        "a guest's welcome says which email is signed in, in bold, that it has no seat, and offers Use another account: a form posting to /auth/switch, back to the welcome",
        welcomed && seen["said"] == json!([said]) && seen["bold"] == json!([guest]) && seen["switches"] == json!([["/auth/switch?return=%2Fwelcome", "post", "Use another account"]]),
        &seen,
    );
    let _ = b.screenshot(&page, &shots.join("guest-welcome.png"));
    // settings: Billing's guest state, then Account
    let still = "document.getAnimations().every((a) => a.playState !== 'running' || a.effect?.getComputedTiming().iterations === Infinity)";
    let settled = go(b, "/settings", &format!("{settings} && {still}"))?;
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
/// it once, the restart's own save failing too, and the notice then says,
/// calmly, since when changes on it were not kept (its agent answered a
/// message after its last save); OK tells it no more, here or after a
/// reload; and settings' Restart computer asks once more, then restarts
/// it, saved.
fn computer_ui(s: &mut Suite, api: &Api, b: &mut Browser, page: &Page, session: &str, chat: &str, shots: &std::path::Path) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let computers = shell(api, session, "GET", "/api/computers", None, &[])?;
    let id = computers.body["computers"][0]["computer"].as_str().unwrap_or("").to_string();
    let view = || shell(api, session, "GET", &format!("/api/computers/{id}"), None, &[]).map(|r| r.body).unwrap_or(Value::Null);
    let shown = |kind: &str| format!("document.querySelector('#computer-notices:not([hidden]) .computer-notice[data-kind={kind}]')");
    // its saves fail, and its agent answers a message: work no save keeps
    // (a stop that lost nothing would tell nothing)
    std::thread::sleep(super::computers::QUEUE_DRAIN);
    let failing = super::computers::lever_with(api, &id, json!({ "op": "fail-saves", "times": 50 }))?;
    let said = "one more thing before the restart";
    let posted = shell(api, session, "POST", &format!("/api/f/{chat}/channels/chat"), Some(&json!({ "id": "computer-ui-work", "body": { "text": said } })), &[])?;
    let after = posted.body["record"]["seq"].as_i64().unwrap_or(0);
    let echoed = || {
        shell(api, session, "GET", &format!("/api/f/{chat}/channels/chat?after={after}&limit=1000"), None, &[])
            .ok()
            .and_then(|r| r.body["records"].as_array().cloned())
            .unwrap_or_default()
            .iter()
            .any(|r| r["body"]["text"].as_str().is_some_and(|t| t.starts_with("echo:") && t.contains(said)))
    };
    let answered = s.eventually(std::time::Duration::from_secs(60), echoed);
    s.eventually(std::time::Duration::from_secs(15), || super::computers::lever(api, &id, "saves").is_ok_and(|r| r.body["keepalives"] == 0));
    // its owner's sleep keeps it awake, and tells them
    let r = shell(api, session, "POST", &format!("/api/computers/{id}/sleep"), Some(&json!({})), &[])?;
    let generation = r.body["generation"].as_u64().unwrap_or(0);
    let warned = b.until(page, &format!("{n}?.innerText.includes(\"can't save\") && !!{n}.querySelector('button[data-action=restart]')", n = shown("unsaved")), wait);
    let said = b.eval(page, "document.getElementById('computer-notices').innerText")?;
    let _ = b.screenshot(page, &shots.join("computer-unsaved.png"));
    s.ok(
        "a sleep whose save fails is shown at once, with no reload: since when its work is unsaved, when it stops, and Restart",
        failing.status == 200 && answered && r.body["phase"] == "awake" && warned && said.as_str().is_some_and(|t| t.contains("hasn't saved since") && t.contains("If it still can't by")),
        &said,
    );
    // Restart, in the notice: its save fails too, so it goes back, and says so
    b.eval(page, &format!("({}.querySelector('button[data-action=restart]').click(), true)", shown("unsaved")))?;
    let back = b.until(page, &format!("{n}?.innerText.includes('Your computer restarted') && !!{n}.querySelector('button[data-action=seen]')", n = shown("went_back")), std::time::Duration::from_secs(90));
    let v = view();
    let said = b.eval(page, "document.getElementById('computer-notices').innerText")?;
    let _ = b.screenshot(page, &shots.join("computer-went-back.png"));
    s.ok(
        "Restart in the notice restarts it once, back to its last save, and the notice says, calmly, since when changes on it weren't kept, with an OK",
        back && v["generation"].as_u64() == Some(generation + 1)
            && v["restored"]["rollback"] == true
            && said.as_str().is_some_and(|t| t.contains("Changes on it since") && t.contains("weren't kept") && t.contains("Your chats are all here") && !t.contains("unexpectedly")),
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
/// /api/computers`) failing as a blip's do (counted: `__computerReads`):
/// every one while `window.__failComputers` is `true` (its default), or
/// as many as it counts. With `__holdMakes` set before it, a first run's
/// makes (`POST /api/fragments`, `POST /api/computers`) are held for good
/// (counted: `__makes`), so nothing is made, and so is a wake; with
/// `__holdMembers`, its read of a chat's members waits for
/// `__membersHeld()`, then answers none. Whether it ever said "Making your
/// agent…" (`__makingSaid`), and at which read (`__makingAt`). Its top
/// window's alone.
const FAIL_COMPUTERS: &str = "(() => { if (window !== window.top || window.__computerReads !== undefined) return true; \
    const real = window.fetch.bind(window); Object.assign(window, { __failComputers: window.__failComputers ?? true, __computerReads: 0, __makes: 0, __makingSaid: false }); \
    window.fetch = async (url, o) => { \
      if (url === '/api/computers' && o?.method === 'GET') { window.__computerReads++; const fail = window.__failComputers; \
        if (typeof fail === 'number') window.__failComputers = Math.max(0, fail - 1); \
        if (fail) throw new TypeError('Failed to fetch'); } \
      if (window.__holdMembers && o?.method === 'GET' && /^\\/api\\/f\\/[^/]+\\/members$/.test(url)) { await new Promise((r) => { window.__membersHeld = r; }); window.__membersHeld = null; \
        return new Response('{\"members\":[]}', { status: 200, headers: { 'content-type': 'application/json' } }); } \
      if (window.__holdMakes && o?.method === 'POST' && (url === '/api/fragments' || url.startsWith('/api/computers'))) { if (!url.endsWith('/wake')) window.__makes++; await new Promise(() => {}); } \
      return real(url, o); }; \
    new MutationObserver(() => { if (!window.__makingSaid && document.getElementById('notice')?.textContent.includes('Making your agent')) Object.assign(window, { __makingSaid: true, __makingAt: window.__computerReads }); }) \
      .observe(document, { subtree: true, childList: true, characterData: true }); \
    return true; })()";
/// The band's line for a read of the computer that failed, its Try again ready.
const REFRESH_LINE: &str = "(() => { const r = document.querySelector('#computer-notices:not([hidden]) .computer-notice[data-kind=refresh]'); \
    return !!r && r.innerText.includes(\"Couldn't refresh your computer\") && !!r.querySelector('button[data-action=refresh]:enabled'); })()";
/// The band says nothing of a failed read.
const NO_REFRESH_LINE: &str = "!document.querySelector('#computer-notices .computer-notice[data-kind=refresh]')";
/// The sidebar's Settings pressed, as its own click: a page just opened
/// and resized from the browser's own window may still be moving its
/// sidebar, and a press read from where the button was can miss it (run
/// 39689501852: Settings never opened). What these checks press it for is
/// the move to Settings, not where the button sits.
const TO_SETTINGS: &str = "(document.getElementById('settings').click(), true)";
/// Try again pressed, the computer's reads let through.
const READ_AGAIN: &str = "(window.__failComputers = false, document.querySelector('#computer-notices [data-action=refresh]')?.click(), true)";
/// What the page made and said, under `FAIL_COMPUTERS`.
const MADE: &str = "({ path: location.pathname, notice: document.getElementById('notice').hidden ? null : document.getElementById('notice').innerText, band: document.getElementById('computer-notices').innerText, \
    makingSaid: window.__makingSaid, makingAt: window.__makingAt ?? null, makes: window.__makes, reads: window.__computerReads })";
/// Settings' Computer section, as shown (null before it is).
const COMPUTER_SECTION: &str = "[...document.querySelectorAll('#settings-page h2')].find((h) => h.textContent === 'Computer')?.parentElement.innerText ?? null";

/// The shell at `path` under `FAIL_COMPUTERS` from its first document,
/// its first run's makes held, after `before` (a script setting it).
fn open_failing(b: &mut Browser, api: &Api, path: &str, before: &str) -> Result<Page> {
    let page = b.open_with_script(&format!("{}{path}", api.base), &format!("window.__holdMakes = true; {before} {FAIL_COMPUTERS}"))?;
    b.viewport(&page, 1280, 800, false)?;
    Ok(page)
}

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

/// The shell's chat column with a chat open, and the band's line.
const CHAT_AND_LINE: &str = "!document.getElementById('layout').hidden && !!document.querySelector('#frames iframe:not([hidden])') && ";

/// A read of the computer that fails from the page's start (#437: someone
/// with an agent was sent to "Making your agent…", their agents unknown
/// taken for none), every read of it failing and a first run's makes held
/// (`open_failing`), for someone with an agent (the browser's person). At
/// home: their chat and the line, and no first run; Try again knows their
/// agent and makes nothing. At Settings: its Computer section says it
/// couldn't be loaded, never that their computer starts with their first
/// agent; Try again shows it there.
fn refresh_failed_start_ui(s: &mut Suite, api: &Api, b: &mut Browser, shots: &std::path::Path) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let settle = std::time::Duration::from_secs(2);
    let chat_open = format!("{CHAT_AND_LINE}{REFRESH_LINE}");

    let page = open_failing(b, api, "/", "")?;
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

    // at Settings: what is true of their computer, then the computer itself
    let page = open_failing(b, api, "/settings", "")?;
    let loaded = b.until(&page, &format!("location.pathname === '/settings' && ({COMPUTER_SECTION}) !== null && {REFRESH_LINE}"), wait);
    let said = b.eval(&page, COMPUTER_SECTION)?;
    let _ = b.screenshot(&page, &shots.join("settings-refresh-failed.png"));
    s.ok(
        "their computer unread, Settings' Computer section says it couldn't be loaded, never that it starts with their first agent, under the line",
        loaded && said.as_str().is_some_and(|t| t.contains("Couldn't load your computer") && !t.contains("starts with your first agent")),
        &said,
    );
    b.eval(&page, READ_AGAIN)?;
    let shown = b.until(&page, &format!("{NO_REFRESH_LINE} && location.pathname === '/settings' && !!document.querySelector('#settings-page [data-action=restart-ask]')"), wait);
    let said = b.eval(&page, COMPUTER_SECTION)?;
    let made = b.eval(&page, MADE)?;
    s.ok(
        "Try again there shows their computer in it (its state and Restart), at Settings still, and makes nothing",
        shown && said.as_str().is_some_and(|t| t.contains("State") && !t.contains("Couldn't load")) && made["makingSaid"] == false && made["makes"] == 0,
        json!({ "said": said, "made": made }),
    );
    b.close(page)?;
    Ok(())
}

/// A first run that a failed read of the computer put off, or did not,
/// for a new person (#437; Alex's review of #459), every first-run make
/// held (`open_failing`). With no chats, their first run starts all the
/// same, and Try again during it starts no other. With only the chat an
/// earlier try made (no agent yet), that chat and the line and no first
/// run, until a read answers: the list socket's own, with nothing pressed,
/// or Try again's; then it goes on there. Answered at Settings, nothing
/// starts there, and coming home goes on with it, with no other read or
/// change to set it off. With their agent on their computer but not in
/// its chat, a move to Settings while Try again asks that chat's members
/// starts nothing there (the address and Settings stay); coming home goes
/// on with it, the same way.
fn first_run_put_off_ui(s: &mut Suite, api: &Api, b: &mut Browser, shots: &std::path::Path) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let settle = std::time::Duration::from_secs(2);
    let chat_open = format!("{CHAT_AND_LINE}{REFRESH_LINE}");
    let session = api.sign_in(&format!("shell-ui-unread-{}@e2e.test", crate::api::now_s()))?;
    b.set_cookie(&format!("{}/", api.base), "fragment_session", &session)?;
    let page = open_failing(b, api, "/", "")?;
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

    // the chat an earlier try made, and no agent yet; the page's first read
    // fails, and the list socket's (its opening reads the list) answers
    let r = shell(api, &session, "POST", "/api/fragments", Some(&json!({ "label": "basil-chat", "template": "chat", "title": FIRST_CHAT })), &[])?;
    anyhow::ensure!(r.status == 200, "making an earlier try's chat: {r}");
    let chat = r.body["name"].as_str().unwrap_or("").to_string();
    let card = |s: &Suite, b: &mut Browser, page: &Page| s.eventually(wait, || b.eval_in_frame(page, &chat, "!!document.querySelector('.setup-card')").ok() == Some(Value::Bool(true)));
    let page = open_failing(b, api, "/", "window.__failComputers = 1;")?;
    let on = b.until(&page, &format!("window.__computerReads >= 2 && window.__makingAt >= 2 && window.__makes > 0 && {NO_REFRESH_LINE}"), wait);
    let carded = card(s, b, &page);
    let made = b.eval(&page, MADE)?;
    s.ok(
        "with only the chat an earlier try made, its first read failed: once the list socket's read answers, with nothing pressed, the first run goes on in that chat (its card)",
        on && carded,
        &made,
    );
    b.close(page)?;

    // every read failing until Try again
    let page = open_failing(b, api, "/", "")?;
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
    let carded = card(s, b, &page);
    let _ = b.screenshot(&page, &shots.join("start-refresh-failed-earlier-try.png"));
    let framed = b.eval_in_frame(&page, &chat, "({ text: document.body.innerText.slice(0, 300), hidden: document.hidden })").unwrap_or_else(|e| json!(e.to_string()));
    s.ok("Try again reads it, finds no agent, and the first run goes on in that chat (its card)", on && carded, json!({ "made": b.eval(&page, MADE)?, "chat": framed }));
    b.close(page)?;

    // every read failing, the person goes to Settings and Try again answers
    // there: nothing starts there, and their way back home goes on with it,
    // with no read of its own: said with no read before it but the first
    // run's own, which starts in the same task as its "Making your agent…",
    // before the page's observer hears it (a read that set it off would be
    // a second)
    let page = open_failing(b, api, "/", "")?;
    let shown = b.until(&page, &chat_open, wait);
    b.eval(&page, TO_SETTINGS)?;
    let settings = b.until(&page, &format!("location.pathname === '/settings' && ({COMPUTER_SECTION}) !== null && {REFRESH_LINE}"), wait);
    b.eval(&page, READ_AGAIN)?;
    let read = b.until(&page, &format!("{NO_REFRESH_LINE} && location.pathname === '/settings'"), wait);
    std::thread::sleep(settle);
    let there = b.eval(&page, MADE)?;
    b.click(&page, "#chats .chat-row")?;
    let own_read = there["reads"].as_i64().unwrap_or(-2) + 1;
    let on = b.until(&page, &format!("location.pathname === '/' && window.__makingSaid === true && window.__makingAt <= {own_read} && window.__makes > 0"), wait);
    s.ok(
        "with only that chat, Try again answering at Settings starts nothing there; back home, the first run goes on, with no other read",
        shown && settings && read && there["makingSaid"] == false && there["makes"] == 0 && on,
        json!({ "there": there, "home": b.eval(&page, MADE)? }),
    );
    b.close(page)?;

    // their agent on their computer (asleep), not yet in its chat: Try
    // again asks that chat's members, held, while the person goes to Settings
    let agent = shell(api, &session, "POST", "/api/fragments", Some(&json!({ "label": "basil", "template": "agent", "title": "Basil" })), &[])?;
    let computer = shell(api, &session, "POST", "/api/computers", Some(&json!({})), &[])?;
    let (agent, computer) = (agent.body["name"].as_str().unwrap_or("").to_string(), computer.body["computer"].as_str().unwrap_or("").to_string());
    let r = shell(api, &session, "PUT", &format!("/api/computers/{computer}/agents/{agent}"), None, &[])?;
    anyhow::ensure!(r.status == 200 && !agent.is_empty(), "assigning the agent: {r}");
    let page = open_failing(b, api, "/", "")?;
    let shown = b.until(&page, &chat_open, wait);
    b.eval(&page, &format!("(window.__holdMembers = true, {READ_AGAIN})"))?;
    let asked = b.until(&page, "typeof window.__membersHeld === 'function'", wait);
    b.eval(&page, TO_SETTINGS)?;
    let settings = b.until(&page, "location.pathname === '/settings' && !document.getElementById('settings-page').hidden", wait);
    b.eval(&page, "(window.__holdMembers = false, window.__membersHeld?.(), true)")?;
    std::thread::sleep(settle);
    let made = b.eval(&page, MADE)?;
    let stayed = b.eval(&page, "location.pathname === '/settings' && !document.getElementById('settings-page').hidden && document.getElementById('layout').classList.contains('settings-view')")?;
    s.ok(
        "with their agent not yet in its chat, a move to Settings while Try again asks its members starts nothing there: the address and Settings stay, nothing is made",
        shown && asked && settings && stayed == true && made["makingSaid"] == false && made["makes"] == 0,
        json!({ "made": made, "stayed": stayed }),
    );
    let own_read = made["reads"].as_i64().unwrap_or(-2) + 1;
    b.click(&page, "#chats .chat-row")?;
    let on = b.until(
        &page,
        &format!("location.pathname === '/' && document.getElementById('settings-page').hidden && window.__makingSaid === true && window.__makingAt <= {own_read} && window.__makes > 0"),
        wait,
    );
    s.ok("back home, it goes on with its first run, with no other read", on, b.eval(&page, MADE)?);
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
    // the person connects Google (as another account than their sign-in here),
    // and reading their connections tells their computer
    s.workos.connect(email, crate::SWAP_CONNECTION, true);
    s.workos.label(email, crate::SWAP_CONNECTION, "their-google@gmail.test");
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
    let shown = b.until(page, &format!("document.querySelectorAll('#settings-connections [data-provider]').length === {} && !!document.querySelector('{} [data-use]')", crate::SWAP_KEYS.len() + 5, row("perplexity")), wait);
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
    let accounts = b.eval(page, "[...document.querySelectorAll('#settings-connections [data-account]')].map((a) => [a.closest('[data-provider]').dataset.provider, a.textContent])")?;
    s.ok("a connection's row names the account it is connected to, and no other row names one", accounts == json!([[crate::SWAP_CONNECTION, "their-google@gmail.test"]]), &accounts);
    // each note: its row, its text, and whether it is the line under the
    // row's name (right after its head)
    const NOTES: &str = "[...document.querySelectorAll('#settings-connections .provider-note')].map((n) => [n.closest('[data-provider]').dataset.provider, n.textContent, n.previousElementSibling?.classList.contains('provider-head') ?? false, n.classList.contains('muted')])";
    let notes = b.eval(page, NOTES)?;
    s.ok(
        "a row whose catalog row has a note shows it under the provider's name, muted (Google's, connected); a row without one shows none",
        notes == json!([[crate::SWAP_CONNECTION, crate::SWAP_CONNECTION_NOTE, true, true]]),
        &notes,
    );
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
    let others: Vec<&str> = std::iter::once(crate::SWAP_CONNECTION).chain(crate::SWAP_KEYS.iter().map(|(k, _, _)| *k).filter(|k| *k != "perplexity")).chain([crate::SWAP_OWN, crate::SWAP_ROUTER, "e2e-oauth", crate::SEVERAL]).collect();
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
    // Disconnect: this row names no revocation endpoint, so settings says
    // where the person removes the access at the provider
    let google = row(crate::SWAP_CONNECTION);
    b.click(page, &format!("{google} [data-action=disconnect]"))?;
    let off = b.until(
        page,
        &format!("document.querySelector('{google}')?.dataset.state === 'not_connected' && !document.querySelector('{google} [data-action=disconnect]') && !document.getElementById('notice').hidden && document.getElementById('notice').innerText.includes('may still list')"),
        wait,
    );
    let said = b.eval(page, "document.getElementById('notice').innerText")?;
    s.ok("Disconnect takes a connection away at Pipes; settings shows it not connected, and, unrevoked, says where its access is removed at the provider", off && !s.workos.holds(email, crate::SWAP_CONNECTION), &said);
    let notes = b.eval(page, NOTES)?;
    s.ok("not connected, Google's row still shows its note under its name, and still no other row shows one", notes == json!([[crate::SWAP_CONNECTION, crate::SWAP_CONNECTION_NOTE, true, true]]), &notes);
    // connected again for the checks after, the computer told by a read: its
    // disconnect's fence lifted as a connect known to import lifts it (the
    // row has no `connection_oauth` here: the registry's lever)
    s.workos.connect(email, crate::SWAP_CONNECTION, true);
    let hook = api.unsigned("POST", "/api/test/registry", Some(&json!({ "signins": { "unfenceConnection": { "email": email, "provider": crate::SWAP_CONNECTION } } })))?;
    anyhow::ensure!(hook.status == 200, "lift the fence: {hook}");
    shell(api, session, "GET", "/api/connections", None, &[])?;
    partial_grant_ui(s, api, b, page, session, email)?;
    several_accounts_ui(s, api, b, page, session, email, &lead, &id)
}

/// A connection the person allowed only part of (Google's granular
/// consent: they unticked a box): its row says what they did not allow,
/// in words, with a Reconnect to add it, which goes through the provider's
/// page again; connected with all of it, the row says nothing is missing.
/// It is disconnected again for the checks after.
fn partial_grant_ui(s: &mut Suite, api: &Api, b: &mut Browser, page: &Page, session: &str, email: &str) -> Result<()> {
    const PARTIAL: &str = "e2e-oauth";
    let wait = std::time::Duration::from_secs(30);
    let row = format!("#settings-connections [data-provider={PARTIAL:?}]");
    s.workos.connect(email, PARTIAL, true);
    s.workos.grant(email, PARTIAL, &["mail.read", "openid"]);
    b.reload(page)?;
    let shown = b.until(page, &format!("document.querySelector('{row}')?.dataset.state === 'connected' && !!document.querySelector('{row} .provider-missing')"), wait);
    let said = b.eval(page, &format!("[document.querySelector('{row} .provider-missing')?.textContent ?? null, document.querySelector('{row} [data-action=reconnect]')?.textContent ?? null, document.querySelectorAll('#settings-connections .provider-missing').length]"))?;
    s.ok(
        "a connection the person allowed only part of says in its row what they did not allow, with a Reconnect to add it; no other row says any is missing",
        shown && said == json!(["Not allowed: files.read", "Reconnect to add", 1]),
        &said,
    );
    let windows = b.pages()?.len();
    b.click(page, &format!("{row} [data-action=reconnect]"))?;
    let callback = format!("/api/connections/{PARTIAL}/callback");
    let mut popup = None;
    let came_back = s.eventually(wait, || {
        popup = b.pages().ok().filter(|l| l.len() > windows).and_then(|l| l.into_iter().find(|(_, url)| url.contains(&callback)).map(|(id, _)| id));
        popup.is_some()
    });
    let connected = match &popup {
        Some(id) => {
            let p = b.attach(id)?;
            let connected = b.until(&p, "document.body?.innerText.includes('is connected')", wait);
            b.close(p)?;
            connected
        }
        None => false,
    };
    b.reload(page)?;
    let whole = b.until(page, &format!("document.querySelector('{row}')?.dataset.state === 'connected' && !!document.querySelector('{row} [data-action=disconnect]') && !document.querySelector('{row} .provider-missing') && !document.querySelector('{row} [data-action=reconnect]')"), wait);
    s.ok(
        "Reconnect to add goes through the provider's page again, and connected with all of it the row says nothing is missing and offers no reconnect",
        came_back && connected && whole && s.workos.scopes(email, PARTIAL) == ["mail.read", "files.read", "openid"],
        json!({ "cameBack": came_back, "connected": connected, "whole": whole, "scopes": s.workos.scopes(email, PARTIAL) }),
    );
    let off = shell(api, session, "DELETE", &format!("/api/connections/{PARTIAL}"), None, &[])?;
    anyhow::ensure!(off.status == 200, "disconnect {PARTIAL} after its checks: {off}");
    Ok(())
}

/// A press of `button` that opens the provider's page in a window of its
/// own, and that window back at `provider`'s callback saying it connected:
/// (came back, connected). The window is closed.
fn connected_in_window(s: &mut Suite, b: &mut Browser, page: &Page, button: &str, provider: &str) -> Result<(bool, bool)> {
    let wait = std::time::Duration::from_secs(30);
    let windows = b.pages()?.len();
    b.click(page, button)?;
    let callback = format!("/api/connections/{provider}/callback");
    let mut popup = None;
    let came_back = s.eventually(wait, || {
        popup = b.pages().ok().filter(|l| l.len() > windows).and_then(|l| l.into_iter().find(|(_, url)| url.contains(&callback)).map(|(id, _)| id));
        popup.is_some()
    });
    let connected = match &popup {
        Some(id) => {
            let p = b.attach(id)?;
            let connected = b.until(&p, "document.body?.innerText.includes('is connected')", wait);
            b.close(p)?;
            connected
        }
        None => false,
    };
    Ok((came_back, connected))
}

/// A connection that takes several accounts (`crate::SEVERAL`): its row
/// lists each account, with its label, state, what it was not allowed
/// (with a Reconnect to add) and its own Disconnect; Add another, through
/// the provider's page, adds one; with two, each agent allowed gets a
/// choice of which it uses, Default first, which persists; a saved choice
/// that needs reconnecting is shown as it is, and may be changed back to
/// Default; and one account disconnected leaves the other, with no choice
/// left to make. Its accounts are disconnected again for the checks after.
#[allow(clippy::too_many_arguments)]
fn several_accounts_ui(s: &mut Suite, api: &Api, b: &mut Browser, page: &Page, session: &str, email: &str, lead: &str, computer: &str) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let several = crate::SEVERAL;
    let row = format!("#settings-connections [data-provider={several:?}]");
    let pick = format!("{row} select.provider-account-pick[data-agent={lead:?}]");
    s.workos.connect(email, several, true);
    s.workos.label(email, several, "work@oauth.test");
    s.workos.grant(email, several, &["mail.read", "openid"]);
    b.reload(page)?;
    let shown = b.until(page, &format!("document.querySelectorAll('{row} .provider-account-row').length === 1"), wait);
    let one = b.eval(
        page,
        &format!(
            "[...document.querySelectorAll('{row} .provider-account-row')].map((a) => [a.querySelector('.provider-account')?.textContent, a.dataset.state, a.querySelector('.provider-missing')?.textContent ?? null, a.querySelector('[data-action=reconnect]')?.textContent ?? null, a.querySelector('[data-action=disconnect]')?.textContent ?? null]).concat([[document.querySelector('{row} [data-action=add-account]')?.textContent ?? null, document.querySelectorAll('{row} select').length]])"
        ),
    )?;
    s.ok(
        "a connection that takes several accounts lists each in its row: its label, state, what it was not allowed with a Reconnect to add, and its own Disconnect; the head offers Add another; with one account, no agent is asked which",
        shown && one == json!([["work@oauth.test", "connected", "Not allowed: files.read", "Reconnect to add", "Disconnect"], ["Add another E2e Several account", 0]]),
        &one,
    );
    let (came_back, connected) = connected_in_window(s, b, page, &format!("{row} [data-action=add-account]"), several)?;
    b.reload(page)?;
    let two = b.until(page, &format!("document.querySelectorAll('{row} .provider-account-row').length === 2 && !!document.querySelector('{pick}')"), wait);
    let held = s.workos.accounts(email, several);
    let ids: Vec<String> = held.iter().map(|a| a.id.clone()).collect();
    let choice = b.eval(page, &format!("((p) => [p?.getAttribute('aria-label') ?? null, p?.value ?? null, [...(p?.options ?? [])].map((o) => o.value)])(document.querySelector('{pick}'))"))?;
    s.ok(
        "Add another goes through the provider's page and adds an account; with two, each agent allowed is asked which it uses, Default (the oldest connected) chosen and first, then each account",
        came_back && connected && two && held.len() == 2 && choice[0].as_str().is_some_and(|l| l.ends_with("'s E2e Several account")) && choice[1] == "" && choice[2] == json!(["", ids[0], ids[1]]),
        json!({ "choice": choice, "held": ids }),
    );
    b.eval(page, &format!("((p) => (p.value = {:?}, p.dispatchEvent(new Event('change')), true))(document.querySelector('{pick}'))", ids[1]))?;
    let agent_uses = || -> Value {
        shell(api, session, "GET", &format!("/api/computers/{computer}"), None, &[])
            .ok()
            .and_then(|v| v.body["agents"].as_array().and_then(|l| l.iter().find(|a| a["fragment"] == lead)).map(|a| a["accounts"][several].clone()))
            .unwrap_or(Value::Null)
    };
    let stored = s.eventually(wait, || agent_uses() == ids[1].as_str());
    b.reload(page)?;
    let kept = b.until(page, &format!("document.querySelector('{pick}')?.value === {:?}", ids[1]), wait);
    s.ok("the agent's choice is its computer's, and settings shows it chosen when read again", stored && kept, json!({ "uses": agent_uses() }));
    // the account it chose comes to need reconnecting: shown as it is, never
    // as the other, and Default takes the agent back
    s.workos.authorize_account(email, several, &ids[1], false);
    b.reload(page)?;
    let shown = b.until(page, &format!("document.querySelector('{row} [data-account-id={:?}]')?.dataset.state === 'needs_reauthorization' && document.querySelector('{pick}')?.value === {:?}", ids[1], ids[1]), wait);
    let said = b.eval(page, &format!("((p) => p?.selectedOptions[0]?.textContent ?? null)(document.querySelector('{pick}'))"))?;
    s.ok(
        "a saved choice that needs reconnecting is shown as it is stored, saying so, with Default and the other account to change to",
        shown && said.as_str().is_some_and(|t| t.ends_with("(needs reconnecting)")),
        &said,
    );
    b.eval(page, &format!("((p) => (p.value = '', p.dispatchEvent(new Event('change')), true))(document.querySelector('{pick}'))"))?;
    let reset = s.eventually(wait, || agent_uses().is_null());
    b.reload(page)?;
    let default = b.until(page, &format!("document.querySelector('{pick}')?.value === ''"), wait);
    s.ok("Default takes the agent back to the oldest connected account: its choice cleared, and still offered with two accounts", reset && default, json!({ "uses": agent_uses() }));
    s.workos.authorize_account(email, several, &ids[1], true);
    b.reload(page)?;
    let off = format!("{row} [data-account-id={:?}] [data-action=disconnect]", ids[1]);
    anyhow::ensure!(b.until(page, &format!("document.querySelector('{row} [data-account-id={:?}]')?.dataset.state === 'connected' && !!document.querySelector('{off}')", ids[1]), wait), "{several}'s second account connected again in settings");
    b.click(page, &format!("{row} [data-account-id={:?}] [data-action=disconnect]", ids[1]))?;
    let left = b.until(page, &format!("document.querySelectorAll('{row} .provider-account-row').length === 1 && !document.querySelector('{pick}') && document.querySelector('{row}')?.dataset.state === 'connected'"), wait);
    let held = s.workos.accounts(email, several);
    s.ok(
        "an account's own Disconnect takes it alone away: the other stays connected, and with one left no agent is asked which",
        left && held.len() == 1 && held[0].id == ids[0],
        json!({ "held": held.iter().map(|a| a.id.clone()).collect::<Vec<_>>() }),
    );
    let cleared = shell(api, session, "PUT", &format!("/api/computers/{computer}/agents/{lead}/accounts/{several}"), Some(&json!({ "account": null })), &[])?;
    let off = shell(api, session, "DELETE", &format!("/api/connections/{several}"), None, &[])?;
    anyhow::ensure!(cleared.status == 200 && off.status == 200, "{several}'s accounts cleared after its checks: {cleared}; {off}");
    Ok(())
}

/// Two loads of settings at once (#441): the first one's read of the
/// connections answered (no key yet) and held, the person's own key saved,
/// and settings opened again, which show it set; the first then let go and
/// finished (`HELD_SETTLED`), the row still says set, and the page is still
/// the second load's: an older load never draws over a newer one. The key
/// is taken away again for the checks after.
fn settings_race_ui(s: &mut Suite, api: &Api, b: &mut Browser, page: &Page, session: &str) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let row = format!("#settings-connections [data-provider={:?}]", crate::SWAP_OWN);
    let state = format!("document.querySelector('{row}')?.dataset.state");
    anyhow::ensure!(b.until(page, &format!("location.pathname === '/settings' && {state} === 'not_set'"), wait), "settings show {} not set", crate::SWAP_OWN);
    b.eval(page, &hold("GET", "^/api/connections$", Hold::Answer))?;
    b.eval(page, "(document.getElementById('settings').click(), true)")?;
    let held = b.until(page, "!!window.__held.release", wait);
    let key = format!("/api/connections/{}/key", crate::SWAP_OWN);
    let saved = shell(api, session, "PUT", &key, Some(&json!({ "key": "mail-race-e2e-55aa" })), &[])?;
    b.eval(page, "(document.getElementById('settings').click(), true)")?;
    let newer = b.until(page, &format!("{state} === 'set'"), wait);
    b.eval(page, "(window.__held.newer = document.querySelector('#settings-page .settings-layout'), window.__held.release?.(), true)")?;
    let settled = b.until(page, HELD_SETTLED, wait);
    let after = b.eval(page, &format!("({{ state: {state} ?? null, same: document.querySelector('#settings-page .settings-layout') === window.__held.newer }})"))?;
    b.eval(page, "(window.__held.restore(), true)")?;
    s.ok(
        "two loads of settings at once: the older one, whose read of the connections said no key and answered last, draws nothing over the newer one; the row stays set",
        held && saved.status == 200 && newer && settled && after == json!({ "state": "set", "same": true }),
        json!({ "held": held, "saved": saved.status, "newer": newer, "settled": settled, "after": after }),
    );
    shell(api, session, "DELETE", &key, None, &[])?;
    b.reload(page)?;
    anyhow::ensure!(b.until(page, &format!("{state} === 'not_set'"), wait), "settings show {} not set again", crate::SWAP_OWN);
    Ok(())
}

/// The shell's next `method` call whose path matches `re` (a regular
/// expression's source) refused at once, as a failing network's is: a 503
/// with a message. Every other call, and every one after it, as it is.
fn refuse_once(method: &str, re: &str) -> String {
    format!(
        "(() => {{ const real = window.fetch; const re = new RegExp({}); window.fetch = async (url, o) => {{ if (o?.method === {} && re.test(String(url))) {{ window.fetch = real; \
         return new Response(JSON.stringify({{ error: 'unavailable', message: 'refused for the test' }}), {{ status: 503, headers: {{ 'content-type': 'application/json' }} }}); }} return real(url, o); }}; return true; }})()",
        js(re),
        js(method)
    )
}

/// The shell's notice (`#notice`) as the person sees it: shown, inside the
/// viewport, and nothing over it (the point at its middle is its own); its
/// words, and where the person is.
const NOTICE_SEEN: &str = "(() => { const n = document.getElementById('notice'); const r = n.getBoundingClientRect(); const at = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2); \
    const inside = r.width > 0 && r.height > 0 && r.top >= 0 && r.left >= 0 && r.bottom <= innerHeight && r.right <= innerWidth; \
    return { ok: n.checkVisibility() && inside && !!at && n.contains(at), text: n.innerText, box: [r.left, r.top, r.width, r.height].map(Math.round), width: innerWidth, height: innerHeight, \
      path: location.pathname, settings: !document.getElementById('settings-page').hidden }; })()";

/// On settings, an agent's chip under a connection whose save fails, then
/// pressed again: the failure is said over settings once they reload
/// ("That was not changed", in the viewport, the chip as it was; `shot`
/// taken then); the retry that works takes the provider from the agent and
/// the failure's notice away; a press more gives it back, for the checks
/// after. Whether the failure was said, and whether the retry cleared it,
/// with what was seen.
fn chip_retried(b: &mut Browser, page: &Page, chip: &str, shot: &std::path::Path, wait: std::time::Duration) -> Result<(bool, bool, Value)> {
    let pressed = |state: &str| format!("document.querySelector({})?.getAttribute('aria-pressed') === {}", js(chip), js(state));
    let press = format!("(document.querySelector({}).click(), true)", js(chip));
    b.eval(page, &refuse_once("PUT", "/agents/[^/]+/connections$"))?;
    b.eval(page, &press)?;
    let told = format!("(() => {{ const v = {NOTICE_SEEN}; return v.ok && v.text.startsWith('That was not changed') && v.path === '/settings' && v.settings && {}; }})()", pressed("true"));
    let refused = b.until(page, &told, wait);
    let failed = b.eval(page, NOTICE_SEEN)?;
    let _ = b.screenshot(page, shot);
    b.eval(page, &press)?;
    let retried = b.until(page, &format!("{} && document.getElementById('notice').hidden", pressed("false")), wait);
    let after = b.eval(page, NOTICE_SEEN)?;
    b.eval(page, &press)?;
    let back = b.until(page, &format!("{} && document.getElementById('notice').hidden", pressed("true")), wait);
    Ok((refused, retried && back, json!({ "failed": failed, "retried": after, "back": back })))
}

/// The shell's own notice where the person is (#436), never in a place
/// settings hide: on settings, the agent chip's failed save is said over
/// them, in the viewport, and a retry that works takes it away, on a
/// desktop and a phone (375 px); over a chat, an archive that failed is
/// said too; each place's notice is its own (the chat's clears for
/// settings, settings' for the chat), and a reload on settings shows no
/// Loading… left over the page.
fn notice_ui(s: &mut Suite, b: &mut Browser, page: &Page, chat: &str, shots: &std::path::Path) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let chip = "#settings-connections [data-provider=\"perplexity\"] [data-agent]";
    let (desk, retried, seen) = chip_retried(b, page, chip, &shots.join("desktop-settings-notice.png"), wait)?;
    s.ok("on settings, an agent's chip whose save fails says That was not changed over settings, in the viewport, on a desktop; the chip as it was", desk, &seen["failed"]);
    s.ok("pressed again, its save works: the chip says so, and the failure's notice is gone", retried, &seen);
    open_row(b, page, chat, wait)?;
    let left = b.eval(page, "document.getElementById('notice').hidden")?;
    b.eval(page, &refuse_once("PUT", "/archived$"))?;
    b.eval(page, "(document.getElementById('chat-options').click(), true)")?;
    let archive = b.until(page, "[...document.querySelectorAll('#menu button')].some((b) => b.textContent === 'Archive')", wait) && menu_item(b, page, "Archive")?;
    let told = format!("(() => {{ const v = {NOTICE_SEEN}; return v.ok && v.text.startsWith('It was not archived') && v.path === '/' && !v.settings; }})()");
    let in_chat = archive && b.until(page, &told, wait);
    let seen = b.eval(page, NOTICE_SEEN)?;
    s.ok("then a chat opened takes settings' notice away, and an archive that fails there says so over the chat, in the viewport", left == true && in_chat, json!({ "cleared": left, "seen": seen }));
    b.eval(page, "(document.getElementById('settings').click(), true)")?;
    let gone = b.until(page, &format!("location.pathname === '/settings' && !!document.querySelector({}) && document.getElementById('notice').hidden", js(chip)), wait);
    s.ok("and settings opened from there leave the chat's notice with the chat", gone, b.eval(page, NOTICE_SEEN)?);
    b.viewport(page, 375, 812, true)?;
    b.reload(page)?;
    let loaded = b.until(page, &format!("location.pathname === '/settings' && !!document.querySelector({}) && document.getElementById('notice').hidden", js(chip)), wait);
    let (phone, retried, seen) = chip_retried(b, page, chip, &shots.join("phone-375-settings-notice.png"), wait)?;
    s.ok(
        "on a phone (375 px), settings reloaded show no Loading… over them, the chip's failed save is said over settings, in the viewport, and a retry that works takes it away",
        loaded && phone && retried,
        json!({ "loaded": loaded, "seen": seen }),
    );
    b.viewport(page, 1280, 800, false)?;
    notice_late_ui(s, b, page, chat)
}

/// Where a held call waits: before it is sent (the platform has not had
/// it), or with the platform's answer in (it has done it).
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Hold {
    Request,
    Answer,
}

/// The shell's next `method` call whose path matches `re` (a regular
/// expression's source) held at `at` until `window.__held.release()`;
/// let go with `release('refuse')`, it is answered as a failing network's
/// is (a 503 with a message), never sent if it was held before it was.
/// Every call counted while it is under way (`inflight`), and when the
/// last one ended (`quiet`), so a check can wait for what the held call let
/// go to finish (`HELD_SETTLED`); `restore()` puts the page's fetch back.
pub(super) fn hold(method: &str, re: &str, at: Hold) -> String {
    format!(
        "(() => {{ const real = window.fetch; const re = new RegExp({}); const before = {}; let holding = true; \
         const held = window.__held = {{ release: null, inflight: 0, quiet: performance.now(), restore: () => {{ window.fetch = real; }} }}; \
         const wait = () => new Promise((go) => {{ held.release = go; }}); \
         const refused = () => new Response(JSON.stringify({{ error: 'unavailable', message: 'refused for the test' }}), {{ status: 503, headers: {{ 'content-type': 'application/json' }} }}); \
         window.fetch = async (url, o) => {{ const hold = holding && o?.method === {} && re.test(String(url)); if (hold) holding = false; held.inflight++; \
         try {{ if (hold && before && (await wait()) === 'refuse') return refused(); const r = await real(url, o); if (hold && !before && (await wait()) === 'refuse') return refused(); return r; }} \
         finally {{ held.inflight--; held.quiet = performance.now(); }} }}; return true; }})()",
        js(re),
        at == Hold::Request,
        js(method)
    )
}

/// Once a held call (`hold`) is let go: none of the page's calls under
/// way, and none for half a second.
pub(super) const HELD_SETTLED: &str = "window.__held.inflight === 0 && performance.now() - window.__held.quiet > 500";

/// What answers late is said only where it still belongs (#457 review):
/// an agent's chip whose save is refused, its reload's read held while a
/// chat opens, says nothing over the chat; held again while settings open
/// anew and the chip, pressed again, saves, the older reload let go brings
/// back no failure; a Copy that answers after a chip's failure is said
/// leaves that failure; and a save held, the chip pressed again from
/// settings opened anew (it saves), then the first refused, says nothing:
/// the change it asked for is there. The chip as it was, after.
fn notice_late_ui(s: &mut Suite, b: &mut Browser, page: &Page, chat: &str) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let chip = "#settings-connections [data-provider=\"perplexity\"] [data-agent]";
    let pressed = |v: &str| format!("document.querySelector({})?.getAttribute('aria-pressed') === {}", js(chip), js(v));
    let press = format!("(document.querySelector({}).click(), true)", js(chip));
    let fresh = format!("location.pathname === '/settings' && document.querySelector({})?.disabled === false", js(chip));
    let refuse = refuse_once("PUT", "/agents/[^/]+/connections$");
    let hidden = "document.getElementById('notice').hidden";
    let open_settings = "(document.getElementById('settings').click(), true)";
    anyhow::ensure!(b.until(page, &format!("{fresh} && {}", pressed("true")), wait), "settings, the chip pressed");

    // refused, its reload held, a chat opened, then let go
    b.eval(page, &hold("GET", "^/api/connections$", Hold::Answer))?;
    b.eval(page, &refuse)?;
    b.eval(page, &press)?;
    let held = b.until(page, "!!window.__held.release", wait);
    open_row(b, page, chat, wait)?;
    b.eval(page, "(window.__held.release?.(), true)")?;
    let settled = b.until(page, HELD_SETTLED, wait);
    let seen = b.eval(page, &format!("({{ hidden: {hidden}, path: location.pathname, settings: !document.getElementById('settings-page').hidden }})"))?;
    b.eval(page, "(window.__held.restore(), true)")?;
    s.ok(
        "a chip's refused save whose reload answers once a chat is open says nothing over the chat",
        held && settled && seen == json!({ "hidden": true, "path": "/", "settings": false }),
        &seen,
    );

    // refused, its reload held; settings opened anew, the chip saved; then let go
    b.eval(page, open_settings)?;
    anyhow::ensure!(b.until(page, &format!("{fresh} && {}", pressed("true")), wait), "settings again, the chip pressed");
    b.eval(page, &hold("GET", "^/api/connections$", Hold::Answer))?;
    b.eval(page, &refuse)?;
    b.eval(page, &press)?;
    let held = b.until(page, "!!window.__held.release", wait);
    b.eval(page, open_settings)?;
    let newer = b.until(page, &format!("{fresh} && {}", pressed("true")), wait);
    b.eval(page, &press)?;
    let saved = b.until(page, &format!("{fresh} && {} && {hidden}", pressed("false")), wait);
    b.eval(page, "(window.__held.release?.(), true)")?;
    let settled = b.until(page, HELD_SETTLED, wait);
    let after = b.eval(page, NOTICE_SEEN)?;
    b.eval(page, "(window.__held.restore(), true)")?;
    s.ok(
        "a chip's refused save whose reload answers after settings opened anew and the chip saved there brings back no failure",
        held && newer && saved && settled && b.eval(page, hidden)? == true,
        &after,
    );
    // (an older load may have drawn the chip as it was: settings read again)
    b.reload(page)?;
    anyhow::ensure!(b.until(page, &format!("{fresh} && {}", pressed("false")), wait), "settings reloaded, the chip not pressed");
    b.eval(page, &press)?;
    anyhow::ensure!(b.until(page, &format!("{fresh} && {}", pressed("true")), wait), "the chip pressed again");

    // a Copy held; a chip's failure said; then copied
    b.eval(page, "(window.__copy = null, navigator.clipboard.writeText = (text) => new Promise((done) => { window.__copy = () => { window.__copied = text; done(); }; }), document.querySelector('#settings-page [data-action=copy-prompt]').click(), true)")?;
    let copying = b.until(page, "typeof window.__copy === 'function'", wait);
    b.eval(page, &refuse)?;
    b.eval(page, &press)?;
    let told = format!("(() => {{ const v = {NOTICE_SEEN}; return v.ok && v.text.startsWith('That was not changed') && v.settings; }})()");
    let failed = b.until(page, &format!("{told} && {fresh}"), wait);
    b.eval(page, "(window.__copied = null, window.__copy(), true)")?;
    let copied = b.until(page, "window.__copied !== null", wait);
    let kept = b.eval(page, &told)?;
    b.eval(page, "(navigator.clipboard.writeText = async (text) => { window.__copied = text; }, true)")?;
    s.ok(
        "a Copy that answers after a chip's failure is said leaves that failure said",
        copying && failed && copied && kept == true,
        b.eval(page, NOTICE_SEEN)?,
    );

    // a save held; settings opened anew, the chip saved there; the first refused
    b.eval(page, &hold("PUT", "/agents/[^/]+/connections$", Hold::Request))?;
    b.eval(page, &press)?;
    let held = b.until(page, "!!window.__held.release", wait);
    b.eval(page, open_settings)?;
    let newer = b.until(page, &format!("{fresh} && {}", pressed("true")), wait);
    b.eval(page, &press)?;
    let saved = b.until(page, &format!("{fresh} && {} && {hidden}", pressed("false")), wait);
    b.eval(page, "(window.__held.release?.('refuse'), true)")?;
    let settled = b.until(page, HELD_SETTLED, wait);
    let quiet = b.eval(page, &format!("{hidden} && {}", pressed("false")))?;
    b.eval(page, "(window.__held.restore(), true)")?;
    s.ok(
        "a chip's save refused after the chip, pressed again from settings opened anew, saved the same change says nothing: the change is there",
        held && newer && saved && settled && quiet == true,
        b.eval(page, NOTICE_SEEN)?,
    );
    b.eval(page, &press)?;
    anyhow::ensure!(b.until(page, &format!("{fresh} && {}", pressed("true")), wait), "the chip as it was");
    Ok(())
}

/// How hard an agent thinks (Paul, 2026-10-22: "expose the reasoning level
/// in the model picker as an additional dropdown (while showing that our
/// default is high)"): every agent's settings offer it beside its model,
/// with no provider connected too, Default (High) shown while its
/// agent.json names none. Picked, it is agent.json's `reasoning`, every
/// other setting kept (its tier, its model, its own), which its computer
/// runs its next turn at; shown again as picked after a reload; the
/// default again takes it away. Invalid: a pick whose read of agent.json
/// fails writes nothing and says so (Alex's review of #506: it wrote a
/// fresh file, losing the model); none there (404) is written fresh.
/// The shell at Settings, its computers' view with the tiers' labels named
/// in `drop` taken away from its first document: a deployment that labels
/// fewer routes than the node's two.
fn open_with_fewer_labels(b: &mut Browser, api: &Api, drop: &[&str]) -> Result<Page> {
    let script = format!(
        "(() => {{ if (window !== window.top) return true; const real = window.fetch.bind(window); const drop = {}; \
         window.fetch = async (url, o) => {{ const r = await real(url, o); if (url !== '/api/computers' || o?.method !== 'GET' || !r.ok) return r; \
           const v = await r.clone().json(); for (const c of v.computers ?? []) for (const t of drop) if (c.models?.routes?.[t]) delete c.models.routes[t].label; \
           return new Response(JSON.stringify(v), {{ status: r.status, headers: {{ 'content-type': 'application/json' }} }}); }}; return true; }})()",
        serde_json::to_string(drop).expect("names")
    );
    let page = b.open_with_script(&format!("{}/settings", api.base), &script)?;
    b.viewport(&page, 1280, 800, false)?;
    Ok(page)
}

/// Alex's review of #586 (P2): with a single labelled route and no provider
/// connected, an agent on the other tier still has a choice, its own tier
/// or the route, so Settings offers it; one on the labelled route's own
/// tier has none, and no pick is shown; none labelled, none, as before.
fn one_route_ui(s: &mut Suite, api: &Api, b: &mut Browser, session: &str) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let ready = "document.querySelector('[data-reasoning-for]')?.dataset.read === '1'";
    let lead_of = "document.querySelector('[data-reasoning-for]')?.dataset.reasoningFor ?? ''";
    // what Settings shows of the lead's model pick: none, hidden, or its options and its value
    let shown = |b: &mut Browser, page: &Page, lead: &str| -> Result<Value> {
        b.until(page, &format!("!document.querySelector('[data-model-for={lead:?}]') || document.querySelector('[data-model-for={lead:?}]').dataset.read === '1'"), wait);
        b.eval(page, &format!("(() => {{ const p = document.querySelector('[data-model-for={lead:?}]'); if (!p) return null; if (p.parentElement.hidden) return 'hidden'; return {{ value: p.value, options: [...p.options].map((o) => [o.value, o.textContent]) }}; }})()"))
    };
    let probe = open_with_fewer_labels(b, api, &["medium"])?;
    b.until(&probe, ready, wait);
    let lead = b.eval(&probe, lead_of)?.as_str().unwrap_or("").to_string();
    let file = format!("/api/f/{lead}/file?path=agent.json");
    let agent_json = || shell(api, session, "GET", &file, None, &[]).map(|r| r.body).unwrap_or(Value::Null);
    let write_tier = |tier: &str| shell(api, session, "POST", &format!("/api/f/{lead}/files"), Some(&json!({ "message": "the e2e's tier", "files": [{ "path": "agent.json", "text": format!("{:#}\n", json!({ "tier": tier })) }] })), &[]).map(|r| r.status);
    let view = shell(api, session, "GET", "/api/computers", None, &[])?.body["computers"][0]["models"].clone();
    let (cheap, medium) = (view["cheap"].as_str().unwrap_or("").to_string(), view["medium"].as_str().unwrap_or("").to_string());
    let pick = |b: &mut Browser, page: &Page, value: &str| -> Result<bool> {
        b.eval(page, &format!("(() => {{ const p = document.querySelector('[data-model-for={lead:?}]'); p.value = {value:?}; p.dispatchEvent(new Event('change')); return true; }})()"))?;
        Ok(b.until(page, &format!("document.querySelector('[data-model-for={lead:?}]')?.dataset.saved === {value:?}"), wait))
    };
    // only the cheap tier labelled: an agent on it has no choice
    let wrote = write_tier("cheap")?;
    b.reload(&probe)?;
    b.until(&probe, ready, wait);
    let on_it = shown(b, &probe, &lead)?;
    // one on the medium tier may pick the labelled route, its own tier shown as it is
    let moved = write_tier("medium")?;
    b.reload(&probe)?;
    b.until(&probe, ready, wait);
    let off_it = shown(b, &probe, &lead)?;
    let took = pick(b, &probe, "route:cheap")? && agent_json()["tier"] == "cheap";
    s.ok(
        "one labelled route, no provider: an agent on its tier has no choice and no pick; one on the other tier is offered the route and its own tier, and picks the route",
        wrote == 200 && moved == 200
            && on_it == "hidden"
            && off_it == json!({ "value": "route:medium", "options": [["route:cheap", format!("{} · {cheap}", crate::RECOMMENDED_LABEL)], ["route:medium", format!("Medium ({medium})")]] })
            && took,
        json!({ "onIt": on_it, "offIt": off_it, "took": took }),
    );
    b.close(probe)?;
    // only the medium tier labelled: an agent on the cheap tier may pick it
    let probe = open_with_fewer_labels(b, api, &["cheap"])?;
    b.until(&probe, ready, wait);
    let offered = shown(b, &probe, &lead)?;
    let took = pick(b, &probe, "route:medium")? && agent_json()["tier"] == "medium";
    s.ok(
        "only the other route labelled: an agent on the cheap tier is offered it and its own tier, and picks it",
        offered == json!({ "value": "route:cheap", "options": [["route:medium", format!("{} · {medium}", crate::STANDARD_LABEL)], ["route:cheap", format!("Cheap ({cheap})")]] }) && took,
        json!({ "offered": offered, "took": took }),
    );
    b.close(probe)?;
    // none labelled: no pick without a provider, as before
    let probe = open_with_fewer_labels(b, api, &["cheap", "medium"])?;
    b.until(&probe, ready, wait);
    let none = shown(b, &probe, &lead)?;
    s.ok("no route labelled, no provider: no model pick, as before", none.is_null(), &none);
    b.close(probe)?;
    let back = write_tier("cheap")?;
    anyhow::ensure!(back == 200, "the agent back on the cheap tier: {back}");
    Ok(())
}

fn reasoning_ui(s: &mut Suite, api: &Api, b: &mut Browser, page: &Page, session: &str, shots: &std::path::Path) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    b.reload(page)?;
    let read = b.until(page, "document.querySelector('[data-reasoning-for]')?.dataset.read === '1'", wait);
    let lead = b.eval(page, "document.querySelector('[data-reasoning-for]')?.dataset.reasoningFor ?? ''")?.as_str().unwrap_or("").to_string();
    let pick_of = format!("document.querySelector('[data-reasoning-for={lead:?}]')");
    let options = b.eval(page, &format!("[...({pick_of}?.options ?? [])].map((o) => [o.value, o.textContent])"))?;
    let shown = b.eval(page, &format!("{pick_of}?.value ?? null"))?;
    let every = b.eval(page, "document.querySelectorAll('.agent-setting').length > 0 && document.querySelectorAll('.agent-setting').length === document.querySelectorAll('.agent-setting [data-reasoning-for]').length && document.querySelectorAll('.agent-setting').length === document.querySelectorAll('.agent-setting [data-model-for]').length")?;
    let agent_json = || shell(api, session, "GET", &format!("/api/f/{lead}/file?path=agent.json"), None, &[]).map(|r| r.body).unwrap_or(Value::Null);
    let before = agent_json();
    s.ok(
        "every agent offers how hard it thinks, with no provider connected too: Default (High), Low and High, the default shown while its agent.json names none (and its route, the deployment labelling two)",
        read && !lead.is_empty() && options == json!([["", "Default (High)"], ["low", "Low"], ["high", "High"]]) && shown == "" && every == true && before["tier"] == "cheap" && before.get("reasoning").is_none(),
        json!({ "options": options, "shown": shown, "every": every, "agentJson": before }),
    );
    // its other settings, which a pick never loses: a model named by hand
    // (no provider is connected yet) and a field of its own
    let seed = json!({ "tier": "cheap", "model": { "provider": "openrouter", "id": "z-ai/glm-5.3" }, "kept": "this" });
    let write_json = |v: &Value| shell(api, session, "POST", &format!("/api/f/{lead}/files"), Some(&json!({ "message": "the e2e's settings", "files": [{ "path": "agent.json", "text": format!("{v:#}\n") }] })), &[]).map(|r| r.status);
    let seeded = write_json(&seed)?;
    b.reload(page)?;
    b.until(page, &format!("{pick_of}?.dataset.read === '1'"), wait);
    let said_of = format!("document.querySelector('.agent-setting[data-agent={lead:?}] .agent-model-said')?.textContent");
    let choose = |b: &mut Browser, value: &str| b.eval(page, &format!("(() => {{ const p = {pick_of}; p.value = {value:?}; p.dispatchEvent(new Event('change')); return true; }})()"));
    let pick = |b: &mut Browser, value: &str| -> Result<bool> {
        choose(b, value)?;
        Ok(b.until(page, &format!("{pick_of}?.dataset.saved === {value:?} && !{pick_of}.disabled"), wait))
    };
    // the read of agent.json a pick is written onto fails (Alex's review of
    // #506): nothing is written, it is said plainly, and the pick shows what
    // agent.json holds
    b.eval(page, "(() => { const f = window.fetch; window.__files = 0; window.fetch = (u, o) => { if (o?.method === 'POST' && /\\/files$/.test(String(u))) window.__files++; return f(u, o); }; return true; })()")?;
    b.eval(page, &refuse_once("GET", "/file\\?path=agent\\.json$"))?;
    choose(b, "low")?;
    let refused = b.until(page, &format!("{said_of} === {} && !{pick_of}.disabled", js(UNREAD_SETTINGS)), wait);
    let unwritten = agent_json();
    let (posts, showing) = (b.eval(page, "window.__files")?, b.eval(page, &format!("{pick_of}.value"))?);
    s.ok(
        "a pick whose read of agent.json fails writes nothing (its model and its other settings kept), says so plainly, and shows what agent.json holds",
        seeded == 200 && refused && posts == 0 && unwritten == seed && showing == "",
        json!({ "said": b.eval(page, &said_of)?, "posts": posts, "agentJson": unwritten, "showing": showing }),
    );
    // picked again, read: its own field alone changes
    let saved = pick(b, "low")?;
    let after = agent_json();
    let mut low = seed.clone();
    low["reasoning"] = json!("low");
    b.eval(page, &format!("({pick_of}?.scrollIntoView({{ block: 'center' }}), true)"))?;
    let _ = b.screenshot(page, &shots.join("desktop-agent-reasoning.png"));
    s.ok(
        "picked again, it is the agent's agent.json `reasoning`, every other setting kept (its tier, its model, its own), and settings says its next message uses it",
        saved && after == low && b.eval(page, &said_of)? == "Saved: its next message uses it.",
        json!({ "before": seed, "after": after }),
    );
    b.reload(page)?;
    let again = b.until(page, &format!("{pick_of}?.dataset.read === '1' && {pick_of}.value === 'low'"), wait);
    let back = pick(b, "")?;
    let gone = agent_json();
    s.ok("settings shows it again as picked; the default again takes it away, the rest kept", again && back && gone == seed, &gone);
    // agent.json not there (404) is nothing to keep: a pick writes one
    b.eval(page, "(() => { const real = window.fetch; window.fetch = async (u, o) => { if (o?.method === 'GET' && /\\/file\\?path=agent\\.json$/.test(String(u))) { window.fetch = real; return new Response(JSON.stringify({ error: 'not_found', message: 'no such file' }), { status: 404, headers: { 'content-type': 'application/json' } }); } return real(u, o); }; return true; })()")?;
    let fresh = pick(b, "high")?;
    let made = agent_json();
    s.ok("with no agent.json (404), a pick writes one: its tier and its reasoning", fresh && made == json!({ "tier": "cheap", "reasoning": "high" }), &made);
    // as the lanes after it expect it
    anyhow::ensure!(write_json(&json!({ "tier": "cheap" }))? == 200, "agent.json put back");
    Ok(())
}

/// What settings says when the read of agent.json a pick is written onto
/// fails (cell/shell/shell.js, `UNREAD_SETTINGS`).
const UNREAD_SETTINGS: &str = "Couldn't read this agent's settings, so nothing was changed. Try again.";

/// Settings' own models (Paul, 2026-10-08: "they should be able to connect
/// them from settings ideally"): OpenRouter's row is connected with a
/// press, through its own page (the fake's, which approves at once), with
/// no key pasted and no CLI; then each agent offers the provider's models
/// beside its tier's (named by the tier, never a vendor), and the one
/// picked is its agent.json's `model`, its tier kept, which its computer
/// runs its next turn on. Its tier again takes the model away;
/// disconnecting takes the offer, and a Disconnect that fails says so in
/// its row (#439); a Remove that answers once the person has gone to a
/// chat leaves them there.
fn models_ui(s: &mut Suite, api: &Api, b: &mut Browser, page: &Page, session: &str, chat: &str, shots: &std::path::Path) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let row = format!("#settings-connections [data-provider={:?}]", crate::SWAP_ROUTER);
    b.reload(page)?;
    let offered = b.until(page, &format!("document.querySelector('{row}')?.dataset.state === 'not_set' && !!document.querySelector('{row} [data-action=sign-in]')"), wait);
    let pasted = b.eval(page, &format!("!!document.querySelector('{row} input')"))?;
    s.ok(
        "an own key with a sign-in is connected from its row with a press, never pasted: no key field, and no agent offers its models yet (only the deployment's routes)",
        offered && pasted == false && b.eval(page, "[...document.querySelectorAll('[data-model-for] option')].every((o) => o.value.startsWith('route:'))")? == true,
        b.eval(page, &format!("document.querySelector('{row}')?.innerText"))?,
    );
    // its window opens in the press itself, before the authorize call (one
    // opened once that answers is a popup Safari blocks, as Chrome does once
    // the press is 5 s old; #442): one the browser blocks is said in the
    // row, and starts nothing. The page's fetch counts the authorize calls
    // and holds them `slow` ms.
    let connect = format!("{row} [data-action=sign-in]");
    b.eval(page, "(() => { const f = window.fetch.bind(window); window.authorizes = 0; window.slow = 0; window.fetch = (u, o) => { if (!String(u).endsWith('/authorize')) return f(u, o); window.authorizes++; return new Promise((r) => setTimeout(r, window.slow)).then(() => f(u, o)); }; window.realOpen = window.open; window.open = () => null; return true; })()")?;
    b.click(page, &connect)?;
    let blocked = b.until(page, &format!("document.querySelector('{row} .provider-said')?.textContent.includes('Allow pop-ups for this site')"), wait);
    let started = b.eval(page, "window.authorizes")?;
    s.ok(
        "a window the browser blocks is said in its row, with how to allow it and to try again, and starts nothing",
        blocked && started == 0,
        json!({ "said": b.eval(page, &format!("document.querySelector('{row} .provider-said')?.textContent ?? null"))?, "authorizes": started }),
    );
    // the press opens the provider's page, which sends the browser back to
    // the platform, the authorize call held past the press's 5 s; pressed
    // twice, as a person whose window is slow to come does
    b.eval(page, "(window.open = window.realOpen, window.slow = 6000, true)")?;
    let windows = b.pages()?.len();
    let keys_before = s.openrouter.keys().len();
    b.click(page, &connect)?;
    b.click(page, &connect)?;
    let opened = s.eventually(std::time::Duration::from_secs(3), || b.pages().is_ok_and(|l| l.len() > windows));
    let waiting = b.eval(page, &format!("document.querySelector('{connect}')?.disabled === true && document.querySelector('{row} .provider-said')?.textContent === ''"))?;
    let callback = format!("/api/connections/{}/callback", crate::SWAP_ROUTER);
    let mut popup = None;
    let came_back = s.eventually(wait, || {
        popup = b.pages().ok().and_then(|l| l.into_iter().find(|(_, url)| url.contains(&callback)).map(|(id, _)| id));
        popup.is_some()
    });
    let authorizes = b.eval(page, "window.authorizes")?;
    s.ok(
        "its window opens in the press, before the authorize call answers, and Connect waits for it: a second press starts no second attempt (each replaces the browser's binding)",
        opened && waiting == true && authorizes == 1,
        json!({ "opened": opened, "waiting": waiting, "authorizes": authorizes }),
    );
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
    // each agent offers the provider's models after the deployment's routes,
    // each by its label and its model's id, as their computer's view names them
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
    let medium = connected["models"]["medium"].as_str().unwrap_or("").to_string();
    let vendor = b.eval(page, "document.getElementById('settings-page').innerText.includes('Workers AI') || [...document.querySelectorAll('[data-model-for] option')].some((o) => o.textContent.includes('Workers AI'))")?;
    let labels = [crate::RECOMMENDED_LABEL, crate::STANDARD_LABEL];
    s.ok(
        "then each agent offers the deployment's routes, each by its label and its model's id as their computer's view names it (the one it runs on now picked), then the provider's models, and no vendor: no Workers AI anywhere in settings; the view names each route and the providers whose models the agent may run",
        read && !lead.is_empty()
            && before["tier"] == "cheap"
            && !cheap.is_empty() && cheap != "cheap"
            && ["medium", "fallback", "vision"].iter().all(|n| connected["models"][n].as_str().is_some_and(|id| !id.is_empty()))
            && connected["models"]["routes"]["cheap"]["label"] == labels[0] && connected["models"]["routes"]["medium"]["label"] == labels[1]
            && options.first().is_some_and(|o| o[0] == "route:cheap" && o[1] == format!("{} · {cheap}", labels[0]).as_str())
            && options.get(1).is_some_and(|o| o[0] == "route:medium" && o[1] == format!("{} · {medium}", labels[1]).as_str())
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
    let back = pick(b, "route:cheap")?;
    let gone = agent_json();
    s.ok("settings shows it again as picked; its route again takes the model away, its tier kept", shown && back && gone.get("model").is_none() && gone["tier"] == before["tier"], &gone);
    // the other route: the agent's tier, and back (Paul, 2026-10-24: Standard is one pick away)
    let standard = pick(b, "route:medium")?;
    let on_standard = agent_json();
    let private = pick(b, "route:cheap")?;
    let on_private = agent_json();
    s.ok(
        "picked, the other route is the agent's tier (medium), no model of its own; picked back, the first again",
        standard && on_standard["tier"] == "medium" && on_standard.get("model").is_none() && private && on_private["tier"] == "cheap",
        json!({ "standard": on_standard, "private": on_private }),
    );
    // a Disconnect that does not go through (#439): settings reload, its row
    // still connected, saying so, with Disconnect to try again
    let said = format!("document.querySelector('{row} .provider-said')");
    b.eval(page, &refuse_once("DELETE", &format!("/api/connections/{}/key$", crate::SWAP_ROUTER)))?;
    b.click(page, &format!("{row} [data-action=disconnect]"))?;
    let told = b.until(
        page,
        &format!("document.querySelector('{row}')?.dataset.state === 'set' && {said}?.textContent === {} && {said}.checkVisibility() && document.querySelector('{row} [data-action=disconnect]')?.disabled === false", js(UNREMOVED)),
        wait,
    );
    s.ok(
        "a Disconnect that does not go through: its row, reloaded, still connected, says That didn't go through. Try again., with Disconnect to try again",
        told,
        b.eval(page, &format!("document.querySelector('{row}')?.innerText"))?,
    );
    // disconnected: the key goes, and no agent offers its models
    b.click(page, &format!("{row} [data-action=disconnect]"))?;
    let off = b.until(page, &format!("document.querySelector('{row}')?.dataset.state === 'not_set' && !{said}?.textContent && [...document.querySelectorAll('[data-model-for] option')].every((o) => o.value.startsWith('route:'))"), wait);
    let left = runs_on(&view());
    s.ok(
        "pressed again, Disconnect takes the key away, the failure no longer said, and no agent offers its models (only the routes), nor may run them",
        off && left.is_null(),
        json!({ "row": b.eval(page, &format!("document.querySelector('{row}')?.innerText"))?, "modelProviders": left }),
    );
    // an action's answer once the person has gone to a chat leaves them in
    // it (#439): an own key's Remove, its answer held while a chat opens
    let own = format!("#settings-connections [data-provider={:?}]", crate::SWAP_OWN);
    let key = format!("/api/connections/{}/key", crate::SWAP_OWN);
    let saved = shell(api, session, "PUT", &key, Some(&json!({ "key": "mail-left-e2e-55aa" })), &[])?;
    b.reload(page)?;
    let set = b.until(page, &format!("document.querySelector('{own}')?.dataset.state === 'set'"), wait);
    b.eval(page, &hold("DELETE", &format!("{key}$"), Hold::Answer))?;
    b.click(page, &format!("{own} [data-action=remove-key]"))?;
    let held = b.until(page, "!!window.__held.release", wait);
    open_row(b, page, chat, wait)?;
    b.eval(page, "(window.__held.release?.(), true)")?;
    let settled = b.until(page, HELD_SETTLED, wait);
    let seen = b.eval(page, "({ path: location.pathname, settings: !document.getElementById('settings-page').hidden, chat: [...document.querySelectorAll('#frames iframe')].find((f) => !f.hidden)?.dataset.fragment ?? null })")?;
    b.eval(page, "(window.__held.restore(), true)")?;
    let listed = shell(api, session, "GET", "/api/connections", None, &[])?;
    let gone = listed.body["providers"].as_array().and_then(|l| l.iter().find(|p| p["provider"] == crate::SWAP_OWN)).map(|p| p["state"].clone()).unwrap_or(Value::Null);
    s.ok(
        "a Remove that answers once the person has gone to a chat takes the key away and leaves them in the chat: settings stay hidden",
        saved.status == 200 && set && held && settled && seen == json!({ "path": "/", "settings": false, "chat": chat }) && gone == "not_set",
        json!({ "seen": seen, "state": gone }),
    );
    b.click(page, "#settings")?;
    anyhow::ensure!(b.until(page, &format!("location.pathname === '/settings' && document.querySelector('{own}')?.dataset.state === 'not_set'"), wait), "settings again");
    Ok(())
}

/// What a connection's row says when its Disconnect or Remove did not go
/// through (shell.js `unremoved`).
const UNREMOVED: &str = "That didn't go through. Try again.";

/// A failure belongs to the visit of settings it was pressed in (#462
/// review): an own key's Remove, then an agent's chip, each held before it
/// is sent while a chat opens and settings open again, then refused; and a
/// Remove refused at once, its reload's read held while a chat opens and
/// settings open again: the settings shown say nothing of it, the key
/// still set and the chip as it was. And the same key's Remove pressed in
/// two visits, both refused, in either order: the newer visit's failure is
/// what is said. The key taken away after.
fn left_and_back_ui(s: &mut Suite, api: &Api, b: &mut Browser, page: &Page, session: &str, chat: &str) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let own = format!("#settings-connections [data-provider={:?}]", crate::SWAP_OWN);
    let key = format!("/api/connections/{}/key", crate::SWAP_OWN);
    let chip = "#settings-connections [data-provider=\"perplexity\"] [data-agent]";
    let pressed = format!("document.querySelector({})?.getAttribute('aria-pressed') === 'true' && document.querySelector({}).disabled === false", js(chip), js(chip));
    let open_settings = "(document.getElementById('settings').click(), true)";
    let saved = shell(api, session, "PUT", &key, Some(&json!({ "key": "mail-back-e2e-55aa" })), &[])?;
    b.reload(page)?;
    let set = format!("location.pathname === '/settings' && document.querySelector('{own}')?.dataset.state === 'set' && !!document.querySelector('{own} [data-action=remove-key]')");
    let ready = saved.status == 200 && b.until(page, &format!("{set} && {pressed}"), wait);

    // the Remove: held, a chat, settings again, then refused
    b.eval(page, &hold("DELETE", &format!("{key}$"), Hold::Request))?;
    b.click(page, &format!("{own} [data-action=remove-key]"))?;
    let held = b.until(page, "!!window.__held.release", wait);
    open_row(b, page, chat, wait)?;
    b.eval(page, open_settings)?;
    let back = b.until(page, &set, wait);
    b.eval(page, "(window.__held.release?.('refuse'), true)")?;
    let settled = b.until(page, HELD_SETTLED, wait);
    let row = b.eval(page, &format!("({{ state: document.querySelector('{own}')?.dataset.state ?? null, said: document.querySelector('{own} .provider-said')?.textContent ?? '' }})"))?;
    b.eval(page, "(window.__held.restore(), true)")?;
    s.ok(
        "a Remove refused once the person has left settings and come back says nothing in the settings shown: the key still set",
        ready && held && back && settled && row == json!({ "state": "set", "said": "" }),
        &row,
    );

    // the Remove refused here, its reload's read held while a chat opens
    // and settings open again; then let go
    b.eval(page, &hold("GET", "^/api/connections$", Hold::Answer))?;
    b.eval(page, &refuse_once("DELETE", &format!("{key}$")))?;
    b.click(page, &format!("{own} [data-action=remove-key]"))?;
    let held = b.until(page, "!!window.__held.release", wait);
    open_row(b, page, chat, wait)?;
    b.eval(page, open_settings)?;
    let back = b.until(page, &set, wait);
    let shown = b.eval(page, &format!("document.querySelector('{own} .provider-said')?.textContent ?? ''"))?;
    b.eval(page, "(window.__held.release?.(), true)")?;
    let settled = b.until(page, HELD_SETTLED, wait);
    let row = b.eval(page, &format!("({{ state: document.querySelector('{own}')?.dataset.state ?? null, said: document.querySelector('{own} .provider-said')?.textContent ?? '' }})"))?;
    b.eval(page, "(window.__held.restore(), true)")?;
    s.ok(
        "a Remove refused in one visit of settings, its reload held until the person has left and come back, says nothing in the settings shown, then or once let go",
        held && back && settled && shown == "" && row == json!({ "state": "set", "said": "" }),
        json!({ "shown": shown, "after": row }),
    );

    // the same key's Remove pressed in two visits, both refused: the newer
    // visit's first, then the older one's; then the other way round. The
    // newer visit's failure is said either way.
    let said = format!("(document.querySelector('{own} .provider-said')?.textContent ?? '')");
    let remove = format!("(document.querySelector('{own} [data-action=remove-key]').click(), true)");
    let failed_here = format!("{set} && {said} === {}", js(UNREMOVED));
    b.eval(page, &hold("DELETE", &format!("{key}$"), Hold::Request))?;
    b.eval(page, &remove)?;
    let held = b.until(page, "!!window.__held.release", wait);
    open_row(b, page, chat, wait)?;
    b.eval(page, open_settings)?;
    let back = b.until(page, &set, wait);
    b.eval(page, &refuse_once("DELETE", &format!("{key}$")))?;
    b.eval(page, &remove)?;
    let newer = b.until(page, &failed_here, wait);
    b.eval(page, "(window.__held.release?.('refuse'), true)")?;
    let settled = b.until(page, HELD_SETTLED, wait);
    let kept = b.eval(page, &failed_here)?;
    let seen = b.eval(page, &said)?;
    b.eval(page, "(window.__held.restore(), true)")?;
    s.ok(
        "the same key's Remove refused in a newer visit of settings, then the older visit's refused: the newer visit's failure is still said",
        held && back && newer && settled && kept == true,
        &seen,
    );
    b.eval(page, &hold("DELETE", &format!("{key}$"), Hold::Request))?;
    b.eval(page, &remove)?;
    let held = b.until(page, "!!window.__held.release", wait);
    b.eval(page, "(window.__older = window.__held, true)")?;
    open_row(b, page, chat, wait)?;
    b.eval(page, open_settings)?;
    let back = b.until(page, &format!("{set} && {said} === ''"), wait);
    b.eval(page, &hold("DELETE", &format!("{key}$"), Hold::Request))?;
    b.eval(page, &remove)?;
    let held_newer = b.until(page, "!!window.__held.release", wait);
    b.eval(page, "(window.__older.release?.('refuse'), true)")?;
    let settled = b.until(page, "window.__older.inflight === 0 && performance.now() - window.__older.quiet > 500", wait);
    b.eval(page, "(window.__held.release?.('refuse'), true)")?;
    let newer = b.until(page, &failed_here, wait);
    let seen = b.eval(page, &said)?;
    b.eval(page, "(window.__held.restore(), window.__older.restore(), true)")?;
    s.ok(
        "the same key's Remove refused in an older visit of settings, then the newer visit's refused: the newer visit's failure is said",
        held && back && held_newer && settled && newer,
        &seen,
    );

    // the chip: held, a chat, settings again, then refused
    b.eval(page, &hold("PUT", "/agents/[^/]+/connections$", Hold::Request))?;
    b.eval(page, &format!("(document.querySelector({}).click(), true)", js(chip)))?;
    let held = b.until(page, "!!window.__held.release", wait);
    open_row(b, page, chat, wait)?;
    b.eval(page, open_settings)?;
    let back = b.until(page, &pressed, wait);
    b.eval(page, "(window.__held.release?.('refuse'), true)")?;
    let settled = b.until(page, HELD_SETTLED, wait);
    let quiet = b.eval(page, &format!("document.getElementById('notice').hidden && {pressed}"))?;
    b.eval(page, "(window.__held.restore(), true)")?;
    s.ok(
        "an agent's chip refused once the person has left settings and come back says nothing in the settings shown: the chip as it was",
        held && back && settled && quiet == true,
        b.eval(page, "document.getElementById('notice').innerText")?,
    );
    shell(api, session, "DELETE", &key, None, &[])?;
    b.reload(page)?;
    anyhow::ensure!(b.until(page, &format!("document.querySelector('{own}')?.dataset.state === 'not_set' && {pressed}"), wait), "settings, the key gone");
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
    let r = api.share(&theirs, &me, "viewer")?;
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

/// A shell whose first list socket is never answered (its upgrade held, as
/// a dropped connection or a phone's changing network leaves one: only a
/// close opened it again, and a page back on screen opened one only when it
/// had none, so its sidebar stayed as it was until a reload). Valid: the
/// page closes it once it has waited past any healthy open
/// (`LIST_OPEN_WAIT_MS`), opens another, and is live again: an app
/// archived elsewhere leaves its dock, and comes back unarchived, with no
/// reload; so does one taken off the dock, and put back (#587).
fn held_list_socket(s: &mut Suite, api: &Api, b: &mut Browser, main: &Page, session: &str) -> Result<()> {
    // the open wait and its first wait, jittered
    let wait = std::time::Duration::from_secs(30);
    // the new socket's open, after the close
    let live = std::time::Duration::from_secs(10);
    let page = b.open_with_script(&format!("{}/", api.base), &crate::browser::hold_first_socket("/api/fragments/watch"))?;
    b.viewport(&page, 1280, 800, false)?;
    let app = "document.querySelector('#apps [data-key^=\"app:\"]')?.dataset.key.slice(4) ?? ''";
    let shown = b.until(&page, &format!("!document.getElementById('layout').hidden && ({app}) !== ''"), wait);
    let name = b.eval(&page, app)?.as_str().unwrap_or("").to_string();
    let closed = b.until(&page, "window.__heldSockets === 1 && window.__heldClosed === 1", wait);
    let row = format!("document.querySelector({})", js(&format!("#apps [data-key={}]", js(&format!("app:{name}")))));
    let archive = |on: bool| shell(api, session, "PUT", &format!("/api/fragments/{name}/archived"), Some(&json!({ "archived": on })), &[]);
    let r = archive(true)?;
    let left = b.until(&page, &format!("!{row}"), live);
    let r2 = archive(false)?;
    let back = b.until(&page, &format!("!!{row}"), live);
    s.ok(
        "a shell whose first list socket is never answered closes it once it has waited past any healthy open, opens another, and its dock follows an app archived elsewhere, and unarchived, with no reload",
        shown && closed && r.status == 200 && r2.status == 200 && left && back,
        json!({ "app": name, "held": b.eval(&page, "[window.__heldSockets, window.__heldClosed]")?, "left": left, "back": back }),
    );
    // the person's own dock (#587): taken off elsewhere, put back
    let dock = |on: bool| shell(api, session, "PUT", &format!("/api/fragments/{name}/dock"), Some(&json!({ "dock": on })), &[]);
    let r = dock(false)?;
    let left = b.until(&page, &format!("!{row}"), live);
    let r2 = dock(true)?;
    let back = b.until(&page, &format!("!!{row}"), live);
    s.ok("and an app taken off the dock elsewhere leaves it, and comes back put on it, with no reload", r.status == 200 && r2.status == 200 && left && back, json!({ "app": name, "left": left, "back": back }));
    b.close(page)?;
    // the person's own tab, on screen again
    b.front(main)?;
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
        listed && got == want && got.iter().any(|(c, n)| c == "software-development" && n == "apps-finite"),
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
/// another; so does the New chat row below the agent, in the sidebar
/// (screenshots light and dark). The agent's own chat keeps its label (`fragment ask`, after,
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
    // New chat, the row below their agent: the menu's, one click from the top
    b.click(page, &format!("#chats [data-key={}]", js(&format!("chat:{reader_chat}"))))?;
    b.until(page, "document.getElementById('chat-title').textContent === 'Reader'", wait);
    let row_shown = b.eval(page, "(() => { const r = document.getElementById('sidebar-start-chat'); const top = document.getElementById('primary-agent').getBoundingClientRect(); const at = r.getBoundingClientRect(); return !r.hidden && at.height > 0 && at.top >= top.bottom && r.textContent === 'New chat'; })()")?;
    for scheme in ["light", "dark"] {
        b.color_scheme(page, scheme)?;
        let _ = b.screenshot(page, &shots.join(format!("desktop-new-chat-row-{scheme}.png")));
    }
    b.color_scheme(page, "light")?;
    b.click(page, "#sidebar-start-chat")?;
    let row_opened = b.until(page, "document.getElementById('chat-title').textContent === 'New chat'", wait);
    let made = chats_with(&own_chat);
    s.ok(
        "New chat, the row below their agent, does as its menu's: from the Reader's chat it opens that empty chat again, not another",
        row_shown == true && row_opened && open_chat(b, page) == third && made.len() == before.len() + 2,
        json!({ "shown": row_shown, "chats": made }),
    );
    let own = own_named(api, me.session, &own_chat)?;
    s.ok("the agent's own chat keeps its label, apart from the new ones", label_of(&own) == own_chat && label_of(&fresh) == format!("{own_chat}-2"), json!([own, fresh]));
    b.click(page, &format!("#chats [data-key={}]", js(&format!("chat:{reader_chat}"))))?;
    b.until(page, "document.getElementById('chat-title').textContent === 'Reader'", wait);
    Ok(())
}

/// An app's window asked to load again before the shell knows its origin
/// (Alex's review of PR 582). A fragment's frame names its origin in its
/// policy (`allow="… microphone <origin>"`) before it navigates (a policy
/// is read as a navigation starts), and the origin is its status's: a
/// Reload pressed while that was still asked navigated at once under the
/// bare policy, which names only the mint's origin, and the page stayed
/// refused the microphone. Valid: in a shell of its own (no origin known),
/// the app's status held, its window opened and its Reload pressed, the
/// frame waits (no `src` yet); its status let through, it loads once,
/// under the policy that names its origin, and its page may use the
/// microphone (Chrome's fake, allowed).
fn app_policy_ui(s: &mut Suite, api: &Api, b: &mut Browser, page: &Page, app: &str) -> Result<()> {
    let wait = std::time::Duration::from_secs(30);
    let pane = format!("document.querySelector({})", js(&format!("#stack .pane[data-key=\"app:{app}\"]")));
    // the app's window elsewhere closed: its page here is the only one
    b.eval(page, &format!("({pane}?.querySelector('button[aria-label=Close]')?.click(), true)"))?;
    b.until(page, &format!("!{pane}"), wait);
    // its status held until let through, from the page's start
    let hold = format!(
        "(() => {{ const real = window.fetch; let go; window.__let = new Promise((r) => {{ go = r; }}); window.__letGo = () => go(); window.__holding = false; window.fetch = async (url, o) => {{ if (window.__holding && String(url).includes({})) await window.__let; return real(url, o); }}; }})();",
        js(&format!("/api/f/{app}/status"))
    );
    let shell = b.open_with_script(&format!("{}/", api.base), &hold)?;
    b.viewport(&shell, 1280, 860, false)?;
    let icon = format!("document.querySelector({})", js(&format!("#apps [data-key=\"app:{app}\"]")));
    let up = b.until(&shell, &format!("!document.getElementById('layout').hidden && !!{icon}"), wait);
    b.eval(&shell, &format!("(window.__holding = true, {icon}.click(), true)"))?;
    let opened = b.until(&shell, &format!("!!{pane}?.querySelector('iframe') && !!{pane}.querySelector('button[aria-label=Reload]')"), wait);
    b.eval(&shell, &format!("({pane}.querySelector('button[aria-label=Reload]').click(), true)"))?;
    let waited = b.eval(&shell, &format!("!{pane}.querySelector('iframe').hasAttribute('src')"))?;
    b.eval(&shell, "(window.__letGo(), true)")?;
    let named = b.until(&shell, &format!("/microphone https?:/.test({pane}?.querySelector('iframe')?.allow ?? '') && {pane}.querySelector('iframe').hasAttribute('src')"), wait);
    let allow = b.eval(&shell, &format!("{pane}?.querySelector('iframe')?.allow ?? null"))?;
    let told = s.eventually(wait, || {
        let Ok(frame) = b.frame(app) else { return false };
        let state = b.eval(&frame, "navigator.permissions.query({ name: 'microphone' }).then((p) => p.state)");
        let _ = b.detach(frame);
        state.ok() == Some(json!("granted"))
    });
    s.ok(
        "an app's window opened and reloaded while the shell still asks for its origin waits for it, then loads under the policy that names it: its page may use the microphone",
        up && opened && waited == true && named && told,
        json!({ "up": up, "opened": opened, "waited": waited, "allow": allow, "told": told }),
    );
    b.close(shell)?;
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
        let r = api.share(&theirs, who, role)?;
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
