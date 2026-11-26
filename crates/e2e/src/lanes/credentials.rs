//! The computer's swap (Paul, 2026-10-04; docs/computers.md, "Connections
//! and operator keys"), as the computers lane drives it on the stub: its
//! scripted agent reads what its guest is given (`credentials`) and sends
//! requests as any SDK would (`fetch … with $NAME`), with no header of
//! ours, to the catalog's real hosts, which the swap sends to the upstream
//! fake. Each placement (a header, a query parameter, basic auth), each
//! kind (a connection through the WorkOS fake, the operator's keys, an own
//! key), and each refusal (a forged tag, another computer's, the wrong
//! host, the wrong place, a call an operator key's row denies), then the
//! charges and the counts, and a key's call its owner's ledger's gate
//! refuses.

use std::time::Duration;

use anyhow::Result;
use base64::Engine;
use serde_json::{json, Value};

use super::jobs::events;
use super::ledger::{charged, entries, forget_gate, usage};
use crate::api::Api;
use crate::{Suite, ORG_CONNECTION, ORG_CONNECTION_HOST, SEVERAL, SEVERAL_HOST, SWAP_CONNECTION, SWAP_CONNECTION_ENV, SWAP_CONNECTION_HOST, SWAP_KEYS, SWAP_OWN, SWAP_OWN_ENV, SWAP_OWN_HOST, SWAP_ROUTER, SWAP_ROUTER_ENV, SWAP_ROUTER_HOST};

/// Who the swap's checks act as, and on what.
pub(super) struct Swapping<'a> {
    pub owner: &'a crate::Keys,
    pub stranger: &'a crate::Keys,
    pub id: &'a str,
    pub agent: &'a str,
    pub identity: &'a str,
    /// The month its owner's ledger counts the agent's uses in
    /// (`computers::pin_month`).
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
    "an own key: its owner gives it, sealed in their Vault, and it is the guest's at once",
    "basic auth: the own key in its password's place, its user as it came",
    "a key that is no printable token is refused, and one sent bare (not in {key}) refused without being quoted back; taken away, the guest is given it no more",
    "each operator key's call its vendor answered is charged to the agent's owner's ledger after it, as the agent, what its vendor's answer said it used (or a call, where its answers say nothing), at its unit's list price and the margin; a call none of its row's charges prices, nothing",
    "this month's uses, in the owner's ledger: each call by agent, a connection's and an own key's counted, never charged, an operator key's at its charge",
    "no one else reads them (a stranger's are their own, none), and a month is YYYY-MM",
    "a request with no placeholder goes on as it came",
    "at zero credit, a gate that has heard nothing of its owner lets one operator key's call start, asking their ledger beside it, and charges it; the next is refused before it is made, reaching no provider",
    "firecrawl: its SDK header is swapped only at its catalog host",
    "fal: its SDK header is swapped only at its catalog host",
    "browser-use: its SDK header is swapped only at its catalog host",
    "xai: its SDK header is swapped only at its catalog host",
    "x: its SDK header is swapped only at its catalog host",
    "disconnected, an account Pipes holds again with no connect known to have imported it (an import landed late) is not its owner's: read as not connected, its swaps refused, no token asked",
    "of several accounts at a connection, an agent that picked none is swapped its owner's default, the oldest connected: Pipes' token for that account",
    "its owner picks another account for it: its next request is swapped that account's token",
    "the account it picked disconnected, it falls back to the default, its pick kept until it is next written",
    "ElevenLabs' text to speech reaches its host as it came, body and all, and is charged the characters its answer says it billed (character-cost), not a call; a read of its music (a GET) nothing",
    "and one its provider refused (a bad request, a rate limit) made nothing: no row, and the agent's count charges nothing",
    "fal: a synchronous Klein run reaches its host as it came and is charged the units its answer says (x-fal-billable-units); a queued submit, whose answer says none, and a poll nothing",
    "xai: each POST is charged the ticks its answer says it cost (usage.cost_in_usd_ticks), an X search and a plain chat alike; a read nothing",
    "google-places: charged nothing, a search, a place's details and its photo alike (kept uncharged, 2026-11-20); at its calls an hour from a computer (4 here), the next is refused 429 rate_limited, saying when to try again, reaching no provider; one refused before it goes (at zero credit) is none of them",
    "firecrawl: a scrape is charged the credits its answer says (data.metadata.creditsUsed), and a call around it nothing",
    "browser-use: a session's start and a task's are each charged five minutes, and a call around them (a status read) nothing",
    "x: a search is charged the posts its answer counts (meta.result_count), and a lookup whose answer counts none nothing",
    "past $5 of its agents' calls on shared keys in an hour, its owner is told so (a spending notice, the same until the next step, counting down to an end ahead on the real clock), and nothing pauses: the key's next call goes through and is charged",
    "its owner sees it over the agent's chat, in the server's words, with no button",
    "a connection whose accounts live at hosts of their own (Salesforce's orgs): a call to its account's own host, as Pipes' metadata names it, is swapped the account's credential, and one to its named host Pipes' token",
    "a call to another account's host under its suffix (another org's) is refused, naming its own account's host, and reaches no one",
    "an account whose Pipes metadata names no host under the suffix, or another host, has its calls there refused, reaching no one",
    "connected through its callback as one org, then again as another: the account is made again with the new org's host, never updated beside the old one's, so the token goes to the new org's host and the old org's is refused, reaching no one",
    "an operator key on a call its row denies (each vendor's: an account-wide read, a key minted or disabled) is refused 403, naming the call, and reaches no provider",
    "spelled otherwise (upper case, a doubled and a trailing /, a format suffix, an encoded letter) it is refused the same, and a path the row can't read plainly (a ;) is refused too, reaching no provider",
    "on the same keys, the calls their rows do not deny go on: Browser Use's start on the very path whose list is denied, ElevenLabs' speech, Firecrawl's scrape",
    "an operator key its provider echoes back reaches the guest named as its provider ({{name}}), never as the key",
];

/// What one call of a charge's unit is charged (`<vendor>:<unit>`).
fn charge_of(key: &str) -> i64 {
    charge_for(key, 1)
}

/// What `units` of a charge's unit (`<vendor>:<unit>`, the price book's
/// charges for its vendor: fragment_core::price DEFAULT_CHARGES) are
/// charged, its margin with them.
fn charge_for(key: &str, units: i64) -> i64 {
    let (vendor, unit) = key.split_once(':').expect("a charge's unit is <vendor>:<unit>");
    let charge = fragment_core::catalog::default_charges(vendor).into_iter().find(|c| c.unit == unit).expect("each unit has a charge");
    let (micros, per) = (charge.price.micros, charge.price.per);
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
/// queue: one `landscape_16_9` image.
const FAL_ASK: &str = r#"{"prompt":"a lighthouse at dusk","image_size":"landscape_16_9"}"#;

/// A Firecrawl scrape, as Hermes' `web_extract` posts it.
const FIRECRAWL_SCRAPE: &str = r#"{"url":"https://example.com/","formats":["markdown","html"]}"#;

/// An X search, as Hermes' `x_search` posts it, and a plain Grok chat
/// call: no spaces, so no word of theirs is a rule of the scripted agent's.
const XAI_SEARCH: &str = r#"{"model":"grok-4.3","input":"news","tools":[{"type":"x_search"}]}"#;
const XAI_CHAT: &str = r#"{"model":"grok-4.3","messages":[{"role":"user","content":"hi"}]}"#;

/// The placeholder the platform gives `agent` on computer `id` for
/// `provider`: a local run holds the node's host secret, so it makes one
/// as the platform does (a forgery needs the secret; a guest has none).
pub(super) fn placeholder(s: &Suite, id: &str, agent: &str, provider: &str) -> String {
    let kind = if [SWAP_CONNECTION, SEVERAL, ORG_CONNECTION].contains(&provider) { fragment_core::catalog::Kind::Connection } else { fragment_core::catalog::Kind::Operator };
    let tag = fragment_core::swap::TagKey::derive(&s.host_secret).tag(id, agent, provider);
    fragment_core::swap::Placeholder::new(kind, provider, &tag).text()
}

/// The upstream fake's last request, after `before` of them.
fn last_seen(s: &Suite, before: usize) -> Value {
    s.upstream.seen().get(before..).and_then(|l| l.last().cloned()).unwrap_or(Value::Null)
}

/// The own key `swap_checks` sets, behind basic auth, and one it sends bare
/// (a JSON string, not `{key}`), refused.
const SWAP_OWN_KEY: &str = "mail-own-e2e-55aa";
const SWAP_OWN_BARE: &str = "mail-own-e2e-bare-0c9e";

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

    // a connection whose accounts live at hosts of their own (its row's
    // account_host): its named host takes Pipes' token, and a host under
    // its suffix the account's credential, at the account's own host alone
    let under = fragment_fakes::workos::ACCOUNT_HOSTS_UNDER;
    let own = format!("acme.{under}");
    s.workos.connect(&email, ORG_CONNECTION, true);
    s.workos.keep_metadata(&email, ORG_CONNECTION, json!({ "instance_url": format!("https://{own}") }));
    // the owner's read tells the computer at once, as above
    let r = api.signed(w.owner, "GET", "/api/connections", None)?;
    anyhow::ensure!(state_of(&r, ORG_CONNECTION) == "connected", "the account host's connection connected: {r}");
    let fetch = |s: &Suite, host: &str| format!("fetch http://{host}/services/data/v62.0/sobjects with {}", placeholder(s, w.id, w.agent, ORG_CONNECTION));
    let swapped_at = |s: &Suite, host: &str| s.workos.tokens(ORG_CONNECTION).last().is_some_and(|t| s.upstream.seen().last().is_some_and(|seen| seen["host"] == host && seen["auth"]["authorization"] == format!("Bearer {t}")));
    let at_own = say(s, 164, &fetch(s, &own))?;
    let own_swapped = swapped_at(s, &own);
    let at_login = say(s, 165, &fetch(s, ORG_CONNECTION_HOST))?;
    s.ok(SWAP_CHECKS[41], at_own.starts_with("fetched 200") && own_swapped && at_login.starts_with("fetched 200") && swapped_at(s, ORG_CONNECTION_HOST), json!({ "own": at_own, "login": at_login }));
    let seen_before = s.upstream.seen().len();
    let another = say(s, 166, &fetch(s, &format!("rival.{under}")))?;
    s.ok(SWAP_CHECKS[42], another.starts_with("fetched 403") && another.contains(&format!("is at {own}")) && s.upstream.seen().len() == seen_before, &another);
    s.workos.keep_metadata(&email, ORG_CONNECTION, json!({}));
    let unnamed = say(s, 167, &fetch(s, &own))?;
    s.workos.keep_metadata(&email, ORG_CONNECTION, json!({ "instance_url": "https://acme.org.test" }));
    let elsewhere = say(s, 168, &fetch(s, &own))?;
    s.ok(
        SWAP_CHECKS[43],
        [&unnamed, &elsewhere].iter().all(|said| said.starts_with("fetched 5") && said.contains(&format!("names no host under {under}"))) && s.upstream.seen().len() == seen_before,
        json!({ "unnamed": unnamed, "elsewhere": elsewhere }),
    );
    // through its callback, as a person connects: one org, then another;
    // an account there already is made again, its host the new grant's
    let session = api.sign_in(&email)?;
    let first = connected_as(api, &session, ORG_CONNECTION, json!({ "access": "write" }), "acme")?;
    let at_first = say(s, 169, &fetch(s, &own))?;
    let first_swapped = swapped_at(s, &own);
    let rival = format!("rival.{under}");
    let second = connected_as(api, &session, ORG_CONNECTION, json!({ "access": "write" }), "rival")?;
    let at_second = say(s, 170, &fetch(s, &rival))?;
    let second_swapped = swapped_at(s, &rival);
    let seen_before = s.upstream.seen().len();
    let at_old = say(s, 178, &fetch(s, &own))?;
    s.ok(
        SWAP_CHECKS[44],
        first.status == 200 && at_first.starts_with("fetched 200") && first_swapped && second.status == 200 && at_second.starts_with("fetched 200") && second_swapped
            && at_old.starts_with("fetched 403") && at_old.contains(&format!("is at {rival}")) && s.upstream.seen().len() == seen_before
            && s.workos.metadata(&email, ORG_CONNECTION) == Some(json!({ "instance_url": format!("https://{rival}") })),
        json!({ "first": first.status, "atFirst": at_first, "second": second.status, "atSecond": at_second, "atOld": at_old }),
    );
    let gone = api.signed(w.owner, "DELETE", &format!("/api/connections/{ORG_CONNECTION}"), None)?;
    anyhow::ensure!(gone.status == 200, "the account host's checks cleared: {gone}");

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
    // its fourth and fifth calls of the hour: after one refused at zero credit, below

    // Hermes' tool vendors share the existing upstream fake: each real
    // host and SDK header, with no vendor-specific response simulation.
    // The fake answers with what arrived, its auth headers too, as a
    // provider that echoes its key would.
    let mut echoes = vec![];
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
        echoes.push((name, said));
    }
    let hidden = echoes.iter().all(|(name, said)| said.contains(&format!("{{{{{name}}}}}")) && !SWAP_KEYS.iter().any(|(_, value, _)| said.contains(value)));
    s.ok(SWAP_CHECKS[SWAP_CHECKS.len() - 1], hidden, json!({ "echoes": echoes }));
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

    // a key every agent shares never goes on a call its row denies
    // (deploy/e2e.jsonc's `deny`, as deploy/example.jsonc's), refused
    // before the gate or any provider, so neither charged nor counted below
    let mut denied = vec![];
    for (n, url, env, header, posting, call) in [
        (190, "http://api.elevenlabs.io/v1/workspaces/api-keys/disable?api_key_name=self", "ELEVENLABS_API_KEY", "xi-api-key", true, "POST /v1/workspaces/api-keys/disable"),
        (191, "http://rest.fal.ai/tokens/", "FAL_KEY", "authorization", true, "* /tokens/**"),
        (192, "http://api.firecrawl.dev/v2/team/activity", "FIRECRAWL_API_KEY", "authorization", false, "* /*/team/**"),
        // the same lists under their other names (Firecrawl's routes)
        (186, "http://api.firecrawl.dev/v2/crawl/ongoing", "FIRECRAWL_API_KEY", "authorization", false, "GET /*/crawl/ongoing"),
        (187, "http://api.firecrawl.dev/v2/browser", "FIRECRAWL_API_KEY", "authorization", false, "GET /v2/browser"),
        (193, "http://api.browser-use.com/api/v3/browsers", "BROWSER_USE_API_KEY", "x-browser-use-api-key", false, "GET /api/*/browsers"),
        (194, "http://api.perplexity.ai/generate_auth_token", "PERPLEXITY_API_KEY", "authorization", true, "* /generate_auth_token"),
        (195, "http://api.x.com/2/usage/tweets", "X_API_BEARER_TOKEN", "authorization", false, "* /2/usage/**"),
    ] {
        let said = say(s, n, &format!("fetch {url} with ${env} in {header}{}", if posting { " posting {}" } else { "" }))?;
        denied.push((said.starts_with("fetched 403") && said.contains(&format!("never sent on {call}")), said));
    }
    s.ok(SWAP_CHECKS[45], denied.iter().all(|(ok, _)| *ok) && s.upstream.seen().len() == seen_before, json!(denied));
    let mut spelled = vec![];
    for (n, path) in [(196, "/API/V3//Browsers/"), (197, "/api/v3/browsers.json"), (198, "/api/v3/%62rowsers")] {
        let said = say(s, n, &format!("fetch http://api.browser-use.com{path} with $BROWSER_USE_API_KEY in x-browser-use-api-key"))?;
        spelled.push((said.starts_with("fetched 403") && said.contains("never sent on GET /api/*/browsers"), said));
    }
    let unplain = say(s, 199, "fetch http://api.browser-use.com/api/v3/browsers;x with $BROWSER_USE_API_KEY in x-browser-use-api-key")?;
    spelled.push((unplain.starts_with("fetched 403") && unplain.contains("goes only on a path read plainly"), unplain));
    s.ok(SWAP_CHECKS[46], spelled.iter().all(|(ok, _)| *ok) && s.upstream.seen().len() == seen_before, json!(spelled));
    s.ok(SWAP_CHECKS[47], session_sent && spoken_sent && scrape_sent, json!({ "session": session_sent, "spoken": spoken_sent, "scrape": scrape_sent }));

    // an own key, behind basic auth
    let own = SWAP_OWN_KEY;
    let r = api.signed(w.owner, "PUT", &format!("/api/connections/{SWAP_OWN}/key"), Some(&json!({ "key": own })))?;
    let listed = api.signed(w.owner, "GET", "/api/connections", None)?;
    let given = say(s, 114, "credentials")?;
    s.ok(SWAP_CHECKS[14], r.status == 200 && state_of(&listed, SWAP_OWN) == "set" && given.contains(&format!("{SWAP_OWN} {SWAP_OWN_ENV} fck_{SWAP_OWN}_")), json!({ "put": r.body, "given": given }));
    let said = say(s, 115, &format!("fetch http://{SWAP_OWN_HOST}/v3/send with ${SWAP_OWN_ENV} as basic api"))?;
    let seen = last_seen(s, seen_before);
    let basic = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("api:{own}")));
    s.ok(SWAP_CHECKS[15], said.starts_with("fetched 200") && seen["host"] == SWAP_OWN_HOST && seen["auth"]["authorization"] == basic, json!({ "said": said, "seen": seen }));
    let bad = api.signed(w.owner, "PUT", &format!("/api/connections/{SWAP_OWN}/key"), Some(&json!({ "key": "two words" })))?;
    let bare = api.signed(w.owner, "PUT", &format!("/api/connections/{SWAP_OWN}/key"), Some(&json!(SWAP_OWN_BARE)))?;
    let not_own = api.signed(w.owner, "PUT", "/api/connections/perplexity/key", Some(&json!({ "key": "pplx-mine" })))?;
    let gone = api.signed(w.owner, "DELETE", &format!("/api/connections/{SWAP_OWN}/key"), None)?;
    let listed = api.signed(w.owner, "GET", "/api/connections", None)?;
    let given = say(s, 116, "credentials")?;
    s.ok(
        SWAP_CHECKS[16],
        bad.status == 400 && bare.status == 400 && !bare.text.contains(SWAP_OWN_BARE) && not_own.status == 400 && gone.status == 200 && state_of(&listed, SWAP_OWN) == "not_set" && !given.contains(SWAP_OWN_ENV),
        json!({ "bad": bad.status, "bare": bare.status, "bareQuoted": bare.text.contains(SWAP_OWN_BARE), "notOwn": not_own.status, "gone": gone.status, "given": given }),
    );

    // the charges: each operator key's call its vendor answered, at what its
    // answer said (fragment_fakes upstream's `usage_of`), and the month's
    // counts: Perplexity's search, a call; the speech's characters; Klein's
    // units; the scrape's credit; Browser Use's two starts; X's search's
    // posts; and xAI's two POSTs' ticks. Every other call made no row: a
    // read, a call its vendor refused, Places', a queued submit, a lookup
    use fragment_fakes::upstream::{FAL_UNITS, FIRECRAWL_SCRAPE_CREDITS, XAI_TICKS, X_POSTS};
    let spoken = SPOKEN_CHARS as u64;
    let expected: [(&str, u64, usize); 7] = [("perplexity:call", 1, 1), ("elevenlabs:character", spoken, 1), ("fal:flux-2-klein-9b", FAL_UNITS, 1), ("firecrawl:credit", FIRECRAWL_SCRAPE_CREDITS, 1), ("browser-use:session", 1, 2), ("x:post", X_POSTS, 1), ("xai:tick", XAI_TICKS, 2)];
    let held_rows: usize = expected.iter().map(|(_, _, n)| n).sum();
    let held = || entries(api, &owner_id, &format!("key:{}:", w.id));
    let metered = s.eventually(Duration::from_secs(20), || held().iter().filter(|e| charged(e).is_some()).count() == held_rows);
    let rows = held();
    let theirs = |r: &Value| r["entry"]["row"]["agent"] == w.identity && r["entry"]["row"]["computer"] == w.id;
    let held_row = |key: &str, units: u64| rows.iter().filter(|r| *usage(r) == json!({ "kind": "key", "key": key, "units": units }) && theirs(r) && charged(r) == Some(charge_for(key, units as i64))).count();
    let priced = rows.len() == held_rows && expected.iter().all(|(key, units, n)| held_row(key, *units) == *n);
    s.ok(SWAP_CHECKS[17], metered && priced, json!(rows));
    let speech = charge_for("elevenlabs:character", SPOKEN_CHARS);
    s.ok(SWAP_CHECKS[31], spoken_sent && held_row("elevenlabs:character", spoken) == 1 && speech < 150_000 / 10, json!({ "seen": spoken_seen, "speech": speech, "rows": rows }));
    // the agent's count, below, has its 4 ElevenLabs calls at the made one's charge alone
    s.ok(SWAP_CHECKS[32], refused_sent, json!({ "said": refused_said, "rows": rows }));
    let image = charge_for("fal:flux-2-klein-9b", FAL_UNITS as i64);
    s.ok(SWAP_CHECKS[33], fal_sent && held_row("fal:flux-2-klein-9b", FAL_UNITS) == 1, json!({ "seen": fal_seen, "image": image, "rows": rows }));
    let tick = charge_for("xai:tick", XAI_TICKS as i64);
    s.ok(SWAP_CHECKS[34], xai_seen == [true, true] && held_row("xai:tick", XAI_TICKS) == 2, json!({ "seen": xai_seen, "tick": tick, "rows": rows }));
    let credit = charge_for("firecrawl:credit", FIRECRAWL_SCRAPE_CREDITS as i64);
    s.ok(SWAP_CHECKS[36], scrape_sent && held_row("firecrawl:credit", FIRECRAWL_SCRAPE_CREDITS) == 1, json!({ "credit": credit, "rows": rows }));
    s.ok(SWAP_CHECKS[37], session_sent && reused_sent && held_row("browser-use:session", 1) == 2, json!({ "rows": rows }));
    let posts = charge_for("x:post", X_POSTS as i64);
    s.ok(SWAP_CHECKS[38], lookup_sent && search_sent && held_row("x:post", X_POSTS) == 1, json!({ "posts": posts, "rows": rows }));
    let expect = [
        (SWAP_CONNECTION, 2, 0),
        ("perplexity", 1, charge_of("perplexity:call")),
        ("elevenlabs", 4, speech),
        ("google-places", 3, 0),
        ("firecrawl", 2, credit),
        ("fal", 3, image),
        ("browser-use", 3, 2 * charge_of("browser-use:session")),
        ("xai", 3, 2 * tick),
        ("x", 3, posts),
        (SWAP_OWN, 1 + OWN_MOVE_CALLS, 0),
    ];
    let uses = || api.signed(w.owner, "GET", "/api/ledger/uses", None).map(|r| r.body).unwrap_or(Value::Null);
    let counted = |u: &Value| expect.iter().all(|(p, calls, micros)| u["uses"].as_array().is_some_and(|l| l.iter().any(|x| x["provider"] == *p && x["agent"] == w.agent && x["calls"] == *calls && x["micros"] == *micros)));
    let all = s.eventually(Duration::from_secs(20), || counted(&uses()));
    let u = uses();
    let month = w.month;
    s.ok(SWAP_CHECKS[18], all && u["month"] == month, &u);
    let theirs = api.signed(w.stranger, "GET", &format!("/api/ledger/uses/{month}"), None)?;
    let bad_month = api.signed(w.owner, "GET", "/api/ledger/uses/2026-13", None)?;
    let this = api.signed(w.owner, "GET", &format!("/api/ledger/uses/{month}"), None)?;
    s.ok(
        SWAP_CHECKS[19],
        theirs.status == 200 && theirs.body["uses"] == json!([]) && bad_month.status == 400 && this.status == 200 && this.body == u,
        json!({ "stranger": theirs.body, "badMonth": bad_month.status, "month": this.body }),
    );

    let said = say(s, 117, "fetch http://api.perplexity.ai/plain with nothing-swapped")?;
    let seen = s.upstream.seen().last().cloned().unwrap_or_default();
    s.ok(SWAP_CHECKS[20], said.starts_with("fetched 200") && seen["auth"]["authorization"] == "Bearer nothing-swapped", &seen);

    // with the owner's paid calls capped at none (their ledger's gate
    // refuses as at zero credit), a gate that has heard nothing of them
    // (as an isolate made anew) lets one key's call start, asking their
    // ledger beside it (crate::pipe), and refuses the next before it is made
    let cap = |max: u64| api.unsigned("POST", "/api/test/ledger", Some(&json!({ "identity": owner_id, "op": "paid-calls", "max": max })));
    let capped = cap(0)?;
    forget_gate(api, &owner_id)?;
    let seen_before = s.upstream.seen().len();
    let went = say(s, 118, "fetch http://api.perplexity.ai/search with $PERPLEXITY_API_KEY")?;
    let taught = super::ledger::gate_hears(s, api, &owner_id, "refused");
    let seen_went = s.upstream.seen().len();
    let refused = say(s, 127, "fetch http://api.perplexity.ai/search with $PERPLEXITY_API_KEY")?;
    // Places', refused so too, is not of its calls an hour: its fourth goes after
    let places = say(s, 148, "fetch http://places.googleapis.com/v1/places:searchText?fields=name with $GOOGLE_PLACES_API_KEY in query key")?;
    let unreached = s.upstream.seen().len() == seen_went;
    let uncapped = cap(u64::from(u32::MAX))?;
    forget_gate(api, &owner_id)?;
    let fourth = say(s, 146, "fetch http://places.googleapis.com/v1/places:searchText?fields=name with $GOOGLE_PLACES_API_KEY in query key")?;
    let places_before = s.upstream.seen().len();
    let fifth = say(s, 147, "fetch http://places.googleapis.com/v1/places:searchText?fields=name with $GOOGLE_PLACES_API_KEY in query key")?;
    let limited = places.starts_with("fetched 402")
        && places.contains("google-places's call is not made")
        && fourth.starts_with("fetched 200")
        && fifth.starts_with("fetched 429")
        && fifth.contains(r#""error":"rate_limited""#)
        && fifth.contains("google-places takes at most 4 calls an hour from this computer")
        && fifth.contains("retry_after_ms")
        && s.upstream.seen().len() == places_before;
    s.ok(SWAP_CHECKS[35], detail_sent && photo_sent && limited && !held().iter().any(|r| usage(r)["key"].as_str().is_some_and(|k| k.starts_with("google-places"))), json!({ "places": places, "fourth": fourth, "fifth": fifth }));
    let charged_once = s.eventually(Duration::from_secs(20), || held().iter().filter(|e| charged(e).is_some()).count() == held_rows + 1);
    s.ok(
        SWAP_CHECKS[21],
        capped.status == 200
            && uncapped.status == 200
            && went.starts_with("fetched 200")
            && seen_went == seen_before + 1
            && taught
            && refused.starts_with("fetched 402")
            && refused.contains("perplexity's call is not made")
            && unreached
            && charged_once,
        json!({ "went": went, "refused": refused, "capped": capped.body }),
    );

    // the hour's spend on shared keys (FIN-616: a notice, where FIN-448's
    // hourly pause was): long speech calls take this hour's charges past $5
    let step = fragment_core::ledger::said::SPENDING_MICROS;
    let spent: i64 = held().iter().filter_map(charged).sum();
    let each = charge_for("elevenlabs:character", LONG_CHARS);
    let calls = (step - spent + each - 1) / each;
    anyhow::ensure!((1..=6).contains(&calls), "a few long calls pass the step: {calls}");
    let long = format!(r#"{{"text":"{}","model_id":"eleven_multilingual_v2"}}"#, "a".repeat(LONG_CHARS as usize));
    let speak = |text: &str| format!("fetch http://api.elevenlabs.io/v1/text-to-speech/e2e-voice with $ELEVENLABS_API_KEY in xi-api-key posting {text}");
    let mut spoke = vec![];
    for n in 0..calls as u32 {
        spoke.push(say(s, 150 + n, &speak(&long))?);
    }
    let notices = || api.signed(w.owner, "GET", "/api/notices", None).map(|r| r.body["notices"].clone()).unwrap_or(Value::Null);
    let spending = || notices().as_array().and_then(|l| l.iter().find(|n| n["kind"] == "spending").cloned());
    let told = s.eventually(Duration::from_secs(20), || spending().is_some());
    let seen_before = s.upstream.seen().len();
    let after = say(s, 157, &speak(SPOKEN))?;
    let reached = s.upstream.seen().len() == seen_before + 1;
    let charged_after = s.eventually(Duration::from_secs(20), || held().iter().filter(|e| charged(e).is_some()).count() == held_rows + 1 + calls as usize + 1);
    let notice = spending().unwrap_or(Value::Null);
    s.ok(
        SWAP_CHECKS[39],
        spoke.iter().all(|said| said.starts_with("fetched 200"))
            && told
            && notice["title"] == "Your agents spent over $5 on tools in the last hour"
            && notice["level"] == "info"
            // its end is on the epoch's clock, whatever the ledger's says
            // (this lane moves the ledger's to mid-month: `pin_month`)
            && notice["until"]["at"].as_i64().is_some_and(|at| at > crate::api::now_ms())
            && after.starts_with("fetched 200")
            && reached
            && charged_after,
        json!({ "spent": spent, "calls": calls, "notice": notice, "after": after }),
    );
    let in_chat = spend_notice_in_chat(s, api, w.owner)?;
    s.ok(
        SWAP_CHECKS[40],
        in_chat.as_ref().is_some_and(|shown| *shown == json!({ "said": "Your agents spent over $5 on tools in the last hour", "buttons": [] })),
        json!({ "inChat": in_chat }),
    );
    Ok(turns)
}

/// The hour's spending notice over the agent's chat, as the shell shows
/// it: its words and its buttons; null while it is not shown.
const SPEND_NOTICE: &str = "(() => { const box = document.getElementById('computer-notices'); const r = box?.querySelector('.computer-notice[data-kind=\"spending\"]'); if (!r || box.hidden) return null; return { said: r.querySelector('strong')?.textContent ?? null, buttons: [...r.querySelectorAll('button')].map((x) => x.textContent) }; })()";

/// The person's chat page in Chrome (the shell, signed in as `owner`): the
/// spending notice as it shows, shot at 1280 and 390 wide, light, in the
/// run's `spend-notice/`. None without Chrome.
fn spend_notice_in_chat(s: &mut Suite, api: &Api, owner: &crate::Keys) -> Result<Option<Value>> {
    let Some(mut b) = s.browser()? else { return Ok(None) };
    let shots = s.dir("spend-notice");
    let session = api.sign_in(&Api::email_of(owner))?;
    b.set_cookie(&format!("{}/", api.base), "fragment_session", &session)?;
    let page = b.open(&format!("{}/", api.base))?;
    b.viewport(&page, 1280, 900, false)?;
    let mut shown = Value::Null;
    s.eventually(Duration::from_secs(30), || {
        shown = b.eval(&page, SPEND_NOTICE).unwrap_or(Value::Null);
        !shown.is_null()
    });
    super::telegram_settings::shoot(&mut b, &page, &shots, "s1-spending", Some("#computer-notices"))?;
    b.close(page)?;
    Ok(Some(shown))
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

/// A connect of `provider` through its callback, as Settings makes one (its
/// body `body`): the person's session begins it, the stand-in provider's
/// consent signs in as `account` and grants all it asks for, and their
/// browser comes back to the callback, whose answer this is.
fn connected_as(api: &Api, session: &str, provider: &str, body: Value, account: &str) -> Result<crate::api::Reply> {
    let r = api.call(crate::api::Call {
        method: "POST",
        url: format!("{}/api/connections/{provider}/authorize", api.base),
        body: Some(body.to_string().into_bytes()),
        content_type: Some("application/json"),
        cookie: Some(format!("fragment_session={session}")),
        extra: vec![("x-fragment-shell", "1".into()), ("sec-fetch-site", "same-origin".into()), ("origin", api.base.clone())],
        ..crate::api::Call::default()
    })?;
    anyhow::ensure!(r.status == 200, "begin {provider}: {r}");
    let bound = r.cookies().into_iter().find(|c| c.starts_with("fragment_connection=")).ok_or_else(|| anyhow::anyhow!("no connection cookie: {r}"))?;
    let mut to = reqwest::Url::parse(r.body["url"].as_str().unwrap_or_default())?;
    to.query_pairs_mut().append_pair("account", account);
    let consent = api.external(to.as_str())?;
    anyhow::ensure!(consent.status == 302, "the stand-in provider's consent: {consent}");
    api.call(crate::api::Call { method: "GET", url: consent.header("location"), cookie: Some(format!("fragment_session={session}; {bound}")), ..crate::api::Call::default() })
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
    let uses = || api.signed(w.owner, "GET", "/api/ledger/uses", None).map(|r| r.body).unwrap_or(Value::Null);
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

/// The checks `vault_checks` makes (FIN-466), as a run without the stub
/// or the fakes skips them.
pub(super) const VAULT_CHECKS: &[&str] = &[
    "a Vault secret its person binds to the agent is in its view at its next read, its own placeholder toward its hosts; one bound to another agent, or parked, is not",
    "a header: the Vault's value in its placeholder's place at its host, and an answer that echoes it names it as its secret, never the value",
    "a query parameter: the Vault's value in its place",
    "a Vault secret sent to a host not its own is refused, and reaches no provider",
    "a secret bound to another agent, or parked, is refused this agent's placeholder, and reaches no provider",
    "a Nostr key signs: the request reaches its host with a NIP-98 event over its URL, method and body, by that key, and never the key",
    "a request the Vault signs that carries another placeholder is refused, and reaches no provider",
    "removed from the Vault, a secret's next use is refused, and reaches no provider",
    "no value the Vault holds is in the guest's view, in any answer it was given, or in a node log",
    "an answer that echoes a Vault's value as an event stream, or among bytes that are no text, names it as its secret, never the value",
    "basic auth: a short password's header echoed back is named as its secret, never its base64",
    "a signed request goes with the query its signature covers, byte for byte, an empty piece and all",
    "a secret named as a connection the deployment offers (named before it offered it) stays its person's: in the view as a key, its placeholder filled toward its host",
    "one request carrying that connection's placeholder and the secret's, at the connection's host, each where its row takes it: each is filled with its own (Pipes' token in the header, the Vault's value in the query), never the other's",
    "a Nostr key kept parked, then pointed at its host by its person with no value, is in the agent's view and signs there by that key",
];

/// A Vault secret bound while its computer sleeps, to a host none of its
/// starts caught before (a local node keeps a container's intercepts from
/// one start to the next), filled there after its wake: its start asked
/// the Vault beside the container's start and caught the host
/// (`ComputerCell::try_start`, `arm_more`).
pub(super) const VAULT_AFTER_WAKE: &str = "a Vault secret bound while it slept is filled at its host after its wake: its start asked the Vault beside the container's start and caught the host";
const WAKE_VAULT_HOST: &str = "api.vault-wake.test";
const WAKE_VAULT_SECRET: &str = "e2e-vault-wake";

/// `VAULT_AFTER_WAKE`'s secret, bound while its computer sleeps: its value.
pub(super) fn vault_while_asleep(api: &Api, w: &Swapping) -> Result<String> {
    let value = format!("vault-wake-{}", &fragment_nip98::Keys::generate().pubkey_hex()[..24]);
    vault_put(api, w, WAKE_VAULT_SECRET, json!({ "value": value, "hosts": [WAKE_VAULT_HOST], "agents": [w.agent] }))?;
    Ok(value)
}

/// `VAULT_AFTER_WAKE`, awake again: the agent's request with the secret's
/// placeholder, filled at its host. Answers the turns it took.
pub(super) fn vault_after_wake(s: &mut Suite, w: &Swapping, fetched: &dyn Fn(&Suite, u32, &str) -> Result<String>, value: &str) -> Result<usize> {
    let before = s.upstream.seen().len();
    let r = fetched(s, 621, &format!("fetch http://{WAKE_VAULT_HOST}/v1/me with {}", placeholder(s, w.id, w.agent, WAKE_VAULT_SECRET)))?;
    let seen = last_seen(s, before);
    s.ok(VAULT_AFTER_WAKE, r.starts_with("fetched 200") && seen["host"] == WAKE_VAULT_HOST && seen["auth"]["authorization"] == format!("Bearer {value}") && !r.contains(value), json!({ "said": r, "seen": seen }));
    Ok(1)
}

/// A Vault's secret, as its person puts it (`PUT /api/vault/{name}`).
fn vault_put(api: &Api, w: &Swapping, name: &str, body: Value) -> Result<crate::api::Reply> {
    let r = api.signed(w.owner, "PUT", &format!("/api/vault/{name}"), Some(&body))?;
    anyhow::ensure!(r.status == 200, "put {name}: {r}");
    Ok(r)
}

/// A person's Vault through the swap (FIN-466), as the stub's scripted
/// agent sends each request: its secrets bound per agent, filled toward
/// their hosts alone, a Nostr key signing, echoes named, removal at once.
/// Answers how many turns of the agent's they took.
pub(super) fn vault_checks(s: &mut Suite, api: &Api, w: &Swapping, fetched: &dyn Fn(&Suite, u32, &str) -> Result<String>) -> Result<usize> {
    let mut turns = 0;
    let mut said: Vec<String> = vec![];
    let mut say = |s: &Suite, n: u32, text: &str, said: &mut Vec<String>| -> Result<String> {
        turns += 1;
        let r = fetched(s, n, text)?;
        said.push(r.clone());
        Ok(r)
    };
    let value = format!("vault-e2e-{}", &fragment_nip98::Keys::generate().pubkey_hex()[..24]);
    let other = format!("vault-other-{}", &fragment_nip98::Keys::generate().pubkey_hex()[..24]);
    let nostr = fragment_nip98::Keys::generate();
    let (host, other_host) = ("api.vault.test", "api.vault-other.test");
    vault_put(api, w, "e2e-vault", json!({ "value": value, "hosts": [host], "agents": [w.agent] }))?;
    vault_put(api, w, "e2e-vault-query", json!({ "value": value, "hosts": [host], "place": { "query": "key" }, "agents": [w.agent] }))?;
    vault_put(api, w, "e2e-vault-other", json!({ "value": other, "hosts": [other_host], "agents": ["someone--k3x9"] }))?;
    vault_put(api, w, "e2e-vault-parked", json!({ "value": other, "hosts": [], "agents": [w.agent] }))?;
    vault_put(api, w, "e2e-nostr", json!({ "value": nostr.secret_hex(), "hosts": [host], "place": { "sign": "nip98" }, "agents": [w.agent] }))?;
    vault_put(api, w, "e2e-vault-basic", json!({ "value": "pw12", "hosts": [host], "place": { "basic": "password" }, "agents": [w.agent] }))?;
    // a key handed off parked (FIN-463 F11), pointed at its host below
    let handed = fragment_nip98::Keys::generate();
    vault_put(api, w, "e2e-nostr-handed", json!({ "value": handed.secret_hex(), "hosts": [], "place": { "sign": "nip98" }, "agents": [w.agent] }))?;
    let names = ["e2e-vault", "e2e-vault-query", "e2e-vault-other", "e2e-vault-parked", "e2e-nostr", "e2e-vault-basic", "e2e-nostr-handed"];
    let made: std::collections::BTreeMap<&str, String> = names.iter().map(|n| (*n, placeholder(s, w.id, w.agent, n))).collect();
    let ph = |name: &str| made[name].clone();

    let given = say(s, 601, "credentials", &mut said)?;
    s.ok(
        VAULT_CHECKS[0],
        given.contains(&ph("e2e-vault")) && given.contains(&ph("e2e-vault-query")) && given.contains(&ph("e2e-nostr")) && !given.contains("e2e-vault-other") && !given.contains("e2e-vault-parked") && !given.contains("e2e-nostr-handed"),
        &given,
    );

    let before = s.upstream.seen().len();
    let r = say(s, 602, &format!("fetch http://{host}/v1/me with {}", ph("e2e-vault")), &mut said)?;
    let seen = last_seen(s, before);
    s.ok(
        VAULT_CHECKS[1],
        r.starts_with("fetched 200") && seen["host"] == host && seen["auth"]["authorization"] == format!("Bearer {value}") && r.contains("Bearer {{e2e-vault}}") && !r.contains(&value),
        json!({ "said": r, "seen": seen }),
    );
    let before = s.upstream.seen().len();
    let r = say(s, 603, &format!("fetch http://{host}/v1/search?q=a with {} in query key", ph("e2e-vault-query")), &mut said)?;
    let seen = last_seen(s, before);
    s.ok(VAULT_CHECKS[2], r.starts_with("fetched 200") && seen["query"].as_array().is_some_and(|q| q.contains(&json!(["key", value])) && q.contains(&json!(["q", "a"]))), json!({ "said": r, "seen": seen }));

    let before = s.upstream.seen().len();
    let r = say(s, 604, &format!("fetch http://{SWAP_CONNECTION_HOST}/drive/v3/files with {}", ph("e2e-vault")), &mut said)?;
    s.ok(VAULT_CHECKS[3], r.starts_with("fetched 403") && r.contains("goes to api.vault.test") && s.upstream.seen().len() == before, &r);

    let before = s.upstream.seen().len();
    let a = say(s, 605, &format!("fetch http://{host}/v1/me with {}", ph("e2e-vault-other")), &mut said)?;
    let b = say(s, 606, &format!("fetch http://{host}/v1/me with {}", ph("e2e-vault-parked")), &mut said)?;
    s.ok(
        VAULT_CHECKS[4],
        a.starts_with("fetched 403") && a.contains("no secret of") && b.starts_with("fetched 403") && b.contains("parked") && s.upstream.seen().len() == before,
        json!({ "other": a, "parked": b }),
    );

    let before = s.upstream.seen().len();
    let posted = r#"{"redirect":"site"}"#;
    let r = say(s, 607, &format!("fetch http://{host}/v1/sites/redirect with {} posting {posted}", ph("e2e-nostr")), &mut said)?;
    let seen = last_seen(s, before);
    let header = seen["auth"]["authorization"].as_str().unwrap_or_default().to_string();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64;
    let signer = fragment_nip98::verify(Some(&header), "POST", &format!("https://{host}/v1/sites/redirect"), posted.as_bytes(), now, 120);
    s.ok(
        VAULT_CHECKS[5],
        r.starts_with("fetched 200") && signer.as_deref() == Ok(nostr.pubkey_hex()) && seen["body"] == posted && !r.contains(&nostr.secret_hex()) && !header.contains(&nostr.secret_hex()),
        json!({ "said": r, "seen": seen, "signer": format!("{signer:?}") }),
    );
    let before = s.upstream.seen().len();
    let r = say(s, 608, &format!("fetch http://{host}/v1/me?key={} with {}", ph("e2e-vault-query"), ph("e2e-nostr")), &mut said)?;
    s.ok(VAULT_CHECKS[6], r.starts_with("fetched 400") && r.contains("no other placeholder") && s.upstream.seen().len() == before, &r);

    let stream = say(s, 610, &format!("fetch http://{host}/echo-stream with {}", ph("e2e-vault")), &mut said)?;
    let binary = say(s, 611, &format!("fetch http://{host}/echo-binary with {}", ph("e2e-vault")), &mut said)?;
    s.ok(
        VAULT_CHECKS[9],
        stream.starts_with("fetched 200") && stream.contains("Bearer {{e2e-vault}}") && binary.starts_with("fetched 200") && binary.contains("Bearer {{e2e-vault}}") && !stream.contains(&value) && !binary.contains(&value),
        json!({ "stream": stream, "binary": binary }),
    );
    let before = s.upstream.seen().len();
    let r = say(s, 612, &format!("fetch http://{host}/v1/me with {} as basic user", ph("e2e-vault-basic")), &mut said)?;
    let seen = last_seen(s, before);
    let sent = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode("user:pw12"));
    s.ok(VAULT_CHECKS[10], r.starts_with("fetched 200") && seen["auth"]["authorization"] == sent && r.contains("Basic {{e2e-vault-basic}}") && !r.contains(&sent[6..]), json!({ "said": r, "seen": seen }));
    let before = s.upstream.seen().len();
    let target = "/v1/sites/redirect?a=1&&b=2";
    let r = say(s, 613, &format!("fetch http://{host}{target} with {}", ph("e2e-nostr")), &mut said)?;
    let seen = last_seen(s, before);
    let header = seen["auth"]["authorization"].as_str().unwrap_or_default().to_string();
    let signer = fragment_nip98::verify(Some(&header), "GET", &format!("https://{host}{target}"), b"", now, 120);
    s.ok(VAULT_CHECKS[11], r.starts_with("fetched 200") && seen["target"] == target && signer.as_deref() == Ok(nostr.pubkey_hex()), json!({ "said": r, "seen": seen, "signer": format!("{signer:?}") }));

    // a secret named as a connection the catalog offers (`e2e-oauth`): a key
    // (`fck_`), so its own, beside the connection's (`fcx_`)
    let shared = "e2e-oauth";
    vault_put(api, w, shared, json!({ "value": value, "hosts": [host], "agents": [w.agent] }))?;
    let tag = fragment_core::swap::TagKey::derive(&s.host_secret).tag(w.id, w.agent, shared);
    let as_key = fragment_core::swap::Placeholder::new(fragment_core::catalog::Kind::Own, shared, &tag).text();
    let given = say(s, 614, "credentials", &mut said)?;
    let before = s.upstream.seen().len();
    let r = say(s, 615, &format!("fetch http://{host}/v1/me with {as_key}"), &mut said)?;
    let seen = last_seen(s, before);
    s.ok(
        VAULT_CHECKS[12],
        given.contains(&as_key) && r.starts_with("fetched 200") && seen["host"] == host && seen["auth"]["authorization"] == format!("Bearer {value}") && r.contains(&format!("Bearer {{{{{shared}}}}}")),
        json!({ "given": given, "said": r, "seen": seen }),
    );
    // that connection connected, and the secret toward its host in a query
    // parameter: one request carries both, one name and one tag between them
    let email = Api::email_of(w.owner);
    s.workos.connect(&email, shared, true);
    vault_put(api, w, shared, json!({ "value": value, "hosts": ["oauth.test"], "place": { "query": "key" }, "agents": [w.agent] }))?;
    let as_connection = fragment_core::swap::Placeholder::new(fragment_core::catalog::Kind::Connection, shared, &tag).text();
    let listed = api.signed(w.owner, "GET", "/api/connections", None)?;
    let before = s.upstream.seen().len();
    let r = say(s, 616, &format!("fetch http://oauth.test/v1/me?key={as_key} with {as_connection}"), &mut said)?;
    let seen = last_seen(s, before);
    let token = s.workos.tokens(shared).last().cloned().unwrap_or_default();
    s.ok(
        VAULT_CHECKS[13],
        listed.status == 200 && r.starts_with("fetched 200") && !token.is_empty() && seen["auth"]["authorization"] == format!("Bearer {token}") && seen["query"] == json!([["key", value]]),
        json!({ "said": r, "seen": seen }),
    );
    let off = api.signed(w.owner, "DELETE", &format!("/api/connections/{shared}"), None)?;
    anyhow::ensure!(off.status == 200, "the shared name's connection cleared: {off}");

    // the parked key, pointed at its host with no value: it signs there
    let pointed = vault_put(api, w, "e2e-nostr-handed", json!({ "hosts": [host], "place": { "sign": "nip98" }, "agents": [w.agent] }))?;
    let given = say(s, 617, "credentials", &mut said)?;
    let before = s.upstream.seen().len();
    let r = say(s, 618, &format!("fetch http://{host}/v1/sites/redirect with {} posting {posted}", ph("e2e-nostr-handed")), &mut said)?;
    let seen = last_seen(s, before);
    let header = seen["auth"]["authorization"].as_str().unwrap_or_default().to_string();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64;
    let signer = fragment_nip98::verify(Some(&header), "POST", &format!("https://{host}/v1/sites/redirect"), posted.as_bytes(), now, 120);
    s.ok(
        VAULT_CHECKS[14],
        pointed.body["hosts"] == json!([host]) && given.contains(&ph("e2e-nostr-handed")) && r.starts_with("fetched 200") && signer.as_deref() == Ok(handed.pubkey_hex()) && !r.contains(&handed.secret_hex()) && !header.contains(&handed.secret_hex()),
        json!({ "given": given, "said": r, "seen": seen, "signer": format!("{signer:?}") }),
    );

    let gone = api.signed(w.owner, "DELETE", "/api/vault/e2e-vault", None)?;
    let before = s.upstream.seen().len();
    let r = say(s, 609, &format!("fetch http://{host}/v1/me with {}", ph("e2e-vault")), &mut said)?;
    s.ok(VAULT_CHECKS[7], gone.body["removed"] == true && r.starts_with("fetched 403") && s.upstream.seen().len() == before, json!({ "removed": gone.body, "said": r }));

    let mut logs = String::new();
    for entry in std::fs::read_dir(&s.scratch).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("node-") && name.ends_with(".log") {
            logs.push_str(&std::fs::read_to_string(entry.path()).unwrap_or_default());
        }
    }
    let secrets = [value.as_str(), other.as_str(), &nostr.secret_hex(), &handed.secret_hex()];
    let leaked: Vec<&str> = secrets.iter().copied().filter(|v| said.iter().any(|t| t.contains(v)) || logs.contains(v)).collect();
    s.ok(VAULT_CHECKS[8], !logs.is_empty() && leaked.is_empty(), json!({ "answers": said.len(), "log bytes": logs.len(), "leaked": leaked.len() }));
    for name in ["e2e-vault-query", "e2e-vault-other", "e2e-vault-parked", "e2e-nostr", "e2e-vault-basic", "e2e-oauth", "e2e-nostr-handed"] {
        api.signed(w.owner, "DELETE", &format!("/api/vault/{name}"), None)?;
    }
    Ok(turns)
}

/// An own key a computer kept before the move (M16), and one set since:
/// what `own_move_checks` and `own_signin_moved_checks` use. None is ever
/// in a log.
const OWN_BEFORE: &str = "mail-own-e2e-before-7c41";
const OWN_ROUTER_BEFORE: &str = "sk-or-v1-e2e-before-3d9a";
const OWN_SINCE: &str = "mail-own-e2e-since-b20e";
/// The calls through an own key `own_move_checks` makes that its provider
/// answers: counted as the agent's before `swap_checks` counts its own.
const OWN_MOVE_CALLS: i32 = 2;

/// A person's own keys moved from their computer to their Vault (M16):
/// the checks `own_move_checks` makes, as a run without the stub or the
/// fakes skips them. They run before `swap_checks` sets an own key, so the
/// person has said nothing of one since the move.
pub(super) const OWN_MOVE_CHECKS: &[&str] = &[
    "an own key its computer kept from before the move stays owed through its guest's own read of its view, which gives it to the guest (no hand-over from a guest's request), then goes to its person's Vault at its agent's next call, never lost: that call goes with the key, the computer holds it no more, and its person's list says it is set",
    "one its owner's view reads first goes over then: its agent may run that provider's models",
    "what its person said since stands: a key set after the move is never replaced by the one its computer hands over late (a hand-over again), nor one removed put back, the agent's call then refused, reaching no provider",
];

/// The checks; answers how many turns of the agent's they took. The
/// computer's own keys are written as the release before the move kept
/// them (`legacy-own-key`); each is left removed.
pub(super) fn own_move_checks(s: &mut Suite, api: &Api, w: &Swapping, fetched: &dyn Fn(&Suite, u32, &str) -> Result<String>) -> Result<usize> {
    let mut turns = 0;
    let mut say = |s: &Suite, n: u32, text: &str| -> Result<String> {
        turns += 1;
        fetched(s, n, text)
    };
    // which own keys the computer still holds, `key` kept first
    let kept = |provider: &str, key: Option<&str>| -> Value {
        let r = api.unsigned("POST", "/api/test/computer", Some(&json!({ "computer": w.id, "op": "legacy-own-key", "provider": provider, "key": key })));
        r.map(|r| r.body["held"].clone()).unwrap_or(Value::Null)
    };
    let state = |provider: &str| -> Value {
        let r = api.signed(w.owner, "GET", "/api/connections", None).ok();
        r.and_then(|r| r.body["providers"].as_array().and_then(|l| l.iter().find(|x| x["provider"] == provider)).map(|x| x["state"].clone())).unwrap_or(Value::Null)
    };
    let mail = |s: &Suite| format!("fetch http://{SWAP_OWN_HOST}/v3/send with {} as basic api", placeholder(s, w.id, w.agent, SWAP_OWN));
    let basic = |key: &str| format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("api:{key}")));
    let sent_with = |s: &Suite, before: usize, key: &str| {
        let seen = last_seen(s, before);
        seen["host"] == SWAP_OWN_HOST && seen["auth"]["authorization"] == basic(key)
    };

    // kept as before the move: the next touch hands it over, and the
    // agent's call goes with it
    let wrote = kept(SWAP_OWN, Some(OWN_BEFORE));
    let given = say(s, 214, "credentials")?;
    let owed = kept(SWAP_OWN, None);
    let seen = s.upstream.seen().len();
    let said = say(s, 210, &mail(s))?;
    let went = sent_with(s, seen, OWN_BEFORE);
    let held = kept(SWAP_OWN, None);
    let listed = state(SWAP_OWN);
    let listed_to_guest = given.contains(&format!("{SWAP_OWN} {SWAP_OWN_ENV} fck_{SWAP_OWN}_"));
    s.ok(
        OWN_MOVE_CHECKS[0],
        wrote == json!([SWAP_OWN]) && listed_to_guest && owed == json!([SWAP_OWN]) && said.starts_with("fetched 200") && went && held == json!([]) && listed == "set",
        json!({ "wrote": wrote, "listedToGuest": listed_to_guest, "owedAfterGuestRead": owed, "said": said, "went": went, "held": held, "listed": listed }),
    );

    // read first by its owner's view: which models its agent may run
    let agent_of = |r: &crate::api::Reply| r.body["agents"].as_array().and_then(|l| l.iter().find(|a| a["fragment"] == w.agent)).cloned().unwrap_or(Value::Null);
    let wrote = kept(SWAP_ROUTER, Some(OWN_ROUTER_BEFORE));
    let view = api.signed(w.owner, "GET", &format!("/api/computers/{}", w.id), None)?;
    let held = kept(SWAP_ROUTER, None);
    let listed = state(SWAP_ROUTER);
    let gone = api.signed(w.owner, "DELETE", &format!("/api/connections/{SWAP_ROUTER}/key"), None)?;
    s.ok(
        OWN_MOVE_CHECKS[1],
        wrote == json!([SWAP_ROUTER]) && agent_of(&view)["modelProviders"] == json!([SWAP_ROUTER]) && held == json!([]) && listed == "set" && gone.status == 200 && state(SWAP_ROUTER) == "not_set",
        json!({ "wrote": wrote, "models": agent_of(&view)["modelProviders"], "held": held, "listed": listed, "gone": gone.status }),
    );

    // set since, then removed since: a hand-over again, late, changes neither
    let set = api.signed(w.owner, "PUT", &format!("/api/connections/{SWAP_OWN}/key"), Some(&json!({ "key": OWN_SINCE })))?;
    let late = kept(SWAP_OWN, Some(OWN_BEFORE));
    let seen = s.upstream.seen().len();
    let said = say(s, 211, &mail(s))?;
    let went = sent_with(s, seen, OWN_SINCE);
    let held = kept(SWAP_OWN, None);
    let removed = api.signed(w.owner, "DELETE", &format!("/api/connections/{SWAP_OWN}/key"), None)?;
    let late_again = kept(SWAP_OWN, Some(OWN_BEFORE));
    let seen = s.upstream.seen().len();
    let refused = say(s, 212, &mail(s))?;
    let unreached = s.upstream.seen().len() == seen;
    let held_again = kept(SWAP_OWN, None);
    let listed = state(SWAP_OWN);
    s.ok(
        OWN_MOVE_CHECKS[2],
        set.status == 200
            && late == json!([SWAP_OWN])
            && said.starts_with("fetched 200")
            && went
            && held == json!([])
            && removed.status == 200
            && late_again == json!([SWAP_OWN])
            && refused.starts_with("fetched 403")
            && refused.contains(&format!("give your own {SWAP_OWN} key first"))
            && unreached
            && held_again == json!([])
            && listed == "not_set",
        json!({ "set": set.status, "said": said, "went": went, "held": [held, held_again], "removed": removed.status, "refused": refused, "listed": listed }),
    );
    Ok(turns)
}

/// An own key's sign-in begun before the move (M16), and the log: the
/// checks `own_signin_moved_checks` makes, as a run without the stub or the
/// fakes skips them.
pub(super) const OWN_SIGNIN_MOVED_CHECKS: &[&str] = &[
    "an own key's sign-in begun on its computer before the move finishes after it, in the browser that began it: the key the provider made is its person's, given to the guest as a placeholder and never itself, and the sign-in comes back once",
    "no own key, kept from before the move, set, or made by a sign-in, is in the node's log",
    "each swap through an own key logs how long it waited on the agent's fragment (grantsMs) and on its person's Vault (ownMs), each from one start, and never a key",
];

/// The checks; answers how many turns of the agent's they took. Run after
/// `model_checks`, the provider's key left removed.
pub(super) fn own_signin_moved_checks(s: &mut Suite, api: &Api, w: &Swapping, fetched: &dyn Fn(&Suite, u32, &str) -> Result<String>) -> Result<usize> {
    let mut turns = 0;
    let mut say = |s: &Suite, n: u32, text: &str| -> Result<String> {
        turns += 1;
        fetched(s, n, text)
    };
    let session = api.sign_in(&Api::email_of(w.owner))?;
    let begun = api.unsigned("POST", "/api/test/computer", Some(&json!({ "computer": w.id, "op": "legacy-sign-in", "provider": SWAP_ROUTER })))?;
    let nonce = begun.body["nonce"].as_str().unwrap_or_default().to_string();
    let back = sent_back(api, begun.body["url"].as_str().unwrap_or_default())?;
    let (bound, signed_in) = (format!("fragment_own_key={nonce}"), format!("fragment_session={session}"));
    let exchanges = s.openrouter.exchanges().len();
    let page = came_back(api, &back, &[&bound, &signed_in])?;
    let key = s.openrouter.keys().last().cloned().unwrap_or_default();
    let listed = api.signed(w.owner, "GET", "/api/connections", None)?;
    let set = listed.body["providers"].as_array().and_then(|l| l.iter().find(|x| x["provider"] == SWAP_ROUTER)).map(|x| x["state"].clone()).unwrap_or(Value::Null);
    let given = say(s, 213, "credentials")?;
    let again = came_back(api, &back, &[&bound, &signed_in])?;
    let exchanged = s.openrouter.exchanges().len() - exchanges;
    s.ok(
        OWN_SIGNIN_MOVED_CHECKS[0],
        begun.status == 200
            && back.contains("code=")
            && page.status == 200
            && page.text.contains("is connected")
            && set == "set"
            && given.contains(&format!("{SWAP_ROUTER} {SWAP_ROUTER_ENV} fck_{SWAP_ROUTER}_"))
            && !key.is_empty()
            && !given.contains(&key)
            && !page.text.contains(&key)
            && !listed.text.contains(&key)
            && again.refused_cleanly()
            && exchanged == 1,
        json!({ "begun": begun.status, "page": page.to_string(), "state": set, "again": again.to_string(), "exchanged": exchanged }),
    );
    let gone = api.signed(w.owner, "DELETE", &format!("/api/connections/{SWAP_ROUTER}/key"), None)?;
    anyhow::ensure!(gone.status == 200, "the sign-in's key removed again: {gone}");

    // every own key this lane gave, kept or made
    let log = s.node_log().unwrap_or_default();
    let mut keys: Vec<String> = [OWN_BEFORE, OWN_ROUTER_BEFORE, OWN_SINCE, SWAP_OWN_KEY, SWAP_OWN_BARE].iter().map(|k| k.to_string()).collect();
    keys.extend(s.openrouter.keys());
    let named: Vec<usize> = keys.iter().enumerate().filter(|(_, k)| log.contains(k.as_str())).map(|(i, _)| i).collect();
    s.ok(OWN_SIGNIN_MOVED_CHECKS[1], !log.is_empty() && keys.len() >= 6 && named.is_empty(), json!({ "logBytes": log.len(), "keys": keys.len(), "named": named }));
    // the swap's own line: both waits, for a call through an own key
    let own_swap = format!("fck_{SWAP_OWN}_…");
    let timed: Vec<Value> = crate::logged_json(&log)
        .into_iter()
        .filter(|v| v["egress"] == "swap" && v["credentials"].as_array().is_some_and(|c| c.iter().any(|n| n == own_swap.as_str())) && v["status"] == 200)
        .collect();
    let both = timed.iter().filter(|v| v["grantsMs"].as_i64().is_some_and(|ms| ms >= 0) && v["ownMs"].as_i64().is_some_and(|ms| ms >= 0)).count();
    s.ok(OWN_SIGNIN_MOVED_CHECKS[2], !timed.is_empty() && both == timed.len(), json!({ "lines": timed.len(), "both": both, "first": timed.first() }));
    Ok(turns)
}

/// What an agent may use of its owner's is its agent fragment's
/// (`fragment_core::grants`): the checks `grant_checks` makes, as a run
/// without the stub or the fakes skips them.
pub(super) const GRANT_CHECKS: &[&str] = &[
    "what an agent may use is its owner's alone to change: an agent's own key is refused (403), as is one acting for its owner (400), and its fragment's own route for it answers no one from outside (404); the agent may use what it did",
    "what its owner sets is its agent fragment's, which says so on its events; its computer owes none",
    "a provider taken away is refused at the agent's very next request, reaching no provider; given back, its next request goes",
    "a computer's row from before the move, still owed (narrowed, an account picked), is honoured at the agent's next request by a fragment that holds none: taken whole, and owed no more",
    "taken once: a row its computer sends later (every provider) is ignored by a fragment holding grants of its own, the agent's next request refused, reaching no provider, and owed no more",
];

/// The checks; answers how many turns of the agent's they took. Run while
/// the agent may use every provider, its owner's google connected; left so.
pub(super) fn grant_checks(s: &mut Suite, api: &Api, w: &Swapping, fetched: &dyn Fn(&Suite, u32, &str) -> Result<String>) -> Result<usize> {
    let mut turns = 0;
    let mut say = |s: &Suite, n: u32, text: &str| -> Result<String> {
        turns += 1;
        fetched(s, n, text)
    };
    let owner_id = api.identity(w.owner)?;
    let path = format!("/api/computers/{}/agents/{}/connections", w.id, w.agent);
    let allow = |who: &crate::Keys, query: &str, list: Value| api.signed(who, "PUT", &format!("{path}{query}"), Some(&json!({ "connections": list })));
    let agent_of = |r: &crate::api::Reply| r.body["agents"].as_array().and_then(|l| l.iter().find(|a| a["fragment"] == w.agent)).cloned().unwrap_or(Value::Null);
    let view = || api.signed(w.owner, "GET", &format!("/api/computers/{}", w.id), None).map(|r| agent_of(&r));
    // what the computer owes the agent's fragment from before the move (Computer step 6)
    let row = |written: Option<Value>| api.unsigned("POST", "/api/test/computer", Some(&json!({ "computer": w.id, "op": "legacy-grants", "agent": w.agent, "row": written })));
    let told = || events(api, w.owner, w.agent).into_iter().rev().find(|e| e["kind"] == "agent.grants").unwrap_or(Value::Null);
    let google = |s: &Suite| format!("fetch http://{SWAP_CONNECTION_HOST}/drive/v3/files with {}", placeholder(s, w.id, w.agent, SWAP_CONNECTION));

    let before = view()?;
    let (agent_keys, _) = api.own_agent(w.owner)?;
    let by_agent = allow(&agent_keys, "", json!([]))?;
    let for_owner = allow(&agent_keys, &format!("?for={owner_id}"), json!([]))?;
    let inside = json!({ "computer": w.id, "identity": w.identity, "owner": owner_id, "change": { "connections": [] } });
    let inside = api.signed(w.owner, "POST", &format!("/api/f/{}/computer/grants", w.agent), Some(&inside))?;
    let after = view()?;
    s.ok(
        GRANT_CHECKS[0],
        before["connections"].is_null() && by_agent.status == 403 && for_owner.status == 400 && inside.status == 404 && after["connections"].is_null(),
        json!({ "agent": by_agent.status, "for": for_owner.status, "inside": inside.status, "after": after["connections"] }),
    );

    let set = allow(w.owner, "", json!([SWAP_CONNECTION]))?;
    let kept = row(None)?;
    let event = told();
    s.ok(
        GRANT_CHECKS[1],
        agent_of(&set)["connections"] == json!([SWAP_CONNECTION])
            && event["data"]["grants"]["connections"] == json!([SWAP_CONNECTION])
            && event["data"]["moved"] == false
            && kept.body["row"].is_null(),
        json!({ "set": set.status, "event": event, "row": kept.body }),
    );

    let went = say(s, 180, &google(s))?;
    let off = allow(w.owner, "", json!(["elevenlabs"]))?;
    let seen = s.upstream.seen().len();
    let refused = say(s, 181, &google(s))?;
    let unreached = s.upstream.seen().len() == seen;
    let back = allow(w.owner, "", Value::Null)?;
    let again = say(s, 182, &google(s))?;
    s.ok(
        GRANT_CHECKS[2],
        went.starts_with("fetched 200") && off.status == 200 && refused.starts_with("fetched 403") && refused.contains("may not use google") && unreached && back.status == 200 && again.starts_with("fetched 200"),
        json!({ "went": went, "refused": refused, "again": again }),
    );

    // an agent fragment as before the move: it holds none
    let forgot = api.unsigned("POST", "/api/test/fragment", Some(&json!({ "fragment": w.agent, "op": "forget-grants" })))?;
    let picks = json!({ SEVERAL: "data_installation_e2e_old" });
    let old = json!({ "connections": "[\"elevenlabs\"]", "accounts": picks });
    let wrote = row(Some(old.clone()))?;
    let seen = s.upstream.seen().len();
    let refused = say(s, 183, &google(s))?;
    let unreached = s.upstream.seen().len() == seen;
    let kept = row(None)?;
    let shown = view()?;
    let event = told();
    s.ok(
        GRANT_CHECKS[3],
        forgot.status == 200
            && wrote.body["row"] == old
            && refused.starts_with("fetched 403")
            && refused.contains("may not use google")
            && unreached
            && kept.body["row"].is_null()
            && shown["connections"] == json!(["elevenlabs"])
            && shown["accounts"][SEVERAL] == "data_installation_e2e_old"
            && event["data"]["moved"] == true,
        json!({ "wrote": wrote.body, "refused": refused, "row": kept.body, "shown": shown, "event": event }),
    );

    // the row again, as an ask read before the take would carry it
    let wrote = row(Some(json!({ "connections": "null", "accounts": {} })))?;
    let seen = s.upstream.seen().len();
    let refused = say(s, 184, &google(s))?;
    let unreached = s.upstream.seen().len() == seen;
    let kept = row(None)?;
    let shown = view()?;
    s.ok(
        GRANT_CHECKS[4],
        wrote.status == 200
            && refused.starts_with("fetched 403")
            && refused.contains("may not use google")
            && unreached
            && kept.body["row"].is_null()
            && shown["connections"] == json!(["elevenlabs"])
            && shown["accounts"][SEVERAL] == "data_installation_e2e_old",
        json!({ "refused": refused, "row": kept.body, "shown": shown }),
    );

    // as it was: every provider, each connection's default account
    let back = allow(w.owner, "", Value::Null)?;
    let unpicked = api.signed(w.owner, "PUT", &format!("/api/computers/{}/agents/{}/accounts/{SEVERAL}", w.id, w.agent), Some(&json!({ "account": null })))?;
    anyhow::ensure!(back.status == 200 && unpicked.status == 200, "the agent given every provider again, its pick cleared: {back}; {unpicked}");
    Ok(turns)
}

/// The checks `uses_checks` makes: a computer's uses from before the move
/// (M16) handed to its owner's ledger.
pub(super) const USES_CHECKS: &[&str] = &[
    "a computer's months of uses from before the move, owed (an agent gone among them), are handed to its owner's ledger as its owner reads its view, whole, and owed no more",
    "handed over again (as when its answer was lost before the computer forgot them), the ledger adds nothing twice",
    "another body under a month the ledger took is refused, changing nothing there, and the computer owes it no more",
];

/// A month's uses in `owner`'s ledger, as `(provider, agent, calls,
/// micros)`, sorted.
fn ledger_uses(api: &Api, owner: &crate::Keys, month: &str) -> Vec<(String, String, i64, i64)> {
    let r = api.signed(owner, "GET", &format!("/api/ledger/uses/{month}"), None).map(|r| r.body).unwrap_or(Value::Null);
    let mut rows: Vec<(String, String, i64, i64)> = r["uses"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|u| (u["provider"].as_str().unwrap_or("").to_string(), u["agent"].as_str().unwrap_or("").to_string(), u["calls"].as_i64().unwrap_or(-1), u["micros"].as_i64().unwrap_or(-1)))
        .collect();
    rows.sort();
    rows
}

/// A computer's uses from before the move (Computer step 7, M16): months
/// it owes its owner's ledger, made by its `legacy-uses` lever as the step
/// keeps them, the months before the lane's (`month`), whose uses the lane
/// does not count. Taken once each, whole, as its owner reads its view.
pub(super) fn uses_checks(s: &mut Suite, api: &Api, owner: &crate::Keys, id: &str, agent: &str, month: &str) -> Result<()> {
    use fragment_core::ledger::Month;
    let this = Month::parse(month).ok_or_else(|| anyhow::anyhow!("the lane's month {month:?}"))?;
    let (last, before) = (Month(this.0 - 1).label(), Month(this.0 - 2).label());
    let lever = |body: Value| super::computers::lever_with(api, id, body);
    let owed = || lever(json!({ "op": "legacy-uses" })).map(|r| r.body["owed"].clone()).unwrap_or(Value::Null);
    let touch = || api.signed(owner, "GET", &format!("/api/computers/{id}"), None).map(|r| r.status).unwrap_or(0);
    let gone = "gone--z9x1";
    let kept = |a: &str, p: &str, calls: i64, micros: i64| json!({ "agent": a, "provider": p, "calls": calls, "micros": micros });
    let rows_last = json!([kept(agent, "github", 3, 0), kept(agent, "perplexity", 2, 15_000), kept(gone, "perplexity", 1, 7_500)]);
    let rows_before = json!([kept(agent, "elevenlabs", 4, 90_000)]);
    let row = |p: &str, a: &str, c: i64, m: i64| (p.to_string(), a.to_string(), c, m);
    let mut want_last = vec![row("github", agent, 3, 0), row("perplexity", agent, 2, 15_000), row("perplexity", gone, 1, 7_500)];
    want_last.sort();
    let want_before = vec![row("elevenlabs", agent, 4, 90_000)];
    let held = (ledger_uses(api, owner, &last), ledger_uses(api, owner, &before));

    let planted = [lever(json!({ "op": "legacy-uses", "month": last, "row": rows_last }))?, lever(json!({ "op": "legacy-uses", "month": before, "row": rows_before }))?];
    let both = planted[1].body["owed"].as_object().is_some_and(|o| o.len() == 2);
    let touched = touch();
    let taken = s.eventually(Duration::from_secs(20), || owed() == json!({}));
    let got = (ledger_uses(api, owner, &last), ledger_uses(api, owner, &before));
    s.ok(
        USES_CHECKS[0],
        held == (vec![], vec![]) && both && touched == 200 && taken && got == (want_last.clone(), want_before.clone()),
        json!({ "held": held, "planted": planted[1].body, "taken": taken, "got": got }),
    );

    // its answer lost: the computer still owes what the ledger took
    let again = lever(json!({ "op": "legacy-uses", "month": last, "row": rows_last }))?;
    let touched = touch();
    let taken = s.eventually(Duration::from_secs(20), || owed() == json!({}));
    let after = ledger_uses(api, owner, &last);
    s.ok(USES_CHECKS[1], again.status == 200 && touched == 200 && taken && after == want_last, json!({ "taken": taken, "after": after }));

    let other = lever(json!({ "op": "legacy-uses", "month": last, "row": [kept(agent, "github", 99, 0)] }))?;
    let touched = touch();
    let taken = s.eventually(Duration::from_secs(20), || owed() == json!({}));
    let after = ledger_uses(api, owner, &last);
    s.ok(USES_CHECKS[2], other.status == 200 && touched == 200 && taken && after == want_last, json!({ "taken": taken, "after": after }));
    Ok(())
}

/// The checks `vault_ask_checks` makes (FIN-466, an agent asks), as a run
/// without the stub or the fakes skips them.
pub(super) const VAULT_ASK_CHECKS: &[&str] = &[
    "an agent's `fragment vault ask` is a card on its person's notices: who asks for what, why, where it goes, a masked field, Save and Decline, and its deep link serves the shell; a client that draws no field is sent Open (that link) and Decline in their place",
    "no one else is shown it or answers it: another person's notices hold none, and their answer finds no ask",
    "an empty answer is refused and leaves it waiting",
    "its person's paste saves it: the agent's CLI is told so, with its stand-in, never the value; the card is gone",
    "the stand-in works toward the host asked: the value in its place, and the agent shown only its name",
    "an ask that ended is not answered again (409), and its decline answers it as it stands",
    "a Decline ends an ask: the agent's CLI is told, and nothing is put in the Vault",
    "asked for a name its person keeps already toward a new host, the card offers Allow and Decline (no field): overwrite is refused, stale revision is 409, and Allow keeps the value while adding the new host",
    "no value pasted is in any answer, in the agent's view, or in a node log",
    "a secret named `asks` is still its person's to set and remove: the asks' routes take only a POST or an id",
    "a card answered after a secret of its name was set meanwhile grants nothing: 409, and the secret stays as set",
    "`fragment vault ask --no-wait` returns immediately with a deep link URL and `fragment vault wait <id>` receives the stand-in once answered",
    "an ask that would take its person's Vault past 8 hosts is refused as the agent asks, in the cap's words, and no card is drawn: no one is asked to paste what Save would refuse",
];

/// The person's waiting card for `name` (a `vault_ask` notice), once
/// their notices show it, as the shell's band reads it (`field=1`).
fn ask_card(s: &Suite, api: &Api, owner: &crate::Keys, name: &str) -> Result<Value> {
    let card = || -> Option<Value> {
        let r = api.signed(owner, "GET", "/api/notices?field=1", None).ok()?;
        r.body["notices"].as_array()?.iter().find(|n| n["kind"] == "vault_ask" && n["title"].as_str().is_some_and(|t| t.ends_with(&format!(" {name}")))).cloned()
    };
    s.eventually(Duration::from_secs(60), || card().is_some());
    Ok(card().unwrap_or(Value::Null))
}

/// A person's Vault filled by an agent's ask (FIN-466): the stub's agent
/// runs `fragment vault ask` (its turn waits on the card), its person
/// answers the card from their notices as the shell's band posts it, and
/// the stand-in it is given then works through the swap. `start` posts a
/// message and answers its turn; `replied` waits for that turn's reply.
pub(super) fn vault_ask_checks(
    s: &mut Suite,
    api: &Api,
    w: &Swapping,
    start: &dyn Fn(u32, &str) -> Result<String>,
    replied: &dyn Fn(&Suite, &str) -> Result<String>,
    fetched: &dyn Fn(&Suite, u32, &str) -> Result<String>,
) -> Result<usize> {
    let value = format!("asked-e2e-{}", &fragment_nip98::Keys::generate().pubkey_hex()[..24]);
    let (name, host, host2) = ("e2e-asked", "api.vault.test", "uploads.vault.test");
    let mut said: Vec<String> = vec![];

    let turn = start(701, &format!("vault-ask {name} {host} Push the shop code"))?;
    let card = ask_card(s, api, w.owner, name)?;
    let actions = card["actions"].as_array().cloned().unwrap_or_default();
    let save = actions.first().and_then(|a| a["post"].as_str()).unwrap_or("").to_string();
    let ask_id = card["id"].as_str().unwrap_or("").trim_start_matches("vault_ask:").to_string();
    let deep_page = api.unsigned("GET", &format!("/vault/ask/{ask_id}"), None)?;
    // a client that draws no field (iOS) is sent the card's own page in its place
    let plain = api.signed(w.owner, "GET", "/api/notices", None)?;
    let linked = plain.body["notices"].as_array().and_then(|l| l.iter().find(|n| n["id"] == card["id"]).cloned()).unwrap_or_default();
    let linked_ok = linked["field"].is_null()
        && linked["title"] == card["title"]
        && linked["actions"].as_array().is_some_and(|a| a.len() == 2 && a[0]["label"] == "Open" && a[0]["open"].as_str().is_some_and(|u| u.ends_with(&format!("/vault/ask/{ask_id}"))) && a[0]["post"].is_null() && a[1]["label"] == "Decline");
    let bad_deep_page = api.unsigned("GET", "/vault/ask/not-a-valid-ask-id", None)?;
    s.ok(
        VAULT_ASK_CHECKS[0],
        card["text"] == "Push the shop code"
            && card["detail"].as_str().is_some_and(|d| d.contains(&format!("Only {host} would receive it")) && d.contains("never the secret itself"))
            && card["field"]["placeholder"].is_string()
            && actions.len() == 2
            && actions[0]["label"] == "Save"
            && actions[0]["field"] == true
            && actions[0]["body"] == json!({ "held": "none" })
            && actions[1]["label"] == "Decline"
            && actions[1]["field"].is_null()
            && save.starts_with("/api/vault/asks/")
            && deep_page.status == 200
            && deep_page.text.contains("fragment-release")
            && bad_deep_page.status == 404
            && linked_ok,
        json!({ "card": card, "linked": linked, "deep": deep_page.status, "bad": bad_deep_page.status }),
    );
    let theirs = api.signed(w.stranger, "GET", "/api/notices", None)?;
    let stranger_answer = api.signed(w.stranger, "POST", &save, Some(&json!({ "held": "none", "value": value })))?;
    s.ok(
        VAULT_ASK_CHECKS[1],
        theirs.status == 200 && !theirs.text.contains(name) && stranger_answer.status == 404,
        json!({ "notices": theirs.body, "answer": stranger_answer.status }),
    );
    let empty = api.signed(w.owner, "POST", &save, Some(&json!({ "held": "none", "value": "  " })))?;
    let still = ask_card(s, api, w.owner, name)?;
    s.ok(VAULT_ASK_CHECKS[2], empty.status == 400 && still["id"] == card["id"], json!({ "empty": empty.body, "still": still }));

    let saved = api.signed(w.owner, "POST", &save, Some(&json!({ "held": "none", "value": value })))?;
    let reply = replied(s, &turn)?;
    said.push(reply.clone());
    let stand_in = placeholder(s, w.id, w.agent, name);
    let gone = api.signed(w.owner, "GET", "/api/notices", None)?;
    s.ok(
        VAULT_ASK_CHECKS[3],
        saved.status == 200
            && saved.body["state"] == "saved"
            && saved.body["url"].as_str().is_some_and(|u| u.ends_with(&format!("/vault/ask/{ask_id}")))
            && !saved.text.contains(&value)
            && reply == format!("vault-asked saved {stand_in}")
            && !gone.text.contains(card["id"].as_str().unwrap_or("?")),
        json!({ "saved": saved.body, "reply": reply, "expected": stand_in }),
    );
    let before = s.upstream.seen().len();
    let r = fetched(s, 702, &format!("fetch http://{host}/v1/me with {stand_in}"))?;
    said.push(r.clone());
    let seen = last_seen(s, before);
    s.ok(
        VAULT_ASK_CHECKS[4],
        r.starts_with("fetched 200") && seen["host"] == host && seen["auth"]["authorization"] == format!("Bearer {value}") && !r.contains(&value),
        json!({ "said": r, "seen": seen }),
    );
    let again = api.signed(w.owner, "POST", &save, Some(&json!({ "held": "none", "value": value })))?;
    let declined_after = api.signed(w.owner, "POST", &format!("{save}/decline"), Some(&json!({})))?;
    s.ok(VAULT_ASK_CHECKS[5], again.status == 409 && declined_after.status == 200 && declined_after.body["state"] == "saved", json!({ "again": again.body, "decline": declined_after.body }));

    let turn = start(703, "vault-ask e2e-declined - Read the calendar")?;
    let card = ask_card(s, api, w.owner, "e2e-declined")?;
    let made = api.signed(w.owner, "PUT", "/api/vault/e2e-declined", Some(&json!({ "value": "set-meanwhile", "hosts": [host], "agents": [w.agent] })))?;
    let late = api.signed(w.owner, "POST", card["actions"][0]["post"].as_str().unwrap_or(""), Some(&json!({ "held": "none", "value": "pasted-late" })))?;
    let kept = api.signed(w.owner, "GET", "/api/vault", None)?;
    let gone = api.signed(w.owner, "DELETE", "/api/vault/e2e-declined", None)?;
    let as_set = kept.body["secrets"].as_array().and_then(|l| l.iter().find(|x| x["name"] == "e2e-declined").cloned()).unwrap_or_default();
    s.ok(
        VAULT_ASK_CHECKS[10],
        made.status == 200 && late.status == 409 && as_set["agents"] == json!([w.agent]) && as_set["setAt"] == made.body["setAt"] && gone.status == 200,
        json!({ "made": made.body, "late": late.body, "kept": as_set }),
    );
    let decline = card["actions"][1]["post"].as_str().unwrap_or("").to_string();
    let d = api.signed(w.owner, "POST", &decline, Some(&json!({})))?;
    let reply = replied(s, &turn)?;
    said.push(reply.clone());
    let list = api.signed(w.owner, "GET", "/api/vault", None)?;
    s.ok(VAULT_ASK_CHECKS[6], d.status == 200 && d.body["state"] == "declined" && reply == "vault-asked declined none" && !list.text.contains("e2e-declined"), json!({ "decline": d.body, "reply": reply, "vault": list.body }));

    // Existing secret: Allow card widens hosts while keeping the sealed value untouched
    let before_list = api.signed(w.owner, "GET", "/api/vault", None)?;
    let set_at_before = before_list.body["secrets"].as_array().and_then(|l| l.iter().find(|x| x["name"] == name)).and_then(|x| x["setAt"].as_i64());
    let turn = start(704, &format!("vault-ask {name} {host2} Upload release assets"))?;
    let card = ask_card(s, api, w.owner, name)?;
    let actions = card["actions"].as_array().cloned().unwrap_or_default();
    let allow_post = actions.first().and_then(|a| a["post"].as_str()).unwrap_or("").to_string();
    let allow_body = actions.first().map(|a| a["body"].clone()).unwrap_or(Value::Null);
    let overwrite = api.signed(w.owner, "POST", &allow_post, Some(&json!({ "held": allow_body["held"], "value": "pasted-over" })))?;
    let stale = api.signed(w.owner, "POST", &allow_post, Some(&json!({ "held": "stale-rev", "allow": true })))?;
    let allowed = api.signed(w.owner, "POST", &allow_post, Some(&allow_body))?;
    let reply = replied(s, &turn)?;
    said.push(reply.clone());
    let list = api.signed(w.owner, "GET", "/api/vault", None)?;
    let kept = list.body["secrets"].as_array().and_then(|l| l.iter().find(|x| x["name"] == name).cloned()).unwrap_or_default();
    let before2 = s.upstream.seen().len();
    let r2 = fetched(s, 705, &format!("fetch http://{host2}/v1/upload with {stand_in}"))?;
    said.push(r2.clone());
    let seen2 = last_seen(s, before2);
    s.ok(
        VAULT_ASK_CHECKS[7],
        card["field"].is_null()
            && actions.len() == 2
            && actions[0]["label"] == "Allow"
            && actions[0]["body"]["allow"] == true
            && actions[0]["body"]["held"].is_string()
            && actions[1]["label"] == "Decline"
            && overwrite.status == 400
            && stale.status == 409
            && allowed.status == 200
            && allowed.body["state"] == "saved"
            && reply == format!("vault-asked saved {stand_in}")
            && kept["setAt"].as_i64() == set_at_before
            && kept["hosts"] == json!([host, host2])
            && r2.starts_with("fetched 200")
            && seen2["host"] == host2
            && seen2["auth"]["authorization"] == format!("Bearer {value}"),
        json!({ "card": card, "overwrite": overwrite.status, "stale": stale.status, "allowed": allowed.body, "reply": reply, "kept": kept, "r2": r2, "seen2": seen2 }),
    );

    // Non-blocking two-step flow (`--no-wait` + `wait`) for Telegram/SimpleX bridges
    let turn_nw = start(706, &format!("vault-ask-nowait e2e-nowait {host} Telegram ask"))?;
    let nw_reply = replied(s, &turn_nw)?;
    said.push(nw_reply.clone());
    let mut parts = nw_reply.split_whitespace();
    let (nw_tag, nw_id, nw_url) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let nw_card = ask_card(s, api, w.owner, "e2e-nowait")?;
    let nw_save = nw_card["actions"][0]["post"].as_str().unwrap_or("").to_string();
    let nw_saved = api.signed(w.owner, "POST", &nw_save, Some(&json!({ "held": "none", "value": value })))?;
    let turn_w = start(707, &format!("vault-wait {nw_id}"))?;
    let w_reply = replied(s, &turn_w)?;
    said.push(w_reply.clone());
    let nw_stand_in = placeholder(s, w.id, w.agent, "e2e-nowait");
    api.signed(w.owner, "DELETE", "/api/vault/e2e-nowait", None)?;
    s.ok(
        VAULT_ASK_CHECKS[11],
        nw_tag == "vault-ask-started"
            && nw_id.len() == 32
            && nw_url.ends_with(&format!("/vault/ask/{nw_id}"))
            && nw_saved.status == 200
            && w_reply == format!("vault-asked saved {nw_stand_in}"),
        json!({ "nw_reply": nw_reply, "nw_saved": nw_saved.body, "w_reply": w_reply, "expected": nw_stand_in }),
    );

    // the Vault's hosts full: an ask toward one more is refused before any card
    let mut full = false;
    let mut filled = vec![];
    for i in 0..9 {
        let fill = format!("e2e-cap-{i}");
        let r = api.signed(w.owner, "PUT", &format!("/api/vault/{fill}"), Some(&json!({ "value": "v", "hosts": [format!("cap{i}.vault.test")], "agents": [w.agent] })))?;
        if r.status == 400 && r.body["message"].as_str().is_some_and(|m| m.contains("at most 8 hosts")) {
            full = true;
            break;
        }
        anyhow::ensure!(r.status == 200, "a put toward the Vault's cap: {r}");
        filled.push(fill);
    }
    let turn = start(708, "vault-ask e2e-ninth ninth.vault.test One host too many")?;
    let ninth = replied(s, &turn)?;
    said.push(ninth.clone());
    let told = api.signed(w.owner, "GET", "/api/notices?field=1", None)?;
    let drawn = told.body["notices"].as_array().is_some_and(|l| l.iter().any(|n| n["kind"] == "vault_ask" && n["title"].as_str().is_some_and(|t| t.ends_with(" e2e-ninth"))));
    for fill in &filled {
        api.signed(w.owner, "DELETE", &format!("/api/vault/{fill}"), None)?;
    }
    s.ok(
        VAULT_ASK_CHECKS[12],
        full && ninth.starts_with("vault-ask refused:") && ninth.contains("at most 8 hosts") && told.status == 200 && !drawn,
        json!({ "full": full, "filled": filled.len(), "reply": ninth, "drawn": drawn }),
    );

    let view = api.signed(w.owner, "GET", &format!("/api/computers/{}", w.id), None)?;
    let logs = super::vault::logs(s);
    s.ok(VAULT_ASK_CHECKS[8], !said.iter().any(|t| t.contains(&value)) && !view.text.contains(&value) && !logs.contains(&value), json!({ "said": said }));
    // the run's other checks find the Vault as they left it
    api.signed(w.owner, "DELETE", &format!("/api/vault/{name}"), None)?;

    let set = api.signed(w.owner, "PUT", "/api/vault/asks", Some(&json!({ "value": "asks-e2e", "hosts": [], "agents": [w.agent] })))?;
    let gone = api.signed(w.owner, "DELETE", "/api/vault/asks", None)?;
    s.ok(VAULT_ASK_CHECKS[9], set.status == 200 && gone.status == 200 && gone.body["removed"] == true, json!({ "set": set.body, "removed": gone.body }));
    Ok(8)
}
