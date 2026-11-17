//! The code store a local run's node keeps its git in (docs/self-host.md,
//! seam 5). By default it is the code.storage fake in this process, whose
//! levers the lanes pull. `FRAGMENT_E2E_CODESTORE=macrofiche` puts it
//! outside the run: macrofiche (`MACROFICHE_BIN`), started on the run's
//! scratch with an org key made for the run, and stopped with it.
//!
//! macrofiche is reached only through the contract's routes (its
//! docs/contract.md, section 5), with tokens the harness signs with the
//! org key, as the cell signs its own. The harness's writes are another
//! writer's: a commit pack on main, a deploy's ref move (live created at
//! main's tip, else merged to it), each followed by the owner's refresh,
//! as the CLI follows its own. A check that pulls one of the fake's levers
//! (an outage, a held or sabotaged call, counts of the requests it
//! answered) is a skip that says which (`Suite::skip_lever`).

use std::time::Duration;

use anyhow::{bail, Context, Result};
use fragment_core::codestorage::{self as cs, Claims, FileChange, OrgKey};
use serde_json::{json, Value};

/// Which store a local run uses.
pub const CODESTORE_VAR: &str = "FRAGMENT_E2E_CODESTORE";
/// The subject of the harness's own tokens (the cell's are
/// `fragment-runtime`, an editor's `editor:<id>`).
const SUBJECT: &str = "e2e-harness";
/// The harness's tokens live as long as the cell's own.
const TOKEN_TTL_S: i64 = 300;
/// One call's deadline, as the cell's.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Pages of the org's repos a lookup by name reads, 100 a page: the cell's bound.
const REPO_PAGES_MAX: usize = 100;
const REPO_PAGE: usize = 100;
/// Tries of one write that loses its compare-and-swap to another writer
/// (the cell commits beside the harness: a trigger's write).
const CAS_TRIES: usize = 5;
/// The newest commits a count reads: the contract's largest history page.
pub const HISTORY_MAX: usize = 100;
/// Who the harness's commits name.
const AUTHOR: (&str, &str) = ("e2e", "e2e@users.fragment");

/// Where a local run's git lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Fake,
    Macrofiche,
}

impl Choice {
    /// `FRAGMENT_E2E_CODESTORE`: `fake` (or unset) or `macrofiche`.
    pub fn from_env() -> Result<Choice> {
        match std::env::var(CODESTORE_VAR).as_deref() {
            Err(_) | Ok("fake") => Ok(Choice::Fake),
            Ok("macrofiche") => Ok(Choice::Macrofiche),
            Ok(other) => bail!("{CODESTORE_VAR} is fake or macrofiche, not {other:?}"),
        }
    }
}

/// A store outside the run, through its REST API.
pub struct External {
    pub url: String,
    pub org: String,
    key: OrgKey,
    http: reqwest::blocking::Client,
    /// What the run's banner and skips call it.
    pub label: String,
}

/// A store's answer: its status, its headers and its body.
pub struct Answer {
    pub status: u16,
    headers: reqwest::header::HeaderMap,
    pub body: Vec<u8>,
}

impl Answer {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// The body's first 300 bytes, as a check's detail shows it.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body[..self.body.len().min(300)]).into_owned()
    }

    /// A header's value ("" when absent).
    pub fn header(&self, name: &str) -> String {
        self.headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or("").to_string()
    }
}

impl std::fmt::Display for Answer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.status, self.text())
    }
}

/// A repo-scoped path's repo: the url form a create answered, kept to
/// these bytes by both the fake (a UUID) and the service (the name), so it
/// needs no escaping in a path.
fn checked_repo(repo: &str) -> Result<&str> {
    if repo.is_empty() || !repo.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.') {
        bail!("not a repo the harness names in a path: {repo:?}");
    }
    Ok(repo)
}

impl External {
    pub fn new(url: &str, org: &str, key_pem: &str, label: String) -> Result<External> {
        let key = OrgKey::from_pem(key_pem).map_err(|e| anyhow::anyhow!("{e}"))?;
        let http = reqwest::blocking::Client::builder().timeout(CALL_TIMEOUT).build()?;
        Ok(External { url: url.trim_end_matches('/').to_string(), org: org.to_string(), key, http, label })
    }

    /// A token for one call: `claim` is the token's repo (a repo, a new
    /// repo's name, a repo id, or the org), with exactly `scope`.
    pub fn token(&self, claim: &str, scope: &str) -> String {
        let now = crate::api::now_s();
        self.key.token(&Claims { iss: &self.org, sub: SUBJECT, repo: claim, scopes: &[scope], iat: now, exp: now + TOKEN_TTL_S })
    }

    /// One call, its token naming `claim` with exactly `scope`.
    pub fn call(&self, method: &str, path: &str, query: &[(&str, &str)], claim: &str, scope: &str, body: Option<(&str, Vec<u8>)>) -> Result<Answer> {
        self.send(method, path, query, Some(self.token(claim, scope)), body)
    }

    /// One call with the bearer given (`None`: none at all).
    pub fn send(&self, method: &str, path: &str, query: &[(&str, &str)], bearer: Option<String>, body: Option<(&str, Vec<u8>)>) -> Result<Answer> {
        let method = reqwest::Method::from_bytes(method.as_bytes())?;
        let mut req = self.http.request(method.clone(), format!("{}{path}", self.url)).query(query);
        if let Some(token) = bearer {
            req = req.bearer_auth(token);
        }
        if let Some((content_type, bytes)) = body {
            req = req.header("content-type", content_type).body(bytes);
        }
        let r = req.send().with_context(|| format!("{method} {}{path}", self.url))?;
        let (status, headers) = (r.status().as_u16(), r.headers().clone());
        let body = r.bytes().with_context(|| format!("{method} {path}: the body"))?.to_vec();
        Ok(Answer { status, headers, body })
    }

    /// One repo-scoped call (`/api/repos/{repo}/{op}`), its token naming
    /// the repo with exactly `scope`.
    pub fn repo_call(&self, method: &str, repo: &str, op: &str, query: &[(&str, &str)], scope: &str, body: Option<(&str, Vec<u8>)>) -> Result<Answer> {
        let repo = checked_repo(repo)?;
        self.call(method, &format!("/api/repos/{repo}/{op}"), query, repo, scope, body)
    }

    /// A branch's head, `None` when it is absent (404, as the cell reads it).
    pub fn head(&self, repo: &str, branch: &str) -> Result<Option<String>> {
        let a = self.repo_call("GET", repo, "branch", &[("name", branch)], "git:read", None)?;
        match a.status {
            200 => Ok(Some(cs::branch_head(&a.json()).with_context(|| format!("{branch}'s head is not a sha: {}", a.text()))?)),
            404 => Ok(None),
            s => bail!("GET branch {branch} of {repo}: {s} {}", a.text()),
        }
    }

    /// A file's bytes at a ref (a branch or a commit), `None` when absent.
    pub fn file(&self, repo: &str, at: &str, path: &str) -> Result<Option<Vec<u8>>> {
        let a = self.repo_call("GET", repo, "file", &[("path", path), ("ref", at)], "git:read", None)?;
        match a.status {
            200 => Ok(Some(a.body)),
            404 => Ok(None),
            s => bail!("GET file {path} at {at} of {repo}: {s} {}", a.text()),
        }
    }

    /// Every path at a ref, walked a page at a time (none when the ref is absent).
    pub fn paths(&self, repo: &str, at: &str) -> Result<Vec<String>> {
        let mut all = vec![];
        let mut cursor: Option<String> = None;
        for _ in 0..REPO_PAGES_MAX {
            let mut query = vec![("ref", at), ("limit", "1000")];
            if let Some(c) = &cursor {
                query.push(("cursor", c.as_str()));
            }
            let a = self.repo_call("GET", repo, "files/metadata", &query, "git:read", None)?;
            match a.status {
                200 => {}
                // no such branch: nothing there, as the fake reads one
                404 if a.json()["code"] != "repository_not_found" => return Ok(vec![]),
                _ => bail!("GET files/metadata at {at} of {repo}: {a}"),
            }
            let page = cs::tree_page(&a.json()).map_err(|e| anyhow::anyhow!("{e}"))?;
            all.extend(page.entries.into_iter().map(|e| e.path.to_string()));
            match page.next {
                Some(c) => cursor = Some(c),
                None => return Ok(all),
            }
        }
        bail!("{repo} lists more than {REPO_PAGES_MAX} pages at {at}")
    }

    /// The newest commits of a ref, newest first (at most `HISTORY_MAX`).
    pub fn history(&self, repo: &str, at: &str) -> Result<Vec<String>> {
        let limit = HISTORY_MAX.to_string();
        let a = self.repo_call("GET", repo, "commits", &[("ref", at), ("limit", &limit)], "git:read", None)?;
        if a.status != 200 {
            bail!("GET commits of {at} in {repo}: {a}");
        }
        let commits = a.json()["commits"].as_array().cloned().with_context(|| format!("a history without commits: {}", a.text()))?;
        commits.iter().map(|c| c["sha"].as_str().filter(|s| cs::is_sha(s)).map(str::to_string).with_context(|| format!("a commit without a sha: {c}"))).collect()
    }

    /// A commit of `changes` on `branch`, compare-and-swapped on the head
    /// read just before it, as every writer of fragment's commits: (the
    /// head before, the commit). A try that loses to another writer reads
    /// the head again (`CAS_TRIES`).
    pub fn commit(&self, repo: &str, branch: &str, changes: &[(&str, Option<&[u8]>)], message: &str) -> Result<(Option<String>, String)> {
        let files: Vec<FileChange> = changes
            .iter()
            .map(|(path, bytes)| match bytes {
                Some(bytes) => FileChange::Upsert { path, bytes },
                None => FileChange::Delete { path },
            })
            .collect();
        for _ in 0..CAS_TRIES {
            let before = self.head(repo, branch)?;
            let pack = cs::commit_pack(branch, before.as_deref(), message, AUTHOR, &files);
            let a = self.repo_call("POST", repo, "commit-pack", &[], "git:write", Some(("application/x-ndjson", pack.into_bytes())))?;
            match a.status {
                200 | 201 => return Ok((before, cs::committed(&a.json()).with_context(|| format!("a commit pack's answer names no new head: {}", a.text()))?)),
                409 => continue,
                s => bail!("POST commit-pack on {repo}: {s} {}", a.text()),
            }
        }
        bail!("a commit on {repo}'s {branch} lost its compare-and-swap {CAS_TRIES} times")
    }

    /// Live at main's tip, as a deploy moves it: created there the first
    /// time, else merged (`ff_prefer`, compare-and-swapped on live's head).
    /// Answers live's new head.
    pub fn go_live(&self, repo: &str, message: &str) -> Result<String> {
        for _ in 0..CAS_TRIES {
            let tip = self.head(repo, "main")?.with_context(|| format!("{repo} has no main to deploy"))?;
            let live = self.head(repo, "live")?;
            let a = match &live {
                Some(l) if *l == tip => return Ok(tip),
                None => {
                    let body = json!({ "base_ref": tip, "target_branch": "live", "target_is_ephemeral": false });
                    self.repo_call("POST", repo, "branches/create", &[], "git:write", Some(("application/json", body.to_string().into_bytes())))?
                }
                Some(l) => {
                    let body = json!({
                        "target_branch": "live", "source_ref": "main", "strategy": "ff_prefer", "expected_target_sha": l,
                        "commit_message": message, "author": { "name": AUTHOR.0, "email": AUTHOR.1 },
                    });
                    self.repo_call("POST", repo, "merge", &[], "git:write", Some(("application/json", body.to_string().into_bytes())))?
                }
            };
            if a.status == 409 {
                continue;
            }
            if !matches!(a.status, 200 | 201) {
                bail!("moving {repo}'s live: {a}");
            }
            // the head it moved to, as the store now holds it
            return self.head(repo, "live")?.with_context(|| format!("{repo}'s live is gone after its move: {a}"));
        }
        bail!("live of {repo} moved under the harness {CAS_TRIES} times")
    }

    /// The url form of the repo named `name`, from the org's list, as the
    /// cell finds one after a create answered 409.
    pub fn repo_url(&self, name: &str) -> Result<Option<String>> {
        let limit = REPO_PAGE.to_string();
        let mut cursor: Option<String> = None;
        for _ in 0..REPO_PAGES_MAX {
            let mut query = vec![("limit", limit.as_str())];
            if let Some(c) = &cursor {
                query.push(("cursor", c.as_str()));
            }
            let a = self.call("GET", "/api/repos", &query, &self.org, "org:read", None)?;
            if a.status != 200 {
                bail!("GET /api/repos: {a}");
            }
            let page = a.json();
            if let Some(url) = cs::listed_repo_url(&page, name) {
                return Ok(Some(url));
            }
            cursor = cs::next_repos_cursor(&page);
            if cursor.is_none() {
                return Ok(None);
            }
        }
        bail!("the org lists more than {} repos", REPO_PAGES_MAX * REPO_PAGE)
    }

    /// A new repo named `name` (no branches), as the cell creates one:
    /// answers its url form.
    pub fn create_repo(&self, name: &str) -> Result<String> {
        let body = json!({ "repo_name": name, "default_branch": "main" });
        let a = self.call("POST", "/api/repos", &[], name, "repo:write", Some(("application/json", body.to_string().into_bytes())))?;
        if !matches!(a.status, 200 | 201) {
            bail!("POST /api/repos {name}: {a}");
        }
        let id = cs::created_repo_id(&a.json()).with_context(|| format!("a create names no repo_id: {}", a.text()))?;
        let a = self.call("GET", &format!("/api/repo-urls/{}", checked_repo(&id)?), &[], &id, "org:read", None)?;
        if a.status != 200 {
            bail!("GET /api/repo-urls/{id}: {a}");
        }
        cs::repo_url(&a.json()).with_context(|| format!("a repo url answer names no url: {}", a.text()))
    }

    /// Whether `repo` is gone (a delete's answer, 5.13): every call on it
    /// is 404 from then.
    pub fn deleted(&self, repo: &str) -> Result<bool> {
        let a = self.repo_call("GET", repo, "branch", &[("name", "main")], "git:read", None)?;
        Ok(a.status == 404 && a.json()["code"] == "repository_not_found")
    }
}
