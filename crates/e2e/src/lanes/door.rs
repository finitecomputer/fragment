//! Sign-in through the identity door (crates/identity-door;
//! docs/self-host.md, seam 4), the `door` section, after `signin`: the
//! node started again as a self-hosted box signs people in (`Shape::Door`), its
//! `WORKOS_API_URL` the door's, its WorkOS client and key the door's, with
//! no code of the cell's own for it. Behind the door, an OpenID Connect
//! provider: the fake (crates/fakes/src/oidc.rs), strict, then shaped as
//! tsidp (no `email_verified`; the door vouches for its domain).
//!
//! Goal: the cell, unchanged, signs a person in through the door and the
//! provider; an email the door cannot vouch for, an id_token not to trust,
//! or the provider's refusal signs no one in, said on the platform's page;
//! nothing is good twice; a door restarted while its person is at the
//! provider finishes the sign-in, and one restarted after its callback
//! tells its person to start again; sign-out keeps to the platform.
//! Method: the browser's round trip by hand (the platform's redirect, the
//! door's, the provider's, the door's again, the callback with the state's
//! cookie), the fake shaped and spoiled in turn; the door dropped and
//! started again on its port; the node started plain again after.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use fragment_fakes::oidc::{Oidc, Spoil};
use fragment_identity_door::oidc::{Settings, Vars};
use fragment_identity_door::{Client, Config, Door, ProviderConfig, Vouched};
use fragment_proto::ErrorCode;

use crate::api::{url_enc, Api, Call, Reply};
use crate::{Suite, DOOR_CLIENT, DOOR_KEY};

/// The door's client at the provider.
const PROVIDER_CLIENT: &str = "fragment-door";
const PROVIDER_SECRET: &str = "door-provider-secret";
/// The domain the door vouches for, as a box on tsidp does.
const VOUCHED: &str = "tailnet.e2e.test";
/// The door's own secret (its state's seal, its verifiers and nonces).
const DOOR_SECRET: &[u8] = b"the e2e door's own secret, of thirty-two bytes or more";

/// A sign-in's legs: the platform's redirect to the door, the door's to
/// the provider, the provider's back to the door, the door's back to the
/// platform, and the platform's answer to that (none when the browser was
/// sent nowhere).
struct Trip {
    start: Reply,
    to_provider: Reply,
    to_door: Reply,
    back: Reply,
    callback: Option<Reply>,
}

fn query_of(r: &Reply) -> BTreeMap<String, String> {
    reqwest::Url::parse(&r.header("location")).map(|u| u.query_pairs().into_owned().collect()).unwrap_or_default()
}

impl Trip {
    fn session(&self) -> Option<String> {
        self.callback.as_ref()?.cookies().into_iter().find_map(|c| c.strip_prefix("fragment_session=").and_then(|v| v.split(';').next()).filter(|v| !v.is_empty()).map(str::to_string))
    }

    fn bound(&self) -> Option<String> {
        self.start.cookies().into_iter().find(|c| c.starts_with("fragment_login=") || c.starts_with("fragment_switch="))
    }

    /// The platform's page said `words` (in HTML: no quotes in them), and
    /// no one is signed in.
    fn page_says(&self, words: &str) -> bool {
        self.callback.as_ref().is_some_and(|r| r.status == 400 && r.header("content-type").starts_with("text/html") && r.text.contains(words)) && self.session().is_none()
    }

    fn said(&self) -> String {
        self.callback.as_ref().map_or_else(|| format!("{} / {} / {}", self.to_provider, self.to_door, self.back), |r| r.text.clone())
    }
}

/// A browser's sign-in from `start` (the platform's redirect to the door,
/// a sign-in's or a switch's), through the door and the provider, to the
/// platform's callback.
fn through(api: &Api, start: Reply, door_to: &str) -> Result<Trip> {
    let to_provider = api.external(door_to)?;
    let to_door = api.external(&to_provider.header("location"))?;
    let back = api.external(&to_door.header("location"))?;
    let mut trip = Trip { start, to_provider, to_door, back, callback: None };
    let to = trip.back.header("location");
    if !to.is_empty() {
        trip.callback = Some(api.call(Call { method: "GET", url: to, cookie: trip.bound(), ..Call::default() })?);
    }
    Ok(trip)
}

fn sign_in(api: &Api, hint: &str) -> Result<Trip> {
    let start = api.unsigned("GET", &format!("/auth/login?return=/x&login_hint={}", url_enc(hint)), None)?;
    anyhow::ensure!(start.status == 302, "GET /auth/login: {start}");
    let to = start.header("location");
    through(api, start, &to)
}

fn door_on(provider: &Oidc, port: u16, callback: &str) -> Result<Door> {
    let vouched = Vouched::new(&[VOUCHED.to_string()])?;
    let settings = Settings::parse(Vars { issuer: &provider.url, client_id: PROVIDER_CLIENT, vouches: true, ..Vars::default() })?;
    let p = ProviderConfig { settings, client_secret: PROVIDER_SECRET.into(), ca_pem: None };
    let client = Client::with_secret(DOOR_CLIENT, DOOR_KEY, &[callback])?;
    let url = format!("http://127.0.0.1:{port}");
    Ok(Door::start(Config::new(([127, 0, 0, 1], port).into(), &url, DOOR_SECRET.to_vec(), p, vouched, vec![client])?)?)
}

pub(super) fn door(s: &mut Suite, api: &Api) -> Result<()> {
    // the OpenID Connect fake it scripts, and the node itself: started
    // again as a self-hosted box, and plain after
    if !s.section("door", &[crate::Need::Fakes, crate::Need::Node]) {
        return Ok(());
    }
    let provider = Oidc::start(PROVIDER_CLIENT, PROVIDER_SECRET)?;
    let callback = format!("{}/auth/callback", api.base);
    let door = door_on(&provider, crate::devstack::free_port()?, &callback)?;
    let url = door.url.clone();
    s.stop()?;
    let api = s.start_through_door(&url)?;
    let checked = cases(s, &api, &provider, door, &callback);
    // the node as the sections after expect it, whatever the cases did
    s.stop()?;
    s.start(false)?;
    checked
}

fn cases(s: &mut Suite, api: &Api, provider: &Oidc, door: Door, callback: &str) -> Result<()> {
    let (door_url, port) = (door.url.clone(), door.addr.port());
    let grace = "grace@corp.e2e.test";
    let t = sign_in(api, grace)?;
    let to = t.start.header("location");
    s.ok(
        "a self-hosted box's sign-in starts at its identity door, as WorkOS's: the door's client, the callback here, a state",
        t.start.status == 302 && to.starts_with(&format!("{door_url}/user_management/authorize?")) && to.contains(&format!("client_id={DOOR_CLIENT}")) && to.contains(&url_enc(callback)) && to.contains("state="),
        &t.start,
    );
    let state = reqwest::Url::parse(&to).ok().and_then(|u| u.query_pairs().find(|(k, _)| k == "state").map(|(_, v)| v.into_owned())).unwrap_or_default();
    let at_provider = query_of(&t.to_provider);
    s.ok(
        "the door sends the browser to the OpenID Connect provider: the code flow, PKCE's S256, a nonce, its own callback and state (never the platform's)",
        t.to_provider.status == 302
            && t.to_provider.header("location").starts_with(&format!("{}/authorize?", provider.url))
            && at_provider.get("code_challenge_method").map(String::as_str) == Some("S256")
            && at_provider.get("nonce").is_some_and(|n| n.len() == 43)
            && at_provider.get("redirect_uri") == Some(&format!("{door_url}/oidc/callback"))
            && at_provider.get("state").is_some_and(|st| !st.is_empty() && *st != state),
        &t.to_provider,
    );
    let back = query_of(&t.back);
    s.ok(
        "the provider sends the browser back to the door, which sends it on to the platform with a code of its own and the platform's state",
        t.to_door.status == 302 && t.back.status == 302 && t.back.header("location").starts_with(&format!("{callback}?code=")) && back.get("state") == Some(&state),
        format!("{} / {}", t.to_door, t.back),
    );
    let me = super::signin::who(api, &t.session().unwrap_or_default())?;
    let grace_id = me["id"].as_str().unwrap_or("").to_string();
    s.ok(
        "the platform exchanges the door's code: a session for the provider's person, their email their handle",
        t.callback.as_ref().is_some_and(|r| r.status == 302 && r.header("location").ends_with("/x")) && me["kind"] == "person" && me["email"] == grace,
        format!("{:?} / {me}", t.callback.as_ref().map(|r| r.to_string())),
    );
    let again = api.call(Call { method: "GET", url: t.back.header("location"), cookie: t.bound(), ..Call::default() })?;
    let replayed = api.external(&t.to_door.header("location"))?;
    s.ok(
        "nothing is good twice: the platform's callback again signs no one in (400), and the provider's callback again finds its code spent (access_denied)",
        again.status == 400 && again.code() == Some(ErrorCode::InvalidRequest) && query_of(&replayed).get("error").map(String::as_str) == Some("access_denied"),
        format!("{again} / {replayed}"),
    );

    // who a person is: the provider's subject, never their email
    let t = sign_in(api, grace)?;
    let me = super::signin::who(api, &t.session().unwrap_or_default())?;
    s.ok("signing in again is the same person", !grace_id.is_empty() && me["id"] == grace_id.as_str(), &me);
    let renamed = "grace.h@corp.e2e.test";
    provider.set_email(&provider.user(grace).id, renamed);
    let t = sign_in(api, grace)?;
    let me = super::signin::who(api, &t.session().unwrap_or_default())?;
    s.ok("an email changed at the provider keeps the person, and the new email shows", me["id"] == grace_id.as_str() && me["email"] == renamed, &me);

    // what the door refuses signs no one in, said on the platform's page
    let jo = provider.user("jo@corp.e2e.test");
    provider.set_verified(&jo.id, false);
    let t = sign_in(api, "jo@corp.e2e.test")?;
    s.ok("an email the provider has not verified, in no domain the door vouches for, signs no one in (access_denied)", t.page_says("access_denied") && t.page_says("has not verified"), t.said());
    provider.like_tsidp(true);
    let t = sign_in(api, &format!("ada@{VOUCHED}"))?;
    let ada = super::signin::who(api, &t.session().unwrap_or_default())?;
    s.ok("shaped as tsidp (no email_verified), a person signs in by a domain the door vouches for", ada["email"] == format!("ada@{VOUCHED}").as_str() && ada["id"] != grace_id.as_str(), &ada);
    let t = sign_in(api, "alice@github")?;
    s.ok("a GitHub login's user@github is no mailbox: it signs no one in, whatever is vouched", t.page_says("access_denied") && t.page_says("is no mailbox"), t.said());
    provider.like_tsidp(false);
    provider.spoil_next(Spoil::BadSignature);
    let t = sign_in(api, grace)?;
    s.ok("an id_token whose signature does not verify signs no one in", t.page_says("access_denied") && t.page_says("signature does not verify"), t.said());
    provider.spoil_next(Spoil::WrongAudience);
    let t = sign_in(api, grace)?;
    s.ok("nor does one issued for another client", t.page_says("access_denied") && t.page_says("for another client"), t.said());
    provider.fail_next("access_denied");
    let t = sign_in(api, grace)?;
    s.ok("the provider's own refusal is shown on the platform's page, and signs no one in", t.page_says("access_denied") && t.page_says("the sign-in provider said"), t.said());

    // the door's state lives in the browser; its codes, in its memory
    let start = api.unsigned("GET", &format!("/auth/login?return=/x&login_hint={}", url_enc(grace)), None)?;
    let to_provider = api.external(&start.header("location"))?;
    drop(door);
    let door = door_on(provider, port, callback)?;
    let to_door = api.external(&to_provider.header("location"))?;
    let back = api.external(&to_door.header("location"))?;
    let bound = start.cookies().into_iter().find(|c| c.starts_with("fragment_login="));
    let finished = api.call(Call { method: "GET", url: back.header("location"), cookie: bound, ..Call::default() })?;
    s.ok(
        "a door restarted while its person was at the provider finishes the sign-in (its state lives in the browser)",
        finished.status == 302 && finished.cookies().iter().any(|c| c.starts_with("fragment_session=") && !c.starts_with("fragment_session=;")),
        &finished,
    );
    let start = api.unsigned("GET", &format!("/auth/login?return=/x&login_hint={}", url_enc(grace)), None)?;
    let to_provider = api.external(&start.header("location"))?;
    let to_door = api.external(&to_provider.header("location"))?;
    let back = api.external(&to_door.header("location"))?;
    drop(door);
    let door = door_on(provider, port, callback)?;
    let bound = start.cookies().into_iter().find(|c| c.starts_with("fragment_login="));
    let stale = api.call(Call { method: "GET", url: back.header("location"), cookie: bound, ..Call::default() })?;
    let fresh = sign_in(api, grace)?;
    s.ok(
        "a door restarted after its callback forgot its code: the platform's callback signs no one in and says to start again (400), and the next sign-in works",
        stale.status == 400 && stale.code() == Some(ErrorCode::InvalidRequest) && stale.text.contains("start the sign-in again") && fresh.session().is_some(),
        format!("{stale} / {:?}", fresh.callback.as_ref().map(|r| r.to_string())),
    );

    // the door keeps no session: sign-out keeps to the platform; another
    // account goes to the provider again, which says who
    let session = fresh.session().unwrap_or_default();
    let out = api.call(Call { method: "POST", url: format!("{}/auth/logout", api.base), cookie: Some(format!("fragment_session={session}")), ..Call::default() })?;
    let gone = super::signin::who(api, &session)?;
    s.ok(
        "sign-out ends the session here and stays on the platform (the door names no session to end)",
        out.status == 302 && !out.header("location").contains(&door_url) && gone.is_null(),
        format!("{out} -> {:?} / {gone}", out.header("location")),
    );
    let session = sign_in(api, grace)?.session().unwrap_or_default();
    let switch = api.call(Call { method: "POST", url: format!("{}/auth/switch?return=/x&login_hint={}", api.base, url_enc(&format!("ada@{VOUCHED}"))), cookie: Some(format!("fragment_session={session}")), ..Call::default() })?;
    let to = switch.header("refresh").split_once("url=").map(|(_, u)| u.to_string()).context("a switch's page leaves for the door")?;
    provider.like_tsidp(true);
    let t = through(api, switch, &to)?;
    provider.like_tsidp(false);
    let me = super::signin::who(api, &t.session().unwrap_or_default())?;
    s.ok(
        "\"Use another account\" goes through the door to the provider again, which says who signs in",
        to.starts_with(&format!("{door_url}/user_management/authorize?")) && me["email"] == format!("ada@{VOUCHED}").as_str() && super::signin::who(api, &session)?.is_null(),
        &me,
    );
    drop(door);
    Ok(())
}
