//! A person's Vault (FIN-466; docs/secrets.md, "A person's Vault"): the
//! rules a secret is held to as it is put. The cell's `Vault` object
//! seals what passes and answers its agents' swaps; this decides, purely,
//! what a secret may be.
//!
//! A secret is a value, the hosts it may go to, where it goes on a
//! request (`Placing`), and the agents that may use it. Its hosts, place
//! and agents are set only with its value, so no one who cannot read the
//! value (no one can) sends it anywhere its setter did not mean.

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

/// Whether `name` may name a secret: a catalog name's form, and none the
/// deployment's catalog uses (a placeholder names one or the other).
pub fn name_ok(name: &str, catalog: &Catalog) -> Result<(), VaultError> {
    if !catalog::valid_name(name) {
        return Err(VaultError::Name(format!("{name:?} is not one: 1 to 32 of a-z, 0-9 and -, not starting or ending with -")));
    }
    if catalog.get(name).is_some() {
        return Err(VaultError::Name(format!("{name:?} is a provider this deployment offers: name it otherwise")));
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

/// `put`, under `name`, checked against the rules above.
pub fn check(name: &str, put: &PutSecret, catalog: &Catalog) -> Result<Checked, VaultError> {
    name_ok(name, catalog)?;
    let (placing, place) = placing(&put.place)?;
    match placing {
        Placing::Fill(_) => {
            if !(1..=VALUE_MAX_BYTES).contains(&put.value.len()) || !put.value.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
                return Err(VaultError::Value(format!("1 to {VALUE_MAX_BYTES} printable characters, with no space")));
            }
        }
        Placing::Nip98 => {
            // a key on the curve too: the cell's to check, where its signer is
            if put.value.len() != 64 || !put.value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Catalog {
        Catalog::parse(r#"[{"name": "openrouter", "kind": "own", "hosts": ["openrouter.ai"], "placements": [{"header": "authorization", "format": "Bearer {}"}], "env": ["OPENROUTER_API_KEY"]}]"#).expect("a catalog")
    }

    fn put(value: &str, hosts: &[&str], place: Place, agents: &[&str]) -> PutSecret {
        PutSecret { value: value.into(), hosts: hosts.iter().map(|h| h.to_string()).collect(), place, agents: agents.iter().map(|a| a.to_string()).collect() }
    }

    const AGENT: &str = "ada--k3x9";
    const NSEC: &str = "abababababababababababababababababababababababababababababababab";

    #[test]
    fn a_token_toward_a_host_by_default_goes_as_a_bearer_and_is_stored_so() {
        let c = check("linear", &put("lin_api_abc", &["api.linear.app", "api.linear.app"], Place::default(), &[AGENT, AGENT]), &catalog()).expect("valid");
        assert_eq!(c.hosts, ["api.linear.app"]);
        assert_eq!(c.agents, [AGENT]);
        assert_eq!(c.placing, Placing::Fill(Placement::Header { name: "authorization".into(), format: "Bearer {}".into() }));
        assert_eq!(c.place, Place { header: Some("authorization".into()), format: Some("Bearer {}".into()), ..Place::default() });
    }

    #[test]
    fn each_place_reads_as_the_catalog_s_and_a_signature_as_its_header() {
        let q = check("k", &put("v", &["a.example.com"], Place { query: Some("key".into()), ..Place::default() }, &[AGENT]), &catalog()).expect("query");
        assert_eq!(q.placing, Placing::Fill(Placement::Query { name: "key".into() }));
        let b = check("k", &put("v", &["a.example.com"], Place { basic: Some("password".into()), ..Place::default() }, &[AGENT]), &catalog()).expect("basic");
        assert_eq!(b.placing, Placing::Fill(Placement::Basic { half: Half::Password }));
        let h = check("k", &put("v", &["a.example.com"], Place { header: Some("x-api-key".into()), ..Place::default() }, &[AGENT]), &catalog()).expect("header");
        assert_eq!(h.placing, Placing::Fill(Placement::Header { name: "x-api-key".into(), format: "{}".into() }));
        let s = check("nostr-ada", &put(NSEC, &["finite.site"], Place { sign: Some("nip98".into()), ..Place::default() }, &[AGENT]), &catalog()).expect("sign");
        assert_eq!(s.placing, Placing::Nip98);
        assert_eq!(s.placing.placement(), Placement::Header { name: "authorization".into(), format: "Nostr {}".into() });
    }

    #[test]
    fn a_secret_with_no_hosts_is_parked_and_valid() {
        let c = check("telegram-ada", &put("123:abc", &[], Place::default(), &[AGENT]), &catalog()).expect("parked");
        assert!(c.hosts.is_empty());
    }

    #[test]
    fn names_a_provider_uses_or_of_another_form_are_refused() {
        for name in ["openrouter", "", "Linear", "-x", "x-", "a_b", &"x".repeat(33)] {
            assert!(matches!(check(name, &put("v", &["a.example.com"], Place::default(), &[AGENT]), &catalog()), Err(VaultError::Name(_))), "{name:?}");
        }
    }

    #[test]
    fn values_are_printable_tokens_or_a_nostr_key_as_hex() {
        for v in ["", "has space", "tab\tin", "é", &"x".repeat(VALUE_MAX_BYTES + 1)] {
            assert!(matches!(check("k", &put(v, &["a.example.com"], Place::default(), &[AGENT]), &catalog()), Err(VaultError::Value(_))), "{v:?}");
        }
        let sign = Place { sign: Some("nip98".into()), ..Place::default() };
        // not hex, upper case, short
        for v in ["nsec1abc", &NSEC.to_uppercase(), &NSEC[..62]] {
            assert!(matches!(check("k", &put(v, &["a.example.com"], sign.clone(), &[AGENT]), &catalog()), Err(VaultError::Value(_))), "{v:?}");
        }
    }

    #[test]
    fn hosts_are_exact_public_names_and_at_most_eight() {
        for h in ["1.2.3.4", "[::1]", "localhost", "api.fragment.internal", "fragment.internal", "*.example.com", "Example.com", "example.com:443", "https://example.com", "example", "example.com."] {
            assert!(matches!(check("k", &put("v", &[h], Place::default(), &[AGENT]), &catalog()), Err(VaultError::Hosts(_))), "{h:?}");
        }
        let nine: Vec<String> = (0..9).map(|i| format!("h{i}.example.com")).collect();
        let nine: Vec<&str> = nine.iter().map(String::as_str).collect();
        assert!(matches!(check("k", &put("v", &nine, Place::default(), &[AGENT]), &catalog()), Err(VaultError::Hosts(_))));
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
            assert!(matches!(check("k", &put(NSEC, &["a.example.com"], p.clone(), &[AGENT]), &catalog()), Err(VaultError::Place(_))), "{p:?}");
        }
    }

    #[test]
    fn a_secret_names_one_to_sixteen_agents_by_fragment_name() {
        assert!(matches!(check("k", &put("v", &["a.example.com"], Place::default(), &[]), &catalog()), Err(VaultError::Agents(_))));
        assert!(matches!(check("k", &put("v", &["a.example.com"], Place::default(), &["ada"]), &catalog()), Err(VaultError::Agents(_))));
        let many: Vec<String> = (0..17).map(|i| format!("a{i}--k3x9")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert!(matches!(check("k", &put("v", &["a.example.com"], Place::default(), &many), &catalog()), Err(VaultError::Agents(_))));
    }
}
