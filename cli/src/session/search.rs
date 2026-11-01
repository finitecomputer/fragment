//! An archive's conversations into the platform's search, written by the
//! import that holds their bytes (fragment_core::session_search makes each
//! one's sections) through the search write API: `GET
//! /api/f/{name}/search/sources` and `POST …/search/upsert`.
//!
//! After an import's writes, the archive's search is reconciled with the
//! archive **as it stands then**, not as this import left it: another
//! import may have written since (newer copies, conversations of its own).
//! Its indexes at main's head say each conversation's sha256, and its files
//! changed since them are read (`held`, `found`); each conversation whose
//! sha256 is not the revision search holds for its file is written, its
//! bytes read at that head and its revision theirs. Search is read before
//! the archive, so a revision it holds is never of a newer copy than the
//! archive read after. Each write names the revision it replaces, as
//! search was read holding it (`replaces`), so a write another import made
//! since is never written over; refused (409), that conversation is
//! reconciled again: search and the archive read anew, and written from
//! the archive's new head if they still differ, never with what was read
//! before. Whichever import's write lands first, search ends holding the
//! archive's copy, within `PASSES_MAX` passes; those still overtaken then
//! are said (`unresolved`), for the next import. Nothing is deleted here:
//! the platform forgets a conversation's source when a move of main
//! deletes its file (cell/src/searchable.rs), and what this import did not
//! see is no evidence of a file gone. So an import that writes nothing still puts
//! search right, and an archive made before its host had search is
//! searched from its next import. A host without search (its routes 404),
//! or one that cannot be reached, is said and skipped: the import stands.
//! A search that is full (507: an index's chunks are capped) takes no more,
//! so nothing more is sent: what was not is counted refused with it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use fragment_core::blob;
use fragment_core::session::{self, Entry, Session};
use fragment_core::{searchable, session_search};
use fragment_proto::search::SOURCES_PAGE_MAX;
use serde::{Deserialize, Serialize};

use super::{said_id, Spool};
use crate::api::{encode_q, Client};
use crate::blobs::Blobs;
use crate::codestorage::CodeStorage;

/// Sources one listing asks for (the API's most).
const PAGE: usize = SOURCES_PAGE_MAX;
/// Pages read, at most: an archive's conversations, and a little more.
const PAGES_MAX: usize = session::SESSIONS_MAX / PAGE + 4;
/// Conversations written at once.
const WRITERS: usize = 8;
/// Conversations a refusal is said for, at most (the rest counted).
const REFUSED_SAID: usize = 20;
/// Passes an import's search makes at most: the first, and one more while
/// a write was overtaken (409), search and the archive read anew each time.
pub(super) const PASSES_MAX: usize = 4;

/// What an import did to its archive's search.
#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Searched {
    /// Nothing written: the host has no search, or it could not be reached.
    Skipped { why: String },
    Written {
        written: usize,
        unchanged: usize,
        /// Conversations another import kept writing to search between
        /// this one's reading it and writing them, through its last pass:
        /// search may hold another copy than the archive's until an import
        /// after.
        unresolved: usize,
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

/// One conversation's write: its sections the limits left out, or
/// another's write since this import read search.
#[derive(Debug, PartialEq)]
pub(super) enum Wrote {
    Written(usize),
    Overtaken,
}

/// The archive at main's head as it stands now: the head, and each
/// conversation its `sessions/` holds, by key, as its indexes describe it
/// or, where they may not, as its file is (`found`).
fn archive_now(c: &Client, name: &str, storage: &CodeStorage, blobs: &Blobs) -> Result<(Option<String>, BTreeMap<String, Entry>)> {
    let h = super::held(storage, blobs)?;
    let found: BTreeMap<String, Entry> = super::found(c, name, storage, &h, &BTreeMap::new())?.into_iter().map(|e| (e.key.clone(), e)).collect();
    let entries = h.stored.keys().filter_map(|k| found.get(k).or_else(|| h.indexed.get(k)).map(|e| (k.clone(), e.clone()))).collect();
    Ok((h.head, entries))
}

/// One pass's writes: each conversation's key and how its write went, or
/// why it was refused (its id said); whether a refusal was every one's
/// (`stopped`: not allowed, no route); whether search was full (`full`:
/// those not sent are not among the results).
struct Pass<'a> {
    results: Vec<(&'a String, std::result::Result<Wrote, (String, String)>)>,
    stopped: bool,
    full: bool,
}

/// `todo` written over what search was read holding (`held`), by
/// `WRITERS` at once, each read at `head` (`write_one`).
#[allow(clippy::too_many_arguments)]
fn write_pass<'a>(c: &Client, name: &str, spool: &Spool, storage: &CodeStorage, head: &str, held: &BTreeMap<String, String>, todo: &[(&'a String, &'a Entry)]) -> Pass<'a> {
    // a refusal that would be every one's (not allowed, no route): stop
    let stop = AtomicBool::new(false);
    // search full (507): each write not yet sent would be refused too
    let full = AtomicBool::new(false);
    let write = |todo: &[(&'a String, &'a Entry)]| {
        let blobs = Blobs::new(c, name);
        let mut out = Vec::new();
        for &(key, e) in todo {
            if stop.load(Ordering::Relaxed) || full.load(Ordering::Relaxed) {
                break;
            }
            let replaces = held.get(&session::session_path(key)).map_or("", String::as_str);
            out.push((
                key,
                write_one(c, name, spool, storage, &blobs, head, key, e, replaces).map_err(|(status, why)| {
                    if matches!(status, Some(401 | 403 | 404)) {
                        stop.store(true, Ordering::Relaxed);
                    }
                    if status == Some(507) {
                        full.store(true, Ordering::Relaxed);
                    }
                    (said_id(key), why)
                }),
            ));
        }
        out
    };
    let per = todo.len().div_ceil(WRITERS).max(1);
    let results = std::thread::scope(|scope| {
        let writers: Vec<_> = todo.chunks(per).map(|part| (part, scope.spawn(move || write(part)))).collect();
        // a writer that panicked: each of its part refused, as unknown
        writers.into_iter().flat_map(|(part, w)| w.join().unwrap_or_else(|_| part.iter().map(|&(k, _)| (k, Err((said_id(k), "its writer panicked".into())))).collect())).collect()
    });
    Pass { results, stopped: stop.load(Ordering::Relaxed), full: full.load(Ordering::Relaxed) }
}

/// `name`'s search put right with the archive as it stands now, each
/// conversation read from `spool` when the import holds those bytes, else
/// from the archive at the head read. Each pass reads search, then the
/// archive (so a revision search holds is never newer than the archive
/// read after it), and writes each conversation whose sha256 is not the
/// revision held; those overtaken (another import wrote them since this
/// one read search) are reconciled again by the next pass, read anew, at
/// most `PASSES_MAX` passes, after which they are counted `unresolved`.
pub fn sync(c: &Client, name: &str, spool: &Spool) -> Searched {
    let mut held = match listed(c, name) {
        Ok(held) => held,
        Err(why) => return Searched::Skipped { why },
    };
    let storage = match CodeStorage::connect(c, name) {
        Ok(s) => s,
        Err(e) => return Searched::Skipped { why: format!("{e:#}") },
    };
    let blobs = Blobs::new(c, name);
    let (mut written, mut unchanged, mut left_out, mut refused, mut refused_count) = (0, 0, 0, Vec::new(), 0);
    // after a pass, the conversations its writes found overtaken
    let mut unresolved: BTreeSet<String> = BTreeSet::new();
    for pass in 0..PASSES_MAX {
        if pass > 0 {
            // what another import wrote since: read again, before the archive
            held = match listed(c, name) {
                Ok(held) => held,
                Err(_) => break,
            };
        }
        let (head, entries) = match archive_now(c, name, &storage, &blobs) {
            Ok(now) => now,
            Err(e) if pass == 0 => return Searched::Skipped { why: format!("reading the archive: {e:#}") },
            Err(_) => break,
        };
        let Some(head) = head else {
            // no conversations (any overtaken gone with them)
            unresolved.clear();
            break;
        };
        // the first pass, every conversation; each after, those overtaken
        let asked = if pass == 0 { entries.len() } else { unresolved.len() };
        let todo: Vec<(&String, &Entry)> =
            entries.iter().filter(|(k, _)| pass == 0 || unresolved.contains(*k)).filter(|(k, e)| held.get(&session::session_path(k)) != Some(&e.sha256)).collect();
        unchanged += asked - todo.len();
        let done = write_pass(c, name, spool, &storage, &head, &held, &todo);
        unresolved.clear();
        // those not sent once search was full
        if done.full {
            refused_count += todo.len().saturating_sub(done.results.len());
        }
        for (key, r) in done.results {
            match r {
                Ok(Wrote::Written(left)) => {
                    written += 1;
                    left_out += left;
                }
                Ok(Wrote::Overtaken) => {
                    unresolved.insert(key.clone());
                }
                Err((id, why)) => {
                    refused_count += 1;
                    if refused.len() < REFUSED_SAID {
                        refused.push(super::RefusedOne { id, why });
                    }
                }
            }
        }
        if done.stopped {
            let why = refused.first().map_or_else(String::new, |r| r.why.clone());
            return Searched::Skipped { why: format!("search refused this archive's writes: {why}") };
        }
        if unresolved.is_empty() || done.full {
            break;
        }
    }
    Searched::Written { written, unchanged, unresolved: unresolved.len(), left_out, refused, refused_count }
}

/// One conversation written, its bytes as the import spooled them when
/// they are the archive's at `head`, else read at `head`; its revision their
/// own sha256, written only over `replaces` (the revision search was read
/// holding, `""` for none). A refusal says its status (when search
/// answered) and why.
#[allow(clippy::too_many_arguments)]
pub(super) fn write_one(c: &Client, name: &str, spool: &Spool, storage: &CodeStorage, blobs: &Blobs, head: &str, key: &str, e: &Entry, replaces: &str) -> std::result::Result<Wrote, (Option<u16>, String)> {
    let path = session::session_path(key);
    let bytes = if spool.entries.get(key).is_some_and(|s| s.sha256 == e.sha256) {
        std::fs::read(spool.path(key)).map_err(|err| (None, format!("reading it back: {err}")))?
    } else {
        storage.read_file(&path, head).map_err(anyhow::Error::new).and_then(|b| blobs.resolve(b)).map_err(|err| (None, format!("reading it: {err:#}")))?
    };
    let s: Session = serde_json::from_slice(&bytes).map_err(|err| (None, format!("it is no conversation: {err}")))?;
    let mut indexed = session_search::upsert_of(&path, &blob::sha256_hex(&bytes), &s);
    indexed.upsert.replaces = Some(replaces.to_string());
    // what the platform would refuse is refused here, unsent
    searchable::check(&indexed.upsert).map_err(|why| (None, why))?;
    match c.post_json_by_id(&format!("/api/f/{name}/search/upsert"), &indexed.upsert) {
        Ok(r) if r.ok() => Ok(Wrote::Written(indexed.left)),
        // another import's write since this one read search: theirs stands
        Ok(r) if r.status == 409 => Ok(Wrote::Overtaken),
        Ok(r) => Err((Some(r.status), r.refusal().msg)),
        Err(err) => Err((None, format!("{err:#}"))),
    }
}
