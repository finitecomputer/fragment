//! The code store itself, probed through the contract's routes
//! (macrofiche's docs/contract.md, sections 5 and 15) where fragment relies
//! on an answer no other section asserts, each read with the cell's own
//! parsers (`fragment_core::codestorage`): refused commit packs and the one
//! that changes nothing, the compare-and-swap, a file's identity headers, a
//! listing's pages and last commits, branch and merge conflicts, the merge
//! preview a deploy follows (`Promotion`) and the restore commits it makes,
//! history's order, a repo's delete as a wipe asks it, and the auth floor.
//! Each failure names the request, what the contract expects, and what came
//! back.
//!
//! It runs only on a store outside the run (`FRAGMENT_E2E_CODESTORE`): the
//! fake in this process is the contract's reference.

use anyhow::Result;
use base64::Engine;
use fragment_core::codestorage::{self as cs, Promotion, RepoDeleted};
use serde_json::{json, Value};
use sha1::Digest;

use crate::api::Api;
use crate::store::{Answer, External, AUTHOR};
use crate::{Need, Suite};

/// A sha no repo holds.
const UNKNOWN_SHA: &str = "0123456789abcdef0123456789abcdef01234567";
/// The page size a listing is walked with, so a three-file tree is three pages.
const LISTING_PAGE: &str = "1";
/// Pages a listing walk reads at most: the probe's tree is three files.
const LISTING_PAGES_MAX: usize = 10;

/// The probes' outcomes, made before any is reported (they borrow the
/// suite's store): (label, ok, detail).
#[derive(Default)]
struct Probes(Vec<(String, bool, String)>);

impl Probes {
    /// A check: `request` sent, `expected` by the contract, `actual` back.
    fn check(&mut self, label: &str, ok: bool, request: &str, expected: &str, actual: impl std::fmt::Display) {
        self.0.push((label.into(), ok, format!("{request}: expected {expected}; got {actual}")));
    }
}

/// git's blob id of `bytes`: what `x-blob-sha` names.
fn blob_sha(bytes: &[u8]) -> String {
    let mut h = sha1::Sha1::new();
    h.update(format!("blob {}\0", bytes.len()).as_bytes());
    h.update(bytes);
    hex::encode(h.finalize())
}

/// An NDJSON pack from its lines, as a client streams one.
fn pack(lines: &[Value]) -> Vec<u8> {
    lines.iter().map(|l| format!("{l}\n")).collect::<String>().into_bytes()
}

fn chunk(id: &str, data: &[u8], eof: bool) -> Value {
    json!({ "blob_chunk": { "content_id": id, "data": base64::engine::general_purpose::STANDARD.encode(data), "eof": eof } })
}

fn meta(branch: &str, expected: Option<&str>, files: Value) -> Value {
    let mut m = json!({ "target_branch": branch, "commit_message": "probe", "author": { "name": AUTHOR.0, "email": AUTHOR.1 }, "files": files });
    if let Some(sha) = expected {
        m["expected_target_sha"] = json!(sha);
    }
    json!({ "metadata": m })
}

fn upsert(path: &str, id: &str) -> Value {
    json!({ "path": path, "operation": "upsert", "content_id": id, "mode": "100644" })
}

fn post_pack(x: &External, repo: &str, lines: &[Value]) -> Result<Answer> {
    x.repo_call("POST", repo, "commit-pack", &[], "git:write", Some(("application/x-ndjson", pack(lines))))
}

/// One file written whole on `branch`, expecting `expected`: its answer.
fn write(x: &External, repo: &str, branch: &str, expected: &str, path: &str, bytes: &[u8]) -> Result<Answer> {
    post_pack(x, repo, &[meta(branch, Some(expected), json!([upsert(path, "c0")])), chunk("c0", bytes, true)])
}

fn post_json(x: &External, repo: &str, op: &str, body: &Value) -> Result<Answer> {
    x.repo_call("POST", repo, op, &[], "git:write", Some(("application/json", body.to_string().into_bytes())))
}

/// A restore commit on live, as the cell's deploy and the CLI's rollback
/// send one (`cs::restore_commit`).
fn restore(x: &External, repo: &str, base: &str, expected: &str) -> Result<Answer> {
    let line = cs::restore_commit("live", base, expected, "probe restore", AUTHOR);
    x.repo_call("POST", repo, "restore-commit", &[], "git:write", Some(("application/x-ndjson", line.into_bytes())))
}

/// The merge of main into live, as the cell's deploy sends it.
fn merge(x: &External, repo: &str, expected: &str) -> Result<Answer> {
    let body = json!({ "target_branch": "live", "source_ref": "main", "strategy": "ff_prefer", "expected_target_sha": expected, "commit_message": "probe", "author": { "name": AUTHOR.0, "email": AUTHOR.1 } });
    post_json(x, repo, "merge", &body)
}

/// The merge preview a deploy reads (`cs::DEPLOY_PREVIEW_QUERY`): its
/// answer, and the steps the cell takes from it.
fn preview(x: &External, repo: &str) -> Result<(Answer, Option<Promotion>)> {
    let a = x.repo_call("GET", repo, "merge/preview", &[("source_branch", "main"), ("target_branch", "live")], "git:read", None)?;
    let p = (a.status == 200).then(|| cs::promotion(&a.json())).flatten();
    Ok((a, p))
}

fn text(bytes: Option<Vec<u8>>) -> String {
    format!("{:?}", bytes.map(|b| String::from_utf8_lossy(&b).into_owned()))
}

/// A listing at `at`, walked a file a page: (path, size, last commit) each.
fn listing(x: &External, repo: &str, at: &str) -> Result<Vec<(String, u64, String)>> {
    let mut all = vec![];
    let mut cursor: Option<String> = None;
    for _ in 0..LISTING_PAGES_MAX {
        let mut query = vec![("ref", at), ("limit", LISTING_PAGE)];
        if let Some(c) = &cursor {
            query.push(("cursor", c.as_str()));
        }
        let a = x.repo_call("GET", repo, "files/metadata", &query, "git:read", None)?;
        anyhow::ensure!(a.status == 200, "GET files/metadata: {a}");
        let page = cs::tree_page(&a.json()).map_err(|e| anyhow::anyhow!("{e}"))?;
        all.extend(page.entries.into_iter().map(|e| (e.path.as_str().to_string(), e.size, e.last_commit_sha)));
        cursor = page.next;
        if cursor.is_none() {
            return Ok(all);
        }
    }
    anyhow::bail!("a three-file listing ran past {LISTING_PAGES_MAX} pages")
}

pub fn codestore(s: &mut Suite, _api: &Api) -> Result<()> {
    // the org key signs its tokens: a deployment's secret
    if !s.section("codestore", &[Need::Deployment]) {
        return Ok(());
    }
    let name = s.name("probe");
    let probes = match s.external_store() {
        None => None,
        Some(x) => {
            let mut p = Probes::default();
            probe(&mut p, x, &name)?;
            Some(p.0)
        }
    };
    let Some(probes) = probes else {
        s.skip("the store, probed against the contract", "it probes a store outside the run (FRAGMENT_E2E_CODESTORE=macrofiche or external): the code.storage fake in this process is the contract's reference");
        return Ok(());
    };
    for (label, ok, detail) in probes {
        s.ok(&label, ok, detail);
    }
    Ok(())
}

fn probe(p: &mut Probes, x: &External, name: &str) -> Result<()> {
    repos(p, x, name)?;
    let repo = x.create_repo(&format!("{name}-git"))?;
    let (c1, c2) = packs(p, x, &repo)?;
    deploys(p, x, &repo, &c1, &c2)?;
    delete(p, x, &format!("{name}-gone"))
}

/// 5.1 to 5.3 as the cell's create asks them, and the auth floor.
fn repos(p: &mut Probes, x: &External, name: &str) -> Result<()> {
    let repo = x.create_repo(name)?;
    let again = x.post_repo(name)?;
    p.check("a repo's name made again is 409", again.status == 409, &format!("POST /api/repos {name}, the name taken"), "409", &again);
    let problem = again.header("content-type").starts_with("application/problem+json") && again.json()["code"].is_string() && again.json()["title"].is_string();
    p.check("a refusal is a problem body with a code and a title", problem, "the 409 of a name made again", "application/problem+json, with code and title", format!("{} {}", again.header("content-type"), again.text()));
    let listed = x.repo_url(name)?;
    p.check("the org's list names it, as the cell finds a name after its 409", listed.as_deref() == Some(repo.as_str()), "GET /api/repos, page by page", &repo, format!("{listed:?}"));
    let bare = x.unsigned("GET", &format!("/api/repos/{repo}/branch?name=main"))?;
    p.check("a call without a bearer is 401", bare.status == 401, "GET branch with no authorization", "401", &bare);
    let other = x.create_repo(&format!("{name}-other"))?;
    let theirs = x.call("GET", &format!("/api/repos/{repo}/branch"), &[("name", "main")], &other, "git:read", None)?;
    p.check("a token for another repo on this repo's path is 404", theirs.status == 404, "GET branch of this repo, the token's repo claim the other's", "404", &theirs);
    Ok(())
}

/// 5.4 to 5.8: commit packs and what they wrote; answers the first two
/// commits on main.
fn packs(p: &mut Probes, x: &External, repo: &str) -> Result<(String, String)> {
    let fresh = x.repo_call("GET", repo, "branch", &[("name", "main")], "git:read", None)?;
    p.check("a fresh repo has no main: 404, branch not found", fresh.status == 404 && fresh.json()["detail"] == "branch not found", "GET branch main of a new repo", "404, detail \"branch not found\"", &fresh);
    // a pack whose chunks interleave, with an empty file: one commit
    let lines = [
        meta("main", None, json!([upsert("a.txt", "c0"), upsert("dir/b.txt", "c1"), upsert("dir/sub/c.txt", "c2")])),
        chunk("c0", b"al", false),
        chunk("c1", b"beta", true),
        chunk("c0", b"pha", true),
        chunk("c2", b"", true),
    ];
    let a = post_pack(x, repo, &lines)?;
    let c1 = cs::committed(&a.json()).unwrap_or_default();
    p.check(
        "a commit pack whose chunks interleave lands as one commit",
        matches!(a.status, 200 | 201) && cs::is_sha(&c1),
        "POST commit-pack: a.txt's chunks around dir/b.txt's, and an empty dir/sub/c.txt",
        "201 with result.success and a 40-hex new_sha",
        &a,
    );
    let read = x.file(repo, &c1, "a.txt")?;
    p.check("its chunks join in order", read.as_deref() == Some(&b"alpha"[..]), "GET file a.txt at the commit", "alpha", text(read));
    let beta = blob_sha(b"beta");
    let head = x.repo_call("HEAD", repo, "file", &[("path", "dir/b.txt"), ("ref", &c1)], "git:read", None)?;
    p.check(
        "HEAD file names the blob, its size and its last commit, with no body",
        head.status == 200 && head.header("content-length") == "4" && head.header("x-blob-sha") == beta && head.header("x-last-commit-sha") == c1 && head.body.is_empty(),
        "HEAD file dir/b.txt at the commit",
        &format!("200, content-length 4, x-blob-sha {beta}, x-last-commit-sha {c1}"),
        format!("{} content-length {:?} x-blob-sha {:?} x-last-commit-sha {:?}", head.status, head.header("content-length"), head.header("x-blob-sha"), head.header("x-last-commit-sha")),
    );
    let get = x.repo_call("GET", repo, "file", &[("path", "dir/b.txt"), ("ref", &c1)], "git:read", None)?;
    p.check(
        "GET file carries the same identity, and an etag of the blob",
        get.status == 200 && get.body == b"beta" && get.header("etag") == format!("\"{beta}\"") && get.header("x-blob-sha") == beta && get.header("content-length") == "4",
        "GET file dir/b.txt at the commit",
        &format!("200 beta, etag \"{beta}\""),
        format!("{} etag {:?} x-blob-sha {:?}", get.status, get.header("etag"), get.header("x-blob-sha")),
    );

    // a second commit: last commits move only for what it changed
    let a = write(x, repo, "main", &c1, "a.txt", b"alpha 2")?;
    let c2 = cs::committed(&a.json()).unwrap_or_default();
    let files = listing(x, repo, &c2)?;
    let want = vec![("a.txt".to_string(), 7, c2.clone()), ("dir/b.txt".to_string(), 4, c1.clone()), ("dir/sub/c.txt".to_string(), 0, c1.clone())];
    let mut got = files.clone();
    got.sort();
    p.check(
        "a listing walked a file a page names each file once, recursively, with git's last commits",
        cs::is_sha(&c2) && got == want,
        "GET files/metadata at the second commit, limit 1, following next_cursor",
        &format!("{want:?}"),
        format!("{files:?}"),
    );

    // refusals: nothing lands
    let main_at = |want: &str| x.head(repo, "main").map(|h| h.as_deref() == Some(want));
    let a = write(x, repo, "main", &c1, "stale.txt", b"x")?;
    p.check(
        "a pack on a stale head is 409 precondition_failed, read as moved (not as no change), and nothing lands",
        a.status == 409 && a.json()["result"]["status"] == "precondition_failed" && !cs::nothing_to_commit(&a.body) && main_at(&c2)?,
        "POST commit-pack expecting the first commit while main is at the second",
        "409, result.status precondition_failed, main unmoved",
        &a,
    );
    let a = write(x, repo, "main", &c2, "a.txt", b"alpha 2")?;
    p.check(
        "a pack that changes nothing is 412 \"no changes to commit\", which the cell reads as main's tip (no empty commit)",
        a.status == 412 && cs::nothing_to_commit(&a.body) && main_at(&c2)?,
        "POST commit-pack of a.txt as main holds it",
        "412, result.status precondition_failed, message \"no changes to commit\", main unmoved",
        &a,
    );
    let delete = json!({ "path": "a.txt", "operation": "delete", "content_id": "c0" });
    let a = post_pack(x, repo, &[meta("main", Some(&c2), json!([delete])), chunk("c0", b"", true)])?;
    p.check(
        "a chunk for a delete is refused (400 invalid), and nothing lands",
        a.status == 400 && a.json()["result"]["status"] == "invalid" && main_at(&c2)?,
        "POST commit-pack deleting a.txt with a blob_chunk for its content_id",
        "400, result.status invalid",
        &a,
    );
    let a = post_pack(x, repo, &[meta("main", Some(&c2), json!([upsert("z.txt", "c0")])), chunk("c0", b"z", false)])?;
    p.check("an upsert whose stream has no eof is 400, and nothing lands", a.status == 400 && main_at(&c2)?, "POST commit-pack of z.txt, its only chunk without eof", "400", &a);
    Ok((c1, c2))
}

/// 5.9 to 5.12 and 15: a deploy's steps as the cell takes them from the
/// merge preview (`cell/src/cs.rs`, `promote_live`), a rollback's restore,
/// history, and a conflict.
fn deploys(p: &mut Probes, x: &External, repo: &str, c1: &str, c2: &str) -> Result<()> {
    let live_at = |want: &str| x.head(repo, "live").map(|h| h.as_deref() == Some(want));
    let create = |target: &str, base: &str| post_json(x, repo, "branches/create", &json!({ "base_ref": base, "target_branch": target, "target_is_ephemeral": false }));
    let a = create("live", c1)?;
    let again = create("live", c1)?;
    p.check(
        "a branch made twice is 409 the second time",
        matches!(a.status, 200 | 201) && a.json()["commit_sha"] == c1 && again.status == 409,
        "POST branches/create live at the first commit, twice",
        "200 or 201 naming the base, then 409",
        format!("{a} | {again}"),
    );
    let a = create("other", UNKNOWN_SHA)?;
    p.check("a branch on an unknown base is 404", a.status == 404, &format!("POST branches/create other at {UNKNOWN_SHA}"), "404", &a);

    // live behind main: a fast-forward
    let (a, steps) = preview(x, repo)?;
    p.check("the preview of live behind main is a fast-forward", steps == Some(Promotion::FastForward { live: c1.into() }), "GET merge/preview main into live", "result fast_forward, live's and main's tips", &a);
    let a = merge(x, repo, c2)?;
    p.check("a merge on a stale live is 409, read as moved (no conflict)", a.status == 409 && !cs::merge_conflicted(&a.body), "POST merge main into live expecting the second commit while live is at the first", "409, no merge_conflict", &a);
    let a = merge(x, repo, c1)?;
    p.check(
        "a merge that can fast-forward does: live at main's tip",
        a.status == 200 && a.json()["target"]["new_sha"] == c2 && live_at(c2)?,
        "POST merge main into live (ff_prefer), live behind main",
        &format!("200, target.new_sha {c2}"),
        &a,
    );
    let (a, steps) = preview(x, repo)?;
    p.check("the preview of live at main's tip says so", steps == Some(Promotion::Current { live: c2.into() }), "GET merge/preview, live at main", "source_tip_sha = target_tip_sha", &a);

    // a rollback, then main moves: they diverge
    let c3 = cs::committed(&write(x, repo, "main", c2, "d.txt", b"d")?.json()).unwrap_or_default();
    let a = restore(x, repo, &c3, c2)?;
    p.check("a restore to a commit live never held is 412", a.status == 412, "POST restore-commit of live to main's newer tip", "412", &a);
    let a = restore(x, repo, c1, c2)?;
    let rolled = cs::committed(&a.json()).unwrap_or_default();
    let back = x.file(repo, "live", "a.txt")?;
    let history = x.history(repo, "live")?;
    p.check(
        "a rollback is a new commit on live with the old tree, its first parent live's tip",
        matches!(a.status, 200 | 201) && cs::is_sha(&rolled) && back.as_deref() == Some(&b"alpha"[..]) && history.get(..2) == Some(&[rolled.clone(), c2.to_string()][..]),
        "POST restore-commit of live to the first commit",
        "201, live's a.txt back to alpha, history [restore, second]",
        format!("{a}; a.txt {}; history {history:?}", text(back)),
    );
    let a = restore(x, repo, &rolled, &rolled)?;
    p.check("a restore to live's own tip is 412", a.status == 412, "POST restore-commit of live to its tip", "412", &a);
    let (a, steps) = preview(x, repo)?;
    p.check(
        "the preview of a rolled-back live and a moved main is a merge commit from their base, the second commit",
        steps == Some(Promotion::RestoreThenMerge { live: rolled.clone(), base: c2.into() }),
        "GET merge/preview, live rolled back and main moved",
        &format!("result merge_commit, merge_base_sha {c2}"),
        &a,
    );
    // the deploy's steps: live back to the base, then the merge
    let a = restore(x, repo, c2, &rolled)?;
    let based = cs::committed(&a.json()).unwrap_or_default();
    p.check("the deploy's first step: a restore of live to the base", matches!(a.status, 200 | 201) && cs::is_sha(&based), "POST restore-commit of live to the merge base", "201", &a);
    let a = restore(x, repo, c2, &based)?;
    p.check("a restore that would change nothing is 412 (the cell reads it as live at the base already)", a.status == 412, "POST restore-commit of live to the base again", "412", &a);
    let a = merge(x, repo, &based)?;
    let merged = a.json()["target"]["new_sha"].as_str().unwrap_or_default().to_string();
    let (a_txt, d_txt) = (x.file(repo, "live", "a.txt")?, x.file(repo, "live", "d.txt")?);
    p.check(
        "then the merge: live serves main's files, the rollback's reverts gone",
        a.status == 200 && cs::is_sha(&merged) && live_at(&merged)? && a_txt.as_deref() == Some(&b"alpha 2"[..]) && d_txt.as_deref() == Some(&b"d"[..]),
        "POST merge main into live, live at the base",
        "200, live's a.txt alpha 2 and d.txt d",
        format!("{a}; a.txt {}; d.txt {}", text(a_txt), text(d_txt)),
    );
    let (a, steps) = preview(x, repo)?;
    p.check(
        "the preview of a live that holds main's tip is a restore of main's tip (no_op)",
        steps == Some(Promotion::Restore { live: merged.clone(), main: c3.clone() }),
        "GET merge/preview, main an ancestor of live",
        "result no_op, the tips differing",
        &a,
    );
    let a = restore(x, repo, &c3, &merged)?;
    p.check("and that restore, live's files being main's already, is 412", a.status == 412 && live_at(&merged)?, "POST restore-commit of live to main's tip, live's tree the same", "412, live unmoved", &a);
    let main_history = x.history(repo, "main")?;
    p.check("history is newest first, the ref's tip first", main_history == [c3.clone(), c2.to_string(), c1.to_string()], "GET commits?ref=main", &format!("[{c3}, {c2}, {c1}]"), format!("{main_history:?}"));

    // both sides change a file: a conflict, which a retry cannot pass
    let ours = cs::committed(&write(x, repo, "live", &merged, "a.txt", b"live's side")?.json()).unwrap_or_default();
    write(x, repo, "main", &c3, "a.txt", b"main's side")?;
    let (a, _) = preview(x, repo)?;
    p.check(
        "the preview of a conflict names it",
        a.json()["status"] == "conflicted" && a.json()["conflict_paths"] == json!(["a.txt"]),
        "GET merge/preview, a.txt changed on both sides",
        "status conflicted, conflict_paths [a.txt]",
        &a,
    );
    let a = merge(x, repo, &ours)?;
    p.check("a merge that conflicts is 409 merge_conflict, and live stays", a.status == 409 && cs::merge_conflicted(&a.body) && live_at(&ours)?, "POST merge main into live, a.txt changed on both sides", "409, conflict_type merge_conflict", &a);
    Ok(())
}

/// 5.13: a repo's delete, as the cell asks it for a wiped person's
/// fragments, read as it reads one (`cs::repo_deleted`).
fn delete(p: &mut Probes, x: &External, name: &str) -> Result<()> {
    let repo = x.create_repo(name)?;
    let tip = x.commit(&repo, "main", &[("kept.txt", Some(b"kept"))], "probe")?;
    let path = format!("/api/repos/{repo}");
    let a = x.call("DELETE", &path, &[], &repo, "git:write", None)?;
    p.check(
        "a delete with git's scopes is refused, the same call never passing, and deletes nothing",
        cs::repo_deleted(a.status, &a.body) == RepoDeleted::Refused && x.head(&repo, "main")?.as_deref() == Some(tip.as_str()),
        "DELETE the repo with a git:write token",
        "a 4xx the cell reads as refused (403), main still there",
        &a,
    );
    let a = x.delete_repo(&repo)?;
    p.check("a repo is deleted", cs::repo_deleted(a.status, &a.body) == RepoDeleted::Now, "DELETE /api/repos/{repo} with repo:write", "2xx", &a);
    let (branch, file) = (x.repo_call("GET", &repo, "branch", &[("name", "main")], "git:read", None)?, x.repo_call("GET", &repo, "file", &[("path", "kept.txt"), ("ref", &tip)], "git:read", None)?);
    p.check("then it serves nothing: its branch and its files are 404", branch.status == 404 && file.status == 404, "GET branch main and file kept.txt of the deleted repo", "404, 404", format!("{branch} | {file}"));
    let a = x.delete_repo(&repo)?;
    p.check("a delete again is done, as the cell reads it", cs::repo_deleted(a.status, &a.body) == RepoDeleted::Already, "DELETE the deleted repo again", "409 repository_deleted (or 404, 410)", &a);
    let listed = x.repo_url(name)?;
    let again = x.post_repo(name)?;
    p.check(
        "it leaves the org's list, and its name is never made again",
        listed.is_none() && again.status == 409,
        "GET /api/repos, then POST /api/repos of its name",
        "not listed, then 409",
        format!("{listed:?} | {again}"),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    /// Goal: the probe's blob ids are git's. Method: ids git itself
    /// prints (`printf beta | git hash-object --stdin`, and the empty blob).
    #[test]
    fn blob_ids_are_gits() {
        assert_eq!(super::blob_sha(b""), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
        assert_eq!(super::blob_sha(b"beta"), "e1d65540f4fdf72431ec47e001282cc7e8ed7c0c");
    }
}
