//! The fleet's settings, from Worker variables (`cell/.dev.vars` in dev,
//! rendered `vars` at deploy), built once per isolate (`CONFIG`). Nothing about a fleet is a constant in code
//! (docs/cloudflare-v1.md, decision 4): the hostname suffix and the code.storage org
//! arrive here. The fleet's secrets do not: the host secret, the
//! code.storage key, and WorkOS's client id and API key are Secrets Store
//! bindings, read only by keys.rs.

use std::sync::OnceLock;

use fragment_proto::ledger::Plan;
use fragment_proto::{valid_fragment_name, ErrorCode};
use worker::Env;

use crate::error::{CellError, CellResult};

pub struct CodeStorageConfig {
    pub org: String,
    /// The API base, e.g. `https://api.<org>.code.storage`.
    pub api: String,
    /// `CODESTORAGE_REPO_PREFIX`: what this deployment's repos are named
    /// with first (a branch deployment's `<branch>--`), so deployments that
    /// share an org never share a repo.
    pub repo_prefix: String,
}

/// WorkOS AuthKit (phase 4 slice B): fragment's own environment, configured
/// when its client id is bound (`secrets_store::WORKOS_CLIENT`; keys.rs
/// reads it, and its API key, as `keys::workos`).
pub struct WorkOsConfig {
    /// `WORKOS_API_URL` (default https://api.workos.com; dev and the e2e: the fake).
    pub api: String,
}

/// Stripe (docs/billing.md): configured when its key is bound
/// (`secrets_store::STRIPE_KEY`; keys.rs reads it, and the webhook's
/// signing secret). Without it seats are comped only.
pub struct StripeConfig {
    /// `STRIPE_API_URL` (default https://api.stripe.com; dev and the e2e: the fake).
    pub api: String,
    /// `FRAGMENT_STRIPE_PORTAL`: the deployment's own portal configuration
    /// (never the account's default, which finite-mono's is).
    pub portal: Option<String>,
    /// `FRAGMENT_STRIPE_TAX` (default on): Stripe's automatic tax on a
    /// Checkout, prices before tax (decision 54); `off` where the account
    /// has no tax settings (a sandbox).
    pub tax: bool,
}

pub struct Config {
    /// `FRAGMENT_CONNECTORS`: the connectors an owner may link, and their
    /// relay targets; absent, `connectors::Policy::default()` (docs/computers.md, "Connector policy").
    pub connectors: fragment_core::connectors::Policy,
    /// `CODESTORAGE_ORG` (required) and the settings beside it.
    codestorage: CodeStorageConfig,
    /// `FRAGMENT_HOST_SUFFIX`: fragments are served from `<name>.<suffix>`.
    /// Every deployment names one.
    pub host_suffix: String,
    /// `FRAGMENT_HOST_LABEL_SUFFIX` (`--<branch>`): a branch deployment's
    /// fragments are `<name>--<branch>.<suffix>`, beside the other
    /// branches' in one zone.
    host_label_suffix: Option<String>,
    /// `FRAGMENT_COMPUTER_IMAGE`: the image a new computer is pinned to (a
    /// name in wrangler.jsonc's `containers` images). Unset, the deployment
    /// makes no computers.
    pub computer_image: Option<String>,
    /// `FRAGMENT_COMPUTER_SNAPSHOTS=off`: computers sleep without a
    /// container snapshot and wake from their image and backup (local
    /// workerd takes no snapshots).
    pub computer_snapshots: bool,
    /// `FRAGMENT_COMPUTER_UNSAVED_MAX_MS` (the deploy config's
    /// `computers.unsaved_max_ms`): how long a computer whose sleep's save
    /// keeps failing stays awake before it sleeps unsaved. See
    /// docs/durable-computers.md and `fragment_core::computer`.
    pub computer_unsaved_max_ms: i64,
    /// `FRAGMENT_SUPPORT_URL` (the deploy config's `support_url`): where a
    /// person whose computer will not start gets help, an `https:` page or
    /// a `mailto:` address (`fragment_core::computer::support_url_ok`).
    /// Unset, the shell shows them what to tell whoever runs it.
    pub support_url: Option<String>,
    /// `FRAGMENT_POLL_INTERVAL_S`: a busy fragment's pass, and the poll backstop (default 300).
    pub poll_interval_ms: i64,
    /// `FRAGMENT_EGRESS_LOCAL=allow`: jobs may fetch private and loopback
    /// addresses (dev and e2e fleets, which call local fakes). Never on a
    /// shared fleet.
    pub egress_local: bool,
    /// `FRAGMENT_BLOB_GRACE_S`: how long a blob no branch names is kept
    /// (default 7 days: a rollback within it still has its bytes).
    pub blob_grace_ms: i64,
    /// The shortest wait before a delivery, or a card's shot, is tried
    /// again (10 s; the wait grows with the delivery's age, and a shot's
    /// doubles), and the longest (an hour). `FRAGMENT_DELIVERY_RETRY_S`
    /// pins both: a test fleet's fixed pace.
    pub delivery_retry_s: u32,
    pub delivery_retry_max_s: u32,
    /// `AI_GATEWAY_ID`: the AI Gateway the model route calls through
    /// (models.rs): the deployment's own, named, since `default` makes a
    /// gateway that logs (spike S4).
    pub ai_gateway_id: Option<String>,
    /// `FRAGMENT_AI_URL`: dev and the e2e only. The model route POSTs the
    /// AI binding's input to `<url>/run/<model>` instead (a fake at the
    /// vendor boundary, labeled so: models.rs) and needs no gateway.
    pub ai_url: Option<String>,
    /// `FRAGMENT_MAIL_FROM`: a bare address or unquoted `Name <address>`
    /// (the deploy config's `mail_from`), on a domain onboarded to Email
    /// Sending; none: the deployment sends no mail (mail.rs).
    pub mail_from: Option<fragment_core::mail::Sender>,
    /// `FRAGMENT_MAIL_URL`: dev and the e2e only. Mail is POSTed to
    /// `<url>/send` (the binding's input) instead: a fake at the vendor
    /// boundary, which never sends it.
    pub mail_url: Option<String>,
    workos: Option<WorkOsConfig>,
    stripe: Option<StripeConfig>,
    /// `FRAGMENT_PLATFORM_URL` (required): the platform's own origin, where
    /// sign-in and the platform session live (e.g. https://finite.chat).
    /// It is also the contact a push's VAPID token names (`sub`, RFC 8292):
    /// a push service may refuse one it cannot reach (Apple's answers 403
    /// `BadJwtToken`).
    pub platform_url: String,
    /// `FRAGMENT_DEFAULT_PLAN`: a new person's plan (docs/ledger.md):
    /// `guest`, the default and production's, or `seat` or
    /// `seat_always_on` (dev and the e2e: `seat`).
    pub default_plan: Plan,
    /// `FRAGMENT_OPERATORS`: keys and identities, as npubs or 64 hex
    /// (`parse_list` reads each as 64 hex), that grant credit and set
    /// plans, seats and overdrafts.
    operators: Option<Result<Vec<String>, String>>,
    /// `FRAGMENT_SIGNINS_PENDING_MAX`: sign-ins begun and not finished that
    /// the Registry keeps before it lets the oldest go (default
    /// `SIGNINS_PENDING_MAX_DEFAULT`).
    pub signins_pending_max: u64,
    /// `FRAGMENT_CLI_BEGINS_PER_IP_MINUTE`: CLI codes one address may ask
    /// for a minute (default `CLI_BEGINS_PER_IP_MINUTE_DEFAULT`).
    pub cli_begins_per_ip_minute: u32,
    /// `FRAGMENT_TEST_SECRET` (a Worker secret): the test levers
    /// (`/api/test/*`) answer requests that carry it, on a local fleet or a
    /// branch deployment only (`fragment_core::levers`). Without it they
    /// are no route, as on production.
    pub test_secret: Option<fragment_core::levers::TestSecret>,
    /// Whether this fleet has test levers (`test_secret` is set): the
    /// cells' test controls answer only on one, and only through the
    /// router's routes, which check the secret.
    pub test_hooks: bool,
    /// A branch deployment's levers reach the e2e's own things alone
    /// (`fragment_core::levers::Fleet::Branch`): its `e2e-…` fragments and
    /// its e2e people's ledgers, never the registry's, and its e2e people
    /// are made within a day's caps. A local fleet's reach everything.
    pub levers_scoped: bool,
    /// `FRAGMENT_DEPLOY_ID`: which deployment this is (the deploy sets it;
    /// `/healthz` answers it in `x-fragment-deploy`; default `dev`). It is
    /// the release every call into a platform object carries, and every
    /// object admits only its own (fragment_core::release): a Worker
    /// variable, so the Worker and every object of one deploy run one value.
    pub deploy_id: String,
    /// A published CLI for this deployment's exact inputs, verified by CI.
    /// Absent when its release failed or this deployment does not release.
    pub cli_version: Option<String>,
    /// `FRAGMENT_DEPLOYED_AT`: when this release was deployed, whole
    /// seconds since 1970 (the deploy sets it; none where a fleet sets none,
    /// dev's default). Every call into a platform object carries it beside
    /// the release, and an object that refuses a call from a release
    /// deployed after its own restarts onto it (fragment_core::release
    /// `restarts`). A Worker variable, as the release is.
    pub deployed_at: Option<u64>,
    /// `FRAGMENT_PROVIDERS`: the provider catalog (`fragment_core::catalog`),
    /// every credential a computer's guest may use: the WorkOS Pipes
    /// connections, the operator's keys (each bound as
    /// `secrets_store::operator_key` names it, and each priced, which is the
    /// price book's `keys`) and people's own keys, each with its hosts, its
    /// placements and its environment variables. None by default.
    /// `FRAGMENT_PRICE_BOOK_VERSION` (default the defaults',
    /// `fragment_core::price::DEFAULT_BOOK_VERSION`; the book's is the higher
    /// of the two) is informational (#451).
    pub providers: fragment_core::catalog::Catalog,
    /// The price book this release charges with (`configured_book`): the
    /// core's defaults, each OpenRouter slot's ceiling from `models`, the
    /// operator keys' prices from `providers`, and the version. Built and checked once per isolate; every call into a
    /// ledger is its own release's, so a ledger keeps none of its own
    /// (decision 62).
    pub book: fragment_core::price::PriceBook,
    /// `FRAGMENT_MODELS` (the deploy config's `models`, less its keys): where
    /// each of the model route's names runs, its tiers', `fallback` (a
    /// tier's call is made on it once more when its own fails before
    /// answering) and `vision` (a runtime's calls about an image): a Workers
    /// AI model, or a model on an OpenAI-compatible API, whose key is bound
    /// for its slot (`secrets_store::model_key`). Each slot unnamed is on
    /// Workers AI as before (`fragment_core::models::Models`); one out of
    /// shape, or whose model neither the price book nor its slot's ceiling
    /// prices (one on OpenRouter names its ceiling), is refused, at the
    /// deploy and here. A local fleet's may name its fake on
    /// 127.0.0.1.
    pub models: fragment_core::models::Models,
    /// `FRAGMENT_SWAP_UPSTREAM` (the e2e only): a swapped request goes here,
    /// its host in `x-fragment-upstream-host`, instead of to its host.
    pub swap_upstream: Option<String>,
}

fn var(env: &Env, name: &str) -> Option<String> {
    env.var(name).ok().map(|v| v.to_string().trim().to_string()).filter(|s| !s.is_empty())
}

/// The provider catalog (`FRAGMENT_PROVIDERS`): a deployment whose is
/// malformed is refused at its first request (the deploy checks it first).
fn providers(env: &Env) -> fragment_core::catalog::Catalog {
    var(env, "FRAGMENT_PROVIDERS").map(|v| fragment_core::catalog::Catalog::parse(&v).unwrap_or_else(|e| panic!("FRAGMENT_PROVIDERS: {e}"))).unwrap_or_default()
}

/// Where this fleet runs, as its levers see it (`levers::fleet_of`): its
/// fragments' hosts carry a branch's mark, or it lets jobs reach local
/// addresses (dev's and the e2e's), or neither.
fn levers_fleet(local: bool, branch: bool) -> fragment_core::levers::Fleet {
    fragment_core::levers::fleet_of(branch, local)
}

/// `FRAGMENT_TEST_SECRET`, as this fleet honours it: a local fleet's or a
/// branch deployment's. One set on a deployment of its own, or one too
/// short, is said in the log and honoured nowhere: the levers stay off,
/// and nothing else is refused.
fn test_secret(env: &Env, fleet: fragment_core::levers::Fleet) -> Option<fragment_core::levers::TestSecret> {
    use fragment_core::levers::honoured;
    let secret = env.secret("FRAGMENT_TEST_SECRET").ok().map(|s| s.to_string().trim().to_string()).filter(|s| !s.is_empty());
    match honoured(secret.as_deref(), fleet) {
        Ok(secret) => secret,
        Err(why) => {
            worker::console_error!("{}", serde_json::json!({ "event": "levers.refused", "why": why.message() }));
            None
        }
    }
}

/// `FRAGMENT_DEPLOY_ID`, or `dev` where a fleet names none: a release's
/// id (fragment_core::release::valid), or the isolate does not start.
fn deploy_id(env: &Env) -> String {
    let id = var(env, "FRAGMENT_DEPLOY_ID").unwrap_or_else(|| "dev".into());
    assert!(fragment_core::release::valid(&id), "FRAGMENT_DEPLOY_ID is 1 to {} of letters, digits, `.`, `_` and `-`", fragment_core::release::ID_MAX_BYTES);
    id
}

/// `FRAGMENT_DEPLOYED_AT`, or `None` where a fleet sets none: seconds as
/// digits (fragment_core::release::deployed), or the isolate does not start.
fn deployed_at(env: &Env) -> Option<u64> {
    let v = var(env, "FRAGMENT_DEPLOYED_AT")?;
    let at = fragment_core::release::deployed(&v);
    assert!(at.is_some(), "FRAGMENT_DEPLOYED_AT is whole seconds since 1970, 1 to {} digits", fragment_core::release::DEPLOYED_MAX_DIGITS);
    at
}

/// The isolate's settings, built from the first `env` it is handed and
/// shared by every request, cell, and queue batch it runs after that.
///
/// A cache, so its contract. Source: the deployment's Worker variables.
/// Invalidation: a deploy (or, in dev, a change to `.dev.vars`, which
/// reloads) starts new isolates, and each builds its own. Stale reads:
/// impossible, because a Worker version's variables are fixed for every
/// isolate that runs it; `from_env` checks that on every call against
/// `FRAGMENT_DEPLOY_ID`.
static CONFIG: OnceLock<Config> = OnceLock::new();

/// The price book a release charges with: the core's defaults, each model
/// slot's ceiling (an OpenRouter tier's: `Models::priced`), the
/// configuration's operator keys' prices, and the higher of its version
/// and the defaults' (decision 4: pricing lives there). One the core
/// refuses (its checks are the deploy's too, so none should come) is said
/// aloud, and the defaults charge instead: a billing problem never stops
/// an agent (decision 62).
fn configured_book(providers: &fragment_core::catalog::Catalog, models: &fragment_core::models::Models, version: u32) -> fragment_core::price::PriceBook {
    let mut book = models.priced(fragment_core::price::PriceBook::defaults());
    book.keys = providers.key_prices();
    book.version = book.version.max(version);
    match book.validate() {
        Ok(()) => book,
        Err(fault) => {
            worker::console_error!("{}", serde_json::json!({ "event": "ledger.book-refused", "version": book.version, "fault": format!("{fault:?}") }));
            fragment_core::price::PriceBook::defaults()
        }
    }
}

impl Config {
    /// The isolate's settings (`CONFIG`): built once, from `env`'s variables.
    pub fn from_env(env: &Env) -> &'static Config {
        let cfg = CONFIG.get_or_init(|| Config::build(env));
        // one variable read, against the 19 a build takes: variables that
        // changed under a running isolate would break the contract above
        assert_eq!(deploy_id(env), cfg.deploy_id, "a Worker variable changed under a running isolate");
        cfg
    }

    fn build(env: &Env) -> Config {
        let pinned_retry_s = var(env, "FRAGMENT_DELIVERY_RETRY_S").and_then(|s| s.parse::<u32>().ok()).filter(|s| *s >= 1);
        let suffix = |name: &str| var(env, name).map(|s| s.trim_start_matches('.').to_ascii_lowercase());
        let host_suffix = suffix("FRAGMENT_HOST_SUFFIX").expect("FRAGMENT_HOST_SUFFIX names where fragments are served");
        // a branch deployment's fragments share its zone with other branches'
        let host_label_suffix = var(env, "FRAGMENT_HOST_LABEL_SUFFIX").map(|s| s.to_ascii_lowercase());
        assert!(
            host_label_suffix.as_deref().is_none_or(valid_label_suffix),
            "FRAGMENT_HOST_LABEL_SUFFIX is `--` and a branch name (1-{} of a-z, 0-9 and single dashes inside)",
            fragment_proto::limits::BRANCH_MAX_BYTES
        );
        // the platform's origin is named in frames' `frame-ancestors` and
        // messages' targets (fragment_core::frames), so it is one exactly
        let platform_url = var(env, "FRAGMENT_PLATFORM_URL").map(|u| u.trim_end_matches('/').to_string()).unwrap_or_default();
        assert!(fragment_core::frames::is_origin(&platform_url), "FRAGMENT_PLATFORM_URL (required) is an origin (scheme://host[:port], lower case, no path)");
        assert!(fragment_core::frames::is_origin(&format!("https://{host_suffix}")), "FRAGMENT_HOST_SUFFIX is a host name");
        let egress_local = var(env, "FRAGMENT_EGRESS_LOCAL").as_deref() == Some("allow");
        let providers = providers(env);
        let models = fragment_core::models::Models::parse(var(env, "FRAGMENT_MODELS").as_deref(), &fragment_core::price::PriceBook::defaults(), egress_local)
            .unwrap_or_else(|e| panic!("FRAGMENT_MODELS: {e}"));
        let levers_fleet = levers_fleet(egress_local, host_label_suffix.is_some());
        let test_secret = test_secret(env, levers_fleet);
        Config {
            connectors: var(env, "FRAGMENT_CONNECTORS").map_or_else(fragment_core::connectors::Policy::default, |v| fragment_core::connectors::Policy::parse(&v).unwrap_or_else(|e| panic!("FRAGMENT_CONNECTORS: {e}"))),
            codestorage: {
                let org = var(env, "CODESTORAGE_ORG").unwrap_or_else(|| panic!("CODESTORAGE_ORG names the deployment's code.storage org"));
                let api =
                    var(env, "CODESTORAGE_API_URL").map(|a| a.trim_end_matches('/').to_string()).unwrap_or_else(|| fragment_core::codestorage::default_api(&org));
                let repo_prefix = var(env, "CODESTORAGE_REPO_PREFIX").unwrap_or_default();
                assert!(
                    repo_prefix.is_empty() || (repo_prefix.ends_with("--") && repo_prefix.len() <= 20 && repo_prefix.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')),
                    "CODESTORAGE_REPO_PREFIX is a branch name and `--`"
                );
                CodeStorageConfig { org, api, repo_prefix }
            },
            host_suffix,
            host_label_suffix,
            computer_image: var(env, "FRAGMENT_COMPUTER_IMAGE"),
            computer_snapshots: var(env, "FRAGMENT_COMPUTER_SNAPSHOTS").as_deref() != Some("off"),
            computer_unsaved_max_ms: var(env, "FRAGMENT_COMPUTER_UNSAVED_MAX_MS")
                .map(|v| v.parse::<i64>().ok().filter(|ms| *ms >= 0).unwrap_or_else(|| panic!("FRAGMENT_COMPUTER_UNSAVED_MAX_MS is a whole number of ms, not {v:?}")))
                .unwrap_or(fragment_core::computer::UNSAVED_MAX_MS_DEFAULT),
            support_url: var(env, "FRAGMENT_SUPPORT_URL").map(|u| {
                assert!(fragment_core::computer::support_url_ok(&u), "FRAGMENT_SUPPORT_URL is an https: page or a mailto: address, not {u:?}");
                u
            }),
            poll_interval_ms: var(env, "FRAGMENT_POLL_INTERVAL_S").and_then(|s| s.parse::<i64>().ok()).filter(|s| *s >= 1).unwrap_or(300) * 1000,
            egress_local,
            blob_grace_ms: var(env, "FRAGMENT_BLOB_GRACE_S").and_then(|s| s.parse::<i64>().ok()).filter(|s| *s >= 1).unwrap_or(7 * 24 * 3600) * 1000,
            delivery_retry_s: pinned_retry_s.unwrap_or(10),
            delivery_retry_max_s: pinned_retry_s.unwrap_or(3600),
            workos: crate::keys::bound(env, fragment_core::secrets_store::WORKOS_CLIENT).then(|| WorkOsConfig {
                api: var(env, "WORKOS_API_URL").map(|u| u.trim_end_matches('/').to_string()).unwrap_or_else(|| "https://api.workos.com".into()),
            }),
            stripe: crate::keys::bound(env, fragment_core::secrets_store::STRIPE_KEY).then(|| StripeConfig {
                api: var(env, "STRIPE_API_URL").map(|u| u.trim_end_matches('/').to_string()).unwrap_or_else(|| "https://api.stripe.com".into()),
                portal: var(env, "FRAGMENT_STRIPE_PORTAL").filter(|p| !p.is_empty()),
                tax: match var(env, "FRAGMENT_STRIPE_TAX").as_deref() {
                    None | Some("on") => true,
                    Some("off") => false,
                    Some(other) => panic!("FRAGMENT_STRIPE_TAX is on or off, not {other:?}"),
                },
            }),
            platform_url,
            default_plan: default_plan(env),
            ai_gateway_id: var(env, "AI_GATEWAY_ID").inspect(|id| {
                assert!(id != "default", "AI_GATEWAY_ID names the deployment's own gateway: `default` makes one that logs (spike S4)");
            }),
            ai_url: var(env, "FRAGMENT_AI_URL").map(|u| u.trim_end_matches('/').to_string()),
            mail_from: env.var("FRAGMENT_MAIL_FROM").ok().map(|f| {
                fragment_core::mail::Sender::parse(&f.to_string()).unwrap_or_else(|e| panic!("FRAGMENT_MAIL_FROM: {e}"))
            }),
            mail_url: var(env, "FRAGMENT_MAIL_URL").map(|u| u.trim_end_matches('/').to_string()),
            operators: var(env, "FRAGMENT_OPERATORS").map(|l| fragment_core::npub::parse_list(&l)),
            signins_pending_max: var(env, "FRAGMENT_SIGNINS_PENDING_MAX")
                .and_then(|s| s.parse::<u64>().ok())
                .filter(|n| *n >= 1)
                // the registry counts rows as i64; a larger setting means "no cap to speak of"
                .map(|n| n.min(i64::MAX as u64))
                .unwrap_or(fragment_proto::limits::SIGNINS_PENDING_MAX_DEFAULT),
            cli_begins_per_ip_minute: var(env, "FRAGMENT_CLI_BEGINS_PER_IP_MINUTE")
                .and_then(|s| s.parse::<u32>().ok())
                .filter(|n| *n >= 1)
                .unwrap_or(fragment_proto::limits::CLI_BEGINS_PER_IP_MINUTE_DEFAULT),
            test_hooks: test_secret.is_some(),
            levers_scoped: test_secret.is_some() && levers_fleet == fragment_core::levers::Fleet::Branch,
            test_secret,
            deploy_id: deploy_id(env),
            cli_version: var(env, "FRAGMENT_CLI_VERSION").filter(|v| fragment_core::release::cli_version(v).is_some()),
            deployed_at: deployed_at(env),
            book: configured_book(&providers, &models, var(env, "FRAGMENT_PRICE_BOOK_VERSION").map(|v| v.parse().unwrap_or_else(|_| panic!("FRAGMENT_PRICE_BOOK_VERSION is a whole number"))).unwrap_or(1)),
            providers,
            models,
            swap_upstream: var(env, "FRAGMENT_SWAP_UPSTREAM").map(|u| u.trim_end_matches('/').to_string()),
        }
    }

    /// Whether the signer (its key, 64 hex, if it signed, and its identity)
    /// is one of the deployment's operators.
    pub fn is_operator(&self, key: Option<&str>, identity: &str) -> CellResult<bool> {
        match &self.operators {
            None => Ok(false),
            Some(Err(e)) => Err(CellError::host(format!("FRAGMENT_OPERATORS: {e}"))),
            Some(Ok(listed)) => Ok(listed.iter().any(|l| Some(l.as_str()) == key || fragment_core::npub::identity_of(l) == identity)),
        }
    }

    /// A branch deployment's mark on its fragments' labels (`--<branch>`),
    /// or nothing.
    pub fn host_label_suffix(&self) -> &str {
        self.host_label_suffix.as_deref().unwrap_or("")
    }

    /// Stripe, when this deployment sells seats: else a 400 that says so.
    pub fn stripe(&self) -> CellResult<&StripeConfig> {
        self.stripe
            .as_ref()
            .ok_or_else(|| CellError::invalid(format!("this deployment sells no seats (no {} binding): an operator comps them", fragment_core::secrets_store::STRIPE_KEY)).because(fragment_proto::Reason::NotSold))
    }

    /// The deployment as Stripe's metadata names it: its platform's origin.
    pub fn stripe_deployment(&self) -> &str {
        &self.platform_url
    }

    pub fn workos(&self) -> CellResult<&WorkOsConfig> {
        self.workos
            .as_ref()
            .ok_or_else(|| CellError::new(ErrorCode::HostFailed, format!("sign-in is not configured on this fleet (no {} binding)", fragment_core::secrets_store::WORKOS_CLIENT)))
    }

    /// The platform's origin (`FRAGMENT_PLATFORM_URL`).
    pub fn platform(&self) -> String {
        self.platform_url.clone()
    }

    /// Whether `host` is the platform's own (`FRAGMENT_PLATFORM_URL`'s),
    /// which the router takes before any fragment's under the suffix.
    pub fn is_platform_host(&self, host: &str) -> bool {
        url::Url::parse(&self.platform_url).is_ok_and(|u| u.host_str().is_some_and(|h| h.eq_ignore_ascii_case(host)))
    }

    /// Whether `host` is the suffix's own name. Past `is_platform_host`, it
    /// is no one's: the platform is elsewhere.
    pub fn is_suffix(&self, host: &str) -> bool {
        self.host_suffix.eq_ignore_ascii_case(host)
    }

    pub fn codestorage(&self) -> &CodeStorageConfig {
        &self.codestorage
    }

    /// The fragment a hostname names, when it is `<name>.<suffix>` (a
    /// name is one DNS label, so the suffix's one wildcard certificate
    /// covers every fragment). This is the only way a host becomes a
    /// fragment: an exact single label under the suffix (with a branch's
    /// mark, its own).
    pub fn fragment_of_host(&self, host: &str) -> Option<String> {
        let label = label_under(host, &self.host_suffix)?;
        let name = match &self.host_label_suffix {
            Some(branch) => label.strip_suffix(branch.as_str())?,
            None => &label,
        };
        valid_fragment_name(name).then(|| name.to_string())
    }

    /// A claimed address's label, in this deployment alone. A lookup in
    /// the registry still has to prove it names a fragment.
    pub fn address_of_host(&self, host: &str) -> Option<String> {
        if self.is_platform_host(host) { return None; }
        let label = label_under(host, &self.host_suffix)?;
        let name = label.strip_suffix(self.host_label_suffix())?;
        fragment_core::address::valid(name, self.host_label_suffix()).then(|| name.to_string())
    }

    pub fn address_origin(&self, arrived: &url::Url, label: &str) -> String {
        format!("{}://{label}{}", arrived.scheme(), self.address_suffix(arrived))
    }

    pub fn address_suffix(&self, arrived: &url::Url) -> String {
        let port = arrived.port().map(|p| format!(":{p}")).unwrap_or_default();
        format!("{}.{suffix}{port}", self.host_label_suffix(), suffix = self.host_suffix)
    }

    /// The computer a hostname names (`<24 hex>--computer.<suffix>`, a
    /// branch's mark before the dot): its own origin, where its ports are.
    pub fn computer_of_host(&self, host: &str) -> Option<String> {
        let label = label_under(host, &self.host_suffix)?;
        let label = match &self.host_label_suffix {
            Some(branch) => label.strip_suffix(branch.as_str())?.to_string(),
            None => label,
        };
        fragment_proto::computer::computer_of_label(&label)
    }

    /// A computer's own origin (its ports are served there), on the
    /// platform's scheme and port; `None` for an id that is no computer's.
    pub fn computer_origin(&self, id: &str) -> Option<String> {
        let suffix = &self.host_suffix;
        let label = fragment_proto::computer::computer_label(id)?;
        let platform = url::Url::parse(&self.platform_url).ok()?;
        let port = platform.port().map(|p| format!(":{p}")).unwrap_or_default();
        Some(format!("{}://{label}{}.{suffix}{port}", platform.scheme(), self.host_label_suffix()))
    }

    /// The label a host has under the suffix (`x` of `x.<suffix>`), if it
    /// is one: such a host is a fragment's or no one's, never the platform's.
    pub fn subdomain(&self, host: &str) -> Option<String> {
        label_under(host, &self.host_suffix)
    }

    /// Where a fragment is served, given the URL a request arrived on (its
    /// scheme and port carry over).
    pub fn canonical(&self, arrived: &url::Url, name: &str) -> String {
        format!("{}/", self.origin(arrived, name))
    }

    /// A fragment's own origin as a visitor from outside reaches it, with
    /// no request to take a scheme and port from (card.rs: the renderer
    /// opens it): the platform's (`FRAGMENT_PLATFORM_URL`'s) scheme and port.
    pub fn outside_origin(&self, name: &str) -> String {
        let arrived = url::Url::parse(&self.platform_url).expect("FRAGMENT_PLATFORM_URL is an origin (checked as the isolate starts)");
        self.origin(&arrived, name)
    }

    /// A fragment's own origin, as a browser on its page names it in
    /// `Origin` (`scheme://host[:port]`).
    pub fn origin(&self, arrived: &url::Url, name: &str) -> String {
        // The router resolved this request's public address before making
        // its Routed URL. Keep that real origin through sockets, auth,
        // app fetches and relative links; never rewrite it to the default.
        if arrived.host_str().and_then(|host| self.address_of_host(host)).is_some() {
            return arrived.origin().ascii_serialization();
        }
        let port = arrived.port().map(|p| format!(":{p}")).unwrap_or_default();
        let host = name;
        let branch = self.host_label_suffix.as_deref().unwrap_or("");
        format!("{}://{host}{branch}.{}{port}", arrived.scheme(), self.host_suffix)
    }
}

/// `FRAGMENT_DEFAULT_PLAN`, `guest` when unset. Any other value is the
/// deployment's mistake, found as its first isolate starts.
fn default_plan(env: &Env) -> Plan {
    match var(env, "FRAGMENT_DEFAULT_PLAN").as_deref() {
        None | Some("guest") => Plan::Guest,
        Some("seat") => Plan::Seat,
        Some("seat_always_on") => Plan::SeatAlwaysOn,
        Some(other) => panic!("FRAGMENT_DEFAULT_PLAN is guest, seat or seat_always_on, not {other:?}"),
    }
}

/// A branch's mark on its fragments' labels: `--` and its name (a branch
/// `xtask deploy` makes), so `<name>--<branch>.<suffix>` stays one DNS
/// label under the zone's one wildcard certificate (docs/cloudflare-v1.md,
/// decision 20; a create past it is refused: `fragment_core::names`), and
/// a computer's `<24 hex>--computer--<branch>` fits in one.
fn valid_label_suffix(s: &str) -> bool {
    s.strip_prefix("--").is_some_and(fragment_proto::valid_branch)
}

/// The label `host` has under `suffix` (`x` of `x.<suffix>`), if it is one.
fn label_under(host: &str, suffix: &str) -> Option<String> {
    let host = host.to_ascii_lowercase();
    host.strip_suffix(suffix)?.strip_suffix('.').map(str::to_string)
}
