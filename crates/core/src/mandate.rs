//! Proof of who asked (FIN-615, FIN-720; docs/api.md, "Who asked:
//! mandates"). A mandate is a place's word that a person's own record there
//! asked one of its agents: its asker, the agent, the place, the record,
//! and until when. As a
//! place hands an agent member the records people wrote to it there (`GET
//! …/channels/<c>`, read as the agent itself), each carries one. The
//! agent's calls carry it beside their NIP-98 signature
//! (`x-fragment-mandate`) in place of `for=<its owner>`, and the router
//! checks it in the signer's own lookup, with no round trip: the call acts
//! for its asker, held to their own role (`permit`, rule 3). Its owner's
//! own word lends everything of theirs; anyone else's, only what its owner
//! lends that place now (`Lend`, read where it is kept at each use).
//!
//! Only the platform makes one: its mark is a keyed MAC under a key derived
//! from the host secret (as `swap::TagKey`'s), never stored, rotating with
//! it. An agent's computer can neither make nor alter one, and one names
//! its agent, so no other key may carry it. It holds from its record's
//! writing for `limits::MANDATE_LIFE_MS`, so old words start nothing new.

pub use fragment_proto::Lend;
use fragment_proto::{limits, valid_channel_name, Author};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;

/// What a mandate's text begins with (its form's version).
pub const PREFIX: &str = "fm1.";
/// The request header a mandate rides in.
pub const HEADER: &str = "x-fragment-mandate";
/// What the mandate key is derived from the host secret with.
const SALT: &[u8] = b"fragment mandates";
const INFO: &[u8] = b"v1";
/// A mandate's mark is this many bytes of its MAC, as lower-case hex.
const MARK_BYTES: usize = 16;

/// The key mandates are marked with: HKDF-SHA256 of a host secret under a
/// label of its own (as `swap::TagKey`), so it is never stored, never
/// provisioned apart, and rotates with the host secret.
#[derive(Clone)]
pub struct MandateKey([u8; 32]);

impl std::fmt::Debug for MandateKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MandateKey(…)")
    }
}

impl MandateKey {
    pub fn derive(host_secret: &str) -> MandateKey {
        assert!(host_secret.len() >= crate::seal::HOST_SECRET_MIN_BYTES, "a host secret is checked before mandates are marked with it");
        let mut key = [0u8; 32];
        Hkdf::<Sha256>::new(Some(SALT), host_secret.as_bytes()).expand(INFO, &mut key).expect("32 bytes is a valid HKDF-SHA256 length");
        MandateKey(key)
    }

    fn mac(&self, text: &[u8]) -> Hmac<Sha256> {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.0).expect("HMAC takes a key of any length");
        mac.update(text);
        mac
    }
}

/// A place's word that `asker`'s own record asked `agent` there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mandate {
    /// Whose own words asked (an identity).
    pub asker: String,
    /// The agent they asked (an identity): the only signer that carries it.
    pub agent: String,
    /// The fragment they asked in, by its life's own key (its npub): a name
    /// made again is another place.
    pub place: String,
    /// Their record there.
    pub channel: String,
    pub seq: i64,
    /// Until when it holds, ms since the epoch: its record's `at` and
    /// `limits::MANDATE_LIFE_MS`.
    pub until: i64,
}

/// Why a request's mandate is no proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// Not a mandate's form, or longer than one is.
    Malformed,
    /// Its mark is not the platform's: made or altered elsewhere.
    Forged,
    /// Past its `until`.
    Expired,
}

impl Refused {
    pub fn message(self) -> &'static str {
        match self {
            Refused::Malformed => "a mandate is the text a place gave its agent beside a record, unchanged",
            Refused::Forged => "this mandate is not the platform's: only a place makes one",
            Refused::Expired => "this mandate has run out: an asker's words hold for a day after they were said",
        }
    }
}

impl Mandate {
    /// The mandate for `agent` of a record `asker` wrote in `place`, on
    /// `channel` at `seq`, at `at` (ms); none when a name is not of its
    /// kind, which no mandate could carry.
    pub fn of_record(asker: &str, agent: &str, place: &str, channel: &str, seq: i64, at: i64) -> Option<Mandate> {
        let m = Mandate { asker: asker.into(), agent: agent.into(), place: place.into(), channel: channel.into(), seq, until: at.saturating_add(limits::MANDATE_LIFE_MS) };
        well_formed(&m).then_some(m)
    }

    /// Its text: `fm1.<asker>.<agent>.<place>.<channel>.<seq>.<until>.<mark>`,
    /// its mark of all before it. No field holds a dot (each is of its
    /// kind: `well_formed`), so the text reads back as made.
    pub fn text(&self, key: &MandateKey) -> String {
        assert!(well_formed(self), "a mandate is made of names of their kind (`of_record`)");
        let Mandate { asker, agent, place, channel, seq, until } = self;
        let fields = format!("{PREFIX}{asker}.{agent}.{place}.{channel}.{seq}.{until}");
        let mark = key.mac(fields.as_bytes()).finalize().into_bytes();
        let text = format!("{fields}.{}", hex::encode(&mark[..MARK_BYTES]));
        assert!(text.len() <= limits::MANDATE_MAX_BYTES, "a mandate's names are bounded, so its text is");
        text
    }
}

/// A mandate's fields are each of their kind: the identities and the
/// place's key (npubs), a channel's name, a record's place.
fn well_formed(m: &Mandate) -> bool {
    [&m.asker, &m.agent, &m.place].iter().all(|k| crate::npub::is_identity(k)) && valid_channel_name(&m.channel) && m.seq >= 1 && m.until > 0
}

/// `text` read as a mandate, its mark checked under any of `keys` (the
/// current host secret's, then the previous one's while a rotation is
/// under way), at `now` (ms).
pub fn open(keys: &[MandateKey], text: &str, now: i64) -> Result<Mandate, Refused> {
    assert!(!keys.is_empty(), "a mandate is checked under at least one key");
    if text.len() > limits::MANDATE_MAX_BYTES {
        return Err(Refused::Malformed);
    }
    let (fields, mark) = text.rsplit_once('.').filter(|(f, _)| f.starts_with(PREFIX)).ok_or(Refused::Malformed)?;
    let mark = hex::decode(mark).ok().filter(|m| m.len() == MARK_BYTES).ok_or(Refused::Malformed)?;
    if !keys.iter().any(|k| k.mac(fields.as_bytes()).verify_truncated_left(&mark).is_ok()) {
        return Err(Refused::Forged);
    }
    // marked by the platform, so made by `text`: anything else is a fault
    let parts: Vec<&str> = fields[PREFIX.len()..].split('.').collect();
    let [asker, agent, place, channel, seq, until] = parts[..] else { return Err(Refused::Malformed) };
    let number = |n: &str| n.parse::<i64>().ok().filter(|_| n.bytes().all(|b| b.is_ascii_digit()));
    let (Some(seq), Some(until)) = (number(seq), number(until)) else { return Err(Refused::Malformed) };
    let m = Mandate { asker: asker.into(), agent: agent.into(), place: place.into(), channel: channel.into(), seq, until };
    if !well_formed(&m) {
        return Err(Refused::Malformed);
    }
    if now > m.until {
        return Err(Refused::Expired);
    }
    Ok(m)
}

/// The place a mandate's text names (its key), read from a text already
/// opened (`open`), as an agent fragment keeps one for an open turn.
pub fn place_of(text: &str) -> Option<&str> {
    text.strip_prefix(PREFIX)?.split('.').nth(2)
}

/// Until when a mandate's text holds, read from a text its place made.
pub fn until_of(text: &str) -> Option<i64> {
    text.strip_prefix(PREFIX)?.split('.').nth(5)?.parse().ok()
}

/// Which of a chat's agents (in the order they joined it: the first is
/// its lead) a record's words are to, as their bridges hear them: those
/// its `to` names; with none, an agent's words are to no agent, and a
/// person's to the agents they @-mention, else to the lead. A mention
/// names its agent by its own name, which a place does not know, so
/// words that mention anyone are to every agent (one it was not to lets
/// its ask go as another claims it: cell/src/asks.rs).
pub fn heard_by<'a>(body: &str, from_agent: bool, agents: &[&'a str]) -> Vec<&'a str> {
    let v: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
    let to: Vec<&str> = v.get("to").and_then(|t| t.as_array()).map(|t| t.iter().filter_map(|a| a.as_str()).collect()).unwrap_or_default();
    if !to.is_empty() {
        return agents.iter().copied().filter(|a| to.contains(a)).collect();
    }
    if from_agent {
        return vec![];
    }
    let text = v.as_str().or_else(|| v.get("text").and_then(|t| t.as_str())).unwrap_or_default();
    if mentions_anyone(text) {
        return agents.to_vec();
    }
    agents.first().copied().into_iter().collect()
}

/// Whether `text` @-mentions anyone, as a bridge reads a mention: an `@`
/// that follows no word character, before a word (`[A-Za-z0-9_-]`), so an
/// email address is none.
fn mentions_anyone(text: &str) -> bool {
    let b = text.as_bytes();
    (0..b.len()).any(|i| {
        let after_word = i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
        b[i] == b'@' && !after_word && b.get(i + 1).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'-')
    })
}

/// Whose ask a record of a place hands its agents (FIN-720), as its place
/// opens their turns on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asked {
    /// Someone's own words (`asker_of`), proven by their mandate.
    By(String),
    /// An agent's words in a turn of its own (`turn`, its body's): the ask
    /// that turn carries on, as its place opened it, else the agent's own.
    Handed { agent: String, turn: String },
    /// An agent's words in no turn: its own, with nothing to prove them.
    Unproven(String),
}

/// Whose ask a record is (`Asked`), or no one's: app code's words, the
/// platform's own, an anonymous visitor's.
pub fn asked_of(principal: &str, author: Option<Author>, asker: Option<&str>, body: &str) -> Option<Asked> {
    if author == Some(Author::Agent) {
        if !crate::npub::is_identity(principal) {
            return None;
        }
        let turn = match serde_json::from_str::<serde_json::Value>(body) {
            Ok(serde_json::Value::Object(o)) => o.get("turn").and_then(|t| t.as_str()).filter(|t| fragment_proto::valid_op_id(t)).map(str::to_string),
            _ => None,
        };
        return Some(match turn {
            Some(turn) => Asked::Handed { agent: principal.to_string(), turn },
            None => Asked::Unproven(principal.to_string()),
        });
    }
    asker_of(principal, author, asker, body).map(Asked::By)
}

/// Whose own words a record is, as a mandate names them: a person's own
/// post (its `principal`, an identity: an anonymous visitor asks no one
/// elsewhere), or a door's relay of its agent's owner (its `body`'s
/// `relayed.for`, read only then). The platform stamps `door` only on what
/// it took as a door's (a signed relay its door checked, or the one
/// `relayed` a hosted door's code may write: `effects::check_record`), so
/// `for` is its word in either shape; an inbox's delivery relays no one.
/// The platform's record written for someone (`asker`: a schedule's fire,
/// for its maker, checked again as it fired) is theirs. An agent's, app
/// code's and the platform's own records, and one stored before the author
/// stamp, are no one's.
pub fn asker_of(principal: &str, author: Option<Author>, asker: Option<&str>, body: &str) -> Option<String> {
    let identity = |s: &str| crate::npub::is_identity(s).then(|| s.to_string());
    match author? {
        Author::Person => identity(principal),
        Author::Door => {
            let body: serde_json::Value = serde_json::from_str(body).ok()?;
            body.get("relayed")?.get("for")?.as_str().and_then(identity)
        }
        Author::Platform => asker.and_then(identity),
        Author::Agent | Author::Code | Author::Other => None,
    }
}

/// Whether a record's words are to `agent`: its body names whom it is `to`
/// (identities), or names no one, which is everyone there. A `to` of any
/// other shape is to no one: only the agents a record names hear it as
/// asked of them.
pub fn addressed(body: &str, agent: &str) -> bool {
    // most bodies name no one: no parse for them
    if !body.contains("\"to\"") {
        return true;
    }
    let Ok(serde_json::Value::Object(o)) = serde_json::from_str::<serde_json::Value>(body) else { return true };
    match o.get("to") {
        None | Some(serde_json::Value::Null) => true,
        Some(serde_json::Value::Array(to)) => to.is_empty() || to.iter().any(|t| t.as_str() == Some(agent)),
        Some(_) => false,
    }
}

/// A mandate an agent's bridge holds for an open turn, as the agent
/// fragment checked it: its text, and what of it the decisions read: its
/// day, and its place's own key, where what its owner lends is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Held<'a> {
    pub text: &'a str,
    pub until: i64,
    pub place: &'a str,
}

/// A turn's id (docs/chat-records.md): what one agent, by its fragment's
/// name, does about one record of a place. A place opens the turn its
/// agent will run on someone's ask by this id, as the agent's bridge names
/// it.
pub fn turn_id(agent_fragment: &str, place: &str, channel: &str, seq: i64) -> String {
    use sha2::Digest;
    let digest = Sha256::digest(format!("{agent_fragment}|{place}/{channel}/{seq}").as_bytes());
    hex::encode(digest)[..24].to_string()
}

/// Whether a record's body is words an agent answers with a turn: a
/// message (no `kind`, or `message`), as an agent's bridge hears one; a
/// stop, a card's answer or a command starts none.
pub fn starts_turn(body: &str) -> bool {
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(serde_json::Value::String(_)) => true,
        Ok(serde_json::Value::Object(o)) => match o.get("kind") {
            None => true,
            Some(kind) => kind == "message",
        },
        _ => false,
    }
}

/// Whether an agent's call is its presence in its chats, which stays its
/// own and carries no one's mandate: its live socket, and a fragment's
/// channels (their list, records, posts, drafts, newest), blobs, roster
/// (`members`, `__people`), status and subscriptions; its own fragment
/// list; and anything on its own fragment (`own`, its name), its own
/// state. Every other call is work, held to whose ask is open (FIN-720):
/// this one list decides, and a route not on it is work.
pub fn presence(method: &str, path: &str, own: &str) -> bool {
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    let reads = matches!(method, "GET" | "HEAD");
    match parts.as_slice() {
        ["f", name, ..] | ["api", "f", name, ..] if *name == own => true,
        ["f", _, "__live"] => reads,
        ["f", _, "__people"] => reads,
        ["api", "fragments"] => reads,
        ["api", "f", _, "status" | "members" | "channels" | "subscriptions"] => reads,
        ["api", "f", _, "channels", _] => reads || method == "POST",
        ["api", "f", _, "channels", _, "newest"] => reads,
        ["api", "f", _, "channels", _, "draft"] => method == "PUT",
        ["api", "f", _, "blobs", ..] => reads || method == "PUT",
        _ => false,
    }
}

/// One of an agent's open turns, as its bridge told its agent fragment: in
/// `place` (a fragment's name), started by `asker`'s words, proven by
/// their mandate when it carried one still unexpired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Turn<'a> {
    pub place: &'a str,
    pub asker: &'a str,
    pub mandate: Option<Held<'a>>,
}

/// Whose ask an agent's call for its owner (`for=<its owner>`) carries
/// while `turns` are open. Hermes runs one gateway for every chat, so a
/// call can't say which turn it is (FIN-615: the narrowest, Paul
/// 2026-11-21 "yes"). Anyone's turn beside its owner's is the narrower.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Proof<'a> {
    /// No turn proven, none but its owner's: the bare `for=` as before
    /// (an image that tells no turns, or work no one's words started),
    /// until `for=` goes (FIN-615 PR 8).
    Bare,
    /// Its owner's own words: their newest mandate.
    Owner(Held<'a>),
    /// One other person's words, proven: their newest mandate, theirs to
    /// act for inside what its owner lends each place their turns are in
    /// (`at`: each fragment, by its name and its key), read there at each
    /// use, the narrowest of them.
    Asker { held: Held<'a>, at: Vec<(&'a str, &'a str)> },
    /// No single proven asker (two other people's turns, or one with no
    /// proof): the agent acts as itself, in those turns' places alone,
    /// and lends no one anything of its owner's.
    Itself(Vec<&'a str>),
}

/// `owner`'s agent's proof while `turns` are open (`Proof`).
pub fn proof<'a>(owner: &str, turns: &[Turn<'a>]) -> Proof<'a> {
    let newest = |of: &mut dyn Iterator<Item = &Turn<'a>>| of.filter_map(|t| t.mandate).max_by_key(|m| m.until);
    let others: Vec<&Turn<'a>> = turns.iter().filter(|t| t.asker != owner).collect();
    if others.is_empty() {
        return newest(&mut turns.iter()).map_or(Proof::Bare, Proof::Owner);
    }
    let one = others.iter().all(|t| t.asker == others[0].asker);
    match newest(&mut others.iter().copied()) {
        Some(held) if one && others.iter().all(|t| t.mandate.is_some()) => {
            let mut at: Vec<(&'a str, &'a str)> = others.iter().filter_map(|t| Some((t.place, t.mandate?.place))).collect();
            at.sort_unstable();
            at.dedup();
            Proof::Asker { held, at }
        }
        _ => {
            let mut places: Vec<&'a str> = others.iter().map(|t| t.place).collect();
            places.sort_unstable();
            places.dedup();
            Proof::Itself(places)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goal (FIN-720): a place names a turn as its agent's bridge does
    /// (docs/chat-records.md), and opens one only for words that start one.
    #[test]
    fn a_place_names_a_turn_as_its_bridge_does() {
        assert_eq!(turn_id("juniper--k3x9", "talk--k3x9", "chat", 4), "e3c5ceec11d5e3e016f08aee", "sha256 of `agent|place/channel/seq`, 24 hex");
        assert_ne!(turn_id("juniper--k3x9", "talk--k3x9", "chat", 5), turn_id("juniper--k3x9", "talk--k3x9", "chat", 4));
        assert!(starts_turn(r#"{"text":"hi"}"#) && starts_turn(r#"{"kind":"message","text":"hi"}"#) && starts_turn(r#""hi""#));
        for other in [r#"{"kind":"stop"}"#, r#"{"kind":"prompt_response","prompt":"p","option":"o"}"#, r#"{"kind":"command"}"#, "7", "not json"] {
            assert!(!starts_turn(other), "{other}: starts no turn");
        }
        let (ben, juniper) = (crate::npub::display(&"b".repeat(64)), crate::npub::display(&"a".repeat(64)));
        assert_eq!(asked_of(&ben, Some(Author::Person), None, r#"{"text":"hi"}"#), Some(Asked::By(ben.clone())), "a person's own words: theirs");
        assert_eq!(asked_of(&juniper, Some(Author::Agent), None, r#"{"text":"over to you","turn":"t1"}"#), Some(Asked::Handed { agent: juniper.clone(), turn: "t1".into() }), "an agent's words in its turn carry that turn's ask on");
        assert_eq!(asked_of(&juniper, Some(Author::Agent), None, r#"{"text":"hello"}"#), Some(Asked::Unproven(juniper.clone())), "an agent's words in no turn: its own, unproven");
        assert_eq!(asked_of(&juniper, Some(Author::Code), None, r#"{"text":"hi"}"#), None, "app code's words are no one's");
        assert_eq!(asked_of("anon:x", Some(Author::Person), None, r#"{"text":"hi"}"#), None, "an anonymous visitor asks no one");
        let agents = ["npub1lead", "npub1maple"];
        assert_eq!(heard_by(r#"{"text":"hi"}"#, false, &agents), vec!["npub1lead"], "a person's words naming no one are the lead's");
        assert_eq!(heard_by(r#"{"text":"hi","to":["npub1maple"]}"#, false, &agents), vec!["npub1maple"], "`to` names whom");
        assert_eq!(heard_by(r#"{"text":"@maple hi"}"#, false, &agents), agents.to_vec(), "a mention: every agent, the place knowing no names");
        assert_eq!(heard_by(r#"{"text":"mail a@b.com"}"#, false, &agents), vec!["npub1lead"], "an email address is no mention");
        assert_eq!(heard_by(r#"{"text":"over to you"}"#, true, &agents), Vec::<&str>::new(), "an agent's words are to whom `to` names alone");
        assert_eq!(heard_by(r#"{"text":"over","to":["npub1maple"]}"#, true, &agents), vec!["npub1maple"]);
        assert_eq!(heard_by(r#""hi""#, false, &[]), Vec::<&str>::new(), "no agents, no one");
    }

    /// Goal (FIN-720): a mandate's own day is read back from its text, as
    /// an agent fragment holds it for an open turn.
    #[test]
    fn a_mandates_text_says_its_place_and_its_day() {
        let key = MandateKey::derive(&"s".repeat(crate::seal::HOST_SECRET_MIN_BYTES));
        let (asker, agent, place) = (crate::npub::display(&"b".repeat(64)), crate::npub::display(&"a".repeat(64)), crate::npub::display(&"c".repeat(64)));
        let m = Mandate::of_record(&asker, &agent, &place, "chat", 4, 1_000).unwrap();
        let text = m.text(&key);
        assert_eq!(until_of(&text), Some(1_000 + limits::MANDATE_LIFE_MS));
        assert_eq!(place_of(&text), Some(place.as_str()));
        assert_eq!(until_of("not a mandate"), None);
    }

    /// Goal (FIN-720): one list says what of an agent's calls is its
    /// presence in its chats, its own; everything else is work, held to
    /// whose ask is open, a route not on the list among it.
    #[test]
    fn an_agents_presence_is_one_list_and_all_else_is_work() {
        let own = "juniper--k3x9";
        for (method, path) in [
            ("GET", "/f/talk--k3x9/__live"),
            ("GET", "/f/talk--k3x9/__people"),
            ("GET", "/api/fragments"),
            ("GET", "/api/f/talk--k3x9/status"),
            ("GET", "/api/f/talk--k3x9/members"),
            ("GET", "/api/f/talk--k3x9/channels"),
            ("GET", "/api/f/talk--k3x9/channels/chat"),
            ("POST", "/api/f/talk--k3x9/channels/chat"),
            ("GET", "/api/f/talk--k3x9/channels/chat/newest"),
            ("PUT", "/api/f/talk--k3x9/channels/chat/draft"),
            ("GET", "/api/f/talk--k3x9/blobs/abc"),
            ("PUT", "/api/f/talk--k3x9/blobs/abc"),
            ("GET", "/api/f/talk--k3x9/subscriptions"),
            ("POST", "/api/f/juniper--k3x9/ops/note"),
            ("GET", "/f/juniper--k3x9/memories/MEMORY.md"),
        ] {
            assert!(presence(method, path, own), "{method} {path}: presence");
        }
        for (method, path) in [
            ("POST", "/api/f/recipes--k3x9/ops/note"),
            ("GET", "/f/recipes--k3x9/index.html"),
            ("GET", "/api/f/recipes--k3x9/files"),
            ("POST", "/api/fragments"),
            ("POST", "/api/search"),
            ("PUT", "/api/f/talk--k3x9/members/npub1x"),
            ("DELETE", "/api/f/talk--k3x9/channels/chat"),
            ("PUT", "/api/f/talk--k3x9/secrets/KEY"),
            ("GET", "/api/f/talk--k3x9/a-route-made-later"),
            ("GET", "/api/identities/me"),
        ] {
            assert!(!presence(method, path, own), "{method} {path}: work");
        }
    }

    const HOST: &str = "a host secret of at least thirty-two bytes, for tests";
    const NEXT: &str = "the next host secret, also thirty-two bytes or more";

    fn ids() -> (String, String) {
        (crate::npub::encode(&"a".repeat(64)), crate::npub::encode(&"b".repeat(64)))
    }

    fn a_room() -> String {
        crate::npub::encode(&"d".repeat(64))
    }

    fn ben_asks_juniper(at: i64) -> Mandate {
        let (ben, juniper) = ids();
        Mandate::of_record(&ben, &juniper, &a_room(), "chat", 12, at).expect("well formed")
    }

    /// Goal: a place's mandate opens as itself until its record's day is
    /// over, under the key it was marked with or the next one while a
    /// rotation runs, and the same text opens the same each time (replay);
    /// one made under another key, or altered at any part, is no proof.
    #[test]
    fn a_mandate_opens_as_the_place_made_it() {
        let (old, new) = (vec![MandateKey::derive(HOST)], vec![MandateKey::derive(NEXT)]);
        let rotating = vec![new[0].clone(), old[0].clone()];
        let m = ben_asks_juniper(1_000);
        assert_eq!(m.until, 1_000 + limits::MANDATE_LIFE_MS);
        let text = m.text(&old[0]);
        assert!(text.starts_with(PREFIX) && text.len() <= limits::MANDATE_MAX_BYTES / 2 + 64, "{text}");
        assert_eq!(open(&old, &text, 1_000), Ok(m.clone()));
        assert_eq!(open(&old, &text, 2_000), Ok(m.clone()), "replay: the same text again");
        // a rotation: the new secret's key first, the old one's after it
        assert_eq!(open(&rotating, &text, 2_000), Ok(m.clone()));
        assert_eq!(open(&new, &text, 2_000), Err(Refused::Forged), "once the old secret is gone");
        // its day: still at `until`, run out past it
        assert_eq!(open(&old, &text, m.until), Ok(m.clone()));
        assert_eq!(open(&old, &text, m.until + 1), Err(Refused::Expired));
        // altered: its fields (another asker), its mark, or its form
        let (_, juniper) = ids();
        let other = Mandate { asker: juniper, ..m.clone() }.text(&old[0]);
        let (fields, mark) = (other.rsplit_once('.').unwrap().0, text.rsplit_once('.').unwrap().1);
        assert_eq!(open(&old, &format!("{fields}.{mark}"), 2_000), Err(Refused::Forged), "another's fields under this one's mark");
        let later = text.replacen(&format!(".{}.", m.until), &format!(".{}.", m.until + 1), 1);
        assert_eq!(open(&old, &later, 2_000), Err(Refused::Forged), "its day lengthened");
        let flipped = format!("{}{}", &text[..text.len() - 1], if text.ends_with('0') { '1' } else { '0' });
        assert_eq!(open(&old, &flipped, 2_000), Err(Refused::Forged));
        for bad in ["", "fm1.", "fm2.e30.00", "fm1.e30", &text[PREFIX.len()..], &format!("{text}0"), &"x".repeat(limits::MANDATE_MAX_BYTES + 1)] {
            assert!(matches!(open(&old, bad, 2_000), Err(Refused::Malformed | Refused::Forged)), "{bad:?}");
        }
        assert_eq!(open(&old, &"x".repeat(limits::MANDATE_MAX_BYTES + 1), 2_000), Err(Refused::Malformed));
    }

    /// Goal: a mandate names identities, a fragment and a channel, each of
    /// its kind, or none is made, and one whose fields are marked yet
    /// malformed (made by no place) does not open.
    #[test]
    fn a_mandate_is_made_only_of_names_of_their_kind() {
        let (ben, juniper, room) = (ids().0, ids().1, a_room());
        assert!(Mandate::of_record(&ben, &juniper, &room, "chat", 1, 1).is_some());
        for (asker, agent, place, channel, seq) in [
            ("anon:0123456789abcdef0123456789abcdef", juniper.as_str(), room.as_str(), "chat", 1),
            (ben.as_str(), "npub1short", room.as_str(), "chat", 1),
            (ben.as_str(), juniper.as_str(), "chat--k3x9", "chat", 1),
            (ben.as_str(), juniper.as_str(), room.as_str(), "Chat", 1),
            (ben.as_str(), juniper.as_str(), room.as_str(), "chat", 0),
        ] {
            assert_eq!(Mandate::of_record(asker, agent, place, channel, seq, 1), None, "{asker} {agent} {place} {channel} {seq}");
        }
        // marked, yet not of a mandate's form: a field more, or a seq that is no number
        let key = MandateKey::derive(HOST);
        let (ben, juniper, room) = (ids().0, ids().1, a_room());
        for fields in [format!("{PREFIX}{ben}.{juniper}.{room}.chat.1.2.3"), format!("{PREFIX}{ben}.{juniper}.{room}.chat.+1.2"), format!("{PREFIX}{ben}.{juniper}.chat--k3x9.chat.1.2")] {
            let mark = hex::encode(&key.mac(fields.as_bytes()).finalize().into_bytes()[..MARK_BYTES]);
            assert_eq!(open(std::slice::from_ref(&key), &format!("{fields}.{mark}"), 1), Err(Refused::Malformed), "{fields}");
        }
    }

    /// Goal: a record's asker is whose own words it is: a person's own
    /// post, or a door's relay of its agent's owner; an anonymous
    /// visitor's, an agent's, app code's, the platform's and an unstamped
    /// record are no one's.
    #[test]
    fn a_records_asker_is_whose_own_words_it_is() {
        let (ben, ana) = ids();
        let juniper = crate::npub::encode(&"c".repeat(64));
        let relayed = serde_json::json!({ "text": "hi", "to": [juniper], "relayed": { "via": "simplex", "for": ana, "agent": "juniper--k3x9", "at": 1, "sig": "ab".repeat(64) } }).to_string();
        assert_eq!(asker_of(&ben, Some(Author::Person), None, "{}"), Some(ben.clone()));
        assert_eq!(asker_of(&juniper, Some(Author::Door), None, &relayed), Some(ana.clone()), "a door's signed relay: its owner");
        let hosted = serde_json::json!({ "text": "hi from telegram", "to": [juniper], "relayed": { "via": "telegram", "for": ana, "agent": "juniper--k3x9" } }).to_string();
        assert_eq!(asker_of("npub1telegramapp", Some(Author::Door), None, &hosted), Some(ana.clone()), "a hosted door's relay, as its code writes it: its owner");
        assert_eq!(asker_of("inbox", Some(Author::Door), None, r#"{"source":"external","payload":{}}"#), None, "an inbox delivery relays no one");
        assert_eq!(asker_of("anon:0123456789abcdef0123456789abcdef", Some(Author::Person), None, "{}"), None);
        assert_eq!(asker_of("npub1ownkey", Some(Author::Platform), Some(&ana), "{}"), Some(ana.clone()), "a schedule's fire: its maker's");
        assert_eq!(asker_of("npub1ownkey", Some(Author::Platform), Some("not an identity"), "{}"), None);
        for author in [None, Some(Author::Agent), Some(Author::Code), Some(Author::Platform), Some(Author::Other)] {
            assert_eq!(asker_of(&ben, author, None, &relayed), None, "{author:?}");
        }
        assert_eq!(asker_of(&ben, Some(Author::Agent), Some(&ana), "{}"), None, "only the platform writes for someone");
    }

    /// Goal: a record's words are to the agents it names, or to everyone
    /// when it names no one; a `to` of another shape is to no one.
    #[test]
    fn a_record_is_to_whom_it_names() {
        let (juniper, fred) = ids();
        let to = |v: serde_json::Value| serde_json::json!({ "text": "hi", "to": v }).to_string();
        assert!(addressed(r#"{"text":"hi"}"#, &juniper), "no one named: everyone");
        assert!(addressed(r#""a bare string""#, &juniper));
        assert!(addressed(&to(serde_json::json!([])), &juniper) && addressed(&to(serde_json::Value::Null), &juniper));
        assert!(addressed(&to(serde_json::json!([juniper])), &juniper) && !addressed(&to(serde_json::json!([juniper])), &fred));
        assert!(!addressed(&to(serde_json::json!("everyone")), &juniper) && !addressed(&to(serde_json::json!({ "x": 1 })), &juniper), "another shape: no one");
        assert!(addressed(r#"{"text":"say "to" me"}"#, &juniper), "the word in a text names no one");
    }


    /// Goal: the narrowest ask an agent's call for its owner carries while
    /// turns are open (`Proof`): its owner's newest proof alone; anyone
    /// else's beside it narrows it, theirs, naming the place whose lend
    /// holds it; two other people, or one unproven, leave no one's: the
    /// agent itself, in their places alone; no turn proven of its owner's,
    /// or none at all, is the bare `for=` as before.
    #[test]
    fn the_narrowest_ask_is_carried() {
        let (ana, ben) = ids();
        let carol = crate::npub::encode(&"e".repeat(64));
        let held = |text: &'static str, until: i64, place: &'static str| Some(Held { text, until, place });
        let turn = |place: &'static str, asker: &str, mandate: Option<Held<'static>>| Turn { place, asker: Box::leak(asker.to_string().into_boxed_str()), mandate };
        assert_eq!(proof(&ana, &[]), Proof::Bare);
        assert_eq!(proof(&ana, &[turn("own", &ana, None)]), Proof::Bare, "its owner's, unproven: as before");
        let (a1, a2) = (held("a1", 10, "npub1own"), held("a2", 20, "npub1own"));
        assert_eq!(proof(&ana, &[turn("own", &ana, a1), turn("own", &ana, a2)]), Proof::Owner(a2.unwrap()), "its owner's newest");
        let b = held("b", 5, "npub1room");
        let bens = proof(&ana, &[turn("own", &ana, a2), turn("room", &ben, b), turn("hall", &ben, held("b2", 4, "npub1hall")), turn("room", &ben, b)]);
        assert_eq!(bens, Proof::Asker { held: b.unwrap(), at: vec![("hall", "npub1hall"), ("room", "npub1room")] }, "another's beside its owner's is the narrower: their newest, and each place of theirs");
        let busy = proof(&ana, &[turn("room", &ben, b), turn("hall", &carol, held("c", 9, "npub1hall")), turn("room", &carol, None)]);
        assert_eq!(busy, Proof::Itself(vec!["hall", "room"]), "two other people: no one's, in their places");
        assert_eq!(proof(&ana, &[turn("room", &ben, None)]), Proof::Itself(vec!["room"]), "one other, unproven");
        assert_eq!(proof(&ana, &[turn("room", &ben, b), turn("hall", &ben, None)]), Proof::Itself(vec!["hall", "room"]), "one other with a turn unproven");
        // replay: the same turns decide the same
        assert_eq!(proof(&ana, &[turn("room", &ben, b)]), proof(&ana, &[turn("room", &ben, b)]));
    }
}
