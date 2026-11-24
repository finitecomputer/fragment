//! Proof of who asked (FIN-615, FIN-720; docs/api.md, "Who asked:
//! mandates"). A mandate is a place's word that a person's own record there
//! asked one of its agents: its asker, the agent, the place, the record,
//! until when, and what the agent's owner lends that place (`Lend`). As a
//! place hands an agent member the records people wrote to it there (`GET
//! …/channels/<c>`, read as the agent itself), each carries one. The
//! agent's calls carry it beside their NIP-98 signature
//! (`x-fragment-mandate`) in place of `for=<its owner>`, and the router
//! checks it in the signer's own lookup, with no round trip: the call acts
//! for its asker, held to their own role (`permit`, rule 3). Its owner's
//! own word lends everything of theirs; anyone else's, only the lend.
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
    /// What the agent's owner lends the place, as it was when the place
    /// marked it; its owner's own mandate needs none (`Lend::OWNER`).
    pub lend: Lend,
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
    /// `channel` at `seq`, at `at` (ms), under `lend`; none when a name is
    /// not of its kind, which no mandate could carry.
    pub fn of_record(asker: &str, agent: &str, place: &str, (channel, seq, at): (&str, i64, i64), lend: Lend) -> Option<Mandate> {
        let m = Mandate { asker: asker.into(), agent: agent.into(), place: place.into(), channel: channel.into(), seq, until: at.saturating_add(limits::MANDATE_LIFE_MS), lend };
        well_formed(&m).then_some(m)
    }

    /// Its text: `fm1.<asker>.<agent>.<place>.<channel>.<seq>.<until>.<lend>.<mark>`,
    /// its mark of all before it. No field holds a dot (each is of its
    /// kind: `well_formed`), so the text reads back as made.
    pub fn text(&self, key: &MandateKey) -> String {
        assert!(well_formed(self), "a mandate is made of names of their kind (`of_record`)");
        let Mandate { asker, agent, place, channel, seq, until, lend } = self;
        let fields = format!("{PREFIX}{asker}.{agent}.{place}.{channel}.{seq}.{until}.{}", lend.bits());
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
    let [asker, agent, place, channel, seq, until, lend] = parts[..] else { return Err(Refused::Malformed) };
    let number = |n: &str| n.parse::<i64>().ok().filter(|_| n.bytes().all(|b| b.is_ascii_digit()));
    let lend = Some(lend).filter(|l| l.len() == 1).and_then(|l| l.parse::<u8>().ok()).and_then(Lend::from_bits);
    let (Some(seq), Some(until), Some(lend)) = (number(seq), number(until), lend) else { return Err(Refused::Malformed) };
    let m = Mandate { asker: asker.into(), agent: agent.into(), place: place.into(), channel: channel.into(), seq, until, lend };
    if !well_formed(&m) {
        return Err(Refused::Malformed);
    }
    if now > m.until {
        return Err(Refused::Expired);
    }
    Ok(m)
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
/// fragment checked it: its text, and what of it the decisions read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Held<'a> {
    pub text: &'a str,
    pub until: i64,
    pub lend: Lend,
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
    /// act for inside the place's lend.
    Asker(Held<'a>),
    /// No single proven asker (two other people's turns, or one with no
    /// proof): the agent acts as itself, in those turns' places alone,
    /// and lends no one anything of its owner's.
    Itself(Vec<&'a str>),
}

impl Proof<'_> {
    /// What of its owner's the agent may use meanwhile (the swap's grants,
    /// narrowed).
    pub fn lend(&self) -> Lend {
        match self {
            Proof::Bare | Proof::Owner(_) => Lend::OWNER,
            Proof::Asker(m) => m.lend,
            Proof::Itself(_) => Lend::NONE,
        }
    }
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
        Some(m) if one && others.iter().all(|t| t.mandate.is_some()) => Proof::Asker(m),
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
        Mandate::of_record(&ben, &juniper, &a_room(), ("chat", 12, at), Lend::DEFAULT).expect("well formed")
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
        let lent = text.replacen(&format!(".{}.1.", m.until), &format!(".{}.3.", m.until), 1);
        assert!(lent != text && open(&old, &lent, 2_000) == Err(Refused::Forged), "its lend widened: {lent}");
        // each lend reads back as marked
        for lend in [Lend::NONE, Lend::DEFAULT, Lend { spend: false, accounts: true }, Lend::OWNER] {
            let m = Mandate { lend, ..m.clone() };
            assert_eq!(open(&old, &m.text(&old[0]), 2_000), Ok(m));
        }
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
        assert!(Mandate::of_record(&ben, &juniper, &room, ("chat", 1, 1), Lend::DEFAULT).is_some());
        for (asker, agent, place, channel, seq) in [
            ("anon:0123456789abcdef0123456789abcdef", juniper.as_str(), room.as_str(), "chat", 1),
            (ben.as_str(), "npub1short", room.as_str(), "chat", 1),
            (ben.as_str(), juniper.as_str(), "chat--k3x9", "chat", 1),
            (ben.as_str(), juniper.as_str(), room.as_str(), "Chat", 1),
            (ben.as_str(), juniper.as_str(), room.as_str(), "chat", 0),
        ] {
            assert_eq!(Mandate::of_record(asker, agent, place, (channel, seq, 1), Lend::DEFAULT), None, "{asker} {agent} {place} {channel} {seq}");
        }
        // marked, yet not of a mandate's form: a field more, or a seq that is no number
        let key = MandateKey::derive(HOST);
        let (ben, juniper, room) = (ids().0, ids().1, a_room());
        for fields in [
            format!("{PREFIX}{ben}.{juniper}.{room}.chat.1.2.1.9"),
            format!("{PREFIX}{ben}.{juniper}.{room}.chat.+1.2.1"),
            format!("{PREFIX}{ben}.{juniper}.{room}.chat.1.2.4"),
            format!("{PREFIX}{ben}.{juniper}.{room}.chat.1.2"),
            format!("{PREFIX}{ben}.{juniper}.chat--k3x9.chat.1.2.1"),
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

    /// Goal: the narrowest ask an agent's call for its owner carries while
    /// turns are open (`Proof`): its owner's newest proof alone; anyone
    /// else's beside it narrows it, theirs inside the place's lend; two
    /// other people, or one unproven, leave no one's: the agent itself, in
    /// their places alone, lending nothing; no turn proven of its owner's,
    /// or none at all, is the bare `for=` as before.
    #[test]
    fn the_narrowest_ask_is_carried() {
        let (ana, ben) = ids();
        let carol = crate::npub::encode(&"e".repeat(64));
        let held = |text: &'static str, until: i64, lend: Lend| Some(Held { text, until, lend });
        let turn = |place: &'static str, asker: &str, mandate: Option<Held<'static>>| Turn { place, asker: Box::leak(asker.to_string().into_boxed_str()), mandate };
        assert_eq!(proof(&ana, &[]), Proof::Bare);
        assert_eq!(proof(&ana, &[turn("own", &ana, None)]), Proof::Bare, "its owner's, unproven: as before");
        let (a1, a2) = (held("a1", 10, Lend::OWNER), held("a2", 20, Lend::OWNER));
        assert_eq!(proof(&ana, &[turn("own", &ana, a1), turn("own", &ana, a2)]), Proof::Owner(a2.unwrap()), "its owner's newest");
        let b = held("b", 5, Lend::DEFAULT);
        let bens = proof(&ana, &[turn("own", &ana, a2), turn("room", &ben, b), turn("room", &ben, held("b2", 4, Lend::DEFAULT))]);
        assert_eq!((bens.clone(), bens.lend()), (Proof::Asker(b.unwrap()), Lend::DEFAULT), "another's beside its owner's is the narrower");
        let busy = proof(&ana, &[turn("room", &ben, b), turn("hall", &carol, held("c", 9, Lend::OWNER)), turn("room", &carol, None)]);
        assert_eq!((busy.clone(), busy.lend()), (Proof::Itself(vec!["hall", "room"]), Lend::NONE), "two other people: no one's, in their places");
        assert_eq!(proof(&ana, &[turn("room", &ben, None)]), Proof::Itself(vec!["room"]), "one other, unproven");
        assert_eq!(proof(&ana, &[turn("room", &ben, b), turn("hall", &ben, None)]), Proof::Itself(vec!["hall", "room"]), "one other with a turn unproven");
        assert_eq!((Proof::Bare.lend(), Proof::Owner(a1.unwrap()).lend()), (Lend::OWNER, Lend::OWNER));
        // replay: the same turns decide the same
        assert_eq!(proof(&ana, &[turn("room", &ben, b)]), proof(&ana, &[turn("room", &ben, b)]));
    }
}
