//! The platform's search write API as its contract has it, for a writer's
//! tests until the cell serves it (the search capability's PR): one index a
//! fragment, keyed by source, each upsert replacing every section its source
//! had, the same (source, revision) again changing nothing, and its
//! sources listed in order, a page at a time. It checks every limit the
//! contract sets, so a writer that passes it here is within them; it embeds
//! nothing and ranks nothing (no read route).
//!
//! Routes, under `/api/f/{name}/search/` (the code.storage fake's host
//! routes, answered for a fragment once a test turns its search on): `POST
//! upsert`, `POST delete {source}`, `GET sources?after=&limit=` (`{sources:
//! [{source, revision, sections, at}], next}`, `next` the last source given
//! while more follow, else null), `GET status`.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::http::{Request, Response};

pub const SECTIONS_MAX: usize = 2000;
pub const SECTION_TEXT_MAX_CHARS: usize = 8000;
pub const SECTION_ID_MAX_BYTES: usize = 64;
pub const UPSERT_MAX_BYTES: usize = 4 * 1024 * 1024;
pub const SOURCE_MAX_BYTES: usize = 512;
pub const REVISION_MAX_BYTES: usize = 128;
pub const FIELDS_MAX: usize = 4;
pub const SOURCES_PAGE_MAX: usize = 1000;

/// A source as the index holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct Held {
    pub revision: String,
    pub title: Option<String>,
    pub at: Option<String>,
    pub fields: BTreeMap<String, String>,
    pub sections: Vec<Value>,
}

/// One fragment's index.
#[derive(Debug, Default)]
pub struct Index {
    pub sources: BTreeMap<String, Held>,
    /// Upserts that wrote (a new revision), and that changed nothing.
    pub written: u32,
    pub unchanged: u32,
}

fn refused(status: u16, message: &str) -> Response {
    // the cell's refusal (fragment_proto::ErrorBody)
    let code = if status == 413 { "too_large" } else { "invalid_request" };
    Response::json(status, &json!({ "error": code, "message": message }))
}

fn text(v: &Value, key: &str, max_bytes: usize) -> Result<Option<String>, String> {
    match &v[key] {
        Value::Null => Ok(None),
        Value::String(s) if s.len() <= max_bytes => Ok(Some(s.clone())),
        Value::String(_) => Err(format!("{key} is past {max_bytes} bytes")),
        _ => Err(format!("{key} is not a string")),
    }
}

/// An upsert's body, checked as the contract has it.
fn upsert_of(body: &[u8]) -> Result<(String, Held), String> {
    let v: Value = serde_json::from_slice(body).map_err(|e| format!("the body is not JSON: {e}"))?;
    let source = text(&v, "source", SOURCE_MAX_BYTES)?.filter(|s| !s.is_empty()).ok_or("source is required")?;
    let revision = text(&v, "revision", REVISION_MAX_BYTES)?.filter(|s| !s.is_empty()).ok_or("revision is required")?;
    let mut fields = BTreeMap::new();
    match &v["fields"] {
        Value::Null => {}
        Value::Object(m) if m.len() <= FIELDS_MAX => {
            for (k, f) in m {
                let f = f.as_str().filter(|f| f.chars().count() <= 64).ok_or(format!("fields.{k} is not a short string"))?;
                fields.insert(k.clone(), f.to_string());
            }
        }
        _ => return Err(format!("fields is an object of at most {FIELDS_MAX} strings")),
    }
    let sections = v["sections"].as_array().ok_or("sections is a list")?;
    if sections.len() > SECTIONS_MAX {
        return Err(format!("{} sections, past {SECTIONS_MAX}", sections.len()));
    }
    let mut ids = std::collections::BTreeSet::new();
    for s in sections {
        let id = s["id"].as_str().filter(|id| !id.is_empty() && id.len() <= SECTION_ID_MAX_BYTES).ok_or("a section's id is 1 to 64 bytes")?;
        if !ids.insert(id) {
            return Err(format!("section id {id} is there twice"));
        }
        let t = s["text"].as_str().ok_or(format!("section {id} has no text"))?;
        if t.chars().count() > SECTION_TEXT_MAX_CHARS {
            return Err(format!("section {id}'s text is past {SECTION_TEXT_MAX_CHARS} characters"));
        }
        if !s["context"].is_null() && !s["context"].as_array().is_some_and(|c| c.iter().all(Value::is_string)) {
            return Err(format!("section {id}'s context is a list of strings"));
        }
        if let Some(l) = s.get("location") {
            if !(l["from"].is_u64() && l["to"].is_u64()) {
                return Err(format!("section {id}'s location is {{from, to}}"));
            }
        }
    }
    let held = Held { revision, title: text(&v, "title", 1024)?, at: text(&v, "at", 64)?, fields, sections: sections.clone() };
    Ok((source, held))
}

impl Index {
    /// One request to `op` (what follows `/search/`).
    pub fn handle(&mut self, op: &str, req: &Request) -> Response {
        match (req.method.as_str(), op) {
            ("POST", "upsert") => {
                if req.body.len() > UPSERT_MAX_BYTES {
                    return refused(413, &format!("an upsert is at most {UPSERT_MAX_BYTES} bytes"));
                }
                let (source, held) = match upsert_of(&req.body) {
                    Ok(u) => u,
                    Err(why) => return refused(400, &why),
                };
                let changed = self.sources.get(&source).is_none_or(|h| h.revision != held.revision);
                let sections = held.sections.len();
                if changed {
                    self.written += 1;
                    self.sources.insert(source.clone(), held);
                } else {
                    self.unchanged += 1;
                }
                Response::json(200, &json!({ "source": source, "sections": sections, "changed": changed }))
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
                let page: Vec<Value> = rest.by_ref().take(limit).map(|(s, h)| json!({ "source": s, "revision": h.revision, "sections": h.sections.len(), "at": h.at })).collect();
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
        assert_eq!(post(&mut ix, "upsert", &up("sessions/b.json", "1", &["t0", "t1"])).1["changed"], true);
        assert_eq!(post(&mut ix, "upsert", &up("sessions/b.json", "1", &["t0", "t1"])).1["changed"], false);
        assert_eq!(post(&mut ix, "upsert", &up("sessions/b.json", "2", &["t0"])).1["sections"], 1);
        assert_eq!(ix.sources["sessions/b.json"].sections.len(), 1, "an upsert replaces its source's sections");
        for s in ["sessions/a.json", "sessions/c.json"] {
            post(&mut ix, "upsert", &up(s, "1", &["t0"]));
        }
        let page = get(&mut ix, "sources?limit=2");
        assert_eq!((page["sources"][0]["source"].as_str(), page["next"].as_str()), (Some("sessions/a.json"), Some("sessions/b.json")));
        let page = get(&mut ix, "sources?limit=2&after=sessions%2Fb.json");
        assert_eq!((page["sources"].as_array().unwrap().len(), &page["next"]), (1, &Value::Null));
        assert_eq!(post(&mut ix, "delete", &json!({ "source": "sessions/a.json" })).1["deleted"], true);
        assert_eq!(get(&mut ix, "status")["sources"], 2);

        let long = "y".repeat(SECTION_TEXT_MAX_CHARS + 1);
        for bad in [
            json!({ "source": "s", "revision": "1", "sections": [{ "id": "t0", "text": "a" }, { "id": "t0", "text": "b" }] }),
            json!({ "source": "s", "revision": "1", "sections": [{ "id": "t0", "text": long }] }),
            json!({ "source": "s", "revision": "1", "sections": [{ "id": "x".repeat(65), "text": "a" }] }),
            json!({ "source": "s", "revision": "1", "fields": { "a": "1", "b": "2", "c": "3", "d": "4", "e": "5" }, "sections": [] }),
            json!({ "revision": "1", "sections": [] }),
        ] {
            assert_eq!(post(&mut ix, "upsert", &bad).0, 400, "{bad}");
        }
        let too_many: Vec<Value> = (0..=SECTIONS_MAX).map(|i| json!({ "id": format!("t{i}"), "text": "a" })).collect();
        assert_eq!(post(&mut ix, "upsert", &json!({ "source": "s", "revision": "1", "sections": too_many })).0, 400);
    }
}
