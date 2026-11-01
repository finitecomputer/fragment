//! How long a fragment's search takes (FIN-320): a harness, run by name,
//! that measures rather than checks. Hosted, it measures a preview as a
//! person meets it (`SEARCH_BENCH_CHUNKS=1000,10000 cargo xtask e2e
//! --hosted --config <preview's config> --branch preview --only
//! search-bench --max-paid-calls 400`, then `SEARCH_BENCH_CHUNKS=25000`:
//! a run lends at most 400 paid calls, and every chunk is distinct, so
//! embedded once); locally, on fakes, it proves the harness at small
//! sizes.
//!
//! A person of its own for each fragment, signed in through the levers,
//! writes synthetic sources into it (1k, 10k and 25k chunks): deterministic
//! text of a fixed vocabulary, one chunk a section, nothing of anyone's,
//! at most `POOL_MAX` distinct. Another person, a viewer of each, asks
//! every question: each pays for what they do, within what a person may. For each it times, as the client's wall clock (a Worker's
//! clock stands still through synchronous work, so a server's own timing
//! of a search would read zero):
//!
//! - the first search after the fragment sat idle (`IDLE`), its objects
//!   evicted or not, as the platform chooses;
//! - the first search after a write (the index reads its sign codes
//!   again);
//! - warm searches, `hybrid` (the question embedded, both legs) and
//!   `lexical` (words alone), p50 and p95 of `WARM` each;
//! - the status of its index, warm: a call through the same router,
//!   supervisor and index, which a search's time less this is the
//!   search's own.
//!
//! `SEARCH_BENCH_FANOUT=5,20` also writes as many fragments of
//! `FANOUT_CHUNKS` as its widest, and times one question across the first
//! 5, then 20 (`POST /api/search {q, in}`, what `fragment search --in`
//! asks): the meta-brain's fan-out, embedded once, merged.
//!
//! It writes `timings.json` in its scratch directory and prints a table.
//! Its fragments are the run's (`e2e-<run>-…`): `--sweep <run>` deletes
//! them. Its spend is its person's, which the run reports as it ends.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::api::Api;
use crate::{Need, Suite};

const SECTION: &str = "search-bench";
/// Paid calls a run lends at most, and an e2e person makes at most (the
/// preview's levers): an embedding call takes 100 chunks, and every
/// hybrid question is one more.
const RUN_PAID_CALLS: u64 = 400;
const PERSON_PAID_CALLS: u64 = 200;
/// Distinct sections a fragment's owner writes at most, so its writes'
/// embeddings fit what a person may spend: a larger fragment cycles them
/// (the same text again is its vector reused, by its hash), the same work
/// for each search.
const POOL_MAX: usize = 19_000;
/// Sections a source holds: 200 a write, about 80 KB, each of 60 words
/// (one chunk: the chunker bounds a chunk by its bytes).
const SECTIONS_PER_SOURCE: usize = 200;
/// Warm searches of each mode, a size.
const WARM: usize = 30;
/// How long a fragment sits idle before its first search is timed.
const IDLE: Duration = Duration::from_secs(4 * 60);
/// Chunks in each fragment a fan-out asks (`SEARCH_BENCH_FANOUT`).
const FANOUT_CHUNKS: usize = 500;

/// The words its text is drawn from, by a Zipf-like weight (the first
/// most often): ordinary English, so an embeddings model reads it as
/// prose.
const WORDS: &[&str] = &[
    "the", "of", "and", "to", "in", "a", "is", "that", "for", "it", "as", "was", "with", "be", "by", "on", "not", "he", "this", "are", "or", "his", "from", "at", "which", "but", "have",
    "an", "had", "they", "you", "were", "their", "one", "all", "we", "can", "her", "has", "there", "been", "if", "more", "when", "will", "would", "who", "so", "no", "garden", "harbour",
    "lantern", "river", "market", "ferry", "winter", "ledger", "orchard", "bridge", "kitchen", "letter", "station", "meadow", "engine", "library", "copper", "granite", "festival", "tide",
    "mountain", "village", "compass", "harvest", "signal", "cellar", "violin", "pasture", "canal", "lighthouse", "quarry", "workshop", "chapel", "tavern", "glacier", "monsoon", "carriage",
    "parcel", "almanac", "beacon", "cartographer", "dynamo", "estuary", "foundry", "gazette", "hearth", "inkwell", "junction", "kiln", "lattice", "mariner", "nocturne", "observatory",
    "paddock", "quill", "rampart", "saddle", "thimble", "umbrella", "vineyard", "windmill", "yarrow", "zephyr", "budget", "schedule", "invoice", "meeting", "report", "travel", "repair",
    "recipe", "planting", "taxes", "insurance", "holiday", "birthday", "doctor", "school", "concert", "garage", "rent", "notebook", "photograph", "museum", "voyage", "lecture",
];

/// A deterministic stream of the vocabulary's words (xorshift over a
/// seed), drawn by rank so the first words are the most frequent.
struct Text(u64);

/// The text of section `g` of a fragment drawn from `pool` distinct ones.
fn section_text(seed: u64, g: usize, pool: usize) -> String {
    Text((seed ^ ((g % pool) as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15)) | 1).sentence(60)
}

impl Text {
    fn word(&mut self) -> &'static str {
        let s = &mut self.0;
        *s ^= *s << 13;
        *s ^= *s >> 7;
        *s ^= *s << 17;
        // a power-law draw: rank = n · u², most mass at the front
        let u = (*s >> 11) as f64 / (1u64 << 53) as f64;
        WORDS[((u * u) * WORDS.len() as f64) as usize % WORDS.len()]
    }

    fn sentence(&mut self, words: usize) -> String {
        (0..words).map(|_| self.word()).collect::<Vec<_>>().join(" ")
    }
}

/// p50 and p95 of `ms`, in milliseconds.
fn quantiles(ms: &mut [f64]) -> (f64, f64) {
    ms.sort_by(f64::total_cmp);
    let at = |q: f64| ms[((ms.len() as f64 - 1.0) * q).round() as usize];
    (at(0.5), at(0.95))
}

/// A call's wall time, in milliseconds, and its status.
fn timed(f: impl FnOnce() -> Result<crate::api::Reply>) -> Result<(f64, u16, Value)> {
    let t0 = Instant::now();
    let r = f()?;
    Ok((t0.elapsed().as_secs_f64() * 1000.0, r.status, r.body))
}

/// Sentences and questions that share no word (the question's words are
/// none of its sentence's): words alone cannot find them, meaning can.
const MEANING: [(&str, &str); 2] = [
    ("The kitten curled up asleep beside the warm fireplace.", "cat napping near hearth"),
    ("The ferry to the island leaves the harbour at dawn.", "vessel sailing sunrise port"),
];
/// Sentences beside them, about other things.
const OTHERS: [&str; 2] = ["Quarterly tax returns are due to the revenue office in April.", "Our team meeting moved to Thursday afternoon in the library."];

/// Whether the deployment searches by meaning (its `semantic`), shown by
/// a match words alone cannot make: each question of `MEANING` finds its
/// sentence first by meaning, and by its words finds it not at all.
fn meaning(s: &mut Suite, api: &Api, reader: &fragment_nip98::Keys, lent: u64) -> Result<()> {
    let keys = api.person_paying(lent)?;
    let created = s.create(api, &keys, &s.name("bench-meaning"))?;
    let name = created["name"].as_str().context("its name")?.to_string();
    let shared = api.share(&name, &api.identity(reader)?, "viewer")?;
    anyhow::ensure!(shared.status == 200, "{name}: the reader's share: {shared}");
    let sentences: Vec<&str> = MEANING.iter().map(|(t, _)| *t).chain(OTHERS).collect();
    let sections: Vec<Value> = sentences.iter().enumerate().map(|(i, t)| json!({ "id": format!("m{i}"), "text": t })).collect();
    let w = api.signed(&keys, "POST", &format!("/api/f/{name}/search/upsert"), Some(&json!({ "source": "bench/meaning", "revision": "r1", "title": "Notes", "sections": sections })))?;
    anyhow::ensure!(w.status == 200, "{name}: the meaning probe's write: {w}");
    let mut found = Vec::new();
    for (i, (_, q)) in MEANING.iter().enumerate() {
        let ask = |mode: &str| api.signed(reader, "POST", &format!("/api/f/{name}/search"), Some(&json!({ "q": q, "mode": mode, "limit": 4 })));
        let (by_meaning, by_words) = (ask("semantic")?, ask("lexical")?);
        let first = by_meaning.body["results"][0]["section"].as_str().unwrap_or("").to_string();
        let words_find: Vec<String> = by_words.body["results"].as_array().into_iter().flatten().filter_map(|h| h["section"].as_str().map(str::to_string)).collect();
        println!("      meaning: {q:?}: by meaning first {first} (cosine {}), by words {words_find:?}; semantic {}", by_meaning.body["results"][0]["semantic"]["cosine"], by_meaning.body["semantic"]);
        found.push(json!({ "q": q, "semantic": by_meaning.body["semantic"], "first": first, "want": format!("m{i}"), "by_words": words_find }));
    }
    let on = found.iter().all(|f| f["semantic"] == "on");
    let matched = found.iter().all(|f| f["first"] == f["want"] && !f["by_words"].as_array().is_some_and(|w| w.contains(&f["want"])));
    if s.on_fakes() {
        // the fake's vectors are its words hashed: meaning is a model's
        println!("      (on fakes, whose vectors carry words, not meaning: the match is reported, not checked)");
        return Ok(());
    }
    s.ok("the deployment searches by meaning: a question that shares no word with its sentence finds it first by meaning, and by its words not at all", on && matched, json!(found));
    Ok(())
}

/// A fragment of `chunks` synthetic chunks, labelled `label`, written by a
/// person of its own lent `lent` paid calls, its reader a viewer: its
/// name, its owner, and its row.
fn write(s: &mut Suite, api: &Api, reader: &fragment_nip98::Keys, lent: u64, label: &str, chunks: usize) -> Result<(String, fragment_nip98::Keys, Value)> {
    let keys = api.person_paying(lent)?;
    let created = s.create(api, &keys, &s.name(label))?;
    let name = created["name"].as_str().context("its name")?.to_string();
    let shared = api.share(&name, &api.identity(reader)?, "viewer")?;
    anyhow::ensure!(shared.status == 200, "{name}: the reader's share: {shared}");
    let t0 = Instant::now();
    let seed = 0x9e37_79b9_7f4a_7c15 ^ chunks as u64 ^ label.len() as u64;
    let pool = chunks.min(POOL_MAX);
    let mut written = 0;
    let mut source = 0;
    while written < chunks {
        let n = SECTIONS_PER_SOURCE.min(chunks - written);
        let sections: Vec<Value> = (0..n).map(|i| json!({ "id": format!("s{i}"), "text": section_text(seed, written + i, pool) })).collect();
        // one title for all: a chunk's embedding input begins with it
        let body = json!({ "source": format!("bench/{source:04}"), "revision": "r1", "title": "Bench notes", "sections": sections });
        let r = api.signed(&keys, "POST", &format!("/api/f/{name}/search/upsert"), Some(&body))?;
        anyhow::ensure!(r.status == 200, "{name}: writing bench/{source:04}: {r}");
        written += r.body["chunks"].as_u64().unwrap_or(0) as usize;
        source += 1;
    }
    let status = api.signed(&keys, "GET", &format!("/api/f/{name}/search/status"), None)?;
    let wrote_s = t0.elapsed().as_secs_f64();
    println!("      {name}: {written} chunks in {source} sources, written in {wrote_s:.0} s");
    s.ok(&format!("{chunks} synthetic chunks are written to one fragment's index"), status.body["chunks"].as_u64().is_some_and(|c| c as usize >= chunks), &status);
    Ok((name, keys, json!({ "chunks": written, "distinct": pool, "sources": source, "written_s": wrote_s })))
}

pub fn search_bench(s: &mut Suite, api: &Api) -> Result<()> {
    let why = format!("it writes up to 36k synthetic chunks, embeds them (up to {RUN_PAID_CALLS} paid calls a run), waits {} minutes idle, and times searches", IDLE.as_secs() / 60);
    if !s.section_by_name(SECTION, &[Need::Levers, Need::Models], &why) {
        return Ok(());
    }
    // locally, on fakes: the harness proven at small sizes, nothing idle
    let (sizes, idle): (Vec<usize>, Duration) = match std::env::var("SEARCH_BENCH_CHUNKS") {
        Ok(list) => (list.split(',').filter(|n| !n.trim().is_empty()).map(|n| n.trim().parse().context("SEARCH_BENCH_CHUNKS lists chunk counts")).collect::<Result<_>>()?, if s.on_fakes() { Duration::ZERO } else { IDLE }),
        Err(_) if s.on_fakes() => (vec![200, 1_000], Duration::ZERO),
        Err(_) => (vec![1_000, 10_000], IDLE),
    };
    let fanouts: Vec<usize> = match std::env::var("SEARCH_BENCH_FANOUT") {
        Ok(list) if !list.trim().is_empty() => list.split(',').map(|n| n.trim().parse().context("SEARCH_BENCH_FANOUT lists fragment counts")).collect::<Result<_>>()?,
        _ if s.on_fakes() => vec![2, 3],
        _ => vec![],
    };
    let widest = fanouts.iter().copied().max().unwrap_or(0);
    // what each person spends, at most, before anyone spends any: an owner
    // its distinct chunks' embeddings and the one write after, the reader
    // every hybrid question
    let owns = |n: usize| n.min(POOL_MAX).div_ceil(100) as u64 + 2;
    let asks = sizes.len() as u64 * (WARM as u64 + 2) + fanouts.len() as u64 * (WARM as u64 + 1) + MEANING.len() as u64;
    let calls: u64 = sizes.iter().map(|n| owns(*n)).sum::<u64>() + widest as u64 * owns(FANOUT_CHUNKS) + owns(MEANING.len()) + asks;
    anyhow::ensure!(calls <= RUN_PAID_CALLS && asks <= PERSON_PAID_CALLS, "{sizes:?} chunks take about {calls} paid calls ({asks} of them a reader's), past the {RUN_PAID_CALLS} a run lends or the {PERSON_PAID_CALLS} a person may: split them across runs (SEARCH_BENCH_CHUNKS)");
    let reader = api.person_paying(asks)?;
    meaning(s, api, &reader, owns(MEANING.len()))?;
    let out = s.dir(SECTION);
    let mut rows: Vec<Value> = Vec::new();
    let mut fragments: Vec<(usize, String)> = Vec::new();
    let mut owners = Vec::new();
    // the fan-out's fragments: written first, asked after the idle wait
    let mut fanned: Vec<String> = Vec::with_capacity(widest);
    for i in 0..widest {
        let (name, _, _) = write(s, api, &reader, owns(FANOUT_CHUNKS), &format!("bench-fan{i:02}"), FANOUT_CHUNKS)?;
        fanned.push(name);
    }
    if widest > 0 {
        println!("      {widest} fragments of {FANOUT_CHUNKS} chunks for the fan-out");
    }
    for &chunks in &sizes {
        let (name, keys, row) = write(s, api, &reader, owns(chunks), &format!("bench-{}k", chunks / 1000), chunks)?;
        rows.push(row);
        fragments.push((chunks, name));
        owners.push(keys);
    }
    if !idle.is_zero() {
        println!("      idle for {} s, then each fragment's first search", idle.as_secs());
        std::thread::sleep(idle);
    }
    let mut question = Text(0x5eed);
    let ask = |name: &str, q: &str, mode: &str| api.signed(&reader, "POST", &format!("/api/f/{name}/search"), Some(&json!({ "q": q, "mode": mode, "limit": 10 })));
    for ((row, (chunks, name)), keys) in rows.iter_mut().zip(&fragments).zip(&owners) {
        let (cold_ms, cold_status, cold) = timed(|| ask(name, &question.sentence(3), "hybrid"))?;
        // a write, then the first search: the codes read again
        let w = api.signed(keys, "POST", &format!("/api/f/{name}/search/upsert"), Some(&json!({ "source": "bench/after", "revision": "r1", "sections": [{ "id": "s0", "text": question.sentence(60) }] })))?;
        anyhow::ensure!(w.status == 200, "{name}: the write before the codes' read: {w}");
        let (reread_ms, _, _) = timed(|| ask(name, &question.sentence(3), "hybrid"))?;
        let mut warm = |mode: &str| -> Result<(f64, f64, usize)> {
            let mut ms = Vec::with_capacity(WARM);
            let mut found = 0;
            for _ in 0..WARM {
                let (t, status, body) = timed(|| ask(name, &question.sentence(3), mode))?;
                anyhow::ensure!(status == 200, "{name}: a {mode} search: {status} {body}");
                found += body["results"].as_array().map_or(0, Vec::len);
                ms.push(t);
            }
            let (p50, p95) = quantiles(&mut ms);
            Ok((p50, p95, found / WARM))
        };
        let (h50, h95, h_found) = warm("hybrid")?;
        let (l50, l95, l_found) = warm("lexical")?;
        let mut base = Vec::with_capacity(WARM);
        for _ in 0..WARM {
            base.push(timed(|| api.signed(&reader, "GET", &format!("/api/f/{name}/search/status"), None))?.0);
        }
        let (b50, b95) = quantiles(&mut base);
        // whether meaning searched too is the deployment's (a private one
        // with no private embeddings slot embeds nothing): reported, not
        // checked
        s.ok(
            &format!("searches of the {chunks}-chunk fragment answer"),
            cold_status == 200 && h_found > 0 && l_found > 0,
            json!({ "semantic": cold["semantic"], "hybrid_found": h_found, "lexical_found": l_found }),
        );
        let fields = json!({
            "semantic": cold["semantic"],
            "first_after_idle_ms": cold_ms, "first_after_write_ms": reread_ms,
            "hybrid_p50_ms": h50, "hybrid_p95_ms": h95, "lexical_p50_ms": l50, "lexical_p95_ms": l95,
            "status_p50_ms": b50, "status_p95_ms": b95,
        });
        row.as_object_mut().context("a row")?.extend(fields.as_object().context("fields")?.clone());
    }
    println!("      chunks  semantic  first-after-idle  after-write  hybrid p50/p95   lexical p50/p95   status p50/p95   (ms, client wall)");
    for r in &rows {
        let f = |k: &str| r[k].as_f64().unwrap_or(f64::NAN);
        println!(
            "      {:>6}  {:>8}  {:>16.0}  {:>11.0}  {:>6.0} / {:<6.0}  {:>6.0} / {:<6.0}   {:>5.0} / {:<5.0}",
            r["chunks"], r["semantic"].as_str().unwrap_or("?"), f("first_after_idle_ms"), f("first_after_write_ms"), f("hybrid_p50_ms"), f("hybrid_p95_ms"), f("lexical_p50_ms"), f("lexical_p95_ms"), f("status_p50_ms"), f("status_p95_ms")
        );
    }
    // the fan-out: one question across the first n fragments, embedded once
    let across = |names: &[String], q: &str, mode: &str| api.signed(&reader, "POST", "/api/search", Some(&json!({ "q": q, "in": names, "mode": mode, "limit": 10 })));
    let mut fans: Vec<Value> = Vec::new();
    for &n in &fanouts {
        let names = &fanned[..n];
        let (cold_ms, cold_status, cold) = timed(|| across(names, &question.sentence(3), "hybrid"))?;
        let searched = cold["fragments"].as_array().map_or(0, |f| f.iter().filter(|x| x["state"] == "searched").count());
        let mut warm = |mode: &str| -> Result<(f64, f64)> {
            let mut ms = Vec::with_capacity(WARM);
            for _ in 0..WARM {
                let (t, status, body) = timed(|| across(names, &question.sentence(3), mode))?;
                anyhow::ensure!(status == 200, "a {mode} question across {n}: {status} {body}");
                ms.push(t);
            }
            Ok(quantiles(&mut ms))
        };
        let (h50, h95) = warm("hybrid")?;
        let (l50, l95) = warm("lexical")?;
        s.ok(&format!("a question across {n} fragments answers, each searched"), cold_status == 200 && searched == n && cold["results"].as_array().is_some_and(|r| !r.is_empty()), json!({ "semantic": cold["semantic"], "searched": searched }));
        fans.push(json!({ "fragments": n, "chunks_each": FANOUT_CHUNKS, "semantic": cold["semantic"], "first_ms": cold_ms, "hybrid_p50_ms": h50, "hybrid_p95_ms": h95, "lexical_p50_ms": l50, "lexical_p95_ms": l95 }));
    }
    if !fans.is_empty() {
        println!("      across  semantic  first   hybrid p50/p95   lexical p50/p95   (ms, client wall; {FANOUT_CHUNKS} chunks a fragment)");
        for r in &fans {
            let f = |k: &str| r[k].as_f64().unwrap_or(f64::NAN);
            println!("      {:>6}  {:>8}  {:>5.0}   {:>6.0} / {:<6.0}  {:>6.0} / {:<6.0}", r["fragments"], r["semantic"].as_str().unwrap_or("?"), f("first_ms"), f("hybrid_p50_ms"), f("hybrid_p95_ms"), f("lexical_p50_ms"), f("lexical_p95_ms"));
        }
    }
    let report = json!({ "base": api.base, "idle_s": idle.as_secs(), "warm": WARM, "rows": rows, "across": fans });
    std::fs::write(out.join("timings.json"), serde_json::to_vec_pretty(&report)?)?;
    println!("      (timings: {})", out.join("timings.json").display());
    Ok(())
}
