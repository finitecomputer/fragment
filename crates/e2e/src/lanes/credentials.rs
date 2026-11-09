//! The computer's swap (Paul, 2026-10-04; docs/computers.md, "Connections
//! and operator keys"), as the computers lane drives it on the stub: its
//! scripted agent reads what its guest is given (`credentials`) and sends
//! requests as any SDK would (`fetch … with $NAME`), with no header of
//! ours, to the catalog's real hosts, which the swap sends to the upstream
//! fake. Each placement (a header, a query parameter, basic auth), each
//! kind (a connection through the WorkOS fake, the operator's keys, an own
//! key), and each refusal (a forged tag, another computer's, the wrong
//! host, the wrong place), then the charges and the counts, and a key's
//! call its owner's ledger's gate refuses.

use std::time::Duration;

use anyhow::Result;
use base64::Engine;
use serde_json::{json, Value};

use super::ledger::{charged, entries, forget_gate, usage};
use crate::api::Api;
use crate::{Suite, SEVERAL, SEVERAL_HOST, SWAP_CONNECTION, SWAP_CONNECTION_ENV, SWAP_CONNECTION_HOST, SWAP_KEYS, SWAP_OWN, SWAP_OWN_ENV, SWAP_OWN_HOST, SWAP_ROUTER, SWAP_ROUTER_ENV, SWAP_ROUTER_HOST};

/// Who the swap's checks act as, and on what.
pub(super) struct Swapping<'a> {
    pub owner: &'a crate::Keys,
    pub stranger: &'a crate::Keys,
    pub id: &'a str,
    pub agent: &'a str,
    pub identity: &'a str,
    /// The month the computer's uses are counted in (`computers::pin_month`).
    pub month: &'a str,
}

/// The checks `swap_checks` makes, as a run without the stub or the fakes
/// skips them.
pub(super) const SWAP_CHECKS: &[&str] = &[
    "the guest is given each operator key the deployment offers, each in its environment variable, a placeholder naming the agent",
    "and no connection its owner has not connected: its environment variable holds nothing",
    "a placeholder made for that connection is refused, saying so, and reaches no provider",
    "connected, its owner's read of their connections says so, and its guest is given it at its next read, no token minted to tell",
    "a header: the connection's placeholder from the env, sent as a bearer token, reaches its host as the owner's token from Pipes",
    "and never a header of ours",
    "each request asks Pipes for its token, so a connection that needs authorizing again stops at the next; a provider's redirect is the guest's to follow, never followed with it",
    "a header: an operator key's placeholder as a bearer token reaches its host as the key",
    "a header of the provider's own (xi-api-key) takes its key",
    "a query parameter: the key in place of the placeholder, the rest of the query as it came",
    "a placeholder in a place its provider does not take its key is refused, saying where it goes",
    "a placeholder sent to a host that is not its provider's is refused, and reaches no provider",
    "a forged tag names no agent: refused, reaching no provider",
    "another computer's agent's tag is refused here",
    "an own key: its owner gives it, sealed by the computer, and it is the guest's at once",
    "basic auth: the own key in its password's place, its user as it came",
    "a key that is no printable token is refused; taken away, the guest is given it no more",
    "each operator key's call is charged to the agent's owner's ledger after it, as the agent, at its list price and the margin",
    "this month's uses: each call by agent, a connection's and an own key's counted, never charged, an operator key's at its charge",
    "no one else reads them, and a month is YYYY-MM",
    "a request with no placeholder goes on as it came",
    "an operator key's call its owner's ledger's gate refuses is refused, and reaches no provider",
    "firecrawl: its SDK header is swapped only at its catalog host",
    "fal: its SDK header is swapped only at its catalog host",
    "browser-use: its SDK header is swapped only at its catalog host",
    "xai: its SDK header is swapped only at its catalog host",
    "x: its SDK header is swapped only at its catalog host",
    "disconnected, an account Pipes holds again with no connect known to have imported it (an import landed late) is not its owner's: read as not connected, its swaps refused, no token asked",
    "of several accounts at a connection, an agent that picked none is swapped its owner's default, the oldest connected: Pipes' token for that account",
    "its owner picks another account for it: its next request is swapped that account's token",
    "the account it picked disconnected, it falls back to the default, its pick kept until it is next written",
    "a key's call the book prices by what it says (ElevenLabs' text to speech) reaches its host as it came, body and all, and is charged its text's characters, not a call",
    "and one its provider refused (a bad request, a rate limit) made nothing: its row and the agent's count charge nothing, whatever its text",
    "fal: a synchronous Klein run reaches its host as it came and is charged the pixels it asks for; a queued submit, made later or not, and a poll nothing",
    "xai: a call with the x_search tool is charged an X search, and a plain chat call (its tokens not read) and any other call (a model list's) nothing",
    "google-places: a place's photo is charged its own price, and a search and a place's details (their tier their field mask's) nothing",
    "firecrawl: a scrape is charged its one credit, not a search's two, and a call around it (a status poll) nothing",
    "browser-use: a new session's start is charged its five minutes, and a task on that session and a call around it (its status read) nothing",
    "x: a search is charged ten posts, as before, and a lookup (its returned resources not counted) nothing",
    "a shared key charged its owner's hourly cap pauses: its next call is refused 429 rate_limited (reason key_paused), saying what it spent and when it resumes, reaching no provider and charged nothing",
    "while it is paused, another key's call goes on: the key pauses, not the account",
    "its owner sees it over the agent's chat: the key, and what it spent this hour",
    "as the hour passes, its oldest charges go: the key's calls go again, and its notice with them",
    "a key's cap is its person's to set: an agent's key is refused (403), as are no signature (401), a provider that is no shared key (404) and a cap out of its bounds (400); the key stays paused",
    "its owner taps Allow: unconfirmed (its ask refused, the computer's read failing), the pause stays with its button and a short line; confirmed, the computer lifts the cap a step, the notice goes, and the key's next call goes through",
    "a stale page's tap changes nothing, the computer lifting from the cap it holds; its person's CLI sets a cap and then the default, and the key pauses at each",
];

/// What one call of each key is charged: its list price (fragment_core::
/// price DEFAULT_KEYS) and the 50% margin.
fn charge_of(key: &str) -> i64 {
    charge_for(key, 1)
}

/// What `units` of a key's price are charged, its margin with them.
fn charge_for(key: &str, units: i64) -> i64 {
    let (micros, per) = fragment_core::price::default_key_price(key).expect("each operator key has a list price");
    // ceil(units × micros / per × 1.5), as the price book rounds a row
    let (n, d) = (units * micros * 3, 2 * per as i64);
    (n + d - 1) / d
}

/// A speech call's body, as Hermes' `text_to_speech` posts it: its text,
/// 20 characters, words no rule of the scripted agent's says.
const SPOKEN: &str = r#"{"text":"Hello there, friend.","model_id":"eleven_multilingual_v2"}"#;
const SPOKEN_CHARS: i64 = 20;
/// A long speech call's characters: a few reach a key's hourly cap.
const LONG_CHARS: i64 = 10_000;

/// A fal image's request, as Hermes' `image_generate` posts it to the
/// queue, and the pixels it asks for: one `landscape_16_9` image,
/// 1024 × 576.
const FAL_ASK: &str = r#"{"prompt":"a lighthouse at dusk","image_size":"landscape_16_9"}"#;
const FAL_PIXELS: i64 = 1024 * 576;

/// A Firecrawl scrape, as Hermes' `web_extract` posts it: one credit.
const FIRECRAWL_SCRAPE: &str = r#"{"url":"https://example.com/","formats":["markdown","html"]}"#;

/// An X search, as Hermes' `x_search` posts it, and a plain Grok chat
/// call: no spaces, so no word of theirs is a rule of the scripted agent's.
const XAI_SEARCH: &str = r#"{"model":"grok-4.3","input":"news","tools":[{"type":"x_search"}]}"#;
const XAI_CHAT: &str = r#"{"model":"grok-4.3","messages":[{"role":"user","content":"hi"}]}"#;

/// The placeholder the platform gives `agent` on computer `id` for
/// `provider`: a local run holds the node's host secret, so it makes one
/// as the platform does (a forgery needs the secret; a guest has none).
pub(super) fn placeholder(s: &Suite, id: &str, agent: &str, provider: &str) -> String {
    let kind = if [SWAP_CONNECTION, SEVERAL].contains(&provider) { fragment_core::catalog::Kind::Connection } else { fragment_core::catalog::Kind::Operator };
    let tag = fragment_core::swap::TagKey::derive(&s.host_secret).tag(id, agent, provider);
    fragment_core::swap::Placeholder::new(kind, provider, &tag).text()
}

/// The upstream fake's last request, after `before` of them.
fn last_seen(s: &Suite, before: usize) -> Value {
    s.upstream.seen().get(before..).and_then(|l| l.last().cloned()).unwrap_or(Value::Null)
}

/// The checks; answers how many turns of the agent's they took (each a
/// reply in the chat).
pub(super) fn swap_checks(s: &mut Suite, api: &Api, w: &Swapping, fetched: &dyn Fn(&Suite, u32, &str) -> Result<String>) -> Result<usize> {
    let mut turns = 0;
    let mut say = |s: &Suite, n: u32, text: &str| -> Result<String> {
        turns += 1;
        fetched(s, n, text)
    };
    let owner_id = api.identity(w.owner)?;
    let key_value = |name: &str| SWAP_KEYS.iter().find(|(n, _, _)| *n == name).map(|(_, v, _)| v.to_string()).unwrap_or_default();

    // what the guest is given, before its owner connects anything
    let given = say(s, 101, "credentials")?;
    let every_key = SWAP_KEYS.iter().all(|(name, _, env)| given.contains(&format!("{name} {env} {}", placeholder(s, w.id, w.agent, name))));
    s.ok(SWAP_CHECKS[0], given.starts_with("credentials: ") && every_key, &given);
    s.ok(SWAP_CHECKS[1], !given.contains(SWAP_CONNECTION_ENV) && !given.contains(SWAP_OWN_ENV), &given);
    let seen_before = s.upstream.seen().len();
    let said = say(s, 102, &format!("fetch http://{SWAP_CONNECTION_HOST}/drive/v3/files with ${SWAP_CONNECTION_ENV}"))?;
    let made = say(s, 103, &format!("fetch http://{SWAP_CONNECTION_HOST}/drive/v3/files with {}", placeholder(s, w.id, w.agent, SWAP_CONNECTION)))?;
    s.ok(
        SWAP_CHECKS[2],
        said.contains("fetch failed: no credential") && made.starts_with("fetched 403") && made.contains("not_connected") && made.contains("connect google first") && s.upstream.seen().len() == seen_before,
        json!({ "env": said, "made": made }),
    );

    // connected through Pipes: the owner's read tells the computer at once
    let email = Api::email_of(w.owner);
    s.workos.connect(&email, SWAP_CONNECTION, true);
    let minted_before = s.workos.tokens(SWAP_CONNECTION).len();
    let read_before = s.workos.states_read();
    let r = api.signed(w.owner, "GET", "/api/connections", None)?;
    let state_of = |r: &crate::api::Reply, p: &str| r.body["providers"].as_array().and_then(|l| l.iter().find(|x| x["provider"] == p)).map(|x| x["state"].clone()).unwrap_or(Value::Null);
    let given = say(s, 104, "credentials")?;
    s.ok(
        SWAP_CHECKS[3],
        state_of(&r, SWAP_CONNECTION) == "connected"
            && given.contains(&format!("{SWAP_CONNECTION} {SWAP_CONNECTION_ENV} {}", placeholder(s, w.id, w.agent, SWAP_CONNECTION)))
            && s.workos.tokens(SWAP_CONNECTION).len() == minted_before
            && s.workos.states_read() > read_before,
        json!({ "connections": r.body, "given": given }),
    );

    // a header: the connection's bearer token
    let said = say(s, 105, &format!("fetch http://{SWAP_CONNECTION_HOST}/drive/v3/files with ${SWAP_CONNECTION_ENV}"))?;
    let seen = s.upstream.seen().last().cloned().unwrap_or_default();
    let tokens = s.workos.tokens(SWAP_CONNECTION).split_off(minted_before);
    s.ok(
        SWAP_CHECKS[4],
        said.starts_with("fetched 200") && tokens.len() == 1 && seen["host"] == SWAP_CONNECTION_HOST && seen["auth"]["authorization"] == format!("Bearer {}", tokens[0]),
        json!({ "said": said, "seen": seen, "tokens": tokens }),
    );
    s.ok(SWAP_CHECKS[5], seen["agent"].is_null(), &seen);
    let said = say(s, 106, &format!("fetch http://{SWAP_CONNECTION_HOST}/redirect with ${SWAP_CONNECTION_ENV}"))?;
    let asked = s.workos.tokens(SWAP_CONNECTION).len() == minted_before + 2;
    // its account needs authorizing again: the next request is refused, no token held
    s.workos.connect(&email, SWAP_CONNECTION, false);
    let seen_before = s.upstream.seen().len();
    let stopped = say(s, 119, &format!("fetch http://{SWAP_CONNECTION_HOST}/drive/v3/files with {}", placeholder(s, w.id, w.agent, SWAP_CONNECTION)))?;
    s.workos.connect(&email, SWAP_CONNECTION, true);
    let again = api.signed(w.owner, "GET", "/api/connections", None)?;
    s.ok(
        SWAP_CHECKS[6],
        said.starts_with("fetched 302")
            && asked
            && stopped.starts_with("fetched 403")
            && stopped.contains("connect google again")
            && s.upstream.seen().len() == seen_before
            && state_of(&again, SWAP_CONNECTION) == "connected",
        json!({ "redirect": said, "stopped": stopped }),
    );

    // disconnected, then an account at Pipes again with no connect known
    // to have imported it (an import whose outcome was unknown, landing
    // late: the fake's lever): its swaps stay refused, no token asked
    let off = api.signed(w.owner, "DELETE", &format!("/api/connections/{SWAP_CONNECTION}"), None)?;
    s.workos.connect(&email, SWAP_CONNECTION, true);
    let (minted, seen_before) = (s.workos.tokens(SWAP_CONNECTION).len(), s.upstream.seen().len());
    let fenced = api.signed(w.owner, "GET", "/api/connections", None)?;
    let refused = say(s, 120, &format!("fetch http://{SWAP_CONNECTION_HOST}/drive/v3/files with {}", placeholder(s, w.id, w.agent, SWAP_CONNECTION)))?;
    s.ok(
        SWAP_CHECKS[27],
        off.status == 200
            && state_of(&fenced, SWAP_CONNECTION) == "not_connected"
            && refused.starts_with("fetched 403")
            && refused.contains("connect google first")
            && s.workos.tokens(SWAP_CONNECTION).len() == minted
            && s.upstream.seen().len() == seen_before,
        json!({ "disconnect": off.body, "connections": fenced.body, "refused": refused }),
    );
    // connected again for the checks after: a connect known to import lifts
    // the fence (its row has no `connection_oauth` here: the registry's lever)
    let hook = api.unsigned("POST", "/api/test/registry", Some(&json!({ "signins": { "unfenceConnection": { "email": email, "provider": SWAP_CONNECTION } } })))?;
    anyhow::ensure!(hook.status == 200, "lift the fence: {hook}");
    let again = api.signed(w.owner, "GET", "/api/connections", None)?;
    anyhow::ensure!(state_of(&again, SWAP_CONNECTION) == "connected", "connected again after the fence: {again}");

    // several accounts at one connection: the agent's pick says whose token
    s.workos.connect(&email, SEVERAL, true);
    s.workos.label(&email, SEVERAL, "work@oauth.test");
    let home = s.workos.add_account(&email, SEVERAL, "home", "home@oauth.test");
    let r = api.signed(w.owner, "GET", "/api/connections", None)?;
    let listed: Vec<String> = r.body["providers"].as_array().and_then(|l| l.iter().find(|x| x["provider"] == SEVERAL)).and_then(|x| x["accounts"].as_array()).map(|l| l.iter().filter_map(|a| a["id"].as_str().map(str::to_string)).collect()).unwrap_or_default();
    let work = listed.first().cloned().unwrap_or_default();
    let fetch = |s: &Suite| format!("fetch http://{SEVERAL_HOST}/v1/me with {}", placeholder(s, w.id, w.agent, SEVERAL));
    let whose = |s: &Suite| s.workos.tokens(SEVERAL).last().and_then(|t| s.workos.token_account(t));
    let swapped = |s: &Suite| s.workos.tokens(SEVERAL).last().is_some_and(|t| s.upstream.seen().last().is_some_and(|seen| seen["host"] == SEVERAL_HOST && seen["auth"]["authorization"] == format!("Bearer {t}")));
    let said = say(s, 121, &fetch(s))?;
    s.ok(SWAP_CHECKS[28], listed == [work.clone(), home.clone()] && said.starts_with("fetched 200") && whose(s).as_deref() == Some(work.as_str()) && swapped(s), json!({ "listed": listed, "said": said }));
    let picked = format!("/api/computers/{}/agents/{}/accounts/{SEVERAL}", w.id, w.agent);
    let pick = api.signed(w.owner, "PUT", &picked, Some(&json!({ "account": home })))?;
    let shown = pick.body["agents"].as_array().and_then(|l| l.iter().find(|a| a["fragment"] == w.agent)).map(|a| a["accounts"][SEVERAL].clone()).unwrap_or(Value::Null);
    let said = say(s, 122, &fetch(s))?;
    s.ok(SWAP_CHECKS[29], pick.status == 200 && shown == home.as_str() && said.starts_with("fetched 200") && whose(s).as_deref() == Some(home.as_str()) && swapped(s), json!({ "pick": pick.body, "said": said }));
    let off = api.signed(w.owner, "DELETE", &format!("/api/connections/{SEVERAL}?account={home}"), None)?;
    let said = say(s, 123, &fetch(s))?;
    let view = api.signed(w.owner, "GET", &format!("/api/computers/{}", w.id), None)?;
    let kept = view.body["agents"].as_array().and_then(|l| l.iter().find(|a| a["fragment"] == w.agent)).map(|a| a["accounts"][SEVERAL].clone()).unwrap_or(Value::Null);
    s.ok(SWAP_CHECKS[30], off.status == 200 && said.starts_with("fetched 200") && whose(s).as_deref() == Some(work.as_str()) && swapped(s) && kept == home.as_str(), json!({ "off": off.body, "said": said, "kept": kept }));
    let cleared = api.signed(w.owner, "PUT", &picked, Some(&json!({ "account": null })))?;
    let gone = api.signed(w.owner, "DELETE", &format!("/api/connections/{SEVERAL}"), None)?;
    anyhow::ensure!(cleared.status == 200 && gone.status == 200, "the several accounts' checks cleared: {cleared}; {gone}");

    // the operator's keys: a bearer token, a header of the provider's own, a query parameter
    let said = say(s, 107, "fetch http://api.perplexity.ai/search with $PERPLEXITY_API_KEY")?;
    let seen = s.upstream.seen().last().cloned().unwrap_or_default();
    s.ok(SWAP_CHECKS[7], said.starts_with("fetched 200") && seen["host"] == "api.perplexity.ai" && seen["auth"]["authorization"] == format!("Bearer {}", key_value("perplexity")), json!({ "said": said, "seen": seen }));
    let said = say(s, 108, "fetch http://api.elevenlabs.io/v1/music with $ELEVENLABS_API_KEY in xi-api-key")?;
    let seen = s.upstream.seen().last().cloned().unwrap_or_default();
    s.ok(SWAP_CHECKS[8], said.starts_with("fetched 200") && seen["host"] == "api.elevenlabs.io" && seen["auth"]["xi-api-key"] == key_value("elevenlabs"), json!({ "said": said, "seen": seen }));
    // text to speech on the same key, its body as it came (FIN-443): charged below by its characters
    let said = say(s, 124, &format!("fetch http://api.elevenlabs.io/v1/text-to-speech/e2e-voice with $ELEVENLABS_API_KEY in xi-api-key posting {SPOKEN}"))?;
    let spoken_seen = s.upstream.seen().last().cloned().unwrap_or_default();
    let spoken_sent = said.starts_with("fetched 200")
        && spoken_seen["host"] == "api.elevenlabs.io"
        && spoken_seen["method"] == "POST"
        && spoken_seen["path"] == "/v1/text-to-speech/e2e-voice"
        && spoken_seen["body"] == SPOKEN
        && spoken_seen["auth"]["xi-api-key"] == key_value("elevenlabs");
    // refused by its provider: answered (under 500), so counted, but nothing made
    let mut refused_said = vec![];
    for (n, code) in [(125, 400), (126, 429)] {
        refused_said.push(say(s, n, &format!("fetch http://api.elevenlabs.io/v1/text-to-speech/e2e-voice?e2e_status={code} with $ELEVENLABS_API_KEY in xi-api-key posting {SPOKEN}"))?);
    }
    let refused_sent = refused_said.iter().zip(["fetched 400", "fetched 429"]).all(|(said, code)| said.starts_with(code));
    let said = say(s, 109, "fetch http://places.googleapis.com/v1/places:searchText?fields=name with $GOOGLE_PLACES_API_KEY in query key")?;
    let seen = s.upstream.seen().last().cloned().unwrap_or_default();
    s.ok(
        SWAP_CHECKS[9],
        said.starts_with("fetched 200") && seen["host"] == "places.googleapis.com" && seen["query"] == json!([["fields", "name"], ["key", key_value("google-places")]]),
        json!({ "said": said, "seen": seen }),
    );
    // a place's details (FIN-445): charged below nothing, its tier its field mask's
    let said = say(s, 138, "fetch http://places.googleapis.com/v1/places/ChIJe2e?fields=rating with $GOOGLE_PLACES_API_KEY in query key")?;
    let seen = s.upstream.seen().last().cloned().unwrap_or_default();
    let detail_sent = said.starts_with("fetched 200") && seen["host"] == "places.googleapis.com" && seen["path"] == "/v1/places/ChIJe2e";
    // its photo, answered as JSON (`skipHttpRedirect`; a redirect is charged nothing)
    let said = say(s, 142, "fetch http://places.googleapis.com/v1/places/ChIJe2e/photos/AUe2e/media?maxWidthPx=400&skipHttpRedirect=true with $GOOGLE_PLACES_API_KEY in query key")?;
    let photo_sent = said.starts_with("fetched 200");

    // Hermes' tool vendors share the existing upstream fake: each real
    // host and SDK header, with no vendor-specific response simulation.
    for (i, (n, name, host, env, header, scheme)) in [
        (130, "firecrawl", "api.firecrawl.dev", "FIRECRAWL_API_KEY", "authorization", "Bearer "),
        (131, "fal", "queue.fal.run", "FAL_KEY", "authorization", "Key "),
        (132, "browser-use", "api.browser-use.com", "BROWSER_USE_API_KEY", "x-browser-use-api-key", ""),
        (133, "xai", "api.x.ai", "XAI_API_KEY", "authorization", "Bearer "),
        (134, "x", "api.x.com", "X_API_BEARER_TOKEN", "authorization", "Bearer "),
    ].into_iter().enumerate() {
        let said = say(s, n, &format!("fetch http://{host}/e2e with ${env} in {header}"))?;
        let seen = s.upstream.seen().last().cloned().unwrap_or_default();
        s.ok(SWAP_CHECKS[22 + i],
            said.starts_with("fetched 200") && seen["host"] == host && seen["auth"][header] == format!("{scheme}{}", key_value(name)),
            json!({ "said": said, "seen": seen }));
    }
    // fal's image (FIN-445): queued as Hermes does, charged nothing (made
    // later, or not); run synchronously, charged below by its pixels; and
    // the call above (a poll's) nothing
    let said = say(s, 135, &format!("fetch http://queue.fal.run/fal-ai/flux-2/klein/9b with $FAL_KEY in authorization posting {FAL_ASK}"))?;
    let queued = said.starts_with("fetched 200") && s.upstream.seen().last().is_some_and(|seen| seen["host"] == "queue.fal.run" && seen["method"] == "POST");
    let said = say(s, 143, &format!("fetch http://fal.run/fal-ai/flux-2/klein/9b with $FAL_KEY in authorization posting {FAL_ASK}"))?;
    let fal_seen = s.upstream.seen().last().cloned().unwrap_or_default();
    let fal_sent = queued
        && said.starts_with("fetched 200")
        && fal_seen["host"] == "fal.run"
        && fal_seen["method"] == "POST"
        && fal_seen["path"] == "/fal-ai/flux-2/klein/9b"
        && fal_seen["body"] == FAL_ASK
        && fal_seen["auth"]["authorization"] == format!("Key {}", key_value("fal"));
    // Firecrawl's scrape (FIN-445): charged below its credit, the GET above nothing
    let said = say(s, 139, &format!("fetch http://api.firecrawl.dev/v2/scrape with $FIRECRAWL_API_KEY posting {FIRECRAWL_SCRAPE}"))?;
    let seen = s.upstream.seen().last().cloned().unwrap_or_default();
    let scrape_sent = said.starts_with("fetched 200") && seen["host"] == "api.firecrawl.dev" && seen["method"] == "POST" && seen["path"] == "/v2/scrape" && seen["body"] == FIRECRAWL_SCRAPE;
    // a Browser Use session's start, as Hermes' browser provider makes it (FIN-445)
    let said = say(s, 140, "fetch http://api.browser-use.com/api/v3/browsers with $BROWSER_USE_API_KEY in x-browser-use-api-key posting {}")?;
    let seen = s.upstream.seen().last().cloned().unwrap_or_default();
    let session_sent = said.starts_with("fetched 200") && seen["host"] == "api.browser-use.com" && seen["method"] == "POST" && seen["path"] == "/api/v3/browsers";
    // a task on that session (v4's `sessionId`): no new start
    let said = say(s, 144, r#"fetch http://api.browser-use.com/api/v4/runs with $BROWSER_USE_API_KEY in x-browser-use-api-key posting {"task":"e2e","sessionId":"e2e-session"}"#)?;
    let reused_sent = said.starts_with("fetched 200");
    // X: a lookup of two posts, as the x-api-finite helper asks, charged
    // nothing (its returned resources not counted), and its search ten posts
    let said = say(s, 141, "fetch http://api.x.com/2/tweets?ids=20%2C21 with $X_API_BEARER_TOKEN")?;
    let lookup_sent = said.starts_with("fetched 200") && s.upstream.seen().last().is_some_and(|seen| seen["host"] == "api.x.com" && seen["path"] == "/2/tweets");
    let said = say(s, 145, "fetch http://api.x.com/2/tweets/search/recent?query=e2e&max_results=10 with $X_API_BEARER_TOKEN")?;
    let search_sent = said.starts_with("fetched 200");
    // xAI's X search and a plain chat call (FIN-445): charged below as each is
    let mut xai_seen = vec![];
    for (n, path, body) in [(136, "/v1/responses", XAI_SEARCH), (137, "/v1/chat/completions", XAI_CHAT)] {
        let said = say(s, n, &format!("fetch http://api.x.ai{path} with $XAI_API_KEY posting {body}"))?;
        let seen = s.upstream.seen().last().cloned().unwrap_or_default();
        xai_seen.push(said.starts_with("fetched 200") && seen["host"] == "api.x.ai" && seen["method"] == "POST" && seen["path"] == path && seen["body"] == body);
    }

    // refusals: the wrong place, the wrong host, a forged tag, another computer's
    let seen_before = s.upstream.seen().len();
    let place = say(s, 110, "fetch http://api.perplexity.ai/search with $PERPLEXITY_API_KEY in x-api-key")?;
    s.ok(SWAP_CHECKS[10], place.starts_with("fetched 400") && place.contains("goes in authorization: Bearer {}") && s.upstream.seen().len() == seen_before, &place);
    let host = say(s, 111, &format!("fetch http://{SWAP_CONNECTION_HOST}/x with $PERPLEXITY_API_KEY"))?;
    s.ok(SWAP_CHECKS[11], host.starts_with("fetched 403") && host.contains("is for api.perplexity.ai, not www.googleapis.com") && s.upstream.seen().len() == seen_before, &host);
    let forged = say(s, 112, &format!("fetch http://api.perplexity.ai/search with fck_perplexity_{}", "0".repeat(fragment_core::swap::TAG_HEX)))?;
    s.ok(SWAP_CHECKS[12], forged.starts_with("fetched 403") && forged.contains("names no agent on this computer") && s.upstream.seen().len() == seen_before, &forged);
    let elsewhere = fragment_core::computer::default_computer_of(&api.identity(w.stranger)?);
    let other = say(s, 113, &format!("fetch http://api.perplexity.ai/search with {}", placeholder(s, &elsewhere, w.agent, "perplexity")))?;
    s.ok(SWAP_CHECKS[13], other.starts_with("fetched 403") && other.contains("names no agent on this computer") && s.upstream.seen().len() == seen_before, &other);

    // an own key, behind basic auth
    let own = "mail-own-e2e-55aa";
    let r = api.signed(w.owner, "PUT", &format!("/api/connections/{SWAP_OWN}/key"), Some(&json!({ "key": own })))?;
    let listed = api.signed(w.owner, "GET", "/api/connections", None)?;
    let given = say(s, 114, "credentials")?;
    s.ok(SWAP_CHECKS[14], r.status == 200 && state_of(&listed, SWAP_OWN) == "set" && given.contains(&format!("{SWAP_OWN} {SWAP_OWN_ENV} fck_{SWAP_OWN}_")), json!({ "put": r.body, "given": given }));
    let said = say(s, 115, &format!("fetch http://{SWAP_OWN_HOST}/v3/send with ${SWAP_OWN_ENV} as basic api"))?;
    let seen = last_seen(s, seen_before);
    let basic = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("api:{own}")));
    s.ok(SWAP_CHECKS[15], said.starts_with("fetched 200") && seen["host"] == SWAP_OWN_HOST && seen["auth"]["authorization"] == basic, json!({ "said": said, "seen": seen }));
    let bad = api.signed(w.owner, "PUT", &format!("/api/connections/{SWAP_OWN}/key"), Some(&json!({ "key": "two words" })))?;
    let not_own = api.signed(w.owner, "PUT", "/api/connections/perplexity/key", Some(&json!({ "key": "pplx-mine" })))?;
    let gone = api.signed(w.owner, "DELETE", &format!("/api/connections/{SWAP_OWN}/key"), None)?;
    let listed = api.signed(w.owner, "GET", "/api/connections", None)?;
    let given = say(s, 116, "credentials")?;
    s.ok(
        SWAP_CHECKS[16],
        bad.status == 400 && not_own.status == 400 && gone.status == 200 && state_of(&listed, SWAP_OWN) == "not_set" && !given.contains(SWAP_OWN_ENV),
        json!({ "bad": bad.status, "notOwn": not_own.status, "gone": gone.status, "given": given }),
    );

    // the charges: each operator key's call, at its price; and the month's counts
    let keyed = ["perplexity", "elevenlabs", "x"];
    // each key's call, the speech calls as its part (`price::KEY_PARTS`):
    // the one made, and the two refused; fal's run, its queued submit and
    // its poll; xAI's X search, chat and model list; Places' search,
    // details and photo; Firecrawl's scrape and the GET before it; Browser
    // Use's start, its task on that session and the GET before it; and X's
    // lookup and the GET before it (its search is keyed's)
    let held_rows = keyed.len() + 3 + 3 + 3 + 3 + 2 + 3 + 2;
    let held = || entries(api, &owner_id, &format!("key:{}:", w.id));
    let metered = s.eventually(Duration::from_secs(20), || held().iter().filter(|e| charged(e).is_some()).count() == held_rows);
    let rows = held();
    let theirs = |r: &Value| r["entry"]["row"]["agent"] == w.identity && r["entry"]["row"]["computer"] == w.id;
    let priced = rows.len() == held_rows
        && keyed.iter().all(|k| rows.iter().any(|r| *usage(r) == json!({ "kind": "key", "key": k, "units": 1 }) && theirs(r) && charged(r) == Some(charge_of(k))));
    s.ok(SWAP_CHECKS[17], metered && priced, json!(rows));
    let speech = charge_for("elevenlabs:speech", SPOKEN_CHARS);
    let spoken_row = rows.iter().any(|r| *usage(r) == json!({ "kind": "key", "key": "elevenlabs:speech", "units": SPOKEN_CHARS }) && theirs(r) && charged(r) == Some(speech));
    s.ok(SWAP_CHECKS[31], spoken_sent && spoken_row && speech < charge_of("elevenlabs") / 10, json!({ "seen": spoken_seen, "speech": speech, "rows": rows }));
    let nothing = rows.iter().filter(|r| *usage(r) == json!({ "kind": "key", "key": "elevenlabs:speech", "units": 0 }) && theirs(r) && charged(r) == Some(0)).count();
    // the agent's count, below, has its 4 ElevenLabs calls at the made ones' charges alone
    s.ok(SWAP_CHECKS[32], refused_sent && nothing == 2, json!({ "said": refused_said, "rows": rows }));
    let image = charge_for("fal:image", FAL_PIXELS);
    let fal_made = rows.iter().any(|r| *usage(r) == json!({ "kind": "key", "key": "fal:image", "units": FAL_PIXELS }) && theirs(r) && charged(r) == Some(image));
    let fal_polled = rows.iter().filter(|r| *usage(r) == json!({ "kind": "key", "key": "fal", "units": 0 }) && theirs(r) && charged(r) == Some(0)).count() == 2;
    s.ok(SWAP_CHECKS[33], fal_sent && fal_made && fal_polled && image < charge_of("fal"), json!({ "seen": fal_seen, "image": image, "rows": rows }));
    let held_row = |key: &str, units: i64, micros: i64| rows.iter().filter(|r| *usage(r) == json!({ "kind": "key", "key": key, "units": units }) && theirs(r) && charged(r) == Some(micros)).count();
    let xai_charged = held_row("xai", 1, charge_of("xai")) == 1 && held_row("xai", 0, 0) == 2;
    s.ok(SWAP_CHECKS[34], xai_seen == [true, true] && xai_charged, json!({ "seen": xai_seen, "rows": rows }));
    let photo = charge_of("google-places:photo");
    s.ok(SWAP_CHECKS[35], detail_sent && photo_sent && held_row("google-places:photo", 1, photo) == 1 && held_row("google-places", 0, 0) == 2, json!({ "photo": photo, "rows": rows }));
    let credit = charge_of("firecrawl:credit");
    s.ok(SWAP_CHECKS[36], scrape_sent && held_row("firecrawl:credit", 1, credit) == 1 && held_row("firecrawl", 0, 0) == 1 && credit < charge_of("firecrawl"), json!({ "credit": credit, "rows": rows }));
    s.ok(SWAP_CHECKS[37], session_sent && reused_sent && held_row("browser-use", 1, charge_of("browser-use")) == 1 && held_row("browser-use", 0, 0) == 2, json!({ "rows": rows }));
    s.ok(SWAP_CHECKS[38], lookup_sent && search_sent && held_row("x", 0, 0) == 2, json!({ "rows": rows }));
    let expect = [(SWAP_CONNECTION, 2, 0), ("perplexity", 1, charge_of("perplexity")), ("elevenlabs", 4, charge_of("elevenlabs") + speech), ("google-places", 3, charge_of("google-places:photo")), ("firecrawl", 2, charge_of("firecrawl:credit")), ("fal", 3, image), ("browser-use", 3, charge_of("browser-use")), ("xai", 3, charge_of("xai")), ("x", 3, charge_of("x")), (SWAP_OWN, 1, 0)];
    let uses = || api.signed(w.owner, "GET", &format!("/api/computers/{}/uses", w.id), None).map(|r| r.body).unwrap_or(Value::Null);
    let counted = |u: &Value| expect.iter().all(|(p, calls, micros)| u["uses"].as_array().is_some_and(|l| l.iter().any(|x| x["provider"] == *p && x["agent"] == w.agent && x["calls"] == *calls && x["micros"] == *micros)));
    let all = s.eventually(Duration::from_secs(20), || counted(&uses()));
    let u = uses();
    let month = w.month;
    s.ok(SWAP_CHECKS[18], all && u["month"] == month && u["computer"] == w.id, &u);
    let theirs = api.signed(w.stranger, "GET", &format!("/api/computers/{}/uses", w.id), None)?;
    let bad_month = api.signed(w.owner, "GET", &format!("/api/computers/{}/uses/2026-13", w.id), None)?;
    let this = api.signed(w.owner, "GET", &format!("/api/computers/{}/uses/{month}", w.id), None)?;
    s.ok(SWAP_CHECKS[19], theirs.status == 404 && bad_month.status == 400 && this.status == 200 && this.body == u, json!({ "stranger": theirs.status, "badMonth": bad_month.status, "month": this.body }));

    let said = say(s, 117, "fetch http://api.perplexity.ai/plain with nothing-swapped")?;
    let seen = s.upstream.seen().last().cloned().unwrap_or_default();
    s.ok(SWAP_CHECKS[20], said.starts_with("fetched 200") && seen["auth"]["authorization"] == "Bearer nothing-swapped", &seen);

    // with the owner's paid calls capped at none (their ledger's gate
    // refuses as at zero credit), the call is not made; the gate, which
    // heard the owner within the minute, asks afresh each time
    let cap = |max: u64| api.unsigned("POST", "/api/test/ledger", Some(&json!({ "identity": owner_id, "op": "paid-calls", "max": max })));
    let capped = cap(0)?;
    forget_gate(api, &owner_id)?;
    let seen_before = s.upstream.seen().len();
    let refused = say(s, 118, "fetch http://api.perplexity.ai/search with $PERPLEXITY_API_KEY")?;
    let uncapped = cap(u64::from(u32::MAX))?;
    forget_gate(api, &owner_id)?;
    s.ok(
        SWAP_CHECKS[21],
        capped.status == 200 && uncapped.status == 200 && refused.starts_with("fetched 402") && refused.contains("perplexity's call is not made") && s.upstream.seen().len() == seen_before && held().len() == held_rows,
        json!({ "refused": refused, "capped": capped.body }),
    );

    // its owner's hourly cap on a shared key (FIN-448), held where the
    // computer hands the key out: long speech calls reach it
    let cap = fragment_core::key_hour::KEY_HOUR_CAP;
    let of_elevenlabs = |r: &Value| usage(r)["key"].as_str().is_some_and(|k| k == "elevenlabs" || k.starts_with("elevenlabs:"));
    let spent: i64 = rows.iter().filter(|r| of_elevenlabs(r)).filter_map(charged).sum();
    let each = charge_for("elevenlabs:speech", LONG_CHARS);
    let calls = (cap - spent + each - 1) / each;
    anyhow::ensure!((1..=6).contains(&calls), "a few long calls reach the cap: {calls}");
    let eleven_calls = || {
        let u = api.signed(w.owner, "GET", &format!("/api/computers/{}/uses", w.id), None).map(|r| r.body).unwrap_or(Value::Null);
        u["uses"].as_array().map_or(0, |l| l.iter().filter(|x| x["provider"] == "elevenlabs").map(|x| x["calls"].as_u64().unwrap_or(0)).sum::<u64>())
    };
    let counted_before = eleven_calls();
    let long = format!(r#"{{"text":"{}","model_id":"eleven_multilingual_v2"}}"#, "a".repeat(LONG_CHARS as usize));
    let speak = |text: &str| format!("fetch http://api.elevenlabs.io/v1/text-to-speech/e2e-voice with $ELEVENLABS_API_KEY in xi-api-key posting {text}");
    let mut spoke = vec![];
    for n in 0..calls as u32 {
        spoke.push(say(s, 150 + n, &speak(&long))?);
    }
    let counted = s.eventually(Duration::from_secs(20), || eleven_calls() == counted_before + calls as u64);
    let seen_before = s.upstream.seen().len();
    let paused = say(s, 157, &speak(SPOKEN))?;
    let unreached = s.upstream.seen().len() == seen_before;
    let other = say(s, 158, "fetch http://api.perplexity.ai/search with $PERPLEXITY_API_KEY")?;
    let went = other.starts_with("fetched 200") && s.upstream.seen().last().is_some_and(|l| l["host"] == "api.perplexity.ai");
    let uncharged = s.eventually(Duration::from_secs(20), || held().iter().filter(|e| charged(e).is_some()).count() == held_rows + calls as usize + 1) && eleven_calls() == counted_before + calls as u64;
    s.ok(
        SWAP_CHECKS[39],
        spoke.iter().all(|said| said.starts_with("fetched 200"))
            && counted
            && paused.starts_with("fetched 429")
            && paused.contains(r#""error":"rate_limited""#)
            && paused.contains("elevenlabs's calls are paused: $5.")
            && paused.contains("they resume in about")
            && paused.contains(r#""reason":"key_paused""#)
            && paused.contains("retry_after_ms")
            && unreached
            && uncharged,
        json!({ "spent": spent, "calls": calls, "paused": paused, "counted": counted, "uncharged": uncharged }),
    );
    s.ok(SWAP_CHECKS[40], went, &other);
    let notices = || api.signed(w.owner, "GET", &format!("/api/computers/{}", w.id), None).map(|r| r.body["notices"].clone()).unwrap_or(Value::Null);
    let paused_now = || notices().as_array().is_some_and(|l| l.iter().any(|n| n["kind"] == "key_paused"));
    // its cap is its person's (FIN-448): set on their computer, never by an agent
    let key_cap = |who: &crate::Keys, provider: &str, body: Value| api.signed(who, "POST", &format!("/api/computers/{}/key-caps/{provider}", w.id), Some(&body));
    let (agent_keys, _) = api.own_agent(w.stranger)?;
    let by_agent = key_cap(&agent_keys, "elevenlabs", json!({}))?;
    let unsigned = api.unsigned("POST", &format!("/api/computers/{}/key-caps/elevenlabs", w.id), Some(&json!({})))?;
    let no_key = key_cap(w.owner, SWAP_CONNECTION, json!({}))?;
    let out = key_cap(w.owner, "elevenlabs", json!({ "micros": 0 }))?;
    let both = key_cap(w.owner, "elevenlabs", json!({ "from": cap, "micros": cap }))?;
    s.ok(
        SWAP_CHECKS[43],
        by_agent.status == 403 && unsigned.status == 401 && no_key.status == 404 && out.status == 400 && both.status == 400 && paused_now(),
        json!({ "agent": by_agent.status, "unsigned": unsigned.status, "noKey": no_key.status, "out": out.body, "both": both.body }),
    );
    let told = notices();
    let in_chat = key_notice_in_chat(s, api, w.owner)?;
    s.ok(
        SWAP_CHECKS[41],
        told.as_array().is_some_and(|l| l.iter().any(|n| n["kind"] == "key_paused" && n["provider"] == "elevenlabs" && n["capMicros"] == cap && n["allowMicros"] == 4 * cap))
            && in_chat.as_ref().is_some_and(|(shown, _, _)| *shown == json!({ "said": "ElevenLabs calls paused: $5.03 this hour", "buttons": ["Allow up to $20 an hour"] })),
        json!({ "told": told, "inChat": in_chat }),
    );
    let lifted = say(s, 160, &speak(SPOKEN))?;
    let unconfirmed = json!({ "lines": ["Couldn't confirm that. Try again."], "button": "Allow up to $20 an hour" });
    s.ok(
        SWAP_CHECKS[44],
        in_chat.as_ref().is_some_and(|(_, failed, gone)| *failed == unconfirmed && *gone) && !paused_now() && lifted.starts_with("fetched 200"),
        json!({ "lifted": lifted, "inChat": in_chat, "notices": notices() }),
    );
    // a page still showing the $5 notice taps again: nothing changes, so
    // $20 holds and the next call goes through
    let stale = key_cap(w.owner, "elevenlabs", json!({ "from": cap }))?;
    let kept = say(s, 161, &speak(SPOKEN))?;
    // its person's CLI, as its own: $1, so the key pauses at it; then the default
    let home = s.dir("key-cap-cli");
    let config = home.join(if cfg!(target_os = "macos") { "Library/Application Support" } else { ".config" }).join("fragment");
    std::fs::create_dir_all(&config)?;
    std::fs::write(config.join("config.json"), json!({ "secret_key": w.owner.secret_hex() }).to_string())?;
    let cli = |s: &Suite, usd: &str| {
        let out = s.cli(api, &home, &["ledger", "key-cap", "elevenlabs", usd]);
        (out.status.success(), String::from_utf8_lossy(&out.stdout).trim().to_string(), String::from_utf8_lossy(&out.stderr).trim().to_string())
    };
    let low = cli(s, "1");
    let held_low = say(s, 162, &speak(SPOKEN))?;
    let back = cli(s, "default");
    // a page still showing a $20 notice taps: the $5 the computer holds stays
    let late = key_cap(w.owner, "elevenlabs", json!({ "from": 4 * cap }))?;
    let repaused = say(s, 163, &speak(SPOKEN))?;
    s.ok(
        SWAP_CHECKS[45],
        stale.status == 200
            && kept.starts_with("fetched 200")
            && low.0
            && low.1 == "elevenlabs: $1.00 an hour"
            && held_low.starts_with("fetched 429")
            && held_low.contains("its limit $1.00 an hour")
            && back.0
            && back.1 == "elevenlabs: back to the default"
            && late.status == 200
            && repaused.starts_with("fetched 429")
            && repaused.contains("its limit $5.00 an hour")
            && paused_now(),
        json!({ "stale": stale.status, "kept": kept, "low": low, "heldLow": held_low, "back": back, "late": late.status, "repaused": repaused }),
    );
    // two hours on, by the computer's clock (its uses' month stays)
    let moved = super::computers::lever_with(api, w.id, json!({ "op": "clock", "offsetMs": super::ledger::mid_month_offset_ms() + 2 * fragment_core::key_hour::HOUR_MS }))?;
    let again = say(s, 159, &speak(SPOKEN))?;
    // a view with no notices leaves them out
    let gone = notices().as_array().is_none_or(|l| l.iter().all(|n| n["kind"] != "key_paused"));
    s.ok(SWAP_CHECKS[42], moved.status == 200 && again.starts_with("fetched 200") && gone, json!({ "again": again, "notices": notices() }));
    Ok(turns)
}

/// A paused key's notice over the agent's chat, as the shell shows it: its
/// words and its buttons; null while it is not shown.
const KEY_NOTICE: &str = "(() => { const box = document.getElementById('computer-notices'); const r = box?.querySelector('.computer-notice[data-kind=\"key_paused\"]'); if (!r || box.hidden) return null; return { said: r.querySelector('strong')?.textContent ?? null, buttons: [...r.querySelectorAll('button')].map((x) => x.textContent) }; })()";

/// Taps a paused key's Allow, enabled: whether it was.
const ALLOW: &str = "(() => { const x = document.querySelector('#computer-notices [data-action=\"allow-key\"]'); if (!x || x.disabled) return false; x.click(); return true; })()";
/// The page's asks of Allow refused (503) and its reads of the computer
/// failing as a network fails, until `__restoreFetch()`.
const KEY_ASKS_FAIL: &str = "(() => { const f = window.fetch; window.__restoreFetch = () => { window.fetch = f; }; window.fetch = async (url, o) => { const path = new URL(String(url), location.href).pathname; const method = o?.method ?? 'GET'; if (method === 'POST' && path.includes('/key-caps/')) return new Response(JSON.stringify({ error: 'unavailable', message: 'refused (a test)' }), { status: 503, headers: { 'content-type': 'application/json' } }); if (method === 'GET' && /^\\/api\\/computers\\/[^/]+$/.test(path)) throw new TypeError('Failed to fetch'); return f(url, o); }; return true; })()";
/// A paused key's notice whose tap went unconfirmed: its lines, and its
/// button, enabled; null while that button is not.
const KEY_UNCONFIRMED: &str = "(() => { const r = document.querySelector('#computer-notices .computer-notice[data-kind=\"key_paused\"]'); const b = r?.querySelector('[data-action=\"allow-key\"]'); return r && b && !b.disabled ? { lines: [...r.querySelectorAll('.notice-copy p')].map((p) => p.textContent), button: b.textContent } : null; })()";
/// The person's chat page in Chrome (the shell, signed in as `owner`): the
/// paused key's notice as it shows, shot at 1280 and 390 wide, light, in
/// the run's `key-guard/`; its Allow tapped while its ask and the
/// computer's read fail, and the notice then (shot); tapped again, whether
/// the notice went (shot again). None without Chrome.
fn key_notice_in_chat(s: &mut Suite, api: &Api, owner: &crate::Keys) -> Result<Option<(Value, Value, bool)>> {
    let Some(mut b) = s.browser()? else { return Ok(None) };
    let shots = s.dir("key-guard");
    let session = api.sign_in(&Api::email_of(owner))?;
    b.set_cookie(&format!("{}/", api.base), "fragment_session", &session)?;
    let page = b.open(&format!("{}/", api.base))?;
    b.viewport(&page, 1280, 900, false)?;
    let mut shown = Value::Null;
    s.eventually(Duration::from_secs(30), || {
        shown = b.eval(&page, KEY_NOTICE).unwrap_or(Value::Null);
        !shown.is_null()
    });
    super::telegram_settings::shoot(&mut b, &page, &shots, "k1-paused", Some("#computer-notices"))?;
    // tapped while its ask is refused and the computer's read fails: the
    // pause stays as last read, its button back with a short line
    b.eval(&page, KEY_ASKS_FAIL)?;
    b.eval(&page, ALLOW)?;
    let mut failed = Value::Null;
    s.eventually(Duration::from_secs(30), || {
        failed = b.eval(&page, KEY_UNCONFIRMED).unwrap_or(Value::Null);
        failed["lines"].as_array().is_some_and(|l| !l.is_empty())
    });
    super::telegram_settings::shoot(&mut b, &page, &shots, "k1b-unconfirmed", Some("#computer-notices"))?;
    b.eval(&page, "(window.__restoreFetch(), true)")?;
    let tapped = b.eval(&page, ALLOW)? == true;
    let gone = tapped && b.until(&page, &format!("{KEY_NOTICE} === null"), Duration::from_secs(30));
    super::telegram_settings::shoot(&mut b, &page, &shots, "k2-allowed", None)?;
    b.close(page)?;
    Ok(Some((shown, failed, gone)))
}

/// The checks `model_checks` makes, as a run without the stub or the fakes
/// skips them.
pub(super) const MODEL_CHECKS: &[&str] = &[
    "an own key signed in to is listed with its provider's page to revoke it and the models it offers, not set; its guest is given none",
    "connecting, from settings, sends the person's browser to the provider with the platform's callback, an S256 challenge, a bare state that names no computer, bound to that browser by a cookie, and the key's label",
    "a person who declines at the provider comes back to a page that says so; nothing is exchanged or set",
    "back from the provider with a code, the platform exchanges it with the sign-in's verifier, and the computer seals the key the provider made: connected, the key in no answer",
    "the guest is given a placeholder in OPENROUTER_API_KEY, with its models' base URL, and never the key",
    "its call to the provider with the placeholder reaches the provider with the person's key, counted as the agent's, never charged",
    "a sign-in comes back once: replayed, past its time, or a state that names none here, it is refused and nothing is exchanged",
    "sign-ins under way are bounded: past eight, another is refused until one ends",
    "disconnected, the guest is given it no more",
    "connecting is a browser's, the shell's: a request signed by a key is refused, and binds nothing",
    "a sign-in finishes only in the browser that began it, signed in as its person: sent to anyone else (no state cookie, no session, another person's session even with the cookie, or its state in another kind of round trip's cookie), it seals nothing and exchanges nothing, its refusal a 303 to a page whose address holds no code, and it is still the person's to finish",
    "a browser holds one own-key sign-in at a time: a second Connect replaces the first's binding, so the first comes back refused and the second connects",
];

/// The path and query of `url` (the platform's callback, as the provider
/// sends the browser to it), for the API's base.
fn path_of(url: &str) -> String {
    reqwest::Url::parse(url).map(|u| format!("{}{}", u.path(), u.query().map(|q| format!("?{q}")).unwrap_or_default())).unwrap_or_default()
}

/// Where the provider's page sent the browser (its 302's `location`).
pub(super) fn sent_back(api: &Api, page: &str) -> Result<String> {
    let r = api.external(page)?;
    Ok(r.headers.get("location").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string())
}

/// Settings' Connect, as the shell sends it (its session, from the
/// platform's own page): the provider's page, and the cookie binding the
/// sign-in to this browser.
pub(super) fn shell_authorize(api: &Api, session: &str) -> Result<crate::api::Reply> {
    api.call(crate::api::Call {
        method: "POST",
        url: format!("{}/api/connections/{SWAP_ROUTER}/authorize", api.base),
        body: Some(b"{}".to_vec()),
        content_type: Some("application/json"),
        cookie: Some(format!("fragment_session={session}")),
        extra: vec![("x-fragment-shell", "1".into()), ("sec-fetch-site", "same-origin".into()), ("origin", api.base.clone())],
        ..crate::api::Call::default()
    })
}

/// The `name=value` of the own-key sign-in's cookie an answer set, or "".
pub(super) fn own_key_cookie(r: &crate::api::Reply) -> String {
    r.cookies().into_iter().find(|c| c.starts_with("fragment_own_key=")).unwrap_or_default()
}

/// Where a browser landed back from the provider: the page it shows, and
/// the clean address a refusal sent it to on the way (`?refused=`), or ""
/// when the callback answered at once.
pub(super) struct Landed {
    pub page: crate::api::Reply,
    pub via: String,
}

impl std::ops::Deref for Landed {
    type Target = crate::api::Reply;
    fn deref(&self) -> &crate::api::Reply {
        &self.page
    }
}

impl std::fmt::Display for Landed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (via {:?})", self.page, self.via)
    }
}

impl Landed {
    /// A refusal, as the callback answers one: a 303 to the clean page,
    /// whose address holds no code nor state, and that page, saying so.
    pub(super) fn refused_cleanly(&self) -> bool {
        self.via.contains("?refused=") && !self.via.contains("code=") && !self.via.contains("state=") && self.page.status == 400 && self.page.text.contains("Not connected")
    }
}

/// A browser holding `cookies` (each `name=value`; "" for none) opens
/// `back`, where the provider sent it: the platform's callback, and the
/// clean page a refusal redirects to (303), as a browser follows it.
pub(super) fn came_back(api: &Api, back: &str, cookies: &[&str]) -> Result<Landed> {
    let cookie = cookies.iter().filter(|c| !c.is_empty()).copied().collect::<Vec<_>>().join("; ");
    let at = |path: String| api.call(crate::api::Call { method: "GET", url: format!("{}{path}", api.base), cookie: (!cookie.is_empty()).then(|| cookie.clone()), ..crate::api::Call::default() });
    let first = at(path_of(back))?;
    if first.status != 303 {
        return Ok(Landed { page: first, via: String::new() });
    }
    let via = first.header("location");
    let page = at(path_of(&format!("{}{via}", if via.starts_with('/') { api.base.as_str() } else { "" })))?;
    Ok(Landed { page, via })
}

/// An own key signed in to, through its provider's own page (Paul,
/// 2026-10-08; fragment_core::own_signin): OpenRouter's, at its fake. The
/// person connects it, the platform exchanges the code for a key the
/// provider makes, their computer seals it, and their agent's guest is
/// given only its placeholder, which the swap replaces on the way to the
/// provider. Answers how many turns of the agent's it took.
pub(super) fn model_checks(s: &mut Suite, api: &Api, w: &Swapping, fetched: &dyn Fn(&Suite, u32, &str) -> Result<String>) -> Result<usize> {
    let mut turns = 0;
    let mut say = |s: &Suite, n: u32, text: &str| -> Result<String> {
        turns += 1;
        fetched(s, n, text)
    };
    let row = |r: &crate::api::Reply| r.body["providers"].as_array().and_then(|l| l.iter().find(|x| x["provider"] == SWAP_ROUTER)).cloned().unwrap_or(Value::Null);
    let listed = api.signed(w.owner, "GET", "/api/connections", None)?;
    let given = say(s, 201, "credentials")?;
    let r = row(&listed);
    s.ok(
        MODEL_CHECKS[0],
        r["kind"] == "own" && r["state"] == "not_set" && r["signIn"]["manage"] == "https://openrouter.ai/settings/keys" && r["models"].as_array().is_some_and(|m| m.len() >= 2 && m.iter().all(|x| x["id"].is_string() && x["name"].is_string())) && !given.contains(SWAP_ROUTER_ENV),
        json!({ "row": r, "given": given }),
    );

    // connecting is the shell's, from settings: its session, and a browser
    // to bind the sign-in to; a key's own request is refused
    let owner_session = api.sign_in(&Api::email_of(w.owner))?;
    let stranger_session = api.sign_in(&Api::email_of(w.stranger))?;
    let session_of = |session: &str| format!("fragment_session={session}");
    let by_key = api.signed(w.owner, "POST", &format!("/api/connections/{SWAP_ROUTER}/authorize"), Some(&json!({})))?;
    s.ok(MODEL_CHECKS[9], by_key.status == 403 && own_key_cookie(&by_key).is_empty() && by_key.body["url"].is_null(), &by_key);
    let authorize = || shell_authorize(api, &owner_session);

    // the person declines at the provider: nothing set, the sign-in let go
    s.openrouter.decline_next();
    let declined = authorize()?;
    let back = sent_back(api, declined.body["url"].as_str().unwrap_or_default())?;
    let page = came_back(api, &back, &[&own_key_cookie(&declined), &session_of(&owner_session)])?;
    let exchanges = s.openrouter.exchanges().len();
    s.ok(
        MODEL_CHECKS[2],
        declined.status == 200 && !back.contains("code=") && page.refused_cleanly() && page.text.contains("sent no key back") && exchanges == 0 && row(&api.signed(w.owner, "GET", "/api/connections", None)?)["state"] == "not_set",
        json!({ "back": back, "page": page.to_string(), "exchanges": exchanges }),
    );

    // connecting: the browser goes to the provider's page, its state bound to it
    let asked = authorize()?;
    let url = asked.body["url"].as_str().unwrap_or_default().to_string();
    let q: std::collections::BTreeMap<String, String> = reqwest::Url::parse(&url).map(|u| u.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect()).unwrap_or_default();
    let state = q.get("state").cloned().unwrap_or_default();
    let bound = own_key_cookie(&asked);
    let bound_line = asked.headers.get_all("set-cookie").iter().filter_map(|v| v.to_str().ok()).find(|c| c.contains("fragment_own_key=")).unwrap_or("").to_string();
    s.ok(
        MODEL_CHECKS[1],
        asked.status == 200
            && url.starts_with(&format!("{}/auth?", s.openrouter.url))
            && q.get("callback_url") == Some(&format!("{}/api/connections/{SWAP_ROUTER}/callback", api.base))
            && q.get("code_challenge_method").map(String::as_str) == Some("S256")
            && q.get("code_challenge").is_some_and(|c| c.len() == 43)
            && fragment_core::roundtrip::valid_state(&state)
            && !state.contains(&w.id["computer:".len()..])
            && bound == format!("fragment_own_key={state}")
            && bound_line.contains("HttpOnly") && bound_line.contains("SameSite=Lax") && bound_line.contains("Max-Age=600")
            && q.get("key_label").map(String::as_str) == Some(fragment_core::own_signin::KEY_LABEL),
        json!({ "authorize": asked.body, "query": q, "cookie": bound_line }),
    );

    // the attack (audit H3): the sign-in the owner began, sent to someone
    // else, whose browser comes back with it; then the owner's own
    let back = sent_back(api, &url)?;
    let (exchanges, keys_before) = (s.openrouter.exchanges().len(), s.openrouter.keys().len());
    let elsewhere = [
        ("the owner's session, but another browser (no cookie)", came_back(api, &back, &[&session_of(&owner_session)])?),
        ("the cookie, signed in to no one", came_back(api, &back, &[&bound])?),
        ("another person's browser", came_back(api, &back, &[&session_of(&stranger_session)])?),
        ("another person's session, with the cookie", came_back(api, &back, &[&bound, &session_of(&stranger_session)])?),
        // a round trip's cookie is its kind's alone: a sign-in's holds no own key's state
        ("the owner's session, the state in a sign-in's cookie", came_back(api, &back, &[&format!("fragment_login={state}"), &format!("fragment_switch={state}"), &session_of(&owner_session)])?),
    ];
    let set = |keys: &crate::Keys| api.signed(keys, "GET", "/api/connections", None).map(|r| row(&r)["state"].clone()).unwrap_or(Value::Null);
    let sealed_nowhere = s.openrouter.exchanges().len() == exchanges && s.openrouter.keys().len() == keys_before && set(w.owner) == "not_set" && set(w.stranger) != "set";
    s.ok(
        MODEL_CHECKS[10],
        elsewhere.iter().all(|(_, r)| r.refused_cleanly()) && sealed_nowhere,
        json!(elsewhere.iter().map(|(how, r)| format!("{how}: {r}")).collect::<Vec<_>>()),
    );

    // back with a code, in the browser that began it, signed in: exchanged,
    // the key sealed by the person's own computer
    let page = came_back(api, &back, &[&bound, &session_of(&owner_session)])?;
    let keys = s.openrouter.keys();
    let key = keys.last().cloned().unwrap_or_default();
    let exchanged = s.openrouter.exchanges();
    let listed = api.signed(w.owner, "GET", "/api/connections", None)?;
    s.ok(
        MODEL_CHECKS[3],
        page.status == 200
            && page.text.contains("is connected")
            && keys.len() == 1
            && exchanged.last().is_some_and(|(_, status)| *status == 200)
            && row(&listed)["state"] == "set"
            && !page.text.contains(&key)
            && !listed.text.contains(&key),
        json!({ "page": page.to_string(), "exchanges": exchanged, "listed": row(&listed) }),
    );

    // the guest: a placeholder, its models' base, never the key
    let given = say(s, 202, "credentials")?;
    let placeholder = format!("fck_{SWAP_ROUTER}_");
    s.ok(
        MODEL_CHECKS[4],
        given.contains(&format!("{SWAP_ROUTER} {SWAP_ROUTER_ENV} {placeholder}")) && given.contains(&format!("models at https://{SWAP_ROUTER_HOST}/api/v1")) && !given.contains(&key),
        &given,
    );

    // its call reaches the provider with the person's key
    let calls_before = s.openrouter.calls().len();
    let said = say(s, 203, &format!("fetch http://{SWAP_ROUTER_HOST}/api/v1/key with ${SWAP_ROUTER_ENV}"))?;
    let calls = s.openrouter.calls();
    let uses = || api.signed(w.owner, "GET", &format!("/api/computers/{}/uses", w.id), None).map(|r| r.body).unwrap_or(Value::Null);
    let counted = s.eventually(Duration::from_secs(20), || uses()["uses"].as_array().is_some_and(|l| l.iter().any(|x| x["provider"] == SWAP_ROUTER && x["agent"] == w.agent && x["calls"] == 1 && x["micros"] == 0)));
    s.ok(
        MODEL_CHECKS[5],
        said.starts_with("fetched 200") && calls.len() == calls_before + 1 && calls.last().is_some_and(|(auth, path)| *auth == format!("Bearer {key}") && path == "/api/v1/key") && counted,
        json!({ "said": said, "calls": calls, "uses": uses() }),
    );

    // once only: a replay (its cookie kept), a sign-in past its time, and a
    // state naming no sign-in here, each with a cookie that matches it
    let again = came_back(api, &back, &[&bound, &session_of(&owner_session)])?;
    let expiring = authorize()?;
    let expiring_back = sent_back(api, expiring.body["url"].as_str().unwrap_or_default())?;
    let expired = super::computers::lever_with(api, w.id, json!({ "op": "expire-sign-ins" }))?;
    let late = came_back(api, &expiring_back, &[&own_key_cookie(&expiring), &session_of(&owner_session)])?;
    let forged_state = "a".repeat(fragment_core::roundtrip::STATE_HEX_LEN);
    let forged = format!("{}/api/connections/{SWAP_ROUTER}/callback?code=x&state={forged_state}", api.base);
    let none = came_back(api, &forged, &[&format!("fragment_own_key={forged_state}"), &session_of(&owner_session)])?;
    let garbled = came_back(api, &format!("{}/api/connections/{SWAP_ROUTER}/callback?code=x&state=nope", api.base), &["fragment_own_key=nope", &session_of(&owner_session)])?;
    s.ok(
        MODEL_CHECKS[6],
        again.refused_cleanly()
            && expired.status == 200
            && late.refused_cleanly()
            && none.refused_cleanly()
            && garbled.refused_cleanly()
            && s.openrouter.exchanges().len() == exchanged.len()
            && s.openrouter.keys().len() == 1,
        json!({ "replay": again.to_string(), "expired": expired.body, "late": late.to_string(), "none": none.to_string(), "garbled": garbled.to_string() }),
    );

    // a browser holds one own-key sign-in at a time: a second Connect's
    // binding replaces the first's, so the first comes back refused and
    // the second connects (fragment_core::roundtrip::Trip)
    let (first, second) = (authorize()?, authorize()?);
    let (first_back, second_back) = (sent_back(api, first.body["url"].as_str().unwrap_or_default())?, sent_back(api, second.body["url"].as_str().unwrap_or_default())?);
    let held = own_key_cookie(&second);
    let first_page = came_back(api, &first_back, &[&held, &session_of(&owner_session)])?;
    let second_page = came_back(api, &second_back, &[&held, &session_of(&owner_session)])?;
    s.ok(
        MODEL_CHECKS[11],
        first_page.refused_cleanly() && first_page.text.contains("another began since") && second_page.status == 200 && second_page.text.contains("is connected") && s.openrouter.keys().len() == 2,
        json!({ "first": first_page.to_string(), "second": second_page.to_string() }),
    );
    // the first's sign-in is left under way; past its time, it counts no more
    super::computers::lever_with(api, w.id, json!({ "op": "expire-sign-ins" }))?;

    // bounded: eight under way, the ninth refused
    let pending: Vec<u16> = (0..fragment_core::own_signin::SIGNINS_PENDING_MAX + 1).map(|_| authorize().map(|r| r.status).unwrap_or(0)).collect();
    s.ok(
        MODEL_CHECKS[7],
        pending[..fragment_core::own_signin::SIGNINS_PENDING_MAX].iter().all(|st| *st == 200) && pending[fragment_core::own_signin::SIGNINS_PENDING_MAX] == 429,
        json!(pending),
    );

    // disconnected
    let gone = api.signed(w.owner, "DELETE", &format!("/api/connections/{SWAP_ROUTER}/key"), None)?;
    let given = say(s, 204, "credentials")?;
    s.ok(MODEL_CHECKS[8], gone.status == 200 && !given.contains(SWAP_ROUTER_ENV), json!({ "gone": gone.body, "given": given }));
    Ok(turns)
}
