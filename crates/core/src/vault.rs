//! A person's Vault (FIN-466; docs/secrets.md, "A person's Vault"): the
//! rules a secret is held to as it is put. The cell's `Vault` object
//! seals what passes and answers its agents' swaps; this decides, purely,
//! what a secret may be.
//!
//! A secret is a value, the hosts it may go to, where it goes on a
//! request (`Placing`), and the agents that may use it. Only its person
//! sets one, so only they say where its value goes: with a value, or
//! keeping the one it holds where that value means the same (`kept`).

use fragment_proto::vault::{Place, PutSecret};

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
}

impl std::fmt::Display for VaultError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VaultError::Name(why) => write!(f, "a secret's name: {why}"),
            VaultError::Value(why) => write!(f, "a secret's value: {why}"),
            VaultError::Hosts(why) => write!(f, "a secret's hosts: {why}"),
            VaultError::Place(why) => write!(f, "where a secret goes: {why}"),
            VaultError::Agents(why) => write!(f, "a secret's agents: {why}"),
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
}
