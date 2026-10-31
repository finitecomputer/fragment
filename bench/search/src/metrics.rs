//! Scores over pages: a result is a section, judged by the page it is in.
//! A page's second section in the top k earns nothing: what an agent sees
//! in k results is k sections, however few pages they are.

use std::collections::{BTreeMap, HashSet};

/// A question's pages and how well each answers it (2 answers it, 1 helps).
pub type Judgments = BTreeMap<String, u8>;

fn page_of(id: &str) -> &str {
    id.rsplit_once('#').map_or(id, |(p, _)| p)
}

/// Each of the top `k` results' gain, and whether its page is judged.
fn gains(ids: &[String], judged: &Judgments, k: usize) -> Vec<u8> {
    let mut seen = HashSet::new();
    ids.iter()
        .take(k)
        .map(|id| {
            let page = page_of(id);
            if seen.insert(page) { judged.get(page).copied().unwrap_or(0) } else { 0 }
        })
        .collect()
}

pub fn recall(ids: &[String], judged: &Judgments, k: usize) -> f64 {
    let relevant = judged.values().filter(|&&g| g > 0).count();
    if relevant == 0 {
        return 0.0;
    }
    gains(ids, judged, k).iter().filter(|&&g| g > 0).count() as f64 / relevant as f64
}

pub fn success(ids: &[String], judged: &Judgments, k: usize) -> f64 {
    if gains(ids, judged, k).iter().any(|&g| g > 0) { 1.0 } else { 0.0 }
}

/// nDCG with gains 2^g − 1 and log2 discounts.
pub fn ndcg(ids: &[String], judged: &Judgments, k: usize) -> f64 {
    let dcg = |gs: &[u8]| gs.iter().enumerate().map(|(i, &g)| ((1u32 << g) - 1) as f64 / ((i + 2) as f64).log2()).sum::<f64>();
    let mut ideal: Vec<u8> = judged.values().copied().filter(|&g| g > 0).collect();
    ideal.sort_unstable_by(|a, b| b.cmp(a));
    ideal.truncate(k);
    let best = dcg(&ideal);
    if best == 0.0 { 0.0 } else { dcg(&gains(ids, judged, k)) / best }
}

/// The `p`th percentile (0–100) of `xs`, by nearest rank.
pub fn percentile(xs: &[f64], p: f64) -> f64 {
    if xs.is_empty() {
        return 0.0;
    }
    let mut s = xs.to_vec();
    s.sort_by(f64::total_cmp);
    let rank = ((p / 100.0) * s.len() as f64).ceil().max(1.0) as usize;
    s[rank.min(s.len()) - 1]
}

pub fn mean(xs: &[f64]) -> f64 {
    if xs.is_empty() { 0.0 } else { xs.iter().sum::<f64>() / xs.len() as f64 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_page_counts_once() {
        let judged: Judgments = [("a.md".to_owned(), 2), ("b.md".to_owned(), 1)].into();
        let r = ids(&["a.md#0", "a.md#3", "c.md#1", "b.md#0"]);
        assert_eq!(recall(&r, &judged, 10), 1.0);
        assert_eq!(recall(&r, &judged, 2), 0.5);
        assert_eq!(success(&ids(&["c.md#0"]), &judged, 10), 0.0);
        let perfect = ids(&["a.md#0", "b.md#0"]);
        assert!((ndcg(&perfect, &judged, 10) - 1.0).abs() < 1e-9);
        assert!(ndcg(&r, &judged, 10) < 1.0);
        assert!(ndcg(&r, &judged, 10) > ndcg(&ids(&["c.md#0", "b.md#0", "a.md#0"]), &judged, 10));
    }

    #[test]
    fn percentiles_by_nearest_rank() {
        let xs: Vec<f64> = (1..=20).map(f64::from).collect();
        assert_eq!(percentile(&xs, 50.0), 10.0);
        assert_eq!(percentile(&xs, 95.0), 19.0);
        assert_eq!(percentile(&[3.0], 95.0), 3.0);
    }
}
