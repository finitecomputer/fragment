//! Local-current search. This interface takes a folder and a fragment identity,
//! never a client. The FTS index lives only in memory and is rebuilt per call:
//! no watcher, stat cache, or persisted index can hide an edit or deletion.
use crate::sync;
use anyhow::{Context, Result};
use fragment_core::blob::sha256_hex;
use fragment_proto::RepoPath;
use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use regex::Regex;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::{collections::BTreeSet, fs, io::Read, path::Path, sync::LazyLock};
use unicode_normalization::UnicodeNormalization;

const PAGE_BYTES: u64 = 1024 * 1024;
static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\p{L}\p{N}\p{M}]+").unwrap());

pub fn validate_query(q: &str) -> std::result::Result<Vec<String>, String> {
    if q.chars().count() > 256 {
        return Err("search needs 1–256 characters and 1–16 words".into());
    }
    let normalized: String = q.nfkc().collect::<String>().to_lowercase();
    let words: Vec<String> = WORD
        .find_iter(&normalized)
        .map(|m| m.as_str().to_owned())
        .collect();
    if words.is_empty() || words.len() > 16 {
        return Err("search needs 1–256 characters and 1–16 words".into());
    }
    Ok(words)
}

#[derive(Serialize)]
pub struct Answer {
    pub q: String,
    pub words: Vec<String>,
    pub results: Vec<Hit>,
    pub skipped: Vec<Skipped>,
}
#[derive(Serialize)]
pub struct Hit {
    pub rank: usize,
    pub fragment: String,
    pub path: String,
    pub title: String,
    pub heading: Option<String>,
    pub ancestry: Vec<String>,
    /// synced means equal to the last local sync journal, not a remote check.
    pub state: String,
    pub snippet: String,
}
#[derive(Serialize)]
pub struct Skipped {
    pub path: String,
    pub reason: String,
}

fn indexable(path: &str) -> bool {
    let lower = path.to_lowercase();
    (lower.ends_with(".md") || lower.ends_with(".markdown"))
        && !path.starts_with("site/")
        && !path.starts_with("applib/")
        && !matches!(lower.rsplit('/').next(), Some("agents.md" | "_index.md"))
}

// Sync splits a conflict name before the final extension. Keep its sibling
// marked until the copy is removed, even if sync has since adopted our hash.
fn conflict_original(path: &str) -> Option<String> {
    let (stem, suffix) = path.rsplit_once(".conflict-")?;
    let (stamp, writer_ext) = suffix.split_once('-')?;
    if stamp.parse::<u64>().is_err() {
        return None;
    }
    let (writer, ext) = writer_ext
        .split_once('.')
        .map_or((writer_ext, ""), |(w, e)| (w, e));
    if writer.is_empty() {
        return None;
    }
    Some(if ext.is_empty() {
        stem.into()
    } else {
        format!("{stem}.{ext}")
    })
}

pub fn search(name: &str, dir: &Path, q: &str, limit: u32) -> Result<Answer> {
    let words = validate_query(q).map_err(anyhow::Error::msg)?;
    anyhow::ensure!((1..=50).contains(&limit), "limit must be 1–50");
    anyhow::ensure!(dir.is_dir(), "{} is not a directory", dir.display());
    let state = sync::load_state(dir, name)?;
    let root = dir.to_path_buf();
    // Same ignore rules as sync, except conflict copies must be inventoried.
    // They are evidence, never indexed as ordinary published content.
    let walker = ignore::WalkBuilder::new(dir)
        .hidden(false)
        .filter_entry(move |e| {
            let rel = sync::relative(&root, e.path());
            e.depth() == 0 || sync::syncable(&conflict_original(&rel).unwrap_or(rel))
        })
        .build();
    let mut paths = BTreeSet::new();
    let mut conflicts = BTreeSet::new();
    for entry in walker {
        let entry = entry?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if entry.path().to_str().is_none() {
            continue;
        }
        let path = sync::relative(dir, entry.path());
        if let Some(original) = conflict_original(&path) {
            conflicts.insert(original);
        } else if RepoPath::parse(&path).is_ok() && indexable(&path) {
            paths.insert(path);
        }
    }
    let mut db = Connection::open_in_memory()?;
    let tx = db.transaction()?;
    tx.execute_batch("CREATE VIRTUAL TABLE sections USING fts5(path UNINDEXED, state UNINDEXED, title, location, heading, ancestry, body, tokenize='unicode61 remove_diacritics 2');")?;
    let mut skipped = Vec::new();
    for path in paths {
        let mut bytes = Vec::new();
        fs::File::open(dir.join(&path))
            .with_context(|| format!("read {path}"))?
            .take(PAGE_BYTES)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 >= PAGE_BYTES {
            skipped.push(Skipped {
                path,
                reason: "page is 1 MiB or larger".into(),
            });
            continue;
        }
        let Ok(text) = std::str::from_utf8(&bytes) else {
            skipped.push(Skipped {
                path,
                reason: "not UTF-8".into(),
            });
            continue;
        };
        let status = if state.files.get(path.as_str()).is_some_and(|s| s.conflicted)
            || conflicts.contains(&path)
            || text
                .lines()
                .any(|l| l.starts_with("<<<<<<< ") || l == "=======" || l.starts_with(">>>>>>> "))
        {
            "conflicted"
        } else if state
            .files
            .get(path.as_str())
            .is_some_and(|s| s.sha256 == sha256_hex(&bytes))
        {
            "synced"
        } else {
            "local_only"
        };
        let (title, sections, truncated) = sections(&path, text);
        if truncated {
            skipped.push(Skipped {
                path: path.clone(),
                reason: "sections beyond the first 500 omitted".into(),
            });
        }
        for section in sections {
            tx.execute(
                "INSERT INTO sections VALUES (?1,?2,?3,?1,?4,?5,?6)",
                params![
                    path,
                    status,
                    title,
                    section.heading,
                    serde_json::to_string(&section.ancestry)?,
                    section.body
                ],
            )?;
        }
    }
    tx.commit()?;
    // Parameters contain only quoted prefix words: no FTS operators from q.
    let expression = words
        .iter()
        .map(|w| format!("\"{}\"*", w.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ");
    let mut stmt = db.prepare("SELECT path,state,title,heading,ancestry,snippet(sections,6,'','','…',24) FROM sections WHERE sections MATCH ?1 ORDER BY bm25(sections,0,0,4,2,3,2,1),path,rowid LIMIT ?2")?;
    let rows = stmt.query_map(params![expression, limit], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
        ))
    })?;
    let mut results = Vec::new();
    for row in rows {
        let (path, state, title, heading, ancestry, snippet) = row?;
        let snippet = snippet.split_whitespace().collect::<Vec<_>>().join(" ");
        results.push(Hit {
            rank: results.len() + 1,
            fragment: name.into(),
            path,
            state,
            title,
            heading,
            ancestry: serde_json::from_str(&ancestry)?,
            snippet: clip(&snippet, 300),
        });
    }
    Ok(Answer {
        q: q.into(),
        words,
        results,
        skipped,
    })
}

fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.into();
    }
    let mut end = max - '…'.len_utf8();
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

struct Section {
    heading: Option<String>,
    ancestry: Vec<String>,
    body: String,
}

// CommonMark supplies heading boundaries and rendered heading text, ignoring
// fenced code. Frontmatter's title overrides H1, as in the brain template.
fn sections(path: &str, text: &str) -> (String, Vec<Section>, bool) {
    let text = text.trim_start_matches('\u{feff}');
    let mut body = text;
    let mut title = None;
    let mut lead = String::new();
    if let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    {
        let mut offset = text.len() - rest.len();
        for line in rest.split_inclusive('\n') {
            offset += line.len();
            if matches!(line.trim(), "---" | "...") {
                body = &text[offset..];
                break;
            }
            if let Some((key, value)) = line.split_once(':') {
                let value = value.trim().trim_matches(['\'', '"']);
                match key {
                    "title" if !value.is_empty() => title = Some(value.to_string()),
                    "summary" | "description" | "tags" => {
                        lead.push_str(value);
                        lead.push('\n');
                    }
                    _ => {}
                }
            }
        }
        if body.len() == text.len() {
            // An unclosed block is ordinary Markdown, not metadata.
            title = None;
            lead.clear();
        }
    }
    let mut heads = Vec::new();
    let mut current = None;
    for (event, range) in Parser::new(body).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                current = Some((level as usize, range.start, range.end, String::new()))
            }
            Event::Text(s) | Event::Code(s) => {
                if let Some(h) = &mut current {
                    h.3.push_str(&s);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(h) = &mut current {
                    h.3.push(' ');
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(h) = current.take() {
                    heads.push(h);
                }
            }
            _ => {}
        }
    }
    let title = title
        .or_else(|| heads.iter().find(|h| h.0 == 1).map(|h| h.3.clone()))
        .unwrap_or_else(|| {
            Path::new(path)
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .replace(['-', '_'], " ")
        });
    let mut out = Vec::new();
    let mut push = |heading: Option<String>, ancestry: Vec<String>, text: String| {
        if text.trim().is_empty() && heading.is_none() {
            return;
        }
        let chars: Vec<char> = text.trim().chars().collect();
        if chars.is_empty() {
            out.push(Section {
                heading,
                ancestry,
                body: String::new(),
            });
        } else {
            for chunk in chars.chunks(8000) {
                out.push(Section {
                    heading: heading.clone(),
                    ancestry: ancestry.clone(),
                    body: chunk.iter().collect(),
                });
            }
        }
    };
    push(
        None,
        vec![],
        format!(
            "{lead}\n{}",
            &body[..heads.first().map_or(body.len(), |h| h.1)]
        ),
    );
    let mut above: Vec<(usize, String)> = Vec::new();
    for (i, h) in heads.iter().enumerate() {
        while above.last().is_some_and(|a| a.0 >= h.0) {
            above.pop();
        }
        push(
            Some(h.3.clone()),
            above.iter().map(|a| a.1.clone()).collect(),
            body[h.2..heads.get(i + 1).map_or(body.len(), |h| h.1)].into(),
        );
        above.push((h.0, h.3.clone()));
    }
    let truncated = out.len() > 500;
    out.truncate(500);
    (title.chars().take(300).collect(), out, truncated)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn section_limits_and_unclosed_metadata() {
        let (_, parts, truncated) = sections("a.md", &format!("# Title\n{}", "é".repeat(8001)));
        assert!(!truncated);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].body.chars().count(), 8000);
        let (_, parts, truncated) = sections("a.md", &"## Leaf\ntext\n".repeat(501));
        assert!(truncated);
        assert_eq!(parts.len(), 500);
        let (title, _, _) = sections("a.md", "---\ntitle: not metadata\n# Real title");
        assert_eq!(title, "Real title");
    }
    #[test]
    fn commonmark_sections_keep_ancestry_and_fences() {
        let (title, parts, _) = sections("wiki/a.md", "---\ntitle: 'Garden'\n---\n# Root\nintro\n## **Leaf**\nfind this\n```md\n# fake\n```\n");
        assert_eq!(title, "Garden");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1].heading.as_deref(), Some("Leaf"));
        assert_eq!(parts[1].ancestry, vec!["Root"]);
        assert!(parts[1].body.contains("# fake"));
    }
    #[test]
    fn query_is_words_and_clips_utf8() {
        assert_eq!(
            validate_query("café OR title:hello*").unwrap(),
            vec!["café", "or", "title", "hello"]
        );
        assert!(validate_query("***").is_err());
        assert!(validate_query(&"word ".repeat(17)).is_err());
        assert!(clip(&"é".repeat(200), 300).len() <= 300);
    }
}
