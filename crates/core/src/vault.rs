//! A person's Vault (FIN-466; docs/secrets.md, "A person's Vault"): the
//! rules a secret is held to as it is put. The cell's `Vault` object
//! seals what passes and answers its agents' swaps; this decides, purely,
//! what a secret may be.
//!
//! A secret is a value, the hosts it may go to, where it goes on a
//! request (`Placing`), and the agents that may use it. Only its person
//! sets one, so only they say where its value goes: with a value, or
//! keeping the one it holds where that value means the same (`kept`).

use fragment_proto::notice::{Notice, NoticeAction, NoticeField, NoticeLevel, Until};
use fragment_proto::vault::{AskSecret, Place, PutSecret, Secret};

use crate::catalog::{self, Catalog, Half, Placement};

/// The most secrets one person keeps.
pub const SECRETS_MAX: usize = 64;
/// The most hosts one secret may go to.
pub const HOSTS_MAX: usize = fragment_proto::limits::SECRET_HOSTS_MAX;
/// The most agents one secret is bound to: a computer's agents, at most.
pub const AGENTS_MAX: usize = 16;
/// A filled value, at most: an own key's bound (`own_signin`).
pub const VALUE_MAX_BYTES: usize = crate::own_signin::OWN_KEY_MAX_BYTES;

/// What a signing secret's value is.
pub const NOSTR_KEY: &str = "a Nostr secret key, as 64 lower-case hex";

/// Where a secret goes on a request, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placing {
    /// Its value, where a catalog's credential may go (`catalog::Placement`).
    Fill(Placement),
    /// A NIP-98 signature over the request, made with it (a Nostr key),
    /// in `Authorization: Nostr …`; the key never leaves the Vault.
    Nip98,
}

impl Placing {
    /// Where the swap puts what the Vault answers: a fill's own place, or
    /// a signature's header, the event in place of `{}`.
    pub fn placement(&self) -> Placement {
        match self {
            Placing::Fill(p) => p.clone(),
            Placing::Nip98 => Placement::Header { name: "authorization".into(), format: "Nostr {}".into() },
        }
    }
}

/// A secret as the swap plans it beside the catalog's providers
/// (`Catalog::with`): an own key's row of its own, its hosts and its one
/// place, no environment variable.
pub fn row(name: &str, hosts: Vec<String>, placement: Placement) -> catalog::Provider {
    catalog::Provider {
        name: name.to_string(),
        kind: catalog::Kind::Own,
        hosts,
        placements: vec![placement],
        env: vec![],
        price: None,
        oauth: None,
        connection_oauth: None,
        accounts_max: None,
        account_host: None,
        client_auth: catalog::ClientAuth::Post,
        models: None,
        note: None,
    }
}

/// A secret that passed, in the form the Vault stores: its hosts and
/// agents sorted and each once, its place as given (its default spelled out).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    pub hosts: Vec<String>,
    pub place: Place,
    pub placing: Placing,
    pub agents: Vec<String>,
}

/// Why a secret is refused, typed; each says what to do instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultError {
    Name(String),
    Value(String),
    Hosts(String),
    Place(String),
    Agents(String),
    /// An agent's ask, or its person's answer to one.
    Ask(String),
}

impl std::fmt::Display for VaultError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VaultError::Name(why) => write!(f, "a secret's name: {why}"),
            VaultError::Value(why) => write!(f, "a secret's value: {why}"),
            VaultError::Hosts(why) => write!(f, "a secret's hosts: {why}"),
            VaultError::Place(why) => write!(f, "where a secret goes: {why}"),
            VaultError::Agents(why) => write!(f, "a secret's agents: {why}"),
            VaultError::Ask(why) => write!(f, "an ask: {why}"),
        }
    }
}

impl std::error::Error for VaultError {}

/// Whether `name` has a secret's name's form: a catalog name's (1 to 32 of
/// a-z, 0-9 and -). Checked before a name goes anywhere, a Vault's route
/// above all: no dot, slash or percent sign gets there to choose another.
pub fn name_form(name: &str) -> Result<(), VaultError> {
    match catalog::valid_name(name) {
        true => Ok(()),
        false => Err(VaultError::Name(format!("{name:?} is not one: 1 to 32 of a-z, 0-9 and -, not starting or ending with -"))),
    }
}

/// Whether `name` may name a secret: its form (`name_form`), and no key
/// the deployment's catalog offers (a key's placeholder, `fck_`, names one
/// or the other). A connection's name may be a secret's too: its
/// placeholder is `fcx_`, so a secret named before the deployment offered
/// a connection of that name stays its person's (`Catalog::named`).
pub fn name_ok(name: &str, catalog: &Catalog) -> Result<(), VaultError> {
    name_form(name)?;
    if catalog.named(name, false).is_some() {
        return Err(VaultError::Name(format!("{name:?} is a key this deployment offers: name it otherwise")));
    }
    Ok(())
}

/// The place `place` names, checked: none of its fields, the default
/// (`Authorization: Bearer {}`); otherwise exactly one of a header (with
/// its format, `{}` alone if none), a query parameter, a half of basic
/// auth, or a signature.
pub fn placing(place: &Place) -> Result<(Placing, Place), VaultError> {
    let refused = || VaultError::Place("one of {\"header\", \"format\"?}, {\"query\"}, {\"basic\": \"user\"|\"password\"} or {\"sign\": \"nip98\"}".into());
    let placing = match (&place.header, &place.format, &place.query, place.basic.as_deref(), place.sign.as_deref()) {
        (None, None, None, None, None) => Placing::Fill(Placement::Header { name: "authorization".into(), format: "Bearer {}".into() }),
        (Some(name), format, None, None, None) => Placing::Fill(Placement::Header { name: name.clone(), format: format.clone().unwrap_or_else(|| "{}".into()) }),
        (None, None, Some(name), None, None) => Placing::Fill(Placement::Query { name: name.clone() }),
        (None, None, None, Some("user"), None) => Placing::Fill(Placement::Basic { half: Half::User }),
        (None, None, None, Some("password"), None) => Placing::Fill(Placement::Basic { half: Half::Password }),
        (None, None, None, None, Some("nip98")) => Placing::Nip98,
        _ => return Err(refused()),
    };
    if let Placing::Fill(p) = &placing {
        catalog::placement_ok(p).map_err(VaultError::Place)?;
    }
    // as stored and answered: the default spelled out
    let stored = match &placing {
        Placing::Fill(Placement::Header { name, format }) => Place { header: Some(name.clone()), format: Some(format.clone()), ..Place::default() },
        _ => place.clone(),
    };
    Ok((placing, stored))
}

/// Whether `host` is one a secret may go to: an exact public DNS name
/// (`api.example.com`), lower case, with no port, path or wildcard; never
/// an address, a single label, or the platform's own `.internal` names,
/// which the computer's egress answers itself.
pub fn host_ok(host: &str) -> bool {
    let Ok(h) = crate::secrets::secret_host(host) else { return false };
    let tld_numeric = h.rsplit('.').next().is_some_and(|tld| tld.bytes().all(|b| b.is_ascii_digit()));
    h == host && catalog::valid_host(&h) && !tld_numeric && !h.ends_with(".internal") && h != "localhost"
}

/// `put`, under `name`, checked against the rules above; `held`, the
/// place of the secret already kept under `name`, if one is. A put with no
/// value keeps the one held (`kept`).
pub fn check(name: &str, put: &PutSecret, held: Option<&Place>, catalog: &Catalog) -> Result<Checked, VaultError> {
    name_ok(name, catalog)?;
    let (placing, place) = placing(&put.place)?;
    match (&placing, put.value.as_deref()) {
        (_, None) => kept(name, held, &placing)?,
        (Placing::Fill(_), Some(value)) => {
            if !(1..=VALUE_MAX_BYTES).contains(&value.len()) || !value.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
                return Err(VaultError::Value(format!("1 to {VALUE_MAX_BYTES} printable characters, with no space")));
            }
        }
        (Placing::Nip98, Some(value)) => {
            // a key on the curve too: the cell's to check, where its signer is
            if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
                return Err(VaultError::Value(NOSTR_KEY.into()));
            }
        }
    }
    if put.hosts.len() > HOSTS_MAX {
        return Err(VaultError::Hosts(format!("at most {HOSTS_MAX}, not {}", put.hosts.len())));
    }
    if let Some(bad) = put.hosts.iter().find(|h| !host_ok(h)) {
        return Err(VaultError::Hosts(format!("{bad:?} is not one: an exact public DNS name, lower case, with no scheme, port, path or wildcard")));
    }
    let mut hosts = put.hosts.clone();
    hosts.sort();
    hosts.dedup();
    if put.agents.is_empty() || put.agents.len() > AGENTS_MAX {
        return Err(VaultError::Agents(format!("name 1 to {AGENTS_MAX} of your agents, each by its fragment's name")));
    }
    if let Some(bad) = put.agents.iter().find(|a| !fragment_proto::valid_fragment_name(a)) {
        return Err(VaultError::Agents(format!("{bad:?} is not an agent's fragment name (label--suffix)")));
    }
    let mut agents = put.agents.clone();
    agents.sort();
    agents.dedup();
    Ok(Checked { hosts, place, placing, agents })
}

/// May `name`, set again with no value, keep the one it holds (placed as
/// `held`) and go as `placing` says? Only a secret it holds, and only as
/// the same kind of place: a fill's value is a token, a signature's a
/// Nostr key, so going between the two needs a value of the other's rules.
/// A held value passed these rules as it was put, so it is never opened
/// to be checked again.
fn kept(name: &str, held: Option<&Place>, placing: &Placing) -> Result<(), VaultError> {
    let held = held.ok_or_else(|| VaultError::Value(format!("{name} is no secret of yours yet: a new one needs its value")))?;
    // a stored place that reads as none is no value to keep: set it whole
    let was = self::placing(held).map_err(|_| VaultError::Value(format!("{name}'s place is no longer one: set it again with its value")))?.0;
    match (was, placing) {
        (Placing::Fill(_), Placing::Fill(_)) | (Placing::Nip98, Placing::Nip98) => Ok(()),
        (Placing::Nip98, Placing::Fill(_)) => Err(VaultError::Value(format!("{name} is a Nostr key that signs: to fill a request instead, set it again with a value"))),
        (Placing::Fill(_), Placing::Nip98) => Err(VaultError::Value(format!("{name} fills a request: to sign instead, set it again with a Nostr key"))),
    }
}

/// A secret's move out of its Vault, into a fragment of its person's as
/// that fragment's secret (FIN-466; the cell's `vault/take` and
/// `vault/moved`): one home, moved once, never lost.
///
/// A take claims the value it reads for one destination (`fragment/KEY`),
/// kept in the Vault's own row in the turn that reads it; a take for any
/// other destination is refused while that claim stands. The destination
/// keeps the value, then completes: the Vault removes it only for its
/// claimant, and only while it still holds the value taken. So two moves
/// never both keep it, and a move cut short anywhere leaves it in the
/// Vault, claimed: the same move again takes it again and finishes it.
/// A claim has no clock (one that lapsed could let a second destination
/// keep a value the first had kept already); setting the secret again with
/// a value, or removing it, ends it (set again with none, it keeps both).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claimed {
    /// The secret's name in its Vault.
    pub name: String,
    /// The destination its value is claimed for.
    pub to: String,
}

impl std::fmt::Display for Claimed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} is being moved into {}: move it there again to finish, or set it again with a value in your vault", self.name, self.to)
    }
}

impl std::error::Error for Claimed {}

/// May a move into `to` take `name`, its row claimed for `claim`? While
/// unclaimed, or claimed for `to` itself (a retry, or the same move twice).
/// The caller writes `to` as its claim in the same turn.
pub fn take(name: &str, claim: Option<&str>, to: &str) -> Result<(), Claimed> {
    match claim {
        Some(other) if other != to => Err(Claimed { name: name.to_string(), to: other.to_string() }),
        _ => Ok(()),
    }
}

/// Does `to`'s completed move of revision `taken` remove the secret, its
/// row claimed for `claim` and now holding revision `held`? Only for its
/// claimant, and only the value it took: one set since stays.
pub fn completes(claim: Option<&str>, held: &str, to: &str, taken: &str) -> bool {
    claim == Some(to) && held == taken
}

/// How long an ask waits on its person's card.
pub const ASK_TTL_MS: i64 = 10 * 60_000;
/// How long an ask that ended is still read by its agent (a CLI that
/// polled late learns how it ended), then forgotten.
pub const ASK_KEPT_MS: i64 = 60 * 60_000;
/// The most asks waiting on one person at once.
pub const ASKS_MAX: usize = 8;
/// The most asks one person's Vault keeps, waiting or ended (an ask made
/// again ends the one before it, kept an hour): past it, another waits.
pub const ASKS_KEPT_MAX: usize = 64;
/// An ask's reason, at most, in characters.
pub const WHY_MAX_CHARS: usize = 200;

/// May an agent ask its person for `ask`, the secret under its name held
/// as `held`? Its name a secret's (`name_ok`), its reason one short line,
/// its hosts a secret's (`host_ok`, at most `HOSTS_MAX`). Asking for no
/// value (`keepValue`) asks to send one held to more hosts, so it names
/// one `agent` may use already, the same refusal for one held by others
/// or none (an agent learns nothing of secrets it is not given), and at
/// least one host. What it asks is checked whole again as it is answered
/// (`answered`, then `check`).
pub fn ask_ok(ask: &AskSecret, agent: &str, held: Option<&Secret>, catalog: &Catalog) -> Result<(), VaultError> {
    name_ok(&ask.name, catalog)?;
    let why = ask.why.trim();
    if why.is_empty() || ask.why.chars().count() > WHY_MAX_CHARS || ask.why.chars().any(char::is_control) {
        return Err(VaultError::Ask(format!("say why in one line of 1 to {WHY_MAX_CHARS} characters")));
    }
    if ask.hosts.len() > HOSTS_MAX {
        return Err(VaultError::Hosts(format!("at most {HOSTS_MAX}, not {}", ask.hosts.len())));
    }
    if let Some(bad) = ask.hosts.iter().find(|h| !host_ok(h)) {
        return Err(VaultError::Hosts(format!("{bad:?} is not one: an exact public DNS name, lower case, with no scheme, port, path or wildcard")));
    }
    if ask.keep_value {
        if !held.is_some_and(|h| h.agents.iter().any(|a| a == agent)) {
            return Err(VaultError::Ask(format!("{} is not a secret of yours yet: ask for its value (no --keep-value)", ask.name)));
        }
        if ask.hosts.is_empty() {
            return Err(VaultError::Ask("asking to send a secret somewhere new names a host (--to)".into()));
        }
    }
    Ok(())
}

/// What `agent`'s ask puts in its person's Vault as they answer it with
/// `value` (what they pasted; none for an ask with `keepValue`), the
/// secret under its name held as `held`: its hosts the asked joined to
/// the held, its place the held (none, the default), and its agents the
/// held with `agent`. So an answer never takes a host or an agent from a
/// secret, and the `PutSecret` is held to the Vault's own rules (`check`).
pub fn answered(ask: &AskSecret, agent: &str, held: Option<&Secret>, value: Option<String>) -> Result<PutSecret, VaultError> {
    if ask.keep_value && !held.is_some_and(|h| h.agents.iter().any(|a| a == agent)) {
        return Err(VaultError::Ask(format!("{} is no longer given to this agent: it asks again for its value", ask.name)));
    }
    let value = match (ask.keep_value, value) {
        (true, None) => None,
        (true, Some(_)) => return Err(VaultError::Ask("this ask is Allow or Don't allow: it takes no value".into())),
        (false, Some(v)) if !v.trim().is_empty() => Some(v.trim().to_string()),
        (false, _) => return Err(VaultError::Ask("paste the secret first".into())),
    };
    let hosts = joined(ask, held);
    let mut agents: Vec<String> = held.map(|h| h.agents.clone()).unwrap_or_default();
    agents.push(agent.to_string());
    agents.sort();
    agents.dedup();
    let place = held.map(|h| h.place.clone()).unwrap_or_default();
    Ok(PutSecret { value, hosts, place, agents })
}

/// The hosts a secret goes to once `ask` is answered: the held's, and the
/// asked, sorted.
fn joined(ask: &AskSecret, held: Option<&Secret>) -> Vec<String> {
    let mut hosts: Vec<String> = held.map(|h| h.hosts.clone()).unwrap_or_default();
    hosts.extend(ask.hosts.iter().cloned());
    hosts.sort();
    hosts.dedup();
    hosts
}

/// An ask of `by` (its fragment) on its person's card (`GET
/// /api/notices`): `agent` is what they call it (its title, else its
/// label), `picture` its face, and `held` the secret they keep under its
/// name, if any, which `revision` names (its Save or Allow carries it, so
/// an answer grants what the card said). The field and Save for a value; Allow for one held, sent
/// somewhere new. Its words say everything Save or Allow grants: every
/// host it then works with (the held's too), a replacement, other agents
/// that use it, and that the agent never sees it.
pub fn ask_notice(id: &str, ask: &AskSecret, by: &str, agent: &str, picture: Option<String>, expires_at: i64, (held, revision): (Option<&Secret>, &str)) -> Notice {
    let route = format!("/api/vault/asks/{id}");
    let shown = serde_json::json!({ "held": revision });
    let all = joined(ask, held).join(", ");
    let others = match held.is_some_and(|h| h.agents.iter().any(|a| a != by)) {
        true => " Your other agents that use it can too.",
        false => "",
    };
    let (title, detail, field, actions) = if ask.keep_value {
        (
            format!("{agent} asks to use {} somewhere new", ask.name),
            format!("Sends the {} you keep to {} too: it then works with {all}.{others} {agent} never sees it.", ask.name, ask.hosts.join(", ")),
            None,
            vec![NoticeAction::post("Allow", route.clone(), shown), NoticeAction::post("Don't allow", format!("{route}/decline"), serde_json::json!({}))],
        )
    } else {
        let goes = match all.is_empty() {
            true => "It goes nowhere until you say where.".to_string(),
            false => format!("It only works with {all}."),
        };
        let replaces = match held {
            Some(_) => format!(" It replaces the {} you keep.{others}", ask.name),
            None => String::new(),
        };
        (
            format!("{agent} asks for {}", ask.name),
            format!("{goes}{replaces} {agent} gets a stand-in, never what you paste."),
            Some(NoticeField { placeholder: "Paste it here".into() }),
            vec![NoticeAction::post_field("Save", route.clone(), shown), NoticeAction::post("Decline", format!("{route}/decline"), serde_json::json!({}))],
        )
    };
    Notice {
        id: format!("vault_ask:{id}"),
        kind: "vault_ask".into(),
        level: NoticeLevel::Info,
        title,
        text: Some(ask.why.trim().to_string()),
        detail: Some(detail),
        until: Some(Until { at: expires_at, said: "Expires".into() }),
        actions,
        must_answer: false,
        image: picture,
        field,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Catalog {
        Catalog::parse(r#"[{"name": "openrouter", "kind": "own", "hosts": ["openrouter.ai"], "placements": [{"header": "authorization", "format": "Bearer {}"}], "env": ["OPENROUTER_API_KEY"]}]"#).expect("a catalog")
    }

    fn put(value: &str, hosts: &[&str], place: Place, agents: &[&str]) -> PutSecret {
        PutSecret { value: Some(value.into()), hosts: hosts.iter().map(|h| h.to_string()).collect(), place, agents: agents.iter().map(|a| a.to_string()).collect() }
    }

    const AGENT: &str = "ada--k3x9";
    const NSEC: &str = "abababababababababababababababababababababababababababababababab";

    #[test]
    fn a_token_toward_a_host_by_default_goes_as_a_bearer_and_is_stored_so() {
        let c = check("linear", &put("lin_api_abc", &["api.linear.app", "api.linear.app"], Place::default(), &[AGENT, AGENT]), None, &catalog()).expect("valid");
        assert_eq!(c.hosts, ["api.linear.app"]);
        assert_eq!(c.agents, [AGENT]);
        assert_eq!(c.placing, Placing::Fill(Placement::Header { name: "authorization".into(), format: "Bearer {}".into() }));
        assert_eq!(c.place, Place { header: Some("authorization".into()), format: Some("Bearer {}".into()), ..Place::default() });
    }

    #[test]
    fn each_place_reads_as_the_catalog_s_and_a_signature_as_its_header() {
        let q = check("k", &put("v", &["a.example.com"], Place { query: Some("key".into()), ..Place::default() }, &[AGENT]), None, &catalog()).expect("query");
        assert_eq!(q.placing, Placing::Fill(Placement::Query { name: "key".into() }));
        let b = check("k", &put("v", &["a.example.com"], Place { basic: Some("password".into()), ..Place::default() }, &[AGENT]), None, &catalog()).expect("basic");
        assert_eq!(b.placing, Placing::Fill(Placement::Basic { half: Half::Password }));
        let h = check("k", &put("v", &["a.example.com"], Place { header: Some("x-api-key".into()), ..Place::default() }, &[AGENT]), None, &catalog()).expect("header");
        assert_eq!(h.placing, Placing::Fill(Placement::Header { name: "x-api-key".into(), format: "{}".into() }));
        let s = check("nostr-ada", &put(NSEC, &["finite.site"], Place { sign: Some("nip98".into()), ..Place::default() }, &[AGENT]), None, &catalog()).expect("sign");
        assert_eq!(s.placing, Placing::Nip98);
        assert_eq!(s.placing.placement(), Placement::Header { name: "authorization".into(), format: "Nostr {}".into() });
    }

    #[test]
    fn a_secret_with_no_hosts_is_parked_and_valid() {
        let c = check("telegram-ada", &put("123:abc", &[], Place::default(), &[AGENT]), None, &catalog()).expect("parked");
        assert!(c.hosts.is_empty());
    }

    #[test]
    fn names_a_key_uses_or_of_another_form_are_refused() {
        for name in ["openrouter", "", "Linear", "-x", "x-", "a_b", &"x".repeat(33)] {
            assert!(matches!(check(name, &put("v", &["a.example.com"], Place::default(), &[AGENT]), None, &catalog()), Err(VaultError::Name(_))), "{name:?}");
        }
    }

    /// A connection's name may name a secret (its placeholder is `fcx_`,
    /// the secret's `fck_`): one a person named before the deployment
    /// offered that connection is set again as it was; a key's name never.
    #[test]
    fn a_connections_name_may_name_a_secret_and_a_keys_never() {
        let c = Catalog::parse(r#"[{"name": "openrouter", "kind": "own", "hosts": ["openrouter.ai"], "placements": [{"header": "authorization", "format": "Bearer {}"}], "env": ["OPENROUTER_API_KEY"]},
            {"name": "linear", "kind": "connection", "hosts": ["api.linear.app"], "placements": [{"header": "authorization", "format": "Bearer {}"}], "env": ["LINEAR_OAUTH_ACCESS_TOKEN"]},
            {"name": "perplexity", "kind": "operator", "hosts": ["api.perplexity.ai"], "placements": [{"header": "authorization", "format": "Bearer {}"}], "env": ["PERPLEXITY_API_KEY"]}]"#).expect("a catalog");
        assert!(check("linear", &put("lin_api_abc", &["api.linear.app"], Place::default(), &[AGENT]), None, &c).is_ok());
        for key in ["openrouter", "perplexity"] {
            let refused = check(key, &put("v", &["a.example.com"], Place::default(), &[AGENT]), None, &c);
            assert!(matches!(&refused, Err(VaultError::Name(why)) if why.contains("is a key this deployment offers")), "{key}: {refused:?}");
        }
    }

    #[test]
    fn values_are_printable_tokens_or_a_nostr_key_as_hex() {
        for v in ["", "has space", "tab\tin", "é", &"x".repeat(VALUE_MAX_BYTES + 1)] {
            assert!(matches!(check("k", &put(v, &["a.example.com"], Place::default(), &[AGENT]), None, &catalog()), Err(VaultError::Value(_))), "{v:?}");
        }
        let sign = Place { sign: Some("nip98".into()), ..Place::default() };
        // not hex, upper case, short
        for v in ["nsec1abc", &NSEC.to_uppercase(), &NSEC[..62]] {
            assert!(matches!(check("k", &put(v, &["a.example.com"], sign.clone(), &[AGENT]), None, &catalog()), Err(VaultError::Value(_))), "{v:?}");
        }
    }

    #[test]
    fn hosts_are_exact_public_names_and_at_most_eight() {
        for h in ["1.2.3.4", "[::1]", "localhost", "api.fragment.internal", "fragment.internal", "*.example.com", "Example.com", "example.com:443", "https://example.com", "example", "example.com."] {
            assert!(matches!(check("k", &put("v", &[h], Place::default(), &[AGENT]), None, &catalog()), Err(VaultError::Hosts(_))), "{h:?}");
        }
        let nine: Vec<String> = (0..9).map(|i| format!("h{i}.example.com")).collect();
        let nine: Vec<&str> = nine.iter().map(String::as_str).collect();
        assert!(matches!(check("k", &put("v", &nine, Place::default(), &[AGENT]), None, &catalog()), Err(VaultError::Hosts(_))));
    }

    #[test]
    fn places_are_one_of_the_four_and_held_to_the_catalog_s_rules() {
        let two = Place { header: Some("authorization".into()), query: Some("key".into()), ..Place::default() };
        let format_alone = Place { format: Some("Bearer {}".into()), ..Place::default() };
        let hop = Place { header: Some("host".into()), ..Place::default() };
        let ours = Place { header: Some("x-fragment-agent".into()), ..Place::default() };
        let no_slot = Place { header: Some("authorization".into()), format: Some("Bearer".into()), ..Place::default() };
        let sign_and_header = Place { header: Some("authorization".into()), sign: Some("nip98".into()), ..Place::default() };
        let other_half = Place { basic: Some("both".into()), ..Place::default() };
        let other_sign = Place { sign: Some("nip46".into()), ..Place::default() };
        for p in [two, format_alone, hop, ours, no_slot, sign_and_header, other_half, other_sign] {
            assert!(matches!(check("k", &put(NSEC, &["a.example.com"], p.clone(), &[AGENT]), None, &catalog()), Err(VaultError::Place(_))), "{p:?}");
        }
    }

    #[test]
    fn a_secret_names_one_to_sixteen_agents_by_fragment_name() {
        assert!(matches!(check("k", &put("v", &["a.example.com"], Place::default(), &[]), None, &catalog()), Err(VaultError::Agents(_))));
        assert!(matches!(check("k", &put("v", &["a.example.com"], Place::default(), &["ada"]), None, &catalog()), Err(VaultError::Agents(_))));
        let many: Vec<String> = (0..17).map(|i| format!("a{i}--k3x9")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert!(matches!(check("k", &put("v", &["a.example.com"], Place::default(), &many), None, &catalog()), Err(VaultError::Agents(_))));
    }

    fn keep(hosts: &[&str], place: Place, agents: &[&str]) -> PutSecret {
        PutSecret { value: None, ..put("unused", hosts, place, agents) }
    }

    fn sign() -> Place {
        Place { sign: Some("nip98".into()), ..Place::default() }
    }

    /// A Nostr key handed off parked (FIN-463 F11) is pointed at its host
    /// by its person with no value: kept, and still a signer.
    #[test]
    fn a_parked_key_set_again_with_no_value_keeps_it_toward_its_new_hosts() {
        let parked = check("nostr-ada--k3x9", &put(NSEC, &[], sign(), &[AGENT]), None, &catalog()).expect("parked");
        let c = check("nostr-ada--k3x9", &keep(&["finite.site"], sign(), &[AGENT]), Some(&parked.place), &catalog()).expect("kept");
        assert_eq!((c.hosts, c.placing), (vec!["finite.site".to_string()], Placing::Nip98));
        // a fill's value kept from one fill place to another
        let bearer = check("linear", &put("lin_api_abc", &["api.linear.app"], Place::default(), &[AGENT]), None, &catalog()).expect("bearer");
        let q = check("linear", &keep(&["api.linear.app"], Place { query: Some("key".into()), ..Place::default() }, &[AGENT, "bo--k3x9"]), Some(&bearer.place), &catalog()).expect("query");
        assert_eq!((q.placing, q.agents), (Placing::Fill(Placement::Query { name: "key".into() }), vec![AGENT.to_string(), "bo--k3x9".to_string()]));
    }

    #[test]
    fn no_value_for_a_new_secret_or_across_sign_and_fill_is_refused() {
        let new = check("nostr-ada", &keep(&["finite.site"], sign(), &[AGENT]), None, &catalog());
        assert!(matches!(&new, Err(VaultError::Value(why)) if why.contains("a new one needs its value")), "{new:?}");
        let signer = sign();
        let to_fill = check("nostr-ada", &keep(&["finite.site"], Place::default(), &[AGENT]), Some(&signer), &catalog());
        assert!(matches!(&to_fill, Err(VaultError::Value(why)) if why.contains("Nostr key that signs")), "{to_fill:?}");
        let bearer = Place { header: Some("authorization".into()), format: Some("Bearer {}".into()), ..Place::default() };
        let to_sign = check("linear", &keep(&["api.linear.app"], sign(), &[AGENT]), Some(&bearer), &catalog());
        assert!(matches!(&to_sign, Err(VaultError::Value(why)) if why.contains("set it again with a Nostr key")), "{to_sign:?}");
        // a stored place that reads as none keeps nothing
        let two = Place { header: Some("authorization".into()), query: Some("key".into()), ..Place::default() };
        assert!(matches!(check("linear", &keep(&["api.linear.app"], Place::default(), &[AGENT]), Some(&two), &catalog()), Err(VaultError::Value(_))));
    }

    /// With no value, everything else is checked as with one.
    #[test]
    fn no_value_is_no_pass_on_the_name_hosts_place_or_agents() {
        let signer = sign();
        let held = Some(&signer);
        assert!(matches!(check("openrouter", &keep(&["finite.site"], sign(), &[AGENT]), held, &catalog()), Err(VaultError::Name(_))));
        assert!(matches!(check("k", &keep(&["1.2.3.4"], sign(), &[AGENT]), held, &catalog()), Err(VaultError::Hosts(_))));
        assert!(matches!(check("k", &keep(&["finite.site"], Place { sign: Some("nip46".into()), ..Place::default() }, &[AGENT]), held, &catalog()), Err(VaultError::Place(_))));
        assert!(matches!(check("k", &keep(&["finite.site"], sign(), &[]), held, &catalog()), Err(VaultError::Agents(_))));
        assert!(matches!(check("k", &keep(&["finite.site"], sign(), &["ada"]), held, &catalog()), Err(VaultError::Agents(_))));
    }

    /// An older client's put, a value and all, reads as it did, and one
    /// with no value writes no `value` field at all.
    #[test]
    fn a_put_with_a_value_reads_as_before_and_one_without_names_none() {
        let old: PutSecret = serde_json::from_str(r#"{"value":"lin_api_abc","hosts":["api.linear.app"],"agents":["ada--k3x9"]}"#).expect("an older client's put");
        assert_eq!(old, put("lin_api_abc", &["api.linear.app"], Place::default(), &[AGENT]));
        let none: PutSecret = serde_json::from_str(r#"{"hosts":["finite.site"],"place":{"sign":"nip98"},"agents":["ada--k3x9"]}"#).expect("a put with no value");
        assert_eq!(none.value, None);
        assert!(!serde_json::to_string(&none).expect("json").contains("value"));
    }

    #[test]
    fn a_name_that_could_choose_another_route_is_no_name() {
        for bad in ["../delete/nostr", "%2e%2e/delete/nostr", "%2E%2E%2Fdelete%2Fnostr", "..%2fdelete%2fnostr", "nostr/x", "nostr/../delete", "..", ".", "a.b", "a%2fb", "a\\b", "a?b", "a#b", ""] {
            assert!(name_form(bad).is_err(), "{bad:?}");
        }
        assert_eq!(name_form("telegram-bot"), Ok(()));
    }

    /// One Vault row, as the cell's `secrets` table keeps it: the value's
    /// revision and its claim. Its state is the row, so a restart is
    /// nothing: every step below reads and writes only it.
    #[derive(Default)]
    struct Row(Option<(String, Option<String>)>);

    impl Row {
        fn put(&mut self, revision: &str) {
            self.0 = Some((revision.to_string(), None));
        }
        /// `vault/take`: its revision, the claim written in the same turn.
        fn take(&mut self, to: &str) -> Result<String, String> {
            let (revision, claim) = self.0.as_mut().ok_or("404")?;
            super::take("bot", claim.as_deref(), to).map_err(|e| e.to_string())?;
            *claim = Some(to.to_string());
            Ok(revision.clone())
        }
        /// `vault/moved`: removed?
        fn moved(&mut self, to: &str, taken: &str) -> bool {
            let done = self.0.as_ref().is_some_and(|(held, claim)| completes(claim.as_deref(), held, to, taken));
            if done {
                self.0 = None;
            }
            done
        }
    }

    #[test]
    fn two_moves_that_both_read_before_either_keeps_it_give_it_one_home() {
        let mut row = Row::default();
        row.put("r1");
        let a = row.take("one/BOT").expect("the first take");
        let b = row.take("two/BOT");
        assert_eq!(b, Err("bot is being moved into one/BOT: move it there again to finish, or set it again with a value in your vault".to_string()));
        // one keeps it, then completes: gone from the Vault, kept once
        assert!(row.moved("one/BOT", &a));
        assert!(row.0.is_none());
        // the refused move, again: nothing is there
        assert_eq!(row.take("two/BOT"), Err("404".to_string()));
    }

    #[test]
    fn a_move_cut_short_is_finished_by_the_same_move_again_and_no_other() {
        let mut row = Row::default();
        row.put("r1");
        // taken, then the destination stops before it completes (a crash,
        // a restart, a failed answer): the value stays, claimed
        row.take("one/BOT").expect("the first take");
        assert!(row.take("two/BOT").is_err(), "no clock frees it for another");
        assert!(!row.moved("two/BOT", "r1"), "nor completes another's move");
        let again = row.take("one/BOT").expect("the same move again");
        assert_eq!(again, "r1");
        assert!(row.moved("one/BOT", &again));
        // a replayed completion removes nothing more
        assert!(!row.moved("one/BOT", &again));
    }

    #[test]
    fn the_same_move_twice_at_once_completes_once() {
        let mut row = Row::default();
        row.put("r1");
        let first = row.take("one/BOT").expect("one");
        let second = row.take("one/BOT").expect("the same destination");
        assert!(row.moved("one/BOT", &first));
        assert!(!row.moved("one/BOT", &second));
    }

    #[test]
    fn setting_it_again_ends_a_claim_and_keeps_the_new_value() {
        let mut row = Row::default();
        row.put("r1");
        let taken = row.take("one/BOT").expect("taken");
        // its person sets it again before the move completes: the old
        // value kept there, the new one here and unclaimed
        row.put("r2");
        assert!(!row.moved("one/BOT", &taken));
        assert_eq!(row.take("two/BOT"), Ok("r2".to_string()));
        // the first move's completion, late, removes nothing of the new
        // value, now claimed by another move
        assert!(!row.moved("one/BOT", &taken));
    }

    fn ask(name: &str, why: &str, hosts: &[&str], keep_value: bool) -> AskSecret {
        AskSecret { name: name.into(), why: why.into(), hosts: hosts.iter().map(|h| h.to_string()).collect(), keep_value }
    }

    fn held(hosts: &[&str], place: Place, agents: &[&str]) -> Secret {
        Secret { name: "github".into(), hosts: hosts.iter().map(|h| h.to_string()).collect(), place, agents: agents.iter().map(|a| a.to_string()).collect(), set_at: 1 }
    }

    /// Goal: an agent asks for a secret by a Vault name, in one short line,
    /// toward a secret's hosts. Method: each part valid, then each wrong.
    #[test]
    fn an_ask_names_a_secret_why_in_a_line_and_a_secrets_hosts() {
        assert!(ask_ok(&ask("github", "To push your shop's code", &["api.github.com"], false), AGENT, None, &catalog()).is_ok());
        assert!(ask_ok(&ask("github", "Kept for later", &[], false), AGENT, None, &catalog()).is_ok(), "no hosts: parked, as a put may be");
        for (bad, what) in [
            (ask("openrouter", "a key the deployment offers", &[], false), "name"),
            (ask("GitHub", "x", &[], false), "name"),
            (ask("github", "", &[], false), "why"),
            (ask("github", "   ", &[], false), "why"),
            (ask("github", &"x".repeat(WHY_MAX_CHARS + 1), &[], false), "why"),
            (ask("github", "two\nlines", &[], false), "why"),
            (ask("github", "x", &["https://api.github.com"], false), "hosts"),
            (ask("github", "x", &["*.github.com"], false), "hosts"),
            (ask("github", "x", &["api.fragment.internal"], false), "hosts"),
            (ask("github", "x", &["a.example.com"; 9], false), "hosts"),
        ] {
            assert!(ask_ok(&bad, AGENT, None, &catalog()).is_err(), "{what}: {bad:?}");
        }
        assert!(ask_ok(&ask("github", &"é".repeat(WHY_MAX_CHARS), &[], false), AGENT, None, &catalog()).is_ok(), "counted in characters, not bytes");
    }

    /// Goal: asking to send a secret somewhere new asks about one held,
    /// toward a host. Method: none held, and no host, each refused.
    #[test]
    fn an_ask_with_no_value_names_one_held_and_a_host() {
        let h = held(&["api.github.com"], Place::default(), &[AGENT]);
        assert!(ask_ok(&ask("github", "Upload a release", &["uploads.github.com"], true), AGENT, Some(&h), &catalog()).is_ok());
        assert!(matches!(ask_ok(&ask("github", "Upload", &["uploads.github.com"], true), AGENT, None, &catalog()), Err(VaultError::Ask(_))));
        assert!(matches!(ask_ok(&ask("github", "Upload", &[], true), AGENT, Some(&h), &catalog()), Err(VaultError::Ask(_))));
        // one its person gave other agents alone: refused as one not held
        let theirs = held(&["api.github.com"], Place::default(), &["bea--k3x9"]);
        let refused = |h| ask_ok(&ask("github", "Upload", &["uploads.github.com"], true), AGENT, h, &catalog()).map_err(|e| e.to_string());
        assert!(refused(Some(&theirs)).is_err());
        assert_eq!(refused(Some(&theirs)), refused(None), "an agent learns nothing of a secret it is not given");
    }

    /// Goal: an answer adds, never takes away: the hosts and agents held
    /// stay, the asker and the asked hosts join, the place held is kept,
    /// and the result passes the Vault's own rules. Method: a new secret,
    /// one held by another agent replaced, and one held sent further.
    #[test]
    fn an_answer_joins_what_is_held_and_takes_nothing() {
        let new = answered(&ask("github", "push", &["api.github.com"], false), AGENT, None, Some(" ghp_abc \n".into())).expect("answered");
        assert_eq!(new, PutSecret { value: Some("ghp_abc".into()), hosts: vec!["api.github.com".into()], place: Place::default(), agents: vec![AGENT.into()] }, "trimmed as a paste ends");
        assert!(check("github", &new, None, &catalog()).is_ok());

        let h = held(&["api.github.com"], Place { header: Some("x-token".into()), format: Some("{}".into()), ..Place::default() }, &["bea--k3x9"]);
        let replaced = answered(&ask("github", "push", &["uploads.github.com"], false), AGENT, Some(&h), Some("ghp_new".into())).expect("answered");
        assert_eq!(replaced.hosts, ["api.github.com", "uploads.github.com"]);
        assert_eq!(replaced.agents, [AGENT, "bea--k3x9"]);
        assert_eq!(replaced.place, h.place, "where it goes stays the person's");

        let mine = held(&["api.github.com"], h.place.clone(), &[AGENT, "bea--k3x9"]);
        let allowed = answered(&ask("github", "upload", &["uploads.github.com"], true), AGENT, Some(&mine), None).expect("allowed");
        assert_eq!(allowed.value, None, "Allow keeps the value held");
        assert_eq!(allowed.hosts, ["api.github.com", "uploads.github.com"]);
        assert!(check("github", &allowed, Some(&h.place), &catalog()).is_ok());
    }

    /// Goal: a value ask needs a paste, and an Allow takes none. Method:
    /// an empty or blank paste, and a value sent to an Allow.
    #[test]
    fn a_value_ask_needs_a_paste_and_an_allow_takes_none() {
        for blank in [None, Some(String::new()), Some("  \n".into())] {
            assert!(matches!(answered(&ask("github", "push", &[], false), AGENT, None, blank.clone()), Err(VaultError::Ask(_))), "{blank:?}");
        }
        let h = held(&["api.github.com"], Place::default(), &[AGENT]);
        assert!(matches!(answered(&ask("github", "upload", &["uploads.github.com"], true), AGENT, Some(&h), Some("v".into())), Err(VaultError::Ask(_))));
        // an Allow never gives back what its person took away since it asked
        let taken = held(&["api.github.com"], Place::default(), &["bea--k3x9"]);
        assert!(matches!(answered(&ask("github", "upload", &["uploads.github.com"], true), AGENT, Some(&taken), None), Err(VaultError::Ask(_))));
    }

    /// Goal: the card says who asks, why, where it goes, and that the
    /// agent never sees it; a value ask carries the field its Save sends,
    /// an Allow none. Method: both kinds, on the wire.
    #[test]
    fn the_card_says_who_why_where_and_carries_the_field() {
        let id = "0123456789abcdef0123456789abcdef";
        let n = ask_notice(id, &ask("github", "To push your shop's code", &["api.github.com"], false), AGENT, "Juniper", Some("/p".into()), 9, (None, "none"));
        assert_eq!(n.title, "Juniper asks for github");
        assert_eq!(n.text.as_deref(), Some("To push your shop's code"));
        assert_eq!(n.detail.as_deref(), Some("It only works with api.github.com. Juniper gets a stand-in, never what you paste."));
        assert_eq!(n.until, Some(Until { at: 9, said: "Expires".into() }));
        assert_eq!(n.actions[0].body, Some(serde_json::json!({ "held": "none" })), "Save carries what the card showed held");
        assert!(n.field.is_some() && n.actions[0].field && n.actions[0].post.as_deref() == Some("/api/vault/asks/0123456789abcdef0123456789abcdef"));
        assert!(!n.actions[1].field && n.actions[1].post.as_deref() == Some("/api/vault/asks/0123456789abcdef0123456789abcdef/decline"));
        let parked = held(&[], Place::default(), &[AGENT]);
        let again = ask_notice(id, &ask("github", "Rotate", &[], false), AGENT, "Juniper", None, 9, (Some(&parked), "r1"));
        assert_eq!(again.detail.as_deref(), Some("It goes nowhere until you say where. It replaces the github you keep. Juniper gets a stand-in, never what you paste."));
        // a replacement says every host it keeps, and the other agents that use it
        let shared = held(&["api.github.com"], Place::default(), &[AGENT, "bea--k3x9"]);
        let replace = ask_notice(id, &ask("github", "Rotate", &[], false), AGENT, "Juniper", None, 9, (Some(&shared), "r2"));
        assert_eq!(replace.detail.as_deref(), Some("It only works with api.github.com. It replaces the github you keep. Your other agents that use it can too. Juniper gets a stand-in, never what you paste."));
        let allow = ask_notice(id, &ask("github", "Upload", &["uploads.github.com"], true), AGENT, "Juniper", None, 9, (Some(&shared), "r2"));
        assert_eq!(allow.title, "Juniper asks to use github somewhere new");
        assert_eq!(allow.detail.as_deref(), Some("Sends the github you keep to uploads.github.com too: it then works with api.github.com, uploads.github.com. Your other agents that use it can too. Juniper never sees it."));
        assert!(allow.field.is_none() && allow.actions.iter().all(|a| !a.field) && allow.actions[0].label == "Allow");
    }
}
