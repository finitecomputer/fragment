//! An agent's SimpleX door, the bridge's half (the platform's is
//! crates/core/src/door.rs, whose digests these are; docs/bridge.md,
//! "SimpleX"; Paul's design of 2026-10-23):
//!
//! - **A ticket** starts a link or an unlink: the platform makes one at the
//!   agent's owner's ask alone, from their Settings, signed as the agent
//!   fragment's key, which the bridge holds from its view of the agent
//!   (`key`). The bridge takes one only for this computer and that agent,
//!   while it holds, once (`check`; the nonces it took are in the link's
//!   file). Neither the agent nor anyone else on the computer can make one.
//! - **The door's key** is made for each link (`Key::generate`), its
//!   private half in `door.key` beside the link's state, readable by the
//!   bridge's user alone (0600, in a 0700 directory) and in no process's
//!   environment; its public half is registered at the platform with the
//!   link's ticket, and let go there the moment the owner unlinks.
//! - **What the owner says over SimpleX** is posted to their chat with the
//!   agent as a relayed record the door's key signs (`Key::seal`), and the
//!   platform takes no relayed record without that signature, so the
//!   agent's own key, which its tools use, cannot speak as its owner.
//!
//! What it does not hold off: a process as the bridge's user (today, root,
//! which the agent's tools run as too) can read the key's file. The ledger
//! says so (docs/explorations/simplex-complexity.md, "The door").

use std::io::{Read, Write};
use std::path::Path;

use k256::schnorr::{Signature, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The door SimpleX is: a relayed record's `via`, a ticket's `door`.
pub const DOOR: &str = "simplex";
/// The door key's file, in the agent's SimpleX directory.
pub const KEY_FILE: &str = "door.key";

/// A ticket, as the owner's page hands it on (fragment_core::door::Ticket).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ticket {
    pub door: String,
    /// `link` or `unlink`.
    pub act: String,
    pub agent: String,
    pub computer: String,
    pub nonce: String,
    pub exp: i64,
    pub sig: String,
}

fn hex_of(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `s` (lowercase hex of `N` bytes) as bytes.
fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
    if !hex_of(s, N * 2) {
        return None;
    }
    let mut out = [0u8; N];
    for (i, pair) in s.as_bytes().chunks(2).enumerate() {
        out[i] = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(out)
}

/// Whether `sig_hex` is `key_hex`'s BIP-340 signature of `digest`.
pub fn verify(key_hex: &str, digest: &[u8; 32], sig_hex: &str) -> bool {
    let (Some(key), Some(sig)) = (unhex::<32>(key_hex), unhex::<64>(sig_hex)) else { return false };
    let (Ok(key), Ok(sig)) = (VerifyingKey::from_bytes(&key), Signature::try_from(sig.as_slice())) else { return false };
    key.verify_raw(digest, &sig).is_ok()
}

impl Ticket {
    /// Its fields' shapes (the platform's `well_formed`): none can hold a
    /// line break, so its digest reads one way only.
    fn well_formed(&self) -> bool {
        let computer = self.computer.strip_prefix("computer:").is_some_and(|h| !h.is_empty() && h.len() <= 64 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        self.door == DOOR && matches!(self.act.as_str(), "link" | "unlink") && super::run::dir_name_ok(&self.agent) && computer && hex_of(&self.nonce, 32) && self.exp > 0 && hex_of(&self.sig, 128)
    }

    /// What its signature signs (fragment_core::door::Ticket::digest).
    pub fn digest(&self) -> [u8; 32] {
        let text = format!("fragment door ticket 1\n{}\n{}\n{}\n{}\n{}\n{}", self.door, self.act, self.agent, self.computer, self.nonce, self.exp);
        Sha256::digest(text.as_bytes()).into()
    }
}

/// Whether `t` starts `act` for `agent` on `computer` now (`now_ms`),
/// signed by `key` (the agent's, 64 hex): why not, in plain words. Whether
/// it was taken before is the caller's (the link's file keeps its nonces).
pub fn check(t: &Ticket, act: &str, agent: &str, computer: &str, key: &str, now_ms: i64) -> Result<(), &'static str> {
    if !t.well_formed() {
        return Err("that is not a ticket from your Settings");
    }
    if t.act != act || t.agent != agent || t.computer != computer {
        return Err("that ticket is for something else: start again from your Settings");
    }
    if t.exp < now_ms {
        return Err("that ticket ran out: start again from your Settings");
    }
    if !verify(key, &t.digest(), &t.sig) {
        return Err("that ticket is not signed by your platform: start again from your Settings");
    }
    Ok(())
}

/// A door's key.
#[derive(Clone)]
pub struct Key(SigningKey);

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Key({})", self.public_hex())
    }
}

impl Key {
    /// A new one, from the system's randomness.
    pub fn generate() -> std::io::Result<Key> {
        let mut urandom = std::fs::File::open("/dev/urandom")?;
        // bounded: a 32-byte string is no key with odds of about 2^-128
        for _ in 0..4 {
            let mut secret = [0u8; 32];
            urandom.read_exact(&mut secret)?;
            if let Ok(k) = SigningKey::from_bytes(&secret) {
                return Ok(Key(k));
            }
        }
        Err(std::io::Error::other("no key from the system's randomness"))
    }

    pub fn public_hex(&self) -> String {
        crate::records::hex(&self.0.verifying_key().to_bytes())
    }

    /// Its BIP-340 signature of `digest`, with no auxiliary randomness (the
    /// platform signs so too).
    fn sign(&self, digest: &[u8; 32]) -> String {
        crate::records::hex(&self.0.sign_raw(digest, &[0u8; 32]).expect("BIP-340 signing a 32-byte digest").to_bytes())
    }

    /// The key in `dir`'s `door.key`, when there is one that reads.
    pub fn load(dir: &Path) -> Option<Key> {
        let text = std::fs::read_to_string(dir.join(KEY_FILE)).ok()?;
        SigningKey::from_bytes(&unhex::<32>(text.trim())?).ok().map(Key)
    }

    /// Writes it to `dir`'s `door.key` whole, readable by this user alone
    /// (0600, the directory 0700).
    pub fn store(&self, dir: &Path) -> std::io::Result<()> {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        std::fs::create_dir_all(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        let tmp = dir.join(format!("{KEY_FILE}.tmp"));
        let _ = std::fs::remove_file(&tmp);
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        f.write_all(format!("{}\n", crate::records::hex(&self.0.to_bytes())).as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, dir.join(KEY_FILE))
    }

    /// `dir`'s `door.key` gone (an unlink's).
    pub fn forget(dir: &Path) {
        let _ = std::fs::remove_file(dir.join(KEY_FILE));
    }

    /// Signs a relayed record about to be posted as `id` on `fragment`'s
    /// `channel`: its `relayed` gains `at` (now) and `sig`. Signed again at
    /// each try, so it is always fresh; a try after one that was taken is
    /// another body under the same id, which the platform answers 409.
    pub fn seal(&self, body: &mut Value, fragment: &str, channel: &str, id: &str, now_ms: i64) {
        assert!(body["relayed"].is_object(), "only a relayed record is sealed");
        body["relayed"]["at"] = Value::from(now_ms);
        let digest = record_digest(DOOR, fragment, channel, id, body);
        body["relayed"]["sig"] = Value::from(self.sign(&digest));
    }
}

/// What a door signs a relayed record by (fragment_core::door::record_digest):
/// where it is posted, and its body as canonical JSON less `relayed.sig`.
pub fn record_digest(door: &str, fragment: &str, channel: &str, id: &str, body: &Value) -> [u8; 32] {
    let mut unsigned = body.clone();
    if let Some(r) = unsigned.get_mut("relayed").and_then(Value::as_object_mut) {
        r.remove("sig");
    }
    let mut text = format!("fragment door record 1\n{door}\n{fragment}\n{channel}\n{id}\n");
    canonical(&unsigned, &mut text);
    Sha256::digest(text.as_bytes()).into()
}

/// `v` as JSON with each object's keys in order and no space.
fn canonical(v: &Value, out: &mut String) {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                canonical(&m[k], out);
            }
            out.push('}');
        }
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(x, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // crates/core/src/door.rs's vectors: the platform computes these too
    const TICKET_DIGEST: &str = "fc329f82d4dd4f6c73aaf32e55e0863b13a5de24b9b64ad7e3f2897e985d11e7";
    const RECORD_DIGEST: &str = "6f579428588821dc78f7f0ed9ea310980bafd4857b900ce164c03e732ccb48f4";
    const TICKET_SIG: &str = "979566bc3f8beebe92cd33addd30ba2852132ce15b7b57088ac95075433a192795e10ede4bcdc1ef45605ba0967481c4390f1027b223b5f8c1e131c8cee4f57b";
    const NOW: i64 = 1_790_000_000_000;

    /// The agent's key of the vectors (secret 0x07 × 32).
    fn agent_key() -> Key {
        Key(SigningKey::from_bytes(&[7u8; 32]).expect("a key"))
    }

    fn ticket() -> Ticket {
        Ticket { door: DOOR.into(), act: "link".into(), agent: "juniper--k3x9".into(), computer: "computer:00aa".into(), nonce: "0f".repeat(16), exp: 1_790_000_600_000, sig: TICKET_SIG.into() }
    }

    /// Goal: the bridge's digests and signatures are the platform's, to the
    /// byte (the same vectors as crates/core/src/door.rs).
    #[test]
    fn the_digests_are_the_platforms() {
        assert_eq!(crate::records::hex(&ticket().digest()), TICKET_DIGEST);
        let record = json!({ "text": "hi from my phone", "to": ["npub1juniper"], "relayed": { "via": "simplex", "for": "npub1paul", "agent": "juniper--k3x9", "at": NOW } });
        assert_eq!(crate::records::hex(&record_digest(DOOR, "juniper-chat--a1b2", "chat", "sx:0123456789abcdef0123456789abcdef", &record)), RECORD_DIGEST);
        assert_eq!(agent_key().sign(&ticket().digest()), TICKET_SIG);
    }

    /// Goal (the signed link start): a ticket starts its act for its agent
    /// on its computer while it holds, signed by the agent's key; one
    /// unsigned, forged, another's, or past its time is refused.
    #[test]
    fn only_a_signed_ticket_starts_anything() {
        let key = agent_key().public_hex();
        let ok = |t: &Ticket| check(t, "link", "juniper--k3x9", "computer:00aa", &key, NOW);
        assert_eq!(ok(&ticket()), Ok(()));
        assert!(ok(&Ticket { sig: String::new(), ..ticket() }).is_err(), "unsigned");
        assert!(ok(&Ticket { sig: "ab".repeat(64), ..ticket() }).is_err(), "a signature that is no one's");
        let stranger = Key::generate().expect("a key");
        assert!(ok(&Ticket { sig: stranger.sign(&ticket().digest()), ..ticket() }).is_err(), "signed by another key (the agent's tools hold none of its fragment's)");
        assert!(ok(&Ticket { nonce: "1f".repeat(16), ..ticket() }).is_err(), "its fields changed under its signature");
        assert!(check(&ticket(), "unlink", "juniper--k3x9", "computer:00aa", &key, NOW).is_err(), "a link's ticket starts no unlink");
        assert!(check(&ticket(), "link", "maple--k3x9", "computer:00aa", &key, NOW).is_err(), "another agent's");
        assert!(check(&ticket(), "link", "juniper--k3x9", "computer:00bb", &key, NOW).is_err(), "another computer's");
        assert!(check(&ticket(), "link", "juniper--k3x9", "computer:00aa", &key, 1_790_000_600_001).is_err(), "past its time");
        assert!(check(&ticket(), "link", "juniper--k3x9", "computer:00aa", "xyz", NOW).is_err(), "no key to check it with");
    }

    /// Goal: a sealed record verifies by its door's key over where it is
    /// posted and what it says; changed, moved, or re-signed by another key
    /// it does not.
    #[test]
    fn a_sealed_record_is_its_doors() {
        let door = Key::generate().expect("a key");
        let mut body = json!({ "text": "yes", "to": ["npub1juniper"], "relayed": { "via": "simplex", "for": "npub1paul", "agent": "juniper--k3x9" } });
        door.seal(&mut body, "juniper-chat--a1b2", "chat", "sx:1", NOW);
        assert_eq!(body["relayed"]["at"], NOW);
        let sig = body["relayed"]["sig"].as_str().expect("signed").to_string();
        let at = |fragment: &str, id: &str, b: &Value| verify(&door.public_hex(), &record_digest(DOOR, fragment, "chat", id, b), &sig);
        assert!(at("juniper-chat--a1b2", "sx:1", &body));
        assert!(!at("juniper-chat--a1b2", "sx:2", &body), "another id");
        assert!(!at("other-chat--a1b2", "sx:1", &body), "another fragment");
        let mut changed = body.clone();
        changed["text"] = json!("no");
        assert!(!at("juniper-chat--a1b2", "sx:1", &changed), "another text");
        let other = Key::generate().expect("a key");
        assert!(!verify(&other.public_hex(), &record_digest(DOOR, "juniper-chat--a1b2", "chat", "sx:1", &body), &sig));
    }

    /// Goal: the door's key is kept readable by its user alone, reads back
    /// as itself, and is gone once forgotten.
    #[test]
    fn the_key_is_kept_tight() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("simplex-door-{}", std::process::id())).join("juniper--k3x9");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(Key::load(&dir).is_none());
        let k = Key::generate().expect("a key");
        k.store(&dir).expect("kept");
        let mode = |p: &Path| std::fs::metadata(p).expect("there").permissions().mode() & 0o777;
        assert_eq!((mode(&dir.join(KEY_FILE)), mode(&dir)), (0o600, 0o700));
        assert_eq!(Key::load(&dir).map(|l| l.public_hex()), Some(k.public_hex()));
        Key::forget(&dir);
        assert!(Key::load(&dir).is_none());
        let _ = std::fs::remove_dir_all(dir.parent().expect("its root"));
    }
}
