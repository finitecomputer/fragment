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
/// and whether their cosines compare with the others' (`cosines`).
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub state: Reached,
    pub message: Option<String>,
    pub hits: Vec<Value>,
    pub cosines: bool,
}

/// What one fragment's answer says (`status`, `answer`). Its cosines
/// compare only if it names the question's `space` (none: the question
/// was not embedded) and the one `SCHEME`.
pub fn reached(status: u16, mut answer: Value, space: Option<&wire::Space>) -> Reply {
    let message = answer["message"].as_str().map(str::to_string);
    let none = |state, message| Reply { state, message, hits: vec![], cosines: false };
    match status {
        200 if answer["indexed"] == false => none(Reached::NoIndex, None),
        200 => {
            let named = serde_json::from_value::<wire::Space>(answer["space"].clone()).ok();
            let cosines = space.is_some_and(|s| named.as_ref() == Some(s)) && answer["scheme"] == wire::SCHEME;
            let hits = answer.get_mut("results").and_then(Value::as_array_mut).map(std::mem::take).unwrap_or_default();
            Reply { state: Reached::Searched, message: None, hits, cosines }
        }
        401 | 403 => none(Reached::Denied, message),
        404 => none(Reached::Missing, message),
        _ => none(Reached::Failed, Some(message.unwrap_or_else(|| format!("it answered {status}")))),
    }
}

/// What the merge reads of a hit (fragment_proto::search::Hit): its rank
/// there (its `place`, if it names none), its rank by words, and its
/// cosine, if its answer's compare (`cosines`).
pub fn legs(hit: &Value, place: usize, cosines: bool) -> Legs {
    let rank = |v: &Value| v.as_u64().and_then(|r| usize::try_from(r).ok()).filter(|&r| r >= 1);
    let cosine = hit["semantic"]["cosine"].as_f64().filter(|c| cosines && c.is_finite());
    Legs { rank: rank(&hit["rank"]).unwrap_or(place + 1), lexical: rank(&hit["lexical"]["rank"]), cosine }
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
