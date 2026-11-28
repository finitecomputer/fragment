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
//! **Carried** (FIN-615, PR 16a: `fragment ask`, a hand-off): an agent on
//! its owner's own proven ask that asks another agent of its owner's hands
//! that agent the same ask (`Mandate::carried`), so it too acts for its
//! owner, proven, for as long as the ask holds. Only its owner's own ask is
//! carried (the caller checks): anyone else's is lent and capped per agent,
//! and goes on unproven, the agent acting as itself (docs/api.md).

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

    /// This ask carried by its agent to `agent`, another agent of the same
    /// owner's it asked, the ask its owner's own (the caller checks both):
    /// the same asker, place, record and day, so it holds no longer and says
    /// no more than the ask it came from. None for a name not of its kind.
    pub fn carried(&self, agent: &str) -> Option<Mandate> {
        let m = Mandate { agent: agent.into(), ..self.clone() };
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

/// A mandate's fields read from a text the platform kept after it opened
/// it (`records.carried`): no mark is checked, so never a text from a
/// request.
pub fn kept(text: &str) -> Option<Mandate> {
    let fields = text.strip_prefix(PREFIX)?.rsplit_once('.')?.0;
    let parts: Vec<&str> = fields.split('.').collect();
    let [asker, agent, place, channel, seq, until] = parts[..] else { return None };
    let m = Mandate { asker: asker.into(), agent: agent.into(), place: place.into(), channel: channel.into(), seq: seq.parse().ok()?, until: until.parse().ok()? };
    well_formed(&m).then_some(m)
}

/// The place a mandate's text names (its key), read from a text already
/// opened (`open`), as an agent fragment keeps one for an open turn.
pub fn place_of(text: &str) -> Option<&str> {
    text.strip_prefix(PREFIX)?.split('.').nth(2)
}

/// The record a mandate's text names (the asker's request in its place):
/// its channel and its seq, read as `place_of` reads its place. Allow once
/// lends the first such request after its answer (docs/api.md, "One ask").
pub fn record_of(text: &str) -> Option<(&str, i64)> {
    let mut fields = text.strip_prefix(PREFIX)?.split('.').skip(3);
    Some((fields.next()?, fields.next()?.parse().ok()?))
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
    let to = to_of(&v);
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

/// Whom a record's body names in its `to` (none: no one in particular).
pub fn named(body: &str) -> Vec<String> {
    let v: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
    to_of(&v).into_iter().map(str::to_string).collect()
}

fn to_of(v: &serde_json::Value) -> Vec<&str> {
    v.get("to").and_then(|t| t.as_array()).map(|t| t.iter().filter_map(|a| a.as_str()).collect()).unwrap_or_default()
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
    /// An agent's words: the ask they were said on carried on, where that
    /// is their owner's own (`records.carried`, `Mandate::carried`), else
    /// the agent's own, with nothing to prove it.
    Agent(String),
    /// Words its bridge runs that no one's word proves: app code's, a
    /// door's that relays no one, the platform's own. An ask in the place's
    /// own name, which nothing proves: the agent acts as itself, in that
    /// place alone (`Proof::Itself`), as on anyone's unproven ask.
    NoOne,
}

/// Whose ask a record is (`Asked`), or none: an anonymous visitor's words,
/// whoever wrote them, which its bridge never runs.
pub fn asked_of(principal: &str, author: Option<Author>, asker: Option<&str>, body: &str) -> Option<Asked> {
    if !crate::npub::is_identity(principal) {
        return None;
    }
    if author == Some(Author::Agent) {
        return Some(Asked::Agent(principal.to_string()));
    }
    Some(asker_of(principal, author, asker, body).map_or(Asked::NoOne, Asked::By))
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
/// message (no `kind`, or `message`) that says something, as an agent's
/// bridge hears one; a stop, a card's answer, a command or an object
/// with no words starts none.
pub fn starts_turn(body: &str) -> bool {
    use serde_json::Value;
    match serde_json::from_str::<Value>(body) {
        Ok(Value::String(_)) => true,
        // words, as its bridge reads a message (records.rs `message`): a
        // `text`, or with none, attachments; a status object says nothing
        Ok(Value::Object(o)) => {
            let words = match o.get("text") {
                Some(text) => text.is_string(),
                None => o.get("attachments").and_then(Value::as_array).is_some_and(|a| !a.is_empty()),
            };
            o.get("kind").is_none_or(|kind| kind == "message") && words
        }
        _ => false,
    }
}

/// Whether an agent's call is its presence in its chats, which stays its
/// own and carries no one's mandate: its live socket, and a fragment's
/// channels (their list, records, newest; posts and drafts on `chat` and
/// `work`, where it speaks: an app's own channel is work), blobs, roster
/// (`members`, `__people`), status and subscriptions; its own fragment
/// list; and anything on its own fragment (`own`, its name), its own
/// state. Every other call is work, held to whose ask is open (FIN-720):
/// this one list decides, and a route not on it is work.
/// The channels an agent speaks on as itself in a chat: its words, and its
/// turns' records. A post anywhere else (an app's own channel) is work,
/// for whoever asked.
const SPOKEN: [&str; 2] = ["chat", "work"];

pub fn presence(method: &str, path: &str, own: &str) -> bool {
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    let reads = matches!(method, "GET" | "HEAD");
    match parts.as_slice() {
        ["f", name, ..] | ["api", "f", name, ..] if *name == own => true,
        ["f", _, "__live"] => reads,
        ["f", _, "__people"] => reads,
        ["api", "fragments"] => reads,
        ["api", "f", _, "status" | "members" | "channels" | "subscriptions"] => reads,
        ["api", "f", _, "channels", c] => reads || (method == "POST" && SPOKEN.contains(c)),
        ["api", "f", _, "channels", _, "newest"] => reads,
        ["api", "f", _, "channels", c, "draft"] => method == "PUT" && SPOKEN.contains(c),
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

/// Whether a route takes whom an agent acts for (`for`, or a mandate in its
/// place): a fragment's API, the fragment list, the invites waiting on an
/// owner's email, a search across fragments, a move's copy. The router
/// refuses either anywhere else (lib.rs `FOR_HONORED`). The one list: the
/// CLI names its owner by it (cli/src/api.rs `honors_for`).
pub fn honors_for(method: &str, path: &str) -> bool {
    let p = path.split('?').next().unwrap_or(path);
    let stage_copy = method == "POST" && p.starts_with("/api/migrations/stages/") && p.ends_with("/copy");
    p.starts_with("/api/f/") || p == "/api/fragments" || p == "/api/invites" || p.starts_with("/api/invites/") || (method == "POST" && p == "/api/search") || stage_copy
}

/// One of an agent's calls, as its egress reads it (`signs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Call<'a> {
    /// Its computer says it is its owner's work (`for=<its owner>`): what
    /// the call is for, never whose it may be.
    pub for_owner: bool,
    /// Its presence in its chats (`presence`): its own, unless its
    /// computer says it is its owner's.
    pub presence: bool,
    /// The fragment it goes to, by name, when it goes to one.
    pub fragment: Option<&'a str>,
    /// It goes to that fragment's site (`/f/<name>/…`), which takes no
    /// one's word but its signer's.
    pub site: bool,
    /// Its route takes whom it acts for (`honors_for`).
    pub honors_for: bool,
    /// The platform's invites, waiting on its owner's email.
    pub invites: bool,
    /// Its route is what only its owner does by it: sharing
    /// (`permit::reserved`: members, invites, visibility, a link's
    /// rotation, an address) or answering the invites waiting on their
    /// email. Its owner's work only as its computer says so, never
    /// unnamed (decision 64 lends their role at most an editor's).
    pub reserved: bool,
}

/// How an agent's egress signs a call (FIN-615): its computer says what a
/// call is for (`Call::for_owner`), and the turns its places opened say
/// what it may be (`proof`). Whom an app is told asked is the asker of the
/// narrowest open turn, and with none its owner (decision 64: the agent
/// and its owner are one entity); the agent is credited beside them always.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signs<'a> {
    /// As the agent itself: its presence, a site, or a route that takes no
    /// one's word.
    Itself,
    /// For its owner (`for`): no one else's turn is open, nor its owner's.
    ForOwner,
    /// By a mandate: someone else's, theirs the one open turn; or its
    /// owner's own, while their turn is open, so what it says there carries
    /// their ask (a hand-off).
    Mandate(Held<'a>),
    /// Refused, and why.
    Refused(&'static str),
}

/// `signs`'s rule (`Signs`), the one place it is decided.
pub fn signs<'a>(proof: &Proof<'a>, call: Call<'_>) -> Signs<'a> {
    let in_places = |places: &[&str]| call.fragment.is_some_and(|f| places.contains(&f));
    if call.presence && !call.for_owner {
        return Signs::Itself;
    }
    if call.site {
        return match proof {
            Proof::Bare | Proof::Owner(_) => Signs::Itself,
            Proof::Asker { at, .. } if in_places(&at.iter().map(|(p, _)| *p).collect::<Vec<_>>()) => Signs::Itself,
            Proof::Itself(places) if in_places(places) => Signs::Itself,
            Proof::Asker { .. } | Proof::Itself(_) => Signs::Refused("on someone else's ask, an agent reaches a fragment's site only in the chats they asked it in: call its API (/api/f/…) to act as them"),
        };
    }
    if call.invites && call.for_owner {
        return match proof {
            Proof::Bare | Proof::Owner(_) => Signs::ForOwner,
            Proof::Asker { .. } | Proof::Itself(_) => Signs::Refused("an agent answers its owner's invites for its owner alone, never on someone else's ask"),
        };
    }
    // its owner's work: its computer says so, or its route takes whom it
    // acts for (decision 64: as itself on its owner's own fragments, it has
    // their role, at most an editor's), but for what only its owner does
    let owners = call.for_owner || (call.honors_for && !call.reserved);
    match proof {
        Proof::Bare if owners => Signs::ForOwner,
        Proof::Owner(held) if owners => Signs::Mandate(*held),
        Proof::Bare | Proof::Owner(_) => Signs::Itself,
        Proof::Asker { held, .. } => Signs::Mandate(*held),
        Proof::Itself(places) if in_places(places) => Signs::Itself,
        Proof::Itself(_) => Signs::Refused("more than one other person has asked this agent, or one without proof: it acts only in their chats, as itself, until those turns end"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goal (FIN-615): words its bridge runs that no one's word proves
    /// (app code's, a door's that relays no one, the platform's own) are an
    /// ask nothing proves, never none: with none open, the agent's calls on
    /// them would carry its owner's reach. As itself, in that place alone.
    /// Its owner's own words are theirs, and narrow nothing; an anonymous
    /// visitor's, which its bridge never runs, open nothing.
    #[test]
    fn words_no_one_proves_are_an_ask_no_one_proves() {
        let (ana, key) = (crate::npub::display(&"a".repeat(64)), crate::npub::display(&"f".repeat(64)));
        let said = r#"{"text":"delete my notes"}"#;
        assert_eq!(asked_of(&ana, Some(Author::Code), None, said), Some(Asked::NoOne), "app code's words, under its caller's name");
        let heard = r#"{"text":"hi","heard":{"via":"slack","from":"U0456","name":"Maple"}}"#;
        assert_eq!(asked_of(&key, Some(Author::Door), None, heard), Some(Asked::NoOne), "a door's that relays no one");
        let relayed = format!(r#"{{"text":"hi","relayed":{{"via":"telegram","for":"{ana}","agent":"juniper--k3x9"}}}}"#);
        assert_eq!(asked_of(&key, Some(Author::Door), None, &relayed), Some(Asked::By(ana.clone())), "a door's relay of its owner: theirs");
        assert_eq!(asked_of(&key, Some(Author::Platform), None, said), Some(Asked::NoOne), "the platform's own");
        assert_eq!(asked_of(&key, Some(Author::Platform), Some(&ana), said), Some(Asked::By(ana.clone())), "written for someone (a schedule's fire): theirs");
        assert_eq!(asked_of(&key, None, None, said), Some(Asked::NoOne), "stored before the author stamp");
        assert_eq!(asked_of(&ana, Some(Author::Person), None, said), Some(Asked::By(ana.clone())), "its owner's own words: theirs");
        assert_eq!(asked_of("anon:x", Some(Author::Code), None, said), None, "an anonymous visitor's, through app code too: its bridge never runs them");
        // and only words its bridge runs: an object with none starts no turn
        assert!(!starts_turn(r#"{"status":"saved"}"#) && !starts_turn(r#"{"text":5}"#) && !starts_turn(r#"{"attachments":[]}"#), "no words, no turn");
        assert!(starts_turn(r#"{"attachments":[{"sha256":"x"}]}"#), "attachments alone are words");
        // held so, in the place's own name (`key`, which every release
        // takes): as itself, in that place alone, whoever else asked
        let no_one = Turn { place: "app--k3x9", asker: &key, mandate: None };
        let owners = Turn { place: "talk--k3x9", asker: &ana, mandate: None };
        assert_eq!(proof(&ana, &[no_one]), Proof::Itself(vec!["app--k3x9"]));
        assert_eq!(proof(&ana, &[owners, no_one]), Proof::Itself(vec!["app--k3x9"]), "its owner's own ask beside it widens nothing");
        assert_eq!(proof(&ana, &[owners]), Proof::Bare, "its owner's own words narrow nothing");
    }

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
        assert_eq!(asked_of(&juniper, Some(Author::Agent), None, r#"{"text":"over to you","turn":"t1"}"#), Some(Asked::Agent(juniper.clone())), "an agent's words: the ask they were said on, else its own");
        assert_eq!(asked_of("anon:x", Some(Author::Person), None, r#"{"text":"hi"}"#), None, "an anonymous visitor asks no one");
        let agents = ["npub1lead", "npub1maple"];
        assert_eq!(heard_by(r#"{"text":"hi"}"#, false, &agents), vec!["npub1lead"], "a person's words naming no one are the lead's");
        assert_eq!(heard_by(r#"{"text":"hi","to":["npub1maple"]}"#, false, &agents), vec!["npub1maple"], "`to` names whom");
        assert_eq!(heard_by(r#"{"text":"@maple hi"}"#, false, &agents), agents.to_vec(), "a mention: every agent, the place knowing no names");
        assert_eq!(heard_by(r#"{"text":"mail a@b.com"}"#, false, &agents), vec!["npub1lead"], "an email address is no mention");
        assert_eq!(heard_by(r#"{"text":"over to you"}"#, true, &agents), Vec::<&str>::new(), "an agent's words are to whom `to` names alone");
        assert_eq!(heard_by(r#"{"text":"over","to":["npub1maple"]}"#, true, &agents), vec!["npub1maple"]);
        assert_eq!(heard_by(r#""hi""#, false, &[]), Vec::<&str>::new(), "no agents, no one");
        assert_eq!(named(r#"{"text":"hi","to":["npub1maple"]}"#), vec!["npub1maple".to_string()]);
        assert!(named(r#"{"text":"@maple hi"}"#).is_empty(), "a mention names no one in `to`");
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
        assert_eq!(kept(&text), Some(m.clone()), "a kept text reads back as made");
        assert_eq!(kept("fm1.not.a.mandate"), None);
    }

    /// Goal (FIN-615 PR 8): its computer says what a call is for (its
    /// owner's work, `for=<its owner>`, or its own), and the turns its
    /// places opened say whose it may be: the narrowest open turn's asker;
    /// with none, its owner; on its owner's own turn, their mandate, so a
    /// hand-off carries their ask. A route that takes whom it acts for is
    /// its owner's work unasked (decision 64). The obvious cases, as the
    /// egress signs each.
    #[test]
    fn what_a_call_is_for_is_its_computers_word_and_whose_its_turns() {
        let (ana, ben) = (crate::npub::display(&"a".repeat(64)), crate::npub::display(&"b".repeat(64)));
        let bens = Held { text: "fm1.ben", until: 9, place: "npub1chat" };
        let anas = Held { text: "fm1.ana", until: 9, place: "npub1dm" };
        fn in_chat<'a>(asker: &'a str, mandate: Option<Held<'a>>, place: &'a str) -> Turn<'a> {
            Turn { place, asker, mandate }
        }
        // the turns borrow their askers' names for the test's whole life
        let (ana_s, ben_s): (&'static str, &'static str) = (String::leak(ana.clone()), String::leak(ben.clone()));
        let none: Vec<Turn> = vec![];
        let anas_turn = vec![in_chat(ana_s, Some(anas), "dm--k3x9")];
        let bens_turn = vec![in_chat(ana_s, Some(anas), "dm--k3x9"), in_chat(ben_s, Some(bens), "talk--k3x9")];
        let two = vec![in_chat(ben_s, Some(bens), "talk--k3x9"), in_chat("npub1carol", None, "talk--k3x9")];
        let owner = ana.clone();
        let proof = |turns: &[Turn<'static>]| super::proof(&owner, turns);
        // Calories' "log my food", its computer naming its owner or not: the app is told whom it is for
        let app = Call { for_owner: false, presence: false, fragment: Some("calories--k3x9"), site: false, honors_for: true, invites: false, reserved: false };
        for app in [app, Call { for_owner: true, ..app }] {
            assert_eq!(signs(&proof(&none), app), Signs::ForOwner, "no turn open (a routine, the skill sync, a delegate): for its owner");
            assert_eq!(signs(&proof(&anas_turn), app), Signs::Mandate(anas), "on its owner's own ask: by their mandate");
            assert_eq!(signs(&proof(&bens_turn), app), Signs::Mandate(bens), "on Ben's ask, beside its owner's: as Ben, whatever its computer said");
            assert!(matches!(signs(&proof(&two), app), Signs::Refused(_)), "two others' asks, or one unproven: outside their chats refused");
            assert_eq!(signs(&proof(&two), Call { fragment: Some("talk--k3x9"), ..app }), Signs::Itself, "in their chat, as itself");
        }
        // presence it says is its own stays its own, whoever asked
        let post = Call { presence: true, fragment: Some("talk--k3x9"), ..app };
        for turns in [&none, &anas_turn, &bens_turn, &two] {
            assert_eq!(signs(&proof(turns), post), Signs::Itself, "its own words in its chat are the agent's own");
        }
        // presence it says is its owner's (its owner's list, a status with
        // their share link, a hand-off's question): theirs while no one
        // else's turn is open, and held to that turn while one is
        let theirs = Call { for_owner: true, ..post };
        assert_eq!(signs(&proof(&none), theirs), Signs::ForOwner);
        assert_eq!(signs(&proof(&anas_turn), theirs), Signs::Mandate(anas), "a hand-off on its owner's own ask carries it");
        assert_eq!(signs(&proof(&bens_turn), theirs), Signs::Mandate(bens), "never its owner's on Ben's ask");
        // a route that takes no one's word goes as itself, but for someone else's ask
        let notices = Call { for_owner: false, presence: false, fragment: None, site: false, honors_for: false, invites: false, reserved: false };
        assert_eq!(signs(&proof(&none), notices), Signs::Itself);
        assert_eq!(signs(&proof(&bens_turn), notices), Signs::Mandate(bens), "Ben's mandate, which that route refuses (400): fail closed");
        // a site takes its signer's word alone, whatever its computer said
        let site = Call { site: true, honors_for: false, ..app };
        for site in [site, Call { for_owner: true, ..site }] {
            assert_eq!(signs(&proof(&none), site), Signs::Itself);
            assert!(matches!(signs(&proof(&bens_turn), site), Signs::Refused(_)), "on Ben's ask, a site outside his chats");
            assert_eq!(signs(&proof(&bens_turn), Call { fragment: Some("talk--k3x9"), ..site }), Signs::Itself, "his chat's own site, as itself");
        }
        // what only its owner does (sharing their app, answering their
        // invites): theirs only as its computer says so, never unnamed
        let shares = Call { reserved: true, ..app };
        assert_eq!(signs(&proof(&none), shares), Signs::Itself, "unnamed, as itself: its own membership never shares");
        assert_eq!(signs(&proof(&none), Call { for_owner: true, ..shares }), Signs::ForOwner, "named, for its owner, as they share by it");
        assert_eq!(signs(&proof(&bens_turn), Call { for_owner: true, ..shares }), Signs::Mandate(bens), "never its owner's on Ben's ask");
        // its owner's invites, whose route takes `for` and no mandate
        let invites = Call { for_owner: true, presence: false, fragment: None, site: false, honors_for: true, invites: true, reserved: true };
        assert_eq!(signs(&proof(&none), invites), Signs::ForOwner);
        assert_eq!(signs(&proof(&anas_turn), invites), Signs::ForOwner);
        assert!(matches!(signs(&proof(&bens_turn), invites), Signs::Refused(_)), "never on someone else's ask");
        assert_eq!(signs(&proof(&none), Call { for_owner: false, ..invites }), Signs::Itself, "unnamed, its own");
        // the routes that take whom it acts for
        for (method, path, takes) in [("GET", "/api/f/x/ops", true), ("POST", "/api/fragments", true), ("GET", "/api/invites", true), ("POST", "/api/search", true), ("GET", "/api/search", false), ("POST", "/api/migrations/stages/s1/copy", true), ("GET", "/api/notices", false), ("GET", "/f/x/page", false), ("POST", "/api/models/v1/chat/completions", false)] {
            assert_eq!(honors_for(method, path), takes, "{method} {path}");
        }
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
            ("POST", "/api/f/board--k3x9/channels/said"),
            ("PUT", "/api/f/board--k3x9/channels/said/draft"),
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
        // what an agent fragment reads of one it kept: its place and its record
        assert_eq!((place_of(&text), record_of(&text)), (Some(m.place.as_str()), Some((m.channel.as_str(), m.seq))));
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

    /// Goal (FIN-615, PR 16a): an ask carried to another agent is the same
    /// ask: its asker, place, record and day, made for that agent alone, so
    /// it opens for it under the same keys, says nothing more, and the agent
    /// that carried it can't carry the text made for the other.
    #[test]
    fn a_carried_ask_is_the_same_ask_for_another_agent() {
        let key = vec![MandateKey::derive(HOST)];
        let rowan = crate::npub::encode(&"f".repeat(64));
        let m = ben_asks_juniper(1_000);
        let c = m.carried(&rowan).expect("carried");
        assert_eq!(c, Mandate { agent: rowan.clone(), ..m.clone() });
        assert_eq!(open(&key, &c.text(&key[0]), 2_000), Ok(c.clone()));
        assert_eq!(open(&key, &c.text(&key[0]), c.until + 1), Err(Refused::Expired), "no longer than the ask it came from");
        assert_ne!(c.text(&key[0]), m.text(&key[0]), "made for the agent asked alone");
        assert_eq!(m.carried("npub1short"), None);
    }
}
