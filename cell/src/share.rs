//! Sharing, on the platform's origin (docs/api.md, Sharing): the share
//! sheet, and the access view's answer. The sheet is the platform's page,
//! signed in by its session cookie, and acts through the fragment's own
//! handlers (members.rs) as the signed-in person: the fragment decides who
//! may do what (only its owner changes anything; a member may see who is
//! in, and leave; anyone else gets a 403 page).
//!
//!   GET  /share/<name>         the sheet: who is in (emails and pictures) and their roles;
//!                              for the owner, adding people by email, the invites waiting,
//!                              roles, removing, who may open it, and its share link (copy, a new one);
//!                              for a member, leaving
//!   POST /share/<name>         one change (`action`), then back to the sheet
//!   GET  /api/access           the fragments a person owns, and who may open each (`access`)
//!
//! Every choice is a button (no `<select>`: a closed one changes as an
//! arrow key passes over it, on Linux and Windows, and the sheet sent each
//! change at once), and what takes access away (removing someone, a new
//! share link, leaving) asks a second click.
//!
//! An email is mailed a link to the platform's page, where whoever signs
//! in as it accepts the invite, or declines it (decision 63: the shell's
//! sidebar, through `/api/invites`, below): nobody is made a member by an
//! invite without their yes.
//!
//!   GET  /api/invites                    the invites waiting on a person's emails
//!   POST /api/invites/<name>/accept      their yes: a member now
//!   POST /api/invites/<name>/decline     their no: it waits no more
//!
//! A fragment's page (its author's code, or an agent's) is cross-site from
//! the platform on fragment.club, but one site
//! with it on a fleet whose platform shares the fragments' domain, where
//! the platform's session rides along on its fetches, forms, and frames.
//! Either way the sheet cannot be driven by one:
//!
//! - it cannot read it: it sends no CORS headers, and it refuses every
//!   frame (`auth::unframed`) but the platform's own page's
//!   (`frame-ancestors 'self'`: no fragment is on its origin);
//! - it cannot post to it: every POST's Origin must be the platform's
//!   (`auth::same_origin`) and carry a form token bound to the session
//!   (`fragment_core::form`), which only the page itself holds;
//! - it cannot script a window it opened on it: it answers
//!   `Cross-Origin-Opener-Policy: same-origin` (`auth::unopened`), so the
//!   window is severed from its opener;
//! - it cannot hand it a grant: the sheet reads nothing from its URL, so
//!   no link prefills what a click would approve;
//! - the click that opened it cannot confirm it: its buttons arm
//!   `form::DELAY_MS` after the page shows, and a form sent sooner is
//!   refused.

use std::collections::BTreeMap;

use fragment_core::{form, npub};
use fragment_proto::{
    limits, Access, AccessMember, AccessRow, ErrorBody, ErrorCode, FragmentKind, IdentityKind, Invite, InviteAccepted, InviteList, ListedFragment, Member, MemberList, Role, Shared, Visibility,
};
use futures_util::future::{join, join_all};
use serde_json::{json, Value};
use worker::*;

use crate::ask_registry;
use crate::auth::{self, enc, esc};
use crate::config::Config;
use crate::error::{CellError, CellResult};
use crate::js;
use crate::registry::calls::{self, Profile};
use crate::registry::invites::{AnswerInvite, InvitesFor};
use crate::routed::{Routed, Signed};
use fragment_core::gather::Gathered;

/// A form of a few short fields (the longest: an email).
const FORM_MAX_BYTES: usize = 4 * 1024;
const DAY_MS: i64 = 24 * 3600 * 1000;

/// Agent fragments one naming asks for their faces, at most (`name_agents`:
/// a page's `__people` names at most 64 identities; a chat's agents are
/// fewer): past it, an agent shows by its label.
pub(crate) const FACES_MAX: usize = 64;

/// Arms the page's buttons `DELAY_MS` after it shows (again each time it
/// is shown), copies a link, asks a second click of a button that takes
/// access away (`data-confirm`), and closes the sheet (Done; framed,
/// Escape too). Framed, it tells the page around it, the platform's own,
/// its height as it changes and when it is done.
const SCRIPT: &str = r#"<script>(() => {
  const acting = () => document.querySelectorAll("[data-arm]");
  let timer = 0;
  const arm = () => {
    clearTimeout(timer);
    for (const b of acting()) b.disabled = true;
    timer = setTimeout(() => { for (const b of acting()) b.disabled = false; }, DELAY_MS);
  };
  arm();
  addEventListener("pageshow", arm);
  document.addEventListener("visibilitychange", () => { if (document.visibilityState === "visible") arm(); });
  for (const b of document.querySelectorAll("button[data-copy]")) b.onclick = () => {
    const link = b.dataset.copy, was = b.textContent;
    (navigator.clipboard ? navigator.clipboard.writeText(link) : Promise.reject()).then(() => {
      b.textContent = "Copied";
      setTimeout(() => { b.textContent = was; }, 1500);
    }, () => prompt("Copy this link:", link));
  };
  // what takes access away asks once more: the first click says so, the
  // second sends it (within a few seconds)
  for (const b of document.querySelectorAll("button[data-confirm]")) {
    const was = b.textContent;
    let undo = 0;
    b.addEventListener("click", (e) => {
      if (b.dataset.sure) return;
      e.preventDefault();
      b.dataset.sure = "1";
      b.textContent = b.dataset.confirm;
      clearTimeout(undo);
      undo = setTimeout(() => { delete b.dataset.sure; b.textContent = was; }, 4000);
    });
  }
  // in the platform's dialog (only its pages may frame this one), Done or
  // Escape closes the dialog; in a window of its own, Done closes the
  // window, or it goes to the shell's Apps page, where sharing is
  const framed = parent !== window;
  const tell = (share) => parent.postMessage({ share, height: document.body.offsetHeight }, location.origin);
  if (framed) {
    tell("shown");
    new ResizeObserver(() => tell("resized")).observe(document.body);
    addEventListener("keydown", (e) => { if (e.key === "Escape") tell("done"); });
  }
  for (const b of document.querySelectorAll("button[data-done]")) b.onclick = () => {
    if (framed) return tell("done");
    close();
    if (!window.closed) location.href = "/apps";
  };
})();</script>"#;

fn script() -> String {
    SCRIPT.replace("DELAY_MS", &form::DELAY_MS.to_string())
}

/// A sharing page's CSP: its stylesheets, fonts, images and script file
/// from the platform, its own small scripts inline, and who may frame it
/// (`ancestors`).
fn policy(forms_here: bool, ancestors: &str) -> String {
    let forms = if forms_here { " form-action 'self';" } else { "" };
    format!("default-src 'none'; img-src 'self'; style-src 'self' 'unsafe-inline'; font-src 'self'; script-src 'self' 'unsafe-inline';{forms} base-uri 'none'; frame-ancestors {ancestors}")
}

/// One of sharing's pages: the platform's look, its protections, and
/// armed buttons. `forms_here`: its forms post only to this origin (the
/// join page's redirects on to the fragment's, which `form-action` would
/// refuse).
pub(crate) fn sheet_page(status: u16, title: &str, body: &str, forms_here: bool) -> CellResult<Response> {
    page(status, platform_html(title, body), forms_here)
}

/// A sharing page's answer: `html`, armed, with sharing's protections.
fn page(status: u16, html: String, forms_here: bool) -> CellResult<Response> {
    let html = html + &script();
    let h = Headers::new();
    h.set("content-type", "text/html; charset=utf-8")?;
    h.set("cache-control", "no-store")?;
    auth::unframed(&h)?;
    auth::unopened(&h)?;
    h.set("content-security-policy", &policy(forms_here, "'none'"))?;
    // the join page's URL holds its token: no other origin hears it
    h.set("referrer-policy", "same-origin")?;
    Ok(Response::ok(html)?.with_status(status).with_headers(h))
}

/// A page's skeleton: its `<html>`'s attributes, its title, what goes
/// before its card (`head`: its stylesheets, and on the platform's pages
/// the logo), and its card's body under its heading. The pages' look is in
/// files (cell/shell/sheet.css, platform-pages.css); this frame is filled
/// per request, escaped as it goes, so it stays a string here rather than
/// a file read from the assets on every page.
fn skeleton(attrs: &str, title: &str, head: &str, body: &str) -> String {
    format!(
        r#"<!doctype html><html lang="en"{attrs}><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>{t}</title>
{head}<main><h1>{t}</h1>{body}</main>"#,
        t = esc(title),
    )
}

/// A plain notice in the base look, its stylesheet `sheet` from the
/// page's own origin: `shell::versioned("sheet.css")` on the platform's,
/// `shell::fragment_sheet()` on a fragment's, which so loads nothing from
/// the platform's (auth.rs `html`). Callers keep their own framing,
/// navigation and script policies (a CSP of theirs allows `style-src
/// 'self'`).
pub(crate) fn sheet_html(sheet: &str, title: &str, body: &str) -> String {
    skeleton("", title, &format!("<link rel=\"stylesheet\" href=\"{sheet}\">"), body)
}

/// Framed (the shell's dialog), a page is drawn as the dialog's from its
/// first frame (platform-pages.css `.framed`: no landscape, logo or glass),
/// before its stylesheet is read: the landscape is never fetched.
const FRAMED: &str = "<script>if(parent!==window)document.documentElement.classList.add(\"framed\")</script>";

/// A page on the platform's origin, in the onboarding's look: the base
/// sheet and platform-pages.css over it, and the pointer-following rim's
/// script (platform-pages.js, a module: it waits for the page, and nothing
/// waits for it), each linked by its version and kept by the browser
/// (shell.rs `versioned`); the logo only where it shows (lazy: a framed
/// page hides it).
pub(crate) fn platform_html(title: &str, body: &str) -> String {
    use crate::shell::versioned;
    let head = format!(
        "{FRAMED}<link rel=\"stylesheet\" href=\"{}\"><link rel=\"stylesheet\" href=\"{}\"><script type=\"module\" src=\"{}\"></script>\
         <div class=\"platform-brand\"><img src=\"/__shell/finite-logo.svg\" alt=\"Finite\" width=\"44\" height=\"46\" loading=\"lazy\"></div>",
        versioned("sheet.css"),
        versioned("platform-pages.css"),
        versioned("platform-pages.js"),
    );
    skeleton(" class=\"platform-page\"", title, &head, body)
}

/// The share sheet itself: a sharing page only the platform's own pages
/// may frame (the shell shows it in a dialog: cell/shell/shell.js
/// `share`). `'self'` is the platform's origin alone; every fragment is on
/// another (docs/api.md, Sign-in).
fn sheet_framable(status: u16, title: &str, body: &str) -> CellResult<Response> {
    // `share`: the sheet's own compact look (platform-pages.css `main.share`)
    let mut resp = page(status, platform_html(title, body).replacen("<main>", "<main class=\"share\">", 1), true)?;
    let h = resp.headers_mut();
    h.set("content-security-policy", &policy(true, "'self'"))?;
    h.set("x-frame-options", "SAMEORIGIN")?;
    Ok(resp)
}

/// A page that only says something (a refusal, a stale link): `html`,
/// escaped by the caller.
fn notice(status: u16, title: &str, html: &str) -> CellResult<Response> {
    sheet_page(status, title, &format!("<p>{html}</p>"), true)
}

fn purpose(what: &str, name: &str) -> String {
    format!("{what}:{name}")
}

pub(crate) fn label(name: &str) -> &str {
    fragment_proto::split_fragment_name(name).map_or(name, |(label, _)| label)
}

/// A role as a sentence says it: "a viewer", "an editor".
fn role_phrase(role: Role) -> &'static str {
    match role {
        Role::Editor => "an editor",
        Role::Owner => "its owner",
        _ => "a viewer",
    }
}

/// The route's fragment, or a 404 page.
fn fragment_named(name: &str) -> Option<String> {
    fragment_proto::valid_fragment_name(name).then(|| name.to_string())
}

/// Sharing's routes on the platform origin (the router sends only these).
pub async fn route(req: Request, env: &Env, cfg: &Config, url: &Url, segments: &[&str]) -> CellResult<Response> {
    let method = req.method();
    let (what, name) = match segments {
        [what @ "share", name] => (*what, *name),
        _ => return Err(CellError::new(ErrorCode::NotFound, format!("no route {}", url.path()))),
    };
    let Some(name) = fragment_named(name) else { return notice(404, "No such fragment", "This link names no fragment.") };
    match (method, what) {
        (Method::Get, "share") => sheet(&req, env, cfg, url, &name).await,
        (Method::Post, "share") => share_post(req, env, cfg, url, &name).await,
        (m, _) => Err(CellError::new(ErrorCode::NotFound, format!("no route {} {}", m.as_ref(), url.path()))),
    }
}

/// A fragment's handler at `inner`, asked as the signed-in person: its
/// answer, or its refusal as it made it.
pub(crate) async fn ask(env: &Env, url: &Url, name: &str, who: &Signed, method: Method, inner: &str, body: Option<Value>) -> CellResult<Value> {
    // the page's own query string is nothing the fragment reads
    let mut at = url.clone();
    at.set_query(None);
    let headers = Headers::new();
    Routed { name: name.to_string(), url: at, signed: Some(who.clone()), credential: None }.to_headers(&headers)?;
    if body.is_some() {
        headers.set("content-type", "application/json")?;
    }
    let mut init = RequestInit::new();
    init.with_method(method).with_headers(headers);
    if let Some(b) = body {
        init.with_body(Some(b.to_string().into()));
    }
    let req = Request::new_with_init(&format!("https://fragment.internal{inner}"), &init)?;
    let mut resp = crate::routed::send(env, "FRAGMENT", name, req).await?;
    let status = resp.status_code();
    let v: Value = resp.json().await.map_err(|e| CellError::host(format!("{inner} answered {status}, not JSON: {e}")))?;
    if status == 200 {
        return Ok(v);
    }
    match serde_json::from_value::<ErrorBody>(v) {
        Ok(e) => {
            let refused = CellError::new(e.error, e.message);
            Err(match e.reason {
                Some(reason) => refused.because(reason),
                None => refused,
            })
        }
        Err(_) => Err(CellError::host(format!("{inner} answered {status}"))),
    }
}

fn decoded<T: serde::de::DeserializeOwned>(v: Value, what: &str) -> CellResult<T> {
    serde_json::from_value(v).map_err(|e| CellError::host(format!("{what}: {e}")))
}

/// Emails and pictures for identities, asked of the registry
/// `PROFILES_MAX` at a time. One it cannot answer shows as its id. Their
/// emails are shown: the sheet and the access view are of a fragment's
/// people, shown to its members (docs/cloudflare-v1.md, decision 48), and
/// an agent member's owner, who reads what it reads. A person's own agents
/// are not named (decision 63: "I invite Skyler and his agents, not Skyler
/// and his agent Agent123"): they reach what the person does, unnamed.
pub(crate) async fn profiles(env: &Env, mut ids: Vec<String>) -> BTreeMap<String, Profile> {
    ids.sort_unstable();
    ids.dedup();
    let asked = ids.chunks(crate::registry::PROFILES_MAX).map(|chunk| {
        let call = calls::Profiles { ids: chunk.to_vec(), emails_of: chunk.to_vec() };
        async move { ask_registry(env, &call).await }
    });
    let mut all = BTreeMap::new();
    for answer in join_all(asked).await {
        match answer {
            Ok(a) => all.extend(a.profiles),
            Err(e) => console_error!("sharing's profiles: {:?} {}", e.code, e.message),
        }
    }
    name_agents(env, &mut all).await;
    all
}

/// Names each agent among `profiles` by its agent fragment's face (its
/// title, and its picture's address on the platform), the one source every
/// page and the platform show an agent by (docs/api.md, "An agent's name
/// and picture"): at most `FACES_MAX` fragments asked at once. One not
/// asked, or whose fragment does not answer, keeps its label.
pub(crate) async fn name_agents(env: &Env, profiles: &mut BTreeMap<String, Profile>) {
    let mut asked: Vec<String> = vec![];
    for f in profiles.values().filter_map(|p| p.fragment.clone()) {
        if asked.len() < FACES_MAX && !asked.contains(&f) {
            asked.push(f);
        }
    }
    let none = json!({});
    let faces = join_all(asked.iter().map(|f| crate::fragment::ask(env, f, "computer/face", &none))).await;
    let faces: BTreeMap<&String, Value> = asked.iter().zip(faces).filter_map(|(f, face)| Some((f, face.ok()?))).gathered();
    let face = |f: &Option<String>, key: &str| f.as_ref().and_then(|f| faces.get(f)).and_then(|v| v[key].as_str()).map(str::to_string);
    for p in profiles.values_mut() {
        if p.kind == IdentityKind::Agent && p.fragment.is_some() {
            p.title = face(&p.fragment, "title");
            p.picture = face(&p.fragment, "picture");
        }
    }
}

/// Whether `m` is an agent in with its person, a member who is not the
/// owner (decision 64; members.rs `entity_of`): never an entry of its own,
/// nor named; its person is the one entry. The owner's agents are each in
/// as the owner chose them, and shown as theirs.
pub(crate) fn with_their_person(m: &Member, members: &[Member]) -> bool {
    m.kind == Some(IdentityKind::Agent) && m.owner.as_deref().is_some_and(|o| members.iter().any(|p| p.principal == o && p.role != Role::Owner))
}

/// The ids a member list needs named: its members, and its agent
/// members' owners.
fn named_ids(members: &[Member]) -> Vec<String> {
    members.iter().flat_map(|m| std::iter::once(m.principal.clone()).chain(m.owner.clone())).collect()
}

/// Someone as a sheet names them in a sentence: their email, an agent's
/// title or label, or their npub.
fn name_of(id: &str, p: Option<&Profile>) -> String {
    match p {
        Some(Profile { kind: IdentityKind::Agent, title: Some(t), .. }) => t.clone(),
        Some(Profile { kind: IdentityKind::Agent, name: Some(n), .. }) => n.clone(),
        Some(Profile { email: Some(e), .. }) => e.clone(),
        _ => npub::display(id),
    }
}

/// Someone as a page shows them: a picture (or an initial), their email
/// or an agent's name, and under it what they are to the fragment: an
/// agent's owner, or a person's own agents, who reach what they do.
fn person(m: &Member, sheet: &Sheet) -> String {
    let p = sheet.profiles.get(&m.principal);
    let (name, initial) = match p {
        Some(Profile { kind: IdentityKind::Agent, .. }) => (esc(&name_of(&m.principal, p)), "✦".to_string()),
        Some(Profile { email: Some(e), .. }) => (esc(e), e.chars().next().unwrap_or('?').to_uppercase().to_string()),
        _ => (format!("<code>{}</code>", esc(&npub::display(&m.principal))), "?".to_string()),
    };
    let picture = match p.and_then(|p| p.picture.as_deref()) {
        Some(src) => format!("<img class=\"av\" src=\"{}\" alt=\"\">", esc(src)),
        None => format!("<span class=\"av\" aria-hidden=\"true\">{}</span>", esc(&initial)),
    };
    let you = if m.principal == sheet.me { " <span class=\"role\">(you)</span>" } else { "" };
    let agent = matches!(p, Some(Profile { kind: IdentityKind::Agent, .. })) || m.kind == Some(IdentityKind::Agent);
    let mut under = vec![];
    if sheet.chat_agent.as_deref() == Some(m.principal.as_str()) {
        under.push("This chat's own agent".to_string());
    }
    if agent {
        under.push(match m.owner.as_deref() {
            Some(o) if o == sheet.me => "Your agent".to_string(),
            Some(o) => format!("{}'s agent: they read what it reads", esc(&name_of(o, sheet.profiles.get(o)))),
            None => "An agent".to_string(),
        });
    }
    if sheet.with_agents.contains(&m.principal) {
        under.push("With their agents".to_string());
    }
    let under = if under.is_empty() { String::new() } else { format!("<small>{}</small>", under.join(" · ")) };
    format!("{picture}<span class=\"who\">{name}{you}{under}</span>")
}

/// What the sheet shows, read as the signed-in person.
struct Sheet {
    me: String,
    /// The person's own role (`None`: they read through an agent of theirs).
    role: Option<Role>,
    /// Its members, less the agents in with their person.
    members: Vec<Member>,
    /// The people whose agents are in with them.
    with_agents: Vec<String>,
    /// The invites waiting: the owner's only.
    invites: Vec<Invite>,
    visibility: Visibility,
    /// What it is: inviting someone to a chat warns (`talks_to_owners_agent`).
    kind: FragmentKind,
    /// Its title, else its label.
    title: String,
    /// A chat's own agent, its lead (the first agent in it): it stays an
    /// editor and is not removed (members.rs `chat_agent`).
    chat_agent: Option<String>,
    /// The share link: the owner's only.
    link: Option<String>,
    /// Its address, which opens it for the people in it.
    canonical: String,
    /// It holds a key or a door, so it stays its owner's and their agents'
    /// (reach.rs): the sheet adds no one, and its Copy link is its address,
    /// never the share link.
    reach: bool,
    profiles: BTreeMap<String, Profile>,
    /// The page's form token.
    form: String,
}

impl Sheet {
    fn owner(&self) -> bool {
        self.role == Some(Role::Owner)
    }

    /// The member who signs in as `email`, if one is in.
    fn member_by_email(&self, email: &str) -> Option<&Member> {
        self.members.iter().find(|m| self.profiles.get(&m.principal).and_then(|p| p.email.as_deref()) == Some(email))
    }
}

/// What happened, shown above the sheet.
enum Flash {
    /// An invite waits on the email for its person's yes, and they were
    /// mailed (`again`: it waited already, and was renewed).
    Invited { who: String, role: Role, again: bool },
    /// They are in already, at `role`: the list changes roles.
    Already { who: String, role: Role },
    /// A change the sheet shows no other way.
    Done(&'static str),
    Refused(String),
}

async fn load(env: &Env, cfg: &Config, url: &Url, name: &str, who: &Signed, session: &str) -> CellResult<Sheet> {
    let (members, status) =
        join(ask(env, url, name, who, Method::Get, "/api/members", None), ask(env, url, name, who, Method::Get, "/api/status", None)).await;
    let members: MemberList = decoded(members?, "the members")?;
    let status = status?;
    let role = members.members.iter().find(|m| m.principal == who.id).map(|m| m.role);
    let visibility: Visibility = decoded(status["visibility"].clone(), "the status's visibility")?;
    let kind: FragmentKind = decoded(status["kind"].clone(), "the status's kind")?;
    let title = status["title"].as_str().filter(|t| !t.is_empty()).unwrap_or(label(name)).to_string();
    let chat_agent = match kind {
        FragmentKind::Chat => members.members.iter().find(|m| m.kind == Some(IdentityKind::Agent)).map(|m| m.principal.clone()),
        _ => None,
    };
    let canonical = cfg.canonical(url, name);
    let link = match (role, status["viewToken"].as_str()) {
        (Some(Role::Owner), Some(token)) => Some(format!("{canonical}?view={token}")),
        _ => None,
    };
    // an agent in with its person is no entry of its own (decision 64):
    // its person is the one, "with their agents", never named
    let all = members.members;
    let with_agents: Vec<String> = all.iter().filter(|m| with_their_person(m, &all)).filter_map(|m| m.owner.clone()).collect();
    let members: Vec<Member> = all.iter().filter(|m| !with_their_person(m, &all)).cloned().collect();
    // an agent by its title and picture, as the shell shows it, read
    // beside the owner's invites (the sheet waits on both, not one by one)
    let invites = async {
        match role {
            Some(Role::Owner) => decoded::<InviteList>(ask(env, url, name, who, Method::Get, "/api/invites", None).await?, "the invites").map(|l| l.invites),
            _ => Ok(vec![]),
        }
    };
    let (invites, profiles) = join(invites, profiles(env, named_ids(&members))).await;
    let invites = invites?;
    Ok(Sheet {
        me: who.id.clone(),
        role,
        members,
        with_agents,
        invites,
        visibility,
        kind,
        title,
        chat_agent,
        link,
        canonical,
        reach: status["reach"].as_bool() == Some(true),
        profiles,
        form: form::issue(session, &purpose("share", name), js::now_ms()),
    })
}

fn hidden(name: &str, value: &str) -> String {
    format!("<input type=\"hidden\" name=\"{}\" value=\"{}\">", esc(name), esc(value))
}

/// A form of one action, with the page's token: sent by a button of its
/// own (each armed after a moment).
fn action_form(sheet: &Sheet, action: &str, inner: &str, class: &str) -> String {
    format!("<form method=\"post\" class=\"{class}\">{}{}{inner}</form>", hidden("form", &sheet.form), hidden("action", action))
}

fn role_name(role: Role) -> &'static str {
    match role {
        Role::Owner => "Owner",
        Role::Editor => "Editor",
        Role::Viewer => "Viewer",
        Role::Public => "Anyone",
    }
}

/// Who can open it, as General access says it: an icon, its name, and
/// what it means.
fn general(v: Visibility) -> (&'static str, &'static str, &'static str) {
    match v {
        Visibility::Members => (
            "<rect x=\"5\" y=\"11\" width=\"14\" height=\"10\" rx=\"2\"/><path d=\"M8 11V7a4 4 0 0 1 8 0v4\"/>",
            "Restricted",
            "Only people with access can open it.",
        ),
        Visibility::Link => (
            "<path d=\"M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1\"/><path d=\"M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1\"/>",
            "Anyone with the link",
            "Anyone with the link can view it.",
        ),
        Visibility::Public => (
            "<circle cx=\"12\" cy=\"12\" r=\"9\"/><path d=\"M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18\"/>",
            "Public",
            "Anyone can view it, no link needed.",
        ),
    }
}

const TICK: &str = "<svg viewBox=\"0 0 24 24\" aria-hidden=\"true\"><path d=\"M5 12.5l4.5 4.5L19 7.5\"/></svg>";

fn icon(v: Visibility) -> String {
    let open = if v == Visibility::Members { "" } else { " open" };
    format!("<span class=\"icon{open}\"><svg viewBox=\"0 0 24 24\" aria-hidden=\"true\">{}</svg></span>", general(v).0)
}

/// The owner's role buttons for a member: the one they hold pressed, the
/// other a change.
fn role_buttons(sheet: &Sheet, m: &Member, who: &str) -> String {
    let button = |r: Role| match m.role == r {
        true => format!("<button type=\"button\" aria-pressed=\"true\">{}</button>", role_name(r)),
        false => format!("<button name=\"role\" value=\"{}\" aria-pressed=\"false\" data-arm disabled>{}</button>", r.as_str(), role_name(r)),
    };
    let seg = format!("<span class=\"seg\" role=\"group\" aria-label=\"{}'s role\">{}{}</span>", esc(who), button(Role::Viewer), button(Role::Editor));
    action_form(sheet, "role", &format!("{}{seg}", hidden("member", &m.principal)), "")
}

/// Why a fragment with reach is shared with no one else (reach.rs), in
/// the same words as the shell's Apps page.
const REACH_LINE: &str = "Apps that can reach your accounts stay private to you and your agents for now.";

fn render(sheet: &Sheet, flash: Option<Flash>) -> String {
    let mut out = String::new();
    match flash {
        Some(Flash::Invited { who, role, again: false }) => out += &format!(
            "<div class=\"flash\"><p>Invited <b>{w}</b> as {r}: we mailed them a link.</p></div>",
            w = esc(&who),
            r = role_phrase(role),
        ),
        Some(Flash::Invited { who, role, again: true }) => out += &format!(
            "<div class=\"flash\"><p>Invited <b>{w}</b> again, as {r}: we mailed them a new link.</p></div>",
            w = esc(&who),
            r = role_phrase(role),
        ),
        Some(Flash::Already { who, role: Role::Owner }) => out += &format!("<div class=\"flash\"><p><b>{}</b> is you: you own it.</p></div>", esc(&who)),
        Some(Flash::Already { who, role }) => out += &format!(
            "<div class=\"flash\"><p><b>{}</b> is in already, as {}.</p></div>",
            esc(&who),
            role_phrase(role)
        ),
        Some(Flash::Done(what)) => out += &format!("<div class=\"flash\"><p>{what}</p></div>"),
        Some(Flash::Refused(why)) => out += &format!("<div class=\"flash error\"><p>{}</p></div>", esc(&why)),
        None => {}
    }
    if sheet.owner() && sheet.reach {
        out += &format!("<p class=\"hint\">{REACH_LINE}</p>");
    } else if sheet.owner() {
        let radio = |value: &str, name: &str, checked: bool| {
            format!("<label><input type=\"radio\" name=\"role\" value=\"{value}\"{}><span>{name}</span></label>", if checked { " checked" } else { "" })
        };
        out += &action_form(
            sheet,
            "invite",
            &format!(
                "<input name=\"email\" type=\"email\" required maxlength=\"320\" placeholder=\"Add people by email\" autocomplete=\"off\" aria-label=\"email\">\
                 <span class=\"seg\" role=\"radiogroup\" aria-label=\"their role\">{}{}</span><button data-arm disabled>Invite</button>",
                radio("viewer", "Viewer", true),
                radio("editor", "Editor", false)
            ),
            "add",
        );
        // whoever comes into a chat can talk to the owner's agent in it
        // (Paul, 2026-10-23: the design, and this its safeguard; 10-24:
        // only on a chat, one short line, and no other paragraph here)
        if fragment_core::access::talks_to_owners_agent(sheet.kind, &sheet.members) {
            out += &format!("<p class=\"hint trust\">{}</p>", fragment_core::access::TRUST_WARNING);
        }
    }
    out += "<h2>People with access</h2><ul class=\"people\">";
    for m in &sheet.members {
        out += "<li>";
        out += &person(m, sheet);
        if sheet.owner() && m.role != Role::Owner && sheet.chat_agent.as_deref() != Some(m.principal.as_str()) {
            let who = name_of(&m.principal, sheet.profiles.get(&m.principal));
            let remove = format!(
                "{}<button class=\"text away\" data-confirm=\"Remove?\" aria-label=\"Remove {}\" data-arm disabled>Remove</button>",
                hidden("member", &m.principal),
                esc(&who)
            );
            out += &format!("<span class=\"controls\">{}{}</span>", role_buttons(sheet, m, &who), action_form(sheet, "remove", &remove, ""));
        } else {
            out += &format!("<span class=\"role\">{}</span>", role_name(m.role));
        }
        out += "</li>";
    }
    let now = js::now_ms();
    for i in &sheet.invites {
        let days_left = (i.expires_at - now) / DAY_MS;
        let until = match days_left {
            d if d >= 2 => format!("{d} days left"),
            1 => "a day left".to_string(),
            _ => "less than a day left".to_string(),
        };
        out += &format!(
            "<li><span class=\"av\" aria-hidden=\"true\">✉</span><span class=\"who\">{e}<small>Invited as {r} · {until}</small></span>{revoke}</li>",
            e = esc(&i.email),
            r = role_phrase(i.role),
            revoke = action_form(
                sheet,
                "uninvite",
                &format!("{}<button class=\"text\" aria-label=\"Revoke the invite to {e}\" data-arm disabled>Revoke</button>", hidden("email", &i.email), e = esc(&i.email)),
                "controls"
            ),
        );
    }
    out += "</ul><h2>General access</h2>";
    if sheet.owner() {
        let option = |v: Visibility| {
            let (_, named, means) = general(v);
            let face = format!("{}<span class=\"say\"><b>{named}</b><small>{means}</small></span><span class=\"tick\">{TICK}</span>", icon(v));
            match sheet.visibility == v {
                true => format!("<button type=\"button\" class=\"option\" value=\"{}\" aria-pressed=\"true\">{face}</button>", v.as_str()),
                false => format!("<button class=\"option\" name=\"visibility\" value=\"{}\" aria-pressed=\"false\" data-arm disabled>{face}</button>", v.as_str()),
            }
        };
        // with reach, only what it is now and members only (reach.rs)
        let offered = [Visibility::Members, Visibility::Link, Visibility::Public].into_iter().filter(|v| !sheet.reach || *v == Visibility::Members || *v == sheet.visibility);
        let options = offered.map(option).collect::<String>();
        out += &action_form(sheet, "visibility", &format!("<div class=\"options\" role=\"group\" aria-label=\"general access\">{options}</div>"), "");
    } else {
        let (_, named, means) = general(sheet.visibility);
        out += &format!("<div class=\"static\">{}<span><b>{named}</b><span class=\"hint\">{means}</span></span></div>", icon(sheet.visibility));
        out += "<p class=\"hint\">Only its owner changes who has access.</p>";
    }
    // the share link: its owner's, while it is what opens it
    let shared = sheet.link.as_deref().filter(|_| sheet.visibility == Visibility::Link);
    if sheet.owner() && shared.is_some() {
        out += "<section class=\"more\"><h2>Share link</h2><div class=\"row\"><p class=\"hint\">A new link turns off the old one.</p>";
        out += &action_form(sheet, "rotate", "<button class=\"quiet\" data-confirm=\"Make a new one?\" data-arm disabled>New link</button>", "");
        out += "</div></section>";
    }
    // a member's own: leaving (its owner cannot)
    let leave = match sheet.role {
        Some(r) if r != Role::Owner => action_form(sheet, "leave", "<button class=\"quiet\" data-confirm=\"Leave it?\" data-arm disabled>Leave</button>", "leave"),
        _ => String::new(),
    };
    // Copy link: the share link while it opens it, else its address (the
    // people in it open that; a public one, anyone). With reach, never the
    // share link, though one from before the rule still opens it: the sheet
    // gives it to no one more (reach.rs); New link, above, still ends it
    let copied = shared.filter(|_| !sheet.reach);
    out += &format!(
        "<footer>{leave}<button type=\"button\" class=\"quiet\" data-copy=\"{}\">Copy link</button><button type=\"button\" data-done>Done</button></footer>",
        esc(copied.unwrap_or(&sheet.canonical))
    );
    out
}

/// The platform session a request to one of sharing's pages carries, as
/// the registry is asked for it.
fn carried(req: &Request, url: &Url) -> CellResult<Option<calls::Session>> {
    Ok(auth::cookie_of(req, auth::SESSION_COOKIE, auth::secure(url), "/")?.map(|token| calls::Session { token, fragment: None, frame: false }))
}

async fn sheet(req: &Request, env: &Env, cfg: &Config, url: &Url, name: &str) -> CellResult<Response> {
    let platform = cfg.platform();
    let signed_out = || auth::to_login(&platform, &format!("/share/{name}"));
    let Some(session) = carried(req, url)? else { return signed_out() };
    let token = session.token.clone();
    let live = match ask_registry(env, &session).await {
        Err(e) if e.code == ErrorCode::Unauthenticated => return signed_out(),
        live => live?,
    };
    let who = Signed::new(live.identity, None);
    shown(env, cfg, url, name, &who, &token, None, 200).await
}

/// The sheet as it is now, with `flash` above it; a person it is not
/// shown to gets a refusal page.
#[allow(clippy::too_many_arguments)]
async fn shown(env: &Env, cfg: &Config, url: &Url, name: &str, who: &Signed, session: &str, flash: Option<Flash>, status: u16) -> CellResult<Response> {
    match load(env, cfg, url, name, who, session).await {
        Ok(sheet) => drawn(&sheet, flash, status),
        Err(e) => refused(e, name),
    }
}

/// The sheet `sheet` holds, titled for what its reader may do there.
fn drawn(sheet: &Sheet, flash: Option<Flash>, status: u16) -> CellResult<Response> {
    let title = match sheet.owner() {
        true => format!("Share “{}”", sheet.title),
        false => format!("Who has access to “{}”", sheet.title),
    };
    sheet_framable(status, &title, &render(sheet, flash))
}

/// A sheet that did not load: a refusal page for someone it is not shown
/// to, or a fragment gone.
fn refused(e: CellError, name: &str) -> CellResult<Response> {
    match e.code {
        ErrorCode::Forbidden | ErrorCode::Unauthenticated => {
            notice(403, "Not yours to share", &format!("You are not in {}, so it is not yours to share.", esc(label(name))))
        }
        ErrorCode::NotFound => notice(404, "No such fragment", &format!("There is no {}.", esc(name))),
        _ => Err(e),
    }
}

fn form_fields(bytes: &[u8]) -> BTreeMap<String, String> {
    url::form_urlencoded::parse(bytes).map(|(k, v)| (k.into_owned(), v.into_owned())).gathered()
}

/// A POST's person, once its Origin and form token hold: the token is
/// checked against the session cookie before the registry is asked who it
/// names.
pub(crate) async fn poster(req: &mut Request, env: &Env, url: &Url, platform: &str, purpose: &str) -> CellResult<Result<(String, calls::LiveSession, BTreeMap<String, String>), Response>> {
    auth::same_origin(req, platform)?;
    let bytes = crate::read_body(req, FORM_MAX_BYTES).await?;
    let fields = form_fields(&bytes);
    let Some(session) = carried(req, url)? else {
        return Ok(Err(notice(401, "Sign in first", "This page's session ended. Sign in, then open it again.")?));
    };
    if let Err(refused) = form::check(&session.token, purpose, fields.get("form").map_or("", String::as_str), js::now_ms()) {
        return Ok(Err(notice(403, "Not sent", refused.message())?));
    }
    match ask_registry(env, &session).await {
        Ok(live) => Ok(Ok((session.token, live, fields))),
        Err(e) if e.code == ErrorCode::Unauthenticated => Ok(Err(notice(401, "Sign in first", "This page's session ended. Sign in, then open it again.")?)),
        Err(e) => Err(e),
    }
}

async fn share_post(mut req: Request, env: &Env, cfg: &Config, url: &Url, name: &str) -> CellResult<Response> {
    let platform = cfg.platform();
    let (session, live, fields) = match poster(&mut req, env, url, &platform, &purpose("share", name)).await? {
        Ok(posted) => posted,
        Err(page) => return Ok(page),
    };
    let who = Signed::new(live.identity, None);
    let field = |k: &str| fields.get(k).map_or("", |v| v.trim());
    let member = || match field("member") {
        m if npub::is_identity(m) => Ok(m.to_string()),
        m => Err(CellError::invalid(format!("{m:?} is not a member"))),
    };
    let role = || match Role::parse(field("role")) {
        Some(r @ (Role::Viewer | Role::Editor)) => Ok(r),
        _ => Err(CellError::invalid("a role is viewer or editor")),
    };
    let done = match field("action") {
        "invite" => match invite(env, cfg, url, name, &who, &session, field("email"), role()).await {
            Ok(page) => return Ok(page),
            Err(e) => Err(e),
        },
        "role" => match (member(), role()) {
            (Ok(m), Ok(r)) => ask(env, url, name, &who, Method::Put, &format!("/api/members/{m}"), Some(json!({ "role": r }))).await,
            (Err(e), _) | (_, Err(e)) => Err(e),
        },
        "remove" => match member() {
            Ok(m) => ask(env, url, name, &who, Method::Delete, &format!("/api/members/{m}"), None).await,
            Err(e) => Err(e),
        },
        "uninvite" => match field("email") {
            email if fragment_core::mail::valid_address(email) => ask(env, url, name, &who, Method::Delete, &format!("/api/invites/{}", enc(email)), None).await,
            _ => Err(CellError::invalid("name an invite by its email")),
        },
        "visibility" => match Visibility::parse(field("visibility")) {
            Some(v) => ask(env, url, name, &who, Method::Put, "/api/visibility", Some(json!({ "visibility": v }))).await,
            None => Err(CellError::invalid("choose who can open it")),
        },
        // the share link only: the inbox's token is an integration's,
        // rotated with the CLI
        "rotate" => match ask(env, url, name, &who, Method::Post, "/api/rotate", Some(json!({ "scopes": ["view"] }))).await {
            Ok(_) => return shown(env, cfg, url, name, &who, &session, Some(Flash::Done("A new share link: the old one no longer opens it.")), 200).await,
            Err(e) => Err(e),
        },
        // a member's own, as `fragment members leave`: then the sheet is
        // not theirs to see
        "leave" => {
            let title = ask(env, url, name, &who, Method::Get, "/api/status", None).await.ok().and_then(|s| s["title"].as_str().map(str::to_string));
            match ask(env, url, name, &who, Method::Delete, "/api/members/me", None).await {
                Ok(_) => return left(title.as_deref().unwrap_or(label(name))),
                Err(e) => Err(e),
            }
        }
        _ => Err(CellError::invalid("no such change")),
    };
    match done {
        Ok(_) => Ok(auth::redirect(&format!("/share/{name}"), &[])?.with_status(303)),
        Err(e) if matches!(e.code, ErrorCode::InvalidRequest | ErrorCode::NotFound | ErrorCode::AlreadyExists | ErrorCode::Forbidden | ErrorCode::RateLimited) => {
            let status = e.code.status();
            // sharing refused while it has reach says so in the sheet's words
            let why = match e.reason {
                Some(fragment_proto::Reason::PrivateWhileReaching) => REACH_LINE.to_string(),
                _ => e.message,
            };
            shown(env, cfg, url, name, &who, &session, Some(Flash::Refused(why)), status).await
        }
        Err(e) => Err(e),
    }
}

/// What a member who left sees: it is not theirs to see any more.
fn left(title: &str) -> CellResult<Response> {
    sheet_framable(
        200,
        &format!("You left “{title}”"),
        "<p>It is no longer shared with you. Its owner can share it with you again.</p><footer><button type=\"button\" data-done>Done</button></footer>",
    )
}

/// Shares it with whoever signs in as `email` (decisions 48 and 63): the
/// invite waiting on it, mailed to them, theirs once they accept it.
/// Someone in already is not added again: a role changes in the list, so
/// an Add at the default role never lowers one.
#[allow(clippy::too_many_arguments)]
async fn invite(env: &Env, cfg: &Config, url: &Url, name: &str, who: &Signed, session: &str, email: &str, role: CellResult<Role>) -> CellResult<Response> {
    let role = role?;
    let email = email.trim().to_ascii_lowercase();
    if !fragment_core::mail::valid_address(&email) {
        return Err(CellError::invalid(format!("{email:?} is not an email")));
    }
    let before = match load(env, cfg, url, name, who, session).await {
        Ok(sheet) => sheet,
        Err(e) => return refused(e, name),
    };
    // (anyone but its owner is refused by the fragment, below)
    if let Some(m) = before.member_by_email(&email).filter(|_| before.owner()) {
        return drawn(&before, Some(Flash::Already { who: email, role: m.role }), 200);
    }
    let again = before.invites.iter().any(|i| i.email == email);
    let shared = ask(env, url, name, who, Method::Post, "/api/invites", Some(json!({ "email": email, "role": role }))).await?;
    let flash = match decoded::<Shared>(shared, "the invite")? {
        Shared::Invited(i) => Flash::Invited { who: email, role: i.role, again },
        Shared::Member(_) => return Err(CellError::host("an invite made a member at once: invites wait for a yes (decision 63)")),
    };
    shown(env, cfg, url, name, who, session, Some(flash), 200).await
}

/// `/api/invites…`: a person's own answers to the invites waiting on the
/// emails they sign in with (decision 63; registry/invites.rs). `GET`
/// them; `POST <fragment>/accept`, their yes, which makes them a member
/// there; `POST <fragment>/decline`, their no. Each takes no body (or
/// `{}`). Their agent answers them for them, acting `for` them (the
/// registry decides: invites.rs `invitee_by`).
pub(crate) async fn invites(mut req: Request, env: &Env, url: &Url, rest: &[&str]) -> CellResult<Response> {
    let bytes = crate::read_body(&mut req, limits::BODY_MAX_BYTES).await?;
    let (by, acting_for) = (crate::orgs::signed_by(env, &req, url, &bytes)?, crate::acting_for(url)?);
    let empty = bytes.is_empty() || serde_json::from_slice::<serde_json::Map<String, Value>>(&bytes).is_ok_and(|m| m.is_empty());
    match (req.method(), rest) {
        (Method::Get, []) => crate::json_answer(&ask_registry(env, &InvitesFor { by, acting_for }).await?),
        (Method::Post, [_, "accept" | "decline"]) if !empty => Err(CellError::invalid("this takes no body (or {})")),
        (Method::Post, [fragment, verb @ ("accept" | "decline")]) => {
            let (fragment, yes) = (crate::named_fragment(fragment)?, *verb == "accept");
            match (ask_registry(env, &AnswerInvite { by, acting_for, fragment: fragment.clone(), yes }).await?, yes) {
                (Some(role), true) => crate::json_answer(&InviteAccepted { fragment, role }),
                (None, false) => crate::json_answer(&json!({ "ok": true, "declined": fragment })),
                _ => Err(CellError::host("the registry's answer to an invite's yes or no named no role, or one after a no")),
            }
        }
        (m, _) => Err(CellError::new(ErrorCode::NotFound, format!("no route {} {}", m.as_ref(), url.path()))),
    }
}

/// `GET /api/access`: the fragments `who` owns, in their list's order,
/// each with who may open it (docs/api.md, Sharing). A fragment someone
/// else may be in or invited to (`Sharing::with_others`) is asked for its
/// members and invites as `who`, its first `ACCESS_ASKED_MAX` at most,
/// `ACCESS_ASKED_AT_ONCE` at a time; the rest have their list's counts
/// alone, which no fragment is woken for. Members are named by email (an
/// agent by its label, its title and its owner's email), at most
/// `ACCESS_PROFILES_MAX` identities.
pub(crate) async fn access(env: &Env, url: &Url, who: &Signed) -> CellResult<Access> {
    if who.kind != IdentityKind::Person {
        return Err(CellError::new(ErrorCode::Forbidden, "the access view is a person's: an agent reads each fragment's members"));
    }
    let owned: Vec<ListedFragment> = crate::listed(env, &who.id).await?.fragments.into_iter().filter(|f| f.role == Role::Owner).collect();
    let asked: Vec<String> = owned.iter().filter(|f| f.sharing.is_none_or(|s| s.with_others())).take(limits::ACCESS_ASKED_MAX).map(|f| f.name.clone()).collect();
    assert!(asked.len() <= limits::ACCESS_ASKED_MAX, "the access view asks a bounded number of fragments");
    let mut found: BTreeMap<String, (Vec<Member>, Vec<Invite>)> = BTreeMap::new();
    let mut unnamed: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for round in asked.chunks(limits::ACCESS_ASKED_AT_ONCE) {
        let answers = join_all(round.iter().map(|name| async move {
            let (members, invites) = join(ask(env, url, name, who, Method::Get, "/api/members", None), ask(env, url, name, who, Method::Get, "/api/invites", None)).await;
            let read = match (members, invites) {
                (Ok(m), Ok(i)) => decoded::<MemberList>(m, "the members").and_then(|m| Ok((m.members, decoded::<InviteList>(i, "the invites")?.invites))),
                (Err(e), _) | (_, Err(e)) => Err(e),
            };
            (name.clone(), read)
        }))
        .await;
        for (name, read) in answers {
            match read {
                // an agent in with its person is no entry of its own, nor
                // named among its agents (decision 64)
                Ok((all, invites)) => {
                    let (entity, members): (Vec<Member>, Vec<Member>) = all.iter().cloned().partition(|m| with_their_person(m, &all));
                    unnamed.insert(name.clone(), entity.into_iter().map(|m| m.principal).collect());
                    found.insert(name, (members, invites));
                }
                // deleted since its row was read, or not answering now:
                // its row keeps its counts
                Err(e) => console_error!("the access view did not read {name} ({:?}): {}", e.code, e.message),
            }
        }
    }
    // the people named, the first fragments' first, at most ACCESS_PROFILES_MAX
    let mut ids: Vec<String> = vec![];
    for name in &asked {
        for id in found.get(name).map(|(m, _)| named_ids(&m.iter().take(limits::ACCESS_MEMBERS_MAX + 1).cloned().collect::<Vec<_>>())).unwrap_or_default() {
            if !ids.contains(&id) && ids.len() < limits::ACCESS_PROFILES_MAX {
                ids.push(id);
            }
        }
    }
    let named = profiles(env, ids).await;
    let email = |id: &str| named.get(id).and_then(|p| p.email.clone());
    let fragments = owned
        .into_iter()
        .map(|f| {
            let (members, more, invites) = match found.remove(f.name.as_str()) {
                Some((members, invites)) => {
                    let others: Vec<&Member> = members.iter().filter(|m| m.principal != who.id).collect();
                    let more = others.len().saturating_sub(limits::ACCESS_MEMBERS_MAX) as u64;
                    let listed = others
                        .into_iter()
                        .take(limits::ACCESS_MEMBERS_MAX)
                        .map(|m| AccessMember {
                            principal: m.principal.clone(),
                            role: m.role,
                            kind: m.kind,
                            email: email(&m.principal),
                            name: named.get(&m.principal).and_then(|p| p.name.clone()),
                            title: named.get(&m.principal).and_then(|p| p.title.clone()),
                            owner: m.owner.clone(),
                            owner_email: m.owner.as_deref().and_then(email),
                            // a person's own agents are never named (decision 63)
                            agents: vec![],
                        })
                        .collect();
                    (Some(listed), more, invites)
                }
                None => (None, 0, vec![]),
            };
            // a guest's agent named nowhere, its members read or not (decision 64)
            let others = f.sharing.as_ref().is_none_or(|s| s.with_others());
            let agents = fragment_core::access::access_agents(f.agents, unnamed.get(&f.name).map(Vec::as_slice), others);
            AccessRow { name: f.name, kind: f.kind, title: f.title, sharing: f.sharing, agents, members, more, invites }
        })
        .collect();
    Ok(Access { fragments })
}
