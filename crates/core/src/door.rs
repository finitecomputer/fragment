//! An agent's doors (docs/api.md, "An agent's doors"): ways its owner
//! reaches it other than its chat, run by its computer (SimpleX, decision
//! 32; Paul's design of 2026-10-23). Only its owner opens one, and only
//! what came through it speaks for them; the agent itself can do neither.
//!
//! - **A ticket** starts a link or an unlink. The platform makes one at
//!   its owner's ask alone (from Settings), for one agent on one computer,
//!   for `TICKET_MS`, and signs it as the agent fragment's own key; the
//!   computer's bridge checks it with that key (its view's `key` for the
//!   agent) and takes each once. A link's ticket is redeemed at the
//!   platform once too, with the door's public key.
//! - **The door's key** is the bridge's, made for each link, its private
//!   half in a file of the bridge's own. The platform holds the public half
//!   in the agent fragment, and lets it go the moment its owner unlinks.
//! - **A relayed record** (what its owner said through the door, carried to
//!   their chat with it: docs/chat-records.md) is signed by the door's key
//!   over where it is posted and all it says (`record_digest`). The
//!   platform takes one only as a post by that agent, for its owner, so
//!   signed, signed lately (`SKEW_MS`); an app publishes none
//!   (`effects::check_record`).
//!
//! The digests are tagged (`fragment door …`), so no signature made for
//! one is another's, nor a NIP-98 event's. The bridge computes the same
//! ones (images/bridge/src/simplex/door.rs); the vectors in the tests
//! below hold both to them.

use fragment_proto::valid_fragment_name;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The door SimpleX is (a relayed record's `via`).
pub const SIMPLEX: &str = "simplex";

/// Whether `door` is one there is.
pub fn known(door: &str) -> bool {
    door == SIMPLEX
}

/// A ticket holds this long after it is made: its owner's page hands it
/// to the bridge at once.
pub const TICKET_MS: i64 = 10 * 60_000;

/// A relayed record's `at` (when its door signed it) is at most this far
/// from the platform's clock as it is posted: a signed record copied from
/// the chat later is no message of its owner's again.
pub const SKEW_MS: i64 = 10 * 60_000;

/// What a ticket starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Act {
    Link,
    Unlink,
}

impl Act {
    pub fn as_str(self) -> &'static str {
        match self {
            Act::Link => "link",
            Act::Unlink => "unlink",
        }
    }
}

/// A ticket, as the platform hands its owner's page one and the page hands
/// it to the bridge (docs/api.md, "An agent's doors").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ticket {
    pub door: String,
    pub act: Act,
    /// The agent fragment.
    pub agent: String,
    pub computer: String,
    /// 32 hex, the platform's randomness.
    pub nonce: String,
    /// When it stops holding, ms since the epoch.
    pub exp: i64,
    /// The agent fragment's key's BIP-340 signature of `digest` (128 hex).
    #[serde(default)]
    pub sig: String,
}

fn hex_of(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A computer's id: `computer:` and lowercase hex.
pub fn computer_ok(id: &str) -> bool {
    id.strip_prefix("computer:").is_some_and(|h| !h.is_empty() && h.len() <= 64 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

impl Ticket {
    /// Its fields' shapes. None can hold a line break, so its digest reads
    /// one way only.
    pub fn well_formed(&self) -> bool {
        known(&self.door) && valid_fragment_name(&self.agent) && computer_ok(&self.computer) && hex_of(&self.nonce, 32) && self.exp > 0 && (self.sig.is_empty() || hex_of(&self.sig, 128))
    }

    /// What its signature signs.
    pub fn digest(&self) -> [u8; 32] {
        assert!(self.well_formed(), "a ticket's digest is of a well-formed ticket");
        let text = format!("fragment door ticket 1\n{}\n{}\n{}\n{}\n{}\n{}", self.door, self.act.as_str(), self.agent, self.computer, self.nonce, self.exp);
        Sha256::digest(text.as_bytes()).into()
    }
}

/// A relayed record's `relayed`, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relayed {
    /// The door it came through.
    pub via: String,
    /// The person it speaks for: its agent's owner.
    pub for_: String,
    /// The agent fragment whose door signed it.
    pub agent: String,
    /// When its door signed it, ms since the epoch.
    pub at: i64,
    /// The door's key's signature of `record_digest` (128 hex).
    pub sig: String,
}

/// Whether `body` says it relays someone (has `relayed` at all).
pub fn relays(body: &Value) -> bool {
    body.get("relayed").is_some()
}

/// `body`'s `relayed`: none when it has none; what is wrong with one that
/// is not a door's.
pub fn relayed(body: &Value) -> Option<Result<Relayed, &'static str>> {
    let r = body.get("relayed")?;
    let read = || -> Result<Relayed, &'static str> {
        let o = r.as_object().ok_or("is an object")?;
        let text = |k: &str| o.get(k).and_then(Value::as_str);
        let via = text("via").filter(|v| known(v)).ok_or("names its door (via: simplex)")?;
        let for_ = text("for").filter(|f| !f.is_empty() && f.len() <= 128).ok_or("names whom it speaks for (for)")?;
        let agent = text("agent").filter(|a| valid_fragment_name(a)).ok_or("names its agent fragment (agent)")?;
        let at = o.get("at").and_then(Value::as_i64).filter(|t| *t > 0).ok_or("says when its door signed it (at, ms)")?;
        let sig = text("sig").filter(|s| hex_of(s, 128)).ok_or("carries its door's signature (sig, 128 hex)")?;
        Ok(Relayed { via: via.into(), for_: for_.into(), agent: agent.into(), at, sig: sig.into() })
    };
    Some(read())
}

/// What a door signs a relayed record by: the fragment, channel and post
/// id it is posted under, and its body as canonical JSON (`canonical`)
/// less `relayed.sig`.
pub fn record_digest(door: &str, fragment: &str, channel: &str, id: &str, body: &Value) -> [u8; 32] {
    let mut unsigned = body.clone();
    if let Some(r) = unsigned.get_mut("relayed").and_then(Value::as_object_mut) {
        r.remove("sig");
    }
    let mut text = format!("fragment door record 1\n{door}\n{fragment}\n{channel}\n{id}\n");
    canonical(&unsigned, &mut text);
    Sha256::digest(text.as_bytes()).into()
}

/// `v` as JSON with each object's keys in order and no space: the same
/// text from the same value, whatever order its keys came in.
pub fn canonical(v: &Value, out: &mut String) {
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

    pub(crate) fn ticket() -> Ticket {
        Ticket { door: SIMPLEX.into(), act: Act::Link, agent: "juniper--k3x9".into(), computer: "computer:00aa".into(), nonce: "0f".repeat(16), exp: 1_790_000_600_000, sig: String::new() }
    }

    /// The relayed record of images/bridge/src/simplex/door.rs's vector.
    fn record() -> Value {
        json!({ "text": "hi from my phone", "to": ["npub1juniper"], "relayed": { "via": "simplex", "for": "npub1paul", "agent": "juniper--k3x9", "at": 1_790_000_000_000i64 } })
    }

    /// Goal: a ticket's digest and a relayed record's are these exact
    /// values (the bridge's tests hold the same), and a signature of one is
    /// the agent's key's or its door's alone.
    #[test]
    fn the_digests_are_the_bridges() {
        assert_eq!(hex::encode(ticket().digest()), TICKET_DIGEST);
        assert_eq!(hex::encode(record_digest(SIMPLEX, "juniper-chat--a1b2", "chat", "sx:0123456789abcdef0123456789abcdef", &record())), RECORD_DIGEST);
        let agent = fragment_nip98::Keys::from_secret_hex(&"07".repeat(32)).expect("a key");
        let sig = agent.sign_digest(&ticket().digest());
        assert_eq!(sig, TICKET_SIG, "BIP-340 with no auxiliary randomness: the same each time");
        assert!(fragment_nip98::verify_digest(agent.pubkey_hex(), &ticket().digest(), &sig));
        let mut other = ticket();
        other.act = Act::Unlink;
        assert!(!fragment_nip98::verify_digest(agent.pubkey_hex(), &other.digest(), &sig), "a link's ticket is no unlink's");
    }

    pub(crate) const TICKET_DIGEST: &str = "fc329f82d4dd4f6c73aaf32e55e0863b13a5de24b9b64ad7e3f2897e985d11e7";
    pub(crate) const RECORD_DIGEST: &str = "6f579428588821dc78f7f0ed9ea310980bafd4857b900ce164c03e732ccb48f4";
    pub(crate) const TICKET_SIG: &str = "979566bc3f8beebe92cd33addd30ba2852132ce15b7b57088ac95075433a192795e10ede4bcdc1ef45605ba0967481c4390f1027b223b5f8c1e131c8cee4f57b";

    /// Goal: a record's digest is of what it says and where, whatever
    /// order its keys came in, and never of its own signature.
    #[test]
    fn a_records_digest_is_of_what_it_says() {
        let d = |body: &Value| record_digest(SIMPLEX, "juniper-chat--a1b2", "chat", "sx:1", body);
        let base = d(&record());
        let reordered: Value = serde_json::from_str(r#"{"relayed":{"at":1790000000000,"agent":"juniper--k3x9","for":"npub1paul","via":"simplex"},"to":["npub1juniper"],"text":"hi from my phone"}"#).expect("json");
        assert_eq!(d(&reordered), base, "key order is no part of it");
        let mut signed = record();
        signed["relayed"]["sig"] = json!("ab".repeat(64));
        assert_eq!(d(&signed), base, "its signature is no part of it");
        for change in [json!({ "text": "hi from my phone!" }), json!({ "to": [] }), json!({ "attachments": [] })] {
            let mut b = record();
            for (k, v) in change.as_object().expect("an object") {
                b[k] = v.clone();
            }
            assert_ne!(d(&b), base, "{change}");
        }
        let mut later = record();
        later["relayed"]["at"] = json!(1_790_000_000_001i64);
        assert_ne!(d(&later), base, "when it was signed is");
        assert_ne!(record_digest(SIMPLEX, "juniper-chat--a1b2", "chat", "sx:2", &record()), base, "its post id is");
        assert_ne!(record_digest(SIMPLEX, "other-chat--a1b2", "chat", "sx:1", &record()), base, "its fragment is");
        let mut text = String::new();
        canonical(&json!({ "b": [1, "x\n"], "a": { "d": null, "c": true } }), &mut text);
        assert_eq!(text, r#"{"a":{"c":true,"d":null},"b":[1,"x\n"]}"#);
    }

    /// Goal: `relayed` is read strictly: each field there and of its shape.
    #[test]
    fn relayed_reads_strictly() {
        assert_eq!(relayed(&json!({ "text": "x" })), None);
        let mut b = record();
        b["relayed"]["sig"] = json!("ab".repeat(64));
        assert_eq!(
            relayed(&b),
            Some(Ok(Relayed { via: SIMPLEX.into(), for_: "npub1paul".into(), agent: "juniper--k3x9".into(), at: 1_790_000_000_000, sig: "ab".repeat(64) }))
        );
        assert!(relays(&b) && !relays(&json!({ "text": "x" })));
        for (k, v) in [("via", json!("telegram")), ("for", json!("")), ("agent", json!("../x")), ("at", json!(-1)), ("at", json!("soon")), ("sig", json!("AB".repeat(64))), ("sig", json!("ab"))] {
            let mut bad = b.clone();
            bad["relayed"][k] = v.clone();
            assert!(matches!(relayed(&bad), Some(Err(_))), "{k}: {v}");
        }
        assert!(matches!(relayed(&json!({ "relayed": "simplex" })), Some(Err(_))));
        let mut unsigned = b.clone();
        unsigned["relayed"].as_object_mut().expect("an object").remove("sig");
        assert!(matches!(relayed(&unsigned), Some(Err(_))), "an unsigned one is no door's");
    }

    /// Goal: a ticket's fields hold no line break and are of their shapes.
    #[test]
    fn tickets_are_well_formed() {
        assert!(ticket().well_formed());
        let bad = [
            Ticket { door: "telegram".into(), ..ticket() },
            Ticket { agent: "juniper\n--k3x9".into(), ..ticket() },
            Ticket { computer: "computer:00AA".into(), ..ticket() },
            Ticket { computer: "laptop".into(), ..ticket() },
            Ticket { nonce: "0f".repeat(15), ..ticket() },
            Ticket { exp: 0, ..ticket() },
            Ticket { sig: "ab".into(), ..ticket() },
        ];
        for t in bad {
            assert!(!t.well_formed(), "{t:?}");
        }
        let read: Ticket = serde_json::from_value(json!({ "door": "simplex", "act": "unlink", "agent": "juniper--k3x9", "computer": "computer:00aa", "nonce": "0f".repeat(16), "exp": 5 })).expect("a ticket");
        assert_eq!((read.act, read.sig.as_str()), (Act::Unlink, ""));
    }
}
