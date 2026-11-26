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
//!
//! **Carried** (FIN-615, PR 16a: `fragment ask`): an agent working on
//! someone's ask that asks another agent of its owner's, in a chat of
//! theirs, hands that agent the same ask (`Mandate::carried`): the same
//! asker, place, record and day, and the same agent lent there (`via`), so
//! the agent asked acts for them within what the place where they asked
//! lends, and can only narrow it, never widen it ("Fred for Ben, via
//! Juniper").

pub use fragment_proto::Lend;
use fragment_proto::{limits, valid_channel_name, Author};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;

/// What a mandate's text begins with (its form's version): `fm2` names its
/// place's fragment name and the agent the place lends (`via`).
pub const PREFIX: &str = "fm2.";
/// The form before (FIN-615 PR 16a), naming neither: still opened while a
/// turn may hold one (docs/technical-debt-ledger.md, "A mandate of the
/// first form still opens").
const PREFIX_V1: &str = "fm1.";
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
    /// That fragment's name, where its lend is asked (its key checked
    /// there); none in a mandate of the first form.
    pub name: Option<String>,
    /// Their record there.
    pub channel: String,
    pub seq: i64,
    /// Until when it holds, ms since the epoch: its record's `at` and
    /// `limits::MANDATE_LIFE_MS`.
    pub until: i64,
    /// The agent the place lends for this ask: the one they asked, or, for
    /// a mandate carried to another of its owner's agents, the one first
    /// asked (`carried`).
    pub via: String,
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
    /// The mandate for `agent` of a record `asker` wrote in `place` (its
    /// key; `name`, its fragment's name), on `channel` at `seq`, at `at`
    /// (ms); none when a name is not of its kind, which no mandate could
    /// carry.
    pub fn of_record(asker: &str, agent: &str, place: &str, name: &str, channel: &str, seq: i64, at: i64) -> Option<Mandate> {
        let until = at.saturating_add(limits::MANDATE_LIFE_MS);
        let m = Mandate { asker: asker.into(), agent: agent.into(), place: place.into(), name: Some(name.into()), channel: channel.into(), seq, until, via: agent.into() };
        well_formed(&m).then_some(m)
    }

    /// This ask carried by its agent to `agent`, another agent of its
    /// owner's it asked (the caller checks they share an owner): the same
    /// asker, place, record and day, lent by the same agent there (`via`),
    /// so it narrows nothing it was given and widens nothing. None for a
    /// mandate of the first form (no place name to read its lend at: the
    /// ask goes on unproven, as before), or a name not of its kind.
    pub fn carried(&self, agent: &str) -> Option<Mandate> {
        let m = Mandate { agent: agent.into(), ..self.clone() };
        (m.name.is_some() && well_formed(&m)).then_some(m)
    }

    /// Its text: `fm2.<asker>.<agent>.<place>.<name>.<channel>.<seq>.<until>.<via>.<mark>`,
    /// its mark of all before it. No field holds a dot (each is of its
    /// kind: `well_formed`), so the text reads back as made.
    pub fn text(&self, key: &MandateKey) -> String {
        assert!(well_formed(self), "a mandate is made of names of their kind (`of_record`)");
        let Mandate { asker, agent, place, name, channel, seq, until, via } = self;
        let name = name.as_deref().expect("a mandate is made with its place's name (`of_record`)");
        let fields = format!("{PREFIX}{asker}.{agent}.{place}.{name}.{channel}.{seq}.{until}.{via}");
        let mark = key.mac(fields.as_bytes()).finalize().into_bytes();
        let text = format!("{fields}.{}", hex::encode(&mark[..MARK_BYTES]));
        assert!(text.len() <= limits::MANDATE_MAX_BYTES, "a mandate's names are bounded, so its text is");
        text
    }
}

/// A mandate's fields are each of their kind: the identities and the
/// place's key (npubs), its fragment name, a channel's name, a record's
/// place.
fn well_formed(m: &Mandate) -> bool {
    [&m.asker, &m.agent, &m.place, &m.via].iter().all(|k| crate::npub::is_identity(k))
        && m.name.as_deref().is_none_or(fragment_proto::valid_fragment_name)
        && valid_channel_name(&m.channel)
        && m.seq >= 1
        && m.until > 0
}

/// `text` read as a mandate, its mark checked under any of `keys` (the
/// current host secret's, then the previous one's while a rotation is
/// under way), at `now` (ms).
pub fn open(keys: &[MandateKey], text: &str, now: i64) -> Result<Mandate, Refused> {
    assert!(!keys.is_empty(), "a mandate is checked under at least one key");
    if text.len() > limits::MANDATE_MAX_BYTES {
        return Err(Refused::Malformed);
    }
    let (fields, mark) = text.rsplit_once('.').filter(|(f, _)| f.starts_with(PREFIX) || f.starts_with(PREFIX_V1)).ok_or(Refused::Malformed)?;
    let mark = hex::decode(mark).ok().filter(|m| m.len() == MARK_BYTES).ok_or(Refused::Malformed)?;
    if !keys.iter().any(|k| k.mac(fields.as_bytes()).verify_truncated_left(&mark).is_ok()) {
        return Err(Refused::Forged);
    }
    // marked by the platform, so made by `text` (or the first form's): anything else is a fault
    let parts: Vec<&str> = fields[PREFIX.len()..].split('.').collect();
    let first = fields.starts_with(PREFIX_V1);
    let (asker, agent, place, name, channel, seq, until, via) = match parts[..] {
        [asker, agent, place, channel, seq, until] if first => (asker, agent, place, None, channel, seq, until, agent),
        [asker, agent, place, name, channel, seq, until, via] if !first => (asker, agent, place, Some(name), channel, seq, until, via),
        _ => return Err(Refused::Malformed),
    };
    let number = |n: &str| n.parse::<i64>().ok().filter(|_| n.bytes().all(|b| b.is_ascii_digit()));
    let (Some(seq), Some(until)) = (number(seq), number(until)) else { return Err(Refused::Malformed) };
    let m = Mandate { asker: asker.into(), agent: agent.into(), place: place.into(), name: name.map(str::to_string), channel: channel.into(), seq, until, via: via.into() };
    if !well_formed(&m) {
        return Err(Refused::Malformed);
    }
    if now > m.until {
        return Err(Refused::Expired);
    }
    Ok(m)
}

/// Where a mandate's text says its lend is read, from a text already
/// opened (`open`), as an agent fragment keeps one for an open turn: its
/// place's key, its fragment's name (none in the first form: the turn's
/// own place), and the agent lent there.
pub fn lent_at(text: &str) -> Option<(&str, Option<&str>, &str)> {
    if let Some(rest) = text.strip_prefix(PREFIX_V1) {
        let mut f = rest.split('.');
        let (agent, place) = (f.nth(1)?, f.next()?);
        return Some((place, None, agent));
    }
    let f: Vec<&str> = text.strip_prefix(PREFIX)?.split('.').collect();
    Some((f.get(2)?, Some(*f.get(3)?), f.get(7)?))
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
/// day, and where what its owner lends is kept: its place's own key, that
/// fragment's name (none: the turn's own place), and the agent lent there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Held<'a> {
    pub text: &'a str,
    pub until: i64,
    pub place: &'a str,
    pub name: Option<&'a str>,
    pub via: &'a str,
}

/// Where a lend is read for an ask (`Proof::Asker`): the fragment asked
/// (its name), the key it must have (its life), and the agent whose
/// member row there holds the lend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct LentAt<'a> {
    pub name: &'a str,
    pub place: &'a str,
    pub agent: &'a str,
}

/// Whether a bridge life's word on its agent's turns is heard (cell
/// runs_on.rs `turn_life`), given the lives heard on the computer the agent
/// runs on, each with that computer's start (the Computer DO's generation,
/// which its bridge read as it started), the newest first: by start, then,
/// within one start, by when first heard. Another computer's starts are
/// not comparable: none of its lives is among them. The newest's is; a life never heard of a start no
/// older takes over (its bridge is the computer's one now: every other
/// life's turns are forgotten); any other is retired, its word not heard:
/// a late call from a bridge gone, its first among them (a container
/// before's, which may come after its successor's).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heard {
    Speaks,
    TakesOver,
    Retired,
}

pub fn heard(lives_newest_first: &[(&str, u64)], life: &str, start: u64) -> Heard {
    match lives_newest_first.iter().position(|(l, _)| *l == life) {
        Some(0) => Heard::Speaks,
        Some(_) => Heard::Retired,
        None if lives_newest_first.first().is_some_and(|(_, newest)| *newest > start) => Heard::Retired,
        None => Heard::TakesOver,
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
    /// act for inside what its owner lends each place their asks began in
    /// (`at`: each fragment by its name and key, for the agent lent there:
    /// a carried ask's is its first agent's), read there at each use, the
    /// narrowest of them.
    Asker { held: Held<'a>, at: Vec<LentAt<'a>> },
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
            let mut at: Vec<LentAt<'a>> = others.iter().filter_map(|t| t.mandate.map(|m| LentAt { name: m.name.unwrap_or(t.place), place: m.place, agent: m.via })).collect();
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
        Mandate::of_record(&ben, &juniper, &a_room(), "room--k3x9", "chat", 12, at).expect("well formed")
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
        assert!(Mandate::of_record(&ben, &juniper, &room, "room--k3x9", "chat", 1, 1).is_some());
        for (asker, agent, place, name, channel, seq) in [
            ("anon:0123456789abcdef0123456789abcdef", juniper.as_str(), room.as_str(), "room--k3x9", "chat", 1),
            (ben.as_str(), "npub1short", room.as_str(), "room--k3x9", "chat", 1),
            (ben.as_str(), juniper.as_str(), "chat--k3x9", "room--k3x9", "chat", 1),
            (ben.as_str(), juniper.as_str(), room.as_str(), "room.k3x9", "chat", 1),
            (ben.as_str(), juniper.as_str(), room.as_str(), "room", "chat", 1),
            (ben.as_str(), juniper.as_str(), room.as_str(), "room--k3x9", "Chat", 1),
            (ben.as_str(), juniper.as_str(), room.as_str(), "room--k3x9", "chat", 0),
        ] {
            assert_eq!(Mandate::of_record(asker, agent, place, name, channel, seq, 1), None, "{asker} {agent} {place} {name} {channel} {seq}");
        }
        // marked, yet not of a mandate's form: a field more, or a seq that is no number
        let key = MandateKey::derive(HOST);
        let (ben, juniper, room) = (ids().0, ids().1, a_room());
        for fields in [
            format!("{PREFIX}{ben}.{juniper}.{room}.room--k3x9.chat.1.2.{juniper}.3"),
            format!("{PREFIX}{ben}.{juniper}.{room}.room--k3x9.chat.+1.2.{juniper}"),
            format!("{PREFIX}{ben}.{juniper}.chat--k3x9.room--k3x9.chat.1.2.{juniper}"),
            format!("{PREFIX}{ben}.{juniper}.{room}.chat.1.2"),
            format!("{PREFIX_V1}{ben}.{juniper}.{room}.room--k3x9.chat.1.2.{juniper}"),
        ] {
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

    /// Goal (FIN-720): one bridge life speaks for an agent's turns at a
    /// time: the newest heard; a new one of its computer's start or a later
    /// one takes over; one heard before it is retired (its late word changes
    /// nothing), and so is one never heard of an earlier start (a container
    /// gone, its first word late: after its successor's); the same word
    /// again is heard the same.
    #[test]
    fn the_newest_bridge_life_speaks_for_its_turns() {
        assert_eq!(heard(&[], "a", 3), Heard::TakesOver, "the first life heard");
        let lives = [("b", 4), ("a", 3)];
        assert_eq!(heard(&lives, "b", 4), Heard::Speaks);
        assert_eq!(heard(&lives, "a", 3), Heard::Retired, "a late word from the life before");
        assert_eq!(heard(&lives, "c", 4), Heard::TakesOver, "a new life of the same start (its bridge restarted)");
        assert_eq!(heard(&lives, "c", 5), Heard::TakesOver, "a new life of a later start");
        assert_eq!(heard(&lives, "c", 3), Heard::Retired, "a first word of an earlier start's, late");
        assert_eq!(heard(&lives, "b", 4), heard(&lives, "b", 4), "replay");
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
        let held = |text: &'static str, until: i64, place: &'static str| Some(Held { text, until, place, name: None, via: "npub1juniper" });
        let lent = |name: &'static str, place: &'static str| LentAt { name, place, agent: "npub1juniper" };
        let turn = |place: &'static str, asker: &str, mandate: Option<Held<'static>>| Turn { place, asker: Box::leak(asker.to_string().into_boxed_str()), mandate };
        assert_eq!(proof(&ana, &[]), Proof::Bare);
        assert_eq!(proof(&ana, &[turn("own", &ana, None)]), Proof::Bare, "its owner's, unproven: as before");
        let (a1, a2) = (held("a1", 10, "npub1own"), held("a2", 20, "npub1own"));
        assert_eq!(proof(&ana, &[turn("own", &ana, a1), turn("own", &ana, a2)]), Proof::Owner(a2.unwrap()), "its owner's newest");
        let b = held("b", 5, "npub1room");
        let bens = proof(&ana, &[turn("own", &ana, a2), turn("room", &ben, b), turn("hall", &ben, held("b2", 4, "npub1hall")), turn("room", &ben, b)]);
        assert_eq!(bens, Proof::Asker { held: b.unwrap(), at: vec![lent("hall", "npub1hall"), lent("room", "npub1room")] }, "another's beside its owner's is the narrower: their newest, and each place of theirs");
        let busy = proof(&ana, &[turn("room", &ben, b), turn("hall", &carol, held("c", 9, "npub1hall")), turn("room", &carol, None)]);
        assert_eq!(busy, Proof::Itself(vec!["hall", "room"]), "two other people: no one's, in their places");
        assert_eq!(proof(&ana, &[turn("room", &ben, None)]), Proof::Itself(vec!["room"]), "one other, unproven");
        assert_eq!(proof(&ana, &[turn("room", &ben, b), turn("hall", &ben, None)]), Proof::Itself(vec!["hall", "room"]), "one other with a turn unproven");
        // replay: the same turns decide the same
        assert_eq!(proof(&ana, &[turn("room", &ben, b)]), proof(&ana, &[turn("room", &ben, b)]));
    }

    /// Goal (FIN-615, PR 16a): an ask carried to another agent of its
    /// owner's keeps whose it is, where it began and its day, and is lent
    /// by the agent first asked there; it opens for the agent carrying it,
    /// under the same keys, and is altered by no one. A mandate of the first
    /// form still opens (one a turn may hold across the release), lent at
    /// its turn's own place, and is carried by no one.
    #[test]
    fn a_carried_ask_is_the_same_ask_lent_where_it_began() {
        let key = vec![MandateKey::derive(HOST)];
        let fred = crate::npub::encode(&"f".repeat(64));
        let m = ben_asks_juniper(1_000);
        assert_eq!(lent_at(&m.text(&key[0])), Some((m.place.as_str(), Some("room--k3x9"), m.agent.as_str())), "first asked: lent to itself, where it was asked");
        let c = m.carried(&fred).expect("carried");
        assert_eq!((c.asker.as_str(), c.agent.as_str(), c.place.as_str(), c.name.as_deref(), c.channel.as_str(), c.seq, c.until, c.via.as_str()), (m.asker.as_str(), fred.as_str(), m.place.as_str(), Some("room--k3x9"), "chat", 12, m.until, m.agent.as_str()));
        let text = c.text(&key[0]);
        assert_eq!(open(&key, &text, 2_000), Ok(c.clone()));
        assert_eq!(lent_at(&text), Some((m.place.as_str(), Some("room--k3x9"), m.agent.as_str())), "lent where it began, by the agent first asked");
        assert_eq!(c.carried(&crate::npub::encode(&"9".repeat(64))).map(|again| again.via), Some(m.agent.clone()), "carried again, still lent by the first");
        let lent_to_itself = text.replacen(&format!(".{}.", m.agent), &format!(".{fred}."), 2);
        assert_eq!(open(&key, &lent_to_itself, 2_000), Err(Refused::Forged), "no carrier names itself the agent lent");
        // the first form: made before this release, held by an open turn
        let (ben, juniper, room) = (ids().0, ids().1, a_room());
        let fields = format!("{PREFIX_V1}{ben}.{juniper}.{room}.chat.12.{}", m.until);
        let old = format!("{fields}.{}", hex::encode(&key[0].mac(fields.as_bytes()).finalize().into_bytes()[..MARK_BYTES]));
        let opened = open(&key, &old, 2_000).expect("a first-form mandate opens");
        assert_eq!((opened.name.as_deref(), opened.via.as_str()), (None, juniper.as_str()));
        assert_eq!(lent_at(&old), Some((room.as_str(), None, juniper.as_str())), "lent at its turn's own place, to itself");
        assert_eq!(opened.carried(&fred), None, "a first-form ask goes on unproven, as before");
    }

    /// Goal: a carried ask is lent where it began, by the agent first asked
    /// there, beside any other place's (`Proof::Asker::at`).
    #[test]
    fn a_carried_ask_names_where_it_began_for_its_lend() {
        let (ana, ben) = ids();
        let carried = Some(Held { text: "c", until: 9, place: "npub1room", name: Some("room--k3x9"), via: "npub1juniper" });
        let turn = Turn { place: "asks--k3x9", asker: Box::leak(ben.clone().into_boxed_str()), mandate: carried };
        assert_eq!(proof(&ana, &[turn]), Proof::Asker { held: carried.unwrap(), at: vec![LentAt { name: "room--k3x9", place: "npub1room", agent: "npub1juniper" }] });
    }
}
