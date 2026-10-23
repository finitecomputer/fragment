//! What a person used in a month, in units, not money (docs/self-host.md,
//! seam 10): each model's calls and tokens, and their computers' time
//! awake, on the deployment's machines and on their own. Read from the
//! ledger's own rows (its `entries`, each kept `REF_KEEP_MS`, so this
//! month's are whole) whatever each charged: a call on a model priced at
//! nothing (a self-hosted one, its slot's ceiling zero: docs/self-host.md,
//! seam 3) is a row as any is, and so is an hour on a node of the owner's
//! own (`price::OWN_HARDWARE`, priced at nothing). Nothing more is kept:
//! one query of what is, its groups summed here.

use fragment_proto::ledger::{ModelUse, Used};
use serde_json::Value;

use super::Month;
use crate::price;

/// The groups the query reads at most: every model and instance a book
/// prices; more only across books changed within the month, and the rest
/// unread.
pub const GROUPS_MAX: usize = 2 * price::PRICES_MAX;

/// The query over a ledger's `entries` (the cell's SQLite, as its baseline
/// step makes it), bound to a month's first ms, the next month's, and
/// `GROUPS_MAX`: the month's meter rows, tokens grouped by model and awake
/// time by instance, the models with the most calls first. Every other
/// meter's rows, and a reservation of the releases before decision 62, are
/// no use here.
pub const SQL: &str = "SELECT json_extract(entry, '$.row.usage.kind') AS kind,
  COALESCE(json_extract(entry, '$.row.usage.model'), json_extract(entry, '$.row.usage.instance'), '') AS name,
  COUNT(*) AS calls,
  COALESCE(SUM(json_extract(entry, '$.row.usage.input') + json_extract(entry, '$.row.usage.cached_input') + json_extract(entry, '$.row.usage.cache_write')), 0) AS input,
  COALESCE(SUM(json_extract(entry, '$.row.usage.output')), 0) AS output,
  COALESCE(SUM(json_extract(entry, '$.row.usage.ms')), 0) AS ms
FROM entries
WHERE at_ms >= ? AND at_ms < ? AND json_extract(entry, '$.entry') = 'row' AND json_extract(entry, '$.row.usage.kind') IN ('tokens', 'awake')
GROUP BY kind, name ORDER BY kind, calls DESC, name LIMIT ?";

/// `month`'s use from the groups `SQL` read, each an object of its columns
/// (`kind`, `tokens` or `awake`; `name`, a model's id or an instance's;
/// `calls`, `input`, `output`, `ms`), in its order: each model's, and the
/// time awake on the deployment's instances and on the owner's own.
pub fn summed(month: Month, groups: &[Value]) -> Used {
    assert!(groups.len() <= GROUPS_MAX, "the query reads at most GROUPS_MAX groups");
    let n = |g: &Value, k: &str| g[k].as_u64().expect("a group's sums are whole and never negative");
    let mut used = Used { month: month.label(), models: Vec::new(), awake_ms: 0, own_awake_ms: 0 };
    for g in groups {
        let name = g["name"].as_str().expect("a group names its model or instance");
        match g["kind"].as_str() {
            Some("tokens") => used.models.push(ModelUse { model: name.to_string(), calls: n(g, "calls"), input_tokens: n(g, "input"), output_tokens: n(g, "output") }),
            Some("awake") if name == price::OWN_HARDWARE => used.own_awake_ms += n(g, "ms"),
            Some("awake") => used.awake_ms += n(g, "ms"),
            other => panic!("the query reads tokens and awake time alone, not {other:?}"),
        }
    }
    used
}

#[cfg(test)]
mod tests {
    use fragment_proto::ledger::SetPlan;
    use serde_json::json;

    use super::super::{Entry, Ledger, Memory, Meter, MeterRow, Store};
    use super::*;
    use crate::models::Models;
    use crate::price::{PriceBook, StorageClass, Usage, INSTANCE, OWN_HARDWARE};

    const DAY_MS: i64 = 86_400_000;
    const BONSAI: &str = "bonsai-2-27b";
    const FLASH: &str = "@cf/zai-org/glm-5.3-flash";

    /// Goal: a month's first instant is `of`'s inverse, for every month
    /// a ledger meets. Method: each month from 1970 to 2199, the instant
    /// before it is the month before's, and two known firsts.
    #[test]
    fn a_months_first_instant() {
        for m in 0..(230 * 12) {
            let month = Month(m);
            assert_eq!(Month::of(month.start_ms()), month);
            assert!(month.next().start_ms() > month.start_ms());
            if m > 0 {
                assert_eq!(Month::of(month.start_ms() - 1), Month(m - 1), "{}", month.label());
            }
        }
        assert_eq!(Month::parse("2026-10").unwrap().start_ms(), 1_790_812_800_000);
        assert_eq!(Month::parse("2000-03").unwrap().start_ms(), 951_868_800_000);
    }

    /// The book of a self-hosted deployment whose every tier is Bonsai on
    /// the box (docs/self-host.md, seam 3): its slots' ceiling zero, so a
    /// call on it is priced at nothing.
    fn bonsai_book() -> PriceBook {
        let slot = json!({ "provider": "openai", "base_url": "http://127.0.0.1:18087/v1", "model": BONSAI, "body": { "provider": { "max_price": { "prompt": 0, "completion": 0 } } } });
        let named = json!({ "cheap": slot, "medium": slot, "fallback": slot }).to_string();
        Models::parse(Some(&named), &PriceBook::defaults(), true).expect("Bonsai's models").priced(PriceBook::defaults())
    }

    fn row(reference: &str, usage: Usage, at_ms: i64) -> MeterRow {
        MeterRow { reference: reference.into(), usage, fragment: None, agent: Some("hand".into()), computer: None, at_ms }
    }

    fn tokens(model: &str, input: u64, cached_input: u64, output: u64) -> Usage {
        Usage::Tokens { model: model.into(), input, cached_input, cache_write: 0, output }
    }

    /// The store's entries in a ledger's SQLite, as the cell keeps them
    /// (its baseline step's table, its own `INSERT`), then `SQL` over
    /// `month`, summed.
    fn read(store: &Memory, month: Month) -> Used {
        let db = rusqlite::Connection::open_in_memory().expect("an SQLite");
        db.execute_batch(crate::migrate::LEDGER.steps[0].sql).expect("the ledger's tables");
        let mut put = db.prepare("INSERT INTO entries (ref, at_ms, held, entry) VALUES (?, ?, 0, ?)").expect("the insert");
        for (reference, entry) in &store.entries {
            put.execute(rusqlite::params![reference, entry.at_ms(), serde_json::to_string(entry).unwrap()]).expect("an entry");
        }
        let mut q = db.prepare(SQL).expect("the query");
        let groups = q
            .query_map(rusqlite::params![month.start_ms(), month.next().start_ms(), GROUPS_MAX as i64], |r| {
                let (kind, name): (String, String) = (r.get("kind")?, r.get("name")?);
                let [calls, input, output, ms]: [i64; 4] = [r.get("calls")?, r.get("input")?, r.get("output")?, r.get("ms")?];
                Ok(json!({ "kind": kind, "name": name, "calls": calls, "input": input, "output": output, "ms": ms }))
            })
            .expect("the groups")
            .collect::<Result<Vec<_>, _>>()
            .expect("each group");
        summed(month, &groups)
    }

    /// Goal: a person's month in units, whatever it cost: Bonsai's calls,
    /// priced at nothing, are kept with their tokens and counted as any
    /// model's; their computer's hours on their own node, priced at
    /// nothing, apart from those on the deployment's; last month's rows,
    /// other meters' and a reservation from before are not this month's.
    /// Method: a seat's ledger meters each through `Ledger::meter` with a
    /// self-hosted deployment's book, then the cell's query reads its
    /// entries from a real SQLite.
    #[test]
    fn a_months_use_in_units_zero_priced_rows_kept() {
        let book = bonsai_book();
        assert_eq!(book.price(&tokens(BONSAI, 1_000_000, 0, 1_000_000)).unwrap().charge, 0, "Bonsai costs nothing");
        let month = Month::parse("2026-10").unwrap();
        let (start, now) = (month.start_ms(), month.start_ms() + 20 * DAY_MS);
        let mut store = Memory::default();
        let mut ledger = Ledger::new(start - 40 * DAY_MS);
        ledger.set_plan(&mut store, &SetPlan { id: "p".into(), plan: fragment_proto::ledger::Plan::Seat }, start - 40 * DAY_MS).unwrap();
        let last_month = start - DAY_MS;
        let earlier = Meter { batch: "b0".into(), rows: vec![row("aig:0", tokens(BONSAI, 999, 0, 999), last_month), row("awake:c:0", Usage::Awake { instance: INSTANCE.into(), ms: 999 }, last_month)] };
        ledger.meter(&mut store, &earlier, last_month + 1, &book).unwrap();
        let balance = ledger.status(&store, now, &book).balance_micros;
        let rows = vec![
            row("aig:1", tokens(BONSAI, 50, 0, 20), start),
            row("aig:2", tokens(BONSAI, 30, 10, 5), now - 1),
            row("step:f@1:r:0", tokens(FLASH, 100, 0, 7), now - 2),
            row("awake:c:1", Usage::Awake { instance: OWN_HARDWARE.into(), ms: 3_600_000 }, now - 3),
            row("awake:c:2", Usage::Awake { instance: OWN_HARDWARE.into(), ms: 1_800_000 }, now - 4),
            row("awake:d:1", Usage::Awake { instance: INSTANCE.into(), ms: 600_000 }, now - 5),
            row("req:f@1:9", Usage::Requests { count: 40 }, now - 6),
            row("store:f@1:sqlite:9", Usage::Storage { class: StorageClass::Sqlite, byte_hours: 5 }, now - 7),
        ];
        let metered = ledger.meter(&mut store, &Meter { batch: "b1".into(), rows }, now, &book).unwrap();
        let paid = [tokens(FLASH, 100, 0, 7), Usage::Awake { instance: INSTANCE.into(), ms: 600_000 }, Usage::Requests { count: 40 }, Usage::Storage { class: StorageClass::Sqlite, byte_hours: 5 }];
        let charged: i64 = paid.iter().map(|u| book.price(u).unwrap().charge).sum();
        assert_eq!((metered.rows_new, metered.refused.len(), metered.charged), (8, 0, charged), "every row taken, the free ones charging nothing");
        assert_eq!(ledger.status(&store, now, &book).balance_micros, balance - charged);
        for free in ["aig:1", "aig:2", "awake:c:1"] {
            assert!(matches!(store.entry(free), Some(Entry::Row { charge: 0, .. })), "{free} is kept, at nothing");
        }
        // a reservation of the releases before decision 62, this month: no use here
        store.entries.insert("aig:old".into(), serde_json::from_value(json!({ "entry": "reservation", "at_ms": now - 9, "usage": tokens(BONSAI, 1, 0, 1) })).unwrap());

        let used = read(&store, month);
        let model = |model: &str, calls, input_tokens, output_tokens| ModelUse { model: model.into(), calls, input_tokens, output_tokens };
        assert_eq!(
            used,
            Used { month: "2026-10".into(), models: vec![model(BONSAI, 2, 90, 25), model(FLASH, 1, 100, 7)], awake_ms: 600_000, own_awake_ms: 5_400_000 },
            "this month's alone, cached tokens among those in"
        );
        let before = read(&store, Month(month.0 - 1));
        assert_eq!(before, Used { month: "2026-09".into(), models: vec![model(BONSAI, 1, 999, 999)], awake_ms: 999, own_awake_ms: 0 });
        assert_eq!(read(&store, month.next()), Used { month: "2026-11".into(), models: vec![], awake_ms: 0, own_awake_ms: 0 }, "a month with nothing");
    }

    /// Goal: the query lists models by their calls, the most first, then
    /// by id; awake time on any of the deployment's instances is theirs
    /// together. Method: rows of three models and three instances.
    /// Goal: the query lists models by their calls, the most first, then
    /// by id; awake time on any of the deployment's instances is theirs
    /// together. Method: rows of three models and three instances.
    #[test]
    fn models_by_calls_and_every_deployment_instance_together() {
        let mut book = PriceBook::defaults();
        let free = price::TokenPrices { input: 0, cached_input: 0, cache_write: 0, output: 0 };
        book.models.extend(["a", "b", "c"].map(|m| price::ModelPrice { model: m.into(), price: free }));
        book.instances.push(price::InstancePrice { instance: "1vcpu-3gib".into(), hour: 1 });
        let calls = [("a", 1), ("b", 3), ("c", 1)].into_iter().flat_map(|(m, n)| (0..n).map(move |i| row(&format!("aig:{m}{i}"), tokens(m, 10, 0, 1), DAY_MS)));
        let awake = [("1vcpu-3gib", 5), (INSTANCE, 7), (OWN_HARDWARE, 11)].into_iter().enumerate().map(|(i, (instance, ms))| row(&format!("awake:c:{i}"), Usage::Awake { instance: instance.into(), ms }, DAY_MS));
        let (mut ledger, mut store) = (Ledger::new(0), Memory::default());
        ledger.set_plan(&mut store, &SetPlan { id: "p".into(), plan: fragment_proto::ledger::Plan::Seat }, 0).unwrap();
        ledger.meter(&mut store, &Meter { batch: "b".into(), rows: calls.chain(awake).collect() }, DAY_MS, &book).unwrap();
        let used = read(&store, Month(0));
        assert_eq!(used.models.iter().map(|m| (m.model.as_str(), m.calls, m.input_tokens)).collect::<Vec<_>>(), [("b", 3, 30), ("a", 1, 10), ("c", 1, 10)]);
        assert_eq!((used.awake_ms, used.own_awake_ms, used.month.as_str()), (12, 11, "1970-01"));
    }

    #[test]
    #[should_panic(expected = "tokens and awake time alone")]
    fn a_group_of_another_kind_is_the_querys_bug() {
        summed(Month(0), &[json!({ "kind": "requests", "name": "", "calls": 1, "input": 0, "output": 0, "ms": 0 })]);
    }
}
