//! SQLite's engines: FTS5 as the brain keeps it (templates/brain/app.mjs)
//! and as Finite Brain queries it (finite-brain-cli's search.rs), and
//! vectors as blobs scanned row by row, float32 as Finite Brain stores
//! them and int8 as a Durable Object would (docs/search-bench.md).

use crate::corpus::{words, Section};
use rusqlite::{params, Connection};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap};
use std::path::Path;

/// The brain's own admission cap: past this, no new page is indexed.
pub const BRAIN_INDEX_DB_MAX_BYTES: u64 = 12 * 1024 * 1024;

/// bm25() weights over (id, path, location, title, ancestry, heading, body).
pub const BRAIN_WEIGHTS: [f64; 7] = [0.0, 0.0, 2.0, 4.0, 2.0, 3.0, 1.0];
pub const FBRAIN_WEIGHTS: [f64; 7] = [0.0, 0.0, 3.0, 4.0, 2.0, 2.0, 1.0];

fn open(path: &Path) -> Result<Connection, String> {
    let _ = std::fs::remove_file(path);
    let conn = Connection::open(path).map_err(|e| e.to_string())?;
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;").map_err(|e| e.to_string())?;
    Ok(conn)
}

pub fn file_bytes(conn: &Connection) -> u64 {
    let pages: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0)).unwrap();
    let size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0)).unwrap();
    (pages * size) as u64
}

pub struct Fts {
    pub conn: Connection,
    /// The brain's cap on admitting a new page, when it applies.
    cap: Option<u64>,
    pub skipped_pages: usize,
}

/// A lexical candidate, with what Finite Brain's rescoring reads.
struct Candidate {
    id: String,
    raw: f64,
    fields: [(String, f64); 5],
}

impl Fts {
    pub fn create(path: &Path, cap: Option<u64>) -> Result<Self, String> {
        let conn = open(path)?;
        conn.execute_batch(
            "CREATE VIRTUAL TABLE sections USING fts5(id UNINDEXED, path UNINDEXED, location, title, ancestry, heading, body,
             tokenize = 'unicode61 remove_diacritics 2')",
        )
        .map_err(|e| e.to_string())?;
        Ok(Fts { conn, cap, skipped_pages: 0 })
    }

    /// A page's sections, in, as the brain writes them; false when the
    /// brain's cap keeps the page out.
    pub fn put_page(&mut self, path: &str, sections: &[&Section]) -> Result<bool, String> {
        let tx = self.conn.transaction().map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM sections WHERE path = ?", [path]).map_err(|e| e.to_string())?;
        if let Some(cap) = self.cap {
            if file_bytes(&tx) > cap {
                tx.commit().map_err(|e| e.to_string())?;
                self.skipped_pages += 1;
                return Ok(false);
            }
        }
        {
            let mut ins = tx
                .prepare_cached("INSERT INTO sections (id, path, location, title, ancestry, heading, body) VALUES (?, ?, ?, ?, ?, ?, ?)")
                .map_err(|e| e.to_string())?;
            for s in sections {
                let ancestry = serde_json::to_string(&s.ancestry).unwrap();
                ins.execute(params![s.id, s.path, s.path, s.title, ancestry, s.heading.as_deref().unwrap_or(""), s.body])
                    .map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(true)
    }

    pub fn delete_page(&mut self, path: &str) -> Result<(), String> {
        self.conn.execute("DELETE FROM sections WHERE path = ?", [path]).map(|_| ()).map_err(|e| e.to_string())
    }

    fn matching(&self, expr: &str, weights: &[f64; 7], limit: usize) -> Result<Vec<Candidate>, String> {
        let w = weights.map(|x| format!("{x:.1}")).join(", ");
        let mut stmt = self
            .conn
            .prepare_cached(&format!(
                "SELECT id, location, title, ancestry, heading, body, bm25(sections, {w}) AS score
                 FROM sections WHERE sections MATCH ? ORDER BY score LIMIT ?"
            ))
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![expr, limit as i64], |r| {
                let ancestry: Vec<String> = serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or_default();
                Ok(Candidate {
                    id: r.get(0)?,
                    raw: r.get(6)?,
                    fields: [
                        (r.get(1)?, FBRAIN_WEIGHTS[2]),
                        (r.get(2)?, FBRAIN_WEIGHTS[3]),
                        (ancestry.join(" "), FBRAIN_WEIGHTS[4]),
                        (r.get(4)?, FBRAIN_WEIGHTS[5]),
                        (r.get(5)?, FBRAIN_WEIGHTS[6]),
                    ],
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }

    /// The brain's search today: every word, as a word or a word's start.
    pub fn brain(&self, q: &str, limit: usize) -> Result<Vec<String>, String> {
        let ws = words(q);
        if ws.is_empty() {
            return Ok(Vec::new());
        }
        let expr = ws.iter().map(|w| format!("\"{}\"*", w.replace('"', "\"\""))).collect::<Vec<_>>().join(" ");
        Ok(self.matching(&expr, &BRAIN_WEIGHTS, limit)?.into_iter().map(|c| c.id).collect())
    }

    /// Any of the query's exact tokens, ranked by FTS5's BM25 with `weights`.
    pub fn any(&self, q: &str, weights: &[f64; 7], limit: usize) -> Result<Vec<String>, String> {
        let Some(expr) = any_tokens(q) else { return Ok(Vec::new()) };
        Ok(self.matching(&expr, weights, limit)?.into_iter().map(|c| c.id).collect())
    }

    /// Finite Brain's lexical search: any token, FTS5's best `limit` by
    /// its weights, then ranked again by its own BM25 over the corpus.
    pub fn fbrain(&self, q: &str, limit: usize) -> Result<Vec<String>, String> {
        let Some(expr) = any_tokens(q) else { return Ok(Vec::new()) };
        let terms: Vec<String> = fbrain_tokens(q).into_iter().collect::<std::collections::BTreeSet<_>>().into_iter().collect();
        let candidates = self.matching(&expr, &FBRAIN_WEIGHTS, limit)?;
        let (docs, total_len): (i64, f64) = self
            .conn
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(LENGTH(location) * 3.0 + LENGTH(title) * 4.0 + LENGTH(ancestry) * 2.0
                   + LENGTH(heading) * 2.0 + LENGTH(body)), 0.0) FROM sections",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| e.to_string())?;
        let mut df = Vec::with_capacity(terms.len());
        for t in &terms {
            let n: i64 = self
                .conn
                .query_row("SELECT COUNT(*) FROM sections WHERE sections MATCH ?", [format!("\"{}\"", t.replace('"', "\"\""))], |r| r.get(0))
                .map_err(|e| e.to_string())?;
            df.push(n as f64);
        }
        let avg = (total_len / docs.max(1) as f64).max(1.0);
        let (k1, b) = (1.2, 0.75);
        let mut scored: Vec<(f64, f64, String)> = candidates
            .into_iter()
            .map(|c| {
                let mut tf: BTreeMap<String, f64> = BTreeMap::new();
                let mut len = 0.0;
                for (value, weight) in &c.fields {
                    len += value.chars().count() as f64 * weight;
                    for tok in fbrain_tokens(value) {
                        *tf.entry(tok).or_default() += weight;
                    }
                }
                let norm = k1 * (1.0 - b + b * len / avg);
                let score: f64 = terms
                    .iter()
                    .zip(&df)
                    .map(|(t, &d)| {
                        let f = tf.get(t).copied().unwrap_or(0.0);
                        if f == 0.0 {
                            return 0.0;
                        }
                        let idf = (1.0 + (docs as f64 - d + 0.5) / (d + 0.5)).ln();
                        idf * f * (k1 + 1.0) / (f + norm)
                    })
                    .sum();
                (score, c.raw, c.id)
            })
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)));
        Ok(scored.into_iter().map(|s| s.2).collect())
    }
}

/// Finite Brain's tokens: runs of alphanumerics, lowercased.
fn fbrain_tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).map(str::to_lowercase).collect()
}

/// Finite Brain's FTS5 expression: each token quoted, any of them.
fn any_tokens(q: &str) -> Option<String> {
    let terms: Vec<String> =
        q.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).map(|t| format!("\"{}\"", t.replace('"', "\"\""))).collect();
    (!terms.is_empty()).then(|| terms.join(" OR "))
}

/// How a vector is stored: as it is, or one byte a dimension with a scale.
#[derive(Clone, Copy, PartialEq)]
pub enum Storage {
    F32,
    I8,
}

pub struct Vectors {
    pub conn: Connection,
    storage: Storage,
}

#[derive(PartialEq)]
struct Scored(f32, String);
impl Eq for Scored {}
impl PartialOrd for Scored {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Scored {
    // a min-heap's order: the weakest on top
    fn cmp(&self, other: &Self) -> Ordering {
        other.0.total_cmp(&self.0).then_with(|| self.1.cmp(&other.1))
    }
}

pub fn quantize(v: &[f32]) -> (f32, Vec<u8>) {
    let max = v.iter().fold(0f32, |m, x| m.max(x.abs()));
    let scale = if max > 0.0 { max / 127.0 } else { 1.0 };
    (scale, v.iter().map(|x| ((x / scale).round().clamp(-127.0, 127.0) as i8) as u8).collect())
}

impl Vectors {
    pub fn create(path: &Path, storage: Storage) -> Result<Self, String> {
        let conn = open(path)?;
        conn.execute_batch("CREATE TABLE vectors (id TEXT PRIMARY KEY, path TEXT NOT NULL, scale REAL NOT NULL, vector BLOB NOT NULL);
                            CREATE INDEX vectors_path ON vectors (path);")
            .map_err(|e| e.to_string())?;
        Ok(Vectors { conn, storage })
    }

    pub fn put_page(&mut self, path: &str, rows: &[(&Section, &[f32])]) -> Result<(), String> {
        let tx = self.conn.transaction().map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM vectors WHERE path = ?", [path]).map_err(|e| e.to_string())?;
        {
            let mut ins = tx.prepare_cached("INSERT INTO vectors VALUES (?, ?, ?, ?)").map_err(|e| e.to_string())?;
            for (s, v) in rows {
                let (scale, blob) = match self.storage {
                    Storage::F32 => (1.0, crate::embed::to_blob(v)),
                    Storage::I8 => quantize(v),
                };
                ins.execute(params![s.id, s.path, scale, blob]).map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())
    }

    pub fn delete_page(&mut self, path: &str) -> Result<(), String> {
        self.conn.execute("DELETE FROM vectors WHERE path = ?", [path]).map(|_| ()).map_err(|e| e.to_string())
    }

    /// The `limit` nearest by cosine (dot product of unit vectors), read a
    /// row at a time: the corpus is never held in memory.
    pub fn nearest(&self, q: &[f32], limit: usize) -> Result<Vec<String>, String> {
        let mut stmt = self.conn.prepare_cached("SELECT id, scale, vector FROM vectors").map_err(|e| e.to_string())?;
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        let mut heap: BinaryHeap<Scored> = BinaryHeap::with_capacity(limit + 1);
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let blob = row.get_ref(2).map_err(|e| e.to_string())?.as_blob().map_err(|e| e.to_string())?;
            let score = match self.storage {
                Storage::F32 => blob.as_chunks::<4>().0.iter().zip(q).map(|(c, x)| f32::from_le_bytes(*c) * x).sum::<f32>(),
                Storage::I8 => {
                    let scale: f64 = row.get(1).map_err(|e| e.to_string())?;
                    blob.iter().zip(q).map(|(&b, x)| (b as i8) as f32 * x).sum::<f32>() * scale as f32
                }
            };
            if heap.len() < limit || heap.peek().is_some_and(|w| score > w.0) {
                heap.push(Scored(score, row.get(0).map_err(|e| e.to_string())?));
                if heap.len() > limit {
                    heap.pop();
                }
            }
        }
        let mut out = heap.into_vec();
        out.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        Ok(out.into_iter().map(|s| s.1).collect())
    }
}

/// Reciprocal-rank fusion (1/(k + rank)) of ranked lists of ids.
pub fn rrf(lists: &[&[String]], k: f64) -> Vec<String> {
    let mut scores: BTreeMap<&str, f64> = BTreeMap::new();
    for list in lists {
        for (i, id) in list.iter().enumerate() {
            *scores.entry(id).or_default() += 1.0 / (k + (i + 1) as f64);
        }
    }
    let mut out: Vec<(&str, f64)> = scores.into_iter().collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    out.into_iter().map(|(id, _)| id.to_owned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(path: &str, n: usize, title: &str, body: &str) -> Section {
        Section { id: format!("{path}#{n}"), path: path.into(), title: title.into(), ancestry: vec![], heading: None, body: body.into() }
    }

    #[test]
    fn brain_needs_every_word_and_fbrain_any() {
        let dir = std::env::temp_dir().join(format!("search-bench-fts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut fts = Fts::create(&dir.join("fts.sqlite3"), None).unwrap();
        let a = section("a.md", 0, "Release plan", "We roll back with the deploy tool.");
        let b = section("b.md", 0, "Lunch", "Tacos on friday.");
        fts.put_page("a.md", &[&a]).unwrap();
        fts.put_page("b.md", &[&b]).unwrap();
        assert_eq!(fts.brain("rollback release", 10).unwrap(), Vec::<String>::new());
        assert_eq!(fts.brain("rel roll", 10).unwrap(), vec!["a.md#0"]);
        assert_eq!(fts.fbrain("how do we roll back a release", 10).unwrap(), vec!["a.md#0"]);
        assert_eq!(fts.any("tacos release", &BRAIN_WEIGHTS, 10).unwrap().len(), 2);
        fts.delete_page("a.md").unwrap();
        assert!(fts.fbrain("release", 10).unwrap().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn int8_scan_ranks_as_float_does() {
        let dir = std::env::temp_dir().join(format!("search-bench-vec-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s: Vec<Section> = (0..3).map(|n| section("p.md", n, "t", "b")).collect();
        let vs: Vec<Vec<f32>> = vec![vec![1.0, 0.0, 0.0], vec![0.6, 0.8, 0.0], vec![0.0, 0.0, 1.0]];
        for storage in [Storage::F32, Storage::I8] {
            let mut v = Vectors::create(&dir.join("v.sqlite3"), storage).unwrap();
            let rows: Vec<(&Section, &[f32])> = s.iter().zip(&vs).map(|(s, v)| (s, v.as_slice())).collect();
            v.put_page("p.md", &rows).unwrap();
            assert_eq!(v.nearest(&[0.8, 0.6, 0.0], 2).unwrap(), vec!["p.md#1", "p.md#0"]);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rrf_rewards_agreement() {
        let a = vec!["x".to_owned(), "y".to_owned()];
        let b = vec!["y".to_owned(), "z".to_owned()];
        assert_eq!(rrf(&[&a, &b], 60.0), vec!["y", "x", "z"]);
    }
}
