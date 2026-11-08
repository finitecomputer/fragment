//! The price book (docs/ledger.md; docs/cloudflare-v1.md, decisions 24,
//! 25 and 37, spike S4): what each meter costs at Cloudflare's list
//! price, and what a person is charged for it.
//!
//! A price is micro-dollars per a fixed count of its unit (a million
//! tokens, an awake hour, a GB-month), so every price in Cloudflare's own
//! tables is an exact integer here. A row's charge is
//! `ceil(list × (1 + fee) × (1 + margin))`, computed exactly in 128 bits
//! and rounded up once, never per part. The fee is AI Gateway's 5% on the
//! credits that pay for AI through Unified Billing (spike S4), so it
//! applies to tokens and neurons only; the margin is the operator's, 50%
//! unless the book sets another. The book is data the operator sets (the
//! deploy's configuration); each release prices its ledgers' batches with
//! its own (crate::ledger, `meter`; decision 62).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::decide::Model;

/// One US dollar in micro-dollars.
pub const USD: i64 = 1_000_000;
/// Basis points in a whole: 10,000 bp is 100%.
pub const BP: u32 = 10_000;
/// The operator's margin when the configuration names none (decision 24).
pub const MARGIN_BP_DEFAULT: u32 = 5_000;
/// AI Gateway's fee on the credits that pay for AI (spike S4): AI's cost
/// basis is its list price × 1.05.
pub const CREDITS_FEE_BP_DEFAULT: u32 = 500;
/// A margin above 10× or a fee above 100% is a typo in a configuration,
/// not a price.
pub const MARGIN_BP_MAX: u32 = 100_000;
pub const FEE_BP_MAX: u32 = 10_000;
/// One price, in micro-dollars per its unit's count: $1M a million tokens
/// (or an awake hour) is far past anything Cloudflare lists.
pub const PRICE_MAX: i64 = 1_000_000 * USD;
/// One quantity of a usage: 10^15 tokens, ms or byte-hours is far past
/// any one row (a 20 GB disk for a month is 1.4 × 10^13 byte-hours), and
/// the bound keeps the 128-bit arithmetic below from ever overflowing.
pub const QUANTITY_MAX: u64 = 1_000_000_000_000_000;
/// One row's charge: no single call or sample costs $100,000, so a row
/// that would is a bug upstream, refused rather than billed.
pub const CHARGE_MAX: i64 = 100_000 * USD;
/// Entries in each of a book's lists (models, instances, keys).
pub const PRICES_MAX: usize = 256;
/// A model's, an instance type's or a key's name.
pub const NAME_MAX_BYTES: usize = 128;

/// The largest numerator `price` forms: four token classes at their
/// limits, times the largest fee and margin factors.
const NUMERATOR_MAX: i128 = 4 * (QUANTITY_MAX as i128) * (PRICE_MAX as i128) * ((BP + FEE_BP_MAX) as i128) * ((BP + MARGIN_BP_MAX) as i128);
const _: () = assert!(NUMERATOR_MAX < i128::MAX / 2, "a charge's exact numerator fits 128 bits with room");
const _: () = assert!(CHARGE_MAX <= i64::MAX / 1024, "a ledger sums many charges in an i64");

/// Tokens are priced per million (the catalogs' unit).
pub const TOKENS_PER: u64 = 1_000_000;
/// Neurons are metered in thousandths (Workers AI reports fractions, such
/// as 15.33) and priced per thousand neurons.
pub const MILLI_NEURONS_PER: u64 = 1_000_000;
/// Awake time is metered in milliseconds and priced per hour.
pub const AWAKE_MS_PER: u64 = 3_600_000;
/// Storage is metered in byte-hours (a sample's bytes × the hours since
/// the last sample) and priced per GB-month, taken as 10^9 bytes for 720
/// hours: Cloudflare's GB is no smaller and its month no shorter, so a
/// byte-hour here never costs less than Cloudflare's.
pub const STORAGE_BYTE_HOURS_PER: u64 = 1_000_000_000 * 720;
/// Requests are priced per million.
pub const REQUESTS_PER: u64 = 1_000_000;
/// Browser time is metered in milliseconds and priced per hour.
pub const BROWSER_MS_PER: u64 = 3_600_000;
/// Image transformations are priced per thousand.
pub const IMAGES_PER: u64 = 1_000;

/// What happened, in a meter's own unit (decision 24). The quantities are
/// exactly what the source counted; the book prices them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Usage {
    /// A model's tokens through AI Gateway, by its catalog id. `input`
    /// excludes cached tokens (OpenAI-shaped usage counts them in
    /// `prompt_tokens`, so the intercept subtracts them); `cache_write` is
    /// Anthropic's cache creation. Only the stream's last, cumulative
    /// usage is metered (spike S4).
    Tokens { model: String, input: u64, cached_input: u64, cache_write: u64, output: u64 },
    /// Workers AI neurons through the gateway, in thousandths of a neuron
    /// (the fraction Workers AI reports, × 1000, rounded up; an image's,
    /// from its tiles and steps: crate::media).
    Neurons { milli: u64 },
    /// A computer awake, in milliseconds, on its instance type (the
    /// book's name for it).
    Awake { instance: String, ms: u64 },
    /// Bytes held, sampled: the sample's bytes × the hours since the last.
    Storage { class: StorageClass, byte_hours: u64 },
    /// Requests served (each a router Worker request and the Durable
    /// Object request behind it).
    Requests { count: u64 },
    /// Unique dynamic workers (one fragment's code version) loaded on one
    /// UTC day.
    DynamicWorkers { count: u64 },
    /// Browser Rendering time, in milliseconds (a deploy's screenshot).
    Browser { ms: u64 },
    /// Cloudflare Images' unique transformations.
    Images { count: u64 },
    /// An operator key's use (decision 37), in the key's own unit: a call,
    /// unless its price counts something else.
    Key { key: String, units: u64 },
}

/// Where sampled bytes are held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageClass {
    /// Blobs, site copies, screenshots and backups.
    R2,
    /// A fragment's SQLite (its Durable Object's).
    Sqlite,
    /// A fragment's git repository at code.storage.
    Git,
}

/// Why a usage is not one the book can price.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageFault {
    /// A model, instance or key name that is empty, too long, or not
    /// printable ASCII.
    Name,
    /// A quantity over `QUANTITY_MAX`.
    Quantity,
}

/// Whether `s` is 1 to `max` bytes of printable ASCII without spaces: the
/// shape of every id and name the ledger stores.
pub fn printable(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

impl Usage {
    /// Whether the usage is one a book can price: its names well formed,
    /// its quantities within `QUANTITY_MAX`.
    pub fn validate(&self) -> Result<(), UsageFault> {
        let name = match self {
            Usage::Tokens { model, .. } => Some(model),
            Usage::Awake { instance, .. } => Some(instance),
            Usage::Key { key, .. } => Some(key),
            _ => None,
        };
        if let Some(n) = name {
            if !printable(n, NAME_MAX_BYTES) {
                return Err(UsageFault::Name);
            }
        }
        let largest = match self {
            Usage::Tokens { input, cached_input, cache_write, output, .. } => *input.max(cached_input).max(cache_write).max(output),
            Usage::Neurons { milli } => *milli,
            Usage::Awake { ms, .. } | Usage::Browser { ms } => *ms,
            Usage::Storage { byte_hours, .. } => *byte_hours,
            Usage::Requests { count } | Usage::DynamicWorkers { count } | Usage::Images { count } => *count,
            Usage::Key { units, .. } => *units,
        };
        if largest <= QUANTITY_MAX {
            Ok(())
        } else {
            Err(UsageFault::Quantity)
        }
    }
}

/// A model's prices, micro-dollars per million tokens of each class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenPrices {
    pub input: i64,
    pub cached_input: i64,
    pub cache_write: i64,
    pub output: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelPrice {
    /// The catalog id the intercept calls (`@cf/zai-org/glm-5.3-flash`).
    pub model: String,
    pub price: TokenPrices,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstancePrice {
    pub instance: String,
    /// Micro-dollars per awake hour.
    pub hour: i64,
}

/// Micro-dollars per GB-month (`STORAGE_BYTE_HOURS_PER`), by class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoragePrices {
    pub r2: i64,
    pub sqlite: i64,
    pub git: i64,
}

impl StoragePrices {
    fn of(&self, class: StorageClass) -> i64 {
        match class {
            StorageClass::R2 => self.r2,
            StorageClass::Sqlite => self.sqlite,
            StorageClass::Git => self.git,
        }
    }
}

/// An operator key's price: `micros` per `per` units (a search API at $5
/// per thousand calls is `5_000_000` per `1000`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyPrice {
    pub key: String,
    pub micros: i64,
    pub per: u64,
}

/// Every price, the fee and the margin, and a version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriceBook {
    /// Informational (a ledger's status shows it, `priceBook`): a ledger
    /// takes the configured book whenever its contents differ from its
    /// own, whatever the versions (#451).
    pub version: u32,
    pub margin_bp: u32,
    /// Applied to AI (tokens and neurons), which credits pay for.
    pub credits_fee_bp: u32,
    pub models: Vec<ModelPrice>,
    /// Micro-dollars per thousand neurons (`MILLI_NEURONS_PER`).
    pub neurons: i64,
    pub instances: Vec<InstancePrice>,
    pub storage: StoragePrices,
    /// Micro-dollars per million requests.
    pub requests: i64,
    /// Micro-dollars per unique dynamic worker per day.
    pub dynamic_workers: i64,
    /// Micro-dollars per browser hour.
    pub browser: i64,
    /// Micro-dollars per thousand unique image transformations.
    pub images: i64,
    pub keys: Vec<KeyPrice>,
}

// The default book, at list price on 2026-10-02, each with its source.

/// The tiers' models (decision 23), the route's vision and fallback model
/// (`fragment_core::models`). Sources: Workers AI's
/// catalog (`GET /ai/models/search`, `properties.price`) for both GLMs and
/// DeepSeek V4 Flash (read 2026-10-09; its neurons on two calls that day
/// matched these to the micro-dollar), and the AI model catalog page for
/// Opus 5.5, Anthropic's list price passed through Unified Billing (spike
/// S4; the gateway's own `cost` matched these on every call). Workers AI's
/// models have no cache writes: a write price is the input price, so a
/// write one reported would never be free.
///
/// Finite Private's GLM-5.3 Flash (decision 61), which publishes no price
/// (finite-mono meters a key in weighted tokens against its window, never
/// in dollars), is at Workers AI's GLM-5.3 Flash's list price, the credits
/// fee and the margin with it, so a person pays the same for a call
/// whichever of the two answers it (decision for Paul). Its SGLang reports
/// no cached tokens, so its prompts are all priced uncached.
///
/// A model through OpenRouter (Finite's tiers' since 2026-10-15; decision
/// 61) is priced at its slot's ceiling (`crate::models::Upstream::ceiling`,
/// laid over these by `Models::priced`), so the model a tier runs there is
/// the deployment's config alone (Paul, 2026-10-22). GLM-5.3 Flash's row
/// stays, at the ceiling its slots have named since 2026-10-15 ($0.15 in,
/// cached and written too, $0.50 out per million, with OpenRouter's fee):
/// usage queued under those tiers (a model call's charge on its queue, an
/// AI step's in its fragment's outbox) is priced by the release that
/// takes it, which may run the tiers on another model by then. Any other
/// model swapped out is not kept so: its queued usage charges nothing and
/// logs `ledger.unpriced` (decision 62: billing is best effort).
///
/// The Clef family's (decisions: crate::decide) from Workers AI's pricing
/// page and each model's own (developers.cloudflare.com/workers-ai/models/
/// clef-flash, clef and clef-omni, read 2026-10-14): input tokens only,
/// media counted as input, no output charge and no cache, so a cached or
/// written token is an input token's price.
///
/// The video model's (crate::media::VIDEO_MODEL), from its catalog entry
/// (developers.cloudflare.com/ai/models/alibaba/hh1-t2v, read 2026-10-31):
/// $0.14 a second at 720P and $0.28 at 1080P, through Unified Billing as
/// the gateway's credits pay, so with the credits fee. A row per
/// resolution (`media::Resolution::priced_as`), its output one millisecond
/// of video, priced per million: $140 and $280.
pub const DEFAULT_MODELS: [(&str, TokenPrices); 12] = [
    // $0.15 in, $0.03 cached, $0.50 out per million tokens (the cheap tier's, and vision)
    ("@cf/zai-org/glm-5.3-flash", TokenPrices { input: 150_000, cached_input: 30_000, cache_write: 150_000, output: 500_000 }),
    // Finite Private's (Finite's tiers' by config), at Workers AI's GLM-5.3 Flash's
    ("glm-5-3-flash", TokenPrices { input: 150_000, cached_input: 30_000, cache_write: 150_000, output: 500_000 }),
    // OpenRouter's (Finite's tiers' from 2026-10-15), for its usage queued
    // past a switch: its ceiling, $0.15 in (cached and written too) and
    // $0.50 out, with OpenRouter's 5.5% for the book's 5%
    (crate::models::OPENROUTER_FLASH, TokenPrices { input: openrouter(150_000), cached_input: openrouter(150_000), cache_write: openrouter(150_000), output: openrouter(500_000) }),
    // $1.40 in, $0.26 cached, $4.40 out
    ("@cf/zai-org/glm-5.3", TokenPrices { input: 1_400_000, cached_input: 260_000, cache_write: 1_400_000, output: 4_400_000 }),
    // $4 in, $0.20 cache read, $5 cache write, $20 out (the high tier, off for now)
    ("anthropic/claude-opus-5.5", TokenPrices { input: 4_000_000, cached_input: 200_000, cache_write: 5_000_000, output: 20_000_000 }),
    // $0.44 in, $0.014 cached, $1.32 out (the fallback)
    ("@cf/deepseek-ai/deepseek-v4-flash-0731", TokenPrices { input: 440_000, cached_input: 14_000, cache_write: 440_000, output: 1_320_000 }),
    // Clef-flash, $0.038 in; Clef, $0.24; Clef-omni, $0.15; nothing out
    clef(Model::ClefFlash),
    clef(Model::Clef),
    clef(Model::ClefOmni),
    // BGE-M3's embeddings (the route's embeddings slot unnamed, crate::embed),
    // $0.012 a million input tokens, nothing out
    (crate::embed::MODEL_DEFAULT, TokenPrices { input: 12_000, cached_input: 12_000, cache_write: 12_000, output: 0 }),
    // HappyHorse 1.0's videos: a millisecond its output, $0.14 a second at 720P, $0.28 at 1080P
    ("alibaba/hh1-t2v@720P", TokenPrices { input: 0, cached_input: 0, cache_write: 0, output: 140_000_000 }),
    ("alibaba/hh1-t2v@1080P", TokenPrices { input: 0, cached_input: 0, cache_write: 0, output: 280_000_000 }),
];

/// OpenRouter's fee on the credits that pay for its calls: 5.5% of a card
/// purchase, at least $0.80 (5% in crypto); it marks up no provider's
/// price (openrouter.ai/docs/faq, read 2026-10-15).
pub const OPENROUTER_FEE_BP: u32 = 550;

/// A price OpenRouter bills at `list` as a row of the book: grossed up so
/// that the row with the book's credits fee (AI Gateway's) is at least
/// `list` with OpenRouter's, rounded up once.
pub const fn openrouter(list: i64) -> i64 {
    let n = list as i128 * (BP + OPENROUTER_FEE_BP) as i128;
    let d = (BP + CREDITS_FEE_BP_DEFAULT) as i128;
    ((n + d - 1) / d) as i64
}
const _: () = assert!(OPENROUTER_FEE_BP >= CREDITS_FEE_BP_DEFAULT, "a gross-up never lowers a price");

/// A Clef model's row: its input price for every input token, none out.
const fn clef(m: Model) -> (&'static str, TokenPrices) {
    let input = m.input_price();
    (m.id(), TokenPrices { input, cached_input: input, cache_write: input, output: 0 })
}
/// The default book's version, informational since a ledger takes any book
/// whose contents differ from its own (docs/ledger.md; #451). Before, a
/// ledger took only a newer version, so each change to the defaults
/// raised it, and each still does: 13 for Firecrawl's credits by the call
/// (2026-11-07), after 12 for Google Places' details, photos
/// and autocomplete apart from its searches (2026-11-07), after 11 for
/// xAI's chat and images apart
/// from its X searches (2026-11-07), after 10 for fal's images by their
/// pixels (2026-11-07), after 9 for ElevenLabs' speech by its characters
/// (2026-11-07), after 8 for HappyHorse's videos (2026-10-31),
/// after 7 for OpenRouter's GLM-5.3 Flash (2026-10-15), after 6 for the Clef family (2026-10-14), 5
/// for Finite Private's GLM-5.3 Flash (2026-10-14), 4 for Hermes' tool
/// keys (2026-10-09) and 3 for
/// DeepSeek V4 Flash, past the 2 that finite.place's preview configs
/// already carried. A deployment's `price_book_version` raises it further (its
/// operator keys' prices); the book is the higher of the two.
pub const DEFAULT_BOOK_VERSION: u32 = 13;
/// Workers AI: $0.011 per thousand neurons (spike S4: neurons × $0.000011
/// matched tokens × the catalog price on every call). Images are priced
/// in neurons too (crate::media).
pub const DEFAULT_NEURONS: i64 = 11_000;
/// The default computer (decision 13: 2 vCPU, 6 GiB; a 12 GB disk
/// assumed, where it runs on `DISK_MB`: the 8 GB more, $0.002016 an hour,
/// is not charged yet), from Containers' pricing (Workers Paid, 2026-08-28):
/// - memory, provisioned: 6 GiB × $0.0000025 a GiB-second = $0.054 an hour;
/// - disk, provisioned: 12 GB × $0.00000007 a GB-second = $0.003024 an hour;
/// - CPU, billed on active use only, which the Computer DO cannot see:
///   5% of 2 vCPU (an idle guest) × $0.000020 a vCPU-second = $0.0072 an hour.
///
/// $0.064224 an awake hour, about $46 for an always-on month.
pub const DEFAULT_INSTANCES: [(&str, i64); 1] = [(INSTANCE, 64_224)];
/// The instance every computer runs on: its container starts at its size
/// (`instance_size`), and its awake time is priced by the book's row for it.
pub const INSTANCE: &str = "2vcpu-6gib";

/// The size a computer's container starts at (`ctx.container.start`'s
/// `instance`): one of Containers' named types, or a size of its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum InstanceSize {
    Named(String),
    #[serde(rename_all = "camelCase")]
    Custom { vcpu: u32, memory_mib: u32, disk_mb: u32 },
}

/// Containers' named instance types (developers.cloudflare.com/containers/platform/limits).
const NAMED_INSTANCES: [&str; 6] = ["lite", "basic", "standard-1", "standard-2", "standard-3", "standard-4"];

/// The disk of every custom instance: Containers' most for one (its limits
/// page, "Maximum Disk 20 GB"), whatever its memory ("Custom Container
/// instance types no longer have a disk to memory ratio limit", changelog
/// 2026-09-29). The image shares it (the Hermes image is 5.9 GB), and a
/// start restores `/data` onto what is left with 256 MiB to spare (the
/// Sandbox SDK's floor), so a computer whose `/data` filled a 12 GB disk
/// could never start again (2026-11-03). The price still counts 12 GB
/// (`DEFAULT_INSTANCES`; docs/technical-debt-ledger.md).
pub const DISK_MB: u32 = 20_000;

/// The size an instance of the price book names: a named type as it is,
/// or `<n>vcpu-<m>gib` (the book's own names, decision 13's `2vcpu-6gib`)
/// as a custom size with a disk of `DISK_MB`, within Containers' bounds for
/// one: 1 to 4 vCPU, at most 12 GiB, at least 3 GiB a vCPU.
pub fn instance_size(name: &str) -> Result<InstanceSize, String> {
    if NAMED_INSTANCES.contains(&name) {
        return Ok(InstanceSize::Named(name.to_string()));
    }
    let custom = name.strip_suffix("gib").and_then(|n| n.split_once("vcpu-")).and_then(|(v, m)| Some((v.parse::<u32>().ok()?, m.parse::<u32>().ok()?)));
    let Some((vcpu, gib)) = custom else {
        return Err(format!("an instance is one of {} or <n>vcpu-<m>gib, not {name:?}", NAMED_INSTANCES.join(", ")));
    };
    if !(1..=4).contains(&vcpu) || gib > 12 || gib < 3 * vcpu {
        return Err(format!("{name}: a custom instance has 1 to 4 vCPU and at least 3 GiB a vCPU, at most 12 GiB"));
    }
    Ok(InstanceSize::Custom { vcpu, memory_mib: gib * 1024, disk_mb: DISK_MB })
}
/// R2 Standard $0.015, Durable Objects' SQL storage $0.20 per GB-month.
/// code.storage publishes no price to us: git is at R2's until the
/// operator sets its own.
pub const DEFAULT_STORAGE: StoragePrices = StoragePrices { r2: 15_000, sqlite: 200_000, git: 15_000 };
/// Workers Standard $0.30 per million requests, plus Durable Objects'
/// $0.15 per million for the request each one makes of a fragment.
pub const DEFAULT_REQUESTS: i64 = 450_000;
/// Dynamic Workers: $0.002 per unique dynamic worker per day.
pub const DEFAULT_DYNAMIC_WORKERS: i64 = 2_000;
/// Browser Rendering (now "Browser Run"): $0.09 per browser hour on
/// Workers Paid, past the 10 hours a month the plan includes. Source:
/// developers.cloudflare.com/browser-run/pricing (last updated 2026-04-21,
/// read 2026-10-03); its time is totalled per day in seconds, and the
/// month's total rounded to the nearest hour. A card's shot is a Browser
/// Session (CDP over the binding: crate::card), which also bills
/// concurrent browsers, $2.00 each a month past 10 (the month's average of
/// each day's peak): a deployment-wide charge that no one shot causes, so
/// it is not metered per person.
pub const DEFAULT_BROWSER: i64 = 90_000;
/// Cloudflare Images: $0.50 per thousand unique transformations.
pub const DEFAULT_IMAGES: i64 = 500_000;

/// The operator keys' list prices per call (decision 37; Paul, 2026-10-04:
/// Perplexity, Google Places, xAI, ElevenLabs), `(name, micros, per
/// calls)`, read from each vendor's own pricing page on 2026-10-03. A
/// catalog row of the operator kind takes its name's price here unless it
/// names its own (`fragment_core::catalog`). The swap meters a call, not
/// what the vendor counts (tokens, posts, minutes), so a vendor that bills
/// by those is priced at a typical call of the managed skill that uses it,
/// and the estimate says what it assumes. A part of a key's calls priced
/// apart (`KEY_PARTS`) is listed here too, by its part's name, per that
/// part's unit.
pub const DEFAULT_KEYS: [(&str, i64, u64); 16] = [
    // X API application-bearer reads, separate from Grok's xai key:
    // $0.005 per Post and $0.010 per User resource. The x-api-finite
    // helper's default recent search fetches 10 posts: $0.05 assumed
    // per request, excluding expanded users and vendor deduplication.
    // https://docs.x.com/x-api/getting-started/pricing (2026-10-09).
    // Variable resources per request are recorded in the debt ledger.
    ("x", 50_000, 1),
    // Firecrawl search: 2 credits per 10 results, Hobby PAYG $5 per
    // 1,000 credits. One search is $0.01; a basic scrape is $0.005.
    // https://www.firecrawl.dev/pricing (2026-10-09). A row an older
    // release queued is priced so; this release's are below.
    ("firecrawl", 10_000, 1),
    // A Firecrawl credit (`KEY_PARTS`), Hobby's pay-as-you-go 1,000 for
    // $5: a scrape (Hermes' `web_extract`) 1 a page, 4 more for each LLM
    // format (JSON, a question, highlights, audio, video); a search
    // (Hermes' `web_search`) 2 per 10 results it asks for, and a scrape
    // each if it scrapes them; a map 1. A crawl or a batch scrape is
    // billed as each page completes, so is charged nothing here.
    // Sources: docs.firecrawl.dev/billing, firecrawl.dev/pricing and
    // docs.firecrawl.dev/api-reference/endpoint/search (read 2026-11-07).
    ("firecrawl:credit", 5_000, 1),
    // FAL FLUX.2 Klein 9B: $0.006 per megapixel, a 1 MP image assumed.
    // https://fal.ai/models/fal-ai/flux-2/klein/9b (2026-10-09). A row an
    // older release queued is priced so; this release's are below.
    ("fal", 6_000, 1),
    // A fal generation (`KEY_PARTS`; Hermes' `image_generate`), per pixel
    // its request asks for, at FLUX.2 Klein 9B's "$0.006 per megapixel"
    // whatever the model, a megapixel taken as 1,048,576 pixels, the
    // larger reading, as fal says neither. Source:
    // fal.ai/models/fal-ai/flux-2/klein/9b (read 2026-11-07). Hermes'
    // default, one 1024 × 576 `landscape_16_9` image, is $0.0034. Its
    // queue's polls, statuses and results and its uploads are free: fal
    // prices "per output" (fal.ai/pricing, read 2026-11-07).
    ("fal:image", 6_000, 1_048_576),
    // Browser Use: $0.02 per browser-hour, $5/GB residential proxy.
    // https://browser-use.com/pricing (2026-10-09). A control request is
    // estimated as a 5-minute session, excluding proxy traffic, rounded
    // up to a micro-dollar; create and stop are separate metered calls.
    ("browser-use", 20_000, 12),
    // Perplexity's Search API (`POST /search`, Hermes' own web search):
    // $5.00 per 1,000 requests. Source:
    // docs.perplexity.ai/getting-started/pricing. A Sonar Pro brief
    // (`/v1/sonar`) also bills tokens ($3 in, $15 out per million) and a
    // request fee of $6 to $14 per 1,000: it is metered at the search's
    // price, under what it costs (docs/technical-debt-ledger.md).
    ("perplexity", 5_000_000, 1_000),
    // Google Places API (New), Text Search Enterprise: $35.00 per 1,000
    // calls (0 to 100,000 a month). The goplaces skill's field masks name
    // rating, userRatingCount, websiteUri, nationalPhoneNumber, priceLevel
    // and regularOpeningHours, Enterprise fields; Place Details Enterprise
    // is $20.00, so a call is priced at the dearer of its two. Source:
    // developers.google.com/maps/billing-and-pricing/pricing (last updated
    // 2026-09-28), the free monthly caps not counted. Since FIN-445 only a
    // text or nearby search (`places:searchText`, `places:searchNearby`,
    // Enterprise $35.00 too) is priced so (`KEY_PARTS`); the key's other
    // calls are below, or free.
    ("google-places", 35_000_000, 1_000),
    // Place Details Enterprise (`GET /v1/places/<id>`, the goplaces
    // skill's field mask): $20.00 per 1,000. Source as above (read
    // 2026-11-07).
    ("google-places:details", 20_000_000, 1_000),
    // Place Photos (`GET /v1/places/<id>/photos/<photo>/media`): $7.00 per
    // 1,000. Source as above (read 2026-11-07).
    ("google-places:photo", 7_000_000, 1_000),
    // Autocomplete Requests (`POST /v1/places:autocomplete`): $2.83 per
    // 1,000 (a session's usage is free). Source as above (read
    // 2026-11-07).
    ("google-places:autocomplete", 2_830_000, 1_000),
    // xAI's X Search (Hermes' native `x_search`: `/v1/responses` with the
    // `x_search` tool): $5 per 1,000 posts fetched and $10 per 1,000 user
    // profiles, plus tokens. Source: docs.x.ai/developers/tools/x-search
    // (prices in effect since 2026-09-21) and docs.x.ai/docs/models
    // (grok-4.3: $1.25 in, $2.50 out per million). A call is taken as 20
    // posts ($0.10) and 10,000 tokens in and 1,000 out ($0.015): $0.115,
    // priced at $0.12. Only a call with the `x_search` tool is priced so
    // (`KEY_PARTS`); the key's other calls are below, or free.
    ("xai", 120_000, 1),
    // A Grok chat call on the key without X Search (`/v1/chat/completions`,
    // `/v1/responses`, `/v1/messages`), taken as the X Search call's own
    // tokens, 10,000 in and 1,000 out on grok-4.3: $0.015. Source:
    // docs.x.ai/developers/pricing (read 2026-11-07).
    ("xai:chat", 15_000, 1),
    // A Grok image (`/v1/images/generations`), per image its request asks
    // for (`n`): grok-imagine-image's $0.02 ($0.04 on
    // grok-imagine-image-2.0, charged under its list). Source:
    // docs.x.ai/developers/pricing (read 2026-11-07).
    ("xai:image", 20_000, 1),
    // ElevenLabs' Music API (`/v1/music`, the music-generation skill):
    // $0.15 per minute of music generated. Source: elevenlabs.io/pricing/api
    // (read 2026-11-07). A call is taken as a minute; the skill's jingles
    // are 15 s, its longest compositions 5 minutes. Every call on the key
    // but text to speech (below) is priced so.
    ("elevenlabs", 150_000, 1),
    // ElevenLabs' Text to Speech (`/v1/text-to-speech/…`, Hermes'
    // `text_to_speech`), per character of its request's `text`
    // (`KEY_PARTS`): $0.08 per 1,000 characters on the multilingual
    // models (Hermes' default, `eleven_multilingual_v2`), $0.04 on Flash
    // and Turbo, billed per character. Source: elevenlabs.io/pricing/api
    // (read 2026-11-07). A 200-character reply is $0.016.
    ("elevenlabs:speech", 80_000, 1_000),
];

/// A part of an operator key's calls priced apart from its others, by what
/// its vendor counts (`key_usage` says which calls are the part, and what
/// each uses), as `priced_as`, its price's name in `DEFAULT_KEYS`. Its name
/// has a `:`, which no catalog row's has, so it is never a provider's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyPart {
    pub key: &'static str,
    pub priced_as: &'static str,
    /// The most units one call is charged: its vendor's own limit for one
    /// request, so even a call it answered is never charged for more than
    /// it would take.
    pub units_max: u64,
}

/// The parts the book prices apart (FIN-443 and FIN-445, 2026-11-07):
///
/// - ElevenLabs' text to speech, its streaming and `with-timestamps` calls
///   too, was charged as a minute of music, 10 to 20 times what a short
///   clip costs. A call takes at most 40,000 characters (Flash's limit;
///   the multilingual models take 10,000 and v3 5,000:
///   elevenlabs.io/pricing/api, read 2026-11-07), so one is charged at
///   most $3.20 at list.
/// - fal's generations were charged as an image, and so was each of the
///   SDK's queue polls, its status checks and its result: an image cost
///   several images. A call is charged at most 16 megapixels, $0.096 at
///   list: Klein makes at most 4 images a call (`num_images`), and its
///   largest preset is 1 megapixel.
/// - xAI's chat and image calls were charged as an X search, $0.18 a call:
///   a plain Grok chat call about twelve times its typical cost. A call
///   is charged at most 10 images (`n`, xAI's most a request).
/// - Google Places' details, photos and autocomplete were charged as an
///   Enterprise text search, $35.00 per 1,000: a detail 1.75 times its
///   list, a photo 5 times and an autocomplete 12 times.
/// - Firecrawl's calls were charged as a search, 2 credits: a scrape, 1,
///   twice its list, and each crawl or batch's status poll a search. A
///   call is charged at most 1,000 credits ($5 at list).
pub const KEY_PARTS: [KeyPart; 8] = [
    KeyPart { key: "elevenlabs", priced_as: "elevenlabs:speech", units_max: 40_000 },
    KeyPart { key: "fal", priced_as: "fal:image", units_max: 16 * 1_048_576 },
    KeyPart { key: "xai", priced_as: "xai:chat", units_max: 1 },
    KeyPart { key: "xai", priced_as: "xai:image", units_max: 10 },
    KeyPart { key: "google-places", priced_as: "google-places:details", units_max: 1 },
    KeyPart { key: "google-places", priced_as: "google-places:photo", units_max: 1 },
    KeyPart { key: "google-places", priced_as: "google-places:autocomplete", units_max: 1 },
    KeyPart { key: "firecrawl", priced_as: "firecrawl:credit", units_max: 1_000 },
];

/// The list price of operator key `name` per call, when the price book
/// has one: `(micros, per calls)`.
pub fn default_key_price(name: &str) -> Option<(i64, u64)> {
    DEFAULT_KEYS.iter().find(|(n, _, _)| *n == name).map(|(_, micros, per)| (*micros, *per))
}

/// The largest body whose characters a part's call is read for: ElevenLabs
/// takes at most 40,000 characters a call (Flash's limit), 240,000 bytes
/// even with every one a JSON escape (`\uXXXX`), so a body past this is
/// one its vendor refuses, and is not parsed (the swap's own limit is 32
/// MiB).
pub const KEY_PART_BODY_MAX_BYTES: usize = 1024 * 1024;

/// One call on an operator key as the swap meters it, once answered: its
/// method, host (lowercase), path and body as the guest sent them, and its
/// answer's status (under 500: the swap charges no call its provider
/// failed).
#[derive(Debug, Clone, Copy)]
pub struct KeyCall<'a> {
    pub method: &'a str,
    pub host: &'a str,
    pub path: &'a str,
    pub body: &'a [u8],
    pub status: u16,
}

impl KeyCall<'_> {
    /// Its body's JSON object, when it is one to read.
    fn said(&self) -> Option<serde_json::Map<String, serde_json::Value>> {
        match (self.body.len() <= KEY_PART_BODY_MAX_BYTES).then(|| serde_json::from_slice(self.body).ok()).flatten() {
            Some(serde_json::Value::Object(said)) => Some(said),
            _ => None,
        }
    }
}

/// fal's hosts that run a model (`queue.fal.run`'s queue and `fal.run`'s
/// synchronous runs); its others (`rest.fal.ai`'s uploads) make nothing.
const FAL_RUNS: [&str; 2] = ["queue.fal.run", "fal.run"];

/// FLUX.2 Klein 9B's text to image (Hermes' `image_generate`), whose
/// `image_size` absent is its default, `landscape_4_3` (an edit's is its
/// input image's, which the meter does not see).
const FAL_KLEIN: &str = "/fal-ai/flux-2/klein/9b";

/// fal's `image_size` presets in pixels (FLUX.2 Klein's, and its other
/// image models'), the first Klein's default.
const FAL_SIZES: [(&str, u64); 6] = [
    ("landscape_4_3", 1024 * 768),
    ("landscape_16_9", 1024 * 576),
    ("square_hd", 1024 * 1024),
    ("square", 512 * 512),
    ("portrait_4_3", 768 * 1024),
    ("portrait_16_9", 576 * 1024),
];

/// The pixels a fal generation to `path` asks for: its `num_images`
/// (absent, one) images of its `image_size`, a preset or `{width,
/// height}`, or absent, Klein's default on Klein's text to image; none
/// for any other (an absent size elsewhere, an edit's being its input's,
/// an unknown preset, a size that is no two whole numbers).
fn fal_pixels(path: &str, said: &serde_json::Map<String, serde_json::Value>) -> Option<u64> {
    let each = match said.get("image_size") {
        None if path == FAL_KLEIN => FAL_SIZES[0].1,
        None => return None,
        Some(serde_json::Value::String(preset)) => FAL_SIZES.iter().find(|(name, _)| name == preset)?.1,
        Some(size) => size.get("width")?.as_u64()?.checked_mul(size.get("height")?.as_u64()?)?,
    };
    let images = said.get("num_images").map_or(Some(1), serde_json::Value::as_u64)?;
    each.checked_mul(images)
}

/// Firecrawl's formats that cost 4 credits a page more than a scrape's 1:
/// an LLM's reading (docs.firecrawl.dev/billing, read 2026-11-07).
/// Each is its kind (its bit), charged once a page however often it is
/// named: JSON and v1's `extract` are one, a question and a query one.
const FIRECRAWL_LLM_FORMATS: [(&str, u8); 7] = [("json", 0), ("extract", 0), ("question", 1), ("query", 1), ("highlights", 2), ("audio", 3), ("video", 4)];

/// Firecrawl's sources that are free, and make no page to scrape: its
/// tool catalogues (docs.firecrawl.dev/api-reference/endpoint/search).
const FIRECRAWL_FREE_SOURCES: [&str; 2] = ["alexandria", "skills"];

/// The credits a page scraped with `formats` costs (strings, or objects
/// naming their `type`; absent, markdown alone): 1, and 4 for each kind
/// of LLM format it names.
fn firecrawl_page(formats: Option<&serde_json::Value>) -> u64 {
    let kinds = formats.and_then(serde_json::Value::as_array).map_or(0u8, |all| {
        all.iter()
            .filter_map(|f| f.as_str().or_else(|| f.get("type")?.as_str()))
            .filter_map(|name| FIRECRAWL_LLM_FORMATS.iter().find(|(llm, _)| *llm == name))
            .fold(0, |kinds, (_, kind)| kinds | 1 << kind)
    });
    1 + 4 * u64::from(kinds.count_ones())
}

/// The credits a Firecrawl search asks for: 2 per 10 results (`limit`,
/// absent `limit_default`, a source each of the kinds its `sources` name
/// but the free ones, `FIRECRAWL_FREE_SOURCES`), and a page each when
/// `scrapeOptions` names some `formats` (none, it scrapes nothing). None for a `limit` or
/// `sources` that is no count or list of kinds.
fn firecrawl_search(said: &serde_json::Map<String, serde_json::Value>, limit_default: u64) -> Option<u64> {
    let limit = said.get("limit").map_or(Some(limit_default), serde_json::Value::as_u64)?;
    let sources = match said.get("sources") {
        None => 1,
        Some(all) => {
            let mut kinds: Vec<&str> = all.as_array()?.iter().map(|s| s.as_str().or_else(|| s.get("type")?.as_str())).collect::<Option<_>>()?;
            kinds.retain(|kind| !FIRECRAWL_FREE_SOURCES.contains(kind));
            kinds.sort_unstable();
            kinds.dedup();
            kinds.len() as u64
        }
    };
    let results = limit.checked_mul(sources)?;
    let formats = said.get("scrapeOptions").and_then(|o| o.get("formats")).filter(|f| f.as_array().is_some_and(|all| !all.is_empty()));
    let scraped = formats.map_or(0, |formats| firecrawl_page(Some(formats)));
    Some((2 * results.div_ceil(10)).saturating_add(results.saturating_mul(scraped)))
}

/// What one call on operator key `key` uses, as the swap meters it: a
/// part's call (`KEY_PARTS`) its units, at most its `units_max`, as the
/// part; a call its vendor does not bill (fal's queue polls, statuses,
/// results and cancels, its uploads), or one the book does not price
/// apart on a key priced by its parts (xAI's), none of the key's; any
/// other one call of the key.
///
/// - ElevenLabs' text to speech (`/v1/text-to-speech/…`): the characters
///   of its JSON body's `text`.
/// - A fal generation (a `POST` to a model at `queue.fal.run` or
///   `fal.run`, not under its queue's `…/requests/…`): the pixels it asks
///   for (`fal_pixels`).
/// - xAI: a `POST` to `/v1/responses` or `/v1/chat/completions` whose
///   `tools` name `x_search` is one call of the key (an X search); one
///   without, or to `/v1/messages`, one `xai:chat`; one to
///   `/v1/images/generations` its `n` images (absent, 1).
/// - Google Places (New): a text or nearby search is one call of the key;
///   a place's details (`/v1/places/<id>`), its photo
///   (`…/photos/<photo>/media`, made by its redirect to the image too) or
///   an autocomplete one of its part; any other call none.
/// - Firecrawl (v1 and v2): a scrape, a search (`firecrawl_search`, its
///   `limit` 5 on v1 and 10 on v2 when absent) and a map their credits,
///   as `firecrawl:credit`; any other call (a crawl, a batch scrape or an
///   extract, billed as each page completes, a status poll, free) none.
///
/// A part's call uses none, so is charged nothing, when its vendor
/// refused it (any answer but a 2xx: a bad request, a missing voice, a
/// rate limit, which made nothing) or its units cannot be read (a body
/// that is no JSON object saying them, one past
/// `KEY_PART_BODY_MAX_BYTES`, or none, as a socket's upgrade): billing is
/// best effort, and a gap charges zero rather than a guess above what the
/// call cost (decision 62).
pub fn key_usage(key: &str, call: &KeyCall) -> Usage {
    let (priced_as, units) = match key {
        "elevenlabs" if call.path.starts_with("/v1/text-to-speech/") => ("elevenlabs:speech", call.said().and_then(|said| Some(said.get("text")?.as_str()?.chars().count() as u64))),
        "fal" if call.method == "POST" && FAL_RUNS.contains(&call.host) && !call.path.contains("/requests/") => ("fal:image", call.said().and_then(|said| fal_pixels(call.path, &said))),
        "fal" => return Usage::Key { key: key.to_string(), units: 0 },
        "xai" => {
            let said = call.said();
            let searches = said.as_ref().and_then(|s| s.get("tools")?.as_array()).is_some_and(|tools| tools.iter().any(|t| t.get("type").and_then(serde_json::Value::as_str) == Some("x_search")));
            match (call.method, call.path) {
                ("POST", "/v1/responses" | "/v1/chat/completions") if searches => ("xai", Some(1)),
                // one call, read or not (a long context past the body read)
                ("POST", "/v1/responses" | "/v1/chat/completions" | "/v1/messages") => ("xai:chat", Some(1)),
                ("POST", "/v1/images/generations") => ("xai:image", said.and_then(|s| s.get("n").map_or(Some(1), serde_json::Value::as_u64))),
                // a video, a stored response read, a model list: priced by none
                _ => return Usage::Key { key: key.to_string(), units: 0 },
            }
        }
        // the method is the path's (Google refuses any other), so not read
        "google-places" => match (call.path, call.path.strip_prefix("/v1/places/")) {
            ("/v1/places:searchText" | "/v1/places:searchNearby", _) => ("google-places", Some(1)),
            ("/v1/places:autocomplete", _) => ("google-places:autocomplete", Some(1)),
            (_, Some(place)) if !place.is_empty() && !place.contains('/') => ("google-places:details", Some(1)),
            (_, Some(photo)) if photo.contains("/photos/") && photo.ends_with("/media") => ("google-places:photo", Some(1)),
            _ => return Usage::Key { key: key.to_string(), units: 0 },
        },
        "firecrawl" => {
            let said = call.said();
            let what = call.path.strip_prefix("/v1/").or_else(|| call.path.strip_prefix("/v2/"));
            let credits = match (call.method, what) {
                // a page a call, read or not
                ("POST", Some("scrape")) => Some(said.as_ref().map_or(1, |s| firecrawl_page(s.get("formats")))),
                ("POST", Some("search")) => said.as_ref().and_then(|s| firecrawl_search(s, if call.path.starts_with("/v1/") { 5 } else { 10 })),
                ("POST", Some("map")) => Some(1),
                _ => return Usage::Key { key: key.to_string(), units: 0 },
            };
            ("firecrawl:credit", credits)
        }
        _ => return Usage::Key { key: key.to_string(), units: 1 },
    };
    // a part's, or one of its key's own (xAI's X search), made by a 2xx
    let most = KEY_PARTS.iter().find(|p| p.priced_as == priced_as).map_or(1, |p| p.units_max);
    // a Places photo's answer is a redirect to it unless it asks for JSON
    // (`skipHttpRedirect`), and the swap never follows one
    let redirected = priced_as == "google-places:photo" && matches!(call.status, 301 | 302 | 303 | 307 | 308);
    let made = (200..300).contains(&call.status) || redirected;
    Usage::Key { key: priced_as.to_string(), units: units.filter(|_| made).map_or(0, |u| u.min(most)) }
}

/// Why a book is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BookFault {
    /// A margin over `MARGIN_BP_MAX`, or the credits' fee over `FEE_BP_MAX`.
    Rate,
    /// A price below zero or over `PRICE_MAX`, or a key's `per` outside 1
    /// to `QUANTITY_MAX`.
    Price,
    /// A name that is not `printable`, or named twice in one list.
    Name,
    /// A list longer than `PRICES_MAX`.
    TooMany,
}

/// Why a usage has no charge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriceError {
    Invalid(UsageFault),
    /// The book prices no such model, instance type or key.
    NoPrice,
    /// The charge is over `CHARGE_MAX`.
    TooLarge,
}

/// A usage's money, each figure rounded up once from the exact value:
/// Cloudflare's list price (what the gateway's log reports, for
/// reconciliation), the cost basis (list plus the credits fee), and the
/// charge (cost basis plus the margin).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Priced {
    pub list: i64,
    pub cost: i64,
    pub charge: i64,
}

impl PriceBook {
    /// The default book (`DEFAULT_BOOK_VERSION`): the consts above, the
    /// default fee and margin, and no operator keys (decision 37 names
    /// services, not prices; the operator adds each key with its price).
    pub fn defaults() -> PriceBook {
        PriceBook {
            version: DEFAULT_BOOK_VERSION,
            margin_bp: MARGIN_BP_DEFAULT,
            credits_fee_bp: CREDITS_FEE_BP_DEFAULT,
            models: DEFAULT_MODELS.iter().map(|(model, price)| ModelPrice { model: model.to_string(), price: *price }).collect(),
            neurons: DEFAULT_NEURONS,
            instances: DEFAULT_INSTANCES.iter().map(|(instance, hour)| InstancePrice { instance: instance.to_string(), hour: *hour }).collect(),
            storage: DEFAULT_STORAGE,
            requests: DEFAULT_REQUESTS,
            dynamic_workers: DEFAULT_DYNAMIC_WORKERS,
            browser: DEFAULT_BROWSER,
            images: DEFAULT_IMAGES,
            keys: Vec::new(),
        }
    }

    /// SHA-256 of the book, as hex: the id a ledger keeps it under once it
    /// takes it (`book:<digest>`).
    pub fn digest(&self) -> String {
        hex::encode(Sha256::digest(serde_json::to_vec(self).expect("a book serializes")))
    }

    /// Whether the book is one a ledger may charge with.
    pub fn validate(&self) -> Result<(), BookFault> {
        if self.margin_bp > MARGIN_BP_MAX || self.credits_fee_bp > FEE_BP_MAX {
            return Err(BookFault::Rate);
        }
        if self.models.len() > PRICES_MAX || self.instances.len() > PRICES_MAX || self.keys.len() > PRICES_MAX {
            return Err(BookFault::TooMany);
        }
        let fixed = [self.neurons, self.storage.r2, self.storage.sqlite, self.storage.git, self.requests, self.dynamic_workers, self.browser, self.images];
        let listed = self.models.iter().flat_map(|m| [m.price.input, m.price.cached_input, m.price.cache_write, m.price.output]);
        let listed = listed.chain(self.instances.iter().map(|i| i.hour)).chain(self.keys.iter().map(|k| k.micros));
        if !fixed.into_iter().chain(listed).all(|p| (0..=PRICE_MAX).contains(&p)) {
            return Err(BookFault::Price);
        }
        if !self.keys.iter().all(|k| (1..=QUANTITY_MAX).contains(&k.per)) {
            return Err(BookFault::Price);
        }
        let names_ok = |names: Vec<&str>| {
            let mut sorted = names.clone();
            sorted.sort_unstable();
            sorted.dedup();
            sorted.len() == names.len() && names.iter().all(|n| printable(n, NAME_MAX_BYTES))
        };
        let models = self.models.iter().map(|m| m.model.as_str()).collect();
        let instances = self.instances.iter().map(|i| i.instance.as_str()).collect();
        let keys = self.keys.iter().map(|k| k.key.as_str()).collect();
        if names_ok(models) && names_ok(instances) && names_ok(keys) {
            Ok(())
        } else {
            Err(BookFault::Name)
        }
    }

    /// What an operator key's calls are charged under this book, as the
    /// ledger charges them: `(micros, per)`, micro-dollars for `per` calls
    /// (its price's own count), its list price with the margin, rounded up.
    /// None for a key the book prices not, or a charge it refuses.
    pub fn key_charge(&self, key: &str) -> Option<(i64, u64)> {
        let k = self.keys.iter().find(|k| k.key == key)?;
        let priced = self.price(&Usage::Key { key: k.key.clone(), units: k.per }).ok()?;
        Some((priced.charge, k.per))
    }

    /// What `usage` costs and is charged under this book.
    pub fn price(&self, usage: &Usage) -> Result<Priced, PriceError> {
        usage.validate().map_err(PriceError::Invalid)?;
        let ai = self.credits_fee_bp;
        let (numerator, per, fee_bp) = match usage {
            Usage::Tokens { model, input, cached_input, cache_write, output } => {
                let p = self.models.iter().find(|m| m.model == *model).ok_or(PriceError::NoPrice)?.price;
                let n = term(*input, p.input) + term(*cached_input, p.cached_input) + term(*cache_write, p.cache_write) + term(*output, p.output);
                (n, TOKENS_PER, ai)
            }
            Usage::Neurons { milli } => (term(*milli, self.neurons), MILLI_NEURONS_PER, ai),
            Usage::Awake { instance, ms } => {
                let hour = self.instances.iter().find(|i| i.instance == *instance).ok_or(PriceError::NoPrice)?.hour;
                (term(*ms, hour), AWAKE_MS_PER, 0)
            }
            Usage::Storage { class, byte_hours } => (term(*byte_hours, self.storage.of(*class)), STORAGE_BYTE_HOURS_PER, 0),
            Usage::Requests { count } => (term(*count, self.requests), REQUESTS_PER, 0),
            Usage::DynamicWorkers { count } => (term(*count, self.dynamic_workers), 1, 0),
            Usage::Browser { ms } => (term(*ms, self.browser), BROWSER_MS_PER, 0),
            Usage::Images { count } => (term(*count, self.images), IMAGES_PER, 0),
            Usage::Key { key, units } => {
                let k = self.keys.iter().find(|k| k.key == *key).ok_or(PriceError::NoPrice)?;
                (term(*units, k.micros), k.per, 0)
            }
        };
        priced(numerator, per, fee_bp, self.margin_bp)
    }
}

/// One part of a usage's exact list price, in micro-dollars × its unit's
/// count.
fn term(quantity: u64, micros: i64) -> i128 {
    assert!(quantity <= QUANTITY_MAX, "a usage is validated before it is priced");
    assert!((0..=PRICE_MAX).contains(&micros), "a book is validated before it is kept");
    i128::from(quantity) * i128::from(micros)
}

/// `numerator / per` micro-dollars at list, then with the fee, then with
/// the margin, each rounded up from the exact value (never from a figure
/// already rounded).
fn priced(numerator: i128, per: u64, fee_bp: u32, margin_bp: u32) -> Result<Priced, PriceError> {
    assert!(numerator >= 0, "prices and quantities are never negative");
    assert!(per >= 1, "a unit's count is at least one");
    assert!(fee_bp <= FEE_BP_MAX && margin_bp <= MARGIN_BP_MAX, "a book's rates are validated before it is kept");
    let (per, bp) = (i128::from(per), i128::from(BP));
    let with_fee = numerator * (bp + i128::from(fee_bp));
    let with_margin = with_fee * (bp + i128::from(margin_bp));
    assert!(with_margin <= NUMERATOR_MAX, "QUANTITY_MAX and PRICE_MAX keep the numerator in range");
    let charge = ceil_div(with_margin, per * bp * bp);
    if charge > i128::from(CHARGE_MAX) {
        return Err(PriceError::TooLarge);
    }
    let list = ceil_div(numerator, per);
    let cost = ceil_div(with_fee, per * bp);
    assert!(list <= cost && cost <= charge, "fees and margins only add");
    // each is at most `charge`, which fits CHARGE_MAX
    Ok(Priced { list: list as i64, cost: cost as i64, charge: charge as i64 })
}

fn ceil_div(n: i128, d: i128) -> i128 {
    assert!(n >= 0 && d > 0, "a ceiling of a non-negative amount");
    (n + d - 1) / d
}

/// Micro-dollars as dollars for people: `$0.0412`, `$20.00`.
pub fn dollars(m: i64) -> String {
    let sign = if m < 0 { "-" } else { "" };
    let m = m.unsigned_abs();
    let usd = USD as u64;
    let (whole, frac) = (m / usd, m % usd);
    if frac % 10_000 == 0 {
        format!("{sign}${whole}.{:02}", frac / 10_000)
    } else {
        format!("{sign}${whole}.{:06}", frac).trim_end_matches('0').to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The book's names start the size they price; named types pass as
    /// they are; a size Containers refuses is refused here first.
    #[test]
    fn instance_sizes() {
        assert_eq!(instance_size(INSTANCE), Ok(InstanceSize::Custom { vcpu: 2, memory_mib: 6144, disk_mb: 20_000 }), "Containers' most disk, whatever the memory");
        assert_eq!(serde_json::to_value(instance_size("2vcpu-6gib").unwrap()).unwrap(), serde_json::json!({ "vcpu": 2, "memoryMib": 6144, "diskMb": 20000 }));
        assert_eq!(instance_size("1vcpu-3gib"), Ok(InstanceSize::Custom { vcpu: 1, memory_mib: 3072, disk_mb: 20_000 }));
        assert_eq!(instance_size("standard-3"), Ok(InstanceSize::Named("standard-3".into())));
        assert_eq!(serde_json::to_value(instance_size("standard-3").unwrap()).unwrap(), serde_json::json!("standard-3"));
        assert_eq!(instance_size("4vcpu-12gib"), Ok(InstanceSize::Custom { vcpu: 4, memory_mib: 12_288, disk_mb: 20_000 }));
        for bad in ["", "huge", "2vcpu-4gib", "5vcpu-15gib", "0vcpu-3gib", "1vcpu-13gib", "2vcpu-6", "vcpu-6gib", "-1vcpu-6gib"] {
            assert!(instance_size(bad).is_err(), "{bad:?}");
        }
    }

    fn tokens(model: &str, input: u64, cached_input: u64, cache_write: u64, output: u64) -> Usage {
        Usage::Tokens { model: model.into(), input, cached_input, cache_write, output }
    }

    const FLASH: &str = "@cf/zai-org/glm-5.3-flash";
    const GLM: &str = "@cf/zai-org/glm-5.3";
    const OPUS: &str = "anthropic/claude-opus-5.5";
    const DEEPSEEK: &str = "@cf/deepseek-ai/deepseek-v4-flash-0731";
    const PRIVATE: &str = "glm-5-3-flash";

    /// Goal: the default book is one a ledger accepts, at the version its
    /// last change raised it to. Method: validate it.
    #[test]
    fn the_defaults_are_a_valid_book() {
        let book = PriceBook::defaults();
        assert_eq!(book.validate(), Ok(()));
        assert_eq!(book.version, 13);
        assert_eq!(book.models.len(), 12);
        assert_eq!((book.margin_bp, book.credits_fee_bp), (5_000, 500));
    }

    /// Goal: Finite Private's GLM-5.3 Flash is priced as Workers AI's
    /// (decision 61, a decision for Paul): one turn charges the same on
    /// either. Method: S3's $0.004 Flash turn on both.
    #[test]
    fn finite_private_is_priced_as_workers_ais_flash() {
        let book = PriceBook::defaults();
        let p = |model: &str| book.models.iter().find(|m| m.model == model).unwrap().price;
        assert_eq!(p(PRIVATE), p(FLASH));
        let turn = |model: &str| book.price(&tokens(model, 26_000, 0, 0, 200)).unwrap();
        assert_eq!(turn(PRIVATE), Priced { list: 4_000, cost: 4_200, charge: 6_300 });
        assert_eq!(turn(PRIVATE), turn(FLASH));
    }

    /// Goal: decision 24's formula on S3's simple turn. Method: a Flash
    /// turn that lists at exactly $0.004 (26,000 tokens in, 200 out)
    /// charges $0.0063 = 0.004 × 1.05 × 1.5, with a cost basis of $0.0042.
    #[test]
    fn a_four_tenths_of_a_cent_flash_turn_charges_0_0063() {
        let p = PriceBook::defaults().price(&tokens(FLASH, 26_000, 0, 0, 200)).unwrap();
        assert_eq!(p, Priced { list: 4_000, cost: 4_200, charge: 6_300 });
        assert_eq!(dollars(p.charge), "$0.0063");
    }

    /// Goal: our list price is the gateway's own `cost` (spike S4, B4).
    /// Method: the four calls whose gateway cost the spike recorded.
    #[test]
    fn list_prices_match_the_gateways_cost() {
        let book = PriceBook::defaults();
        let list = |u: Usage| book.price(&u).unwrap().list;
        assert_eq!(list(tokens(OPUS, 19, 0, 0, 5)), 176, "$0.000176");
        assert_eq!(list(tokens(OPUS, 411, 0, 0, 50)), 2_644, "$0.002644");
        assert_eq!(list(tokens(FLASH, 15, 0, 0, 3)), 4, "$0.00000375, rounded up");
        assert_eq!(list(tokens(GLM, 59, 128, 0, 12)), 169, "$0.00016868, rounded up");
    }

    /// Goal: neurons price a Workers AI call as its tokens do (spike S4:
    /// neurons × $0.000011 matched tokens × catalog price on every call).
    /// Method: the spike's metered calls (model, uncached in, cached in,
    /// out, neurons as Workers AI reported them): the exact token price and
    /// the neurons' agree within half a micro-dollar, and metering the
    /// neurons in thousandths (rounded up) charges within a micro-dollar
    /// of the tokens.
    #[test]
    fn neurons_and_tokens_agree_on_the_spikes_calls() {
        let calls: [(&str, u64, u64, u64, f64); 19] = [
            (FLASH, 187, 0, 12, 3.0954543352127075),
            (FLASH, 218, 0, 15, 3.654545336961746),
            (FLASH, 187, 0, 12, 3.095454216003418),
            (FLASH, 90, 128, 14, 2.2127268314361572),
            (GLM, 187, 0, 12, 28.599999278783798),
            (GLM, 218, 0, 11, 32.14545485377312),
            (GLM, 59, 128, 12, 15.334546089172363),
            (GLM, 218, 0, 14, 33.345455169677734),
            (FLASH, 187, 0, 30, 3.9136362075805664),
            (FLASH, 218, 0, 19, 3.8363635540008545),
            (GLM, 187, 0, 12, 28.599998474121094),
            (GLM, 218, 0, 11, 32.14545440673828),
            (FLASH, 6_055, 0, 3, 82.70453992486),
            (GLM, 50_460, 0, 3, 6_423.381640642881),
            (GLM, 122_016, 0, 3, 15_530.508593767881),
            (GLM, 257_908, 50_432, 3, 34_017.87968751788),
            (FLASH, 23, 0, 15, 0.9954546093940735),
            // DeepSeek V4 Flash, 2026-10-09 (the fallback's price)
            (DEEPSEEK, 302, 0, 82, 21.920000076293945),
            (DEEPSEEK, 100, 0, 64, 11.680000305175781),
        ];
        let book = PriceBook::defaults();
        for (model, input, cached, output, neurons) in calls {
            let p = book.models.iter().find(|m| m.model == model).unwrap().price;
            let exact_tokens = (input as f64 * p.input as f64 + cached as f64 * p.cached_input as f64 + output as f64 * p.output as f64) / 1e6;
            let exact_neurons = neurons * 11.0;
            assert!((exact_tokens - exact_neurons).abs() < 0.5, "{model} {input}/{cached}/{output}: {exact_tokens} µ$ by tokens, {exact_neurons} by neurons");
            let by_tokens = book.price(&tokens(model, input, cached, 0, output)).unwrap();
            let by_neurons = book.price(&Usage::Neurons { milli: (neurons * 1000.0).ceil() as u64 }).unwrap();
            assert!((by_tokens.charge - by_neurons.charge).abs() <= 1, "{model} {input}/{cached}/{output}: {by_tokens:?} {by_neurons:?}");
        }
    }

    /// Goal: a row rounds once, from its exact sum, never part by part.
    /// Method: one Flash token in and one cached lists at 0.18 µ$ and
    /// charges ceil(0.2835) = 1, where rounding each part would charge 2.
    #[test]
    fn a_row_rounds_up_once() {
        let book = PriceBook::defaults();
        assert_eq!(book.price(&tokens(FLASH, 1, 1, 0, 0)).unwrap(), Priced { list: 1, cost: 1, charge: 1 });
        assert_eq!(book.price(&tokens(FLASH, 0, 0, 0, 0)).unwrap(), Priced { list: 0, cost: 0, charge: 0 }, "nothing costs nothing");
        // 0.45 µ$ a request: rounded up, never down to free
        assert_eq!(book.price(&Usage::Requests { count: 1 }).unwrap(), Priced { list: 1, cost: 1, charge: 1 });
        assert_eq!(book.price(&Usage::Requests { count: 1_000_000 }).unwrap(), Priced { list: 450_000, cost: 450_000, charge: 675_000 });
    }

    /// Goal: only AI carries the credits fee; every other meter is its
    /// list price × 1.5. Method: one of each kind against the defaults.
    #[test]
    fn every_meter_kind_at_its_default_price() {
        let book = PriceBook::defaults();
        let price = |u: Usage| book.price(&u).unwrap();
        let hour = Usage::Awake { instance: "2vcpu-6gib".into(), ms: 3_600_000 };
        assert_eq!(price(hour), Priced { list: 64_224, cost: 64_224, charge: 96_336 });
        let month = Usage::Awake { instance: "2vcpu-6gib".into(), ms: 30 * 24 * 3_600_000 };
        assert_eq!(dollars(price(month).list), "$46.24128", "an always-on month at list");
        let gb_month = |class| Usage::Storage { class, byte_hours: 1_000_000_000 * 720 };
        assert_eq!(price(gb_month(StorageClass::Sqlite)), Priced { list: 200_000, cost: 200_000, charge: 300_000 });
        assert_eq!(price(gb_month(StorageClass::R2)), Priced { list: 15_000, cost: 15_000, charge: 22_500 });
        assert_eq!(price(gb_month(StorageClass::Git)).list, 15_000);
        assert_eq!(price(Usage::DynamicWorkers { count: 1 }), Priced { list: 2_000, cost: 2_000, charge: 3_000 });
        // a three-second screenshot: 75 µ$ at list
        assert_eq!(price(Usage::Browser { ms: 3_000 }), Priced { list: 75, cost: 75, charge: 113 });
        assert_eq!(price(Usage::Images { count: 1 }), Priced { list: 500, cost: 500, charge: 750 });
        // 1,000 neurons: $0.011 at list, × 1.05 × 1.5
        assert_eq!(price(Usage::Neurons { milli: 1_000_000 }), Priced { list: 11_000, cost: 11_550, charge: 17_325 });
        let mut keyed = book.clone();
        keyed.keys.push(KeyPrice { key: "search".into(), micros: 5_000_000, per: 1_000 });
        assert_eq!(keyed.price(&Usage::Key { key: "search".into(), units: 1 }).unwrap(), Priced { list: 5_000, cost: 5_000, charge: 7_500 }, "no credits fee on an operator key");
        // what settings shows a person: its calls' charge, for its price's count
        assert_eq!(keyed.key_charge("search"), Some((7_500_000, 1_000)));
        assert_eq!(keyed.key_charge("nonesuch"), None, "a key the book prices not");
    }

    /// Goal: each operator key Paul named (2026-10-04) has a list price
    /// per call, and one call of each is charged that and the margin.
    #[test]
    fn the_operator_keys_list_prices() {
        let mut book = PriceBook::defaults();
        book.keys = DEFAULT_KEYS.iter().map(|(key, micros, per)| KeyPrice { key: key.to_string(), micros: *micros, per: *per }).collect();
        assert_eq!(book.validate(), Ok(()));
        let call = |key: &str| book.price(&Usage::Key { key: key.into(), units: 1 }).unwrap();
        assert_eq!(call("perplexity"), Priced { list: 5_000, cost: 5_000, charge: 7_500 }, "$5 per 1,000 searches");
        assert_eq!(call("google-places"), Priced { list: 35_000, cost: 35_000, charge: 52_500 }, "$35 per 1,000 Enterprise text searches");
        assert_eq!(call("xai"), Priced { list: 120_000, cost: 120_000, charge: 180_000 });
        assert_eq!(call("elevenlabs"), Priced { list: 150_000, cost: 150_000, charge: 225_000 }, "a minute of music");
        assert_eq!(call("firecrawl"), Priced { list: 10_000, cost: 10_000, charge: 15_000 }, "two Hobby PAYG credits");
        assert_eq!(call("x"), Priced { list: 50_000, cost: 50_000, charge: 75_000 }, "ten post resources");
        assert_eq!(call("fal"), Priced { list: 6_000, cost: 6_000, charge: 9_000 }, "one MP Klein 9B image");
        assert_eq!(call("browser-use"), Priced { list: 1_667, cost: 1_667, charge: 2_500 }, "five browser minutes");
        let spoken = |units| book.price(&Usage::Key { key: "elevenlabs:speech".into(), units }).unwrap();
        assert_eq!(spoken(200), Priced { list: 16_000, cost: 16_000, charge: 24_000 }, "a 200-character reply, $0.08 per 1,000");
        assert_eq!(spoken(1), Priced { list: 80, cost: 80, charge: 120 });
        assert_eq!(spoken(0), Priced { list: 0, cost: 0, charge: 0 }, "a call whose characters were not read");
        let pixels = |units| book.price(&Usage::Key { key: "fal:image".into(), units }).unwrap();
        assert_eq!(pixels(1_048_576), Priced { list: 6_000, cost: 6_000, charge: 9_000 }, "a megapixel, $0.006");
        assert_eq!(pixels(1024 * 576), Priced { list: 3_375, cost: 3_375, charge: 5_063 }, "Hermes' landscape_16_9 image");
        assert_eq!(pixels(1), Priced { list: 1, cost: 1, charge: 1 }, "rounded up once");
        assert_eq!(call("fal").charge, pixels(1_048_576).charge, "an older release's row: one image, as before");
        assert_eq!(default_key_price("perplexity"), Some((5_000_000, 1_000)));
        assert_eq!(default_key_price("elevenlabs:speech"), Some((80_000, 1_000)));
        assert_eq!(default_key_price("fal:image"), Some((6_000, 1_048_576)));
        assert_eq!(call("xai:chat"), Priced { list: 15_000, cost: 15_000, charge: 22_500 }, "a Grok chat call's tokens");
        assert_eq!(call("xai:image"), Priced { list: 20_000, cost: 20_000, charge: 30_000 }, "a grok-imagine-image");
        assert_eq!(call("google-places:details"), Priced { list: 20_000, cost: 20_000, charge: 30_000 }, "$20 per 1,000 Enterprise details");
        assert_eq!(call("google-places:photo"), Priced { list: 7_000, cost: 7_000, charge: 10_500 }, "$7 per 1,000 photos");
        assert_eq!(call("google-places:autocomplete"), Priced { list: 2_830, cost: 2_830, charge: 4_245 }, "$2.83 per 1,000 autocompletes");
        assert_eq!(call("firecrawl:credit"), Priced { list: 5_000, cost: 5_000, charge: 7_500 }, "a Hobby PAYG credit");
        assert_eq!(default_key_price("nonesuch"), None);
    }

    /// Goal: each part the book prices apart is priced once, in
    /// `DEFAULT_KEYS`, as is its key, and its name is no provider's.
    #[test]
    fn each_key_part_has_one_price_and_no_providers_name() {
        for p in KEY_PARTS {
            assert!(default_key_price(p.key).is_some(), "{p:?}: its key's other calls are priced");
            assert!(default_key_price(p.priced_as).is_some(), "{p:?}: priced");
            assert_eq!(DEFAULT_KEYS.iter().filter(|(n, _, _)| *n == p.priced_as).count(), 1, "{p:?}: once");
            assert!(p.priced_as.starts_with(&format!("{}:", p.key)) && !crate::catalog::valid_name(p.priced_as), "{p:?}: never a catalog row's name");
            assert!(printable(p.priced_as, NAME_MAX_BYTES), "{p:?}: a name the ledger stores");
            assert!(p.units_max >= 1, "{p:?}: a call it made is charged");
        }
        // speech's most characters fit the body read, each a JSON escape
        let speech = KEY_PARTS.iter().find(|p| p.priced_as == "elevenlabs:speech").expect("speech is a part");
        assert!(speech.units_max as usize * 6 + 1024 <= KEY_PART_BODY_MAX_BYTES);
    }

    /// A JSON `POST` on `key` to `path`, answered `status`, as the swap
    /// meters it.
    fn posted(key: &str, path: &str, body: &[u8], status: u16) -> Usage {
        key_usage(key, &KeyCall { method: "POST", host: "api.test", path, body, status })
    }

    /// Goal (FIN-443): ElevenLabs' text to speech is metered by its text's
    /// characters, each of its endpoints, and its music, as every other
    /// call on the key, as one call (a minute). Method: the calls Hermes'
    /// `text_to_speech` and the music skill make, priced as a deployment's
    /// book prices them.
    #[test]
    fn speech_is_metered_by_its_characters_and_music_by_the_call() {
        let mut book = PriceBook::defaults();
        book.keys = DEFAULT_KEYS.iter().map(|(key, micros, per)| KeyPrice { key: key.to_string(), micros: *micros, per: *per }).collect();
        let reply = "a".repeat(200);
        let body = serde_json::to_vec(&serde_json::json!({ "text": reply, "model_id": "eleven_multilingual_v2" })).unwrap();
        let spoken = Usage::Key { key: "elevenlabs:speech".into(), units: 200 };
        for path in ["/v1/text-to-speech/pNInz6obpgDQGcFmaJgB", "/v1/text-to-speech/v/stream", "/v1/text-to-speech/v/with-timestamps", "/v1/text-to-speech/v/stream/with-timestamps"] {
            assert_eq!(posted("elevenlabs", path, &body, 200), spoken, "{path}");
        }
        assert_eq!(book.price(&spoken).unwrap().charge, 24_000, "$0.024, not a minute of music's $0.225");
        let music = serde_json::to_vec(&serde_json::json!({ "prompt": "a jingle", "music_length_ms": 15_000 })).unwrap();
        let minute = Usage::Key { key: "elevenlabs".into(), units: 1 };
        assert_eq!(posted("elevenlabs", "/v1/music", &music, 200), minute);
        assert_eq!(posted("elevenlabs", "/v1/music/plan", &music, 200), minute);
        assert_eq!(book.price(&minute).unwrap().charge, 225_000);
        // characters, not bytes: ElevenLabs bills each character
        let accented = serde_json::to_vec(&serde_json::json!({ "text": "héllo, 世界" })).unwrap();
        assert_eq!(posted("elevenlabs", "/v1/text-to-speech/v", &accented, 200), Usage::Key { key: "elevenlabs:speech".into(), units: 9 });
        // the path's own segment: a sibling endpoint is no speech
        assert_eq!(posted("elevenlabs", "/v1/text-to-speech-other", &body, 200), minute);
        assert_eq!(posted("elevenlabs", "/v2/text-to-speech/v", &body, 200), minute);
        // only the key the part names
        assert_eq!(posted("perplexity", "/v1/text-to-speech/v", &body, 200), Usage::Key { key: "perplexity".into(), units: 1 });
    }

    /// Goal (FIN-443's review): a speech call ElevenLabs refused made
    /// nothing, so is charged nothing, whatever its text; one it answered
    /// is charged at most a request's limit; every other call on the key is
    /// a call, as the swap charges any answer under 500. Method: a 10,000
    /// character request at each answer the swap meters.
    #[test]
    fn a_refused_speech_call_is_charged_nothing() {
        let mut book = PriceBook::defaults();
        book.keys = DEFAULT_KEYS.iter().map(|(key, micros, per)| KeyPrice { key: key.to_string(), micros: *micros, per: *per }).collect();
        let long = serde_json::to_vec(&serde_json::json!({ "text": "a".repeat(10_000) })).unwrap();
        let spoken = |status| posted("elevenlabs", "/v1/text-to-speech/v/stream", &long, status);
        for made in [200, 201, 299] {
            assert_eq!(spoken(made), Usage::Key { key: "elevenlabs:speech".into(), units: 10_000 }, "{made}");
        }
        assert_eq!(book.price(&spoken(200)).unwrap().charge, 1_200_000, "10,000 characters made: $0.80 at list");
        // a bad request, no key, no voice, too long, a rate or concurrency limit, a redirect not followed
        for refused in [400, 401, 402, 403, 404, 409, 413, 422, 429, 302, 304, 100, 0] {
            let u = spoken(refused);
            assert_eq!(u, Usage::Key { key: "elevenlabs:speech".into(), units: 0 }, "{refused}");
            assert_eq!(book.price(&u).unwrap().charge, 0, "{refused}");
        }
        // a 2xx is charged at most a request's limit: 40,000 characters, $4.80
        let huge = serde_json::to_vec(&serde_json::json!({ "text": "a".repeat(1_000_000) })).unwrap();
        let most = posted("elevenlabs", "/v1/text-to-speech/v", &huge, 200);
        assert_eq!(most, Usage::Key { key: "elevenlabs:speech".into(), units: 40_000 });
        assert_eq!(book.price(&most).unwrap().charge, 4_800_000);
        assert_eq!(posted("elevenlabs", "/v1/text-to-speech/v", &huge, 413), Usage::Key { key: "elevenlabs:speech".into(), units: 0 }, "refused as too long: nothing");
        // music, and any other call, is one call at every answer the swap meters
        for status in [200, 400, 429] {
            assert_eq!(posted("elevenlabs", "/v1/music", b"{}", status), Usage::Key { key: "elevenlabs".into(), units: 1 }, "{status}");
        }
    }

    /// Goal: a speech call whose characters the meter cannot read is
    /// charged nothing, never a guess above its cost (decision 62).
    /// Method: bodies that are no JSON object holding `text` as a string,
    /// and none (a socket's upgrade, `stream-input`).
    #[test]
    fn a_speech_call_the_meter_cannot_read_is_charged_nothing() {
        let mut book = PriceBook::defaults();
        book.keys = DEFAULT_KEYS.iter().map(|(key, micros, per)| KeyPrice { key: key.to_string(), micros: *micros, per: *per }).collect();
        let none = Usage::Key { key: "elevenlabs:speech".into(), units: 0 };
        for body in [&b""[..], b"text=hello", b"{\"text\":", b"[\"hello\"]", b"\"hello\"", b"{\"text\":42}", b"{\"text\":null}", b"{\"words\":\"hello\"}", &[0xff, 0xfe][..]] {
            assert_eq!(posted("elevenlabs", "/v1/text-to-speech/v/stream-input", body, 200), none, "{}", String::from_utf8_lossy(body));
        }
        assert_eq!(book.price(&none).unwrap().charge, 0);
        assert_eq!(posted("elevenlabs", "/v1/text-to-speech/v", b"{\"text\":\"\"}", 200), none, "an empty text has no characters");
        // the largest body read, and one past it
        let sized = |n: usize| format!("{{\"text\":\"{}\"}}", "a".repeat(n - 11)).into_bytes();
        assert_eq!(sized(KEY_PART_BODY_MAX_BYTES).len(), KEY_PART_BODY_MAX_BYTES);
        let most = Usage::Key { key: "elevenlabs:speech".into(), units: 40_000 };
        assert_eq!(posted("elevenlabs", "/v1/text-to-speech/v", &sized(KEY_PART_BODY_MAX_BYTES), 200), most, "read, at most its vendor's limit");
        assert_eq!(posted("elevenlabs", "/v1/text-to-speech/v", &sized(KEY_PART_BODY_MAX_BYTES + 1), 200), none, "past it, not parsed");
        // a release whose book has no part's price refuses its rows, as it
        // does any it cannot price: the ledger charges them nothing
        let older = PriceBook { keys: book.keys.iter().filter(|k| k.key != "elevenlabs:speech").cloned().collect(), ..book.clone() };
        assert_eq!(older.price(&Usage::Key { key: "elevenlabs:speech".into(), units: 200 }), Err(PriceError::NoPrice));
    }

    /// A call on fal's key, answered `status`, as the swap meters it.
    fn fal(method: &str, host: &str, path: &str, body: &[u8], status: u16) -> Usage {
        key_usage("fal", &KeyCall { method, host, path, body, status })
    }

    /// Goal (FIN-445): a fal generation is charged by the pixels it asks
    /// for, at Klein's rate, and the calls the SDK makes around it (its
    /// queue's polls, status, result and cancel, its uploads) nothing.
    /// Method: the calls Hermes' `image_generate` makes for one image
    /// (`fal_client` 0.13: a submit, a status poll every 0.25 s and up,
    /// the result), priced as a deployment's book prices them.
    #[test]
    fn a_fal_image_is_metered_by_its_pixels_and_its_queue_by_nothing() {
        let mut book = PriceBook::defaults();
        book.keys = DEFAULT_KEYS.iter().map(|(key, micros, per)| KeyPrice { key: key.to_string(), micros: *micros, per: *per }).collect();
        let hermes = br#"{"prompt":"a lighthouse at dusk","image_size":"landscape_16_9","num_inference_steps":4,"output_format":"png","enable_safety_checker":false}"#;
        let made = Usage::Key { key: "fal:image".into(), units: 1024 * 576 };
        assert_eq!(fal("POST", "queue.fal.run", "/fal-ai/flux-2/klein/9b", hermes, 200), made, "the queue's submit");
        assert_eq!(fal("POST", "fal.run", "/fal-ai/flux-2/klein/9b", hermes, 200), made, "a synchronous run");
        let free = Usage::Key { key: "fal".into(), units: 0 };
        let around = [
            ("GET", "queue.fal.run", "/fal-ai/flux-2/requests/0b1e/status"),
            ("GET", "queue.fal.run", "/fal-ai/flux-2/requests/0b1e/status/stream"),
            ("GET", "queue.fal.run", "/fal-ai/flux-2/requests/0b1e"),
            ("PUT", "queue.fal.run", "/fal-ai/flux-2/requests/0b1e/cancel"),
            ("POST", "queue.fal.run", "/fal-ai/flux-2/requests/0b1e/cancel"),
            ("POST", "rest.fal.ai", "/storage/upload/initiate"),
            ("POST", "rest.fal.ai", "/storage/auth/token"),
            ("GET", "queue.fal.run", "/fal-ai/flux-2/klein/9b"),
        ];
        for (method, host, path) in around {
            assert_eq!(fal(method, host, path, hermes, 200), free, "{method} {host}{path}");
        }
        // one image: a submit, six polls (2 s at fal_client's backoff, 0.25 s
        // and 5% more each) and the result, once eight images' $0.072
        let calls = [made.clone()].into_iter().chain(std::iter::repeat_n(free.clone(), 7));
        let charged: i64 = calls.map(|u| book.price(&u).unwrap().charge).sum();
        assert_eq!(charged, 5_063, "$0.0051");
        assert_eq!(8 * book.price(&Usage::Key { key: "fal".into(), units: 1 }).unwrap().charge, 72_000);
        assert_eq!(book.price(&free).unwrap().charge, 0);
        // the presets, a size of its own, several images, and none named
        let sized = |body: &str| fal("POST", "queue.fal.run", "/fal-ai/flux-2/klein/9b", body.as_bytes(), 200);
        let units = |units| Usage::Key { key: "fal:image".into(), units };
        for (preset, pixels) in [("square_hd", 1_048_576), ("square", 262_144), ("portrait_4_3", 786_432), ("portrait_16_9", 589_824), ("landscape_4_3", 786_432), ("landscape_16_9", 589_824)] {
            assert_eq!(sized(&format!(r#"{{"prompt":"p","image_size":"{preset}"}}"#)), units(pixels), "{preset}");
        }
        assert_eq!(sized(r#"{"prompt":"p","image_size":{"width":1920,"height":1080}}"#), units(1920 * 1080));
        assert_eq!(sized(r#"{"prompt":"p","image_size":"square_hd","num_images":4}"#), units(4 * 1_048_576));
        assert_eq!(sized(r#"{"prompt":"p"}"#), units(1024 * 768), "Klein's default, landscape_4_3");
        assert_eq!(fal("POST", "fal.run", "/fal-ai/flux-2/klein/9b", br#"{"prompt":"p","num_images":2}"#, 200), units(2 * 1024 * 768));
        // an edit's size is its input's unless it names one (FIN-445 review)
        let edit = |body: &str| fal("POST", "queue.fal.run", "/fal-ai/flux-2/klein/9b/edit", body.as_bytes(), 200);
        assert_eq!(edit(r#"{"prompt":"p","image_urls":["https://v3.fal.media/files/a.png"]}"#), units(0), "an edit of an input the meter does not see");
        assert_eq!(edit(r#"{"prompt":"p","image_urls":["a"],"image_size":"square"}"#), units(512 * 512));
        assert_eq!(edit(r#"{"prompt":"p","image_urls":["a"],"image_size":{"width":2048,"height":2048}}"#), units(4 * 1_048_576));
        // another model's default is its own: unread
        assert_eq!(fal("POST", "queue.fal.run", "/fal-ai/flux-2-pro", br#"{"prompt":"p"}"#, 200), units(0));
        assert_eq!(fal("POST", "queue.fal.run", "/fal-ai/flux-2-pro", br#"{"prompt":"p","image_size":"square_hd"}"#, 200), units(1_048_576));
        // only fal's key
        assert_eq!(key_usage("x", &KeyCall { method: "GET", host: "api.x.com", path: "/2/tweets/search/recent", body: b"", status: 200 }), Usage::Key { key: "x".into(), units: 1 });
    }

    /// Goal: a fal generation the meter cannot read, or fal refused, is
    /// charged nothing, and one it made at most a call's most (decision
    /// 62); a release whose book has no image price charges its rows
    /// nothing. Method: bodies that say no size, every answer the swap
    /// meters, and sizes past the cap.
    #[test]
    fn a_fal_call_the_meter_cannot_read_or_fal_refused_is_charged_nothing() {
        let none = Usage::Key { key: "fal:image".into(), units: 0 };
        let submit = |body: &[u8], status| fal("POST", "queue.fal.run", "/fal-ai/flux-2/klein/9b", body, status);
        for body in [&b""[..], b"prompt=p", b"{\"prompt\":", b"[\"p\"]", b"\"p\"", br#"{"image_size":"poster"}"#, br#"{"image_size":{"width":1024}}"#, br#"{"image_size":{"width":-1,"height":1024}}"#, br#"{"image_size":{"width":1.5,"height":1024}}"#, br#"{"image_size":{"width":"1024","height":"1024"}}"#, br#"{"image_size":42}"#, br#"{"num_images":"2"}"#, br#"{"num_images":-1}"#, br#"{"num_images":0}"#, &[0xff, 0xfe][..]] {
            assert_eq!(submit(body, 200), none, "{}", String::from_utf8_lossy(body));
        }
        // a bad request, no key, a locked account, a rate limit, a redirect not followed
        for refused in [400, 401, 403, 404, 422, 429, 302, 100, 0] {
            assert_eq!(submit(br#"{"prompt":"p"}"#, refused), none, "{refused}");
        }
        // at most 16 megapixels a call, even sizes that overflow
        let most = Usage::Key { key: "fal:image".into(), units: 16 * 1_048_576 };
        assert_eq!(submit(br#"{"image_size":{"width":100000,"height":100000}}"#, 200), most);
        assert_eq!(submit(br#"{"image_size":{"width":18446744073709551615,"height":2},"num_images":2}"#, 200), none, "an overflow is no size");
        assert_eq!(submit(br#"{"image_size":"square_hd","num_images":1000}"#, 200), most);
        let mut book = PriceBook::defaults();
        book.keys = DEFAULT_KEYS.iter().map(|(key, micros, per)| KeyPrice { key: key.to_string(), micros: *micros, per: *per }).collect();
        assert_eq!(book.price(&most).unwrap().charge, 144_000, "$0.096 at list");
        // a release before the image price refuses its rows as unpriced
        let older = PriceBook { keys: book.keys.iter().filter(|k| k.key != "fal:image").cloned().collect(), ..book.clone() };
        assert_eq!(older.price(&Usage::Key { key: "fal:image".into(), units: 589_824 }), Err(PriceError::NoPrice));
        assert_eq!(older.price(&Usage::Key { key: "fal".into(), units: 0 }).unwrap().charge, 0, "a poll is free in it too");
    }

    /// A call on xAI's key, answered `status`, as the swap meters it.
    fn xai(method: &str, path: &str, body: &[u8], status: u16) -> Usage {
        key_usage("xai", &KeyCall { method, host: "api.x.ai", path, body, status })
    }

    /// Goal (FIN-445): only an X search on xAI's key is charged as one; a
    /// plain Grok chat call is charged a typical chat's tokens, an image
    /// the images it asks for, and any other call nothing. Method: the
    /// call Hermes' `x_search` makes, and the chat and image calls an
    /// agent's code makes with `XAI_API_KEY`.
    #[test]
    fn an_xai_call_is_charged_as_an_x_search_only_when_it_is_one() {
        let mut book = PriceBook::defaults();
        book.keys = DEFAULT_KEYS.iter().map(|(key, micros, per)| KeyPrice { key: key.to_string(), micros: *micros, per: *per }).collect();
        let hermes = br#"{"model":"grok-4.3","input":[{"role":"user","content":"what is new on X about fragments"}],"tools":[{"type":"x_search"}]}"#;
        let search = Usage::Key { key: "xai".into(), units: 1 };
        assert_eq!(xai("POST", "/v1/responses", hermes, 200), search, "Hermes' x_search");
        assert_eq!(xai("POST", "/v1/chat/completions", br#"{"model":"grok-4.3","messages":[],"tools":[{"type":"web_search"},{"type":"x_search","allowed_x_handles":["x"]}]}"#, 200), search);
        assert_eq!(book.price(&search).unwrap().charge, 180_000, "an X search, as before");
        let chat = Usage::Key { key: "xai:chat".into(), units: 1 };
        for (path, body) in [
            ("/v1/chat/completions", &br#"{"model":"grok-4.3","messages":[{"role":"user","content":"hi"}]}"#[..]),
            ("/v1/responses", br#"{"model":"grok-4.3","input":"hi"}"#),
            ("/v1/responses", br#"{"model":"grok-4.3","input":"hi","tools":[{"type":"web_search"}]}"#),
            ("/v1/responses", br#"{"model":"grok-4.3","input":"hi","tools":"x_search"}"#),
            ("/v1/messages", br#"{"model":"grok-4.3","max_tokens":64,"messages":[{"role":"user","content":"hi"}]}"#),
            ("/v1/chat/completions", b"a body the meter does not read"),
        ] {
            assert_eq!(xai("POST", path, body, 200), chat, "{path} {}", String::from_utf8_lossy(body));
        }
        assert_eq!(book.price(&chat).unwrap().charge, 22_500, "$0.0225, not an X search's $0.18");
        let images = |units| Usage::Key { key: "xai:image".into(), units };
        assert_eq!(xai("POST", "/v1/images/generations", br#"{"model":"grok-imagine-image","prompt":"a cat"}"#, 200), images(1));
        assert_eq!(xai("POST", "/v1/images/generations", br#"{"model":"grok-imagine-image","prompt":"a cat","n":4}"#, 200), images(4));
        assert_eq!(xai("POST", "/v1/images/generations", br#"{"prompt":"a cat","n":50}"#, 200), images(10), "at most a request's 10");
        assert_eq!(book.price(&images(1)).unwrap().charge, 30_000);
        let free = Usage::Key { key: "xai".into(), units: 0 };
        for (method, path) in [("GET", "/v1/models"), ("GET", "/v1/responses/resp_1"), ("DELETE", "/v1/responses/resp_1"), ("POST", "/v1/videos/generations"), ("POST", "/v1/tokenize-text"), ("GET", "/v1/api-key"), ("GET", "/v1/chat/completions")] {
            assert_eq!(xai(method, path, hermes, 200), free, "{method} {path}");
        }
        assert_eq!(book.price(&free).unwrap().charge, 0);
    }

    /// Goal: a call xAI refused made nothing, so is charged nothing, an X
    /// search too; an image call whose count the meter cannot read is
    /// charged nothing; a release whose book has no chat or image price
    /// refuses those rows, and prices an X search as before. Method: every
    /// answer the swap meters, and bodies that say no count.
    #[test]
    fn an_xai_call_refused_or_unread_is_charged_nothing() {
        let hermes = br#"{"model":"grok-4.3","input":"news","tools":[{"type":"x_search"}]}"#;
        // a bad request, no key, no credit, no model, a rate limit, a redirect not followed
        for refused in [400, 401, 403, 404, 422, 429, 302, 0] {
            assert_eq!(xai("POST", "/v1/responses", hermes, refused), Usage::Key { key: "xai".into(), units: 0 }, "{refused}");
            assert_eq!(xai("POST", "/v1/chat/completions", b"{}", refused), Usage::Key { key: "xai:chat".into(), units: 0 }, "{refused}");
            assert_eq!(xai("POST", "/v1/images/generations", b"{}", refused), Usage::Key { key: "xai:image".into(), units: 0 }, "{refused}");
        }
        for body in [&b""[..], b"n=2", b"[1]", br#"{"n":"2"}"#, br#"{"n":-1}"#, br#"{"n":1.5}"#, br#"{"n":0}"#] {
            assert_eq!(xai("POST", "/v1/images/generations", body, 200), Usage::Key { key: "xai:image".into(), units: 0 }, "{}", String::from_utf8_lossy(body));
        }
        let mut book = PriceBook::defaults();
        book.keys = DEFAULT_KEYS.iter().map(|(key, micros, per)| KeyPrice { key: key.to_string(), micros: *micros, per: *per }).collect();
        let older = PriceBook { keys: book.keys.iter().filter(|k| !k.key.starts_with("xai:")).cloned().collect(), ..book.clone() };
        assert_eq!(older.price(&Usage::Key { key: "xai:chat".into(), units: 1 }), Err(PriceError::NoPrice));
        assert_eq!(older.price(&Usage::Key { key: "xai:image".into(), units: 1 }), Err(PriceError::NoPrice));
        assert_eq!(older.price(&Usage::Key { key: "xai".into(), units: 1 }).unwrap().charge, 180_000, "an X search, as before");
    }

    /// A call on Google Places' key, answered `status`, as the swap meters it.
    fn places(method: &str, path: &str, status: u16) -> Usage {
        key_usage("google-places", &KeyCall { method, host: "places.googleapis.com", path, body: b"", status })
    }

    /// Goal (FIN-445): a Google Places search is charged an Enterprise
    /// search as before, a place's details, its photo and an autocomplete
    /// each its own list, and any other call nothing. Method: the goplaces
    /// skill's calls (Places API (New)), and the others the API takes.
    #[test]
    fn a_places_call_is_charged_as_what_it_is() {
        let mut book = PriceBook::defaults();
        book.keys = DEFAULT_KEYS.iter().map(|(key, micros, per)| KeyPrice { key: key.to_string(), micros: *micros, per: *per }).collect();
        let charge = |u: &Usage| book.price(u).unwrap().charge;
        let one = |key: &str| Usage::Key { key: key.into(), units: 1 };
        assert_eq!(places("POST", "/v1/places:searchText", 200), one("google-places"));
        assert_eq!(places("POST", "/v1/places:searchNearby", 200), one("google-places"));
        assert_eq!(charge(&one("google-places")), 52_500, "an Enterprise search, as before");
        assert_eq!(places("GET", "/v1/places/ChIJj61dQgK6j4AR4GeTYWZsKWw", 200), one("google-places:details"));
        assert_eq!(charge(&one("google-places:details")), 30_000, "$0.03, not a search's $0.0525");
        assert_eq!(places("GET", "/v1/places/ChIJj61dQgK6j4AR4GeTYWZsKWw/photos/AUc7tXX/media", 200), one("google-places:photo"), "skipHttpRedirect: its JSON");
        // its default answer is a redirect to the image (FIN-445 review)
        for redirect in [301, 302, 303, 307, 308] {
            assert_eq!(places("GET", "/v1/places/ChIJj61dQgK6j4AR4GeTYWZsKWw/photos/AUc7tXX/media", redirect), one("google-places:photo"), "{redirect}");
        }
        assert_eq!(charge(&one("google-places:photo")), 10_500);
        assert_eq!(places("POST", "/v1/places:autocomplete", 200), one("google-places:autocomplete"));
        let free = Usage::Key { key: "google-places".into(), units: 0 };
        for path in ["/v1/places/", "/v1/places/ChIJ/photos/AUc7", "/v1/places:searchtext", "/v1/places:searchText/x", "/v2/places:searchText", "/maps/api/place/textsearch/json", "/"] {
            assert_eq!(places("GET", path, 200), free, "{path}");
        }
        assert_eq!(charge(&free), 0);
    }

    /// Goal: a Places call Google refused (a bad field mask, a key over
    /// its quota) is charged nothing, a search too; a release whose book
    /// has no details price refuses those rows, and prices a search as
    /// before.
    #[test]
    fn a_places_call_google_refused_is_charged_nothing() {
        for refused in [400, 401, 403, 404, 429, 302, 0] {
            assert_eq!(places("POST", "/v1/places:searchText", refused), Usage::Key { key: "google-places".into(), units: 0 }, "{refused}");
            assert_eq!(places("GET", "/v1/places/ChIJ", refused), Usage::Key { key: "google-places:details".into(), units: 0 }, "{refused}");
        }
        // a photo refused, or not modified, made nothing; a redirect is a photo's alone
        for refused in [400, 403, 404, 429, 300, 304, 0] {
            assert_eq!(places("GET", "/v1/places/ChIJ/photos/AUc7/media", refused), Usage::Key { key: "google-places:photo".into(), units: 0 }, "{refused}");
        }
        assert_eq!(places("POST", "/v1/places:searchText", 302), Usage::Key { key: "google-places".into(), units: 0 });
        assert_eq!(places("GET", "/v1/places/ChIJ", 302), Usage::Key { key: "google-places:details".into(), units: 0 });
        let mut book = PriceBook::defaults();
        book.keys = DEFAULT_KEYS.iter().map(|(key, micros, per)| KeyPrice { key: key.to_string(), micros: *micros, per: *per }).collect();
        let older = PriceBook { keys: book.keys.iter().filter(|k| !k.key.starts_with("google-places:")).cloned().collect(), ..book.clone() };
        assert_eq!(older.price(&Usage::Key { key: "google-places:details".into(), units: 1 }), Err(PriceError::NoPrice));
        assert_eq!(older.price(&Usage::Key { key: "google-places".into(), units: 1 }).unwrap().charge, 52_500);
    }

    /// A JSON `POST` on Firecrawl's key, answered `status`, in credits.
    fn firecrawl(method: &str, path: &str, body: &[u8], status: u16) -> Usage {
        key_usage("firecrawl", &KeyCall { method, host: "api.firecrawl.dev", path, body, status })
    }

    /// Goal (FIN-445): a Firecrawl call is charged the credits it costs,
    /// and its status polls nothing. Method: Hermes' `web_extract` and
    /// `web_search` bodies (its Firecrawl provider), and an agent's SDK
    /// calls.
    #[test]
    fn a_firecrawl_call_is_charged_its_credits() {
        let mut book = PriceBook::defaults();
        book.keys = DEFAULT_KEYS.iter().map(|(key, micros, per)| KeyPrice { key: key.to_string(), micros: *micros, per: *per }).collect();
        let credits = |units| Usage::Key { key: "firecrawl:credit".into(), units };
        let extract = br#"{"url":"https://example.com/","formats":["markdown","html"]}"#;
        assert_eq!(firecrawl("POST", "/v2/scrape", extract, 200), credits(1), "Hermes' web_extract");
        assert_eq!(book.price(&credits(1)).unwrap().charge, 7_500, "$0.0075, not a search's $0.015");
        assert_eq!(firecrawl("POST", "/v1/scrape", extract, 200), credits(1));
        assert_eq!(firecrawl("POST", "/v2/scrape", br#"{"url":"u"}"#, 200), credits(1), "markdown alone");
        assert_eq!(firecrawl("POST", "/v2/scrape", br#"{"url":"u","formats":["markdown",{"type":"json","prompt":"p"}]}"#, 200), credits(5));
        assert_eq!(firecrawl("POST", "/v2/scrape", br#"{"url":"u","formats":["json","highlights",{"type":"screenshot"}]}"#, 200), credits(9), "each LLM format 4 more");
        assert_eq!(firecrawl("POST", "/v2/scrape", b"a body the meter does not read", 200), credits(1), "a page, read or not");
        assert_eq!(firecrawl("POST", "/v2/search", br#"{"query":"q","limit":5}"#, 200), credits(2), "Hermes' web_search");
        assert_eq!(firecrawl("POST", "/v2/search", br#"{"query":"q"}"#, 200), credits(2), "10 results");
        assert_eq!(firecrawl("POST", "/v2/search", br#"{"query":"q","limit":11}"#, 200), credits(4), "rounded up per 10");
        assert_eq!(firecrawl("POST", "/v2/search", br#"{"query":"q","limit":10,"sources":["web","news",{"type":"alexandria"}]}"#, 200), credits(4), "per source, alexandria free");
        assert_eq!(firecrawl("POST", "/v2/search", br#"{"query":"q","limit":5,"scrapeOptions":{"formats":["markdown"]}}"#, 200), credits(2 + 5), "and a page each scraped");
        assert_eq!(firecrawl("POST", "/v2/search", br#"{"query":"q","limit":5,"scrapeOptions":{"formats":[{"type":"json"}]}}"#, 200), credits(2 + 5 * 5));
        // FIN-445 review: scraping only with some formats, v1's own limit, each source kind once
        for scrape_options in [r#""#, r#","scrapeOptions":{}"#, r#","scrapeOptions":{"formats":[]}"#] {
            let body = format!(r#"{{"query":"rust","limit":10{scrape_options}}}"#);
            assert_eq!(firecrawl("POST", "/v2/search", body.as_bytes(), 200), credits(2), "{body}");
            assert_eq!(firecrawl("POST", "/v1/search", body.as_bytes(), 200), credits(2), "{body}");
        }
        assert_eq!(firecrawl("POST", "/v1/search", br#"{"query":"q"}"#, 200), credits(2), "v1's 5 results");
        assert_eq!(firecrawl("POST", "/v1/search", br#"{"query":"q","scrapeOptions":{"formats":["markdown"]}}"#, 200), credits(2 + 5));
        assert_eq!(firecrawl("POST", "/v2/search", br#"{"query":"q","scrapeOptions":{"formats":["markdown"]}}"#, 200), credits(2 + 10), "v2's 10");
        assert_eq!(firecrawl("POST", "/v2/search", br#"{"query":"q","limit":10,"sources":["web","web",{"type":"web"}]}"#, 200), credits(2), "web once");
        // each kind of LLM format once a page, a question and a query one kind (FIN-445 review)
        let scraped = |formats: &str| firecrawl("POST", "/v2/scrape", format!(r#"{{"url":"u","formats":{formats}}}"#).as_bytes(), 200);
        assert_eq!(scraped(r#"[{"type":"json","prompt":"name"},{"type":"json","prompt":"name"}]"#), credits(5));
        assert_eq!(scraped(r#"[{"type":"question","question":"q"},{"type":"query","prompt":"q"}]"#), credits(5));
        assert_eq!(scraped(r#"["json",{"type":"highlights"}]"#), credits(9), "two kinds");
        assert_eq!(scraped(r#"["json","extract"]"#), credits(5), "v1's extract is JSON");
        assert_eq!(firecrawl("POST", "/v2/search", br#"{"query":"q","limit":5,"scrapeOptions":{"formats":[{"type":"json"},{"type":"json"}]}}"#, 200), credits(2 + 5 * 5));
        // the tool catalogues are free and make no page
        for sources in [r#"["skills"]"#, r#"[{"type":"skills"}]"#, r#"["skills","alexandria"]"#] {
            for scrape in ["", r#","scrapeOptions":{"formats":[{"type":"json","prompt":"name"}]}"#] {
                let body = format!(r#"{{"query":"q","limit":10,"sources":{sources}{scrape}}}"#);
                assert_eq!(firecrawl("POST", "/v2/search", body.as_bytes(), 200), credits(0), "{body}");
            }
        }
        assert_eq!(firecrawl("POST", "/v2/search", br#"{"query":"q","limit":10,"sources":["skills","web","alexandria"]}"#, 200), credits(2), "web alone");
        assert_eq!(firecrawl("POST", "/v2/map", br#"{"url":"u"}"#, 200), credits(1));
        let free = Usage::Key { key: "firecrawl".into(), units: 0 };
        // a batch is billed as each page completes, as a crawl is: its submit is no page (FIN-445 review)
        for (method, path) in [("GET", "/v2/crawl/0b1e"), ("GET", "/v2/batch/scrape/0b1e"), ("GET", "/v2/extract/0b1e"), ("POST", "/v2/crawl"), ("POST", "/v2/batch/scrape"), ("POST", "/v1/batch/scrape"), ("DELETE", "/v2/batch/scrape/0b1e"), ("POST", "/v2/extract"), ("POST", "/v2/agent"), ("GET", "/v2/team/credit-usage"), ("GET", "/v2/scrape"), ("POST", "/v3/scrape")] {
            assert_eq!(firecrawl(method, path, br#"{"url":"u","urls":["a","b"],"limit":10}"#, 200), free, "{method} {path}");
        }
    }

    /// Goal: a Firecrawl call it refused (or that returned no document) is
    /// charged nothing, and a search the meter cannot count nothing; a
    /// call at most 1,000 credits; an older book refuses the rows.
    #[test]
    fn a_firecrawl_call_refused_or_uncounted_is_charged_nothing() {
        let none = Usage::Key { key: "firecrawl:credit".into(), units: 0 };
        for refused in [400, 401, 402, 403, 404, 408, 429, 302, 0] {
            assert_eq!(firecrawl("POST", "/v2/scrape", br#"{"url":"u"}"#, refused), none, "{refused}");
            assert_eq!(firecrawl("POST", "/v2/search", br#"{"query":"q"}"#, refused), none, "{refused}");
        }
        for body in [&b""[..], b"query=q", br#"{"query":"q","limit":"5"}"#, br#"{"query":"q","limit":-1}"#, br#"{"query":"q","sources":"web"}"#, br#"{"query":"q","sources":[42]}"#, br#"{"query":"q","limit":0}"#] {
            assert_eq!(firecrawl("POST", "/v2/search", body, 200), none, "{}", String::from_utf8_lossy(body));
        }
        let most = Usage::Key { key: "firecrawl:credit".into(), units: 1_000 };
        assert_eq!(firecrawl("POST", "/v2/search", br#"{"query":"q","limit":100,"sources":["web","news","images"],"scrapeOptions":{"formats":["json"]}}"#, 200), most);
        assert_eq!(firecrawl("POST", "/v2/search", br#"{"query":"q","limit":18446744073709551615,"sources":["web","news"]}"#, 200), none, "an overflow is no count");
        let mut book = PriceBook::defaults();
        book.keys = DEFAULT_KEYS.iter().map(|(key, micros, per)| KeyPrice { key: key.to_string(), micros: *micros, per: *per }).collect();
        assert_eq!(book.price(&most).unwrap().charge, 7_500_000, "$5 at list");
        let older = PriceBook { keys: book.keys.iter().filter(|k| k.key != "firecrawl:credit").cloned().collect(), ..book.clone() };
        assert_eq!(older.price(&Usage::Key { key: "firecrawl:credit".into(), units: 1 }), Err(PriceError::NoPrice));
    }

    /// Goal: the margin and the fee are the operator's. Method: a book at
    /// no margin charges its cost basis; at 100% twice it; at no fee the
    /// cost basis is the list.
    #[test]
    fn the_margin_and_fee_are_configurable() {
        let turn = tokens(FLASH, 26_000, 0, 0, 200);
        let mut book = PriceBook::defaults();
        book.margin_bp = 0;
        assert_eq!(book.price(&turn).unwrap(), Priced { list: 4_000, cost: 4_200, charge: 4_200 });
        book.margin_bp = 10_000;
        assert_eq!(book.price(&turn).unwrap().charge, 8_400);
        book.credits_fee_bp = 0;
        assert_eq!(book.price(&turn).unwrap(), Priced { list: 4_000, cost: 4_000, charge: 8_000 });
    }

    /// Goal: what the book cannot price is refused, typed, never charged
    /// at zero. Method: unknown names, bad names, quantities over the
    /// limit, and a charge over `CHARGE_MAX` at the largest quantities.
    #[test]
    fn what_cannot_be_priced_is_refused() {
        let book = PriceBook::defaults();
        assert_eq!(book.price(&tokens("@cf/other", 1, 0, 0, 1)), Err(PriceError::NoPrice));
        assert_eq!(book.price(&Usage::Awake { instance: "8vcpu".into(), ms: 1 }), Err(PriceError::NoPrice));
        assert_eq!(book.price(&Usage::Key { key: "search".into(), units: 1 }), Err(PriceError::NoPrice));
        assert_eq!(book.price(&tokens("", 1, 0, 0, 1)), Err(PriceError::Invalid(UsageFault::Name)));
        assert_eq!(book.price(&tokens("a model", 1, 0, 0, 1)), Err(PriceError::Invalid(UsageFault::Name)));
        assert_eq!(book.price(&tokens(&"m".repeat(NAME_MAX_BYTES + 1), 1, 0, 0, 1)), Err(PriceError::Invalid(UsageFault::Name)));
        assert_eq!(book.price(&tokens(FLASH, QUANTITY_MAX + 1, 0, 0, 0)), Err(PriceError::Invalid(UsageFault::Quantity)));
        assert_eq!(book.price(&Usage::Requests { count: u64::MAX }), Err(PriceError::Invalid(UsageFault::Quantity)));
        // the largest quantities at the largest price: refused, no overflow
        let mut dear = PriceBook::defaults();
        dear.margin_bp = MARGIN_BP_MAX;
        dear.credits_fee_bp = FEE_BP_MAX;
        dear.models[0].price = TokenPrices { input: PRICE_MAX, cached_input: PRICE_MAX, cache_write: PRICE_MAX, output: PRICE_MAX };
        let q = QUANTITY_MAX;
        assert_eq!(dear.price(&tokens(FLASH, q, q, q, q)), Err(PriceError::TooLarge));
        assert_eq!(dear.price(&Usage::DynamicWorkers { count: q }), Err(PriceError::TooLarge));
    }

    /// Goal: a book that breaks a rule is refused with the rule. Method:
    /// one fault of each kind on the defaults.
    #[test]
    fn a_bad_book_is_refused() {
        let fault = |f: fn(&mut PriceBook)| {
            let mut b = PriceBook::defaults();
            f(&mut b);
            b.validate()
        };
        assert_eq!(fault(|b| b.margin_bp = MARGIN_BP_MAX + 1), Err(BookFault::Rate));
        assert_eq!(fault(|b| b.credits_fee_bp = FEE_BP_MAX + 1), Err(BookFault::Rate));
        assert_eq!(fault(|b| b.requests = -1), Err(BookFault::Price));
        assert_eq!(fault(|b| b.models[1].price.output = PRICE_MAX + 1), Err(BookFault::Price));
        assert_eq!(fault(|b| b.keys.push(KeyPrice { key: "k".into(), micros: 1, per: 0 })), Err(BookFault::Price));
        assert_eq!(fault(|b| b.models[1].model = FLASH.into()), Err(BookFault::Name), "a model priced twice");
        assert_eq!(fault(|b| b.instances[0].instance = String::new()), Err(BookFault::Name));
        assert_eq!(
            fault(|b| b.keys = (0..=PRICES_MAX).map(|i| KeyPrice { key: format!("k{i}"), micros: 1, per: 1 }).collect()),
            Err(BookFault::TooMany)
        );
        assert_eq!(fault(|b| b.keys.push(KeyPrice { key: "k".into(), micros: 1, per: 1 })), Ok(()));
    }

    #[test]
    fn money_for_people() {
        assert_eq!(dollars(20 * USD), "$20.00");
        assert_eq!(dollars(40_000), "$0.04");
        assert_eq!(dollars(41_234), "$0.041234");
        assert_eq!(dollars(-2 * USD), "-$2.00");
        assert_eq!(dollars(i64::MIN), "-$9223372036854.775808");
    }

    /// Usages are tagged by kind, in snake_case, and refuse unknown fields.
    #[test]
    fn a_usage_is_tagged_by_kind() {
        let u = tokens(FLASH, 1, 2, 3, 4);
        assert_eq!(
            serde_json::to_value(&u).unwrap(),
            serde_json::json!({ "kind": "tokens", "model": FLASH, "input": 1, "cached_input": 2, "cache_write": 3, "output": 4 })
        );
        let s = Usage::Storage { class: StorageClass::Sqlite, byte_hours: 9 };
        assert_eq!(serde_json::to_value(&s).unwrap(), serde_json::json!({ "kind": "storage", "class": "sqlite", "byte_hours": 9 }));
        assert!(serde_json::from_value::<Usage>(serde_json::json!({ "kind": "requests", "count": 1, "extra": 2 })).is_err());
        assert!(serde_json::from_value::<Usage>(serde_json::json!({ "kind": "requests", "count": -1 })).is_err(), "quantities are unsigned");
        let billed = serde_json::json!({ "kind": "billed", "vendor": "openrouter", "micros": 1 });
        assert!(serde_json::from_value::<Usage>(billed).is_err(), "a vendor's own bill went with OpenRouter: every meter is Cloudflare's");
    }
}
