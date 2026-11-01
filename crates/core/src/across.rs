//! A question across fragments (docs/api.md, "Search across fragments"):
//! each fragment's own answer, merged into one ranking. Pure: the cell
//! asks each fragment as the signer, which decides alone what its hits
//! are, and hands them here.
//!
//! A fragment ranks its own hits by words (BM25, over its own corpus) and
//! by meaning (cosine), fused by reciprocal rank. Across fragments the
//! two legs differ. Every index's vectors are in one space, quantized
//! alike, so cosines compare: the semantic hits of every fragment sort
//! together. BM25 scores do not (each index's IDF and lengths are its
//! own), so the lexical hits interleave by each fragment's own rank, the
//! nearer meaning first among equals. The two lists fuse by reciprocal
//! rank (`RRF_K`), as each fragment's own did. A hit that names neither
//! leg counts its own rank as a lexical one.
//!
//! BM25 compares once its statistics are pooled (FIN-238): each fragment
//! also says how many chunks it holds and how many hold each word, and
//! each hit's score for each word, so the route scores the hits found by
//! words again as one index over them all would (`pool_words`), and they
//! sort by that before the merge.

use fragment_proto::search::{self as wire, Across, Query, Reached};
use fragment_proto::valid_fragment_name;
use serde_json::Value;

/// Reciprocal rank fusion's constant, as each index's own.
pub const RRF_K: f64 = 60.0;

/// What a question across fragments asks each one for: twice the
/// answer's length, at least `wire::ASKED_MIN`, at most `RESULTS_MAX`, so
/// the merge has depth.
pub fn asked_limit(limit: usize) -> usize {
    (2 * limit).clamp(wire::ASKED_MIN, wire::RESULTS_MAX)
}

/// A question across fragments, checked: the question as one fragment
/// takes it (`searchable::check_query`), and 1 to `IN_MAX` fragments,
/// each named in full, once.
pub fn check(a: &Across) -> Result<(), String> {
    crate::searchable::check_query(&query(a, a.limit))?;
    if a.fragments.is_empty() || a.fragments.len() > wire::IN_MAX {
        return Err(format!("in names 1 to {} fragments, not {}", wire::IN_MAX, a.fragments.len()));
    }
    for (i, f) in a.fragments.iter().enumerate() {
        if !valid_fragment_name(f) {
            return Err(format!("in names fragments in full, <label>--<suffix>, not {f:?}"));
        }
        // at most `IN_MAX`: a look back is cheaper than a set's code
        if a.fragments[..i].contains(f) {
            return Err(format!("{f} is named twice"));
        }
    }
    Ok(())
}

/// The question each fragment is asked, for `limit` hits.
pub fn query(a: &Across, limit: Option<usize>) -> Query {
    Query { q: a.q.clone(), limit, mode: a.mode, fields: a.fields.clone(), after: a.after.clone(), before: a.before.clone() }
}

/// What the merge reads of one hit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Legs {
    /// Its rank in its own fragment's answer, 1 first.
    pub rank: usize,
    /// Its rank by words there, if words found it.
    pub lexical: Option<usize>,
    /// Its cosine to the question, if meaning found it.
    pub cosine: Option<f64>,
}

/// One fragment's answer, read: how it was reached, why not, its hits,
/// whether their cosines compare with the others' (`cosines`), and what
/// its index knows of the question's words, if it said.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub state: Reached,
    pub message: Option<String>,
    pub hits: Vec<Value>,
    pub cosines: bool,
    pub words: Option<wire::WordStats>,
}

/// What one fragment's answer says (`status`, `answer`). Its cosines
/// compare only if it names the question's `space` (none: the question
/// was not embedded) and the one `SCHEME`.
pub fn reached(status: u16, mut answer: Value, space: Option<&wire::Space>) -> Reply {
    let message = answer["message"].as_str().map(str::to_string);
    let none = |state, message| Reply { state, message, hits: vec![], cosines: false, words: None };
    match status {
        200 if answer["indexed"] == false => none(Reached::NoIndex, None),
        200 => {
            let named = serde_json::from_value::<wire::Space>(answer["space"].clone()).ok();
            let cosines = space.is_some_and(|s| named.as_ref() == Some(s)) && answer["scheme"] == wire::SCHEME;
            let hits = answer.get_mut("results").and_then(Value::as_array_mut).map(std::mem::take).unwrap_or_default();
            Reply { state: Reached::Searched, message: None, hits, cosines, words: word_stats(&answer["words"]) }
        }
        401 | 403 => none(Reached::Denied, message),
        404 => none(Reached::Missing, message),
        _ => none(Reached::Failed, Some(message.unwrap_or_else(|| format!("it answered {status}")))),
    }
}

/// What an answer says its index knows of the question's words: none if
/// it says nothing (another release's) or not in that shape, so its hits
/// interleave by their own ranks. Read by hand, not by serde: the cell's
/// Wasm is budgeted.
fn word_stats(w: &Value) -> Option<wire::WordStats> {
    let words = w["words"].as_array()?.iter().map(|w| w.as_str().map(str::to_string)).collect::<Option<_>>()?;
    let with = w["with"].as_array()?.iter().map(Value::as_u64).collect::<Option<_>>()?;
    Some(wire::WordStats { words, chunks: w["chunks"].as_u64()?, with })
}

/// What the merge reads of a hit (fragment_proto::search::Hit): its rank
/// there (its `place`, if it names none), its rank by words, and its
/// cosine, if its answer's compare (`cosines`).
pub fn legs(hit: &Value, place: usize, cosines: bool) -> Legs {
    let rank = |v: &Value| v.as_u64().and_then(|r| usize::try_from(r).ok()).filter(|&r| r >= 1);
    let cosine = hit["semantic"]["cosine"].as_f64().filter(|c| cosines && c.is_finite());
    Legs { rank: rank(&hit["rank"]).unwrap_or(place + 1), lexical: rank(&hit["lexical"]["rank"]), cosine }
}

/// FTS5's IDF of a word `n_with` of `n_all` chunks hold: ln((N − n + 0.5)
/// / (n + 0.5)), at least 1e-6 (fts5_aux.c, `fts5Bm25Function`).
pub fn idf(n_all: f64, n_with: f64) -> f64 {
    ((n_all - n_with + 0.5) / (n_with + 0.5)).ln().max(1e-6)
}

/// Each hit's word scores, for those its words leg found, if `reply`'s
/// answer holds together: it says what its index knows of exactly the
/// question's `words`, no more chunks hold a word than it has, and every
/// hit found by words scores each word, finite and not negative, the
/// scores summing to its BM25 (FTS5 sums an OR's phrases).
fn said(words: &[String], reply: &Reply, legs: &[Legs]) -> Option<Vec<Option<Vec<f64>>>> {
    let stats = reply.words.as_ref()?;
    if stats.words != words || stats.with.len() != words.len() || stats.with.iter().any(|&n| n > stats.chunks) {
        return None;
    }
    let scores = |hit: &Value| -> Option<Vec<f64>> {
        let scores = hit["words"].as_array().filter(|s| s.len() == words.len())?;
        let scores: Vec<f64> = scores.iter().map(|s| s.as_f64().filter(|s| s.is_finite() && *s >= 0.0)).collect::<Option<_>>()?;
        let bm25 = hit["lexical"]["bm25"].as_f64()?;
        // the same sums in the same order, but the last bits
        ((scores.iter().sum::<f64>() - bm25).abs() <= 1e-9 * bm25.abs()).then_some(scores)
    };
    reply.hits.iter().zip(legs).map(|(hit, l)| if l.lexical.is_some() { scores(hit).map(Some) } else { Some(None) }).collect()
}

/// The hits found by words of the fragments that said what their indexes
/// know of the question's `words` (`Reply::words`), ranked by words as one
/// index holding all of theirs would rank them. BM25 weighs a word by its
/// IDF (`idf`), of an index's own chunks and those holding the word, so
/// two fragments' scores do not compare; summed over the fragments that
/// said, they do. Each hit's score for each word, rescaled from its
/// fragment's IDF to the pooled one, summed, ranks it among them all: by
/// words 1, 2, … (the nearer meaning, then the order asked, first among
/// equals). So they interleave with a fragment's that did not say
/// (another release's, or an answer that does not hold together) by its
/// own ranks, as one fragment's. Fewer than two that said: unchanged, so
/// one fragment's answer stays its own exactly. A fragment chose its hits
/// by its own statistics: one it did not return is not found here.
pub fn pool_words(words: &[String], replies: &[Reply], legs: &mut [Vec<Legs>]) {
    assert_eq!(replies.len(), legs.len(), "the legs of each reply");
    let said: Vec<Option<Vec<Option<Vec<f64>>>>> = replies.iter().zip(legs.iter()).map(|(r, l)| said(words, r, l)).collect();
    let pooled: Vec<&wire::WordStats> = replies.iter().zip(&said).filter(|(_, s)| s.is_some()).filter_map(|(r, _)| r.words.as_ref()).collect();
    if pooled.len() < 2 {
        return;
    }
    // as floats: no sum of counts overflows
    let chunks: f64 = pooled.iter().map(|s| s.chunks as f64).sum();
    let all: Vec<f64> = (0..words.len()).map(|w| idf(chunks, pooled.iter().map(|s| s.with[w] as f64).sum())).collect();
    let mut scored: Vec<(f64, usize, usize)> = Vec::new();
    for (f, hits) in said.iter().enumerate() {
        let (Some(hits), Some(stats)) = (hits, &replies[f].words) else { continue };
        let own: Vec<f64> = stats.with.iter().map(|&n| idf(stats.chunks as f64, n as f64)).collect();
        for (h, scores) in hits.iter().enumerate() {
            if let Some(scores) = scores {
                scored.push((scores.iter().zip(&own).zip(&all).map(|((s, own), all)| s / own * all).sum(), f, h));
            }
        }
    }
    let cosine = |k: usize| legs[scored[k].1][scored[k].2].cosine.unwrap_or(f64::NEG_INFINITY);
    let order = sorted((0..scored.len()).collect(), &|a, b| {
        scored[b].0.total_cmp(&scored[a].0).then(cosine(b).total_cmp(&cosine(a))).then((scored[a].1, scored[a].2).cmp(&(scored[b].1, scored[b].2)))
    });
    for (place, k) in order.into_iter().enumerate() {
        legs[scored[k].1][scored[k].2].lexical = Some(place + 1);
    }
}

/// One hit of the merged answer: the fragment's place among those asked,
/// the hit's in that fragment's answer, and its fused score.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    pub fragment: usize,
    pub hit: usize,
    pub score: f64,
}

/// `keys` in `cmp`'s order. One comparator type for every sort here: each
/// closure given to a sort compiles a quicksort of its own into the cell's
/// Wasm (5.5 KB each).
fn sorted(mut keys: Vec<usize>, cmp: &dyn Fn(usize, usize) -> std::cmp::Ordering) -> Vec<usize> {
    keys.sort_unstable_by(|&a, &b| cmp(a, b));
    keys
}

/// The best `limit` of `answers` (each fragment's hits, in the order
/// asked), best first.
pub fn merge(answers: &[Vec<Legs>], limit: usize) -> Vec<Placed> {
    let all: Vec<(usize, usize, Legs)> = answers.iter().enumerate().flat_map(|(f, hits)| hits.iter().enumerate().map(move |(i, l)| (f, i, *l))).collect();
    let cosine = |k: usize| all[k].2.cosine.unwrap_or(f64::NEG_INFINITY);
    let by_words = |k: usize| {
        let l = all[k].2;
        l.lexical.or((l.cosine.is_none()).then_some(l.rank))
    };
    let semantic: Vec<usize> = (0..all.len()).filter(|&k| all[k].2.cosine.is_some()).collect();
    // every order below ends at the hit's own place, so no two compare equal
    // and an unstable sort is exact
    let place = |a: usize, b: usize| all[a].0.cmp(&all[b].0).then(all[a].1.cmp(&all[b].1));
    let semantic = sorted(semantic, &|a, b| cosine(b).total_cmp(&cosine(a)).then(all[a].2.rank.cmp(&all[b].2.rank)).then(place(a, b)));
    let lexical: Vec<usize> = (0..all.len()).filter(|&k| by_words(k).is_some()).collect();
    let lexical = sorted(lexical, &|a, b| by_words(a).cmp(&by_words(b)).then(cosine(b).total_cmp(&cosine(a))).then(place(a, b)));
    let mut score = vec![0.0; all.len()];
    for list in [&lexical, &semantic] {
        for (place, &k) in list.iter().enumerate() {
            score[k] += 1.0 / (RRF_K + (place + 1) as f64);
        }
    }
    assert!(score.iter().all(|&s| s > 0.0), "every hit is in one list at least");
    let mut order = sorted((0..all.len()).collect(), &|a, b| score[b].total_cmp(&score[a]).then(all[a].2.rank.cmp(&all[b].2.rank)).then(place(a, b)));
    order.truncate(limit);
    order.into_iter().map(|k| Placed { fragment: all[k].0, hit: all[k].1, score: score[k] }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fragment_proto::search::Mode;
    use std::collections::BTreeMap;

    fn words(rank: usize) -> Legs {
        Legs { rank, lexical: Some(rank), cosine: None }
    }

    fn meaning(rank: usize, cosine: f64) -> Legs {
        Legs { rank, lexical: None, cosine: Some(cosine) }
    }

    fn places(merged: &[Placed]) -> Vec<(usize, usize)> {
        merged.iter().map(|p| (p.fragment, p.hit)).collect()
    }

    fn across(fragments: &[&str]) -> Across {
        Across { q: "launch".into(), fragments: fragments.iter().map(|f| f.to_string()).collect(), limit: None, mode: Mode::Hybrid, fields: BTreeMap::new(), after: None, before: None }
    }

    /// Goal: one fragment's answer comes back as it was; several with no
    /// legs interleave by their own ranks, in the order asked among equals.
    #[test]
    fn answers_without_legs_interleave_by_rank() {
        let bare = |rank| Legs { rank, lexical: None, cosine: None };
        assert_eq!(places(&merge(&[vec![bare(1), bare(2), bare(3)]], 10)), [(0, 0), (0, 1), (0, 2)]);
        assert_eq!(places(&merge(&[vec![bare(1), bare(2)], vec![bare(1), bare(2)]], 10)), [(0, 0), (1, 0), (0, 1), (1, 1)]);
        assert_eq!(merge(&[vec![bare(1), bare(2)], vec![bare(1)]], 2).len(), 2, "the best `limit`");
        assert!(merge(&[vec![], vec![]], 10).is_empty());
    }

    /// Goal: cosines compare across fragments, so a nearer hit wins
    /// whatever its own rank; BM25 does not, so between two fragments'
    /// first hits by words the nearer meaning decides.
    #[test]
    fn meaning_compares_across_fragments_and_words_do_not() {
        let merged = merge(&[vec![meaning(1, 0.5)], vec![meaning(1, 0.6), meaning(2, 0.9)]], 10);
        assert_eq!(places(&merged), [(1, 1), (1, 0), (0, 0)]);
        let tie = |cosine| Legs { rank: 1, lexical: Some(1), cosine: Some(cosine) };
        assert_eq!(places(&merge(&[vec![tie(0.2)], vec![tie(0.8)]], 10)), [(1, 0), (0, 0)]);
    }

    /// Goal: a section both legs find rises above one either leg found
    /// first, as within one fragment.
    #[test]
    fn found_by_both_legs_rises() {
        let both = Legs { rank: 2, lexical: Some(2), cosine: Some(0.7) };
        let merged = merge(&[vec![words(1), both], vec![meaning(1, 0.9)]], 10);
        assert_eq!(merged[0].fragment, 0);
        assert_eq!(merged[0].hit, 1);
        assert!((merged[0].score - (1.0 / 62.0 + 1.0 / 62.0)).abs() < 1e-12, "{merged:?}");
    }

    #[test]
    fn a_question_across_fragments_is_checked_whole() {
        assert!(check(&across(&["research--k3x9", "delivery--m4p7"])).is_ok());
        assert!(check(&across(&[])).unwrap_err().contains("1 to 32"));
        let many: Vec<String> = (0..=wire::IN_MAX).map(|i| format!("b{i}--k3x9")).collect();
        assert!(check(&across(&many.iter().map(String::as_str).collect::<Vec<_>>())).unwrap_err().contains("1 to 32"));
        assert!(check(&across(&["research"])).unwrap_err().contains("in full"));
        assert!(check(&across(&["research--k3x9", "research--k3x9"])).unwrap_err().contains("twice"));
        let mut blank = across(&["research--k3x9"]);
        blank.q = "  ".into();
        assert!(check(&blank).is_err(), "the question as one fragment takes it");
        blank.q = "launch".into();
        blank.limit = Some(wire::RESULTS_MAX + 1);
        assert!(check(&blank).is_err());
    }

    #[test]
    fn each_fragment_is_asked_for_depth() {
        assert_eq!(asked_limit(1), wire::ASKED_MIN);
        assert_eq!(asked_limit(15), 30);
        assert_eq!(asked_limit(wire::RESULTS_MAX), wire::RESULTS_MAX);
        let a = across(&["research--k3x9"]);
        assert_eq!(query(&a, Some(20)).limit, Some(20));
        assert_eq!(query(&a, Some(20)).q, "launch");
    }

    #[test]
    fn each_answer_says_how_its_fragment_was_reached() {
        use serde_json::json;
        let space = wire::Space { model: "@cf/baai/bge-m3".into(), dimensions: 1024, version: None, document_prefix: None, query_prefix: None };
        let named = json!({ "model": "@cf/baai/bge-m3", "dimensions": 1024 });
        let hit = json!({ "rank": 1, "source": "s", "section": "t0" });
        let answer = |space: &Value, scheme: &str| json!({ "results": [hit.clone()], "semantic": "on", "indexed": true, "space": space, "scheme": scheme });
        let searched = reached(200, answer(&named, wire::SCHEME), Some(&space));
        assert_eq!((searched.state, searched.hits.len(), searched.cosines), (Reached::Searched, 1, true));
        let null_version = json!({ "model": "@cf/baai/bge-m3", "dimensions": 1024, "version": null });
        assert!(reached(200, answer(&null_version, wire::SCHEME), Some(&space)).cosines, "a space is compared as one, not as its JSON");
        let other = json!({ "model": "@cf/baai/bge-m3", "dimensions": 1024, "version": "2" });
        assert!(!reached(200, answer(&other, wire::SCHEME), Some(&space)).cosines, "another space's cosines");
        assert!(!reached(200, answer(&named, "f32"), Some(&space)).cosines, "another scheme's");
        assert!(!reached(200, answer(&named, wire::SCHEME), None).cosines, "a question not embedded");
        assert!(!reached(200, json!({ "results": [hit.clone()], "semantic": "on", "indexed": true }), Some(&space)).cosines, "an answer naming no space");
        assert_eq!(reached(200, json!({ "results": [], "semantic": "on", "indexed": false }), Some(&space)).state, Reached::NoIndex);
        let indexed_empty = reached(200, json!({ "results": [], "semantic": "on", "indexed": true }), Some(&space));
        assert_eq!((indexed_empty.state, indexed_empty.hits.len()), (Reached::Searched, 0), "an index whose only write failed");
        let denied = reached(403, json!({ "error": "forbidden", "message": "viewers and up" }), None);
        assert_eq!((denied.state, denied.message.as_deref()), (Reached::Denied, Some("viewers and up")));
        assert_eq!(reached(404, json!({ "error": "not_found", "message": "no fragment" }), None).state, Reached::Missing);
        assert_eq!(reached(502, Value::Null, None).message.as_deref(), Some("it answered 502"));
        assert!(reached(200, Value::Null, None).hits.is_empty());
    }

    /// The question's words in these tests.
    const Q: [&str; 2] = ["harbour", "lantern"];

    fn q() -> Vec<String> {
        Q.iter().map(|w| w.to_string()).collect()
    }

    /// A fragment's answer by words alone: its chunks, how many hold each
    /// word, and each hit's score for each word.
    fn said_answer(chunks: u64, with: [u64; 2], hits: &[[f64; 2]]) -> Value {
        use serde_json::json;
        let results: Vec<Value> = hits.iter().enumerate().map(|(i, s)| json!({ "rank": i + 1, "lexical": { "rank": i + 1, "bm25": s[0] + s[1] }, "words": s })).collect();
        json!({ "results": results, "semantic": "off", "indexed": true, "words": { "words": Q, "chunks": chunks, "with": with } })
    }

    /// Answers read as the route reads them, each hit's legs with them.
    fn read(answers: Vec<Value>) -> (Vec<Reply>, Vec<Vec<Legs>>) {
        let replies: Vec<Reply> = answers.into_iter().map(|a| reached(200, a, None)).collect();
        let all = replies.iter().map(|r| r.hits.iter().enumerate().map(|(i, h)| legs(h, i, r.cosines)).collect()).collect();
        (replies, all)
    }

    fn by_words(all: &[Vec<Legs>]) -> Vec<Vec<Option<usize>>> {
        all.iter().map(|hits| hits.iter().map(|l| l.lexical).collect()).collect()
    }

    /// A big fragment where "harbour" is rare and "lantern" common, and a
    /// small one the other way round. Its own statistics make "lantern"
    /// the small one's best word; across both, "harbour" is the rare one.
    fn harbour_and_lantern() -> Vec<Value> {
        vec![said_answer(1000, [10, 900], &[[2.0, 0.0], [0.0, 1.5e-6]]), said_answer(10, [9, 1], &[[0.0, 3.0], [2e-6, 0.0]])]
    }

    #[test]
    fn idf_is_fts5s() {
        assert!((idf(1000.0, 10.0) - (990.5f64 / 10.5).ln()).abs() < 1e-12);
        assert_eq!(idf(10.0, 9.0), 1e-6, "a word most chunks hold weighs at least 1e-6");
        assert_eq!(idf(0.0, 0.0), 1e-6);
    }

    /// Goal: the hits found by words rank as one index over every fragment
    /// that said would rank them: a word rare across them all decides,
    /// not one rare in a small fragment alone.
    #[test]
    fn pooled_words_rank_as_one_index() {
        let (replies, mut all) = read(harbour_and_lantern());
        let today = merge(&all, 10);
        assert_eq!(places(&today), [(0, 0), (1, 0), (0, 1), (1, 1)], "each fragment's own ranks interleave");
        pool_words(&q(), &replies, &mut all);
        // pooled: 1010 chunks, 19 hold "harbour" (IDF 3.93), 901 "lantern" (1e-6)
        assert_eq!(by_words(&all), [vec![Some(2), Some(4)], vec![Some(3), Some(1)]]);
        assert_eq!(places(&merge(&all, 10)), [(1, 1), (0, 0), (1, 0), (0, 1)]);
        assert_eq!(all[0][0].rank, 1, "a hit's own rank stays its fragment's");
    }

    /// Goal: one fragment's answer is exactly its own, whatever it says:
    /// fewer than two fragments that said leave every rank as it was.
    #[test]
    fn one_fragment_is_merged_as_today() {
        let answers = harbour_and_lantern();
        for answers in [vec![answers[0].clone()], vec![answers[1].clone()], vec![answers[0].clone(), json_bare_answer()]] {
            let (replies, mut all) = read(answers);
            let before = all.clone();
            pool_words(&q(), &replies, &mut all);
            assert_eq!(all, before);
        }
    }

    /// An answer of a release before pooled words: hits found by words,
    /// no word scores, no statistics.
    fn json_bare_answer() -> Value {
        serde_json::json!({ "results": [{ "rank": 1, "lexical": { "rank": 1, "bm25": 9.0 } }, { "rank": 2, "lexical": { "rank": 2, "bm25": 4.0 } }], "semantic": "off", "indexed": true })
    }

    /// Goal: during a release, a fragment that does not say interleaves by
    /// its own ranks, as today, with the ones that do ranked as one.
    #[test]
    fn a_fragment_that_does_not_say_interleaves_as_today() {
        let mut answers = harbour_and_lantern();
        answers.insert(1, json_bare_answer());
        let (replies, mut all) = read(answers);
        assert!(replies[1].words.is_none());
        pool_words(&q(), &replies, &mut all);
        assert_eq!(by_words(&all), [vec![Some(2), Some(4)], vec![Some(1), Some(2)], vec![Some(3), Some(1)]]);
        // by words, each rank once of the pooled and once of the other, in the order asked
        assert_eq!(places(&merge(&all, 10)), [(1, 0), (2, 1), (0, 0), (1, 1), (2, 0), (0, 1)]);
        // a later release's statistics, with a field this one does not know, still pool
        let mut later = harbour_and_lantern();
        later[0]["words"]["more"] = serde_json::json!(1);
        let (replies, mut all) = read(later);
        pool_words(&q(), &replies, &mut all);
        assert_eq!(by_words(&all), [vec![Some(2), Some(4)], vec![Some(3), Some(1)]]);
    }

    /// Goal: an answer whose word statistics do not hold together is read
    /// as one that did not say, and the others still pool.
    #[test]
    fn statistics_that_do_not_hold_together_are_not_pooled() {
        use serde_json::json;
        // each a way to break the third fragment's answer
        type Break = fn(&mut Value);
        let broken: [(&str, Break); 11] = [
            ("other words", |a| a["words"]["words"] = json!(["harbor", "lantern"])),
            ("words in another order", |a| a["words"]["words"] = json!(["lantern", "harbour"])),
            ("a count short", |a| a["words"]["with"] = json!([1])),
            ("more chunks hold a word than it has", |a| a["words"]["with"] = json!([3, 1])),
            ("statistics of another shape", |a| a["words"]["chunks"] = json!("two")),
            ("a hit by words with no scores", |a| drop(a["results"][0].as_object_mut().and_then(|h| h.remove("words")))),
            ("a score short", |a| a["results"][1]["words"] = json!([1.0])),
            ("a negative score", |a| a["results"][1]["words"] = json!([1.0, -0.5])),
            ("a score not a number", |a| a["results"][1]["words"] = json!([1.0, "x"])),
            ("scores that do not sum to its BM25", |a| a["results"][1]["words"] = json!([0.25, 0.0])),
            ("a hit by words with no BM25", |a| a["results"][1]["lexical"] = json!({ "rank": 2 })),
        ];
        for (why, break_it) in broken {
            let mut answers = harbour_and_lantern();
            let mut third = said_answer(2, [1, 1], &[[1.0, 1.0], [0.5, 0.0]]);
            break_it(&mut third);
            answers.push(third);
            let (replies, mut all) = read(answers);
            pool_words(&q(), &replies, &mut all);
            assert_eq!(by_words(&all), [vec![Some(2), Some(4)], vec![Some(3), Some(1)], vec![Some(1), Some(2)]], "{why}");
        }
        // a hit the meaning leg alone found needs no scores
        let mut answers = harbour_and_lantern();
        answers[1]["results"][0] = json!({ "rank": 1, "semantic": { "rank": 1, "cosine": 0.5 } });
        let (replies, mut all) = read(answers);
        pool_words(&q(), &replies, &mut all);
        assert_eq!(by_words(&all), [vec![Some(2), Some(3)], vec![None, Some(1)]]);
    }

    /// Goal: the same answers merge the same, however often: pooling reads
    /// no rank it wrote, so pooled again is pooled once.
    #[test]
    fn pooled_again_is_pooled_once() {
        let (replies, mut all) = read(harbour_and_lantern());
        pool_words(&q(), &replies, &mut all);
        let once = all.clone();
        pool_words(&q(), &replies, &mut all);
        assert_eq!(all, once);
        let (replies, mut again) = read(harbour_and_lantern());
        pool_words(&q(), &replies, &mut again);
        assert_eq!(merge(&again, 10), merge(&once, 10));
    }

    #[test]
    fn a_hit_names_its_legs() {
        use serde_json::json;
        let hit = json!({ "rank": 3, "lexical": { "rank": 2, "bm25": 7.5 }, "semantic": { "rank": 4, "cosine": 0.61 } });
        let l = legs(&hit, 0, true);
        assert_eq!((l.rank, l.lexical, l.cosine), (3, Some(2), Some(0.61)));
        assert_eq!(legs(&hit, 0, false).cosine, None, "a cosine that does not compare is none");
        let bare = legs(&json!({ "source": "s", "rank": 0 }), 4, true);
        assert_eq!((bare.rank, bare.lexical, bare.cosine), (5, None, None));
    }
}
