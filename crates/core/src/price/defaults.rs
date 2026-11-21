//! The default book, at list price, each price with its source.

use super::book::{StoragePrices, TokenPrices};
use super::limits::{BP, CREDITS_FEE_BP_DEFAULT};
use crate::decide::Model;

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
///
/// Workers AI's models from its pricing page (developers.cloudflare.com/
/// workers-ai/platform/pricing/, read 2026-11-16), as each model's page
/// lists them: a model with no cached price has none, so its cached or
/// written token is an input token's price. GLM-5.3 Flash's and DeepSeek V4
/// Flash's rows stay past their retirement on 2026-11-20 (410 Gone from
/// then): the usage stored and queued under them is priced by them still.
pub const DEFAULT_MODELS: [(&str, TokenPrices); 15] = [
    // $0.30 in, $0.006 cached, $1.20 out per million tokens (the cheap tier's, and vision)
    (crate::models::DEEPSEEK_FLASH, TokenPrices { input: 300_000, cached_input: 6_000, cache_write: 300_000, output: 1_200_000 }),
    // $0.30 in, $0.10 cached, $1.00 out (the fallback)
    (crate::models::INKLING_SMALL, TokenPrices { input: 300_000, cached_input: 100_000, cache_write: 300_000, output: 1_000_000 }),
    // $0.10 in, $0.30 out, no cache (an icon's subject: crate::icon::SUBJECT_MODEL)
    (crate::icon::SUBJECT_MODEL, TokenPrices { input: 100_000, cached_input: 100_000, cache_write: 100_000, output: 300_000 }),
    // retired 2026-11-20: $0.15 in, $0.03 cached, $0.50 out (the cheap tier's and vision until then)
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
    // retired 2026-11-20: $0.44 in, $0.014 cached, $1.32 out (the fallback until then)
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
/// raised it, and each still does: 15 for the operator keys' charges by
/// their vendors' own numbers (2026-11-21, FIN-616), after 14 for Workers
/// AI's DeepSeek V4.1 Flash, Inkling 2 Small and Gemma 4 26B (2026-11-16),
/// after 13 for
/// Firecrawl's credits by the call (2026-11-07), after 12 for Google Places' photos apart from its
/// searches (2026-11-07), after 11 for
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
pub const DEFAULT_BOOK_VERSION: u32 = 15;
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

/// One of the price book's charges for an operator key, as a catalog row
/// names one (`catalog::Charge`, which says how each is read): the calls
/// it is for, where its answer says the call's units (a header, or a JSON
/// pointer), their unit, and the unit's list price, `micros` per `per`.
#[derive(Debug, Clone, Copy)]
pub struct DefaultCharge {
    pub calls: &'static [&'static str],
    pub header: Option<&'static str>,
    pub json: Option<&'static str>,
    pub unit: &'static str,
    pub micros: i64,
    pub per: u64,
}

const NO_READ: DefaultCharge = DefaultCharge { calls: &[], header: None, json: None, unit: "", micros: 0, per: 1 };

/// The operator keys' charges by their vendors' own numbers (FIN-616),
/// each vendor's answers read as its docs say (read 2026-11-21), a row of
/// the operator kind taking its name's unless it names its own. A vendor
/// whose answers say nothing is charged by the call, at an estimate where
/// it bills something else. Google Places' answers say nothing, and its
/// calls are charged nothing (2026-11-20: kept uncharged, with a rate
/// limit of its own).
pub const DEFAULT_CHARGES: [(&str, &[DefaultCharge]); 7] = [
    // xAI: every inference answer says what it cost, its tools' calls and
    // images included, as `usage.cost_in_usd_ticks`, 10^10 ticks a dollar,
    // a stream on its last chunk (docs.x.ai/developers/cost-tracking).
    // Only a POST's is read: a stored response read again, a deferred
    // image's or a video's poll says its cost again each time.
    ("xai", &[DefaultCharge { calls: &["POST /**"], json: Some("/usage/cost_in_usd_ticks"), unit: "tick", micros: 1, per: 10_000, ..NO_READ }]),
    // ElevenLabs: text to speech and sound effects say the characters
    // they billed, `character-cost` (elevenlabs.io/docs/api-reference/
    // introduction; its own skills read `x-character-count`), priced at
    // the multilingual models' $0.08 per 1,000 (elevenlabs.io/pricing/api,
    // read 2026-11-07), which the header does not tell from Flash's and
    // Turbo's $0.04. Music says nothing: a call is taken as a minute of
    // music, $0.15; the music skill's jingles are 15 s, its longest 5 min.
    (
        "elevenlabs",
        &[
            DefaultCharge { calls: &["* /**"], header: Some("character-cost"), unit: "character", micros: 80_000, per: 1_000, ..NO_READ },
            DefaultCharge { calls: &["* /**"], header: Some("x-character-count"), unit: "character", micros: 80_000, per: 1_000, ..NO_READ },
            DefaultCharge { calls: &["POST /v1/music", "POST /v1/music/stream", "POST /v1/music/detailed"], unit: "music", micros: 150_000, per: 1, ..NO_READ },
        ],
    ),
    // Firecrawl: a scrape says its credits in `data.metadata.creditsUsed`
    // (v1 and v2), a v2 search in `creditsUsed`
    // (docs.firecrawl.dev/api-reference/v2-openapi.json; its scrape
    // worker); a map says none and costs 1. Hobby's pay-as-you-go: $5 per
    // 1,000 credits (firecrawl.dev/pricing). A crawl's, a batch scrape's
    // and an extract's polls say the credits used so far, again each
    // poll, so are not read.
    (
        "firecrawl",
        &[
            DefaultCharge { calls: &["POST /*/scrape"], json: Some("/data/metadata/creditsUsed"), unit: "credit", micros: 5_000, per: 1, ..NO_READ },
            DefaultCharge { calls: &["POST /v2/search"], json: Some("/creditsUsed"), unit: "credit", micros: 5_000, per: 1, ..NO_READ },
            DefaultCharge { calls: &["POST /*/map"], unit: "credit", micros: 5_000, per: 1, ..NO_READ },
        ],
    ),
    // fal: each answer says its billing units, `x-fal-billable-units`, in
    // its model's own unit (fal.ai/docs/documentation/model-apis/
    // common-parameters), so each model is priced apart: FLUX.2 Klein 9B's
    // "$0.006 per megapixel" (fal.ai/models/fal-ai/flux-2/klein/9b, read
    // 2026-11-07). Another model's units are priced by no charge here.
    ("fal", &[DefaultCharge { calls: &["POST /fal-ai/flux-2/klein/9b"], header: Some("x-fal-billable-units"), unit: "flux-2-klein-9b", micros: 6_000, per: 1, ..NO_READ }]),
    // X API: a read is billed by the resources it returns, $0.005 a post
    // (docs.x.com/x-api/getting-started/pricing), which an answer's
    // `meta.result_count` counts: X bills a post read again within 24
    // hours nothing, and a user $0.010.
    ("x", &[DefaultCharge { calls: &["GET /2/**"], json: Some("/meta/result_count"), unit: "post", micros: 5_000, per: 1, ..NO_READ }]),
    // Perplexity's Search API: $5.00 per 1,000 requests
    // (docs.perplexity.ai/getting-started/pricing); its answer says no cost.
    ("perplexity", &[DefaultCharge { calls: &["* /**"], unit: "call", micros: 5_000_000, per: 1_000, ..NO_READ }]),
    // Browser Use: $0.02 a browser-hour (browser-use.com/pricing); its
    // answers say a session's cost only on its reads and its stop, so a
    // session's or a task's start is taken as five minutes.
    ("browser-use", &[DefaultCharge { calls: &["POST /api/*/sessions", "POST /api/*/tasks", "POST /api/*/runs", "POST /api/*/browsers"], unit: "session", micros: 20_000, per: 12, ..NO_READ }]),
];

/// The names an older release's queued key rows carry, priced as it
/// priced them (its estimates a call, and the parts it read from a
/// request), so a row it sent just before a deploy is charged as it would
/// have been (the debt ledger, "An older release's key rows").
pub const LEGACY_KEYS: [(&str, i64, u64); 13] = [
    ("x", 50_000, 1),
    ("firecrawl", 10_000, 1),
    ("firecrawl:credit", 5_000, 1),
    ("fal", 6_000, 1),
    ("fal:image", 6_000, 1_048_576),
    ("browser-use", 20_000, 12),
    ("perplexity", 5_000_000, 1_000),
    ("google-places", 35_000_000, 1_000),
    ("google-places:photo", 7_000_000, 1_000),
    ("xai", 120_000, 1),
    ("xai:image", 20_000, 1),
    ("elevenlabs", 150_000, 1),
    ("elevenlabs:speech", 80_000, 1_000),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::price::book::{dollars, KeyPrice, PriceBook, Priced};
    use crate::price::testing::{tokens, FLASH};
    use crate::price::usage::{StorageClass, Usage};

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
        assert_eq!(book.version, 15);
        assert_eq!(book.models.len(), 15);
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

    /// Goal: each vendor's own number is charged at its unit's list price
    /// and the margin, rounded up once a row: xAI's ticks (10^10 a dollar),
    /// ElevenLabs' characters, Firecrawl's credits, Klein's units, X's
    /// posts, and the calls of the vendors whose answers say nothing.
    #[test]
    fn each_vendors_number_is_charged_at_its_units_list_price() {
        let mut book = PriceBook::defaults();
        for (vendor, charges) in DEFAULT_CHARGES {
            for c in charges {
                let key = format!("{vendor}:{}", c.unit);
                if !book.keys.iter().any(|k| k.key == key) {
                    book.keys.push(KeyPrice { key, micros: c.micros, per: c.per });
                }
            }
        }
        assert_eq!(book.validate(), Ok(()));
        let charged = |key: &str, units| book.price(&Usage::Key { key: key.into(), units }).unwrap();
        // an X search through Grok that cost $0.1234567: 123,456.7 micro-dollars, rounded up once
        assert_eq!(charged("xai:tick", 1_234_567_000), Priced { list: 123_457, cost: 123_457, charge: 185_186 });
        assert_eq!(charged("xai:tick", 1), Priced { list: 1, cost: 1, charge: 1 }, "a tick is charged a micro-dollar at least");
        assert_eq!(charged("elevenlabs:character", 200), Priced { list: 16_000, cost: 16_000, charge: 24_000 }, "a 200-character reply, $0.08 per 1,000");
        assert_eq!(charged("elevenlabs:music", 1), Priced { list: 150_000, cost: 150_000, charge: 225_000 }, "a minute of music");
        assert_eq!(charged("firecrawl:credit", 5), Priced { list: 25_000, cost: 25_000, charge: 37_500 }, "a scrape of 5 credits");
        assert_eq!(charged("fal:flux-2-klein-9b", 1), Priced { list: 6_000, cost: 6_000, charge: 9_000 }, "a megapixel of Klein");
        assert_eq!(charged("x:post", 7), Priced { list: 35_000, cost: 35_000, charge: 52_500 }, "a search that returned 7 posts");
        assert_eq!(charged("perplexity:call", 1), Priced { list: 5_000, cost: 5_000, charge: 7_500 }, "$5 per 1,000 searches");
        assert_eq!(charged("browser-use:session", 1), Priced { list: 1_667, cost: 1_667, charge: 2_500 }, "five browser minutes");
        // an older release's rows, priced as it priced them
        let mut older = book.clone();
        // Firecrawl's credit is both an older release's name and a charge's, at one price
        let new_names: Vec<String> = book.keys.iter().map(|k| k.key.clone()).collect();
        older.keys.extend(LEGACY_KEYS.iter().filter(|(name, ..)| !new_names.iter().any(|n| n == name)).map(|(name, micros, per)| KeyPrice { key: name.to_string(), micros: *micros, per: *per }));
        assert_eq!(LEGACY_KEYS.iter().find(|(n, ..)| *n == "firecrawl:credit").map(|(_, m, p)| (*m, *p)), Some((5_000, 1)));
        assert_eq!(older.validate(), Ok(()));
        let legacy = |key: &str, units| older.price(&Usage::Key { key: key.into(), units }).unwrap().charge;
        assert_eq!(legacy("elevenlabs:speech", 200), charged("elevenlabs:character", 200).charge, "speech's characters as before");
        assert_eq!(legacy("perplexity", 1), charged("perplexity:call", 1).charge);
    }
}
