//! The price book and the charge it makes of a usage: list, cost basis and margin, each rounded up once.

use super::defaults::{DEFAULT_BOOK_VERSION, DEFAULT_BROWSER, DEFAULT_DYNAMIC_WORKERS, DEFAULT_IMAGES, DEFAULT_INSTANCES, DEFAULT_MODELS, DEFAULT_NEURONS, DEFAULT_REQUESTS, DEFAULT_STORAGE};
use super::limits::{AWAKE_MS_PER, BP, BROWSER_MS_PER, CHARGE_MAX, CREDITS_FEE_BP_DEFAULT, FEE_BP_MAX, IMAGES_PER, MARGIN_BP_DEFAULT, MARGIN_BP_MAX, MILLI_NEURONS_PER, NAME_MAX_BYTES, NUMERATOR_MAX, PRICES_MAX, PRICE_MAX, QUANTITY_MAX, REQUESTS_PER, STORAGE_BYTE_HOURS_PER, TOKENS_PER, USD};
use super::usage::{printable, StorageClass, Usage, UsageFault};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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
    /// The catalog id the intercept calls (`@cf/deepseek-ai/deepseek-v4.1-flash-1017`).
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
    use crate::price::testing::{tokens, FLASH};

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
}
