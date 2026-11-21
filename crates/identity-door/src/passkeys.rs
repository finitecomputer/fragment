//! The door's own sign-in: passkeys (WebAuthn), in place of an OpenID
//! Connect provider. The door still answers the platform as WorkOS's
//! endpoints do (serve.rs); only who vouches for the person changes: their
//! own passkey, checked here by webauthn-rs (Kanidm's, maintained; nothing
//! of WebAuthn is written here).
//!
//! **Who may come in:**
//!
//! - *A first passkey* comes only with an invite: a one-time link for one
//!   email, good for `INVITE_TTL_S`, that the home's operator makes
//!   (`fragment-identity-door invite`). It makes a new account and nothing
//!   else: an invite for an email that already has an account is refused,
//!   so no invite reaches into an existing one.
//! - *An invite asked for* by the door's inviter (`passkeys.inviter`: one
//!   key, a platform's, that starts accounts here for its own signed-in
//!   people): its NIP-98 signed ask at `INVITES_PATH` names an email (and a
//!   nonce, so two asks in one second are two events); a new
//!   one gets an invite good for `INVITE_ASKED_TTL_S`, and one with an
//!   account gets "sign in" and nothing else (`asked_invite`). So the
//!   inviter can start accounts here and get into none. An ask is answered
//!   once, within `ASK_WINDOW_S` of its signing and after the door started.
//! - *Every later passkey* needs the account's own: a sign-in with one of
//!   its passkeys first, or, when they are all lost, one of its recovery
//!   codes (`RECOVERY_CODES`, shown once at enrollment, kept as their
//!   SHA-256, each good once). There is no other way in, and no reset by an
//!   operator.
//! - *A sign-in* names the account's email, and only that account's
//!   passkeys can answer its challenge (user verification required).
//!
//! **What the browser and the door share:** nothing but the page's flow.
//! Each flow (a challenge in progress) lives in the door's memory alone,
//! for `FLOW_TTL`, once, at most `FLOWS_MAX` at a time; a restart forgets
//! them, and a person begins again. The platform's own state rides sealed
//! in the sign-in page (state.rs) and back out to its callback, as with a
//! provider. No cookie is set.
//!
//! **What WebAuthn checks for us:** the relying party id (the box's
//! domain) and the one origin allowed (the platform's own: an app's
//! subdomain is refused), the challenge, user verification, the
//! signature by the passkey's public key, and its counter.

pub mod pages;
pub mod store;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use url::Url;
use webauthn_rs::prelude::{
    CreationChallengeResponse, Passkey, PasskeyAuthentication, PasskeyRegistration, PublicKeyCredential, RegisterPublicKeyCredential, RequestChallengeResponse, Uuid, Webauthn, WebauthnBuilder,
};

use crate::codes::random_code;
use crate::person::{subject_of, Person};
use crate::state::Begun;
use store::{Account, Invite, State, Store, StoreError, EMAIL_BYTES_MAX, RECOVERY_CODES};

/// A challenge in progress, at most this long.
pub const FLOW_TTL: Duration = Duration::from_secs(300);
/// Challenges in progress at once, at most.
pub const FLOWS_MAX: usize = 256;
/// An invite is good this long, unless its maker says less.
pub const INVITE_TTL_S: i64 = 24 * 3600;
/// An invite's own life, at most.
pub const INVITE_TTL_MAX_S: i64 = 7 * 24 * 3600;
/// Where the inviter asks for an invite, under the door's public URL.
pub const INVITES_PATH: &str = "/passkeys/invites";
/// An asked invite's life: its person's browser goes to it at once.
pub const INVITE_ASKED_TTL_S: i64 = 3600;
/// How far an ask's signing (NIP-98's `created_at`) may be from the door's
/// clock, either way.
pub const ASK_WINDOW_S: i64 = 60;
/// Asks remembered at once, each for `2 * ASK_WINDOW_S` (as long as its
/// replay could pass the clock), at most.
pub const ASKS_MAX: usize = 256;
/// An ask's nonce at most: NIP-98 signs with no randomness and to the
/// second, so an inviter makes two asks in one second distinct by one.
pub const NONCE_CHARS_MAX: usize = 64;
/// The issuer a passkey account's subject is made under: the door's own
/// for its relying party (person.rs), never a provider's.
pub fn issuer_of(rp_id: &str) -> String {
    format!("passkeys:{rp_id}")
}

/// The passkey door's configuration, checked.
#[derive(Clone, Debug)]
pub struct PasskeysConfig {
    /// The relying party: the box's domain (`nitro.example.com`).
    pub rp_id: String,
    /// The one origin a passkey is used from: the platform's.
    pub origin: Url,
    pub rp_name: String,
    /// The state's directory (the box's encrypted disk).
    pub state_dir: PathBuf,
    /// The one key (64 hex) whose signed asks make invites, if any.
    pub inviter: Option<String>,
}

/// What an inviter's ask gets.
#[derive(Debug, PartialEq, Eq)]
pub enum Invited {
    /// A new account's invite: its link's token.
    New(String),
    /// The email has an account here: its person signs in.
    HasAccount,
}

#[derive(Debug, thiserror::Error)]
pub enum PasskeyError {
    /// What the person is told; nothing more is said to them.
    #[error("{0}")]
    Refused(&'static str),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("WebAuthn: {0}")]
    WebAuthn(String),
    /// An ask for an invite that is not the inviter's, fresh and first.
    #[error("{0}")]
    NotAsked(String),
}

impl PasskeyError {
    /// A refusal the person can act on (a 4xx), or the door's own failure.
    pub fn is_refusal(&self) -> bool {
        matches!(self, PasskeyError::Refused(_))
    }
}

fn webauthn_error(e: impl std::fmt::Display) -> PasskeyError {
    PasskeyError::WebAuthn(e.to_string())
}

/// What a flow is waiting for.
enum Flow {
    /// A sign-in for the platform: its sealed round trip, and the account.
    SignIn { begun: Begun, account: Uuid, auth: PasskeyAuthentication },
    /// An invite's new account, its first passkey being made.
    Enroll { invite_sha256: String, email: String, account: Uuid, reg: PasskeyRegistration },
    /// Another passkey for an account: first its own passkey's assertion…
    AddProve { account: Uuid, auth: PasskeyAuthentication },
    /// …then the new passkey.
    AddMake { account: Uuid, reg: PasskeyRegistration },
    /// A new passkey for an account whose passkeys are lost, by one of its
    /// recovery codes (spent only once the passkey is made).
    Recover { account: Uuid, code_sha256: String, reg: PasskeyRegistration },
}

pub struct Passkeys {
    pub config: PasskeysConfig,
    webauthn: Webauthn,
    store: Store,
    flows: Mutex<HashMap<String, (Instant, Flow)>>,
    /// The asks answered (each event's id) and when: replays refused.
    asked: Mutex<HashMap<String, i64>>,
    /// When this door started (seconds): an ask signed before it may have
    /// been answered by the door before, whose memory of it is gone.
    started_s: i64,
}

/// An email as an account takes it: a mailbox, trimmed and lower-cased.
pub fn email_ok(email: &str) -> Option<String> {
    let e = email.trim().to_ascii_lowercase();
    let (local, domain) = e.split_once('@')?;
    let ok = !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.') && e.len() <= EMAIL_BYTES_MAX && e.bytes().all(|b| b.is_ascii_graphic()) && !domain.contains('@');
    ok.then_some(e)
}

/// A verified NIP-98 header's event id and signing time (its id checked
/// against its content by the verifier, so the same ask always has it).
fn ask_id(authorization: Option<&str>) -> Option<(String, i64)> {
    use base64::Engine;
    #[derive(serde::Deserialize)]
    struct Signed {
        id: String,
        created_at: i64,
    }
    let b64 = authorization?.strip_prefix("Nostr ")?.trim();
    let raw = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    let s: Signed = serde_json::from_slice(&raw).ok()?;
    Some((s.id, s.created_at))
}

fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

/// A token for a link or a flow: 32 random bytes, hex.
fn token() -> String {
    hex::encode(random_code())
}

/// A recovery code: 20 characters of Crockford's base 32 (100 bits), shown
/// in groups of four.
fn recovery_code() -> String {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let bytes = random_code();
    let chars: Vec<u8> = bytes.iter().take(20).map(|b| ALPHABET[(b & 31) as usize]).collect();
    chars.chunks(4).map(|c| std::str::from_utf8(c).expect("ASCII")).collect::<Vec<_>>().join("-")
}

/// A recovery code as given (case, spaces and dashes aside), hashed.
fn code_sha256(given: &str) -> String {
    let norm: String = given.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect();
    sha256_hex(&norm)
}

impl Passkeys {
    pub fn new(config: PasskeysConfig) -> Result<Passkeys, PasskeyError> {
        let webauthn = WebauthnBuilder::new(&config.rp_id, &config.origin).map_err(webauthn_error)?.rp_name(&config.rp_name).build().map_err(webauthn_error)?;
        let store = Store::open(&config.state_dir)?;
        let started_s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        Ok(Passkeys { config, webauthn, store, flows: Mutex::new(HashMap::new()), asked: Mutex::new(HashMap::new()), started_s })
    }

    fn put(&self, flow: Flow) -> Result<String, PasskeyError> {
        let mut flows = self.flows.lock().expect("the flows' lock is never poisoned");
        let now = Instant::now();
        flows.retain(|_, (at, _)| now.duration_since(*at) < FLOW_TTL);
        if flows.len() >= FLOWS_MAX {
            return Err(PasskeyError::Refused("too many sign-ins are under way: try again in a minute"));
        }
        let id = token();
        flows.insert(id.clone(), (now, flow));
        assert!(flows.len() <= FLOWS_MAX);
        Ok(id)
    }

    /// The flow `id`, taken: a flow is answered once.
    fn take(&self, id: &str) -> Result<Flow, PasskeyError> {
        let mut flows = self.flows.lock().expect("the flows' lock is never poisoned");
        match flows.remove(id) {
            Some((at, flow)) if at.elapsed() < FLOW_TTL => Ok(flow),
            _ => Err(PasskeyError::Refused("this sign-in expired or was already answered: start again")),
        }
    }

    fn account(&self, state: &State, email: &str) -> Result<Account, PasskeyError> {
        let email = email_ok(email).ok_or(PasskeyError::Refused("that is not an email"))?;
        state.by_email(&email).cloned().ok_or(PasskeyError::Refused("no account here has that email: ask for an invite"))
    }

    fn person(&self, a: &Account) -> Person {
        Person { subject: subject_of(&issuer_of(&self.config.rp_id), &a.id.to_string()), email: a.email.clone() }
    }

    // ---- invites

    /// Makes an invite for `email`, good for `ttl_s`: its link's token (the
    /// door keeps its SHA-256 alone). Refused when `email` has an account.
    pub fn invite(&self, email: &str, ttl_s: i64, now_s: i64) -> Result<String, PasskeyError> {
        let email = email_ok(email).ok_or(PasskeyError::Refused("that is not an email"))?;
        if !(60..=INVITE_TTL_MAX_S).contains(&ttl_s) {
            return Err(PasskeyError::Refused("an invite is good for a minute to a week"));
        }
        let t = token();
        let sha = sha256_hex(&t);
        self.store.change(|st| {
            st.prune(now_s);
            if st.by_email(&email).is_some() {
                return Err(PasskeyError::Refused("that email already has an account: its own passkeys add more"));
            }
            st.invites.retain(|i| i.email != email);
            st.invites.push(Invite { token_sha256: sha.clone(), email: email.clone(), expires_at_s: now_s + ttl_s });
            Ok(())
        })?;
        Ok(t)
    }

    /// An invite the inviter asked for, at `url` (the door's own for
    /// `INVITES_PATH`) with `body` (`{"email": …}`) and its
    /// `authorization` (NIP-98): checked as the inviter's, fresh, signed
    /// after the door started, and answered once; then a new email's
    /// invite, or `HasAccount` for an email with one, nothing made.
    pub fn asked_invite(&self, authorization: Option<&str>, url: &str, body: &[u8], now_s: i64) -> Result<Invited, PasskeyError> {
        let not = |why: &str| PasskeyError::NotAsked(why.to_string());
        let inviter = self.config.inviter.as_deref().ok_or_else(|| not("this door takes no asks for invites"))?;
        let signer = fragment_nip98::verify(authorization, "POST", url, body, now_s, ASK_WINDOW_S).map_err(|e| PasskeyError::NotAsked(e.to_string()))?;
        if signer != inviter {
            return Err(not("not signed by this door's inviter"));
        }
        let (id, created_at) = ask_id(authorization).ok_or_else(|| not("not a NIP-98 event"))?;
        if created_at <= self.started_s {
            return Err(not("asked before this door started: ask again"));
        }
        self.once(&id, now_s)?;
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Ask {
            email: String,
            #[serde(default)]
            nonce: Option<String>,
        }
        let ask: Ask = serde_json::from_slice(body).map_err(|_| PasskeyError::Refused("an ask is {\"email\": …, \"nonce\"?: …}"))?;
        if ask.nonce.as_deref().is_some_and(|n| n.len() > NONCE_CHARS_MAX || !n.bytes().all(|b| b.is_ascii_alphanumeric())) {
            return Err(PasskeyError::Refused("a nonce is at most 64 letters and digits"));
        }
        let email = email_ok(&ask.email).ok_or(PasskeyError::Refused("that is not an email"))?;
        if self.store.read()?.by_email(&email).is_some() {
            return Ok(Invited::HasAccount);
        }
        Ok(Invited::New(self.invite(&email, INVITE_ASKED_TTL_S, now_s)?))
    }

    /// An ask's event `id`, answered the first time alone, while its replay
    /// could still pass the clock.
    fn once(&self, id: &str, now_s: i64) -> Result<(), PasskeyError> {
        let mut asked = self.asked.lock().expect("the asks' lock is never poisoned");
        asked.retain(|_, at| now_s.saturating_sub(*at) <= 2 * ASK_WINDOW_S);
        if asked.contains_key(id) {
            return Err(PasskeyError::NotAsked("this ask was answered already".into()));
        }
        if asked.len() >= ASKS_MAX {
            return Err(PasskeyError::Refused("too many invites asked at once: try again in a minute"));
        }
        asked.insert(id.to_string(), now_s);
        assert!(asked.len() <= ASKS_MAX);
        Ok(())
    }

    /// The email an open invite is for.
    pub fn invite_email(&self, invite: &str, now_s: i64) -> Result<String, PasskeyError> {
        let sha = sha256_hex(invite);
        let st = self.store.read()?;
        st.invites.iter().find(|i| bool::from(i.token_sha256.as_bytes().ct_eq(sha.as_bytes())) && i.expires_at_s > now_s).map(|i| i.email.clone()).ok_or(PasskeyError::Refused("this invite is used, expired, or not this home's"))
    }

    // ---- enrolling: an invite's new account and its first passkey

    pub fn enroll_begin(&self, invite: &str, now_s: i64) -> Result<(String, CreationChallengeResponse), PasskeyError> {
        let email = self.invite_email(invite, now_s)?;
        if self.store.read()?.by_email(&email).is_some() {
            return Err(PasskeyError::Refused("that email already has an account: its own passkeys add more"));
        }
        let account = Uuid::new_v4();
        let (ccr, reg) = self.webauthn.start_passkey_registration(account, &email, &email, None).map_err(webauthn_error)?;
        let flow = self.put(Flow::Enroll { invite_sha256: sha256_hex(invite), email, account, reg })?;
        Ok((flow, ccr))
    }

    /// The new account, made: its recovery codes, to be shown once.
    pub fn enroll_finish(&self, flow: &str, credential: &RegisterPublicKeyCredential, now_s: i64) -> Result<Vec<String>, PasskeyError> {
        let Flow::Enroll { invite_sha256, email, account, reg } = self.take(flow)? else {
            return Err(PasskeyError::Refused("this is not an enrollment: start again"));
        };
        let passkey = self.webauthn.finish_passkey_registration(credential, &reg).map_err(|_| PasskeyError::Refused("that passkey was not made for this enrollment"))?;
        let codes: Vec<String> = (0..RECOVERY_CODES).map(|_| recovery_code()).collect();
        let hashes: Vec<String> = codes.iter().map(|c| code_sha256(c)).collect();
        self.store.change(|st| {
            st.prune(now_s);
            // the invite is spent once, and only for an email still without
            // an account: two enrollments racing on one invite make one
            let at = st.invites.iter().position(|i| i.token_sha256 == invite_sha256 && i.email == email).ok_or(PasskeyError::Refused("this invite is used or expired"))?;
            if st.by_email(&email).is_some() {
                return Err(PasskeyError::Refused("that email already has an account"));
            }
            if st.credential_taken(&passkey) {
                return Err(PasskeyError::Refused("that passkey is another account's"));
            }
            st.invites.remove(at);
            st.accounts.push(Account { id: account, email: email.clone(), created_at_s: now_s, passkeys: vec![passkey.clone()], recovery: hashes.clone() });
            Ok(())
        })?;
        eprintln!("identity-door: {email} enrolled a passkey (an invite spent)");
        Ok(codes)
    }

    // ---- signing in for the platform

    pub fn signin_begin(&self, begun: Begun, email: &str) -> Result<(String, RequestChallengeResponse), PasskeyError> {
        let a = self.account(&self.store.read()?, email)?;
        let (rcr, auth) = self.webauthn.start_passkey_authentication(&a.passkeys).map_err(webauthn_error)?;
        let flow = self.put(Flow::SignIn { begun, account: a.id, auth })?;
        Ok((flow, rcr))
    }

    /// The account's own passkey answered: the person, and the round trip
    /// back to the platform.
    pub fn signin_finish(&self, flow: &str, credential: &PublicKeyCredential) -> Result<(Begun, Person), PasskeyError> {
        let Flow::SignIn { begun, account, auth } = self.take(flow)? else {
            return Err(PasskeyError::Refused("this is not a sign-in: start again"));
        };
        let a = self.prove(account, credential, &auth)?;
        Ok((begun, self.person(&a)))
    }

    /// Checks an assertion of one of `account`'s passkeys, and keeps its
    /// counter: the account as it is now.
    fn prove(&self, account: Uuid, credential: &PublicKeyCredential, auth: &PasskeyAuthentication) -> Result<Account, PasskeyError> {
        let result = self.webauthn.finish_passkey_authentication(credential, auth).map_err(|_| PasskeyError::Refused("that passkey did not answer for this account"))?;
        self.store.change(|st| {
            let a = st.by_id_mut(account).ok_or(PasskeyError::Refused("that account is gone"))?;
            for p in a.passkeys.iter_mut() {
                p.update_credential(&result);
            }
            Ok(a.clone())
        })
    }

    // ---- another passkey: proven by one of the account's own

    pub fn add_begin(&self, email: &str) -> Result<(String, RequestChallengeResponse), PasskeyError> {
        let a = self.account(&self.store.read()?, email)?;
        let (rcr, auth) = self.webauthn.start_passkey_authentication(&a.passkeys).map_err(webauthn_error)?;
        Ok((self.put(Flow::AddProve { account: a.id, auth })?, rcr))
    }

    pub fn add_prove(&self, flow: &str, credential: &PublicKeyCredential) -> Result<(String, CreationChallengeResponse), PasskeyError> {
        let Flow::AddProve { account, auth } = self.take(flow)? else {
            return Err(PasskeyError::Refused("this is not a passkey's approval: start again"));
        };
        let a = self.prove(account, credential, &auth)?;
        let exclude = a.passkeys.iter().map(|p| p.cred_id().clone()).collect();
        let (ccr, reg) = self.webauthn.start_passkey_registration(a.id, &a.email, &a.email, Some(exclude)).map_err(webauthn_error)?;
        Ok((self.put(Flow::AddMake { account: a.id, reg })?, ccr))
    }

    pub fn add_finish(&self, flow: &str, credential: &RegisterPublicKeyCredential) -> Result<(), PasskeyError> {
        let Flow::AddMake { account, reg } = self.take(flow)? else {
            return Err(PasskeyError::Refused("this is not a new passkey: start again"));
        };
        let passkey = self.webauthn.finish_passkey_registration(credential, &reg).map_err(|_| PasskeyError::Refused("that passkey was not made for this account"))?;
        self.keep(account, passkey, None)
    }

    // ---- a lost account: a recovery code makes a new passkey

    pub fn recover_begin(&self, email: &str, code: &str) -> Result<(String, CreationChallengeResponse), PasskeyError> {
        let a = self.account(&self.store.read()?, email)?;
        let sha = code_sha256(code);
        if !a.recovery.iter().any(|h| bool::from(h.as_bytes().ct_eq(sha.as_bytes()))) {
            return Err(PasskeyError::Refused("that recovery code is not this account's, or was used"));
        }
        let exclude = a.passkeys.iter().map(|p| p.cred_id().clone()).collect();
        let (ccr, reg) = self.webauthn.start_passkey_registration(a.id, &a.email, &a.email, Some(exclude)).map_err(webauthn_error)?;
        Ok((self.put(Flow::Recover { account: a.id, code_sha256: sha, reg })?, ccr))
    }

    pub fn recover_finish(&self, flow: &str, credential: &RegisterPublicKeyCredential) -> Result<(), PasskeyError> {
        let Flow::Recover { account, code_sha256, reg } = self.take(flow)? else {
            return Err(PasskeyError::Refused("this is not a recovery: start again"));
        };
        let passkey = self.webauthn.finish_passkey_registration(credential, &reg).map_err(|_| PasskeyError::Refused("that passkey was not made for this recovery"))?;
        self.keep(account, passkey, Some(code_sha256))
    }

    /// Adds `passkey` to `account`, spending `code` (a recovery's) with it.
    fn keep(&self, account: Uuid, passkey: Passkey, code: Option<String>) -> Result<(), PasskeyError> {
        let email = self.store.change(|st| {
            if st.credential_taken(&passkey) {
                return Err(PasskeyError::Refused("that passkey is already an account's"));
            }
            let a = st.by_id_mut(account).ok_or(PasskeyError::Refused("that account is gone"))?;
            if let Some(code) = &code {
                let at = a.recovery.iter().position(|h| h == code).ok_or(PasskeyError::Refused("that recovery code was used meanwhile"))?;
                a.recovery.remove(at);
            }
            if a.passkeys.len() >= store::PASSKEYS_MAX {
                return Err(PasskeyError::Refused("this account holds as many passkeys as it may"));
            }
            a.passkeys.push(passkey.clone());
            Ok(a.email.clone())
        })?;
        eprintln!("identity-door: {email} added a passkey ({})", if code.is_some() { "by a recovery code" } else { "approved by its own" });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The door's inviter, and a key that is not it.
    fn inviter() -> fragment_nip98::Keys {
        fragment_nip98::Keys::from_secret_hex(&"11".repeat(32)).unwrap()
    }
    fn stranger() -> fragment_nip98::Keys {
        fragment_nip98::Keys::from_secret_hex(&"22".repeat(32)).unwrap()
    }
    const ASK_URL: &str = "https://home.example.com/__door/passkeys/invites";

    fn clock() -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
    }

    /// An ask for `email`, signed by `by` at `at`, and its body.
    fn ask(by: &fragment_nip98::Keys, email: &str, at: i64) -> (String, Vec<u8>) {
        let body = serde_json::to_vec(&serde_json::json!({ "email": email })).unwrap();
        (by.header("POST", ASK_URL, &body, at), body)
    }

    fn door(name: &str) -> (Passkeys, PathBuf) {
        let d = std::env::temp_dir().join(format!("door-passkeys-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let p = Passkeys::new(PasskeysConfig { rp_id: "home.example.com".into(), origin: Url::parse("https://home.example.com").unwrap(), rp_name: "Fragment".into(), state_dir: d.clone(), inviter: Some(inviter().pubkey_hex().to_string()) }).unwrap();
        (p, d)
    }

    // Goal: emails are mailboxes, trimmed and lower-cased; anything else
    // is refused.
    #[test]
    fn emails_are_mailboxes() {
        assert_eq!(email_ok(" Ada@Example.COM ").as_deref(), Some("ada@example.com"));
        for bad in ["", "ada", "ada@", "@example.com", "ada@localhost", "ada@example.", "a b@example.com", "a@b@example.com", &format!("{}@example.com", "a".repeat(250))] {
            assert_eq!(email_ok(bad), None, "{bad:?}");
        }
    }

    // Goal: an invite opens once for its email, within its life; an
    // invite's link is never kept (only its hash); an email with an account
    // gets none. Replay: a second invite for the same email replaces the
    // first. Restart: a door opened again on the state reads it.
    #[test]
    fn invites_are_one_email_one_life() {
        let (p, d) = door("invites");
        let t = p.invite("bo@example.com", 3600, 1_000).unwrap();
        assert_eq!(p.invite_email(&t, 1_000).unwrap(), "bo@example.com");
        assert!(p.invite_email(&t, 1_000 + 3600).is_err(), "expired at its end");
        assert!(p.invite_email(&"0".repeat(64), 1_000).is_err());
        let state = std::fs::read_to_string(d.join("accounts.json")).unwrap();
        assert!(!state.contains(&t) && state.contains(&sha256_hex(&t)), "the token's hash alone is kept");
        let t2 = p.invite("BO@example.com", 3600, 1_001).unwrap();
        assert!(p.invite_email(&t, 1_001).is_err(), "the newer invite replaces the older");
        let again = Passkeys::new(p.config.clone()).unwrap();
        assert_eq!(again.invite_email(&t2, 1_002).unwrap(), "bo@example.com");
        assert!(p.invite("x@example.com", 30, 1_000).is_err() && p.invite("x@example.com", INVITE_TTL_MAX_S + 1, 1_000).is_err());
        p.store
            .change(|st| -> Result<(), PasskeyError> {
                st.accounts.push(Account { id: Uuid::new_v4(), email: "cy@example.com".into(), created_at_s: 0, passkeys: vec![], recovery: vec![] });
                Ok(())
            })
            .unwrap();
        assert!(matches!(p.invite("cy@example.com", 3600, 1_000), Err(PasskeyError::Refused(_))), "no invite into an existing account");
        std::fs::remove_dir_all(&d).unwrap();
    }

    // Goal: the inviter's ask makes a new email's invite, good for an hour,
    // and gets an email with an account "sign in", nothing made; no ask
    // that is not the inviter's, fresh and first makes anything. Replay:
    // the same ask twice is answered once. Restart: an ask signed before
    // the door started is refused (its memory of asks is gone).
    #[test]
    fn asked_invites_are_the_inviters_alone() {
        let (p, d) = door("asked");
        let now = clock() + 1;
        let (h, body) = ask(&inviter(), "Fay@Example.com", now);
        let Ok(Invited::New(t)) = p.asked_invite(Some(&h), ASK_URL, &body, now) else { panic!("a new email's invite") };
        assert_eq!(p.invite_email(&t, now).unwrap(), "fay@example.com");
        assert!(p.invite_email(&t, now + INVITE_ASKED_TTL_S).is_err(), "an hour, no more");
        assert!(matches!(p.asked_invite(Some(&h), ASK_URL, &body, now), Err(PasskeyError::NotAsked(_))), "a replay");
        let nonced = br#"{"email":"fay@example.com","nonce":"0a1b2c"}"#;
        let h = inviter().header("POST", ASK_URL, nonced, now);
        assert!(matches!(p.asked_invite(Some(&h), ASK_URL, nonced, now), Ok(Invited::New(_))), "the same second, another nonce: another ask");
        let long = format!(r#"{{"email":"fay@example.com","nonce":"{}"}}"#, "a".repeat(NONCE_CHARS_MAX + 1));
        let h = inviter().header("POST", ASK_URL, long.as_bytes(), now);
        assert!(matches!(p.asked_invite(Some(&h), ASK_URL, long.as_bytes(), now), Err(PasskeyError::Refused(_))), "a nonce too long");

        p.store
            .change(|st| -> Result<(), PasskeyError> {
                st.accounts.push(Account { id: Uuid::new_v4(), email: "gil@example.com".into(), created_at_s: 0, passkeys: vec![], recovery: vec![] });
                Ok(())
            })
            .unwrap();
        let (h, body) = ask(&inviter(), "gil@example.com", now);
        assert_eq!(p.asked_invite(Some(&h), ASK_URL, &body, now).unwrap(), Invited::HasAccount);
        assert!(!p.store.read().unwrap().invites.iter().any(|i| i.email == "gil@example.com"), "no invite into an existing account");

        let refused = |h: Option<&str>, url: &str, body: &[u8]| matches!(p.asked_invite(h, url, body, now), Err(PasskeyError::NotAsked(_)));
        let (h, body) = ask(&stranger(), "hal@example.com", now);
        assert!(refused(Some(&h), ASK_URL, &body), "another key");
        let (h, body) = ask(&inviter(), "hal@example.com", now - ASK_WINDOW_S - 1);
        assert!(refused(Some(&h), ASK_URL, &body), "stale");
        let (h, body) = ask(&inviter(), "hal@example.com", now);
        assert!(refused(Some(&h), "https://home.example.com/__door/passkeys/other", &body), "another URL");
        assert!(refused(Some(&h), ASK_URL, br#"{"email":"ivy@example.com"}"#), "another body");
        assert!(refused(None, ASK_URL, &body), "unsigned");
        let (h, body) = ask(&inviter(), "hal@example.com", p.started_s);
        assert!(refused(Some(&h), ASK_URL, &body), "signed before the door started");
        let bad = br#"{"email":"hal@example.com","more":1}"#;
        let h2 = inviter().header("POST", ASK_URL, bad, now);
        assert!(matches!(p.asked_invite(Some(&h2), ASK_URL, bad, now), Err(PasskeyError::Refused(_))), "an ask is its email alone");
        assert!(!p.store.read().unwrap().invites.iter().any(|i| i.email.starts_with("hal") || i.email.starts_with("ivy")), "none of those made one");

        let mut no_inviter = p.config.clone();
        no_inviter.inviter = None;
        let q = Passkeys::new(no_inviter).unwrap();
        let (h, body) = ask(&inviter(), "jo@example.com", clock() + 1);
        assert!(matches!(q.asked_invite(Some(&h), ASK_URL, &body, clock() + 1), Err(PasskeyError::NotAsked(_))), "a door with no inviter takes no asks");
        std::fs::remove_dir_all(&d).unwrap();
    }

    // Goal: the asks remembered are bounded, and forgotten once their
    // replay could no longer pass the clock.
    #[test]
    fn asks_remembered_are_bounded() {
        let (p, d) = door("asks-bounded");
        let now = clock() + 1;
        for i in 0..ASKS_MAX {
            p.once(&format!("{i:064x}"), now).unwrap();
        }
        assert!(matches!(p.once(&"f".repeat(64), now), Err(PasskeyError::Refused(_))), "full");
        assert!(p.once(&"f".repeat(64), now + 2 * ASK_WINDOW_S + 1).is_ok(), "the old ones forgotten");
        std::fs::remove_dir_all(&d).unwrap();
    }

    // Goal: a flow is answered once, and not past its life; a flow of one
    // kind does not answer another's step.
    #[test]
    fn flows_are_single_use() {
        let (p, d) = door("flows");
        let t = p.invite("dee@example.com", 3600, 1_000).unwrap();
        let (flow, ccr) = p.enroll_begin(&t, 1_000).unwrap();
        assert_eq!(ccr.public_key.rp.id, "home.example.com");
        assert!(matches!(p.add_finish(&flow, &dummy_registration()), Err(PasskeyError::Refused(_))), "an enrollment's flow is no other step's");
        assert!(matches!(p.take(&flow), Err(PasskeyError::Refused(_))), "and it was spent by the wrong step");
        let (flow, _) = p.enroll_begin(&t, 1_000).unwrap();
        assert!(matches!(p.enroll_finish(&flow, &dummy_registration(), 1_000), Err(PasskeyError::Refused(_))), "a credential this enrollment never asked for");
        assert!(p.store.read().unwrap().accounts.is_empty(), "nothing made");
        assert!(p.invite_email(&t, 1_000).is_ok(), "and the invite is not spent");
        std::fs::remove_dir_all(&d).unwrap();
    }

    // Goal: sign-in, another passkey and recovery are refused for an email
    // with no account, and recovery for a code not the account's.
    #[test]
    fn strangers_get_no_challenge() {
        let (p, d) = door("strangers");
        let begun = Begun { client_id: "c".into(), callback: 0, client_state: None, at_s: 0, salt: "s".into() };
        assert!(matches!(p.signin_begin(begun, "nobody@example.com"), Err(PasskeyError::Refused(_))));
        assert!(matches!(p.add_begin("nobody@example.com"), Err(PasskeyError::Refused(_))));
        p.store
            .change(|st| -> Result<(), PasskeyError> {
                st.accounts.push(Account { id: Uuid::new_v4(), email: "ed@example.com".into(), created_at_s: 0, passkeys: vec![], recovery: vec![code_sha256("AAAA-BBBB-CCCC-DDDD-EEEE")] });
                Ok(())
            })
            .unwrap();
        assert!(matches!(p.recover_begin("ed@example.com", "AAAA-BBBB-CCCC-DDDD-EEEF"), Err(PasskeyError::Refused(_))));
        assert!(p.recover_begin("ed@example.com", "aaaa bbbb cccc dddd eeee").is_ok(), "a code as typed, case and spaces aside");
        std::fs::remove_dir_all(&d).unwrap();
    }

    // Goal: recovery codes are 100 bits each, distinct, in groups of four.
    #[test]
    fn recovery_codes_are_strong_and_distinct() {
        let codes: Vec<String> = (0..RECOVERY_CODES).map(|_| recovery_code()).collect();
        for c in &codes {
            assert_eq!(c.len(), 24);
            assert_eq!(c.split('-').count(), 5);
        }
        let mut sorted = codes.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), codes.len());
        assert_eq!(code_sha256(&codes[0]), code_sha256(&codes[0].to_lowercase().replace('-', " ")));
    }

    fn dummy_registration() -> RegisterPublicKeyCredential {
        serde_json::from_value(serde_json::json!({
            "id": "AAAA", "rawId": "AAAA", "type": "public-key",
            "response": { "attestationObject": "AAAA", "clientDataJSON": "AAAA" }
        }))
        .unwrap()
    }
}
