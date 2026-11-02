//! A fragment's search index, its arithmetic: how a vector is held, how
//! each leg scores, and how the legs fuse. The cell's `SearchIndex` object
//! (cell/src/search_index.rs, one per fragment life) holds the rows; this
//! decides what they mean, the same in every index.
//!
//! **A vector** is held as `fragment_proto::search::SCHEME` names it: the
//! unit vector's components as int8 at the one scale `SCALE` (127/0.25:
//! on BGE-M3's vectors, 0.012% of components clip), and their sign bits
//! (`code`). Every index's int8 dot product over `SCALE`² is the same
//! estimate of a cosine, so hits compare across fragments of one space.
//!
//! **The meaning leg** reads every chunk's sign bits against the float
//! question (`Table`: a byte's eight components' signed sum, looked up),
//! keeps the best `candidates`, and reranks those by int8 (`dot`). On
//! public BGE-M3 vectors (FIN-310, 300 questions over 2,184 chunks), the
//! sign pass over the best 2.3% then int8 kept 0.967 of exact float's top
//! 10, over 4.6% 0.980; int8 over every chunk, 0.981.
//!
//! **Fusion** is reciprocal rank (k = `RRF_K`) of each leg's sections,
//! each ranked by its best chunk, as the LanceDB service fused them.

use base64::Engine;

/// int8 per unit of a unit vector's component: 127 at 0.25.
pub const SCALE: f32 = 508.0;
/// Reciprocal rank fusion's k.
pub const RRF_K: f64 = 60.0;
/// The chunks each leg brings to the fusion.
pub const DEPTH: usize = 100;
/// The share of an index's chunks (percent) the sign pass hands to int8,
/// at least `DEPTH`: the exact hits fall within a share of the corpus,
/// not at a fixed rank (p99 2.8%, FIN-310).
pub const CANDIDATES_PERCENT: usize = 3;
/// A hit's snippet, at most, in bytes.
pub const SNIPPET_MAX_BYTES: usize = 300;
/// The question's distinct words the words leg asks for, at most.
pub const WORDS_MAX: usize = 32;

/// A vector as an index holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    /// Component `i`'s sign (1: positive) at bit `i % 8` of byte `i / 8`.
    pub code: Vec<u8>,
    pub q: Vec<i8>,
}

/// The bytes of a `dims`-component vector's sign code.
pub fn code_bytes(dims: usize) -> usize {
    dims.div_ceil(8)
}

/// `v` scaled to length 1, or none: a zero or non-finite vector has no
/// direction.
pub fn unit(v: &[f32]) -> Option<Vec<f32>> {
    let norm = v.iter().map(|x| f64::from(*x) * f64::from(*x)).sum::<f64>().sqrt();
    if !norm.is_finite() || norm == 0.0 || v.iter().any(|x| !x.is_finite()) {
        return None;
    }
    Some(v.iter().map(|x| (f64::from(*x) / norm) as f32).collect())
}

/// `v` held: made a unit vector first, so a model that returns another
/// length is held alike. None for a vector with no direction.
pub fn hold(v: &[f32]) -> Option<Held> {
    let u = unit(v)?;
    let q = u.iter().map(|x| (x * SCALE).round().clamp(-127.0, 127.0) as i8).collect();
    let mut code = vec![0u8; code_bytes(u.len())];
    for (i, x) in u.iter().enumerate() {
        if *x > 0.0 {
            code[i / 8] |= 1 << (i % 8);
        }
    }
    Some(Held { code, q })
}

/// The int8 dot product of two held vectors: Wasm SIMD's, sixteen
/// components a step, where the cell is built for it (`simd128`:
/// xtask/src/build.rs), else `dot_scalar`'s. The two agree exactly (an
/// i8·i8 product and its sums fit i32 at any dimensions a slot names).
pub fn dot(a: &[i8], b: &[i8]) -> i32 {
    assert_eq!(a.len(), b.len(), "held vectors of one space");
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    let d = simd::dot(a, b);
    #[cfg(not(all(target_arch = "wasm32", target_feature = "simd128")))]
    let d = dot_scalar(a, b);
    d
}

/// `dot`, one component at a time: the reference SIMD's is checked against
/// (the index's test lever, in the Wasm itself).
pub fn dot_scalar(a: &[i8], b: &[i8]) -> i32 {
    a.iter().zip(b).map(|(x, y)| i32::from(*x) * i32::from(*y)).sum()
}

#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
mod simd {
    use core::arch::wasm32::*;

    /// Sixteen lanes of `s` from `at`.
    fn lanes(s: &[i8], at: usize) -> v128 {
        let c = &s[at..at + 16];
        i8x16(c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7], c[8], c[9], c[10], c[11], c[12], c[13], c[14], c[15])
    }

    /// Each sixteen components widened to i16 and multiplied in pairs
    /// into i32 (`i32x4.dot_i16x8_s`); the tail one at a time.
    pub(super) fn dot(a: &[i8], b: &[i8]) -> i32 {
        let whole = a.len() / 16 * 16;
        let mut acc = i32x4_splat(0);
        for at in (0..whole).step_by(16) {
            let (x, y) = (lanes(a, at), lanes(b, at));
            acc = i32x4_add(acc, i32x4_dot_i16x8(i16x8_extend_low_i8x16(x), i16x8_extend_low_i8x16(y)));
            acc = i32x4_add(acc, i32x4_dot_i16x8(i16x8_extend_high_i8x16(x), i16x8_extend_high_i8x16(y)));
        }
        let lanes = i32x4_extract_lane::<0>(acc) + i32x4_extract_lane::<1>(acc) + i32x4_extract_lane::<2>(acc) + i32x4_extract_lane::<3>(acc);
        lanes + super::dot_scalar(&a[whole..], &b[whole..])
    }
}

/// Whether `dot` agrees with `dot_scalar` on a pseudo-random pair of each
/// length from 0 to 1040, every one (a slot's 1024, its tails, and a
/// step past): what the index's test lever answers, run in the very Wasm
/// a deploy ships.
pub fn dot_agrees() -> bool {
    let mut s: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        (s >> 32) as i8
    };
    (0..=1040usize).all(|len| {
        let (a, b): (Vec<i8>, Vec<i8>) = (0..len).map(|_| (next(), next())).unzip();
        dot(&a, &b) == dot_scalar(&a, &b)
    })
}

/// An int8 dot product as the cosine it estimates.
pub fn cosine(dot: i32) -> f64 {
    f64::from(dot) / f64::from(SCALE * SCALE)
}

/// The float question against sign codes: for each byte of a code, each of
/// its 256 values' signed sum of the question's eight components there
/// (`+x` where the bit is set, `-x` where not), so a code's score is one
/// lookup a byte.
pub struct Table(Vec<[f32; 256]>);

impl Table {
    pub fn of(question: &[f32]) -> Table {
        let mut out = Vec::with_capacity(code_bytes(question.len()));
        for part in question.chunks(8) {
            let mut t = [0f32; 256];
            t[0] = -part.iter().sum::<f32>();
            for v in 1..256usize {
                // `v` is `v` without its lowest set bit, plus that bit
                let low = v.trailing_zeros() as usize;
                t[v] = t[v & (v - 1)] + part.get(low).map_or(0.0, |x| 2.0 * x);
            }
            out.push(t);
        }
        Table(out)
    }

    /// The bytes of a code this table reads.
    pub fn bytes(&self) -> usize {
        self.0.len()
    }

    pub fn score(&self, code: &[u8]) -> f32 {
        assert_eq!(code.len(), self.0.len(), "a code of the question's space");
        self.0.iter().zip(code).map(|(t, b)| t[usize::from(*b)]).sum()
    }
}

/// A fence, at most: what JavaScript holds exactly, and SQLite's integer.
pub const FENCE_MAX: u64 = (1 << 53) - 1;

/// Whether a write or delete sent at fence `asked` is taken, the source's
/// stored fence `stored` (none: none taken yet). Only a lower one is
/// refused: the same batch's retry, at its own fence, is taken again.
pub fn admits(stored: Option<u64>, asked: u64) -> bool {
    assert!(asked <= FENCE_MAX, "a fence is checked against FENCE_MAX before it is compared");
    stored.is_none_or(|s| asked >= s)
}

/// Whether an index holding `held` chunks takes a source's write of `new`
/// chunks that replaces `replaced` of them, holding at most `max`: a
/// source written again counts once, so one at capacity is replaced.
pub fn fits(held: u64, replaced: u64, new: u64, max: u64) -> bool {
    held.saturating_sub(replaced).checked_add(new).is_some_and(|n| n <= max)
}

/// How many of `n` chunks the sign pass hands to int8.
pub fn candidates(n: usize) -> usize {
    (n * CANDIDATES_PERCENT / 100).max(DEPTH).min(n)
}

/// The `k` best of `scored`, best first; ties by the lower index.
pub fn best<T: Copy + Ord>(mut scored: Vec<(T, f32)>, k: usize) -> Vec<(T, f32)> {
    let order = |a: &(T, f32), b: &(T, f32)| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0));
    if scored.len() > k && k > 0 {
        scored.select_nth_unstable_by(k - 1, order);
    }
    scored.truncate(k);
    scored.sort_unstable_by(order);
    scored
}

/// A vector sent as base64 of little-endian float32s, exactly `dims` of
/// them, every one finite.
pub fn vector_of(b64: &str, dims: u32) -> Result<Vec<f32>, String> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).map_err(|e| format!("a vector is base64: {e}"))?;
    if bytes.len() != dims as usize * 4 {
        return Err(format!("a vector is {dims} float32s, not {} bytes", bytes.len()));
    }
    let v: Vec<f32> = bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
    if v.iter().any(|x| !x.is_finite()) {
        return Err("a vector's components are finite".into());
    }
    Ok(v)
}

/// The question's words, as the words leg asks for them: its runs of
/// alphanumerics, lower-cased, each once, in order, at most `WORDS_MAX`.
pub fn words_of(q: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    for w in q.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()) {
        let w = w.to_lowercase();
        if !words.contains(&w) {
            words.push(w);
        }
        if words.len() == WORDS_MAX {
            break;
        }
    }
    words
}

/// One word as FTS5 asks for it alone: quoted. Alphanumerics alone
/// (`words_of`), so nothing to escape inside the quotes.
pub fn phrase(word: &str) -> String {
    format!("\"{word}\"")
}

/// `words` as FTS5 asks for any of them.
pub fn any_of(words: &[String]) -> String {
    words.iter().map(|w| phrase(w)).collect::<Vec<_>>().join(" OR ")
}

/// The question's words as FTS5 asks for them: any of them (`words_of`);
/// none, no words leg.
pub fn match_of(q: &str) -> Option<String> {
    let words = words_of(q);
    (!words.is_empty()).then(|| any_of(&words))
}

/// A line's label, its first word or two and a colon ("Agent: "), at most
/// this long: kept when a snippet starts inside its line, so a cut line still
/// says whose words it holds (FIN-365: a Session Archive's "Agent: …").
pub const LABEL_MAX_BYTES: usize = 24;

/// `line`'s label, when it begins with one: letters (and spaces between
/// them) then ": ", within `LABEL_MAX_BYTES`.
fn label_of(line: &str) -> Option<&str> {
    let head = line.char_indices().take_while(|&(i, _)| i < LABEL_MAX_BYTES).last().map_or(0, |(i, c)| i + c.len_utf8());
    let colon = line[..head].find(": ")?;
    let name = &line[..colon];
    let worded = name.starts_with(|c: char| c.is_alphabetic()) && name.chars().all(|c| c.is_alphabetic() || c == ' ');
    (worded && colon + 2 <= LABEL_MAX_BYTES).then(|| &line[..colon + 2])
}

/// At most `SNIPPET_MAX_BYTES` of `body`, around the first of the
/// question's words it holds (else its start), cut at characters; begun
/// inside a line with a label, that label first (`label_of`).
pub fn snippet(body: &str, q: &str) -> String {
    let lower = body.to_lowercase();
    let at = q
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 2)
        .filter_map(|w| lower.find(&w.to_lowercase()))
        .min()
        // lower-casing may move a byte offset: one off a boundary starts over
        .filter(|&i| i < body.len() && body.is_char_boundary(i))
        .unwrap_or(0);
    let mut start = at.saturating_sub(SNIPPET_MAX_BYTES / 3);
    while !body.is_char_boundary(start) {
        start -= 1;
    }
    // the line it starts in, and that line's label: begun inside the label,
    // it starts at the line; past it, the label leads
    let line = body[..start].rfind('\n').map_or(0, |i| i + 1);
    let mut label = (start > line).then(|| label_of(&body[line..])).flatten();
    if label.is_some_and(|l| start < line + l.len()) {
        start = line;
        label = None;
    }
    let budget = SNIPPET_MAX_BYTES - label.map_or(0, str::len);
    let mut end = (start + budget).min(body.len());
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    let mut s = String::new();
    if let Some(l) = label {
        s.push_str(l);
    }
    if start > 0 {
        s.push('…');
    }
    s.push_str(body[start..end].trim());
    if end < body.len() {
        s.push('…');
    }
    s
}

/// One chunk a leg found: its section, itself, and its score there (BM25,
/// higher better; or a cosine).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Found {
    pub section: i64,
    pub chunk: i64,
    pub score: f64,
}

/// A section's place in one leg: its rank there (from 1) and its best
/// chunk's score.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Place {
    pub rank: usize,
    pub score: f64,
}

/// A section fused: its score, its place in each leg that found it, and
/// the chunk it is shown by (the words leg's best when the words matched).
#[derive(Debug, Clone, PartialEq)]
pub struct Fused {
    pub section: i64,
    pub chunk: i64,
    pub score: f64,
    pub lexical: Option<Place>,
    pub semantic: Option<Place>,
}

/// Each leg's sections, best chunk first (`lexical` and `semantic` best
/// first), fused by reciprocal rank: the best `limit`, ties by section.
pub fn fuse(lexical: &[Found], semantic: &[Found], limit: usize) -> Vec<Fused> {
    let mut by: std::collections::BTreeMap<i64, Fused> = std::collections::BTreeMap::new();
    for (leg, found) in [(0, lexical), (1, semantic)] {
        let mut rank = 0;
        for f in found {
            let e = by.entry(f.section).or_insert(Fused { section: f.section, chunk: f.chunk, score: 0.0, lexical: None, semantic: None });
            let place = if leg == 0 { &mut e.lexical } else { &mut e.semantic };
            if place.is_some() {
                continue; // a later chunk of a section this leg ranked
            }
            rank += 1;
            *place = Some(Place { rank, score: f.score });
            e.score += 1.0 / (RRF_K + rank as f64);
        }
    }
    let mut all: Vec<Fused> = by.into_values().collect();
    all.sort_unstable_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.section.cmp(&b.section)));
    all.truncate(limit);
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic pseudo-random vector (xorshift), roughly unit.
    fn noise(seed: u64, dims: usize) -> Vec<f32> {
        let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        (0..dims)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                ((s >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) as f32
            })
            .collect()
    }

    fn exact(a: &[f32], b: &[f32]) -> f64 {
        let (a, b) = (unit(a).unwrap(), unit(b).unwrap());
        a.iter().zip(&b).map(|(x, y)| f64::from(*x) * f64::from(*y)).sum()
    }

    #[test]
    fn a_held_vector_estimates_its_cosine() {
        for dims in [1024, 1023, 384] {
            let (a, b) = (noise(1, dims), noise(2, dims));
            let mix: Vec<f32> = a.iter().zip(&b).map(|(x, y)| x + 0.5 * y).collect();
            let (ha, hm) = (hold(&a).unwrap(), hold(&mix).unwrap());
            assert_eq!((ha.code.len(), ha.q.len()), (code_bytes(dims), dims));
            let est = cosine(dot(&ha.q, &hm.q));
            assert!((est - exact(&a, &mix)).abs() < 0.02, "{dims}: {est} vs {}", exact(&a, &mix));
            // a vector with itself: within the rounding of every component
            assert!((cosine(dot(&ha.q, &ha.q)) - 1.0).abs() < 0.02);
        }
        // the length a model returns does not move it
        let a = noise(3, 64);
        let twice: Vec<f32> = a.iter().map(|x| x * 2.0).collect();
        assert_eq!(hold(&a), hold(&twice));
    }

    #[test]
    fn a_vector_with_no_direction_is_not_held() {
        assert_eq!(hold(&[0.0; 8]), None);
        assert_eq!(hold(&[1.0, f32::NAN]), None);
        assert_eq!(hold(&[f32::INFINITY, 0.0]), None);
    }

    #[test]
    fn components_past_a_quarter_clip() {
        let mut v = vec![0.0f32; 16];
        v[0] = 1.0;
        let h = hold(&v).unwrap();
        assert_eq!(h.q[0], 127, "1.0 holds as 127, not 508");
        assert_eq!(h.code, vec![1, 0]);
    }

    #[test]
    fn the_table_scores_a_code_as_the_signs_sum() {
        for dims in [16, 13] {
            let q = noise(4, dims);
            let t = Table::of(&q);
            for seed in 0..20 {
                let h = hold(&noise(100 + seed, dims)).unwrap();
                let want: f32 = q.iter().enumerate().map(|(i, x)| if h.code[i / 8] >> (i % 8) & 1 == 1 { *x } else { -x }).sum();
                assert!((t.score(&h.code) - want).abs() < 1e-4, "{dims}");
            }
        }
    }

    #[test]
    fn the_sign_pass_then_int8_finds_the_exact_top() {
        // 2000 chunks, each near one of 40 topics; a question near one
        let dims = 256;
        let topics: Vec<Vec<f32>> = (0..40).map(|t| noise(1000 + t, dims)).collect();
        let chunks: Vec<Vec<f32>> = (0..2000u64).map(|i| topics[(i % 40) as usize].iter().zip(noise(5000 + i, dims)).map(|(t, n)| t + 0.6 * n).collect()).collect();
        let held: Vec<Held> = chunks.iter().map(|c| hold(c).unwrap()).collect();
        let question: Vec<f32> = topics[7].iter().zip(noise(9, dims)).map(|(t, n)| t + 0.6 * n).collect();
        let truth: Vec<usize> = best(chunks.iter().enumerate().map(|(i, c)| (i, exact(&question, c) as f32)).collect(), 10).into_iter().map(|(i, _)| i).collect();
        let t = Table::of(&unit(&question).unwrap());
        let first = best(held.iter().enumerate().map(|(i, h)| (i, t.score(&h.code))).collect(), candidates(held.len()));
        let hq = hold(&question).unwrap();
        let reranked = best(first.iter().map(|(i, _)| (*i, dot(&hq.q, &held[*i].q) as f32)).collect(), 10);
        let kept = reranked.iter().filter(|(i, _)| truth.contains(i)).count();
        assert!(kept >= 9, "kept {kept} of the exact top 10");
    }

    #[test]
    fn dot_agrees_with_its_reference_at_every_length() {
        assert!(dot_agrees());
        // the extremes: every product at its most
        let (a, b) = (vec![-128i8; 1040], vec![-128i8; 1040]);
        assert_eq!(dot(&a, &b), 1040 * 128 * 128);
        assert_eq!(dot(&a, &vec![127i8; 1040]), -(1040 * 128 * 127));
    }

    #[test]
    fn an_index_holds_up_to_its_cap_counting_a_replaced_source_once() {
        assert!(fits(2, 0, 1, 3), "up to the cap");
        assert!(!fits(3, 0, 1, 3), "past it");
        assert!(fits(3, 2, 2, 3), "a source replaced at capacity, as large");
        assert!(!fits(3, 2, 3, 3), "or larger, past it");
        assert!(fits(3, 3, 0, 3), "emptied");
        assert!(fits(0, 5, 3, 3), "a count that drifted below what it replaces is no room");
        assert!(!fits(u64::MAX, 0, 1, u64::MAX));
    }

    #[test]
    fn candidates_are_a_share_at_least_the_depth() {
        assert_eq!(candidates(50), 50);
        assert_eq!(candidates(1000), 100);
        assert_eq!(candidates(50_000), 1500);
    }

    #[test]
    fn best_keeps_the_top_in_order() {
        let got = best(vec![(3, 0.5), (1, 0.9), (2, 0.9), (4, 0.1)], 3);
        assert_eq!(got, vec![(1, 0.9), (2, 0.9), (3, 0.5)]);
        assert_eq!(best(vec![(1, 1.0)], 5), vec![(1, 1.0)]);
        assert_eq!(best::<i32>(vec![], 5), vec![]);
    }

    #[test]
    fn vectors_are_checked_as_they_arrive() {
        let ok = base64::engine::general_purpose::STANDARD.encode([1.0f32, -2.0].iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>());
        assert_eq!(vector_of(&ok, 2), Ok(vec![1.0, -2.0]));
        assert!(vector_of(&ok, 3).is_err(), "another space's length");
        let nan = base64::engine::general_purpose::STANDARD.encode(f32::NAN.to_le_bytes());
        assert!(vector_of(&nan, 1).is_err());
        assert!(vector_of("not base64!", 1).is_err());
    }

    #[test]
    fn the_words_leg_asks_for_any_word_quoted() {
        assert_eq!(words_of("Harbour lantern, harbour"), ["harbour", "lantern"]);
        assert_eq!(match_of("Harbour lantern, harbour"), Some("\"harbour\" OR \"lantern\"".into()));
        assert_eq!(match_of("\"x\" OR y* NEAR(z)"), Some("\"x\" OR \"or\" OR \"y\" OR \"near\" OR \"z\"".into()), "FTS5's syntax is only words here");
        assert_eq!(match_of("?! —"), None);
        let many: String = (0..100).map(|i| format!("w{i} ")).collect();
        assert_eq!(match_of(&many).unwrap().matches(" OR ").count(), WORDS_MAX - 1);
    }

    #[test]
    fn snippets_center_the_words_and_cut_at_characters() {
        let body = format!("{} the harbour lantern audit {}", "é".repeat(200), "x".repeat(400));
        let s = snippet(&body, "lantern");
        assert!(s.contains("lantern") && s.starts_with('…') && s.ends_with('…'));
        assert!(s.len() <= SNIPPET_MAX_BYTES + 2 * '…'.len_utf8());
        assert_eq!(snippet("short body", "nothing"), "short body");
    }

    /// Goal: a snippet begun inside a line keeps that line's label, so a
    /// cut line still says whose words it holds (FIN-365). Valid: a long
    /// "Agent: " line found deep in leads with its label. Invalid: a line
    /// with no label (a page's paragraph), or a "label" too long or not
    /// words, is cut as before. Edge: begun inside the label itself, the
    /// snippet starts at the line; begun at a line's start, nothing doubles.
    #[test]
    fn a_snippet_begun_inside_a_labelled_line_keeps_its_label() {
        let reply = format!("Nice sketch. {} Pole beans fix nitrogen, which helps the tomatoes. {}", "The south fence gets the most sun. ".repeat(4), "x ".repeat(200));
        let body = format!("Person: Here's the plot plan.\nAgent: {reply}\nPerson: thanks");
        let s = snippet(&body, "nitrogen");
        assert!(s.starts_with("Agent: …") && s.contains("nitrogen"), "{s}");
        assert!(s.len() <= SNIPPET_MAX_BYTES + 2 * '…'.len_utf8(), "{}", s.len());
        // no label: a page's paragraph, cut as before
        let page = format!("Tomatoes need sun. {} Pole beans fix nitrogen. {}", "Water them well. ".repeat(8), "y ".repeat(200));
        let p = snippet(&page, "nitrogen");
        assert!(p.starts_with('…') && p.contains("nitrogen"), "{p}");
        // not a label: too long, or not words
        for line in [format!("A label far too long to be one: {reply}"), format!("https://x.test: {reply}"), format!("2026-10-01: {reply}")] {
            let t = snippet(&line, "nitrogen");
            assert!(t.starts_with('…') && t.contains("nitrogen"), "{t}");
        }
        // begun inside the label: the line's start, the label whole and once
        // (the word 3 bytes past the window's lead, the window starts in "Agent: ")
        let near = format!("q\nAgent: {} nitrogen", "b".repeat(SNIPPET_MAX_BYTES / 3 - 7 + 3 - 1));
        let lead = near.find("nitrogen").unwrap() - SNIPPET_MAX_BYTES / 3;
        assert!((2..2 + "Agent: ".len()).contains(&lead), "the window starts inside the label: {lead}");
        let n = snippet(&near, "nitrogen");
        assert!(n.starts_with("…Agent: b") && n.matches("Agent:").count() == 1, "{n}");
        // begun at a line's start: as before
        assert_eq!(snippet("Agent: beans fix nitrogen", "nitrogen"), "Agent: beans fix nitrogen");
        // a label of more than one byte a character
        let s2 = snippet(&format!("Élève: {reply}"), "nitrogen");
        assert!(s2.starts_with("Élève: …") && s2.contains("nitrogen"), "{s2}");
    }

    #[test]
    fn fusion_ranks_sections_by_their_best_chunks() {
        let f = |section, chunk, score| Found { section, chunk, score };
        let lexical = [f(1, 10, 9.0), f(1, 11, 8.0), f(2, 20, 7.0)];
        let semantic = [f(2, 21, 0.8), f(3, 30, 0.7)];
        let hits = fuse(&lexical, &semantic, 10);
        let order: Vec<i64> = hits.iter().map(|h| h.section).collect();
        assert_eq!(order, vec![2, 1, 3]);
        assert_eq!(hits[0].lexical, Some(Place { rank: 2, score: 7.0 }));
        assert_eq!(hits[0].semantic, Some(Place { rank: 1, score: 0.8 }));
        assert_eq!(hits[0].chunk, 20, "shown by its words' best chunk");
        assert_eq!(hits[1].lexical, Some(Place { rank: 1, score: 9.0 }));
        assert_eq!((hits[1].chunk, hits[1].semantic), (10, None), "its best chunk, its second unranked");
        assert_eq!((hits[2].chunk, hits[2].lexical, hits[2].semantic.map(|p| p.rank)), (30, None, Some(2)));
        assert_eq!(fuse(&lexical, &semantic, 1).len(), 1);
    }

    #[test]
    fn a_fence_refuses_only_a_lower_one() {
        assert!(admits(None, 0) && admits(None, FENCE_MAX), "none stored: any");
        assert!(admits(Some(7), 7), "the same batch's retry");
        assert!(admits(Some(7), 8));
        assert!(!admits(Some(7), 6), "a stale batch's");
        assert_eq!(FENCE_MAX, 9_007_199_254_740_991);
    }
}
