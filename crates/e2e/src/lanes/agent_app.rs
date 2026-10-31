//! An app an agent made, working in a browser (2026-10-22: Paul's Hermes
//! made a shared grocery list on his self-hosted stack, and its page sat on
//! "connecting…" for good. Its own script threw before it asked for
//! anything, and nothing told anyone). The check, `app_works`, opens the
//! app as its owner does, at its own origin in Chrome, and reads what the
//! page did from its DevTools events: the signals a preview card's shot
//! hears (`card::Heard`), plus its socket's frames and its calls:
//!
//! 1. it opens as its owner, signed in at its own origin through the
//!    platform's `/auth/fragment`, and loads;
//! 2. it throws nothing and logs no error: an uncaught exception, a console
//!    error, a failed load or a refusal;
//! 3. its live connection comes up: its socket to `__live` opens and the
//!    fragment says hello;
//! 4. a second tab does the same, and when the app shares presence, each
//!    tab sees the other;
//! 5. an item typed into the first tab's text box shows there (Enter adds
//!    it, as a person adds one) and, with no reload, in the second: in each
//!    tab's list (not a line of activity naming it), a run of its own query
//!    answering with it, and the page the one that loaded (`listed`);
//! 6. the page's own query answers: every run of it returns a result, never
//!    an error.
//!
//! Every event a watched page said is read, or each check of the whole
//! visit fails, saying how many were not.
//!
//! Each failure says what the page did ("the page threw: TypeError: …"),
//! never only that a wait ran out.
//!
//! Two lanes run it:
//!
//! - **`hermes`** (local, by name) runs it on the app Hermes makes from its
//!   chat on the scripted model. That app is fixed (the todo template), so a
//!   failure is the platform's: serving, live, presence, origins, or a
//!   session's cookies on `*.localhost`.
//! - **`agent-app`**, this section, runs it on what a real agent makes when
//!   asked in words for a shared grocery list, so it also tests what the
//!   agent made. It runs on a preview's Hermes image on its real model, by
//!   name (`cargo xtask e2e --hosted … --only agent-app`), for up to half an
//!   hour, spending up to `PAID_CALLS` of the run's paid calls. A local run
//!   never has a real agent (needs.rs `RealAgent`).

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use fragment_core::card::Heard;
use fragment_nip98::Keys;
use fragment_proto::PageErrorKind;
use serde_json::{json, Value};

use super::agent_smoke::live_app;
use super::computers::{phase, turn_of, work_of, QUEUE_DRAIN};
use super::jobs::records;
use super::ledger::entries;
use crate::api::{now_s, Api};
use crate::browser::{Browser, Page, WATCHED_EVENTS_MAX};
use crate::{Need, Suite};

pub const SECTION: &str = "agent-app";

/// The paid calls the run lends the section's person: one turn that makes
/// an app (a model call per tool step: a skill read, the app's files, its
/// deploy, a look at it) and Hermes' title.
const PAID_CALLS: u64 = 30;
/// A new computer's first start on a preview, to its agent following its chat.
const FIRST_START: Duration = Duration::from_secs(10 * 60);
/// The turn that makes the app and deploys it: a real model writing files.
const WORK: Duration = Duration::from_secs(30 * 60);
/// Its owner's sleep, asked until it is asleep.
const SLEEP: Duration = Duration::from_secs(3 * 60);
const POLL: Duration = Duration::from_secs(3);

/// A page, through `/auth/fragment`'s sign-in, to its load.
const LOAD: Duration = Duration::from_secs(30);
/// A loaded page's socket to its hello.
const LIVE: Duration = Duration::from_secs(20);
/// After its hello, a page's scripts render for this long before it is read
/// (as a card's shot waits: `card::SETTLE_MS`).
const SETTLE: Duration = Duration::from_millis(fragment_core::card::SETTLE_MS);
/// An item added, to it showing in each tab.
const CHANGE: Duration = Duration::from_secs(15);
/// A page's query runs and calls, settled, to their answers.
const ANSWERED: Duration = Duration::from_secs(5);
/// What the detail of a failure lists of each kind (errors, queries, calls).
const LISTED_MAX: usize = 5;

/// Asks `f` every `POLL` until it answers, or `bound` passes.
#[track_caller]
fn within<T>(bound: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let at = std::panic::Location::caller();
    let t0 = Instant::now();
    loop {
        if let Some(v) = f() {
            return Some(v);
        }
        if t0.elapsed() >= bound {
            println!("      (a wait ran out its {:?} at {}:{})", bound, at.file(), at.line());
            return None;
        }
        std::thread::sleep(POLL);
    }
}

/// What the person's paid calls were, from their ledger.
fn paid(api: &Api, identity: &str) -> usize {
    entries(api, identity, "aig:").len() + entries(api, identity, "step:").len()
}

pub fn agent_app(s: &mut Suite, api: &Api) -> Result<()> {
    let why = format!("it asks a real agent on a Hermes computer to make an app, for up to half an hour, and spends up to {PAID_CALLS} of the run's paid calls");
    if !s.section_by_name(SECTION, &[Need::Levers, Need::Computers, Need::Models, Need::Chrome, Need::RealAgent], &why) {
        return Ok(());
    }
    anyhow::ensure!(api.signs_in_by_levers(), "a real agent's person signs in through a preview's levers");
    let evidence = s.dir(SECTION);
    let keys = Keys::generate();
    let (session, owner_id) = api.e2e_sign_in(&Api::email_of(&keys), PAID_CALLS)?;
    let me = api.approve(&session, &keys)?;
    let email = me.body["email"].as_str().context("the person has an email")?.to_string();
    println!("      ({email} as {owner_id}, lent {PAID_CALLS} paid calls)");
    // their skills fragment, as the shell makes a person's: the managed
    // skills (apps-finite among them) an agent making an app reads
    let skills = api.create_with(&keys, json!({ "label": s.name("skills"), "template": "skills" }))?;
    let made = api.signed(&keys, "POST", "/api/computers", Some(&json!({})))?;
    let id = made.body["computer"].as_str().unwrap_or("").to_string();
    let label = s.name("juniper");
    let (agent, identity) = super::agent_ask::agent(api, &keys, &id, &label, "Juniper", &format!("You are Juniper, {email}'s agent. Help with whatever they ask.\n"))?;
    let chat = api.create_with(&keys, json!({ "label": format!("{label}-chat"), "template": "chat", "title": "Juniper" }))?;
    let chat_name = chat.body["name"].as_str().unwrap_or("").to_string();
    let joined = api.signed(&keys, "PUT", &format!("/api/f/{chat_name}/members/{identity}"), Some(&json!({ "role": "editor" })))?;
    let _ = api.signed(&keys, "POST", &format!("/api/computers/{id}/wake"), Some(&json!({})));
    let set_up = skills.status == 200 && made.status == 200 && chat.status == 200 && joined.status == 200;
    s.ok("an e2e person's agent on their computer, as the shell makes it (their skills fragment too), and its chat with them", set_up, json!({ "skills": skills.status, "computer": made.body, "chat": chat.body }));
    if !set_up {
        return Ok(());
    }
    let following = || {
        let subs = api.signed(&keys, "GET", &format!("/api/f/{chat_name}/subscriptions"), None).ok()?;
        let follows = subs.body["subscriptions"].as_array()?.iter().any(|x| x["wake"] == true && x["principal"] == identity.as_str() && x["channel"] == "chat");
        (phase(api, &keys, &id) == "awake" && follows).then_some(())
    };
    let ready = within(FIRST_START, following).is_some();
    let view = api.signed(&keys, "GET", &format!("/api/computers/{id}"), None).map(|r| r.body).unwrap_or(Value::Null);
    s.ok("its computer wakes, and the agent follows its chat", ready, &view);
    if !ready {
        let _ = api.signed(&keys, "POST", &format!("/api/computers/{id}/sleep"), Some(&json!({})));
        return Ok(());
    }

    // ---- asked in words for a shared list, as Paul asked his
    let app_label = s.name("groceries");
    let ask = format!(
        "Make me a shared grocery list app called {app_label}, and deploy it. One page: a text box where typing an item and pressing Enter adds it to the list, \
         and the list updates live for everyone who has the page open. Keep the list in the app's own state, read by a query. Tell me when it's live."
    );
    let said = api.signed(&keys, "POST", &format!("/api/f/{chat_name}/channels/chat"), Some(&json!({ "id": "app-1", "body": { "text": ask } })))?;
    let seq = said.body["record"]["seq"].as_i64().context("a post answers its record's seq")?;
    let turn = turn_of(&agent, &chat_name, "chat", seq);
    let t0 = Instant::now();
    let work = || work_of(&records(api, &keys, &chat_name, "work"), &turn).into_iter().map(|r| r["body"].clone()).collect::<Vec<Value>>();
    let ended = within(WORK, || work().into_iter().find(|w| w["kind"] == "turn.end"));
    if ended.is_none() {
        // stopped, as its asker would, so what it left is what is checked
        let stop = json!({ "id": "app-stop", "body": { "kind": "stop", "turn": turn } });
        let _ = api.signed(&keys, "POST", &format!("/api/f/{chat_name}/channels/chat"), Some(&stop));
    }
    let steps = work();
    let line = |t: &str| t.chars().take(160).collect::<String>().replace('\n', " ");
    let tools: Vec<String> = steps.iter().filter(|w| w["kind"] == "turn.step").map(|w| format!("{} {}", w["tool"].as_str().unwrap_or(""), line(w["args"].as_str().unwrap_or("")))).collect();
    println!("      (its turn: {:.1?}, ended {}; {} steps)", t0.elapsed(), ended.as_ref().map(|e| e["outcome"].clone()).unwrap_or(Value::Null), tools.len());
    for t in &tools {
        println!("      (  {t})");
    }
    std::fs::write(evidence.join("turn.json"), serde_json::to_vec_pretty(&json!({ "ask": ask, "work": steps }))?)?;
    let live = live_app(api, &keys, &app_label);
    s.ok(
        &format!("asked for a shared grocery list called {app_label}, the agent makes it and deploys it: theirs, and live, within {} min (model-dependent)", WORK.as_secs() / 60),
        live.is_some() && ended.is_some(),
        json!({ "live": live, "ended": ended, "steps": tools }),
    );
    if let Some(name) = live.as_ref().and_then(|v| v["listed"]["name"].as_str()) {
        match s.browser()? {
            None => s.ok("Chrome is installed to open the app (set CHROME_BIN)", false, "no Chrome found"),
            Some(mut chrome) => app_works(s, api, &mut chrome, &session, &keys, name, Made::ByAgent, &evidence)?,
        }
    }
    let spent = paid(api, &owner_id);
    s.ok(&format!("its paid calls stay within the {PAID_CALLS} lent"), spent as u64 <= PAID_CALLS, spent);

    // asleep at the end; the run's sweep deletes its `e2e-<run>-` fragments
    std::thread::sleep(QUEUE_DRAIN);
    let slept = within(SLEEP, || {
        let r = api.signed(&keys, "POST", &format!("/api/computers/{id}/sleep"), Some(&json!({}))).ok()?;
        (r.body["phase"] == "asleep").then_some(())
    });
    s.ok("its computer sleeps at the end", slept.is_some(), phase(api, &keys, &id));
    println!("      (its turn and the app's pages: {})", evidence.display());
    Ok(())
}

// ---- the check

/// Who made the app the check opens, which says whose a failure is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Made {
    /// Fixed commands on the scripted model: the todo template, so a
    /// failure is the platform's.
    Fixed,
    /// A real agent, in words: a failure may be its page's.
    ByAgent,
}

impl Made {
    fn what(self) -> &'static str {
        match self {
            Made::Fixed => "the app Hermes made (the todo template)",
            Made::ByAgent => "the app the agent made",
        }
    }

    /// A script that says whether the page's list shows `item`: its list,
    /// not anywhere in the page, which a line of activity naming it also is
    /// (Alex's review of #547). With `listed`, whose evidence is the page's
    /// own query answering with it, this is the check that the list the
    /// query backs is what changed.
    fn in_list(self, item: &str) -> String {
        let item = serde_json::to_string(&item.to_lowercase()).expect("a string encodes");
        match self {
            // the todo template's list is `#todos`, an item's text its li's
            // span; its activity feed, `#activity`, says `added “<item>”`
            // (templates/todo/site/index.html)
            Made::Fixed => format!("[...document.querySelectorAll('#todos > li > span')].some((e) => e.textContent.trim().toLowerCase() === {item})"),
            // an app whose markup the check cannot know: an element shown
            // whose own text (its text nodes, not its children's) is the
            // item, give or take marks around it (a bullet, an emoji, a ×).
            // A list shows an item as itself; a line of activity says more
            // around it (`added “<item>”`, `Ann added <item>`).
            Made::ByAgent => IN_LIST_BY_AGENT.replace("ITEM", &item),
        }
    }
}

const IN_LIST_BY_AGENT: &str = "[...document.body.querySelectorAll('*')].some((e) => e.getClientRects().length > 0 && \
    [...e.childNodes].filter((n) => n.nodeType === Node.TEXT_NODE).map((n) => n.data).join('') \
    .toLowerCase().replace(/\\s+/g, ' ').replace(/^[^\\p{L}\\p{N}]+|[^\\p{L}\\p{N}]+$/gu, '') === ITEM)";

/// Opens the app `name` as its owner (`session`, their platform session) in
/// two tabs at its own origin, and checks it works (the module's list),
/// each failure saying what the page did. Its pages' shots and what each
/// did go to `evidence`.
#[allow(clippy::too_many_arguments)]
pub(super) fn app_works(s: &mut Suite, api: &Api, chrome: &mut Browser, session: &str, owner: &Keys, name: &str, made: Made, evidence: &Path) -> Result<()> {
    let what = made.what();
    if made == Made::Fixed {
        println!("      (the app is fixed: what fails below is the platform's)");
    }
    let origin = api.site_origin(name);
    let url = api.site_url(name, "");
    // which of its operations are queries: its installed manifest's
    let status = api.status(owner, name)?;
    let kinds: BTreeMap<String, String> =
        status.body["code"]["operations"].as_object().into_iter().flatten().map(|(op, o)| (op.clone(), o["kind"].as_str().unwrap_or("").to_string())).collect();
    chrome.set_cookie(&format!("{}/", api.base), "fragment_session", session)?;

    // ---- the first tab
    let t0 = Instant::now();
    let a = chrome.open_watched(&url)?;
    let (loaded, a_seen) = load(s, chrome, &a, &origin)?;
    // its document as it loaded, signed in: one more is a reload, and a
    // page reloaded to show a change is no live update (Alex's review of #547)
    let a_loads = a_seen.loads();
    println!("      ({name}'s first tab: loaded {loaded} and live {} in {:.1?})", a_seen.live(), t0.elapsed());
    let text = |chrome: &mut Browser, page: &Page| chrome.eval(page, "document.body ? document.body.innerText.slice(0, 300) : ''").unwrap_or(Value::Null);
    let opened = loaded && a_seen.document().is_some_and(|(at, status)| status == 200 && at.starts_with(&origin));
    s.ok(
        &format!("{what}: opened as its owner, its page loads at its own origin (signed in there through the platform's /auth/fragment)"),
        opened,
        json!({ "why": a_seen.why(&kinds), "documents": a_seen.documents, "said": text(chrome, &a) }),
    );
    s.ok(&format!("{what}: its page throws nothing and logs no error as it loads (a card shot's signals)"), a_seen.quiet(), a_seen.why(&kinds));
    s.ok(&format!("{what}: its live connection comes up: its socket to {origin}/__live opens, and the fragment says hello"), a_seen.live(), a_seen.why(&kinds));
    if !opened {
        let _ = chrome.screenshot(&a, &evidence.join("app-first-tab.png"));
        write_seen(evidence, &[("first", &a_seen)], &kinds)?;
        return Ok(());
    }

    // ---- a second tab, the same person's
    let b = chrome.open_watched(&url)?;
    let (b_loaded, b_seen) = load(s, chrome, &b, &origin)?;
    let b_loads = b_seen.loads();
    s.ok(&format!("{what}: a second tab opens it the same way: it loads, throws nothing, and its live connection comes up"), b_loaded && b_seen.quiet() && b_seen.live(), b_seen.why(&kinds));
    let a_seen = read(chrome, &a, &origin)?;
    let present = format!("{what}: it shares presence, and each tab sees the other: the second's hello lists the first, and the first hears the second arrive");
    match a_seen.presence_sent + b_seen.presence_sent {
        0 => s.skip(&present, "its page shares no presence"),
        _ => {
            let seen = s.eventually(CHANGE, || read(chrome, &a, &origin).is_ok_and(|a| a.others_heard > 0));
            let a_seen = read(chrome, &a, &origin)?;
            s.ok(&present, seen && b_seen.present_at_hello() > 0, json!({ "first": a_seen.presence(), "second": b_seen.presence() }));
        }
    }

    // ---- an item added in the first tab, as a person adds one
    let item = format!("oat milk {:04x}", now_s() % 0x10000);
    let in_list = made.in_list(&item);
    let typed = chrome.eval(&a, FOCUS_BOX)?.as_bool() == Some(true);
    let shown = typed && {
        chrome.type_text(&a, &item)?;
        chrome.press(&a, "Enter", false)?;
        chrome.until(&a, &in_list, CHANGE)
    };
    let a_seen = settled(chrome, &a, &origin)?;
    let a_listed = match typed {
        true => listed_in(chrome, &a, &a_seen, &kinds, shown, &item, a_loads),
        false => Err("the page has no text box to type an item into (an enabled, visible input of type text)".into()),
    };
    let a_why = with_bodies(chrome, &a, &a_seen, &kinds);
    s.ok(
        &format!("{what}: an item typed into the first tab's text box, Enter pressed, shows in its list there: its own query answers with it, and the page is the one that loaded"),
        a_listed.is_ok(),
        format!("{}{a_why}; it says: {}", a_listed.as_ref().err().map(|e| format!("{e}; ")).unwrap_or_default(), text(chrome, &a)),
    );
    let b_shown = typed && chrome.until(&b, &in_list, CHANGE);
    let b_seen = settled(chrome, &b, &origin)?;
    let b_listed = match typed {
        true => listed_in(chrome, &b, &b_seen, &kinds, b_shown, &item, b_loads),
        false => Err("nothing was typed in the first tab".into()),
    };
    s.ok(
        &format!("{what}: and, with no reload, it shows in the second tab's list (live): its query runs again and answers with it"),
        b_listed.is_ok(),
        format!("{}{}; it says: {}", b_listed.as_ref().err().map(|e| format!("{e}; ")).unwrap_or_default(), b_seen.why(&kinds), text(chrome, &b)),
    );

    // ---- its own query, and the whole visit's errors
    let answered = all_answered(&[("first", &a_seen), ("second", &b_seen)], &kinds);
    s.ok(
        &format!("{what}: the page's own query answers: it ran its query (over its socket or HTTP), and every run returned a result, never an error"),
        answered.is_ok(),
        answered.err().unwrap_or_default(),
    );
    s.ok(&format!("{what}: neither tab threw or logged an error while it was used"), a_seen.quiet() && b_seen.quiet(), format!("first: {}; second: {}", a_seen.why(&kinds), b_seen.why(&kinds)));
    let _ = chrome.screenshot(&a, &evidence.join("app-first-tab.png"));
    let _ = chrome.screenshot(&b, &evidence.join("app-second-tab.png"));
    write_seen(evidence, &[("first", &a_seen), ("second", &b_seen)], &kinds)?;
    chrome.close(b)?;
    chrome.close(a)?;
    Ok(())
}

/// `listed`, for a page open in `chrome`: the bodies of its query's runs
/// over HTTP are read only when its socket's answers lack the item.
fn listed_in(chrome: &mut Browser, page: &Page, seen: &Seen, kinds: &BTreeMap<String, String>, shown: bool, item: &str, loads: usize) -> Result<(), String> {
    let http: Vec<String> = match seen.answered_with(item) {
        true => vec![],
        false => seen
            .calls
            .iter()
            .rev()
            .filter(|c| c.status == Some(200) && kinds.get(&c.op).is_some_and(|k| k == "query"))
            .take(LISTED_MAX)
            .filter_map(|c| chrome.response_body(page, &c.request).ok())
            .collect(),
    };
    listed(seen, shown, &http, item, loads)
}

/// Whether a tab's list shows `item`, from the app's own state: `shown`,
/// its list shows it (`Made::in_list`, read in the page); a run of its own
/// query answered with it, over its socket or HTTP (`http`, the bodies of
/// its runs there), so what it shows is its query's, run again (Alex's
/// review of #547: a line of activity names the item too, with the list
/// never run again); its document is the one that loaded, its first `loads`
/// documents and no more (a page reloaded to show a change is no live
/// update); and every event it said was read. `Err` says what failed.
fn listed(seen: &Seen, shown: bool, http: &[String], item: &str, loads: usize) -> Result<(), String> {
    let mut why: Vec<String> = vec![];
    if let Some(at) = seen.loaded_since(loads) {
        why.push(format!("the page loaded again while the item was added (at {at}): a reload, which is no live update"));
    }
    if !shown {
        why.push(format!("its list does not show “{item}”"));
    }
    if !seen.answered_with(item) && !http.iter().any(|b| names(b, item)) {
        why.push(format!("no run of its own query answered with “{item}”: its list was not run again from the app's state"));
    }
    why.extend(seen.unread());
    match why.is_empty() {
        true => Ok(()),
        false => Err(why.join("; ")),
    }
}

/// Whether every run of the pages' own query answered a result, never an
/// error, and they ran one; with every event they said read (a run past
/// what a watched page keeps may have failed). `Err` says why not.
fn all_answered(pages: &[(&str, &Seen)], kinds: &BTreeMap<String, String>) -> Result<(), String> {
    let queries: Vec<Value> = pages.iter().flat_map(|(_, seen)| seen.queries(kinds)).collect();
    let mut why: Vec<String> = vec![];
    if queries.is_empty() {
        let first = pages.first().map(|(_, seen)| seen.why(kinds)).unwrap_or_default();
        why.push(format!("the page ran no query of its own (its manifest's operations: {}); {first}", json!(kinds)));
    }
    let failed: Vec<&Value> = queries.iter().filter(|q| q["answer"] != "result").take(LISTED_MAX).collect();
    if !failed.is_empty() {
        why.push(json!(failed).to_string());
    }
    why.extend(pages.iter().filter_map(|(tab, seen)| seen.unread().map(|u| format!("the {tab} tab: {u}"))));
    match why.is_empty() {
        true => Ok(()),
        false => Err(why.join("; ")),
    }
}

/// Focuses the page's text box (its first enabled, visible text input):
/// whether there is one.
const FOCUS_BOX: &str = "(() => { const box = [...document.querySelectorAll('input:not([type]), input[type=text], input[type=search]')] \
    .find((e) => !e.disabled && !e.readOnly && e.getClientRects().length > 0); \
    if (!box) return false; box.scrollIntoView({ block: 'nearest' }); box.focus(); return document.activeElement === box; })()";

/// A watched page to its load at `origin` (`LOAD`), then its live socket's
/// hello (`LIVE`) and `SETTLE`: whether it loaded, and what it did.
fn load(s: &Suite, chrome: &mut Browser, page: &Page, origin: &str) -> Result<(bool, Seen)> {
    let at = format!("location.origin === {} && document.readyState === 'complete'", serde_json::to_string(origin)?);
    let loaded = chrome.until(page, &at, LOAD);
    if loaded {
        s.eventually(LIVE, || read(chrome, page, origin).is_ok_and(|seen| seen.live()));
        std::thread::sleep(SETTLE);
    }
    Ok((loaded, read(chrome, page, origin)?))
}

/// What the page did, `SETTLE` after a change, once none of its query runs
/// or calls is waiting on an answer (at most `ANSWERED` more): the reading
/// that found none, so a run sent after it is not one left unanswered.
fn settled(chrome: &mut Browser, page: &Page, origin: &str) -> Result<Seen> {
    std::thread::sleep(SETTLE);
    let t0 = Instant::now();
    loop {
        let seen = read(chrome, page, origin)?;
        if seen.pending() == 0 || t0.elapsed() >= ANSWERED {
            return Ok(seen);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn read(chrome: &mut Browser, page: &Page, origin: &str) -> Result<Seen> {
    let w = chrome.watched(page)?;
    Ok(Seen::read(&w.events, w.dropped, origin))
}

/// `why`, with the body of each of the page's calls that failed (what the
/// fragment said: a schema's refusal, an operation it lacks).
fn with_bodies(chrome: &mut Browser, page: &Page, seen: &Seen, kinds: &BTreeMap<String, String>) -> String {
    let failed: Vec<String> = seen
        .calls
        .iter()
        .filter(|c| c.status.is_some_and(|st| st >= 400))
        .take(LISTED_MAX)
        .map(|c| format!("{} answered {}: {}", c.op, c.status.unwrap_or(0), chrome.response_body(page, &c.request).unwrap_or_default().chars().take(300).collect::<String>()))
        .collect();
    match failed.is_empty() {
        true => seen.why(kinds),
        false => format!("{}; {}", seen.why(kinds), failed.join("; ")),
    }
}

fn write_seen(evidence: &Path, pages: &[(&str, &Seen)], kinds: &BTreeMap<String, String>) -> Result<()> {
    let pages: BTreeMap<&str, Value> = pages.iter().map(|(tab, seen)| (*tab, seen.summary(kinds))).collect();
    std::fs::write(evidence.join("app-pages.json"), serde_json::to_vec_pretty(&pages)?)?;
    Ok(())
}

// ---- what a page did, from its DevTools events

/// One of the page's sockets to its fragment's `__live`.
#[derive(Debug, Clone, Default, PartialEq)]
struct Live {
    request: String,
    url: String,
    /// Its handshake's answer: 101 when it opened.
    status: Option<u16>,
    /// Its hello: this page's id there, and how many were present.
    hello: Option<(String, usize)>,
    closed: bool,
    /// A frame the browser could not read, or a failed handshake.
    error: Option<String>,
    /// The fragment's `error` frames.
    said: Vec<String>,
}

/// One run of a query the page asked for, and its answer: `Ok` its result
/// (as JSON), `Err` what the fragment answered instead.
#[derive(Debug, Clone, PartialEq)]
struct Query {
    op: String,
    /// Its socket's request and the query's id there.
    id: String,
    answer: Option<Result<String, String>>,
}

/// A call over HTTP to one of its operations (`__op/<op>`).
#[derive(Debug, Clone, PartialEq)]
struct HttpCall {
    request: String,
    op: String,
    /// Its answer's status, once it came.
    status: Option<u16>,
    /// The browser's reason when it never came.
    failed: Option<String>,
}

/// What a page did, read from its `Runtime`, `Log` and `Network` events.
#[derive(Debug, Default)]
struct Seen {
    /// Its errors, as a preview card's shot hears them.
    heard: Heard,
    /// Its documents' answers, in order: the redirects and the page (URLs
    /// without their queries: a sign-in's token is no evidence).
    documents: Vec<(String, u16)>,
    sockets: Vec<Live>,
    /// The runs of its queries over its sockets.
    asked: Vec<Query>,
    calls: Vec<HttpCall>,
    /// Presence it sent, and others' it heard (not its own).
    presence_sent: usize,
    others_heard: usize,
    /// Its main frame's documents, in order, each its loader's (a
    /// navigation's, a reload's; a redirect is its request's, so the same
    /// load) and where it ended: the first document's frame is its main one.
    loads: Vec<(String, String)>,
    main_frame: Option<String>,
    /// Events it said past what a watched page keeps.
    dropped: u64,
}

/// Whether `text` (a query's answer, an element's) names `item`, in any case.
fn names(text: &str, item: &str) -> bool {
    text.to_lowercase().contains(&item.to_lowercase())
}

/// A URL without its query and fragment.
fn bare(url: &str) -> String {
    url.split(['?', '#']).next().unwrap_or("").to_string()
}

/// Whether `url` is a socket to `origin`'s `__live` (`http`'s origin is
/// `ws`'s, `https`'s `wss`'s).
fn is_live(url: &str, origin: &str) -> bool {
    let ws = origin.replacen("https://", "wss://", 1).replacen("http://", "ws://", 1);
    bare(url) == format!("{ws}/__live")
}

impl Seen {
    fn read(events: &[Value], dropped: u64, origin: &str) -> Seen {
        let mut seen = Seen { dropped, ..Seen::default() };
        let mut documents: BTreeMap<String, String> = BTreeMap::new();
        for e in events {
            seen.heard.hear(e);
            let p = &e["params"];
            let request = p["requestId"].as_str().unwrap_or("").to_string();
            match e["method"].as_str().unwrap_or("") {
                "Network.requestWillBeSent" => {
                    let url = p["request"]["url"].as_str().unwrap_or("");
                    if p["type"] == "Document" {
                        if let Some(status) = p["redirectResponse"]["status"].as_u64() {
                            seen.documents.push((bare(p["redirectResponse"]["url"].as_str().unwrap_or("")), status as u16));
                        }
                        documents.insert(request.clone(), url.to_string());
                        let frame = p["frameId"].as_str().unwrap_or("");
                        if seen.main_frame.get_or_insert_with(|| frame.to_string()) == frame {
                            let loader = p["loaderId"].as_str().unwrap_or(&request).to_string();
                            match seen.loads.last_mut() {
                                Some((l, at)) if *l == loader => *at = bare(url),
                                _ => seen.loads.push((loader, bare(url))),
                            }
                        }
                    }
                    if let Some(op) = bare(url).strip_prefix(&format!("{origin}/__op/")) {
                        if !seen.calls.iter().any(|c| c.request == request) {
                            seen.calls.push(HttpCall { request, op: op.to_string(), status: None, failed: None });
                        }
                    }
                }
                "Network.responseReceived" => {
                    let status = p["response"]["status"].as_u64().unwrap_or(0) as u16;
                    if p["type"] == "Document" {
                        seen.documents.push((bare(p["response"]["url"].as_str().unwrap_or("")), status));
                    }
                    if let Some(c) = seen.calls.iter_mut().find(|c| c.request == request) {
                        c.status = Some(status);
                    }
                }
                "Network.loadingFailed" => {
                    let why = p["errorText"].as_str().unwrap_or("failed").to_string();
                    if let Some(url) = documents.get(&request) {
                        seen.documents.push((format!("{} ({why})", bare(url)), 0));
                    }
                    if let Some(c) = seen.calls.iter_mut().find(|c| c.request == request) {
                        c.failed = Some(why);
                    }
                }
                "Network.webSocketCreated" if is_live(p["url"].as_str().unwrap_or(""), origin) => {
                    seen.sockets.push(Live { request, url: bare(p["url"].as_str().unwrap_or("")), ..Live::default() });
                }
                "Network.webSocketHandshakeResponseReceived" => {
                    if let Some(l) = seen.socket(&request) {
                        l.status = p["response"]["status"].as_u64().map(|s| s as u16);
                    }
                }
                "Network.webSocketFrameError" => {
                    if let Some(l) = seen.socket(&request) {
                        l.error = p["errorMessage"].as_str().map(str::to_string);
                    }
                }
                "Network.webSocketClosed" => {
                    if let Some(l) = seen.socket(&request) {
                        l.closed = true;
                    }
                }
                "Network.webSocketFrameSent" if seen.has(&request) => {
                    let m: Value = serde_json::from_str(p["response"]["payloadData"].as_str().unwrap_or("")).unwrap_or(Value::Null);
                    match m["type"].as_str() {
                        Some("query") => seen.asked.push(Query {
                            op: m["op"].as_str().unwrap_or("").to_string(),
                            id: format!("{request}/{}", m["id"].as_str().unwrap_or("")),
                            answer: None,
                        }),
                        Some("presence") => seen.presence_sent += 1,
                        _ => {}
                    }
                }
                "Network.webSocketFrameReceived" if seen.has(&request) => {
                    let m: Value = serde_json::from_str(p["response"]["payloadData"].as_str().unwrap_or("")).unwrap_or(Value::Null);
                    let own = seen.socket(&request).and_then(|l| l.hello.as_ref().map(|(id, _)| id.clone()));
                    match m["type"].as_str() {
                        Some("hello") => {
                            let hello = (m["id"].as_str().unwrap_or("").to_string(), m["presence"].as_array().map_or(0, Vec::len));
                            if let Some(l) = seen.socket(&request) {
                                l.hello = Some(hello);
                            }
                        }
                        Some("result") => {
                            let id = format!("{request}/{}", m["id"].as_str().unwrap_or(""));
                            let answer = match m.get("error") {
                                None | Some(Value::Null) => Ok(m["result"].to_string()),
                                Some(e) => Err(format!("{} {}: {}", m["status"], e.as_str().unwrap_or(""), m["message"].as_str().unwrap_or(""))),
                            };
                            // a live query's runs share its id, one at a time
                            if let Some(q) = seen.asked.iter_mut().find(|q| q.id == id && q.answer.is_none()) {
                                q.answer = Some(answer);
                            }
                        }
                        Some("presence") if !m["data"].is_null() && own.as_deref().is_some_and(|own| m["id"].as_str() != Some(own)) => seen.others_heard += 1,
                        Some("error") => {
                            if let Some(l) = seen.socket(&request) {
                                l.said.push(m["message"].as_str().unwrap_or("").to_string());
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        seen
    }

    fn has(&self, request: &str) -> bool {
        self.sockets.iter().any(|l| l.request == request)
    }

    fn socket(&mut self, request: &str) -> Option<&mut Live> {
        self.sockets.iter_mut().find(|l| l.request == request)
    }

    /// The page's document as it stands: the last answer, and where.
    fn document(&self) -> Option<(&str, u16)> {
        self.documents.last().map(|(at, status)| (at.as_str(), *status))
    }

    /// Its live connection is up: a socket to `__live` that opened, was
    /// said hello, and is open still.
    fn live(&self) -> bool {
        self.sockets.iter().any(|l| l.status == Some(101) && l.hello.is_some() && !l.closed)
    }

    /// It threw nothing and logged no error, and every event it said was
    /// read (one past what a watched page keeps may be a throw).
    fn quiet(&self) -> bool {
        self.heard.errors.is_empty() && self.heard.dropped == 0 && self.dropped == 0
    }

    /// That some of what it did is unread: its events past what a watched
    /// page keeps (`Watched::keep` keeps the first).
    fn unread(&self) -> Option<String> {
        (self.dropped > 0).then(|| format!("{} of its events came past the {WATCHED_EVENTS_MAX} a watched page keeps, so what it did after them is unread", self.dropped))
    }

    /// How many documents its main frame has loaded.
    fn loads(&self) -> usize {
        self.loads.len()
    }

    /// Where its main frame loaded again after its first `loads` documents:
    /// the first such load's.
    fn loaded_since(&self, loads: usize) -> Option<&str> {
        self.loads.get(loads).map(|(_, at)| at.as_str())
    }

    /// Whether a run of its own query over its socket answered with `item`.
    fn answered_with(&self, item: &str) -> bool {
        self.asked.iter().any(|q| matches!(&q.answer, Some(Ok(result)) if names(result, item)))
    }

    /// How many were present when its open socket was said hello.
    fn present_at_hello(&self) -> usize {
        self.sockets.iter().rev().find_map(|l| l.hello.as_ref().map(|(_, n)| *n)).unwrap_or(0)
    }

    /// Runs of its queries not yet answered.
    fn pending(&self) -> usize {
        self.asked.iter().filter(|q| q.answer.is_none()).count() + self.calls.iter().filter(|c| c.status.is_none() && c.failed.is_none()).count()
    }

    /// Every run of a query the page made: over its socket, and over HTTP
    /// (a call to an operation its manifest says is a query).
    fn queries(&self, kinds: &BTreeMap<String, String>) -> Vec<Value> {
        let answer = |a: &Option<Result<String, String>>| match a {
            None => json!("no answer"),
            Some(Ok(_)) => json!("result"),
            Some(Err(e)) => json!(e),
        };
        let socket = self.asked.iter().map(|q| json!({ "op": q.op, "via": "socket", "answer": answer(&q.answer) }));
        let http = self.calls.iter().filter(|c| kinds.get(&c.op).is_some_and(|k| k == "query")).map(|c| {
            let a = match (c.status, &c.failed) {
                (Some(200), _) => Some(Ok(String::new())),
                (Some(st), _) => Some(Err(format!("{st}"))),
                (None, Some(why)) => Some(Err(why.clone())),
                (None, None) => None,
            };
            json!({ "op": c.op, "via": "http", "answer": answer(&a) })
        });
        socket.chain(http).collect()
    }

    fn presence(&self) -> Value {
        json!({ "sent": self.presence_sent, "othersHeard": self.others_heard, "presentAtHello": self.present_at_hello() })
    }

    /// The page's errors of `kinds` as said, each once (the same throw
    /// again, as a reload makes it, is counted), at most `LISTED_MAX`.
    fn errors(&self, kinds: &[PageErrorKind]) -> Vec<String> {
        let said = |kind: PageErrorKind| match kind {
            PageErrorKind::Exception => "the page threw",
            PageErrorKind::Console => "the page logged an error",
            PageErrorKind::Network => "a load failed",
            PageErrorKind::Security => "the browser refused",
        };
        // each: what it said, where it was first, and how many times
        let mut listed: Vec<(String, String, usize)> = vec![];
        for e in self.heard.errors.iter().filter(|e| kinds.contains(&e.kind)) {
            let text = format!("{}: {}", said(e.kind), e.text.lines().next().unwrap_or(""));
            match listed.iter_mut().find(|(t, _, _)| *t == text) {
                Some((_, _, n)) => *n += 1,
                None => listed.push((text, e.source.as_deref().map(|s| format!(" (at {s})")).unwrap_or_default(), 1)),
            }
        }
        listed.into_iter().take(LISTED_MAX).map(|(t, at, n)| if n > 1 { format!("{t}{at} ({n} times)") } else { format!("{t}{at}") }).collect()
    }

    /// What went wrong, most telling first: what it threw, its document,
    /// its socket, its queries, its calls, then what it logged and the
    /// loads that failed (often their echoes); or that nothing did.
    fn why(&self, kinds: &BTreeMap<String, String>) -> String {
        let mut why: Vec<String> = self.errors(&[PageErrorKind::Exception]);
        match self.document() {
            None => why.push("no document answered".into()),
            Some((at, status)) if status != 200 => why.push(format!("its document answered {status} at {at}")),
            Some(_) => {}
        }
        match self.sockets.last() {
            None => why.push("it opened no socket to its fragment's __live: its script never asked for live (did it run?)".into()),
            Some(l) if l.status.is_some_and(|s| s != 101) => why.push(format!("its socket to __live was refused: {}", l.status.unwrap_or(0))),
            Some(l) if l.status.is_none() && l.error.is_none() => why.push("its socket to __live never got an answer".into()),
            Some(l) if l.error.is_some() => why.push(format!("its socket to __live failed: {}", l.error.as_deref().unwrap_or(""))),
            Some(l) if l.hello.is_none() => why.push("its socket to __live opened, and the fragment never said hello".into()),
            Some(l) if l.closed => why.push(format!("its socket to __live closed ({} sockets in all)", self.sockets.len())),
            Some(_) => {}
        }
        for l in &self.sockets {
            why.extend(l.said.iter().take(LISTED_MAX).map(|m| format!("the fragment said: {m}")));
        }
        for q in self.queries(kinds).iter().filter(|q| q["answer"] != "result").take(LISTED_MAX) {
            why.push(format!("its query {} ({}) answered {}", q["op"].as_str().unwrap_or(""), q["via"].as_str().unwrap_or(""), q["answer"].as_str().unwrap_or("")));
        }
        for c in self.calls.iter().filter(|c| kinds.get(&c.op).is_none_or(|k| k != "query")).filter(|c| c.status != Some(200)).take(LISTED_MAX) {
            match (c.status, &c.failed) {
                (Some(st), _) => why.push(format!("its call {} answered {st}", c.op)),
                (None, Some(f)) => why.push(format!("its call {} failed: {f}", c.op)),
                (None, None) => why.push(format!("its call {} has no answer yet", c.op)),
            }
        }
        why.extend(self.errors(&[PageErrorKind::Console, PageErrorKind::Network, PageErrorKind::Security]));
        if self.heard.dropped > 0 {
            why.push(format!("and {} more errors", self.heard.dropped));
        }
        why.extend(self.unread());
        match why.is_empty() {
            true => "nothing went wrong that it said".into(),
            false => why.join("; "),
        }
    }

    fn summary(&self, kinds: &BTreeMap<String, String>) -> Value {
        json!({
            "why": self.why(kinds),
            "errors": self.heard.errors.iter().map(|e| json!({ "kind": format!("{:?}", e.kind), "text": e.text, "source": e.source })).collect::<Vec<_>>(),
            "documents": self.documents,
            "sockets": self.sockets.iter().map(|l| json!({ "url": l.url, "status": l.status, "hello": l.hello.is_some(), "closed": l.closed, "error": l.error, "said": l.said })).collect::<Vec<_>>(),
            "queries": self.queries(kinds),
            "calls": self.calls.iter().map(|c| json!({ "op": c.op, "status": c.status, "failed": c.failed })).collect::<Vec<_>>(),
            "presence": self.presence(),
            "loads": self.loads.iter().map(|(_, at)| at).collect::<Vec<_>>(),
            "dropped": self.dropped,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::Watched;

    const ORIGIN: &str = "http://groceries--ann.fragment.localhost:8790";

    fn event(method: &str, params: Value) -> Value {
        json!({ "method": method, "sessionId": "s", "params": params })
    }

    fn document(request: &str, url: &str, status: u16) -> Value {
        event("Network.responseReceived", json!({ "requestId": request, "type": "Document", "response": { "url": url, "status": status } }))
    }

    fn sent(request: &str, frame: Value) -> Value {
        event("Network.webSocketFrameSent", json!({ "requestId": request, "response": { "opcode": 1, "payloadData": frame.to_string() } }))
    }

    fn received(request: &str, frame: Value) -> Value {
        event("Network.webSocketFrameReceived", json!({ "requestId": request, "response": { "opcode": 1, "payloadData": frame.to_string() } }))
    }

    /// A page that opened its socket and was said hello, as a healthy one is.
    fn opened(request: &str, id: &str, present: usize) -> Vec<Value> {
        vec![
            event("Network.webSocketCreated", json!({ "requestId": request, "url": "ws://groceries--ann.fragment.localhost:8790/__live" })),
            event("Network.webSocketHandshakeResponseReceived", json!({ "requestId": request, "response": { "status": 101 } })),
            received(request, json!({ "type": "hello", "id": id, "principal": "npub1ann", "presence": vec![json!({ "id": "x" }); present] })),
        ]
    }

    fn kinds() -> BTreeMap<String, String> {
        [("list", "query"), ("add", "mutation")].into_iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    /// Goal: Paul's grocery list, read as it was. Method: its page's events
    /// as Chrome sent them: its document through the sign-in's redirects,
    /// then the throw that stopped its script before it asked for live.
    #[test]
    fn a_page_that_threw_says_so_first() {
        let events = vec![
            event("Network.requestWillBeSent", json!({ "requestId": "d", "type": "Document", "request": { "url": format!("{ORIGIN}/__signin?token=secret"), "method": "GET" },
                "redirectResponse": { "url": "http://127.0.0.1:8790/auth/fragment?name=groceries--ann&return=/", "status": 302 } })),
            document("d", &format!("{ORIGIN}/"), 200),
            event("Runtime.exceptionThrown", json!({ "exceptionDetails": { "text": "Uncaught", "url": format!("{ORIGIN}/"), "lineNumber": 41, "columnNumber": 20,
                "exception": { "type": "object", "subtype": "error", "description": "TypeError: Cannot set properties of null (setting 'onsubmit')\n    at http://a/:42:21" } } })),
        ];
        let seen = Seen::read(&events, 0, ORIGIN);
        assert!(!seen.quiet() && !seen.live());
        assert_eq!(seen.document(), Some((format!("{ORIGIN}/").as_str(), 200)));
        assert!(seen.documents.iter().all(|(at, _)| !at.contains("secret")), "a sign-in's token is never evidence: {:?}", seen.documents);
        let why = seen.why(&kinds());
        assert!(why.starts_with("the page threw: TypeError: Cannot set properties of null (setting 'onsubmit') (at http://groceries--ann.fragment.localhost:8790/:42:21)"), "{why}");
        assert!(why.contains("it opened no socket to its fragment's __live"), "{why}");
        // the form, with no handler, submits itself: the page loads again at `/?` and throws again
        let mut again = events.clone();
        again.push(event("Runtime.exceptionThrown", json!({ "exceptionDetails": { "text": "Uncaught", "url": format!("{ORIGIN}/?"), "lineNumber": 41, "columnNumber": 20,
            "exception": { "type": "object", "subtype": "error", "description": "TypeError: Cannot set properties of null (setting 'onsubmit')" } } })));
        let why = Seen::read(&again, 0, ORIGIN).why(&kinds());
        assert!(why.starts_with("the page threw: TypeError: Cannot set properties of null (setting 'onsubmit') (at http://groceries--ann.fragment.localhost:8790/:42:21) (2 times)"), "{why}");
        assert_eq!(why.matches("the page threw").count(), 1, "{why}");
    }

    /// Goal: a page that works reads as one: live up, its query answered,
    /// the other tab's presence heard, nothing said wrong.
    #[test]
    fn a_working_page_reads_as_one() {
        let mut events = vec![document("d", &format!("{ORIGIN}/"), 200)];
        events.extend(opened("w1", "me", 1));
        events.extend([
            sent("w1", json!({ "type": "query", "id": "live-1", "op": "list", "input": {} })),
            received("w1", json!({ "type": "result", "id": "live-1", "result": { "items": [] } })),
            sent("w1", json!({ "type": "presence", "data": { "joined": 1 } })),
            // its own presence, echoed, is not another's
            received("w1", json!({ "type": "presence", "id": "me", "principal": "npub1ann", "data": { "joined": 1 } })),
            received("w1", json!({ "type": "changed" })),
            // a live query's next run shares its id
            sent("w1", json!({ "type": "query", "id": "live-1", "op": "list", "input": {} })),
            received("w1", json!({ "type": "result", "id": "live-1", "result": { "items": ["oat milk"] } })),
            event("Runtime.consoleAPICalled", json!({ "type": "log", "args": [{ "type": "string", "value": "hi" }] })),
        ]);
        let seen = Seen::read(&events, 0, ORIGIN);
        assert!(seen.quiet() && seen.live(), "{}", seen.why(&kinds()));
        assert_eq!(seen.why(&kinds()), "nothing went wrong that it said");
        assert_eq!((seen.asked.len(), seen.pending(), seen.presence_sent, seen.others_heard, seen.present_at_hello()), (2, 0, 1, 0, 1));
        assert!(seen.queries(&kinds()).iter().all(|q| q["answer"] == "result"));
        let mut more = events.clone();
        more.push(received("w1", json!({ "type": "presence", "id": "other", "principal": "npub1ann", "data": { "joined": 2 } })));
        more.push(received("w1", json!({ "type": "presence", "id": "gone", "principal": "npub1ann", "data": null })));
        assert_eq!(Seen::read(&more, 0, ORIGIN).others_heard, 1, "another's arrival is heard; a leaving is not one");
    }

    /// Goal: each way the live connection fails is said as itself.
    #[test]
    fn a_socket_that_failed_says_how() {
        let created = event("Network.webSocketCreated", json!({ "requestId": "w", "url": "ws://groceries--ann.fragment.localhost:8790/__live" }));
        let refused = Seen::read(&[created.clone(), event("Network.webSocketHandshakeResponseReceived", json!({ "requestId": "w", "response": { "status": 403 } }))], 0, ORIGIN);
        assert!(!refused.live() && refused.why(&kinds()).contains("its socket to __live was refused: 403"));
        let silent = Seen::read(&[created.clone(), event("Network.webSocketHandshakeResponseReceived", json!({ "requestId": "w", "response": { "status": 101 } }))], 0, ORIGIN);
        assert!(!silent.live() && silent.why(&kinds()).contains("never said hello"));
        let unanswered = Seen::read(std::slice::from_ref(&created), 0, ORIGIN);
        assert!(unanswered.why(&kinds()).contains("never got an answer"));
        let mut closed = opened("w", "me", 0);
        closed.push(event("Network.webSocketClosed", json!({ "requestId": "w" })));
        assert!(!Seen::read(&closed, 0, ORIGIN).live());
        // a socket to another origin, or another path, is not its live connection
        let elsewhere = Seen::read(&[event("Network.webSocketCreated", json!({ "requestId": "x", "url": "ws://other--ann.fragment.localhost:8790/__live" }))], 0, ORIGIN);
        assert!(elsewhere.sockets.is_empty());
        assert!(is_live("wss://a--b--p5.finite.place/__live", "https://a--b--p5.finite.place") && !is_live("wss://a--b--p5.finite.place/__live/x", "https://a--b--p5.finite.place"));
        let mut said = opened("w", "me", 0);
        said.push(received("w", json!({ "type": "error", "message": "unknown channel groceries" })));
        assert!(Seen::read(&said, 0, ORIGIN).why(&kinds()).contains("the fragment said: unknown channel groceries"));
    }

    /// Goal: the page's own query is every run it made, over its socket or
    /// HTTP (an operation its manifest calls a query), and one that
    /// answered an error, or not at all, says so; a mutation is no query.
    #[test]
    fn its_queries_and_calls_are_read_with_their_answers() {
        let mut events = opened("w", "me", 0);
        events.extend([
            sent("w", json!({ "type": "query", "id": "live-1", "op": "list", "input": {} })),
            received("w", json!({ "type": "result", "id": "live-1", "error": "app_error", "status": 500, "message": "no such table: items" })),
            sent("w", json!({ "type": "query", "id": "live-2", "op": "count", "input": {} })),
            event("Network.requestWillBeSent", json!({ "requestId": "h1", "type": "Fetch", "request": { "url": format!("{ORIGIN}/__op/list"), "method": "POST" } })),
            event("Network.responseReceived", json!({ "requestId": "h1", "type": "Fetch", "response": { "url": format!("{ORIGIN}/__op/list"), "status": 200 } })),
            event("Network.requestWillBeSent", json!({ "requestId": "h2", "type": "Fetch", "request": { "url": format!("{ORIGIN}/__op/add"), "method": "POST" } })),
            event("Network.responseReceived", json!({ "requestId": "h2", "type": "Fetch", "response": { "url": format!("{ORIGIN}/__op/add"), "status": 400 } })),
            event("Network.requestWillBeSent", json!({ "requestId": "h3", "type": "Fetch", "request": { "url": format!("{ORIGIN}/__op/add"), "method": "POST" } })),
            event("Network.loadingFailed", json!({ "requestId": "h3", "type": "Fetch", "errorText": "net::ERR_CONNECTION_RESET" })),
        ]);
        let seen = Seen::read(&events, 0, ORIGIN);
        let queries = seen.queries(&kinds());
        assert_eq!(queries.iter().map(|q| (q["op"].as_str().unwrap(), q["via"].as_str().unwrap(), q["answer"].clone())).collect::<Vec<_>>(), [
            ("list", "socket", json!("500 app_error: no such table: items")),
            ("count", "socket", json!("no answer")),
            ("list", "http", json!("result")),
        ]);
        assert_eq!(seen.pending(), 1, "the count asked, unanswered");
        let why = seen.why(&kinds());
        assert!(why.contains("its query list (socket) answered 500 app_error: no such table: items") && why.contains("its query count (socket) answered no answer"), "{why}");
        // a query's own error comes before its echo, the load the browser says failed
        let mut echoed = events.clone();
        echoed.push(event("Log.entryAdded", json!({ "entry": { "source": "network", "level": "error", "text": "Failed to load resource: the server responded with a status of 422 (Unprocessable Entity)", "url": format!("{ORIGIN}/__op/list") } })));
        let why = Seen::read(&echoed, 0, ORIGIN).why(&kinds());
        assert!(why.find("its query list").unwrap() < why.find("a load failed").unwrap(), "{why}");
        assert!(why.contains("its call add answered 400") && why.contains("its call add failed: net::ERR_CONNECTION_RESET"), "{why}");
        // a page that asked nothing has no query, and says nothing of one
        assert!(Seen::read(&opened("w", "me", 0), 0, ORIGIN).queries(&kinds()).is_empty());
    }

    /// Goal: a page sent to a refusal (another's app, a session the
    /// fragment's origin lost) is not a page that loaded; one past what is
    /// kept says so.
    #[test]
    fn a_refused_document_and_a_flood_are_said() {
        let seen = Seen::read(&[document("d", &format!("{ORIGIN}/"), 403)], 0, ORIGIN);
        assert!(seen.why(&kinds()).contains("its document answered 403"));
        let none = Seen::read(&[], 0, ORIGIN);
        assert!(none.why(&kinds()).contains("no document answered") && none.document().is_none());
        let flood = Seen::read(&opened("w", "me", 0), 7, ORIGIN);
        assert!(!flood.quiet() && flood.why(&kinds()).contains("7 of its events came past"), "{}", flood.why(&kinds()));
    }

    /// A main-frame document requested: its frame `F`, its loader its request.
    fn navigated(request: &str, url: &str) -> Value {
        event("Network.requestWillBeSent", json!({ "requestId": request, "loaderId": request, "frameId": "F", "type": "Document", "request": { "url": url, "method": "GET" } }))
    }

    const ITEM: &str = "oat milk 1a2b";

    /// The first tab, loaded and live, its list run once (empty): a page
    /// as `app_works` stamps it before the item is added.
    fn loaded() -> Vec<Value> {
        let mut events = vec![navigated("d", &format!("{ORIGIN}/")), document("d", &format!("{ORIGIN}/"), 200)];
        events.extend(opened("w", "me", 0));
        events.extend([
            sent("w", json!({ "type": "query", "id": "live-1", "op": "list", "input": {} })),
            received("w", json!({ "type": "result", "id": "live-1", "result": { "todos": [] } })),
            sent("w", json!({ "type": "subscribe", "channel": "activity", "last": 20 })),
        ]);
        events
    }

    /// The item added: its line of activity, and the fragment's word that
    /// its state changed.
    fn added() -> Vec<Value> {
        vec![
            received("w", json!({ "type": "record", "channel": "activity", "seq": 1, "body": { "did": "added", "text": ITEM } })),
            received("w", json!({ "type": "changed" })),
        ]
    }

    /// Goal (Alex's review of #547): the item is in the list its query
    /// backs, not anywhere in the page. Method: a page whose activity
    /// arrives, and whose list is never run again, fails, though a line of
    /// activity names the item; one whose query answers with it, over its
    /// socket or HTTP, passes.
    #[test]
    fn the_item_is_in_the_list_its_query_answers() {
        let mut stale = loaded();
        stale.extend(added());
        let seen = Seen::read(&stale, 0, ORIGIN);
        let why = listed(&seen, true, &[], ITEM, seen.loads()).unwrap_err();
        assert!(why.contains("no run of its own query answered with “oat milk 1a2b”"), "{why}");
        // its query run again, answered with it (in another case: what the app stored)
        let mut fresh = stale.clone();
        fresh.extend([
            sent("w", json!({ "type": "query", "id": "live-1", "op": "list", "input": {} })),
            received("w", json!({ "type": "result", "id": "live-1", "result": { "todos": [{ "id": 1, "text": "Oat milk 1a2b", "done": false }] } })),
        ]);
        let seen = Seen::read(&fresh, 0, ORIGIN);
        assert_eq!(listed(&seen, true, &[], ITEM, 1), Ok(()));
        // and a list that does not show it, though its query answered with it
        assert!(listed(&seen, false, &[], ITEM, 1).unwrap_err().contains("its list does not show “oat milk 1a2b”"));
        // over HTTP: a run's answer, its body
        let seen = Seen::read(&stale, 0, ORIGIN);
        assert_eq!(listed(&seen, true, &[r#"{"todos":[{"text":"oat milk 1a2b"}]}"#.into()], ITEM, 1), Ok(()));
        assert!(listed(&seen, true, &[r#"{"todos":[]}"#.into()], ITEM, 1).is_err());
    }

    /// Goal (Alex's review of #547): a page that reloads to show the item
    /// is no live update. Method: its main frame loads again after the
    /// stamp and its new document's query answers with the item: it fails,
    /// saying it reloaded; the sign-in's redirects are one load, and a
    /// frame inside the page is not the page.
    #[test]
    fn a_reload_is_no_live_update() {
        let mut events = vec![
            event("Network.requestWillBeSent", json!({ "requestId": "d", "loaderId": "d", "frameId": "F", "type": "Document", "request": { "url": format!("{ORIGIN}/") } })),
            event("Network.requestWillBeSent", json!({ "requestId": "d", "loaderId": "d", "frameId": "F", "type": "Document", "request": { "url": format!("{ORIGIN}/__signin?token=secret") },
                "redirectResponse": { "url": "http://127.0.0.1:8790/auth/fragment?name=groceries--ann&return=/", "status": 302 } })),
        ];
        events.extend(loaded().into_iter().skip(1));
        events.push(event("Network.requestWillBeSent", json!({ "requestId": "i", "loaderId": "i", "frameId": "G", "type": "Document", "request": { "url": "https://example.com/" } })));
        let seen = Seen::read(&events, 0, ORIGIN);
        assert_eq!(seen.loads(), 1, "{:?}", seen.loads);
        let stamp = seen.loads();
        events.extend(added());
        events.push(navigated("d2", &format!("{ORIGIN}/?")));
        events.push(document("d2", &format!("{ORIGIN}/"), 200));
        events.extend(opened("w2", "me2", 0));
        events.extend([
            sent("w2", json!({ "type": "query", "id": "live-1", "op": "list", "input": {} })),
            received("w2", json!({ "type": "result", "id": "live-1", "result": { "todos": [{ "text": ITEM }] } })),
        ]);
        let seen = Seen::read(&events, 0, ORIGIN);
        let why = listed(&seen, true, &[], ITEM, stamp).unwrap_err();
        assert!(why.starts_with(&format!("the page loaded again while the item was added (at {ORIGIN}/)")), "{why}");
        assert!(listed(&seen, true, &[], ITEM, seen.loads()).is_ok(), "stamped after the reload, it is the page");
    }

    /// Goal (Alex's review of #547): a page past what a watched page keeps
    /// passes nothing on what came after. Method: a healthy page's events,
    /// to the limit, then its query failing and a throw, past it: each
    /// check that reads the whole visit fails, saying how many were unread.
    #[test]
    fn a_page_past_what_is_kept_is_unread() {
        let mut w = Watched::default();
        let mut healthy = loaded();
        healthy.extend(added());
        healthy.extend([
            sent("w", json!({ "type": "query", "id": "live-1", "op": "list", "input": {} })),
            received("w", json!({ "type": "result", "id": "live-1", "result": { "todos": [{ "text": ITEM }] } })),
        ]);
        let log = event("Runtime.consoleAPICalled", json!({ "type": "log", "args": [{ "type": "string", "value": "tick" }] }));
        let pad = WATCHED_EVENTS_MAX - healthy.len();
        for e in healthy.into_iter().chain(std::iter::repeat_n(log, pad)) {
            w.keep(e);
        }
        let prefix = Seen::read(&w.events, w.dropped, ORIGIN);
        assert!(prefix.quiet() && all_answered(&[("first", &prefix)], &kinds()).is_ok() && listed(&prefix, true, &[], ITEM, 1).is_ok(), "{}", prefix.why(&kinds()));
        w.keep(sent("w", json!({ "type": "query", "id": "live-1", "op": "list", "input": {} })));
        w.keep(received("w", json!({ "type": "result", "id": "live-1", "error": "app_error", "status": 500, "message": "no such table: todos" })));
        w.keep(event("Runtime.exceptionThrown", json!({ "exceptionDetails": { "text": "Uncaught", "exception": { "description": "TypeError: x is undefined" } } })));
        assert_eq!((w.events.len(), w.dropped), (WATCHED_EVENTS_MAX, 3));
        let seen = Seen::read(&w.events, w.dropped, ORIGIN);
        let unread = format!("3 of its events came past the {WATCHED_EVENTS_MAX} a watched page keeps, so what it did after them is unread");
        assert!(!seen.quiet() && seen.why(&kinds()).contains(&unread), "{}", seen.why(&kinds()));
        assert_eq!(all_answered(&[("first", &seen)], &kinds()), Err(format!("the first tab: {unread}")));
        assert_eq!(listed(&seen, true, &[], ITEM, 1), Err(unread));
    }
}
