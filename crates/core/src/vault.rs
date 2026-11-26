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
use fragment_proto::vault::{AnswerAsk, AskSecret, Place, PutSecret, Secret};

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

/// Whether a secret going to `hosts` keeps its person's Vault within
/// `egress::VAULT_HOSTS_MAX` distinct hosts, the catalog's counted like any
/// other (a computer registers each Vault host as its own, so a catalog's
/// change never moves a Vault over its share: `computer::egress`).
/// `others`: the hosts the Vault's other secrets go to; `held`: every host
/// it goes to now, this secret's too. A Vault over the cap from before it
/// is refused only a host it does not go to yet, so each secret may still
/// be set again as it is, and fewer hosts are always fine.
pub fn room(hosts: &[String], others: &[String], held: &[String]) -> Result<(), VaultError> {
    use crate::computer::egress::VAULT_HOSTS_MAX;
    let after: std::collections::BTreeSet<&String> = others.iter().chain(hosts).collect();
    let new = hosts.iter().any(|h| !held.contains(h));
    if after.len() <= VAULT_HOSTS_MAX || !new {
        return Ok(());
    }
    Err(VaultError::Hosts(format!(
        "your vault sends secrets to at most {VAULT_HOSTS_MAX} hosts, and this would make {}: take a host off another secret, or remove one, first",
        after.len()
    )))
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

/// A move that names a host its Vault does not point the secret at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wider {
    /// The secret's name in its Vault.
    pub name: String,
    /// The host named that its Vault does not list.
    pub host: String,
}

impl std::fmt::Display for Wider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} goes only to the hosts set for it in your vault, not {}", self.name, self.host)
    }
}

impl std::error::Error for Wider {}

/// May a move of `name` keep it toward `asked`, its Vault pointing it at
/// `held`? Only within `held`: a move narrows what its person said in
/// their Vault, never widens it. A parked secret (no hosts) was pointed at
/// none, so its move names them; only its person moves one (the cell's
/// `put_secret`), so those hosts are their own word too.
pub fn within(name: &str, held: &[String], asked: &[String]) -> Result<(), Wider> {
    if held.is_empty() {
        return Ok(());
    }
    match asked.iter().find(|h| !held.contains(h)) {
        Some(host) => Err(Wider { name: name.to_string(), host: host.clone() }),
        None => Ok(()),
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

/// May an agent ask its person for `ask`? Its name a secret's (`name_ok`),
/// its reason one short line, its hosts a secret's (`host_ok`, at most
/// `HOSTS_MAX`). Whether its person keeps one of that name is not asked
/// here: the agent learns nothing of secrets it is not given, and its card
/// tells its person (`ask_notice`).
pub fn ask_ok(ask: &AskSecret, catalog: &Catalog) -> Result<(), VaultError> {
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
    Ok(())
}

/// Whether `id` has a Vault ask id's form: 32 lower-case hex characters.
pub fn ask_id_ok(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The platform-origin URL where the person can open a Vault ask directly
/// (`/vault/ask/{id}`), reachable from any chat bridge (Telegram, SimpleX).
pub fn ask_url(platform: &str, id: &str) -> String {
    format!("{}/vault/ask/{id}", platform.trim_end_matches('/'))
}

fn join_unique(base: &[String], extra: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(base.len() + extra.len());
    out.extend(base.iter().cloned());
    out.extend(extra.iter().cloned());
    out.sort();
    out.dedup();
    out
}

/// The hosts an answer to `ask` puts its secret toward: the ones asked,
/// and, for one `held` already (an Allow), its own beside them. The Vault
/// holds them to its cap (`room`) as the ask is made, so its person never
/// answers a card whose answer would be refused.
pub fn ask_hosts(ask: &AskSecret, held: Option<&Secret>) -> Vec<String> {
    join_unique(held.map(|h| h.hosts.as_slice()).unwrap_or_default(), &ask.hosts)
}

/// What `agent`'s ask puts in its person's Vault as they answer it:
/// - when `held` is `None`: a new secret with the pasted value (`!allow`),
///   toward the hosts asked, in the default place, for `agent` alone;
/// - when `held` is `Some(h)`: an Allow (`allow: true`, no value) keeping
///   the existing sealed value and place, adding `agent`, and unioning any
///   asked hosts (checked against `HOSTS_MAX` and `AGENTS_MAX`).
///
/// An ask never overwrites an existing secret's value.
pub fn answered(ask: &AskSecret, agent: &str, held: Option<&Secret>, answer: &AnswerAsk) -> Result<PutSecret, VaultError> {
    match held {
        None => {
            if answer.allow {
                return Err(VaultError::Ask(format!("{} is not in your vault yet: paste its value to save it", ask.name)));
            }
            let value = answer.value.as_deref().map(str::trim).filter(|v| !v.is_empty()).ok_or_else(|| VaultError::Ask("paste the secret first".into()))?;
            let hosts = ask_hosts(ask, None);
            Ok(PutSecret { value: Some(value.to_string()), hosts, place: Place::default(), agents: vec![agent.to_string()] })
        }
        Some(h) => {
            if !answer.allow {
                return Err(VaultError::Ask(format!("you already keep {}: allow it to keep its saved value, or replace it with `fragment vault set {}`", ask.name, ask.name)));
            }
            if answer.value.is_some() {
                return Err(VaultError::Ask("Allow keeps the saved secret: it takes no value".into()));
            }
            let hosts = ask_hosts(ask, Some(h));
            if hosts.len() > HOSTS_MAX {
                return Err(VaultError::Hosts(format!("at most {HOSTS_MAX}, not {}", hosts.len())));
            }
            let agents = join_unique(&h.agents, &[agent.to_string()]);
            if agents.len() > AGENTS_MAX {
                return Err(VaultError::Agents(format!("at most {AGENTS_MAX}, not {}", agents.len())));
            }
            Ok(PutSecret { value: None, hosts, place: h.place.clone(), agents })
        }
    }
}

/// An ask on its person's card (`GET /api/notices`): `agent_fragment` is the
/// asking agent's fragment name, `agent` is what its person calls it (its
/// title, else its label), `picture` its face, and `held` is the secret
/// currently kept under `ask.name` beside its revision (`"none"` if none).
/// - a new secret (`None`): the masked field, Save (carrying `{ held }`),
///   and Decline;
/// - one already kept (`Some`): no field, Allow (carrying `{ allow: true,
///   held }`) and Decline, saying what an Allow adds (this agent, new
///   hosts); Decline alone when that would pass the secret's limits.
pub fn ask_notice(
    id: &str,
    ask: &AskSecret,
    agent_fragment: &str,
    agent: &str,
    picture: Option<String>,
    expires_at: i64,
    held: (Option<&Secret>, &str),
) -> Notice {
    let route = format!("/api/vault/asks/{id}");
    let decline = NoticeAction::post("Decline", format!("{route}/decline"), serde_json::json!({}));
    let (title, detail, field, actions) = match held {
        (None, revision) => {
            let goes = match ask.hosts.is_empty() {
                true => format!("It goes nowhere until you say where, and {agent} never sees it."),
                false => format!("Only {} would receive it. {agent} gets a stand-in to send there, never the secret itself.", ask.hosts.join(", ")),
            };
            (
                format!("{agent} asks for {}", ask.name),
                goes,
                Some(NoticeField { placeholder: "Paste it here".into() }),
                vec![NoticeAction::post_field("Save", route, serde_json::json!({ "held": revision })), decline],
            )
        }
        (Some(secret), revision) => {
            let shares = !secret.agents.iter().any(|a| a == agent_fragment);
            let hosts = join_unique(&secret.hosts, &ask.hosts);
            let new_hosts: Vec<&str> = hosts.iter().filter(|h| !secret.hosts.contains(h)).map(String::as_str).collect();
            let title = format!("{agent} asks to use {}", ask.name);
            let kept = match secret.hosts.is_empty() {
                true => format!("You already keep {}, parked: sent nowhere yet.", ask.name),
                false => format!("You already keep {} for {}.", ask.name, secret.hosts.join(", ")),
            };
            if hosts.len() > HOSTS_MAX || secret.agents.len() + usize::from(shares) > AGENTS_MAX {
                let detail = format!("{kept} Allowing this would pass its limit of {HOSTS_MAX} hosts and {AGENTS_MAX} agents: decline it, or change {} with `fragment vault set`.", ask.name);
                (title, detail, None, vec![decline])
            } else {
                let mut grants = Vec::new();
                if shares {
                    grants.push(format!("shares it with {agent}"));
                }
                if !new_hosts.is_empty() {
                    grants.push(format!("sends it to {} too", new_hosts.join(", ")));
                }
                let does = match grants.is_empty() {
                    true => "Allowing changes nothing saved.".to_string(),
                    false => format!("Allowing {}.", grants.join(" and ")),
                };
                let others = match !new_hosts.is_empty() && secret.agents.iter().any(|a| a != agent_fragment) {
                    true => " Your other agents that use it can send there too.",
                    false => "",
                };
                let detail = format!("{kept} {does}{others} {agent} gets a stand-in, never the secret itself.");
                let allow = NoticeAction::post("Allow", route, serde_json::json!({ "allow": true, "held": revision }));
                (title, detail, None, vec![allow, decline])
            }
        }
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

/// An ask's card (`ask_notice`) for a client that draws no field and may
/// hide a card's words behind its main action (`GET /api/notices` without
/// `field=1`: iOS today): no field, and in place of Save or Allow, Open,
/// the ask's own page (`ask_url`), which draws the whole card. A grant is
/// never pressed from a card that may not show what it grants.
pub fn linked(mut notice: Notice, url: &str) -> Notice {
    let decline = notice.actions.pop();
    assert!(decline.as_ref().is_some_and(|d| d.label == "Decline"), "an ask's card ends with its Decline");
    notice.field = None;
    notice.actions = std::iter::once(NoticeAction::open("Open", url)).chain(decline).collect();
    notice
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Catalog {
        Catalog::parse(r#"[{"name": "openrouter", "kind": "own", "hosts": ["openrouter.ai"], "placements": [{"header": "authorization", "format": "Bearer {}"}], "env": ["OPENROUTER_API_KEY"]}]"#).expect("a catalog")
    }

    fn strings(l: &[&str]) -> Vec<String> {
        l.iter().map(|h| h.to_string()).collect()
    }

    #[test]
    fn a_vault_sends_to_at_most_eight_hosts() {
        use crate::computer::egress::VAULT_HOSTS_MAX;
        let eight: Vec<String> = (0..VAULT_HOSTS_MAX).map(|i| format!("h{i}.example.com")).collect();
        let ninth = strings(&["h8.example.com"]);
        // eight across its other secrets: a ninth host is refused, and says why
        let refused = room(&ninth, &eight, &eight).expect_err("a ninth host");
        assert!(refused.to_string().contains("at most 8 hosts") && refused.to_string().contains("would make 9"), "{refused}");
        // one it goes to already costs nothing more; a catalog's host counts as any other
        assert_eq!(room(&strings(&["h0.example.com", "h1.example.com"]), &eight, &eight), Ok(()));
        assert!(room(&strings(&["openrouter.ai"]), &eight, &eight).is_err(), "a catalog's host is one of the eight");
        // the secret that went to h7 moved to h8: still eight
        assert_eq!(room(&ninth, &eight[..7], &eight), Ok(()));
        // a Vault over the cap from before it: set again as it is, or with fewer, is fine
        let twelve: Vec<String> = (0..12).map(|i| format!("h{i}.example.com")).collect();
        assert_eq!(room(&twelve[..2], &twelve[2..], &twelve), Ok(()));
        assert_eq!(room(&twelve[..1], &twelve[2..], &twelve), Ok(()));
        assert!(room(&strings(&["new.example.com"]), &twelve[2..], &twelve).is_err(), "a host it does not go to yet");
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

    fn ask(name: &str, why: &str, hosts: &[&str]) -> AskSecret {
        AskSecret { name: name.into(), why: why.into(), hosts: hosts.iter().map(|h| h.to_string()).collect() }
    }

    /// Goal: an agent asks for a secret by a Vault name, in one short line,
    /// toward a secret's hosts. Method: each part valid, then each wrong.
    #[test]
    fn an_ask_names_a_secret_why_in_a_line_and_a_secrets_hosts() {
        assert!(ask_ok(&ask("github", "To push your shop's code", &["api.github.com"]), &catalog()).is_ok());
        assert!(ask_ok(&ask("github", "Kept for later", &[]), &catalog()).is_ok(), "no hosts: parked, as a put may be");
        for (bad, what) in [
            (ask("openrouter", "a key the deployment offers", &[]), "name"),
            (ask("GitHub", "x", &[]), "name"),
            (ask("github", "", &[]), "why"),
            (ask("github", "   ", &[]), "why"),
            (ask("github", &"x".repeat(WHY_MAX_CHARS + 1), &[]), "why"),
            (ask("github", "two\nlines", &[]), "why"),
            (ask("github", "x", &["https://api.github.com"]), "hosts"),
            (ask("github", "x", &["*.github.com"]), "hosts"),
            (ask("github", "x", &["api.fragment.internal"]), "hosts"),
            (ask("github", "x", &["a.example.com"; 9]), "hosts"),
        ] {
            assert!(ask_ok(&bad, &catalog()).is_err(), "{what}: {bad:?}");
        }
        assert!(ask_ok(&ask("github", &"é".repeat(WHY_MAX_CHARS), &[]), &catalog()).is_ok(), "counted in characters, not bytes");
    }

    #[test]
    fn an_ask_is_held_to_the_vaults_hosts_as_its_answer_would_be() {
        use crate::computer::egress::VAULT_HOSTS_MAX;
        let c = catalog();
        let eight: Vec<String> = (0..VAULT_HOSTS_MAX).map(|i| format!("h{i}.example.com")).collect();
        // a new secret toward a ninth host: refused as the ask is made, in the cap's words
        let new = ask_hosts(&ask("ninth", "a ninth", &["h8.example.com"]), None);
        assert_eq!(new, strings(&["h8.example.com"]));
        assert!(room(&new, &eight, &eight, &c).expect_err("a ninth host").to_string().contains("at most 8 hosts"));
        // one toward a host the Vault goes to already costs nothing
        assert_eq!(room(&ask_hosts(&ask("again", "same host", &["h0.example.com"]), None), &eight, &eight, &c), Ok(()));
        // an Allow's hosts are the kept secret's and the asked ones together
        let kept = held(&["h7.example.com"], Place::default(), &["other"]);
        let allow = ask_hosts(&ask("site7", "and a ninth", &["h8.example.com"]), Some(&kept));
        assert_eq!(allow, strings(&["h7.example.com", "h8.example.com"]));
        assert!(room(&allow, &eight[..7], &eight, &c).is_err(), "Allow adding a ninth host");
        assert_eq!(room(&ask_hosts(&ask("site7", "no new host", &[]), Some(&kept)), &eight[..7], &eight, &c), Ok(()));
    }

    fn held(hosts: &[&str], place: Place, agents: &[&str]) -> Secret {
        Secret {
            name: "github".into(),
            hosts: hosts.iter().map(|h| h.to_string()).collect(),
            place,
            agents: agents.iter().map(|a| a.to_string()).collect(),
            set_at: 1,
        }
    }

    #[test]
    fn ask_id_and_url_follow_their_forms() {
        let id = "0123456789abcdef0123456789abcdef";
        assert!(ask_id_ok(id));
        for bad in ["", "0123", &"A".repeat(32), &"g".repeat(32), "../0123456789abcdef0123456789abc"] {
            assert!(!ask_id_ok(bad), "{bad:?}");
        }
        assert_eq!(ask_url("https://finite.chat/", id), format!("https://finite.chat/vault/ask/{id}"));
        assert_eq!(ask_url("https://finite.chat", id), format!("https://finite.chat/vault/ask/{id}"));
    }

    /// Goal: an answer makes a new secret for the asking agent alone when none
    /// is held, and an Allow for an existing secret keeps its sealed value and
    /// place while adding the asking agent and unioning hosts. Method: new,
    /// shared to second agent, widened with new host, and every invalid mix.
    #[test]
    fn an_answer_makes_a_new_secret_or_allows_an_existing_one_without_overwriting() {
        let new_ans = AnswerAsk { held: "none".into(), value: Some(" ghp_abc \n".into()), allow: false };
        let new = answered(&ask("github", "push", &["uploads.github.com", "api.github.com"]), AGENT, None, &new_ans).expect("answered");
        assert_eq!(new, PutSecret { value: Some("ghp_abc".into()), hosts: vec!["api.github.com".into(), "uploads.github.com".into()], place: Place::default(), agents: vec![AGENT.into()] }, "trimmed as a paste ends");
        assert!(check("github", &new, None, &catalog()).is_ok());

        for blank in [None, Some(String::new()), Some("  \n".into())] {
            let bad = AnswerAsk { held: "none".into(), value: blank.clone(), allow: false };
            assert!(matches!(answered(&ask("github", "push", &[]), AGENT, None, &bad), Err(VaultError::Ask(_))), "{blank:?}");
        }
        assert!(matches!(
            answered(&ask("github", "push", &[]), AGENT, None, &AnswerAsk { held: "none".into(), value: None, allow: true }),
            Err(VaultError::Ask(_))
        ), "cannot Allow when no secret exists");

        // existing secret: second agent allowed, keeping value & place, adding agent & unioning hosts
        let custom_place = Place { header: Some("x-token".into()), format: Some("Token {}".into()), ..Place::default() };
        let existing = held(&["api.github.com"], custom_place.clone(), &[AGENT]);
        let allow_ans = AnswerAsk { held: "rev1".into(), value: None, allow: true };
        let shared = answered(&ask("github", "upload", &["uploads.github.com"]), "bea--k3x9", Some(&existing), &allow_ans).expect("allowed");
        assert_eq!(shared, PutSecret {
            value: None,
            hosts: vec!["api.github.com".into(), "uploads.github.com".into()],
            place: custom_place.clone(),
            agents: vec![AGENT.into(), "bea--k3x9".into()],
        });
        assert!(check("github", &shared, Some(&existing.place), &catalog()).is_ok());

        // existing secret: refuses overwrite (allow: false) or value on Allow
        assert!(matches!(
            answered(&ask("github", "rotate", &[]), AGENT, Some(&existing), &AnswerAsk { held: "rev1".into(), value: Some("new_val".into()), allow: false }),
            Err(VaultError::Ask(_))
        ));
        assert!(matches!(
            answered(&ask("github", "share", &[]), "bea--k3x9", Some(&existing), &AnswerAsk { held: "rev1".into(), value: Some("new_val".into()), allow: true }),
            Err(VaultError::Ask(_))
        ));
        // exceeding host cap on union is refused
        let eight = held(&["a.example.com", "b.example.com", "c.example.com", "d.example.com", "e.example.com", "f.example.com", "g.example.com", "h.example.com"], Place::default(), &[AGENT]);
        assert!(matches!(
            answered(&ask("github", "more", &["i.example.com"]), "bea--k3x9", Some(&eight), &allow_ans),
            Err(VaultError::Hosts(_))
        ));
    }

    /// Goal: the card says who asks, why, where it goes, and that the
    /// agent never sees it: Save + masked field for a new secret, and
    /// Allow + Decline (no field, carrying held revision) when the secret
    /// already exists (sharing to another agent, widening hosts, or
    /// changing nothing). Method: every card variant on the wire.
    #[test]
    fn the_card_says_who_why_where_and_carries_the_field_or_allow() {
        let id = "0123456789abcdef0123456789abcdef";
        let n = ask_notice(id, &ask("github", "To push your shop's code", &["api.github.com"]), AGENT, "Juniper", Some("/p".into()), 9, (None, "none"));
        assert_eq!(n.title, "Juniper asks for github");
        assert_eq!(n.text.as_deref(), Some("To push your shop's code"));
        assert_eq!(n.detail.as_deref(), Some("Only api.github.com would receive it. Juniper gets a stand-in to send there, never the secret itself."));
        assert_eq!(n.until, Some(Until { at: 9, said: "Expires".into() }));
        assert_eq!(n.actions[0].body, Some(serde_json::json!({ "held": "none" })));
        assert!(n.field.is_some() && n.actions[0].field && n.actions[0].post.as_deref() == Some("/api/vault/asks/0123456789abcdef0123456789abcdef"));
        assert!(!n.actions[1].field && n.actions[1].post.as_deref() == Some("/api/vault/asks/0123456789abcdef0123456789abcdef/decline"));

        let open = linked(n.clone(), "https://finite.chat/vault/ask/a1");
        assert!(open.field.is_none() && open.title == n.title && open.detail == n.detail, "{open:?}");
        assert_eq!(open.actions.iter().map(|a| (a.label.as_str(), a.open.as_deref(), a.post.is_some())).collect::<Vec<_>>(), [("Open", Some("https://finite.chat/vault/ask/a1"), false), ("Decline", None, true)]);
        let parked = ask_notice(id, &ask("github", "Kept for later", &[]), AGENT, "Juniper", None, 9, (None, "none"));
        assert_eq!(parked.detail.as_deref(), Some("It goes nowhere until you say where, and Juniper never sees it."));

        // Another agent asking for an existing secret (same hosts): Allow + Decline, no field
        let existing = held(&["api.github.com"], Place::default(), &[AGENT]);
        let share = ask_notice(id, &ask("github", "Review PRs", &["api.github.com"]), "bea--k3x9", "Bea", None, 9, (Some(&existing), "rev1"));
        assert_eq!(share.title, "Bea asks to use github");
        assert!(share.field.is_none());
        assert_eq!(share.actions.len(), 2);
        assert_eq!(share.actions[0].label, "Allow");
        assert!(!share.actions[0].field);
        assert_eq!(share.actions[0].body, Some(serde_json::json!({ "allow": true, "held": "rev1" })));
        assert_eq!(share.actions[1].label, "Decline");
        assert_eq!(share.detail.as_deref(), Some("You already keep github for api.github.com. Allowing shares it with Bea. Bea gets a stand-in, never the secret itself."));

        // Another agent asking for an existing secret + new host: calls out new host & other agents
        let widen = ask_notice(id, &ask("github", "Upload release", &["uploads.github.com"]), "bea--k3x9", "Bea", None, 9, (Some(&existing), "rev1"));
        assert_eq!(widen.detail.as_deref(), Some("You already keep github for api.github.com. Allowing shares it with Bea and sends it to uploads.github.com too. Your other agents that use it can send there too. Bea gets a stand-in, never the secret itself."));

        // Same agent asking to widen hosts
        let same_widen = ask_notice(id, &ask("github", "Upload release", &["uploads.github.com"]), AGENT, "Juniper", None, 9, (Some(&existing), "rev1"));
        assert!(same_widen.detail.as_deref().is_some_and(|d| d.contains("Allowing sends it to uploads.github.com too.") && !d.contains("other agents")), "{:?}", same_widen.detail);
        assert_eq!(same_widen.actions[0].label, "Allow");

        // Same agent already has secret and host: Allow changes nothing
        let same_have = ask_notice(id, &ask("github", "Need token", &["api.github.com"]), AGENT, "Juniper", None, 9, (Some(&existing), "rev1"));
        assert!(same_have.field.is_none() && same_have.actions[0].label == "Allow");
        assert!(same_have.detail.as_deref().is_some_and(|d| d.contains("Allowing changes nothing saved.")), "{:?}", same_have.detail);

        // Host cap exceeded: Decline alone
        let eight = held(&["a.example.com", "b.example.com", "c.example.com", "d.example.com", "e.example.com", "f.example.com", "g.example.com", "h.example.com"], Place::default(), &[AGENT]);
        let capped = ask_notice(id, &ask("github", "One more", &["i.example.com"]), "bea--k3x9", "Bea", None, 9, (Some(&eight), "rev8"));
        assert!(capped.field.is_none() && capped.actions.len() == 1 && capped.actions[0].label == "Decline");
    }

    #[test]
    fn a_move_keeps_a_secret_within_its_vault_hosts_and_names_a_parked_one_s() {
        let held = |h: &[&str]| h.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let tg = held(&["api.telegram.org"]);
        // the same hosts, or fewer
        assert_eq!(within("bot", &tg, &tg), Ok(()));
        assert_eq!(within("bot", &held(&["a.example.com", "b.example.com"]), &held(&["b.example.com"])), Ok(()));
        // any other host, alone or beside one of its own, is refused, named
        assert_eq!(within("bot", &tg, &held(&["127.0.0.1"])), Err(Wider { name: "bot".into(), host: "127.0.0.1".into() }));
        assert_eq!(within("bot", &tg, &held(&["api.telegram.org", "evil.example.com"])), Err(Wider { name: "bot".into(), host: "evil.example.com".into() }));
        // parked (no hosts): its person's move names them
        assert_eq!(within("bot", &[], &tg), Ok(()));
    }
}
