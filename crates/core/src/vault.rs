//! A person's Vault (FIN-466; docs/secrets.md, "A person's Vault"): the
//! rules a secret is held to as it is put. The cell's `Vault` object
//! seals what passes and answers its agents' swaps; this decides, purely,
//! what a secret may be.
//!
//! A secret is a value, the hosts it may go to, where it goes on a
//! request (`Placing`), and the agents and apps that may use it (an app
//! under the name its code fills, `AppUse`). Only its person sets one, so
//! only they say where its value goes: with a value, or keeping the one it
//! holds where that value means the same (`kept`).

use fragment_proto::vault::{AppUse, Place, PutSecret};

use crate::catalog::{self, Catalog, Half, Placement};

/// The most secrets one person keeps.
pub const SECRETS_MAX: usize = 64;
/// The most hosts one secret may go to.
pub const HOSTS_MAX: usize = fragment_proto::limits::SECRET_HOSTS_MAX;
/// The most agents one secret is bound to: a computer's agents, at most.
pub const AGENTS_MAX: usize = 16;
/// The most apps one secret is bound to.
pub const APPS_MAX: usize = 16;
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
        charges: vec![],
        calls_an_hour: None,
        oauth: None,
        connection_oauth: None,
        accounts_max: None,
        account_host: None,
        client_auth: catalog::ClientAuth::Post,
        models: None,
        note: None,
        title: None,
        scope_names: Default::default(),
        deny: vec![],
    }
}

/// A secret that passed, in the form the Vault stores: its hosts, agents
/// and apps sorted and each once, its place as given (its default spelled
/// out).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    pub hosts: Vec<String>,
    pub place: Place,
    pub placing: Placing,
    pub agents: Vec<String>,
    pub apps: Vec<AppUse>,
}

/// Why a secret is refused, typed; each says what to do instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultError {
    Name(String),
    Value(String),
    Hosts(String),
    Place(String),
    Agents(String),
    Apps(String),
}

impl std::fmt::Display for VaultError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VaultError::Name(why) => write!(f, "a secret's name: {why}"),
            VaultError::Value(why) => write!(f, "a secret's value: {why}"),
            VaultError::Hosts(why) => write!(f, "a secret's hosts: {why}"),
            VaultError::Place(why) => write!(f, "where a secret goes: {why}"),
            VaultError::Agents(why) => write!(f, "a secret's agents: {why}"),
            VaultError::Apps(why) => write!(f, "a secret's apps: {why}"),
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
/// which the computer's egress answers itself. On a fleet that lets jobs
/// reach its own network (`local`: dev and the e2e, `FRAGMENT_EGRESS_LOCAL`),
/// also an address or name there, its fakes', as an app's own secret may
/// name one (`secrets::may_fill`); never an `.internal` one.
pub fn host_ok(host: &str, local: bool) -> bool {
    let Ok(h) = crate::secrets::secret_host(host) else { return false };
    if h != host || h.ends_with(".internal") {
        return false;
    }
    if local && url::Url::parse(&format!("http://{h}/")).is_ok_and(|u| crate::egress::local(&u)) {
        return true;
    }
    let tld_numeric = h.rsplit('.').next().is_some_and(|tld| tld.bytes().all(|b| b.is_ascii_digit()));
    catalog::valid_host(&h) && !tld_numeric && h != "localhost"
}

/// `put`, under `name`, checked against the rules above; `held`, the
/// place of the secret already kept under `name`, if one is; `local`,
/// whether its hosts may be its fleet's own (`host_ok`). A put with no
/// value keeps the one held (`kept`).
pub fn check(name: &str, put: &PutSecret, held: Option<&Place>, catalog: &Catalog, local: bool) -> Result<Checked, VaultError> {
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
    if let Some(bad) = put.hosts.iter().find(|h| !host_ok(h, local)) {
        return Err(VaultError::Hosts(format!("{bad:?} is not one: an exact public DNS name, lower case, with no scheme, port, path or wildcard")));
    }
    let mut hosts = put.hosts.clone();
    hosts.sort();
    hosts.dedup();
    if put.agents.len() > AGENTS_MAX || (put.agents.is_empty() && put.apps.is_empty()) {
        return Err(VaultError::Agents(format!("name 1 to {AGENTS_MAX} of your agents, each by its fragment's name, or an app of yours (apps)")));
    }
    if let Some(bad) = put.agents.iter().find(|a| !fragment_proto::valid_fragment_name(a)) {
        return Err(VaultError::Agents(format!("{bad:?} is not an agent's fragment name (label--suffix)")));
    }
    let mut agents = put.agents.clone();
    agents.sort();
    agents.dedup();
    Ok(Checked { hosts, place, apps: apps(&put.apps, &placing)?, placing, agents })
}

/// The apps a put binds, checked: at most `APPS_MAX`, each an app's
/// fragment name once, under a name of a secret's in its code; sorted. A
/// key that signs is bound to none: an app's code places a value itself,
/// and a signing key's never leaves the Vault.
fn apps(given: &[AppUse], placing: &Placing) -> Result<Vec<AppUse>, VaultError> {
    if given.len() > APPS_MAX {
        return Err(VaultError::Apps(format!("at most {APPS_MAX}, not {}", given.len())));
    }
    if !given.is_empty() && *placing == Placing::Nip98 {
        return Err(VaultError::Apps("a Nostr key that signs is its agents' alone: an app's code places a value, and this one never leaves your vault".into()));
    }
    if let Some(bad) = given.iter().find(|a| !fragment_proto::valid_fragment_name(&a.app)) {
        return Err(VaultError::Apps(format!("{:?} is not an app's fragment name (label--suffix)", bad.app)));
    }
    if let Some(bad) = given.iter().find(|a| !fragment_proto::valid_secret_name(&a.key)) {
        return Err(VaultError::Apps(format!("{:?} is not a name an app's code fills (^[A-Z][A-Z0-9_]{{0,63}}$)", bad.key)));
    }
    let mut apps = given.to_vec();
    apps.sort();
    apps.dedup();
    if let Some(twice) = apps.windows(2).find(|w| w[0].app == w[1].app) {
        return Err(VaultError::Apps(format!("{} is named twice: an app fills a secret by one name", twice[0].app)));
    }
    Ok(apps)
}

/// `held`'s apps (a secret placed as `place`) with `app` lent it under
/// `key`, in place of any name it had for that app: checked as a put's
/// apps are (`apps`), so a lend is held to the same rules.
pub fn lend(held: &[AppUse], place: &Place, app: &str, key: &str) -> Result<Vec<AppUse>, VaultError> {
    let (placing, _) = placing(place).map_err(|_| VaultError::Place("this secret's place is no longer one: set it again with its value".into()))?;
    let mut apps: Vec<AppUse> = held.iter().filter(|a| a.app != app).cloned().collect();
    apps.push(AppUse { app: app.to_string(), key: key.to_string() });
    self::apps(&apps, &placing)
}

/// The secret of `held` (each a secret's name and its apps) that binds
/// `app`'s `key`, other than `name`'s: an app's key is filled by one secret
/// of its person's, so a put that would bind a second is refused.
pub fn binds_already<'a>(name: &str, apps: &[AppUse], held: impl IntoIterator<Item = (&'a str, &'a [AppUse])>) -> Option<(&'a str, AppUse)> {
    held.into_iter().filter(|(other, _)| *other != name).find_map(|(other, theirs)| apps.iter().find(|a| theirs.contains(a)).map(|a| (other, a.clone())))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Catalog {
        Catalog::parse(r#"[{"name": "openrouter", "kind": "own", "hosts": ["openrouter.ai"], "placements": [{"header": "authorization", "format": "Bearer {}"}], "env": ["OPENROUTER_API_KEY"]}]"#).expect("a catalog")
    }

    fn put(value: &str, hosts: &[&str], place: Place, agents: &[&str]) -> PutSecret {
        PutSecret { value: Some(value.into()), hosts: hosts.iter().map(|h| h.to_string()).collect(), place, agents: agents.iter().map(|a| a.to_string()).collect(), apps: vec![] }
    }

    const AGENT: &str = "ada--k3x9";
    const NSEC: &str = "abababababababababababababababababababababababababababababababab";

    #[test]
    fn a_token_toward_a_host_by_default_goes_as_a_bearer_and_is_stored_so() {
        let c = check("linear", &put("lin_api_abc", &["api.linear.app", "api.linear.app"], Place::default(), &[AGENT, AGENT]), None, &catalog(), false).expect("valid");
        assert_eq!(c.hosts, ["api.linear.app"]);
        assert_eq!(c.agents, [AGENT]);
        assert_eq!(c.placing, Placing::Fill(Placement::Header { name: "authorization".into(), format: "Bearer {}".into() }));
        assert_eq!(c.place, Place { header: Some("authorization".into()), format: Some("Bearer {}".into()), ..Place::default() });
    }

    #[test]
    fn each_place_reads_as_the_catalog_s_and_a_signature_as_its_header() {
        let q = check("k", &put("v", &["a.example.com"], Place { query: Some("key".into()), ..Place::default() }, &[AGENT]), None, &catalog(), false).expect("query");
        assert_eq!(q.placing, Placing::Fill(Placement::Query { name: "key".into() }));
        let b = check("k", &put("v", &["a.example.com"], Place { basic: Some("password".into()), ..Place::default() }, &[AGENT]), None, &catalog(), false).expect("basic");
        assert_eq!(b.placing, Placing::Fill(Placement::Basic { half: Half::Password }));
        let h = check("k", &put("v", &["a.example.com"], Place { header: Some("x-api-key".into()), ..Place::default() }, &[AGENT]), None, &catalog(), false).expect("header");
        assert_eq!(h.placing, Placing::Fill(Placement::Header { name: "x-api-key".into(), format: "{}".into() }));
        let s = check("nostr-ada", &put(NSEC, &["finite.site"], Place { sign: Some("nip98".into()), ..Place::default() }, &[AGENT]), None, &catalog(), false).expect("sign");
        assert_eq!(s.placing, Placing::Nip98);
        assert_eq!(s.placing.placement(), Placement::Header { name: "authorization".into(), format: "Nostr {}".into() });
    }

    #[test]
    fn a_secret_with_no_hosts_is_parked_and_valid() {
        let c = check("telegram-ada", &put("123:abc", &[], Place::default(), &[AGENT]), None, &catalog(), false).expect("parked");
        assert!(c.hosts.is_empty());
    }

    #[test]
    fn names_a_key_uses_or_of_another_form_are_refused() {
        for name in ["openrouter", "", "Linear", "-x", "x-", "a_b", &"x".repeat(33)] {
            assert!(matches!(check(name, &put("v", &["a.example.com"], Place::default(), &[AGENT]), None, &catalog(), false), Err(VaultError::Name(_))), "{name:?}");
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
        assert!(check("linear", &put("lin_api_abc", &["api.linear.app"], Place::default(), &[AGENT]), None, &c, false).is_ok());
        for key in ["openrouter", "perplexity"] {
            let refused = check(key, &put("v", &["a.example.com"], Place::default(), &[AGENT]), None, &c, false);
            assert!(matches!(&refused, Err(VaultError::Name(why)) if why.contains("is a key this deployment offers")), "{key}: {refused:?}");
        }
    }

    #[test]
    fn values_are_printable_tokens_or_a_nostr_key_as_hex() {
        for v in ["", "has space", "tab\tin", "é", &"x".repeat(VALUE_MAX_BYTES + 1)] {
            assert!(matches!(check("k", &put(v, &["a.example.com"], Place::default(), &[AGENT]), None, &catalog(), false), Err(VaultError::Value(_))), "{v:?}");
        }
        let sign = Place { sign: Some("nip98".into()), ..Place::default() };
        // not hex, upper case, short
        for v in ["nsec1abc", &NSEC.to_uppercase(), &NSEC[..62]] {
            assert!(matches!(check("k", &put(v, &["a.example.com"], sign.clone(), &[AGENT]), None, &catalog(), false), Err(VaultError::Value(_))), "{v:?}");
        }
    }

    #[test]
    fn hosts_are_exact_public_names_and_at_most_eight() {
        for h in ["1.2.3.4", "[::1]", "localhost", "api.fragment.internal", "fragment.internal", "*.example.com", "Example.com", "example.com:443", "https://example.com", "example", "example.com."] {
            assert!(matches!(check("k", &put("v", &[h], Place::default(), &[AGENT]), None, &catalog(), false), Err(VaultError::Hosts(_))), "{h:?}");
        }
        let nine: Vec<String> = (0..9).map(|i| format!("h{i}.example.com")).collect();
        let nine: Vec<&str> = nine.iter().map(String::as_str).collect();
        assert!(matches!(check("k", &put("v", &nine, Place::default(), &[AGENT]), None, &catalog(), false), Err(VaultError::Hosts(_))));
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
            assert!(matches!(check("k", &put(NSEC, &["a.example.com"], p.clone(), &[AGENT]), None, &catalog(), false), Err(VaultError::Place(_))), "{p:?}");
        }
    }

    #[test]
    fn a_secret_names_one_to_sixteen_agents_by_fragment_name() {
        assert!(matches!(check("k", &put("v", &["a.example.com"], Place::default(), &[]), None, &catalog(), false), Err(VaultError::Agents(_))));
        assert!(matches!(check("k", &put("v", &["a.example.com"], Place::default(), &["ada"]), None, &catalog(), false), Err(VaultError::Agents(_))));
        let many: Vec<String> = (0..17).map(|i| format!("a{i}--k3x9")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert!(matches!(check("k", &put("v", &["a.example.com"], Place::default(), &many), None, &catalog(), false), Err(VaultError::Agents(_))));
    }

    fn app(app: &str, key: &str) -> AppUse {
        AppUse { app: app.into(), key: key.into() }
    }

    fn with_apps(put: PutSecret, apps: &[AppUse]) -> PutSecret {
        PutSecret { apps: apps.to_vec(), ..put }
    }

    const APP: &str = "tg--k3x9";

    /// Goal: a secret is bound per app as per agent (FIN-615, PR 12), an app
    /// under the name its code fills. Method: an app alone, apps beside
    /// agents, repeats; then each way an app's binding is wrong.
    #[test]
    fn a_secret_may_name_apps_each_once_under_a_name_its_code_fills() {
        let alone = check("telegram-ada", &with_apps(put("123:abc", &["api.telegram.org"], Place::default(), &[]), &[app(APP, "TELEGRAM_BOT_TOKEN")]), None, &catalog(), false).expect("an app alone");
        assert_eq!((alone.agents.len(), alone.apps), (0, vec![app(APP, "TELEGRAM_BOT_TOKEN")]));
        let both = check("linear", &with_apps(put("lin", &["api.linear.app"], Place::default(), &[AGENT]), &[app("b--k3x9", "LINEAR"), app("a--k3x9", "LINEAR"), app("b--k3x9", "LINEAR")]), None, &catalog(), false).expect("apps beside an agent");
        assert_eq!(both.apps, vec![app("a--k3x9", "LINEAR"), app("b--k3x9", "LINEAR")], "sorted, each once");
        let bad = [
            vec![app("tg", "TOKEN")],
            vec![app(APP, "token")],
            vec![app(APP, "_TOKEN")],
            vec![app(APP, "TOKEN"), app(APP, "OTHER")],
            (0..=APPS_MAX).map(|i| app(&format!("a{i}--k3x9"), "TOKEN")).collect(),
        ];
        for apps in bad {
            let refused = check("k", &with_apps(put("v", &["a.example.com"], Place::default(), &[]), &apps), None, &catalog(), false);
            assert!(matches!(refused, Err(VaultError::Apps(_))), "{apps:?}: {refused:?}");
        }
        // a key that signs never leaves its Vault, so no app's code places it
        let signer = check("nostr-ada", &with_apps(put(NSEC, &["finite.site"], sign(), &[AGENT]), &[app(APP, "NOSTR")]), None, &catalog(), false);
        assert!(matches!(&signer, Err(VaultError::Apps(why)) if why.contains("never leaves")), "{signer:?}");
    }

    /// Goal: an app's key is filled by one secret of its person's. Method:
    /// another secret binding the same app's key, the same app under
    /// another key, and the secret itself set again.
    #[test]
    fn an_apps_key_is_bound_by_one_secret_alone() {
        let tg = [app(APP, "TELEGRAM_BOT_TOKEN")];
        let other = [app(APP, "OTHER"), app("b--k3x9", "TELEGRAM_BOT_TOKEN")];
        let held = [("telegram-ada", &tg[..]), ("other", &other[..])];
        assert_eq!(binds_already("telegram-new", &tg, held), Some(("telegram-ada", tg[0].clone())));
        assert_eq!(binds_already("telegram-ada", &tg, held), None, "the secret set again is not another");
        assert_eq!(binds_already("x", &[app(APP, "THIRD")], held), None);
    }

    /// Goal: a lend adds one app's name for a secret and nothing else, held
    /// to a put's rules. Method: a new app, the same app renamed, a signing
    /// key, a bad name.
    #[test]
    fn a_lend_names_one_app_once_and_is_held_to_a_puts_rules() {
        let bearer = Place { header: Some("authorization".into()), format: Some("Bearer {}".into()), ..Place::default() };
        let held = [app("b--k3x9", "OTHER")];
        assert_eq!(lend(&held, &bearer, APP, "TOKEN"), Ok(vec![app("b--k3x9", "OTHER"), app(APP, "TOKEN")]));
        assert_eq!(lend(&[app(APP, "OLD")], &bearer, APP, "NEW"), Ok(vec![app(APP, "NEW")]), "an app's name for it replaced, never two");
        assert!(matches!(lend(&[], &sign(), APP, "TOKEN"), Err(VaultError::Apps(_))), "a key that signs is lent to no app");
        assert!(matches!(lend(&[], &bearer, APP, "token"), Err(VaultError::Apps(_))));
    }

    /// Goal: a dev or e2e fleet's fakes may be a Vault secret's hosts, as an
    /// app's own secret's, and nowhere else; `.internal` never. Method: each
    /// host, local and not.
    #[test]
    fn a_local_fleet_takes_its_own_hosts_and_never_an_internal_one() {
        for h in ["127.0.0.1", "10.0.0.7", "localhost"] {
            assert!(host_ok(h, true), "{h}");
            assert!(!host_ok(h, false), "{h}");
        }
        for h in ["api.fragment.internal", "fragment.internal", "Localhost", "127.0.0.1:8080"] {
            assert!(!host_ok(h, true), "{h}");
        }
        assert!(host_ok("api.telegram.org", true) && host_ok("api.telegram.org", false));
    }

    /// Goal: rows and puts from before apps read as they did, and a secret
    /// with none writes none (the wire's lock: added, nothing an older client
    /// feels). Method: the old forms, decoded and encoded.
    #[test]
    fn a_secret_from_before_apps_reads_with_none_and_writes_none() {
        let row: fragment_proto::vault::Secret = serde_json::from_str(r#"{"name":"linear","hosts":["api.linear.app"],"place":{},"agents":["ada--k3x9"],"setAt":7}"#).expect("an older row");
        assert!(row.apps.is_empty());
        assert!(!serde_json::to_string(&row).expect("json").contains("apps"));
        let put: PutSecret = serde_json::from_str(r#"{"hosts":["api.telegram.org"],"agents":[],"apps":[{"app":"tg--k3x9","key":"TELEGRAM_BOT_TOKEN"}]}"#).expect("a put with apps");
        assert_eq!(put.apps, vec![app(APP, "TELEGRAM_BOT_TOKEN")]);
        assert!(serde_json::from_str::<PutSecret>(r#"{"hosts":[],"agents":[],"apps":[{"app":"tg--k3x9","key":"K","as":"x"}]}"#).is_err(), "an app's binding is its two fields");
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
        let parked = check("nostr-ada--k3x9", &put(NSEC, &[], sign(), &[AGENT]), None, &catalog(), false).expect("parked");
        let c = check("nostr-ada--k3x9", &keep(&["finite.site"], sign(), &[AGENT]), Some(&parked.place), &catalog(), false).expect("kept");
        assert_eq!((c.hosts, c.placing), (vec!["finite.site".to_string()], Placing::Nip98));
        // a fill's value kept from one fill place to another
        let bearer = check("linear", &put("lin_api_abc", &["api.linear.app"], Place::default(), &[AGENT]), None, &catalog(), false).expect("bearer");
        let q = check("linear", &keep(&["api.linear.app"], Place { query: Some("key".into()), ..Place::default() }, &[AGENT, "bo--k3x9"]), Some(&bearer.place), &catalog(), false).expect("query");
        assert_eq!((q.placing, q.agents), (Placing::Fill(Placement::Query { name: "key".into() }), vec![AGENT.to_string(), "bo--k3x9".to_string()]));
    }

    #[test]
    fn no_value_for_a_new_secret_or_across_sign_and_fill_is_refused() {
        let new = check("nostr-ada", &keep(&["finite.site"], sign(), &[AGENT]), None, &catalog(), false);
        assert!(matches!(&new, Err(VaultError::Value(why)) if why.contains("a new one needs its value")), "{new:?}");
        let signer = sign();
        let to_fill = check("nostr-ada", &keep(&["finite.site"], Place::default(), &[AGENT]), Some(&signer), &catalog(), false);
        assert!(matches!(&to_fill, Err(VaultError::Value(why)) if why.contains("Nostr key that signs")), "{to_fill:?}");
        let bearer = Place { header: Some("authorization".into()), format: Some("Bearer {}".into()), ..Place::default() };
        let to_sign = check("linear", &keep(&["api.linear.app"], sign(), &[AGENT]), Some(&bearer), &catalog(), false);
        assert!(matches!(&to_sign, Err(VaultError::Value(why)) if why.contains("set it again with a Nostr key")), "{to_sign:?}");
        // a stored place that reads as none keeps nothing
        let two = Place { header: Some("authorization".into()), query: Some("key".into()), ..Place::default() };
        assert!(matches!(check("linear", &keep(&["api.linear.app"], Place::default(), &[AGENT]), Some(&two), &catalog(), false), Err(VaultError::Value(_))));
    }

    /// With no value, everything else is checked as with one.
    #[test]
    fn no_value_is_no_pass_on_the_name_hosts_place_or_agents() {
        let signer = sign();
        let held = Some(&signer);
        assert!(matches!(check("openrouter", &keep(&["finite.site"], sign(), &[AGENT]), held, &catalog(), false), Err(VaultError::Name(_))));
        assert!(matches!(check("k", &keep(&["1.2.3.4"], sign(), &[AGENT]), held, &catalog(), false), Err(VaultError::Hosts(_))));
        assert!(matches!(check("k", &keep(&["finite.site"], Place { sign: Some("nip46".into()), ..Place::default() }, &[AGENT]), held, &catalog(), false), Err(VaultError::Place(_))));
        assert!(matches!(check("k", &keep(&["finite.site"], sign(), &[]), held, &catalog(), false), Err(VaultError::Agents(_))));
        assert!(matches!(check("k", &keep(&["finite.site"], sign(), &["ada"]), held, &catalog(), false), Err(VaultError::Agents(_))));
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
}
