//! `fragment quote <name> <amount>… [<name> <amount>…]` (FIN-448): what a
//! planned job would cost, priced by the platform with the book its ledger
//! charges with, and what the payer has left (docs/api.md, Ledger: `POST
//! /api/ledger/quote`), as whoever the CLI signs as (an agent: its owner's
//! ledger). It reads and spends nothing. `fragment mcp` serves the same as
//! its `fragment-quote` tool.
//!
//! The friendly form is items one after another, each a name and its
//! amounts:
//!
//! - a name: an operator key, a part of one (`elevenlabs:speech`), or a
//!   model (`cheap`, `medium`, a model's id);
//! - `<n>`, `<n>x` or `x<n>`: its calls (absent, one);
//! - `<n>in`, `<n>cached`, `<n>write`, `<n>out`: a model's tokens a call;
//! - `<n>` and any other word (`60000chars`, `1048576px`): a key's units a
//!   call, in its own unit (a part that counts more than one a call must
//!   say them: speech's characters, an image's pixels).
//!
//! `fragment quote elevenlabs:speech 60000chars`, `fragment quote fal:image
//! 40x 1048576px`, `fragment quote cheap 2000in 500out 40x`.

use anyhow::Result;
use fragment_core::price::dollars;
use fragment_proto::ledger::{Planned, PlannedKey, PlannedModel, Quote, QuoteAsk, Standing, Unquoted, QUOTE_CALLS_MAX, QUOTE_ITEMS_MAX};
use serde_json::{json, Value};

use crate::api::Client;

/// `fragment mcp`'s quote tool: a name no operation can have (an
/// operation's is `a-z`, `0-9` and `_`: `valid_op_name`).
pub const TOOL: &str = "fragment-quote";

/// One amount of an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Amount {
    Calls(u64),
    Units(u64),
    Input(u64),
    Cached(u64),
    Write(u64),
    Output(u64),
}

/// `word` as an amount (`60000chars`, `40x`, `x40`, `2,000in`), or none
/// when it is a name (it does not start with a digit, or `x` and one).
fn amount(word: &str) -> Option<Result<Amount, String>> {
    let (digits, unit, calls_first) = match word.strip_prefix('x') {
        Some(rest) if rest.starts_with(|c: char| c.is_ascii_digit()) => (rest, "", true),
        _ if word.starts_with(|c: char| c.is_ascii_digit()) => {
            let end = word.find(|c: char| !(c.is_ascii_digit() || c == ',' || c == '_')).unwrap_or(word.len());
            (&word[..end], &word[end..], false)
        }
        _ => return None,
    };
    let n: u64 = match digits.replace([',', '_'], "").parse() {
        Ok(n) => n,
        Err(_) => return Some(Err(format!("{word:?} is no amount: a whole number, then its unit"))),
    };
    Some(Ok(match unit.to_ascii_lowercase().as_str() {
        _ if calls_first => Amount::Calls(n),
        "" | "x" | "call" | "calls" => Amount::Calls(n),
        "in" | "input" => Amount::Input(n),
        "cached" => Amount::Cached(n),
        "write" | "written" => Amount::Write(n),
        "out" | "output" => Amount::Output(n),
        u if u.chars().all(|c| c.is_ascii_alphabetic()) => Amount::Units(n),
        _ => return Some(Err(format!("{word:?} is no amount: a whole number, then its unit (chars, in, out, x)"))),
    }))
}

/// The items `words` name, in the friendly form above.
pub fn parse(words: &[String]) -> Result<Vec<Planned>, String> {
    let mut items: Vec<(String, Vec<Amount>)> = vec![];
    for word in words {
        match amount(word) {
            None => items.push((word.clone(), vec![])),
            Some(a) => match items.last_mut() {
                Some((_, amounts)) => amounts.push(a?),
                None => return Err(format!("{word:?} comes before any name: name what it counts first (elevenlabs:speech {word})")),
            },
        }
    }
    if items.len() > QUOTE_ITEMS_MAX {
        return Err(format!("a quote prices at most {QUOTE_ITEMS_MAX} items"));
    }
    items.into_iter().map(|(name, amounts)| item(name, &amounts)).collect()
}

/// One item: a model's when it names tokens, else a key's.
fn item(name: String, amounts: &[Amount]) -> Result<Planned, String> {
    let once = |pick: fn(&Amount) -> Option<u64>, what: &str| -> Result<Option<u64>, String> {
        let mut found = amounts.iter().filter_map(pick);
        let first = found.next();
        match found.next() {
            Some(_) => Err(format!("{name}: its {what} are named twice")),
            None => Ok(first),
        }
    };
    let calls = once(|a| if let Amount::Calls(n) = a { Some(*n) } else { None }, "calls")?;
    if calls.is_some_and(|c| !(1..=QUOTE_CALLS_MAX).contains(&c)) {
        return Err(format!("{name}: its calls are 1 to {QUOTE_CALLS_MAX}"));
    }
    let units = once(|a| if let Amount::Units(n) = a { Some(*n) } else { None }, "units")?;
    let input = once(|a| if let Amount::Input(n) = a { Some(*n) } else { None }, "input tokens")?;
    let cached = once(|a| if let Amount::Cached(n) = a { Some(*n) } else { None }, "cached tokens")?;
    let write = once(|a| if let Amount::Write(n) = a { Some(*n) } else { None }, "written tokens")?;
    let output = once(|a| if let Amount::Output(n) = a { Some(*n) } else { None }, "output tokens")?;
    let tokens = [input, cached, write, output];
    match (units, tokens.iter().any(Option::is_some)) {
        (Some(_), true) => Err(format!("{name}: a key's units or a model's tokens, not both")),
        (_, false) => Ok(Planned::Key(PlannedKey { key: name, units, calls })),
        (None, true) => {
            let [input, cached_input, cache_write, output] = tokens.map(|t| t.unwrap_or(0));
            Ok(Planned::Model(PlannedModel { model: name, input, cached_input, cache_write, output, calls }))
        }
    }
}

/// `items` quoted by the platform.
pub fn quote(client: &Client, items: Vec<Planned>) -> Result<Quote> {
    client.call_as(client.post_json("/api/ledger/quote", &QuoteAsk { items })?)
}

/// What an item is called, for a person.
fn named(item: &Planned) -> String {
    match item {
        Planned::Key(k) => k.key.clone(),
        Planned::Model(m) => m.model.clone(),
        Planned::Usage(u) => u.usage["kind"].as_str().unwrap_or("usage").to_string(),
    }
}

/// Why an item has no price, for a person.
fn why(u: Unquoted) -> &'static str {
    match u {
        Unquoted::NoPrice => "not priced here (no such key or model in this platform's price book)",
        Unquoted::NoUnits => "say each call's units (its vendor's count: characters, pixels): `60000chars`",
        Unquoted::Invalid => "no usage the ledger counts",
        Unquoted::TooLarge => "one call past any one charge",
    }
}

/// The answer as a person reads it: the total and what is left in one
/// line ("about $4.80; you have $95.40 left this month"; nothing asked,
/// what is left alone), then each item's
/// charge when there are several, each unpriced one's reason, and what is
/// stopped, if anything.
pub fn said(asked: &[Planned], q: &Quote) -> Vec<String> {
    let unknown = q.items.iter().any(|i| i.unknown.is_some());
    let what = if unknown { " for what is priced" } else { "" };
    let left = match q.ledger.balance_micros {
        b if b < 0 => format!("you owe {}", dollars(-b)),
        _ => format!("you have {} left this month", dollars(q.left_micros)),
    };
    let mut lines = vec![match asked.is_empty() {
        true => left,
        false => format!("about {}{what}; {left}", dollars(q.total_micros)),
    }];
    for (item, quoted) in asked.iter().zip(&q.items) {
        match (quoted.charge_micros, quoted.unknown) {
            (Some(c), _) if asked.len() > 1 => lines.push(format!("  {}\t{} × {}", named(item), quoted.calls, dollars(quoted.each_micros.unwrap_or(c)))),
            (None, Some(u)) => lines.push(format!("  {}: {}", named(item), why(u))),
            _ => {}
        }
    }
    if q.ledger.standing != Standing::Ok {
        lines.push(format!("(now: {})", crate::standing_text(&q.ledger.standing)));
    }
    lines
}

/// The tool `fragment mcp` serves: the route's question, its answer.
pub fn tool() -> Value {
    let calls = json!({ "type": "integer", "minimum": 1, "maximum": QUOTE_CALLS_MAX, "description": "How many calls (absent, one)" });
    let tokens = |what: &str| json!({ "type": "integer", "minimum": 0, "description": format!("{what} tokens a call") });
    json!({
        "name": TOOL,
        "description": "What a planned job would cost your owner (you, if you are a person), priced with the same book the platform charges with, and what they have left this month. Quote before anything likely to cost more than about $1, or repeated or looping, and say both in one sentence. Each item is an operator key or part's calls (`elevenlabs:speech`, its units each call's characters), a model's calls with their tokens (`cheap`, `medium`), or any other usage. An item the book does not price is said to be unknown, never guessed. It spends nothing; with no items it reads what is left.",
        "inputSchema": {
            "type": "object",
            "required": ["items"],
            "properties": {
                "items": {
                    "type": "array",
                    "maxItems": QUOTE_ITEMS_MAX,
                    "items": { "oneOf": [
                        { "type": "object", "required": ["key"], "additionalProperties": false, "properties": {
                            "key": { "type": "string", "description": "An operator key (`perplexity`, `fal`) or a part of one (`elevenlabs:speech`)" },
                            "units": { "type": "integer", "minimum": 0, "description": "Each call's units in the key's own unit: a part counting more than one a call must say them (`elevenlabs:speech` its text's characters, `fal:image` its pixels); absent, one" },
                            "calls": calls,
                        } },
                        { "type": "object", "required": ["model"], "additionalProperties": false, "properties": {
                            "model": { "type": "string", "description": "A tier (`cheap`, `medium`) or a model's id" },
                            "input": tokens("Uncached input"), "cachedInput": tokens("Cached input"), "cacheWrite": tokens("Cache-written"), "output": tokens("Output"),
                            "calls": calls,
                        } },
                        { "type": "object", "required": ["usage"], "additionalProperties": false, "properties": {
                            "usage": { "type": "object", "description": "A ledger row's usage: {\"kind\": \"awake\", \"instance\": \"2vcpu-6gib\", \"ms\": 3600000}" },
                            "calls": calls,
                        } },
                    ] },
                },
            },
            "additionalProperties": false,
        },
        "annotations": { "readOnlyHint": true, "openWorldHint": false },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fragment_proto::ledger::{LedgerStatus, Plan, Quoted, SeatState, Why};

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    fn key(name: &str, units: Option<u64>, calls: Option<u64>) -> Planned {
        Planned::Key(PlannedKey { key: name.into(), units, calls })
    }

    /// Goal: the friendly form says what the route takes. Method: the
    /// guide's examples and each amount's spellings.
    #[test]
    fn the_friendly_form_names_its_items() {
        assert_eq!(parse(&words("elevenlabs:speech 60000chars")), Ok(vec![key("elevenlabs:speech", Some(60_000), None)]));
        assert_eq!(parse(&words("fal 40")), Ok(vec![key("fal", None, Some(40))]));
        assert_eq!(parse(&words("fal:image 40x 1048576px")), Ok(vec![key("fal:image", Some(1_048_576), Some(40))]));
        assert_eq!(parse(&words("elevenlabs:speech x300 200chars perplexity 7")), Ok(vec![key("elevenlabs:speech", Some(200), Some(300)), key("perplexity", None, Some(7))]));
        assert_eq!(parse(&words("elevenlabs:speech 60,000chars")), parse(&words("elevenlabs:speech 60_000Chars")));
        assert_eq!(
            parse(&words("cheap 2000in 300cached 500out 40calls")),
            Ok(vec![Planned::Model(PlannedModel { model: "cheap".into(), input: 2_000, cached_input: 300, cache_write: 0, output: 500, calls: Some(40) })])
        );
        assert_eq!(parse(&words("medium 10out")), Ok(vec![Planned::Model(PlannedModel { model: "medium".into(), input: 0, cached_input: 0, cache_write: 0, output: 10, calls: None })]));
        assert_eq!(parse(&[]), Ok(vec![]), "nothing: what is left, alone");
        // a part with no units is the platform's to name (`no_units`), not a guess here
        assert_eq!(parse(&words("elevenlabs:speech")), Ok(vec![key("elevenlabs:speech", None, None)]));
    }

    /// Invalid: an amount before any name, one named twice, units and
    /// tokens at once, calls out of bounds, a number that is none.
    #[test]
    fn the_friendly_form_refuses_what_it_cannot_read() {
        for bad in ["40 fal", "fal 4 5", "fal 4chars 5px", "cheap 10in 5chars", "fal 0", "fal 1000001x", "fal 99999999999999999999chars", "fal 4ch-ars"] {
            assert!(parse(&words(bad)).is_err(), "{bad}");
        }
        assert!(parse(&vec!["fal".to_string(); QUOTE_ITEMS_MAX + 1]).is_err());
    }

    fn ledger(balance: i64, standing: Standing) -> LedgerStatus {
        LedgerStatus {
            plan: Plan::Seat,
            seat: SeatState::Active,
            month: "2026-11".into(),
            balance_micros: balance,
            included_micros: balance.max(0),
            included_granted_micros: 50_000_000,
            purchased_micros: 0,
            reserved_micros: 0,
            available_micros: balance + 2_000_000,
            overdraft_micros: 2_000_000,
            standing,
            price_book: 9,
            fragments: vec![],
        }
    }

    fn priced(each: i64, calls: u64) -> Quoted {
        Quoted { priced_as: None, each_micros: Some(each), calls, charge_micros: Some(each * calls as i64), unknown: None }
    }

    /// Goal: the one line Paul asked for ("about $4.80; you have $95.40
    /// left this month"), and what it leaves out said. Method: a priced
    /// job, one with an unknown item, and an owing payer.
    #[test]
    fn the_answer_is_one_line_a_person_can_say() {
        let asked = [key("elevenlabs:speech", Some(60_000), None)];
        let q = Quote { items: vec![priced(4_800_000, 1)], total_micros: 4_800_000, left_micros: 95_400_000, ledger: ledger(95_400_000, Standing::Ok) };
        assert_eq!(said(&asked, &q), ["about $4.80; you have $95.40 left this month"]);
        let asked = [key("elevenlabs:speech", Some(200), Some(300)), key("nope", None, None)];
        let unknown = Quoted { priced_as: None, each_micros: None, calls: 1, charge_micros: None, unknown: Some(Unquoted::NoPrice) };
        let q = Quote { items: vec![priced(24_000, 300), unknown], total_micros: 7_200_000, left_micros: 0, ledger: ledger(-3_000_000, Standing::AgentsStopped { why: Why::NoCredit }) };
        let lines = said(&asked, &q);
        assert_eq!(lines[0], "about $7.20 for what is priced; you owe $3.00");
        assert_eq!(lines[1], "  elevenlabs:speech\t300 × $0.024");
        assert!(lines[2].starts_with("  nope: not priced here"), "{lines:?}");
        assert_eq!(lines[3], "(now: agents and AI are stopped (no_credit))");
        // nothing asked: what is left, alone
        let q = Quote { items: vec![], total_micros: 0, left_micros: 95_400_000, ledger: ledger(95_400_000, Standing::Ok) };
        assert_eq!(said(&[], &q), ["you have $95.40 left this month"]);
    }

    /// The tool's items are the route's: each example of the guide's, as
    /// the friendly form parses it, reads back as the route's type.
    #[test]
    fn the_tools_items_are_the_routes() {
        let t = tool();
        assert_eq!(t["name"], TOOL);
        let items = parse(&words("elevenlabs:speech 60000chars cheap 2000in 500out 40x")).unwrap();
        let back: QuoteAsk = serde_json::from_value(json!({ "items": items })).unwrap();
        assert_eq!(back.items, items);
        assert_eq!(t["inputSchema"]["properties"]["items"]["items"]["oneOf"].as_array().map(Vec::len), Some(3));
    }
}
