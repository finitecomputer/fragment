//! The book's bounds, and the unit each meter is priced per.

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
pub(super) const NUMERATOR_MAX: i128 = 4 * (QUANTITY_MAX as i128) * (PRICE_MAX as i128) * ((BP + FEE_BP_MAX) as i128) * ((BP + MARGIN_BP_MAX) as i128);
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
