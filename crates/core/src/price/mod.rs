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

mod book;
mod defaults;
mod instance;
mod keys;
mod limits;
mod reported;
#[cfg(test)]
mod testing;
mod usage;

pub use book::{dollars, BookFault, InstancePrice, KeyPrice, ModelPrice, PriceBook, PriceError, Priced, StoragePrices, TokenPrices};
pub use defaults::{openrouter, DefaultCharge, DEFAULT_BOOK_VERSION, DEFAULT_CHARGES, DEFAULT_BROWSER, DEFAULT_DYNAMIC_WORKERS, DEFAULT_IMAGES, DEFAULT_INSTANCES, DEFAULT_KEYS, DEFAULT_MODELS, DEFAULT_NEURONS, DEFAULT_REQUESTS, DEFAULT_STORAGE, INSTANCE, OPENROUTER_FEE_BP};
pub use instance::{instance_size, InstanceSize, DISK_MB};
pub use keys::{default_key_price, key_usage, KeyCall, KeyPart, KEY_PARTS, KEY_PART_BODY_MAX_BYTES};
pub use limits::{AWAKE_MS_PER, BP, BROWSER_MS_PER, CHARGE_MAX, CREDITS_FEE_BP_DEFAULT, FEE_BP_MAX, IMAGES_PER, MARGIN_BP_DEFAULT, MARGIN_BP_MAX, MILLI_NEURONS_PER, NAME_MAX_BYTES, PRICES_MAX, PRICE_MAX, QUANTITY_MAX, REQUESTS_PER, STORAGE_BYTE_HOURS_PER, TOKENS_PER, USD};
pub use reported::{pointer_segments, POINTER_SEGMENTS_MAX};
pub use usage::{printable, StorageClass, Usage, UsageFault};
