//! The platform's search write API (docs/api.md, "A fragment's search"),
//! for a writer's tests with no cell: one index a fragment, keyed by
//! source, each upsert replacing every section its source had, the same
//! (source, revision) again writing nothing, and its sources listed in
//! order, a page at a time. An upsert is checked as the cell checks it
//! (`fragment_proto::search::Upsert`, `fragment_core::searchable::check`),
//! so a writer that passes here passes there; it embeds nothing and ranks
//! nothing (no read route). The e2e's search section runs a writer against
//! the real service.
//!
//! Routes, under `/api/f/{name}/search/` (the code.storage fake's host
//! routes, answered for a fragment once a test turns its search on): `POST
//! upsert` (`{source, revision, written, chunks}`), `POST delete {source}`,
//! `GET sources?after=&limit=` (`{sources: [{source, revision, at,
//! chunks}], next}`, `next` the last source given while more follow, else
//! null), `GET status`.

use std::collections::BTreeMap;

use fragment_core::searchable;
use fragment_proto::search::{Section, Upsert, SOURCES_PAGE_MAX, UPSERT_MAX_BYTES};
use serde_json::{json, Value};

use crate::http::{Request, Response};

/// A source as the index holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct Held {
    pub revision: String,
    pub title: Option<String>,
    pub at: Option<String>,
    pub fields: BTreeMap<String, String>,
    pub sections: Vec<Section>,
}

/// One fragment's index.
#[derive(Debug, Default)]
pub struct Index {
    pub sources: BTreeMap<String, Held>,
    /// Upserts that wrote (a new revision), and that changed nothing.
    pub written: u32,
    pub unchanged: u32,
    /// Chunks it holds at most (each section one here), as the cell's
    /// index caps them: a write past it is refused, 507 `storage_full`.
    pub chunks_max: Option<usize>,
}

fn refused(status: u16, message: &str) -> Response {
    // the cell's refusal (fragment_proto::ErrorBody)
    let code = if status == 413 { "too_large" } else { "invalid_request" };
    Response::json(status, &json!({ "error": code, "message": message }))
}

/// An upsert's body, checked as the cell checks it: its source, the
/// revision it replaces (`replaces`), and what it holds.
fn upsert_of(body: &[u8]) -> Result<(String, Option<String>, Held), String> {
    let up: Upsert = serde_json::from_slice(body).map_err(|e| format!("the body is no upsert: {e}"))?;
    searchable::check(&up)?;
    Ok((up.source, up.replaces, Held { revision: up.revision, title: up.title, at: up.at, fields: up.fields, sections: up.sections }))
}

impl Index {
    /// One request to `op` (what follows `/search/`).
    pub fn handle(&mut self, op: &str, req: &Request) -> Response {
        match (req.method.as_str(), op) {
            ("POST", "upsert") => {
                if req.body.len() > UPSERT_MAX_BYTES {
                    return refused(413, &format!("an upsert is at most {UPSERT_MAX_BYTES} bytes"));
                }
                let (source, replaces, held) = match upsert_of(&req.body) {
                    Ok(u) => u,
                    Err(why) => return refused(400, &why),
                };
                // as the service: the same revision again writes nothing; else
                // one naming another revision than the source's is refused
                let current = self.sources.get(&source).map(|h| h.revision.clone());
                if current.as_deref() != Some(held.revision.as_str()) {
                    if let Some(r) = replaces {
                        if (r.is_empty() && current.is_some()) || (!r.is_empty() && current.as_deref() != Some(r.as_str())) {
                            return Response::json(409, &json!({ "error": "conflict", "message": "the source's revision is not the one this replaces", "current": current }));
                        }
                    }
                }
                let changed = current.as_deref() != Some(held.revision.as_str());
                let sections = held.sections.len();
                let holds = self.sources.iter().filter(|(s, _)| **s != source).map(|(_, h)| h.sections.len()).sum::<usize>() + sections;
                if let Some(max) = self.chunks_max.filter(|max| changed && holds > *max) {
                    let message = format!("this fragment's index holds {holds} chunks, at most {max}: split its sources across fragments");
                    return Response::json(507, &json!({ "error": "storage_full", "message": message }));
                }
                if changed {
                    self.written += 1;
                    self.sources.insert(source.clone(), held);
                } else {
                    self.unchanged += 1;
                }
                Response::json(200, &json!({ "source": source, "revision": self.sources[&source].revision, "written": changed, "chunks": sections }))
            }
            ("POST", "delete") => {
                let v: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
                let Some(source) = v["source"].as_str() else { return refused(400, "source is required") };
                let deleted = self.sources.remove(source).is_some();
                Response::json(200, &json!({ "source": source, "deleted": deleted }))
            }
            ("GET", "sources") => {
                let limit = match req.query.get("limit").map(|l| l.parse::<usize>()) {
                    None => SOURCES_PAGE_MAX,
                    Some(Ok(l)) if (1..=SOURCES_PAGE_MAX).contains(&l) => l,
                    Some(_) => return refused(400, &format!("limit is 1 to {SOURCES_PAGE_MAX}")),
                };
                let after = req.query.get("after").filter(|a| !a.is_empty());
                let mut rest = self.sources.iter().filter(|(s, _)| after.is_none_or(|a| s.as_str() > a.as_str()));
                let page: Vec<Value> = rest.by_ref().take(limit).map(|(s, h)| json!({ "source": s, "revision": h.revision, "at": h.at, "chunks": h.sections.len() })).collect();
                let next = if rest.next().is_some() { page.last().map(|p| p["source"].clone()) } else { None };
                Response::json(200, &json!({ "sources": page, "next": next }))
            }
            ("GET", "status") => {
                let sections: usize = self.sources.values().map(|h| h.sections.len()).sum();
                Response::json(200, &json!({ "sources": self.sources.len(), "sections": sections }))
            }
            _ => refused(404, &format!("no search route {} {op}", req.method)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(index: &mut Index, op: &str, body: &Value) -> (u16, Value) {
        let r = index.handle(op, &Request::new("POST", &format!("/{op}"), &[], body.to_string().as_bytes()));
        (r.status, serde_json::from_slice(&r.body).unwrap())
    }

    fn get(index: &mut Index, target: &str) -> Value {
        let (op, _) = target.split_once('?').unwrap_or((target, ""));
        let r = index.handle(op, &Request::new("GET", target, &[], b""));
        assert_eq!(r.status, 200);
        serde_json::from_slice(&r.body).unwrap()
    }

    /// Goal: the fake holds a writer to the contract: an upsert replaces its
    /// source, the same revision changes nothing, sources list in order a
    /// page at a time, and what breaks a limit is refused. Valid, replay,
    /// invalid.
    #[test]
    fn the_fake_holds_a_writer_to_the_contract() {
        let mut ix = Index::default();
        let up = |source: &str, revision: &str, ids: &[&str]| json!({ "source": source, "revision": revision, "sections": ids.iter().map(|id| json!({ "id": id, "text": "x" })).collect::<Vec<_>>() });
        assert_eq!(post(&mut ix, "upsert", &up("sessions/b.json", "1", &["t0", "t1"])).1["written"], true);
        assert_eq!(post(&mut ix, "upsert", &up("sessions/b.json", "1", &["t0", "t1"])).1["written"], false);
        assert_eq!(post(&mut ix, "upsert", &up("sessions/b.json", "2", &["t0"])).1["chunks"], 1);
        assert_eq!(ix.sources["sessions/b.json"].sections.len(), 1, "an upsert replaces its source's sections");
        let mut stale = up("sessions/b.json", "3", &["t0"]);
        stale["replaces"] = json!("1");
        assert_eq!(post(&mut ix, "upsert", &stale), (409, json!({ "error": "conflict", "message": "the source's revision is not the one this replaces", "current": "2" })));
        stale["replaces"] = json!("2");
        assert_eq!(post(&mut ix, "upsert", &stale).1["written"], true, "one naming the source's revision is written");
        let mut fresh = up("sessions/new.json", "1", &["t0"]);
        fresh["replaces"] = json!("");
        assert_eq!(post(&mut ix, "upsert", &fresh).1["written"], true, "\"\" replaces none");
        assert_eq!(post(&mut ix, "upsert", &fresh).1["written"], false, "the same revision again writes nothing, whatever it replaces");
        ix.sources.remove("sessions/new.json");
        for s in ["sessions/a.json", "sessions/c.json"] {
            post(&mut ix, "upsert", &up(s, "1", &["t0"]));
        }
        let page = get(&mut ix, "sources?limit=2");
        assert_eq!((page["sources"][0]["source"].as_str(), page["next"].as_str()), (Some("sessions/a.json"), Some("sessions/b.json")));
        let page = get(&mut ix, "sources?limit=2&after=sessions%2Fb.json");
        assert_eq!((page["sources"].as_array().unwrap().len(), &page["next"]), (1, &Value::Null));
        assert_eq!(post(&mut ix, "delete", &json!({ "source": "sessions/a.json" })).1["deleted"], true);
        assert_eq!(get(&mut ix, "status")["sources"], 2);

        let long = "y".repeat(fragment_proto::search::TEXT_MAX_CHARS + 1);
        for bad in [
            json!({ "source": "s", "revision": "1", "sections": [{ "id": "t0", "text": "a" }, { "id": "t0", "text": "b" }] }),
            json!({ "source": "s", "revision": "1", "sections": [{ "id": "t0", "text": long }] }),
            json!({ "source": "s", "revision": "1", "sections": [{ "id": "x".repeat(65), "text": "a" }] }),
            json!({ "source": "s", "revision": "1", "fields": { "a": "1", "b": "2", "c": "3", "d": "4", "e": "5" }, "sections": [] }),
            json!({ "revision": "1", "sections": [] }),
            json!({ "source": "s", "revision": "1", "sections": [{ "id": "t0", "text": "a\u{7}" }] }),
            json!({ "source": "s", "revision": "1", "fields": { "kind": "a=b" }, "sections": [] }),
        ] {
            assert_eq!(post(&mut ix, "upsert", &bad).0, 400, "{bad}");
        }
        let too_many: Vec<Value> = (0..=fragment_proto::search::SECTIONS_MAX).map(|i| json!({ "id": format!("t{i}"), "text": "a" })).collect();
        assert_eq!(post(&mut ix, "upsert", &json!({ "source": "s", "revision": "1", "sections": too_many })).0, 400);
    }
}
