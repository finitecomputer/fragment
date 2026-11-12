//! The door's state at the provider: what a sign-in began with (its
//! client, the callback to send the browser back to, the platform's own
//! state, when), sealed with an HMAC under a key derived from the door's
//! secret, so the door keeps nothing between `authorize` and its callback.
//! A door restarted in the middle opens it all the same; one whose secret
//! changed does not, and the person starts again.
//!
//! The browser carries it, so it is not secret (a client id, a registered
//! callback, the platform's state: all in the browser's address bar
//! already), only unforgeable. The PKCE verifier and the nonce are derived
//! from it (`oidc::SignInSecrets`), so only the door can redeem the code
//! the provider issues for it. Its binding to one browser is the
//! platform's: the platform's state, handed back, finishes a sign-in only
//! in the browser whose cookie holds it.

use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::oidc::{b64url, SECRET_MIN_BYTES};

/// How long a sign-in at the provider may take: the platform's own
/// round trip lasts ten minutes (`roundtrip::TTL_S`).
pub const STATE_TTL_S: i64 = 600;
/// A sealed state at most (its parts are bounded: a client id, an index,
/// the platform's state of at most 1 KiB).
pub const STATE_BYTES_MAX: usize = 2048;

/// What a sign-in began with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Begun {
    /// The client's id.
    #[serde(rename = "c")]
    pub client_id: String,
    /// Which of the client's registered callbacks.
    #[serde(rename = "r")]
    pub callback: u8,
    /// The platform's own state, handed back to its callback.
    #[serde(rename = "s", skip_serializing_if = "Option::is_none", default)]
    pub client_state: Option<String>,
    /// When it began (seconds).
    #[serde(rename = "t")]
    pub at_s: i64,
    /// Random bytes of its own, so no two sign-ins share a state, and so
    /// no two share a verifier or a nonce.
    #[serde(rename = "n")]
    pub salt: String,
}

/// Why a state signs no one in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StateError {
    #[error("this sign-in's state is malformed")]
    Malformed,
    #[error("this sign-in's state is not the door's (or the door's secret changed since it began)")]
    Forged,
    #[error("this sign-in began more than {} minutes ago", STATE_TTL_S / 60)]
    Expired,
}

/// The key states are sealed with, derived from the door's secret.
#[derive(Clone)]
pub struct StateKey([u8; 32]);

impl StateKey {
    pub fn of(secret: &[u8]) -> StateKey {
        assert!(secret.len() >= SECRET_MIN_BYTES, "the door's secret is checked at start");
        let mut key = [0u8; 32];
        Hkdf::<Sha256>::new(Some(b"fragment identity door state"), secret).expand(b"v1", &mut key).expect("32 bytes is a valid HKDF-SHA256 length");
        StateKey(key)
    }

    fn mac(&self) -> Hmac<Sha256> {
        <Hmac<Sha256> as Mac>::new_from_slice(&self.0).expect("HMAC takes any key")
    }

    /// `begun`, sealed: its JSON in base64url, a dot, its HMAC.
    pub fn seal(&self, begun: &Begun) -> String {
        let body = b64url(serde_json::to_string(begun).expect("a state serializes").as_bytes());
        let mut mac = self.mac();
        mac.update(body.as_bytes());
        let sealed = format!("{body}.{}", b64url(&mac.finalize().into_bytes()));
        assert!(sealed.len() <= STATE_BYTES_MAX, "a state's parts are bounded");
        sealed
    }

    /// What a sealed state began with, if the door sealed it and it is not
    /// past `STATE_TTL_S` at `now_s`.
    pub fn open(&self, sealed: &str, now_s: i64) -> Result<Begun, StateError> {
        use base64::Engine;
        if sealed.len() > STATE_BYTES_MAX {
            return Err(StateError::Malformed);
        }
        let Some((body, tag)) = sealed.split_once('.') else { return Err(StateError::Malformed) };
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let Ok(tag) = b64.decode(tag) else { return Err(StateError::Malformed) };
        let mut mac = self.mac();
        mac.update(body.as_bytes());
        // constant time
        if mac.verify_slice(&tag).is_err() {
            return Err(StateError::Forged);
        }
        let begun: Begun = b64.decode(body).ok().and_then(|json| serde_json::from_slice(&json).ok()).ok_or(StateError::Malformed)?;
        let age = now_s.saturating_sub(begun.at_s);
        if !(0..STATE_TTL_S).contains(&age) {
            return Err(StateError::Expired);
        }
        Ok(begun)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"a door secret of at least thirty-two bytes, for tests";

    fn begun() -> Begun {
        Begun { client_id: "client_cell".into(), callback: 0, client_state: Some("a".repeat(64)), at_s: 1_000, salt: "00ff".into() }
    }

    /// Goal: a state the door sealed opens, after a restart too (the same
    /// secret); a changed byte, another secret's, or one past its time
    /// does not; nor does anything else.
    #[test]
    fn a_state_opens_only_as_sealed() {
        let key = StateKey::of(SECRET);
        let sealed = key.seal(&begun());
        assert_eq!(StateKey::of(SECRET).open(&sealed, 1_000 + STATE_TTL_S - 1), Ok(begun()), "a restarted door opens it");
        assert_eq!(key.open(&sealed, 1_000 + STATE_TTL_S), Err(StateError::Expired));
        assert_eq!(key.open(&sealed, 999), Err(StateError::Expired), "from the future");
        assert_eq!(StateKey::of(b"another door secret of thirty-two bytes or more").open(&sealed, 1_000), Err(StateError::Forged));
        let (body, tag) = sealed.split_once('.').unwrap();
        let other = key.seal(&Begun { client_id: "client_other".into(), ..begun() });
        let (other_body, _) = other.split_once('.').unwrap();
        assert_eq!(key.open(&format!("{other_body}.{tag}"), 1_000), Err(StateError::Forged), "another body under this tag");
        assert_eq!(key.open(&format!("{body}.{}", "A".repeat(43)), 1_000), Err(StateError::Forged));
        assert_eq!(key.open("nothing", 1_000), Err(StateError::Malformed));
        assert_eq!(key.open(&"a".repeat(STATE_BYTES_MAX + 1), 1_000), Err(StateError::Malformed));
    }
}
