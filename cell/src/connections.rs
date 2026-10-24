//! A person's providers (decisions 22 and 37, docs/computers.md): every
//! provider of the deployment's catalog (`FRAGMENT_PROVIDERS`), and the
//! state of the person's credential at each. Their agents use them
//! through their computer's swap (computer.rs).
//!
//! - `GET /api/connections` → `Providers`: each provider, its kind, hosts,
//!   environment variables and (an operator key's) price, and its state: a
//!   connection `connected`, `needs_reauthorization` or `not_connected`,
//!   as WorkOS Pipes says of the account (its connected account: no token
//!   is minted to tell); an operator key `offered`; an own key `set` or
//!   `not_set`. Reading it tells the person's computer the connections'
//!   states, so a guest learns of a new connection at its next read.
//! - `POST /api/connections/{provider}/authorize` → `{url}`: where the
//!   person's browser goes to connect a connection. Its own provider's code
//!   returns to Fragment; browser/session/provider binding is checked before
//!   exchanging it and importing tokens into Pipes (docs/pipes-browser-binding.md).
//!   Where the row takes several accounts (`accounts_max`), its body may
//!   ask for another (`{add: true}`) or one of theirs again (`{account}`).
//! - `DELETE /api/connections/{provider}[?account=<id>]` → `{provider,
//!   state, revoked}`: a connection disconnected (one of several accounts,
//!   by its id; with several, one must be named). Its grant is revoked at
//!   the provider where its row names a `revoke` endpoint (with a token
//!   Pipes mints for it), then its account is deleted at Pipes, which ends
//!   every swap of it; `revoked` says whether the provider confirmed. The
//!   person's computer is told at once. Before anything goes, the registry
//!   fences the account: no connect begun before the disconnect connects
//!   it after (a 429 while one is importing).
//! - `PUT /api/connections/{provider}/key {key}` and `DELETE …/key`: the
//!   person's own key for an `own` provider, kept sealed by their computer
//!   (one computer per person for now: decision 13), which swaps it in.
//! - An own key whose row has a sign-in (`oauth`) is connected from
//!   settings instead, never pasted (Paul, 2026-10-08;
//!   `fragment_core::own_signin`): `POST …/authorize`, from the shell's
//!   session, begins it (their computer keeps its nonce and verifier, and a
//!   cookie binds the nonce to their browser) and answers the provider's
//!   page, and `GET /api/connections/{provider}/callback?code=&state=`,
//!   where the provider sends their browser back, finishes it only in that
//!   browser (`auth::trip_bound`), signed in as them: it exchanges the code
//!   for their key and has their own computer seal it. Disconnecting is
//!   `DELETE …/key`; the key itself is revoked at the provider (its
//!   `manage` page).

use std::collections::BTreeMap;

use fragment_core::catalog::{ConnectionOAuth, Kind, KeyOAuth};
use fragment_core::connection_accounts::{self as accounts, Held, Intent};
use fragment_core::own_signin;
use fragment_core::roundtrip::{self, Trip};
use fragment_proto::computer::{ConnectionAccount, ProviderModel, ProviderPrice, ProviderSignIn, ProviderState, ProviderView, Providers};
use fragment_proto::{ErrorCode, IdentityKind};
use serde_json::{json, Value};
use worker::*;

use crate::config::Config;
use crate::error::{CellError, CellResult};
use crate::fragment::json_response;
use crate::keys::PipesAccount;
use crate::registry::calls::SubjectOf;
use crate::routed::Signed;

/// An own key, at most (a provider's API key is far shorter).
pub use fragment_core::own_signin::OWN_KEY_MAX_BYTES;

/// Whether `key` is a key the swap may send: a printable token.
pub fn own_key_ok(key: &str) -> bool {
    (1..=OWN_KEY_MAX_BYTES).contains(&key.len()) && key.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

/// The WorkOS user the person signed in as, which their connections are.
pub async fn workos_user(env: &Env, cfg: &Config, who: &str) -> CellResult<Option<String>> {
    let issuer = crate::keys::workos(env, cfg).await?.issuer();
    Ok(crate::ask_registry(env, &SubjectOf { identity: who.into(), issuer }).await?.subject)
}

/// What a WorkOS user's account stands fenced at by their disconnects
/// (registry/connection_signin.rs `fence_connection`): providers whole,
/// and single accounts of several, by identifier. Pipes' word is not
/// taken for them until a connect of theirs is known to import again.
async fn fenced_at(env: &Env, cfg: &Config, user: &str) -> CellResult<crate::registry::calls::FencedProviders> {
    let issuer = crate::keys::workos(env, cfg).await?.issuer();
    crate::ask_registry(env, &crate::registry::calls::ConnectionFences { issuer, subject: user.into() }).await
}

/// Each connection's state for a person (as Pipes says, with no token,
/// save one their disconnect fences: not connected, Pipes not asked),
/// whose WorkOS user `user` finds (`workos_user`, or the one their computer
/// keeps), asked only when the catalog has a connection: connected while
/// any of their accounts there is. One account a provider is its
/// compatibility connection, asked of Pipes alone; where the row takes
/// several (`accounts_max`), Pipes' plural list is read once for all of
/// them, and an account a disconnect fences is not theirs.
/// `accounts`, when asked for, gets each one's accounts as Pipes describes
/// them (`keys::PipesAccount`: id, label and the scopes each was granted;
/// several oldest first), from the same answers. (One function, not a
/// second beside it: each is instantiated once per caller's future, and
/// the cell's Wasm has a budget.)
pub async fn connection_states(
    env: &Env,
    cfg: &Config,
    user: impl std::future::Future<Output = CellResult<Option<String>>>,
    mut accounts: Option<&mut BTreeMap<String, Vec<PipesAccount>>>,
) -> CellResult<BTreeMap<String, ProviderState>> {
    let connections: Vec<&fragment_core::catalog::Provider> = cfg.providers.providers().iter().filter(|p| p.kind == Kind::Connection).collect();
    let mut states = BTreeMap::new();
    if connections.is_empty() {
        return Ok(states);
    }
    let user = user.await?;
    let api = &cfg.workos()?.api;
    let fenced = match &user {
        Some(u) => fenced_at(env, cfg, u).await?,
        None => Default::default(),
    };
    let plural = match &user {
        Some(u) if connections.iter().any(|p| p.several()) => Some(crate::keys::pipes_accounts(env, api, u).await?),
        _ => None,
    };
    // at most `catalog::PROVIDERS_MAX` providers, one question each
    for p in connections {
        let provider = p.name.as_str();
        // disconnected, and no connect known to have imported since:
        // whatever Pipes holds (an import landed late) is not theirs
        let whole = fenced.providers.iter().any(|f| f == provider);
        let held = match (&user, &plural) {
            (None, _) => vec![],
            // of several, marked, so a disconnect that did not finish is
            // theirs to ask again (`accounts::fenced`)
            (Some(_), Some(list)) if p.several() => {
                let theirs = accounts::listed(list, provider).map_err(|e| CellError::new(ErrorCode::UpstreamFailed, e))?;
                let here: Vec<&str> = fenced.accounts.iter().filter(|f| f.provider == provider).map(|f| f.identifier.as_str()).collect();
                accounts::fenced(theirs, whole, &here)
            }
            (Some(_), _) if whole => vec![],
            (Some(u), _) => Some(crate::keys::pipes_state(env, api, provider, u).await?).filter(|a| a.state != ProviderState::NotConnected).into_iter().collect(),
        };
        states.insert(provider.to_string(), accounts::state_of(&held));
        if let Some(accounts) = accounts.as_deref_mut() {
            accounts.insert(provider.to_string(), held);
        }
    }
    Ok(states)
}

/// What a person's computer holds of their accounts (`connection_states`'
/// `accounts`) to choose one for a swap: each of a connection's that takes
/// several, in order (computer.rs `connection_token`).
pub fn held_of(cfg: &Config, listed: &BTreeMap<String, Vec<PipesAccount>>) -> BTreeMap<String, Vec<Held>> {
    listed.iter().filter(|(p, _)| cfg.providers.get(p).is_some_and(|p| p.several())).map(|(p, l)| (p.clone(), accounts::held(l))).collect()
}

/// `provider`'s accounts of the person signed in as `who`, as their list
/// names them (`connection_states`), those they disconnected marked.
async fn accounts_of(env: &Env, cfg: &Config, who: &str, provider: &str) -> CellResult<Vec<PipesAccount>> {
    let mut listed = BTreeMap::new();
    connection_states(env, cfg, workos_user(env, cfg, who), Some(&mut listed)).await?;
    Ok(listed.remove(provider).unwrap_or_default())
}

/// What a disconnect of one of a person's several accounts takes away.
enum Going {
    /// An account Pipes holds, fenced or not (a disconnect asked again
    /// finishes what the one before did not), and how many others of
    /// theirs are there.
    Held(PipesAccount, usize),
    /// One a disconnect of theirs fenced, which Pipes deleted already: what
    /// is left is telling their computer.
    Deleted(String),
    /// None: they have none there.
    Nothing,
}

/// The account a disconnect at `provider` takes away: the one `named` (its
/// id), only ever one of the person's own, as their list names it, fenced
/// or not, or as their own disconnect of it recorded it once Pipes deleted
/// it (an account with no identifier too); naming none, the one they have
/// (with several, they name it).
async fn going(env: &Env, cfg: &Config, who: &str, provider: &str, named: Option<&str>) -> CellResult<Going> {
    let theirs = accounts_of(env, cfg, who, provider).await?;
    let others = |a: &PipesAccount| theirs.iter().filter(|x| !x.fenced && x.id != a.id).count();
    let Some(id) = named else {
        let visible: Vec<&PipesAccount> = theirs.iter().filter(|a| !a.fenced).collect();
        return match visible.as_slice() {
            [] => Ok(Going::Nothing),
            [a] => Ok(Going::Held((*a).clone(), 0)),
            _ => {
                let named: Vec<String> = visible.iter().map(|a| format!("{} ({})", a.id.as_deref().unwrap_or_default(), a.label.as_deref().unwrap_or("no name"))).collect();
                Err(CellError::invalid(format!("you have {} {provider} accounts connected: name the one to disconnect, ?account=<id> (yours: {})", visible.len(), named.join(", "))))
            }
        };
    };
    if let Some(a) = theirs.iter().find(|a| a.id.as_deref() == Some(id)) {
        return Ok(Going::Held(a.clone(), others(a)));
    }
    let issuer = crate::keys::workos(env, cfg).await?.issuer();
    let asked = crate::registry::calls::Disconnected { identity: who.into(), issuer, provider: provider.to_string(), account: id.to_string() };
    let deleted = crate::ask_registry(env, &asked).await?.disconnected;
    match deleted {
        true => Ok(Going::Deleted(id.to_string())),
        false => Err(CellError::new(ErrorCode::NotFound, format!("no {provider} account {id:?} of yours: GET /api/connections lists them"))),
    }
}

pub(crate) async fn route(req: &Request, env: &Env, url: &Url, signed: &Signed, method: Method, rest: &[&str], body: &[u8]) -> CellResult<Response> {
    if signed.identity.kind != IdentityKind::Person {
        return Err(CellError::new(ErrorCode::Forbidden, "connections are a person's: their agents use them through their computer"));
    }
    let who = signed.identity.id.as_str();
    let cfg = Config::from_env(env);
    let offered = || cfg.providers.providers().iter().map(|p| p.name.clone()).collect::<Vec<_>>();
    let provider_of = |name: &str| cfg.providers.get(name).ok_or_else(|| CellError::new(ErrorCode::NotFound, format!("no provider {name:?} here (this deployment offers {:?})", offered())));
    let computer = fragment_core::computer::default_computer_of(who);
    match (method.clone(), rest) {
        (Method::Get, []) => {
            let mut listed = BTreeMap::new();
            let states = connection_states(env, cfg, workos_user(env, cfg, who), Some(&mut listed)).await?;
            // the person's computer keeps them, for its guest's next read; one
            // not made yet has nothing to tell, and so no own key either
            let told = json!({ "connections": states, "accounts": held_of(cfg, &listed) });
            let own: Vec<String> = match crate::computer::ask(env, &computer, "computer/own-keys", &told).await {
                Ok(v) => serde_json::from_value(v["providers"].clone()).map_err(|e| CellError::host(format!("the computer's own keys: {e}")))?,
                Err(e) if e.code == ErrorCode::NotFound => vec![],
                Err(e) => return Err(e),
            };
            let providers = cfg
                .providers
                .providers()
                .iter()
                .map(|p| (p, listed.get(&p.name).map(Vec::as_slice).unwrap_or_default()))
                .map(|(p, held)| (p, held, accounts::default_of(held)))
                .map(|(p, held, account)| ProviderView {
                    provider: p.name.clone(),
                    kind: p.kind,
                    state: match p.kind {
                        Kind::Connection => states.get(&p.name).copied().unwrap_or(ProviderState::NotConnected),
                        Kind::Operator => ProviderState::Offered,
                        Kind::Own if own.contains(&p.name) => ProviderState::Set,
                        Kind::Own => ProviderState::NotSet,
                    },
                    hosts: p.hosts.clone(),
                    env: p.env.clone(),
                    price: p.price.map(|pr| ProviderPrice { micros: pr.micros, per: pr.per }),
                    sign_in: p.oauth.as_ref().map(|o| ProviderSignIn { manage: o.manage.clone() }),
                    models: p.models.as_ref().map(|m| m.offer.iter().map(|o| ProviderModel { id: o.id.clone(), name: o.name.clone() }).collect()),
                    // the default account's, of several
                    account: account.and_then(|a| a.label.clone()),
                    granted: account.and_then(|a| a.scopes.clone()),
                    missing: account.and_then(|a| missing(p.connection_oauth.as_ref(), a)),
                    note: p.note.clone(),
                    accounts: p.several().then(|| {
                        held.iter()
                            .filter_map(|a| {
                                let (granted, missing) = match a.fenced {
                                    true => (None, None),
                                    false => (a.scopes.clone(), missing(p.connection_oauth.as_ref(), a)),
                                };
                                Some(ConnectionAccount { id: a.id.clone()?, account: a.label.clone(), state: a.state, granted, missing, disconnecting: a.fenced })
                            })
                            .collect()
                    }),
                    accounts_max: p.several().then(|| p.accounts_max()),
                })
                .collect();
            json_response(&Providers { providers })
        }
        (Method::Post, [provider, "authorize"]) => {
            let p = provider_of(provider)?;
            if let (Kind::Own, Some(o)) = (p.kind, &p.oauth) {
                // its round trip is bound to a browser, so it begins in one:
                // the shell's, signed in by its session (a key's answer would
                // set its cookie in no browser)
                if signed.key.is_some() {
                    return Err(CellError::new(ErrorCode::Forbidden, format!("{provider} is connected from settings, in a browser signed in to the platform")));
                }
                return sign_in(env, cfg, url, &computer, provider, o).await;
            }
            if p.kind != Kind::Connection {
                return Err(CellError::invalid(format!("{provider} is no connection: it is a key, which is not authorized through WorkOS")));
            }
            if signed.key.is_some() {
                return Err(CellError::new(ErrorCode::Forbidden, "connect from settings in a browser signed in to the platform"));
            }
            let o = p.connection_oauth.as_ref().ok_or_else(|| CellError::new(ErrorCode::NotConnected, "browser-bound authorization is not configured for this connection"))?;
            reachable(cfg, &o.authorize)?;
            reachable(cfg, &o.exchange)?;
            let session = crate::auth::platform_session_token(req, url)?.ok_or_else(|| CellError::new(ErrorCode::Unauthenticated, "sign in before connecting"))?;
            let asked: Value = if body.iter().all(u8::is_ascii_whitespace) { Value::Null } else { serde_json::from_slice(body).map_err(|e| CellError::invalid(format!("body: {e}")))? };
            let intent = Intent::asked(&asked).map_err(CellError::invalid)?;
            if intent != Intent::Connect {
                if !p.several() {
                    return Err(CellError::invalid(format!("{provider} connects one account here: connect it with no body")));
                }
                // said before the provider's page: the callback checks again
                let theirs: Vec<PipesAccount> = accounts_of(env, cfg, who, provider).await?.into_iter().filter(|a| !a.fenced).collect();
                match &intent {
                    Intent::Add if theirs.len() >= usize::from(p.accounts_max()) => {
                        return Err(CellError::invalid(format!("you have {} {provider} accounts connected, the most this platform connects: disconnect one to add another", theirs.len())));
                    }
                    Intent::Reconnect(id) if !theirs.iter().any(|a| a.id.as_deref() == Some(id.as_str())) => {
                        return Err(CellError::new(ErrorCode::NotFound, format!("no {provider} account {id:?} of yours: GET /api/connections lists them")));
                    }
                    _ => {}
                }
            }
            let begun = crate::ask_registry(env, &crate::registry::calls::BeginConnection { session, provider: provider.to_string(), intent }).await?;
            if !roundtrip::valid_state(&begun.state) || !own_signin::valid_token(&begun.challenge) { return Err(CellError::host("invalid connection challenge")); }
            let to = fragment_core::connection_signin::authorize_url(o, &callback_url(cfg, provider), &begun.challenge, &begun.state);
            let mut resp = json_response(&json!({ "provider": provider, "url": to }))?;
            resp.headers_mut().append("set-cookie", &crate::auth::trip_begun(Trip::Connection, &begun.state, url))?;
            Ok(resp)
        }
        (Method::Delete, [provider]) => {
            let p = provider_of(provider)?;
            match p.kind {
                Kind::Connection => {}
                Kind::Own => return Err(CellError::invalid(format!("{provider} is no connection to disconnect: your own key is removed with DELETE /api/connections/{provider}/key"))),
                Kind::Operator => return Err(CellError::invalid(format!("{provider} is no connection to disconnect: it is the platform's key, not yours"))),
            }
            // which of several accounts: the one named, or the one there is
            let named = crate::auth::query(url, "account");
            if named.is_some() && !p.several() {
                return Err(CellError::invalid(format!("{provider} connects one account here: disconnect it with no account named")));
            }
            // of several: the one going (asked again, still the person's
            // once fenced, or deleted at Pipes), and their others there
            let going = match p.several() {
                true => Some(going(env, cfg, who, provider, named.as_deref()).await?),
                false => None,
            };
            // first, no connect begun before this one connects it after:
            // the registry ends those pending and cancels those exchanging
            // (429 while one imports), and names the person's WorkOS user;
            // of several, it fences that account alone for good
            let issuer = crate::keys::workos(env, cfg).await?.issuer();
            let lasting = match &going {
                Some(Going::Held(a, others)) => accounts::lasting(Some(a), *others),
                // its fence stands already
                Some(Going::Deleted(_)) => accounts::Lasting::Nothing,
                Some(Going::Nothing) | None => accounts::Lasting::Provider,
            };
            let id = match &going {
                Some(Going::Held(a, _)) => a.id.clone(),
                Some(Going::Deleted(id)) => Some(id.clone()),
                Some(Going::Nothing) | None => None,
            };
            // of several, the account it takes is recorded by its id, its
            // fence or none, so a disconnect asked again finishes
            let fence = crate::registry::calls::FenceConnection { identity: who.into(), issuer, provider: provider.to_string(), lasting, account: id.clone() };
            let user = crate::ask_registry(env, &fence).await?.subject;
            // each step may be asked again: a token Pipes no longer gives
            // revokes nothing, and an account it no longer holds is deleted
            let revoked = match (&user, &going) {
                (Some(u), Some(Going::Held(..)) | None) => disconnect(env, cfg, provider, p.connection_oauth.as_ref(), u, id.as_deref()).await?,
                _ => false,
            };
            let states = disconnected_told(env, cfg, &computer, provider, user).await?;
            let state = states.get(*provider).copied().unwrap_or(ProviderState::NotConnected);
            json_response(&json!({ "provider": provider, "account": id, "state": state, "revoked": revoked }))
        }
        (Method::Put | Method::Delete, [provider, "key"]) => {
            if provider_of(provider)?.kind != Kind::Own {
                return Err(CellError::invalid(format!("{provider} takes no key of yours: only an own key's provider does")));
            }
            let key = if method == Method::Put {
                let v: Value = serde_json::from_slice(body).map_err(|e| CellError::invalid(format!("body: {e}")))?;
                let key = v["key"].as_str().map(str::trim).unwrap_or_default().to_string();
                if !own_key_ok(&key) {
                    return Err(CellError::invalid(format!("a key is 1 to {OWN_KEY_MAX_BYTES} printable characters, with no space")));
                }
                Some(key)
            } else {
                None
            };
            match crate::computer::ask(env, &computer, "computer/own-key", &json!({ "provider": provider, "key": key })).await {
                Ok(_) => json_response(&json!({ "provider": provider, "state": if key.is_some() { ProviderState::Set } else { ProviderState::NotSet } })),
                Err(e) if e.code == ErrorCode::NotFound => Err(CellError::new(ErrorCode::NotFound, "your own keys are kept by your computer: make it first (POST /api/computers)")),
                Err(e) => Err(e),
            }
        }
        (m, _) => Err(CellError::new(ErrorCode::NotFound, format!("no route {} /api/connections/{}", m.as_ref(), rest.join("/")))),
    }
}

/// Whether the cell may send a person's browser, or a code, to `url`:
/// https, or a local fleet's fake where its egress may reach local
/// addresses (the catalog takes loopback http only for them).
fn reachable(cfg: &Config, url: &str) -> CellResult<()> {
    if url.starts_with("https://") || cfg.egress_local {
        return Ok(());
    }
    Err(CellError::new(ErrorCode::HostFailed, format!("{url} is a local fleet's, and this one's egress may not reach local addresses")))
}

/// What `account`'s row asks for that the person did not allow it (none
/// said where Pipes names no scopes, or the row asks for none).
fn missing(o: Option<&ConnectionOAuth>, account: &PipesAccount) -> Option<Vec<String>> {
    o.zip(account.scopes.as_deref()).map(|(o, granted)| fragment_core::connection_signin::missing(&o.scopes, granted))
}

/// Where the provider sends the person's browser back.
fn callback_url(cfg: &Config, provider: &str) -> String {
    format!("{}/api/connections/{provider}/callback", cfg.platform())
}

/// `user`'s connection to `provider` taken away (of several, the account
/// `account` names): its grant revoked at the provider first, while Pipes
/// still holds a token for it (`o.revoke`; a failure there is logged and
/// the account goes anyway: the person asked for their agents to lose it),
/// then its account deleted at Pipes, which ends every swap of it. Answers
/// whether the provider confirmed the revoke.
async fn disconnect(env: &Env, cfg: &Config, provider: &str, o: Option<&ConnectionOAuth>, user: &str, account: Option<&str>) -> CellResult<bool> {
    let api = &cfg.workos()?.api;
    let revoked = match o.and_then(|o| o.revoke.as_deref()) {
        Some(revoke) => match revoke_grant(env, cfg, api, provider, user, account, revoke).await {
            Ok(r) => r,
            Err(e) => {
                console_error!("{}", json!({ "connection-disconnect": provider, "not-revoked": e.message }));
                false
            }
        },
        None => false,
    };
    crate::keys::pipes_delete(env, api, provider, user, account).await?;
    Ok(revoked)
}

/// The grant behind `user`'s account at `provider` (of several, the one
/// `account` names), revoked with an access token Pipes mints for it (one
/// Pipes refuses, the account inactive or gone, has no grant left to
/// revoke here).
async fn revoke_grant(env: &Env, cfg: &Config, api: &str, provider: &str, user: &str, account: Option<&str>, revoke: &str) -> CellResult<bool> {
    reachable(cfg, revoke)?;
    let (status, answer) = crate::keys::pipes_token(env, api, provider, user, account).await?;
    let Some(token) = (status == 200 && answer["active"] == true).then(|| answer["access_token"]["access_token"].as_str()).flatten().filter(|t| !t.is_empty()) else {
        return Ok(false);
    };
    crate::keys::connection_revoke(revoke, &fragment_core::connection_signin::revoke_form(token)).await
}

/// The person's computer told their connections' states after a
/// disconnect, so its guest is given that one no more at its next read and
/// its swaps refuse it at once (they go by these states: computer.rs
/// `connection_token`), rather than once its states age out. A computer
/// not made yet has no guest. One not told is the disconnect's failure,
/// which the person asks again (each step of it may be asked twice).
/// Answers the states it told.
async fn disconnected_told(env: &Env, cfg: &Config, computer: &str, provider: &str, user: Option<String>) -> CellResult<BTreeMap<String, ProviderState>> {
    let mut listed = BTreeMap::new();
    let states = connection_states(env, cfg, std::future::ready(Ok(user)), Some(&mut listed)).await?;
    match crate::computer::ask(env, computer, "computer/own-keys", &json!({ "connections": states, "accounts": held_of(cfg, &listed) })).await {
        Ok(_) => Ok(states),
        Err(e) if e.code == ErrorCode::NotFound => Ok(states),
        Err(e) => {
            console_error!("{}", json!({ "connection-disconnect": provider, "computer-not-told": e.message }));
            Err(CellError::new(e.code, format!("{provider} is disconnected, but your computer could not be told yet: disconnect again ({})", e.message)))
        }
    }
}

/// A sign-in to `provider`'s own key, begun: the person's computer keeps
/// its nonce and verifier, and the nonce, the round trip's state, is bound
/// to their browser (`auth::trip_begun`); answers the provider's page.
async fn sign_in(env: &Env, cfg: &Config, url: &Url, computer: &str, provider: &str, o: &KeyOAuth) -> CellResult<Response> {
    reachable(cfg, &o.authorize)?;
    let begun = match crate::computer::ask(env, computer, "computer/sign-in", &json!({ "provider": provider })).await {
        Ok(v) => v,
        Err(e) if e.code == ErrorCode::NotFound => return Err(CellError::new(ErrorCode::NotFound, "your own keys are kept by your computer: make it first (POST /api/computers)")),
        Err(e) => return Err(e),
    };
    let (nonce, challenge) = (begun["nonce"].as_str().unwrap_or_default(), begun["challenge"].as_str().unwrap_or_default());
    if !roundtrip::valid_state(nonce) || !own_signin::valid_token(challenge) {
        return Err(CellError::host("the computer began a sign-in with no nonce"));
    }
    let to = own_signin::authorize_url(o, &callback_url(cfg, provider), challenge, nonce);
    let mut resp = json_response(&json!({ "provider": provider, "url": to }))?;
    resp.headers_mut().append("set-cookie", &crate::auth::trip_begun(Trip::OwnKey, nonce, url))?;
    Ok(resp)
}

/// Why an own-key sign-in did not finish. Each is a page that says what to
/// do, shown at a URL that holds no code (`?refused=<word>`): the callback
/// answers a refusal with a redirect there, so the provider's code (unspent,
/// when the browser was not the one that began it) leaves the address bar
/// and the history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unfinished {
    /// This browser did not begin it (no cookie, or another sign-in's), or
    /// began it too long ago, or began another since.
    NotBegunHere,
    /// No one is signed in to the platform here.
    SignedOut,
    /// The provider sent no code back: the person declined, or it failed.
    Declined,
    /// The person's computer has no such sign-in under way: finished
    /// already, expired, made for another provider, or never theirs.
    NotUnderWay,
    /// The provider refused the code (a 4xx): expired, or spent.
    CodeRefused,
    /// A connection's provider granted nothing its row asks for beyond
    /// signing in: the person unticked everything else.
    NoScopes,
    /// Another account, refused by Pipes: it connects one account a person
    /// for now (404 `multiple_connections_unavailable`). Nothing imported.
    OneAccount,
    /// Another account, past the row's `accounts_max` or Pipes' limit.
    /// Nothing imported.
    AccountLimit,
    /// Of several accounts, an earlier connect there may still land (its
    /// outcome unknown). Nothing imported.
    Finishing,
    /// The provider failed, or answered no key.
    NoKey,
}

impl Unfinished {
    const ALL: [Unfinished; 10] = [
        Unfinished::NotBegunHere,
        Unfinished::SignedOut,
        Unfinished::Declined,
        Unfinished::NotUnderWay,
        Unfinished::CodeRefused,
        Unfinished::NoScopes,
        Unfinished::OneAccount,
        Unfinished::AccountLimit,
        Unfinished::Finishing,
        Unfinished::NoKey,
    ];

    /// Its word in the clean URL.
    fn word(self) -> &'static str {
        match self {
            Unfinished::NotBegunHere => "elsewhere",
            Unfinished::SignedOut => "signed-out",
            Unfinished::Declined => "declined",
            Unfinished::NotUnderWay => "not-under-way",
            Unfinished::CodeRefused => "code-refused",
            Unfinished::NoScopes => "no-scopes",
            Unfinished::OneAccount => "one-account",
            Unfinished::AccountLimit => "account-limit",
            Unfinished::Finishing => "finishing",
            Unfinished::NoKey => "no-key",
        }
    }

    fn parse(word: &str) -> Option<Unfinished> {
        Unfinished::ALL.into_iter().find(|u| u.word() == word)
    }

    /// What its page says, of `provider` (its name for people: `named`).
    fn message(self, provider: &str) -> String {
        let says = match self {
            Unfinished::NotBegunHere => "This sign-in did not begin in this browser, or began too long ago, or another began since: connect again from settings.",
            Unfinished::SignedOut => "You are signed out here: sign in, then connect again from settings.",
            Unfinished::Declined => "The provider sent no key back (declined, or it failed): connect again from settings if you meant to.",
            Unfinished::NotUnderWay => "This sign-in is not one under way for you (finished already, or expired): connect again from settings.",
            Unfinished::CodeRefused => "The provider refused the sign-in's code (expired, or used): connect again from settings.",
            Unfinished::NoScopes => "You allowed nothing your agents could use, only who you are, so nothing was connected: connect again from settings and leave at least one permission ticked.",
            Unfinished::OneAccount => {
                return format!("This platform can connect one {} account for now, and you have one connected, so nothing was added. To use another, disconnect the one you have in settings, then connect again.", named(provider));
            }
            Unfinished::AccountLimit => return format!("You have connected as many {} accounts as this platform allows, so nothing was added: disconnect one in settings to add another.", named(provider)),
            Unfinished::Finishing => return format!("An earlier connect of a {} account may still be finishing, so nothing was connected: connect again from settings in a minute.", named(provider)),
            Unfinished::NoKey => "The provider gave no key back: connect again from settings.",
        };
        says.to_string()
    }
}

/// A provider's name for people, as settings writes it: its first letter
/// upper case (`google` is Google).
fn named(provider: &str) -> String {
    let mut chars = provider.chars();
    chars.next().map(|c| c.to_ascii_uppercase().to_string() + chars.as_str()).unwrap_or_default()
}

/// `GET /api/connections/{provider}/callback?code=&state=` on the
/// platform's host: the person's browser, back from the provider's page.
/// It finishes only the sign-in this browser began (its cookie holds the
/// state: `auth::trip_bound`), and only for the person signed in here: the
/// computer is theirs (`computer::SignedIn::of`), never one a URL names, so
/// a sign-in someone else began and sent them seals nothing anywhere. The
/// code is exchanged for their key, which their computer seals; a page
/// says so. A refusal at the provider (the person declined: no code) or
/// here is a 303 to `?refused=<word>`, a page that says what to do;
/// nothing is set.
pub(crate) async fn callback(req: &Request, env: &Env, url: &Url, provider: &str) -> CellResult<Response> {
    let cfg = Config::from_env(env);
    if !cfg.is_platform_host(url.host_str().unwrap_or_default()) {
        return Err(CellError::new(ErrorCode::NotFound, "a sign-in comes back to the platform's own host"));
    }
    let page = |status: u16, title: &str, says: &str| crate::auth::page(status, title, &format!("<p>{}</p><p><a href=\"/settings\">Back to settings</a></p>", crate::auth::esc(says)));
    let Some(p) = cfg.providers.get(provider) else { return page(400, "Not connected", "This platform offers no such provider.") };
    let trip = match p.kind {
        Kind::Own if p.oauth.is_some() => Trip::OwnKey,
        Kind::Connection if p.connection_oauth.is_some() => Trip::Connection,
        _ => return page(400, "Not connected", "Browser authorization is not configured for this provider."),
    };
    // the clean page a refusal is sent to: its word, and nothing else
    if let Some(word) = crate::auth::query(url, "refused") {
        let why = Unfinished::parse(&word).map_or_else(|| "This sign-in did not finish: connect again from settings.".to_string(), |u| u.message(provider));
        return page(400, "Not connected", &why);
    }
    let finished = if trip == Trip::Connection { finish_connection(req, env, url, provider).await? } else { finish(req, env, cfg, url, provider, p.oauth.as_ref().expect("checked above")).await? };
    let spent = (finished != Err(Unfinished::NotBegunHere)).then(|| crate::auth::trip_spent(trip, url));
    let mut resp = match finished {
        Ok(()) => page(200, "Connected", &format!("{provider} is connected: your agents can use it now. You can close this window."))?,
        Err(why) => {
            console_log!("{}", json!({ "connection-sign-in": provider, "unfinished": why.word() }));
            crate::auth::redirect(&format!("{}?refused={}", callback_url(cfg, provider), why.word()), &[])?.with_status(303)
        }
    };
    if let Some(spent) = spent {
        resp.headers_mut().append("set-cookie", &spent)?;
    }
    Ok(resp)
}

async fn finish_connection(req: &Request, env: &Env, url: &Url, provider: &str) -> CellResult<Result<(), Unfinished>> {
    use crate::registry::calls::{ConnectionFinished, FinishConnection};
    let Some(bound) = crate::auth::trip_bound(req, url, &[Trip::Connection])? else { return Ok(Err(Unfinished::NotBegunHere)) };
    let Some(session) = crate::auth::platform_session_token(req, url)? else { return Ok(Err(Unfinished::SignedOut)) };
    let code = crate::auth::query(url, "code").filter(|c| own_signin::valid_code(c) && crate::auth::query(url, "error").is_none());
    let call = FinishConnection::of(bound, session, provider.to_string(), code);
    Ok(match crate::ask_registry(env, &call).await {
        Ok(ConnectionFinished::Connected) => Ok(()),
        Ok(ConnectionFinished::NotUnderWay) => Err(Unfinished::NotUnderWay),
        Ok(ConnectionFinished::Declined) => Err(Unfinished::Declined),
        Ok(ConnectionFinished::CodeRefused) => Err(Unfinished::CodeRefused),
        Ok(ConnectionFinished::NoScopes) => Err(Unfinished::NoScopes),
        Ok(ConnectionFinished::OneAccount) => Err(Unfinished::OneAccount),
        Ok(ConnectionFinished::AccountLimit) => Err(Unfinished::AccountLimit),
        Ok(ConnectionFinished::Finishing) => Err(Unfinished::Finishing),
        Ok(ConnectionFinished::Failed) => Err(Unfinished::NoKey),
        Err(e) if e.code == ErrorCode::Unauthenticated => Err(Unfinished::SignedOut),
        // Do not leave a code in the URL on network/import failures. The
        // server has spent the transaction; connecting again is required.
        Err(_) => Err(Unfinished::NoKey),
    })
}

/// The own-key sign-in this request comes back with, finished: bound to
/// this browser, for the person signed in here, whose computer began it
/// (another's has no such nonce, and keeps none of theirs); its verifier
/// taken (once), the provider's code exchanged with it, and the key sealed
/// by that computer. `Err`: nothing is set.
async fn finish(req: &Request, env: &Env, cfg: &Config, url: &Url, provider: &str, o: &KeyOAuth) -> CellResult<Result<(), Unfinished>> {
    let Some(bound) = crate::auth::trip_bound(req, url, &[Trip::OwnKey])? else { return Ok(Err(Unfinished::NotBegunHere)) };
    let Some((_, live)) = crate::auth::platform_session(req, env, url).await? else { return Ok(Err(Unfinished::SignedOut)) };
    let signed = crate::computer::SignedIn::of(bound, &live.identity, provider);
    // taken whatever comes back: a declined sign-in is let go too
    let taken = crate::computer::signed_in(env, &signed).await;
    let Some(code) = crate::auth::query(url, "code").filter(|c| own_signin::valid_code(c)) else { return Ok(Err(Unfinished::Declined)) };
    let verifier = match taken {
        Ok(v) => v,
        Err(e) if matches!(e.code, ErrorCode::NotFound | ErrorCode::InvalidRequest) => return Ok(Err(Unfinished::NotUnderWay)),
        Err(e) => return Err(e),
    };
    reachable(cfg, &o.exchange)?;
    let host = url::Url::parse(&o.exchange).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_default();
    let (status, answer) = crate::keys::own_key_exchange(&o.exchange, &own_signin::exchange_body(&code, &verifier), &host).await?;
    let key = match own_signin::key_of(status, &answer) {
        Ok(k) => k,
        Err(own_signin::ExchangeError::Refused(..)) => return Ok(Err(Unfinished::CodeRefused)),
        Err(own_signin::ExchangeError::Failed(why)) => {
            console_error!("{}", json!({ "own-key-sign-in": provider, "no-key": why }));
            return Ok(Err(Unfinished::NoKey));
        }
    };
    crate::computer::ask(env, signed.computer(), "computer/own-key", &json!({ "provider": provider, "key": key })).await?;
    Ok(Ok(()))
}
