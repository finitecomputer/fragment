//! A quote (FIN-448; docs/ledger.md, "A quote"): what a planned job would
//! be charged, so an agent can say "about $4.80, and you have $95 left this
//! month" before it starts. Each item is priced as the ledger prices a row
//! (`row_charge`, the one function both call) with the book the answering
//! release charges with (`Config::book`), so a quote reads whatever keys and
//! parts the book has (`price::KEY_PARTS`) and copies no price. A name the
//! book does not price is said to be unknown, never guessed. A quote holds
//! and keeps nothing.

use fragment_proto::ledger::{Plan, Planned, Quoted, Unquoted, QUOTE_CALLS_MAX, QUOTE_ITEMS_MAX};

use super::row_charge;
use crate::models::Models;
use crate::price::{PriceBook, PriceError, Usage, CHARGE_MAX, KEY_PARTS};

const _: () = assert!(
    (QUOTE_ITEMS_MAX as i128) * (QUOTE_CALLS_MAX as i128) * (CHARGE_MAX as i128) <= i64::MAX as i128,
    "a quote's total fits an i64: its items, each at most its calls of the largest row"
);

/// Why a quote is refused whole (400): its items are not one a person
/// plans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteFault {
    /// More than `QUOTE_ITEMS_MAX` items.
    TooMany,
    /// An item's `calls` is 0 or more than `QUOTE_CALLS_MAX`.
    Calls,
}

impl std::fmt::Display for QuoteFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QuoteFault::TooMany => write!(f, "a quote prices at most {QUOTE_ITEMS_MAX} items"),
            QuoteFault::Calls => write!(f, "an item's calls are 1 to {QUOTE_CALLS_MAX} (absent, one)"),
        }
    }
}

/// An item's calls: absent, one.
fn calls_of(item: &Planned) -> Option<u64> {
    match item {
        Planned::Key(k) => k.calls,
        Planned::Model(m) => m.calls,
        Planned::Usage(u) => u.calls,
    }
}

/// Whether `items` are a quote's: at most `QUOTE_ITEMS_MAX`, each of 1 to
/// `QUOTE_CALLS_MAX` calls. Checked before the payer's ledger is asked.
pub fn check(items: &[Planned]) -> Result<(), QuoteFault> {
    if items.len() > QUOTE_ITEMS_MAX {
        return Err(QuoteFault::TooMany);
    }
    if !items.iter().all(|i| calls_of(i).is_none_or(|c| (1..=QUOTE_CALLS_MAX).contains(&c))) {
        return Err(QuoteFault::Calls);
    }
    Ok(())
}

/// What `items` would be charged on a ledger of `plan`, priced with `book`,
/// a model named by the route's names resolved by `models` as the model
/// route resolves them: each item's quote, in order, and the priced ones'
/// total. `items` pass `check` first.
pub fn quote(items: &[Planned], book: &PriceBook, models: &Models, plan: Plan) -> (Vec<Quoted>, i64) {
    assert_eq!(check(items), Ok(()), "a quote's items are checked before they are priced");
    let quoted: Vec<Quoted> = items.iter().map(|item| quote_one(item, book, models, plan)).collect();
    let total = quoted.iter().filter_map(|q| q.charge_micros).sum();
    (quoted, total)
}

fn quote_one(item: &Planned, book: &PriceBook, models: &Models, plan: Plan) -> Quoted {
    let calls = calls_of(item).unwrap_or(1);
    let unknown = |priced_as: Option<String>, why| Quoted { priced_as, each_micros: None, calls, charge_micros: None, unknown: Some(why) };
    let usage = match usage_of(item, models) {
        Ok(usage) => usage,
        Err((priced_as, why)) => return unknown(priced_as, why),
    };
    let priced_as = Some(name_of(&usage));
    match row_charge(&usage, book, plan) {
        Ok((_, each)) => {
            let charge = i64::try_from(calls).ok().and_then(|c| each.checked_mul(c)).expect("QUOTE_CALLS_MAX × CHARGE_MAX fits an i64");
            Quoted { priced_as, each_micros: Some(each), calls, charge_micros: Some(charge), unknown: None }
        }
        Err(PriceError::NoPrice) => unknown(priced_as, Unquoted::NoPrice),
        Err(PriceError::Invalid(_)) => unknown(priced_as, Unquoted::Invalid),
        Err(PriceError::TooLarge) => unknown(priced_as, Unquoted::TooLarge),
    }
}

/// One call of `item` as the ledger's row of it: a key's or a part's
/// units, a model's tokens on the id its name runs (a route's name, as the
/// model route reads it; else the book's id), or a usage as a meter counts
/// it. Else why it is none.
fn usage_of(item: &Planned, models: &Models) -> Result<Usage, (Option<String>, Unquoted)> {
    match item {
        Planned::Key(k) => {
            // a part that counts what its vendor does (characters, pixels,
            // credits) is quoted at the units said; one that counts at most
            // one a call (an image, a photo), as any key, a call's one
            let counted = KEY_PARTS.iter().any(|p| p.priced_as == k.key && p.units_max > 1);
            match (k.units, counted) {
                (None, true) => Err((Some(k.key.clone()), Unquoted::NoUnits)),
                (units, _) => Ok(Usage::Key { key: k.key.clone(), units: units.unwrap_or(1) }),
            }
        }
        Planned::Model(m) => {
            let id = match models.named(Some(&m.model), None) {
                Ok(named) => models.route(named).map_err(|_| (Some(m.model.clone()), Unquoted::NoPrice))?.id().to_string(),
                Err(_) => m.model.clone(),
            };
            Ok(Usage::Tokens { model: id, input: m.input, cached_input: m.cached_input, cache_write: m.cache_write, output: m.output })
        }
        Planned::Usage(u) => serde_json::from_value(u.usage.clone()).map_err(|_| (None, Unquoted::Invalid)),
    }
}

/// What the book names a usage by: its key's or model's or instance's
/// name, else its kind.
fn name_of(usage: &Usage) -> String {
    match usage {
        Usage::Key { key, .. } => key.clone(),
        Usage::Tokens { model, .. } => model.clone(),
        Usage::Awake { instance, .. } => instance.clone(),
        other => serde_json::to_value(other).expect("a usage serializes")["kind"].as_str().expect("a usage has its kind").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::sim::{apply, Op, Outcome, T0};
    use crate::ledger::{Ledger, Memory, Meter, MeterRow};
    use crate::price::{KeyPrice, StorageClass};
    use fragment_proto::ledger::{PlannedKey, PlannedModel, PlannedUsage, SetPlan};
    use serde_json::json;

    /// The default book with operator keys and every part of theirs
    /// (`KEY_PARTS`), as a deployment offering them prices them
    /// (`Catalog::key_prices`).
    fn book() -> PriceBook {
        let mut book = PriceBook::defaults();
        let parts = KEY_PARTS.iter().map(|p| p.priced_as);
        for name in ["elevenlabs", "perplexity", "fal", "xai", "google-places", "firecrawl"].into_iter().chain(parts) {
            let (micros, per) = crate::price::default_key_price(name).unwrap();
            book.keys.push(KeyPrice { key: name.into(), micros, per });
        }
        book
    }

    fn key(name: &str, units: Option<u64>, calls: Option<u64>) -> Planned {
        Planned::Key(PlannedKey { key: name.into(), units, calls })
    }

    fn model(name: &str, input: u64, cached_input: u64, output: u64, calls: Option<u64>) -> Planned {
        Planned::Model(PlannedModel { model: name.into(), input, cached_input, cache_write: 0, output, calls })
    }

    fn usage(v: serde_json::Value, calls: Option<u64>) -> Planned {
        Planned::Usage(PlannedUsage { usage: v, calls })
    }

    /// A seat's ledger (or another plan's), whose rows the tests meter.
    struct Payer {
        ledger: Ledger,
        store: Memory,
        book: PriceBook,
        n: u32,
    }

    impl Payer {
        fn on(plan: Plan) -> Payer {
            let mut p = Payer { ledger: Ledger::new(T0), store: Memory::default(), book: book(), n: 0 };
            apply(&mut p.ledger, &mut p.store, &Op::Plan(SetPlan { id: "plan:1".into(), plan }), T0, &mut p.book).unwrap();
            p
        }

        /// What the ledger charges for `calls` rows of `usage`, each a
        /// call's own batch, as the swap and the model route send them.
        fn charged(&mut self, usage: &Usage, calls: u64) -> i64 {
            let before = self.ledger.status(&self.store, T0, &self.book).balance_micros;
            for _ in 0..calls {
                self.n += 1;
                let reference = format!("key:c:{}", self.n);
                let row = MeterRow { reference: reference.clone(), usage: usage.clone(), fragment: None, agent: None, computer: None, at_ms: T0 };
                let answer = apply(&mut self.ledger, &mut self.store, &Op::Meter(Meter { batch: reference, rows: vec![row] }), T0, &mut self.book).unwrap();
                let Outcome::Metered(m) = answer else { panic!("a batch answers Metered") };
                assert_eq!((m.rows_new, m.refused.len()), (1, 0), "{usage:?} is charged");
            }
            before - self.ledger.status(&self.store, T0, &self.book).balance_micros
        }
    }

    /// Goal: each kind of item is quoted exactly what the ledger charges
    /// for its calls: a key's, a part's by its units, a model's tokens by a
    /// route's name and by an id, and any other usage. Method: quote each,
    /// then meter the same rows on a seat's ledger, a call a batch, and
    /// compare its balance's fall with the quote, item by item and whole.
    #[test]
    fn each_item_is_quoted_what_the_ledger_charges_for_it() {
        let models = Models::defaults();
        let cheap = models.route(models.named(Some("cheap"), None).unwrap()).unwrap().id().to_string();
        let items = [
            // 300 clips of about 200 characters, and an audiobook's 60,000 in one
            key("elevenlabs:speech", Some(200), Some(300)),
            key("elevenlabs:speech", Some(60_000), None),
            // the key's other calls (a minute of music), and a search
            key("elevenlabs", None, Some(3)),
            key("perplexity", None, Some(7)),
            // FIN-445's parts: fal's megapixel image, xAI's X search and its
            // image (one a call, so none said), a Places photo, Firecrawl's credits
            key("fal:image", Some(1_048_576), Some(40)),
            key("xai", None, Some(5)),
            key("xai:image", None, Some(4)),
            key("google-places:photo", None, Some(10)),
            key("firecrawl:credit", Some(5), Some(20)),
            // 40 turns on the cheap tier, by its name and by its id
            model("cheap", 2_000, 300, 500, Some(40)),
            model(&cheap, 2_000, 300, 500, Some(40)),
            // a decision's model, which no chat slot runs, by the book's id
            model(crate::decide::Model::ClefFlash.id(), 1_000, 0, 0, Some(2)),
            // an hour awake, and a day of a GB in R2
            usage(json!({ "kind": "awake", "instance": crate::price::INSTANCE, "ms": 3_600_000 }), None),
            usage(json!({ "kind": "storage", "class": "r2", "byte_hours": 24_000_000_000u64 }), Some(5)),
        ];
        let mut payer = Payer::on(Plan::Seat);
        let (quoted, total) = quote(&items, &payer.book.clone(), &models, Plan::Seat);
        let rows = [
            (Usage::Key { key: "elevenlabs:speech".into(), units: 200 }, 300),
            (Usage::Key { key: "elevenlabs:speech".into(), units: 60_000 }, 1),
            (Usage::Key { key: "elevenlabs".into(), units: 1 }, 3),
            (Usage::Key { key: "perplexity".into(), units: 1 }, 7),
            (Usage::Key { key: "fal:image".into(), units: 1_048_576 }, 40),
            (Usage::Key { key: "xai".into(), units: 1 }, 5),
            (Usage::Key { key: "xai:image".into(), units: 1 }, 4),
            (Usage::Key { key: "google-places:photo".into(), units: 1 }, 10),
            (Usage::Key { key: "firecrawl:credit".into(), units: 5 }, 20),
            (Usage::Tokens { model: cheap.clone(), input: 2_000, cached_input: 300, cache_write: 0, output: 500 }, 40),
            (Usage::Tokens { model: cheap.clone(), input: 2_000, cached_input: 300, cache_write: 0, output: 500 }, 40),
            (Usage::Tokens { model: crate::decide::Model::ClefFlash.id().into(), input: 1_000, cached_input: 0, cache_write: 0, output: 0 }, 2),
            (Usage::Awake { instance: crate::price::INSTANCE.into(), ms: 3_600_000 }, 1),
            (Usage::Storage { class: StorageClass::R2, byte_hours: 24_000_000_000 }, 5),
        ];
        let mut charged_all = 0;
        for ((q, (row, calls)), item) in quoted.iter().zip(&rows).zip(&items) {
            let charged = payer.charged(row, *calls);
            assert_eq!(q.charge_micros, Some(charged), "{item:?} is quoted what the ledger charged");
            assert_eq!((q.calls, q.unknown), (*calls, None), "{item:?}");
            assert!(charged > 0, "{item:?} costs something");
            charged_all += charged;
        }
        assert_eq!(total, charged_all, "the total is what the whole job was charged");
        // the names each was priced as: a route's name resolved to its model's id
        let names: Vec<_> = quoted.iter().map(|q| q.priced_as.clone().unwrap()).collect();
        assert_eq!(names, ["elevenlabs:speech", "elevenlabs:speech", "elevenlabs", "perplexity", "fal:image", "xai", "xai:image", "google-places:photo", "firecrawl:credit", cheap.as_str(), cheap.as_str(), crate::decide::Model::ClefFlash.id(), crate::price::INSTANCE, "storage"]);
        // FIN-443's example: 60,000 characters at $0.08 per 1,000 with the 50% margin
        assert_eq!(quoted[1].charge_micros, Some(7_200_000));
        assert_eq!(quoted[0].charge_micros, quoted[1].charge_micros, "300 clips of 200 characters cost what 60,000 do");
        // a megapixel image at $0.006, an X search at $0.12, each with the margin
        assert_eq!((quoted[4].each_micros, quoted[5].each_micros), (Some(9_000), Some(180_000)));
    }

    /// Goal: the always-on seat's awake time is quoted at nothing, as its
    /// ledger charges it (decision 25). Method: the same hour on both seats.
    #[test]
    fn an_always_on_seats_awake_time_is_quoted_free() {
        let hour = [usage(json!({ "kind": "awake", "instance": crate::price::INSTANCE, "ms": 3_600_000 }), None)];
        let models = Models::defaults();
        for (plan, each) in [(Plan::Seat, 64_224 * 3 / 2), (Plan::SeatAlwaysOn, 0)] {
            let mut payer = Payer::on(plan);
            let (quoted, total) = quote(&hour, &payer.book.clone(), &models, plan);
            let charged = payer.charged(&Usage::Awake { instance: crate::price::INSTANCE.into(), ms: 3_600_000 }, 1);
            assert_eq!((quoted[0].each_micros, total, charged), (Some(each), each, each), "{plan:?}");
        }
    }

    /// Goal: what the book does not price is said so, never guessed, and
    /// counts nothing in the total. Method: a key the deployment does not
    /// offer, a part without its units, a model no slot runs and the book
    /// has not, the refused `high` tier, usages that are none, and a row
    /// past any one's charge, beside one priced item.
    #[test]
    fn an_unknown_item_is_said_so_and_counts_nothing() {
        let models = Models::defaults();
        let items = [
            key("perplexity", None, None),
            key("nope", None, Some(4)),
            key("elevenlabs:speech", None, Some(4)),
            key("fal:image", None, None),
            key("firecrawl:credit", None, None),
            model("gpt-nope", 10, 0, 10, None),
            model("high", 10, 0, 10, None),
            usage(json!({ "kind": "awake", "instance": "16vcpu-64gib", "ms": 1 }), None),
            usage(json!({ "kind": "spending", "micros": 5 }), None),
            usage(json!({ "kind": "key", "key": "two words", "units": 1 }), None),
            usage(json!({ "kind": "requests", "count": crate::price::QUANTITY_MAX + 1 }), None),
            key("elevenlabs:speech", Some(crate::price::QUANTITY_MAX), None),
        ];
        let (quoted, total) = quote(&items, &book(), &models, Plan::Seat);
        let why: Vec<_> = quoted.iter().map(|q| q.unknown).collect();
        use Unquoted::*;
        assert_eq!(why, [None, Some(NoPrice), Some(NoUnits), Some(NoUnits), Some(NoUnits), Some(NoPrice), Some(NoPrice), Some(NoPrice), Some(Invalid), Some(Invalid), Some(Invalid), Some(TooLarge)]);
        assert!(quoted[1..].iter().all(|q| q.charge_micros.is_none() && q.each_micros.is_none()), "{quoted:?}");
        assert_eq!(total, quoted[0].charge_micros.unwrap(), "only the priced item counts");
        assert_eq!((quoted[1].calls, quoted[2].priced_as.as_deref()), (4, Some("elevenlabs:speech")), "it says what was asked");
        // a deployment that offers no ElevenLabs prices none of it: its part unknown too
        let (bare, _) = quote(&[key("elevenlabs:speech", Some(200), None)], &PriceBook::defaults(), &models, Plan::Seat);
        assert_eq!(bare[0].unknown, Some(NoPrice));
    }

    /// Invalid: too many items, or calls out of bounds, are refused whole;
    /// none and the bounds themselves pass.
    #[test]
    fn a_quotes_bounds() {
        assert_eq!(check(&[]), Ok(()), "a quote of nothing reads the ledger alone");
        let one = key("perplexity", None, Some(QUOTE_CALLS_MAX));
        assert_eq!(check(&vec![one.clone(); QUOTE_ITEMS_MAX]), Ok(()));
        assert_eq!(check(&vec![one; QUOTE_ITEMS_MAX + 1]), Err(QuoteFault::TooMany));
        for calls in [0, QUOTE_CALLS_MAX + 1] {
            assert_eq!(check(&[key("perplexity", None, Some(calls))]), Err(QuoteFault::Calls), "{calls}");
            assert_eq!(check(&[model("cheap", 1, 0, 1, Some(calls))]), Err(QuoteFault::Calls));
            assert_eq!(check(&[usage(json!({ "kind": "requests", "count": 1 }), Some(calls))]), Err(QuoteFault::Calls));
        }
        // the largest quote's total fits: every item its most calls of the dearest row
        let dearest = PriceBook { keys: vec![KeyPrice { key: "dear".into(), micros: CHARGE_MAX * 2 / 3, per: 1 }], ..PriceBook::defaults() };
        let (quoted, total) = quote(&vec![key("dear", None, Some(QUOTE_CALLS_MAX)); QUOTE_ITEMS_MAX], &dearest, &Models::defaults(), Plan::Seat);
        assert!(quoted.iter().all(|q| q.unknown.is_none()), "{:?}", quoted[0]);
        assert_eq!(total, QUOTE_ITEMS_MAX as i64 * QUOTE_CALLS_MAX as i64 * quoted[0].each_micros.unwrap());
    }
}
