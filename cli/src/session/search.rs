//! An archive's conversations into the platform's search, written by the
//! import that holds their bytes (fragment_core::session_search makes each
//! one's sections) through the search write API: `GET
//! /api/f/{name}/search/sources`, `POST …/search/upsert` and `…/delete`.
//!
//! After an import's writes, the archive's search is reconciled with what
//! it holds: each conversation whose sha256 (its index entry's) is not the
//! revision search holds for its file is written, and each of the archive's
//! sources search holds that the archive no longer does is deleted. So an
//! import that writes nothing still puts search right, and an archive made
//! before its host had search is searched from its next import. A host
//! without search (its routes 404), or one that cannot be reached, is said
//! and skipped: the import stands.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use fragment_core::session::{self, Entry, Session};
use fragment_core::session_search;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{said_id, Spool};
use crate::api::{encode_q, Client};
use crate::blobs::Blobs;
use crate::codestorage::{CodeStorage, MAIN};

/// Sources one listing asks for (the API's most).
const PAGE: usize = 1000;
/// Pages read, at most: an archive's conversations, and a little more.
const PAGES_MAX: usize = session::SESSIONS_MAX / PAGE + 4;
/// Conversations written at once.
const WRITERS: usize = 8;
/// Conversations a refusal is said for, at most (the rest counted).
const REFUSED_SAID: usize = 20;

/// What an import did to its archive's search.
#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Searched {
    /// Nothing written: the host has no search, or it could not be reached.
    Skipped { why: String },
    Written {
        written: usize,
        deleted: usize,
        unchanged: usize,
        /// Sections past search's limits, left out of their conversations.
        left_out: usize,
        /// Conversations search refused or could not take, each with why
        /// (at most `REFUSED_SAID`), and how many in all.
        refused: Vec<super::RefusedOne>,
        refused_count: usize,
    },
}

#[derive(Deserialize)]
struct Page {
    sources: Vec<Listed>,
    next: Option<String>,
}

#[derive(Deserialize)]
struct Listed {
    source: String,
    revision: String,
}

/// The revision search holds of each source, or why there is none to read.
fn listed(c: &Client, name: &str) -> std::result::Result<BTreeMap<String, String>, String> {
    let mut held = BTreeMap::new();
    let mut after: Option<String> = None;
    for _ in 0..PAGES_MAX {
        let from = after.as_deref().map(|a| format!("&after={}", encode_q(a))).unwrap_or_default();
        let resp = c.get(&format!("/api/f/{name}/search/sources?limit={PAGE}{from}")).map_err(|e| format!("{e:#}"))?;
        if resp.status == 404 {
            return Err("this host has no search yet".into());
        }
        let page: Page = c.call_as(resp).map_err(|e| format!("{e:#}"))?;
        held.extend(page.sources.into_iter().map(|s| (s.source, s.revision)));
        match page.next {
            None => return Ok(held),
            Some(n) if after.as_ref().is_some_and(|a| n <= *a) => return Err(format!("search's sources stopped moving on at {}", said_id(&n))),
            Some(n) => after = Some(n),
        }
    }
    Err(format!("search lists more than {} sources", PAGES_MAX * PAGE))
}

/// `name`'s search put right with `entries` (every conversation it holds
/// after the import), each read from `spool` when the import holds it, else
/// from the archive.
pub fn sync(c: &Client, name: &str, spool: &Spool, entries: &BTreeMap<String, Entry>) -> Searched {
    let held = match listed(c, name) {
        Ok(held) => held,
        Err(why) => return Searched::Skipped { why },
    };
    let todo: Vec<(&String, &Entry)> = entries.iter().filter(|(k, e)| held.get(&session::session_path(k)) != Some(&e.sha256)).collect();
    let gone: Vec<&String> = held.keys().filter(|s| session::key_of_path(s).is_some_and(|k| !entries.contains_key(k))).collect();
    let storage = match CodeStorage::connect(c, name) {
        Ok(s) => s,
        Err(e) => return Searched::Skipped { why: format!("{e:#}") },
    };
    // a refusal that would be every one's (not allowed, no route): stop
    let stop = AtomicBool::new(false);
    let write = |todo: &[(&String, &Entry)]| -> Vec<std::result::Result<usize, (String, String)>> {
        let blobs = Blobs::new(c, name);
        let mut out = Vec::new();
        for (key, e) in todo {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            out.push(write_one(c, name, spool, &storage, &blobs, key, e).map_err(|(status, why)| {
                if matches!(status, Some(401 | 403 | 404)) {
                    stop.store(true, Ordering::Relaxed);
                }
                (said_id(key), why)
            }));
        }
        out
    };
    let per = todo.len().div_ceil(WRITERS).max(1);
    let results: Vec<_> = std::thread::scope(|scope| {
        let writers: Vec<_> = todo.chunks(per).map(|part| scope.spawn(move || write(part))).collect();
        writers.into_iter().flat_map(|w| w.join().unwrap_or_else(|_| vec![Err(("a writer".into(), "panicked".into()))])).collect()
    });
    let (mut written, mut left_out, mut refused, mut refused_count) = (0, 0, Vec::new(), 0);
    for r in results {
        match r {
            Ok(left) => {
                written += 1;
                left_out += left;
            }
            Err((id, why)) => {
                refused_count += 1;
                if refused.len() < REFUSED_SAID {
                    refused.push(super::RefusedOne { id, why });
                }
            }
        }
    }
    if stop.load(Ordering::Relaxed) {
        let why = refused.first().map_or_else(String::new, |r| r.why.clone());
        return Searched::Skipped { why: format!("search refused this archive's writes: {why}") };
    }
    let mut deleted = 0;
    for source in gone {
        match c.post_json_by_id(&format!("/api/f/{name}/search/delete"), &json!({ "source": source })) {
            Ok(r) if r.ok() => deleted += 1,
            Ok(r) => {
                refused_count += 1;
                if refused.len() < REFUSED_SAID {
                    refused.push(super::RefusedOne { id: said_id(source), why: format!("its delete: {}", r.refusal().msg) });
                }
            }
            Err(e) => {
                refused_count += 1;
                if refused.len() < REFUSED_SAID {
                    refused.push(super::RefusedOne { id: said_id(source), why: format!("its delete: {e:#}") });
                }
            }
        }
    }
    Searched::Written { written, deleted, unchanged: entries.len() - todo.len(), left_out, refused, refused_count }
}

/// One conversation written: its sections left out, or the status of a
/// refusal (when search answered) and why.
fn write_one(c: &Client, name: &str, spool: &Spool, storage: &CodeStorage, blobs: &Blobs, key: &str, e: &Entry) -> std::result::Result<usize, (Option<u16>, String)> {
    let path = session::session_path(key);
    let bytes = if spool.entries.get(key).is_some_and(|s| s.sha256 == e.sha256) {
        std::fs::read(spool.path(key)).map_err(|err| (None, format!("reading it back: {err}")))?
    } else {
        read(storage, blobs, &path).map_err(|err| (None, format!("reading it: {err:#}")))?
    };
    let s: Session = serde_json::from_slice(&bytes).map_err(|err| (None, format!("it is no conversation: {err}")))?;
    let indexed = session_search::upsert_of(&path, &e.sha256, &s);
    match c.post_json_by_id(&format!("/api/f/{name}/search/upsert"), &indexed.upsert) {
        Ok(r) if r.ok() => Ok(indexed.left),
        Ok(r) => Err((Some(r.status), r.refusal().msg)),
        Err(err) => Err((None, format!("{err:#}"))),
    }
}

fn read(storage: &CodeStorage, blobs: &Blobs, path: &str) -> Result<Vec<u8>> {
    blobs.resolve(storage.read_file(path, MAIN)?)
}
