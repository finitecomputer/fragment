//! The quality gate for a question across fragments (FIN-238, decision 5):
//! the corpus cut by its top-level folder (a wiki: one brain each, as a
//! parent brain's children are), each searched as a fragment's own index
//! searches (`fragment_core::search_index`, over chunks cut by
//! `fragment_core::searchable::chunked`, as cell/src/search_index.rs runs
//! them), merged by the route's own code (`fragment_core::across::merge`),
//! and scored against one index over the whole corpus built for the test,
//! and against Finite Brain's hybrid.

use std::collections::HashMap;
use std::path::Path;

use fragment_core::search_index as engine;
use fragment_core::{across, searchable};
use fragment_proto::search as wire;
use rusqlite::{params, Connection};

use crate::corpus::{Page, Section};

/// The headings above a section a brain sends (templates/brain/app.mjs).
const CONTEXT_MAX: usize = 8;

/// The fragment a page is in when the corpus is cut by wiki: its top-level
/// folder, or `.` for a page at the root.
pub fn wiki_of(path: &str) -> &str {
    path.split_once('/').map_or(".", |(w, _)| w)
}

/// A page as the brain writes it to its index: its path the source, its
/// sections (those with text) with the headings above them as context.
fn upsert(page: &Page, sections: &[Section]) -> (wire::Upsert, Vec<String>) {
    let title = sections.first().map(|s| s.title.clone()).unwrap_or_else(|| crate::corpus::stem(&page.path));
    let mut out = Vec::new();
    let mut ids = Vec::new();
    for (i, s) in sections.iter().enumerate() {
        if s.body.trim().is_empty() {
            continue;
        }
        let context = s.ancestry.iter().rev().take(CONTEXT_MAX).rev().cloned().collect();
        out.push(wire::Section { id: format!("s{i}"), text: s.body.clone(), heading: s.heading.clone(), context, at: None, location: None });
        ids.push(s.id.clone());
    }
    let up = wire::Upsert { source: page.path.clone(), revision: "r".into(), replaces: None, title: Some(title), at: None, fields: Default::default(), sections: out, fence: None };
    (up, ids)
}

/// One page's chunks: what each embeds (`input`), its words, and its
/// section (the bench's section id).
pub struct Cut {
    pub title: String,
    pub chunks: Vec<(String, searchable::Chunk, Option<String>, String)>,
}

/// Every page's chunks, as a slot of `window` tokens and `prefix` cuts them.
pub fn cut(pages: &[(Page, Vec<Section>)], window: u32, prefix: Option<&str>) -> Vec<Cut> {
    pages
        .iter()
        .map(|(page, sections)| {
            let (up, ids) = upsert(page, sections);
            let chunks = searchable::chunked(&up, window, prefix)
                .into_iter()
                .map(|c| {
                    let s = &up.sections[c.section];
                    (ids[c.section].clone(), c.clone(), s.heading.clone(), s.context.join("\n"))
                })
                .collect();
            Cut { title: up.title.unwrap_or_default(), chunks }
        })
        .collect()
}

/// A fragment's index as the cell keeps it: its chunks' words in FTS5 (the
/// same table, tokenizer and field weights), their vectors held as signs
/// and int8.
pub struct Index {
    conn: Connection,
    /// Each chunk (its rowid, from 1) its section's id.
    section_of: Vec<String>,
    codes: Vec<u8>,
    q: Vec<Vec<i8>>,
    /// Each chunk's float vector, unit length: only the bench's diagnosis
    /// reads it (`Variant::exact`), the cell keeps signs and int8.
    units: Vec<Vec<f32>>,
    width: usize,
}

/// What the bench's diagnosis changes in a search: an exact float meaning
/// leg (none of the cell's signs and int8), and each leg's depth.
#[derive(Clone, Copy)]
pub struct Variant {
    pub exact: bool,
    pub depth: usize,
}

impl Default for Variant {
    fn default() -> Self {
        Variant { exact: false, depth: engine::DEPTH }
    }
}

/// One section an index answered: its place in its own answer, by words
/// and by meaning (as a fragment's `/search/asked` answer names them).
pub struct Hit {
    pub section: String,
    pub lexical: Option<usize>,
    /// Its best chunk's BM25 by words: comparable across fragments only
    /// when scored with pooled statistics (`Pooled`).
    pub score: Option<f64>,
    pub cosine: Option<f64>,
}

/// Word statistics pooled across the fragments a question asks (what each
/// child's `fts5vocab` would give the route): their chunks in all, and in
/// how many each of the question's words occurs.
pub struct Pooled {
    chunks: f64,
    with: HashMap<String, f64>,
}

/// FTS5's IDF: ln((N - n + 0.5) / (n + 0.5)), at least 1e-6.
fn idf(n_all: f64, n_with: f64) -> f64 {
    ((n_all - n_with + 0.5) / (n_with + 0.5)).ln().max(1e-6)
}

/// The question's words as FTS5 phrases, each alone (`match_of`'s words).
fn words(q: &str) -> Vec<String> {
    engine::match_of(q).map(|m| m.split(" OR ").map(str::to_string).collect()).unwrap_or_default()
}

impl Pooled {
    pub fn of(fragments: &[Index], q: &str) -> Result<Pooled, String> {
        let mut with: HashMap<String, f64> = HashMap::new();
        for f in fragments {
            for w in words(q) {
                let n: i64 = f.conn.query_row("SELECT count(*) FROM chunk_words WHERE chunk_words MATCH ?", [&w], |r| r.get(0)).map_err(|e| e.to_string())?;
                *with.entry(w).or_default() += n as f64;
            }
        }
        Ok(Pooled { chunks: fragments.iter().map(|f| f.chunks() as f64).sum(), with })
    }
}

impl Index {
    pub fn build(path: &Path, cuts: &[&Cut], vector_of: &HashMap<String, Vec<f32>>, dims: usize) -> Result<Index, String> {
        let _ = std::fs::remove_file(path);
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.execute_batch(
            "PRAGMA journal_mode = OFF; PRAGMA synchronous = OFF;
             CREATE VIRTUAL TABLE chunk_words USING fts5(title, heading, context, body, content = '', contentless_delete = 1, tokenize = 'unicode61 remove_diacritics 2');",
        )
        .map_err(|e| e.to_string())?;
        let width = engine::code_bytes(dims);
        let mut ix = Index { conn, section_of: Vec::new(), codes: Vec::new(), q: Vec::new(), units: Vec::new(), width };
        let tx = ix.conn.transaction().map_err(|e| e.to_string())?;
        {
            let mut ins = tx.prepare("INSERT INTO chunk_words (rowid, title, heading, context, body) VALUES (?, ?, ?, ?, ?)").map_err(|e| e.to_string())?;
            for cut in cuts {
                for (section, chunk, heading, context) in &cut.chunks {
                    let v = vector_of.get(&chunk.hash).ok_or("a chunk was not embedded")?;
                    let held = engine::hold(v).ok_or("a chunk's vector has no direction")?;
                    ix.section_of.push(section.clone());
                    let rowid = ix.section_of.len() as i64;
                    ins.execute(params![rowid, cut.title, heading, context, chunk.text]).map_err(|e| e.to_string())?;
                    ix.codes.extend_from_slice(&held.code);
                    ix.q.push(held.q);
                    ix.units.push(engine::unit(v).expect("a held vector has a direction"));
                }
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(ix)
    }

    pub fn chunks(&self) -> usize {
        self.section_of.len()
    }

    /// The section of chunk `rowid`, as the index's own `section` id: its
    /// place among the sections, which is what `fuse` groups by.
    fn section_key(&self, keys: &mut HashMap<String, i64>, names: &mut Vec<String>, rowid: i64) -> i64 {
        let name = &self.section_of[rowid as usize - 1];
        *keys.entry(name.clone()).or_insert_with(|| {
            names.push(name.clone());
            names.len() as i64 - 1
        })
    }

    /// The words leg with pooled statistics: each word's BM25 here (FTS5's,
    /// with this index's IDF and lengths) rescaled to the pooled IDF, summed
    /// over the question's words. With this index's own statistics it is
    /// the OR query's BM25 exactly (FTS5 sums its phrases' scores).
    fn lexical_pooled(&self, q: &str, pooled: &Pooled) -> Result<Vec<(i64, f64)>, String> {
        let mut score: HashMap<i64, f64> = HashMap::new();
        let n_all = self.chunks() as f64;
        for w in words(q) {
            let mut stmt = self.conn.prepare_cached("SELECT rowid, -bm25(chunk_words, 4.0, 3.0, 2.0, 1.0) FROM chunk_words WHERE chunk_words MATCH ?").map_err(|e| e.to_string())?;
            let rows: Vec<(i64, f64)> = stmt.query_map([&w], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| e.to_string())?.collect::<Result<_, _>>().map_err(|e| e.to_string())?;
            let here = idf(n_all, rows.len() as f64);
            let all = idf(pooled.chunks, pooled.with.get(&w).copied().unwrap_or(0.0));
            for (chunk, s) in rows {
                *score.entry(chunk).or_default() += s * all / here;
            }
        }
        let mut out: Vec<(i64, f64)> = score.into_iter().collect();
        out.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        out.truncate(engine::DEPTH);
        Ok(out)
    }

    /// The question, answered as cell/src/search_index.rs's `search` does,
    /// its best `limit` sections.
    pub fn search(&self, q: &str, question: &[f32], limit: usize) -> Result<Vec<Hit>, String> {
        self.search_with(q, question, limit, None)
    }

    /// `search`, its words leg scored with `pooled` statistics when given.
    pub fn search_with(&self, q: &str, question: &[f32], limit: usize, pooled: Option<&Pooled>) -> Result<Vec<Hit>, String> {
        self.search_variant(q, question, limit, pooled, Variant::default(), None)
    }

    /// `search`, changed in one way for the bench's diagnosis (C): an exact
    /// float meaning leg, legs of another depth, or another words leg's
    /// sections (`words`, best first).
    pub fn search_variant(&self, q: &str, question: &[f32], limit: usize, pooled: Option<&Pooled>, variant: Variant, words: Option<&[String]>) -> Result<Vec<Hit>, String> {
        let mut keys: HashMap<String, i64> = HashMap::new();
        let mut names: Vec<String> = Vec::new();
        // the words leg: the best DEPTH chunks by BM25 over their fields (title 4, heading 3, context 2, body 1)
        let mut lexical = Vec::new();
        if let Some(sections) = words {
            // another engine's words leg, its sections in its order (each its own placeholder chunk)
            for (n, name) in sections.iter().take(variant.depth).enumerate() {
                let section = *keys.entry(name.clone()).or_insert_with(|| {
                    names.push(name.clone());
                    names.len() as i64 - 1
                });
                lexical.push(engine::Found { section, chunk: -(n as i64) - 1, score: -(n as f64) });
            }
        } else if let Some(pooled) = pooled {
            for (chunk, score) in self.lexical_pooled(q, pooled)? {
                lexical.push(engine::Found { section: self.section_key(&mut keys, &mut names, chunk), chunk, score });
            }
        } else if let Some(words) = engine::match_of(q) {
            let mut stmt = self
                .conn
                .prepare_cached("SELECT rowid, -bm25(chunk_words, 4.0, 3.0, 2.0, 1.0) AS score FROM chunk_words WHERE chunk_words MATCH ? ORDER BY score DESC, rowid LIMIT ?")
                .map_err(|e| e.to_string())?;
            let rows = stmt.query_map(params![words, variant.depth as i64], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?))).map_err(|e| e.to_string())?;
            for row in rows {
                let (chunk, score) = row.map_err(|e| e.to_string())?;
                lexical.push(engine::Found { section: self.section_key(&mut keys, &mut names, chunk), chunk, score });
            }
        }
        // the meaning leg: every sign code against the question, the best `candidates` reranked by int8
        let mut semantic = Vec::new();
        if let (Some(unit), true) = (engine::unit(question), variant.exact) {
            let scored: Vec<(i64, f32)> = self.units.iter().enumerate().map(|(i, u)| (i as i64 + 1, u.iter().zip(&unit).map(|(a, b)| a * b).sum())).collect();
            for (chunk, cosine) in engine::best(scored, variant.depth) {
                semantic.push(engine::Found { section: self.section_key(&mut keys, &mut names, chunk), chunk, score: f64::from(cosine) });
            }
        } else if let Some(unit) = engine::unit(question) {
            let held = engine::hold(&unit).expect("a unit vector is held");
            let table = engine::Table::of(&unit);
            let scored: Vec<(i64, f32)> = self.codes.chunks_exact(self.width).enumerate().map(|(i, code)| (i as i64 + 1, table.score(code))).collect();
            let first = engine::best(scored, engine::candidates(self.chunks()));
            let reranked: Vec<(i64, f32)> = first.iter().map(|(id, _)| (*id, engine::dot(&held.q, &self.q[*id as usize - 1]) as f32)).collect();
            for (chunk, dot) in engine::best(reranked, variant.depth) {
                semantic.push(engine::Found { section: self.section_key(&mut keys, &mut names, chunk), chunk, score: engine::cosine(dot as i32) });
            }
        }
        Ok(engine::fuse(&lexical, &semantic, limit)
            .into_iter()
            .map(|f| Hit { section: names[f.section as usize].clone(), lexical: f.lexical.map(|p| p.rank), score: f.lexical.map(|p| p.score), cosine: f.semantic.map(|p| p.score) })
            .collect())
    }
}

/// The question across `fragments`, as the route merges their answers:
/// each asked for `asked_limit(k)`, the best `k` of all.
pub fn federated(fragments: &[Index], q: &str, question: &[f32], k: usize) -> Result<Vec<String>, String> {
    let asked = across::asked_limit(k);
    let answers: Vec<Vec<Hit>> = fragments.iter().map(|f| f.search(q, question, asked)).collect::<Result<_, _>>()?;
    let legs: Vec<Vec<across::Legs>> = answers
        .iter()
        .map(|hits| hits.iter().enumerate().map(|(i, h)| across::Legs { rank: i + 1, lexical: h.lexical, cosine: h.cosine }).collect())
        .collect();
    Ok(across::merge(&legs, k).into_iter().map(|p| answers[p.fragment][p.hit].section.clone()).collect())
}

/// A prototype of the route's merge with pooled word statistics (FIN-238,
/// the bench's B): each fragment's words leg scored with `Pooled`, so BM25
/// compares across fragments and the words list sorts by score, as the
/// meaning list sorts by cosine; then reciprocal rank fusion, as
/// `across::merge`. The route's merge is unchanged; this measures whether
/// changing it would pay.
pub fn federated_pooled(fragments: &[Index], q: &str, question: &[f32], k: usize) -> Result<Vec<String>, String> {
    let pooled = Pooled::of(fragments, q)?;
    let asked = across::asked_limit(k);
    let answers: Vec<Vec<Hit>> = fragments.iter().map(|f| f.search_with(q, question, asked, Some(&pooled))).collect::<Result<_, _>>()?;
    let all: Vec<(usize, usize, &Hit)> = answers.iter().enumerate().flat_map(|(f, hits)| hits.iter().enumerate().map(move |(i, h)| (f, i, h))).collect();
    let cosine = |h: &Hit| h.cosine.unwrap_or(f64::NEG_INFINITY);
    let place = |i: usize| (all[i].0, all[i].1);
    let mut lexical: Vec<usize> = (0..all.len()).filter(|&i| all[i].2.score.is_some()).collect();
    lexical.sort_by(|&a, &b| all[b].2.score.unwrap_or(0.0).total_cmp(&all[a].2.score.unwrap_or(0.0)).then(cosine(all[b].2).total_cmp(&cosine(all[a].2))).then(place(a).cmp(&place(b))));
    let mut semantic: Vec<usize> = (0..all.len()).filter(|&i| all[i].2.cosine.is_some()).collect();
    semantic.sort_by(|&a, &b| cosine(all[b].2).total_cmp(&cosine(all[a].2)).then(place(a).cmp(&place(b))));
    let mut score = vec![0.0; all.len()];
    for list in [&lexical, &semantic] {
        for (n, &i) in list.iter().enumerate() {
            score[i] += 1.0 / (across::RRF_K + (n + 1) as f64);
        }
    }
    let mut order: Vec<usize> = (0..all.len()).collect();
    order.sort_by(|&a, &b| score[b].total_cmp(&score[a]).then(place(a).cmp(&place(b))));
    Ok(order.into_iter().take(k).map(|i| all[i].2.section.clone()).collect())
}
