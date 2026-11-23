//! An agent's doors (docs/api.md, "An agent's doors"): ways its owner
//! reaches it other than its chat, run by its computer (SimpleX, decision
//! 32; Paul's design of 2026-10-23). A door's id is a connector's
//! (`connectors::id_ok`), and the deployment's policy says which may be
//! opened (`connectors::Policy::authorize`). Only its owner opens one, and only
//! what came through it speaks for them; the agent itself can do neither.
//!
//! - **A ticket** starts a link, an unlink, or an adoption (its owner keeps
//!   their SimpleX conversation with their old finite.computer agent, set
//!   aside from its move's stage: FIN-463; an adoption is a link to a
//!   contact that exists already, so it is redeemed as a link's is). The platform makes one at
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
//! - **A hosted door** is a connector fragment its owner bound as one of
//!   their agent's doors, running the platform's blessed template named
//!   for the door (`telegram`): it carries their messages in its own
//!   channels, `relayed` exactly `Hosted::relayed`, unsigned, since the
//!   platform knows which fragment publishes (`effects::check_record`).
//!   Its binding is the connector fragment's, set by its owner in person.
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

/// A connector fragment bound as `door` of its owner's `agent`, while its
/// code is still the blessed template of that name and the deployment
/// allows the door: what it may say it relays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hosted {
    pub door: String,
    /// The agent fragment.
    pub agent: String,
    /// The owner's identity, as a relayed record's `for` names them.
    pub owner: String,
}

impl Hosted {
    /// The one `relayed` its records may carry.
    pub fn relayed(&self) -> Value {
        serde_json::json!({ "via": self.door, "for": self.owner, "agent": self.agent })
    }
}

/// The blessed template a fragment's installed code is (`code.loader_id`,
/// `blessed:<template>@<release>`), if it is one.
pub fn blessed_template(loader_id: &str) -> Option<&str> {
    loader_id.strip_prefix("blessed:")?.split_once('@').map(|(t, _)| t)
}

/// Where a SimpleX connector's jobs reach their own door (cell/door.mjs):
/// a name of the platform's, never one on the internet.
pub const DOOR_HOST: &str = "door.fragment.internal";

/// The headers that tell a door which door is asked (`door_id`) and the
/// connector that asks (its name, its inbox's): the platform's alone to
/// set, over whatever a job set (cell/src/jobs.rs; cell/door.mjs reads them).
pub const DOOR_NAME_HEADER: &str = "x-fragment-door";
pub const CONNECTOR_HEADER: &str = "x-fragment-connector";

/// A door id's digits of its connector's life.
pub const DOOR_LIFE_HEX: usize = 12;

/// A SimpleX connector's door: its name and the first `DOOR_LIFE_HEX` hex
/// digits of a digest of its life's own key (its npub, drawn new for each
/// life), as its repo is named (`codestorage::repo_name`). A name made
/// again, by anyone, never reaches a door an earlier life made, nor the
/// SimpleX link it holds. `None`: not a fragment's name, or not a key's npub.
pub fn door_id(fragment: &str, life: &str) -> Option<String> {
    if !valid_fragment_name(fragment) || !crate::npub::is_identity(life) {
        return None;
    }
    let digest = hex::encode(Sha256::digest(format!("fragment door life\0{life}").as_bytes()));
    Some(format!("{fragment}--{}", &digest[..DOOR_LIFE_HEX]))
}

/// The fragment a door's id is of: a connector's (`door_id`), or, for a
/// door the e2e's lever starts by a fragment's name alone, that name.
pub fn door_of(id: &str) -> Option<&str> {
    if valid_fragment_name(id) {
        return Some(id);
    }
    let (fragment, life) = id.rsplit_once("--")?;
    let digits = life.len() == DOOR_LIFE_HEX && life.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    (digits && valid_fragment_name(fragment)).then_some(fragment)
}

/// Where a job's fetch of `raw` goes, from a fragment whose installed code
/// is the blessed template `blessed`: to its own door (the URL, `Some`)
/// when it names `DOOR_HOST`, or out (`None`: the egress check's to judge).
/// Only a SimpleX connector reaches a door, the one named as its fragment
/// is (cell/src/jobs.rs): any other fragment's fetch of the door's host is
/// refused, never sent out, as is one with a port or credentials.
pub fn door_route(raw: &str, blessed: Option<&str>) -> Result<Option<url::Url>, String> {
    let Ok(url) = url::Url::parse(raw) else { return Ok(None) };
    if url.host_str().map(|h| h.trim_end_matches('.')) != Some(DOOR_HOST) {
        return Ok(None);
    }
    if blessed != Some(SIMPLEX) {
        return Err(format!("{DOOR_HOST} is a SimpleX connector's own door: no other fragment reaches it"));
    }
    let plain = url.scheme() == "https" && url.host_str() == Some(DOOR_HOST) && url.port().is_none() && url.username().is_empty() && url.password().is_none();
    if !plain {
        return Err(format!("its door is https://{DOOR_HOST}/<what it asks>, with no port or credentials"));
    }
    Ok(Some(url))
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
    Adopt,
}

impl Act {
    pub fn as_str(self) -> &'static str {
        match self {
            Act::Link => "link",
            Act::Unlink => "unlink",
            Act::Adopt => "adopt",
        }
    }

    /// Whether it opens the door (a key is redeemed with its ticket): a
    /// link, or an adoption.
    pub fn opens(self) -> bool {
        matches!(self, Act::Link | Act::Adopt)
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
        crate::connectors::id_ok(&self.door) && valid_fragment_name(&self.agent) && computer_ok(&self.computer) && hex_of(&self.nonce, 32) && self.exp > 0 && (self.sig.is_empty() || hex_of(&self.sig, 128))
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

/// Whether `body` carries an outside speaker (`heard` at all).
pub fn hears(body: &Value) -> bool {
    body.get("heard").is_some()
}

/// Whether `body` carries hosted-door room metadata (`heard`, `room`, or
/// `context`, non-null or present): only a connector's own app publishes
/// these on a connector fragment, never an external HTTP post or a
/// scheduled record.
pub fn room_fields(body: &Value) -> bool {
    body.get("heard").is_some() || body.get("room").is_some() || body.get("context").is_some()
}

/// Whether a `heard` body matches the open hosted door `hosted` (`heard.via`
/// is that door's id, and `body` does not also carry `relayed`).
pub fn heard_via_ok(body: &Value, hosted: &Hosted) -> bool {
    if relays(body) {
        return false;
    }
    body.get("heard").and_then(|h| h.get("via")).and_then(Value::as_str) == Some(hosted.door.as_str())
}

/// `body`'s `relayed`: none when it has none; what is wrong with one that
/// is not a door's.
pub fn relayed(body: &Value) -> Option<Result<Relayed, &'static str>> {
    let r = body.get("relayed")?;
    let read = || -> Result<Relayed, &'static str> {
        let o = r.as_object().ok_or("is an object")?;
        let text = |k: &str| o.get(k).and_then(Value::as_str);
        let via = text("via").filter(|v| crate::connectors::id_ok(v)).ok_or("names its door (via, a connector's id)")?;
        let for_ = text("for").filter(|f| !f.is_empty() && f.len() <= 128).ok_or("names whom it speaks for (for)")?;
        let agent = text("agent").filter(|a| valid_fragment_name(a)).ok_or("names its agent fragment (agent)")?;
        let at = o.get("at").and_then(Value::as_i64).filter(|t| *t > 0).ok_or("says when its door signed it (at, ms)")?;
        let sig = text("sig").filter(|s| hex_of(s, 128)).ok_or("carries its door's signature (sig, 128 hex)")?;
        Ok(Relayed { via: via.into(), for_: for_.into(), agent: agent.into(), at, sig: sig.into() })
    };
    Some(read())
}

/// Whether a relayed record may be appended to a fragment `fragment_owner`
/// owns, posted by an agent whose owner is `agent_owner` (each as the
/// platform holds it: an npub, or 64 hex): only into its agent's owner's
/// own fragment. A door signs the fragment's name, not its life, and a
/// name is free the moment its fragment is deleted; anyone may make one by
/// that full name, public, taking posts from anyone, and it would take
/// their words (security candidate 10: a SimpleX mirror chat deleted and
/// made again by a stranger). Every door relays only into its owner's
/// own: a SimpleX door into the chat its agent made for them, a hosted
/// door into its own channels (`Hosted`, its binding its owner's).
pub fn lands_in_owners(fragment_owner: &str, agent_owner: &str) -> bool {
    !agent_owner.is_empty() && crate::npub::display(fragment_owner) == crate::npub::display(agent_owner)
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
            keys.sort_unstable();
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

    /// Goal: a SimpleX connector's jobs reach their own door alone, and no
    /// other fragment's fetch of the door's host leaves the platform.
    /// Valid: the connector's, to the door's host. Invalid: any other
    /// fragment's (another connector, an app, none blessed), in any
    /// spelling of the host; the connector's with a port, credentials or
    /// cleartext. Every other URL is the egress check's.
    /// Goal: a connector's door is its life's (the review of #1450: a name
    /// deleted and made again by someone else, keyed by the name alone,
    /// took over the earlier owner's door, its SimpleX link and its
    /// messages). Valid: one life, one door, whose fragment it names back.
    /// Invalid: another life's, the same name; a name or key that is none.
    #[test]
    fn a_connectors_door_is_its_lifes() {
        let (one, two) = (crate::npub::encode(&"3b".repeat(32)), crate::npub::encode(&"5c".repeat(32)));
        let a = door_id("juniper-sx--k3x9", &one).expect("a door");
        assert_eq!(a, door_id("juniper-sx--k3x9", &one).unwrap(), "one life, one door");
        assert_ne!(a, door_id("juniper-sx--k3x9", &two).unwrap(), "a name made again: another door");
        assert!(a.starts_with("juniper-sx--k3x9--") && a.len() <= 63 + 2 + DOOR_LIFE_HEX && a.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'), "{a}: a door's name (cell/door-rules.mjs nameOk)");
        assert_eq!(door_of(&a), Some("juniper-sx--k3x9"));
        assert_eq!(door_of("juniper-sx--k3x9"), Some("juniper-sx--k3x9"), "the lever's door, named as a fragment is");
        for bad in ["juniper-sx--k3x9--0123456789AB", "juniper-sx--k3x9--0123", "juniper--0123456789ab", "../x", ""] {
            assert_eq!(door_of(bad), None, "{bad}");
        }
        assert_eq!(door_id("not a name", &one), None);
        assert_eq!(door_id("juniper-sx--k3x9", "npub1nope"), None);
    }

    #[test]
    fn only_a_simplex_connector_reaches_its_door() {
        let asked = door_route("https://door.fragment.internal/reply", Some("simplex")).expect("its own door");
        assert_eq!(asked.map(|u| u.path().to_string()).as_deref(), Some("/reply"));
        for raw in ["https://door.fragment.internal/reply", "https://DOOR.fragment.internal/x", "https://door.fragment.internal./x", "http://door.fragment.internal:8080/x"] {
            for blessed in [Some("telegram"), Some("chat"), None] {
                assert!(door_route(raw, blessed).is_err(), "{raw} from {blessed:?}");
            }
        }
        for raw in ["http://door.fragment.internal/x", "https://door.fragment.internal:444/x", "https://a:b@door.fragment.internal/x", "https://door.fragment.internal./x"] {
            assert!(door_route(raw, Some("simplex")).is_err(), "{raw}");
        }
        for raw in ["https://api.telegram.org/bot", "https://fragment.internal/x", "https://door.fragment.internal.example.com/x", "not a url"] {
            assert_eq!(door_route(raw, Some("simplex")), Ok(None), "{raw}: the egress check's");
        }
    }

    /// Goal (security candidate 10): a relayed record lands only in a
    /// fragment its agent's owner owns. Valid: theirs, however each side
    /// holds the owner (npub or hex). Invalid: a stranger's (a name its
    /// owner deleted, made again by someone else), by either form; no
    /// owner at all.
    #[test]
    fn a_relayed_record_lands_only_in_its_owners_fragment() {
        let owner = "3bf0c63fcb93463407af97a5e5ee64fa883d107ef9e558472c4eb9aaaefa459d";
        let owner_npub = crate::npub::display(owner);
        assert!(owner_npub.starts_with("npub1"));
        for (fragment, agent) in [(owner, owner_npub.as_str()), (owner_npub.as_str(), owner), (owner_npub.as_str(), owner_npub.as_str()), (owner, owner)] {
            assert!(lands_in_owners(fragment, agent), "its owner's own: {fragment} {agent}");
        }
        let stranger = "cd".repeat(32);
        let stranger_npub = crate::npub::display(&stranger);
        for fragment in [stranger.as_str(), stranger_npub.as_str()] {
            assert!(!lands_in_owners(fragment, &owner_npub), "a stranger's fragment by its name: {fragment}");
            assert!(!lands_in_owners(fragment, owner), "{fragment}");
        }
        assert!(!lands_in_owners("", ""), "no owner is no one's");
        assert!(!lands_in_owners(owner, ""), "an agent with no owner relays no one");
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
        other.act = Act::Adopt;
        assert_eq!(hex::encode(other.digest()), ADOPT_DIGEST, "an adoption's, the bridge's too");
        assert!(!fragment_nip98::verify_digest(agent.pubkey_hex(), &other.digest(), &sig), "nor an adoption's");
        assert!(Act::Link.opens() && Act::Adopt.opens() && !Act::Unlink.opens());
    }

    /// images/bridge/src/simplex/door.rs holds the same.
    pub(crate) const ADOPT_DIGEST: &str = "d4e45efd715720407801163b884480a90be37f9c66a9ac103edba3bf28391e9f";

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
        for (k, v) in [("via", json!("../simplex")), ("via", json!("SimpleX")), ("for", json!("")), ("agent", json!("../x")), ("at", json!(-1)), ("at", json!("soon")), ("sig", json!("AB".repeat(64))), ("sig", json!("ab"))] {
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
            Ticket { door: "../simplex".into(), ..ticket() },
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
        let read: Ticket = serde_json::from_value(json!({ "door": "simplex", "act": "adopt", "agent": "juniper--k3x9", "computer": "computer:00aa", "nonce": "0f".repeat(16), "exp": 5 })).expect("a ticket");
        assert_eq!(read.act, Act::Adopt);
        assert!(serde_json::from_value::<Ticket>(json!({ "door": "simplex", "act": "keep", "agent": "juniper--k3x9", "computer": "computer:00aa", "nonce": "0f".repeat(16), "exp": 5 })).is_err(), "no other act");
    }
}
