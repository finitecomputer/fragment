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

/// What a brain sends of a page (templates/brain/app.mjs `source`): the
/// headings above a section, and the bytes of each field.
const CONTEXT_MAX: usize = 8;
const TITLE_MAX_BYTES: usize = 512;
const HEADING_MAX_BYTES: usize = 512;
const CONTEXT_ITEM_MAX_BYTES: usize = 256;
const FIELD_MAX_BYTES: usize = 64;

/// At most `max` bytes of `text`, cut at a whole character (app.mjs `clip`).
fn clip(text: &str, max: usize) -> String {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

/// Text as the index takes it: no control characters but newlines and
/// tabs (app.mjs `clean`).
fn clean(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n").chars().filter(|c| !matches!(*c as u32, 0..=8 | 0x0b..=0x1f | 0x7f)).collect()
}

/// The wiki a page is in, as the brain names it (app.mjs `wikiOf`): its
/// top-level folder, or none at the root.
fn wiki_field(path: &str) -> String {
    path.split_once('/').map_or(String::new(), |(w, _)| clip(&clean(w), FIELD_MAX_BYTES))
}

/// The fragment a page is in when the corpus is cut by wiki: its top-level
/// folder, or `.` for a page at the root.
pub fn wiki_of(path: &str) -> &str {
    path.split_once('/').map_or(".", |(w, _)| w)
}

/// A page as the brain writes it to its index (app.mjs `source`, held to
/// it by the parity fixtures): its path the source, its title clipped, its
/// wiki a field, and each section whose text is something once its wiki
/// links read as their words and it is cleaned, with the headings above it
/// as context, each clipped.
pub fn upsert(page: &Page, sections: &[Section]) -> (wire::Upsert, Vec<String>) {
    let title = sections.first().map(|s| s.title.clone()).unwrap_or_else(|| crate::corpus::stem(&page.path));
    let mut out = Vec::new();
    let mut ids = Vec::new();
    for (i, s) in sections.iter().enumerate() {
        let text = clean(&crate::corpus::link_words(&s.body));
        if text.trim().is_empty() {
            continue;
        }
        let context = s.ancestry.iter().rev().take(CONTEXT_MAX).rev().map(|a| clip(&clean(a), CONTEXT_ITEM_MAX_BYTES)).collect();
        let heading = s.heading.as_deref().filter(|h| !h.is_empty()).map(|h| clip(&clean(h), HEADING_MAX_BYTES));
        out.push(wire::Section { id: format!("s{i}"), text, heading, context, at: None, location: None });
        ids.push(s.id.clone());
    }
    let wiki = wiki_field(&page.path);
    let fields = if wiki.is_empty() { Default::default() } else { [("wiki".to_string(), wiki)].into_iter().collect() };
    let up = wire::Upsert { source: page.path.clone(), revision: "r".into(), replaces: None, title: Some(clip(&clean(&title), TITLE_MAX_BYTES)), at: None, fields, sections: out, fence: None };
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
    width: usize,
}

/// One section an index answered: its place in its own answer, by words
/// and by meaning (as a fragment's `/search/asked` answer names them).
pub struct Hit {
    pub section: String,
    pub lexical: Option<usize>,
    pub cosine: Option<f64>,
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
        let mut ix = Index { conn, section_of: Vec::new(), codes: Vec::new(), q: Vec::new(), width };
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

    /// The question, answered as cell/src/search_index.rs's `search` does,
    /// its best `limit` sections.
    pub fn search(&self, q: &str, question: &[f32], limit: usize) -> Result<Vec<Hit>, String> {
        let mut keys: HashMap<String, i64> = HashMap::new();
        let mut names: Vec<String> = Vec::new();
        // the words leg: the best DEPTH chunks by BM25 over their fields (title 4, heading 3, context 2, body 1)
        let mut lexical = Vec::new();
        if let Some(words) = engine::match_of(q) {
            let mut stmt = self
                .conn
                .prepare_cached("SELECT rowid, -bm25(chunk_words, 4.0, 3.0, 2.0, 1.0) AS score FROM chunk_words WHERE chunk_words MATCH ? ORDER BY score DESC, rowid LIMIT ?")
                .map_err(|e| e.to_string())?;
            let rows = stmt.query_map(params![words, engine::DEPTH as i64], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?))).map_err(|e| e.to_string())?;
            for row in rows {
                let (chunk, score) = row.map_err(|e| e.to_string())?;
                lexical.push(engine::Found { section: self.section_key(&mut keys, &mut names, chunk), chunk, score });
            }
        }
        // the meaning leg: every sign code against the question, the best `candidates` reranked by int8
        let mut semantic = Vec::new();
        if let Some(unit) = engine::unit(question) {
            let held = engine::hold(&unit).expect("a unit vector is held");
            let table = engine::Table::of(&unit);
            let scored: Vec<(i64, f32)> = self.codes.chunks_exact(self.width).enumerate().map(|(i, code)| (i as i64 + 1, table.score(code))).collect();
            let first = engine::best(scored, engine::candidates(self.chunks()));
            let reranked: Vec<(i64, f32)> = first.iter().map(|(id, _)| (*id, engine::dot(&held.q, &self.q[*id as usize - 1]) as f32)).collect();
            for (chunk, dot) in engine::best(reranked, engine::DEPTH) {
                semantic.push(engine::Found { section: self.section_key(&mut keys, &mut names, chunk), chunk, score: engine::cosine(dot as i32) });
            }
        }
        Ok(engine::fuse(&lexical, &semantic, limit)
            .into_iter()
            .map(|f| Hit { section: names[f.section as usize].clone(), lexical: f.lexical.map(|p| p.rank), cosine: f.semantic.map(|p| p.score) })
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
