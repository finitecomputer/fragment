//! Feedback (docs/api.md, Feedback): a person reports a problem or sends a
//! message to the deployment's operators, with up to three screenshots;
//! the operators read it, each screenshot as it came, are mailed it, mark
//! it done and open again, and delete it. Valid: a problem with its
//! screenshots, a message alone. Invalid: each way a form is out of shape,
//! nothing kept; past a person's day, 429; anyone but its person sends
//! none, and anyone but an operator reads none. Replay: a send asked again
//! with its id is the same one, mailed once; a mark asked again changes
//! and logs nothing, a delete asked again is 404, and a delete's bytes go
//! (BLOBS' keys read through a lever). Failures (a local node's registry
//! hooks): a send whose answer is lost, once or twice, or that stops
//! before it is kept, keeps its screenshots and is finished asked again;
//! one never finished, and a deleted one whose deletes fail, keep their
//! record through a failed run and a registry's restart until their bytes
//! are gone. And in Chrome, Settings' Help sends one with a screenshot
//! picked, and an answer to a dialog closed and opened again changes
//! nothing of the new one. What the run sent is deleted as it ends, so a
//! preview's operators read none of it.

use std::time::Duration;

use anyhow::Result;
use fragment_nip98::Keys;
use serde_json::{json, Value};

use crate::api::{Api, Call, Reply};
use crate::{Need, Suite};

const BOUNDARY: &str = "e2e-feedback-5c1a";
const FORM: &str = "multipart/form-data; boundary=e2e-feedback-5c1a";
const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, b'I', b'H', b'D', b'R'];
const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F', 0];

/// One field of a form: its name, and its bytes; a file's with its type.
enum Field<'a> {
    Text(&'a str, &'a str),
    File(&'a str, &'a str, &'a [u8]),
}

fn form(fields: &[Field<'_>]) -> Vec<u8> {
    let mut b = Vec::new();
    for f in fields {
        match f {
            Field::Text(name, value) => b.extend(format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes()),
            Field::File(name, mime, bytes) => {
                b.extend(format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"shot\"\r\nContent-Type: {mime}\r\n\r\n").as_bytes());
                b.extend(*bytes);
                b.extend(b"\r\n");
            }
        }
    }
    b.extend(format!("--{BOUNDARY}--\r\n").as_bytes());
    b
}

fn send(api: &Api, keys: Option<&Keys>, body: Vec<u8>, content_type: &str) -> Result<Reply> {
    api.call(Call { method: "POST", url: format!("{}/api/feedback", api.base), body: Some(body), content_type: Some(content_type), keys, ..Call::default() })
}

/// Every feedback the operators list, newest first (every page).
fn listed(api: &Api, operator: &Keys, open: bool) -> Result<Vec<Value>> {
    let mut all = vec![];
    let mut before: Option<u64> = None;
    // bounded: a run's own are few, and a page is 50
    for _ in 0..40 {
        let mut q = vec![];
        if open {
            q.push("open=1".to_string());
        }
        if let Some(b) = before {
            q.push(format!("before={b}"));
        }
        let r = api.signed(operator, "GET", &format!("/api/admin/feedback?{}", q.join("&")), None)?;
        anyhow::ensure!(r.status == 200, "the operators' feedback: {r}");
        all.extend(r.body["feedback"].as_array().cloned().unwrap_or_default());
        before = r.body["next"].as_u64();
        if before.is_none() {
            break;
        }
    }
    Ok(all)
}

fn of(list: &[Value], id: &str) -> Value {
    list.iter().find(|f| f["id"] == id).cloned().unwrap_or(Value::Null)
}

/// A send's own id, as the shell's dialog makes one.
fn fresh_id() -> String {
    Keys::generate().pubkey_hex()[..32].to_string()
}

/// The keys BLOBS holds under a feedback's prefix (the lever's): its
/// screenshots' bytes, there or gone.
fn blob_keys(api: &Api, person: &str, id: &str) -> Result<Vec<String>> {
    let r = api.unsigned("POST", "/api/test/feedback", Some(&json!({ "person": person, "id": id })))?;
    anyhow::ensure!(r.status == 200, "the feedback lever: {r}");
    Ok(r.body["keys"].as_array().into_iter().flatten().filter_map(|k| k.as_str().map(str::to_string)).collect())
}

/// One of the registry's test hooks (a local node's).
fn hook(api: &Api, body: Value) -> Result<Reply> {
    let r = api.unsigned("POST", "/api/test/registry", Some(&body))?;
    anyhow::ensure!(r.status == 200, "the registry hook {body}: {r}");
    Ok(r)
}

pub fn feedback(s: &mut Suite, api: &Api) -> Result<()> {
    if !s.section("feedback", &[Need::Deployment, Need::Operator]) {
        return Ok(());
    }
    let operator = s.operator.clone();
    let admin = |method: &str, path: &str, body: Option<&Value>| api.signed(&operator, method, &format!("/api/admin/feedback{path}"), body);
    let ivy = api.person()?;
    let ivy_id = api.identity(&ivy)?;
    let ivy_email = Api::email_of(&ivy);

    // ---- valid: a problem, with its page and two screenshots
    let mail_before = s.mail.sent_to(crate::FEEDBACK_TO).len();
    let r = send(
        api,
        Some(&ivy),
        form(&[Field::Text("kind", "problem"), Field::Text("message", "  It broke.\r\nTwice.\r\n"), Field::Text("page", "/settings"), Field::File("screenshot", "image/png", PNG), Field::File("screenshot", "image/jpeg", JPEG)]),
        FORM,
    )?;
    let id = r.body["id"].as_str().unwrap_or("").to_string();
    s.ok("a person reports a problem with two screenshots: its id, and their count", r.status == 200 && id.len() == 32 && r.body["screenshots"] == 2, &r);
    let open = listed(api, &operator, true)?;
    let f = of(&open, &id);
    s.ok(
        "the operators read it, open: who sent it (their npub and email), its kind, its message as kept (newlines, trimmed), its page, the release that took it, and each screenshot's type by its bytes",
        f["person"] == ivy_id.as_str()
            && f["email"] == ivy_email.as_str()
            && f["kind"] == "problem"
            && f["message"] == "It broke.\nTwice."
            && f["page"] == "/settings"
            && f["release"].as_str().is_some_and(|r| !r.is_empty())
            && f["screenshots"] == json!([{ "mime": "image/png", "size": PNG.len() }, { "mime": "image/jpeg", "size": JPEG.len() }])
            && f["resolvedAt"].is_null()
            && f["createdAt"].as_i64().is_some_and(|t| t > 0),
        &f,
    );
    let shot = admin("GET", &format!("/{id}/screenshots/0"), None)?;
    let second = admin("GET", &format!("/{id}/screenshots/1"), None)?;
    s.ok(
        "each screenshot is served to the operators as it came, typed by its bytes, never sniffed, sandboxed",
        shot.status == 200
            && shot.bytes == PNG
            && shot.header("content-type") == "image/png"
            && shot.header("x-content-type-options") == "nosniff"
            && shot.header("content-security-policy").contains("sandbox")
            && second.status == 200
            && second.bytes == JPEG
            && second.header("content-type") == "image/jpeg",
        format!("{shot} / {second}"),
    );
    let missing = [admin("GET", &format!("/{id}/screenshots/2"), None)?, admin("GET", &format!("/{id}/screenshots/x"), None)?, admin("GET", &format!("/{}/screenshots/0", "0".repeat(32)), None)?];
    s.ok("a screenshot it does not hold, or of no feedback, is 404", missing.iter().all(|r| r.status == 404), json!(missing.iter().map(|r| r.status).collect::<Vec<_>>()));
    if !s.hosted() {
        let mail = s.mail.sent_to(crate::FEEDBACK_TO);
        let notice = mail.get(mail_before).map(|m| (m.subject.clone(), m.text.clone())).unwrap_or_default();
        s.ok(
            "the operators are mailed it at the deployment's feedback_to: who sent what, its message, its page, its screenshots counted, and the admin's page",
            mail.len() == mail_before + 1
                && notice.0 == format!("Finite.Computer: {ivy_email} reported a problem")
                && notice.1.contains("It broke.\nTwice.")
                && notice.1.contains("Page: /settings\n2 screenshots")
                && notice.1.contains(&format!("{}/admin", api.base)),
            format!("{notice:?}"),
        );
    }
    let r = send(api, Some(&ivy), form(&[Field::Text("message", "Thanks for this."), Field::Text("kind", "message")]), FORM)?;
    let message = of(&listed(api, &operator, true)?, r.body["id"].as_str().unwrap_or(""));
    s.ok("a message alone: no page, no screenshots", r.status == 200 && message["kind"] == "message" && message["page"].is_null() && message["screenshots"] == json!([]), &message);

    // ---- replay: a send asked again with its id (the shell's dialog's) is the same one
    let mailed = s.mail.sent_to(crate::FEEDBACK_TO).len();
    let mine = fresh_id();
    let with_id = || form(&[Field::Text("id", &mine), Field::Text("kind", "message"), Field::Text("message", "Sent twice, kept once."), Field::File("screenshot", "image/png", PNG)]);
    let (first, again) = (send(api, Some(&ivy), with_id(), FORM)?, send(api, Some(&ivy), with_id(), FORM)?);
    let kept = listed(api, &operator, false)?.into_iter().filter(|f| f["id"] == mine.as_str()).count();
    let mails = s.mail.sent_to(crate::FEEDBACK_TO).len() - mailed;
    s.ok(
        "a send asked again with its id is the same one: answered as kept, listed once, its screenshot once, and (locally) mailed once",
        first.status == 200 && again.status == 200 && first.body["id"] == mine.as_str() && again.body == first.body && kept == 1 && blob_keys(api, &ivy_id, &mine)?.len() == 1 && (s.hosted() || mails == 1),
        json!({ "first": first.body, "again": again.body, "listed": kept, "mailed": mails }),
    );
    let stolen = send(api, Some(&api.person()?), with_id(), FORM)?;
    s.ok("another person's send by its id is refused (409), theirs untouched", stolen.status == 409 && of(&listed(api, &operator, false)?, &mine)["person"] == ivy_id.as_str(), &stolen);
    let bad_id = send(api, Some(&ivy), form(&[Field::Text("id", "../../x"), Field::Text("kind", "message"), Field::Text("message", "hi")]), FORM)?;
    s.ok("an id that is not 32 lowercase hex digits is 400", bad_id.status == 400, &bad_id);

    // ---- invalid: each refused, nothing kept
    let theirs = |list: &[Value]| list.iter().filter(|f| f["person"] == ivy_id.as_str()).count();
    let before = theirs(&listed(api, &operator, false)?);
    let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\" onload=\"alert(1)\"/>";
    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        ("no kind", form(&[Field::Text("message", "hi")]), FORM),
        ("a kind that is neither", form(&[Field::Text("kind", "bug"), Field::Text("message", "hi")]), FORM),
        ("no message", form(&[Field::Text("kind", "problem"), Field::Text("message", " \r\n ")]), FORM),
        ("a page that is not the shell's", form(&[Field::Text("kind", "problem"), Field::Text("message", "hi"), Field::Text("page", "https://evil.example/")]), FORM),
        ("a field it does not take", form(&[Field::Text("kind", "problem"), Field::Text("message", "hi"), Field::Text("email", "someone@else.example")]), FORM),
        ("a screenshot that is no image (an SVG)", form(&[Field::Text("kind", "problem"), Field::Text("message", "hi"), Field::File("screenshot", "image/png", svg)]), FORM),
        ("four screenshots", form(&[Field::Text("kind", "problem"), Field::Text("message", "hi"), Field::File("screenshot", "image/png", PNG), Field::File("screenshot", "image/png", PNG), Field::File("screenshot", "image/png", PNG), Field::File("screenshot", "image/png", PNG)]), FORM),
        ("a body that is no form", br#"{"kind":"problem","message":"hi"}"#.to_vec(), "application/json"),
    ];
    let mut said = vec![];
    for (what, body, ct) in cases {
        let r = send(api, Some(&ivy), body, ct)?;
        said.push(json!({ "case": what, "status": r.status, "message": r.message() }));
    }
    let mut big = vec![0u8; fragment_core::feedback::SCREENSHOT_MAX_BYTES + 1];
    big[..PNG.len()].copy_from_slice(PNG);
    let large = send(api, Some(&ivy), form(&[Field::Text("kind", "problem"), Field::Text("message", "hi"), Field::File("screenshot", "image/png", &big)]), FORM)?;
    let after = theirs(&listed(api, &operator, false)?);
    s.ok(
        "a form out of shape is 400, saying why, and a screenshot past 5 MiB 413: none is kept",
        said.iter().all(|r| r["status"] == 400 && r["message"].as_str().is_some_and(|m| !m.is_empty())) && large.status == 413 && after == before,
        json!({ "refused": said, "large": large.status, "kept": after - before }),
    );
    let unsigned = send(api, None, form(&[Field::Text("kind", "problem"), Field::Text("message", "hi")]), FORM)?;
    s.ok("an unsigned send is 401", unsigned.status == 401, &unsigned);

    // ---- a person's day: ten, then 429
    let rex = api.person()?;
    let rex_id = api.identity(&rex)?;
    let mut statuses = vec![];
    for i in 0..fragment_core::feedback::PER_DAY_MAX {
        statuses.push(send(api, Some(&rex), form(&[Field::Text("kind", "message"), Field::Text("message", &format!("note {i}"))]), FORM)?.status);
    }
    let over = send(api, Some(&rex), form(&[Field::Text("kind", "message"), Field::Text("message", "one more")]), FORM)?;
    let others = send(api, Some(&ivy), form(&[Field::Text("kind", "message"), Field::Text("message", "mine still goes")]), FORM)?;
    s.ok(
        "a person sends ten in a day; the eleventh is 429, saying when to send more, and another person's still go",
        statuses.iter().all(|s| *s == 200) && over.status == 429 && over.message().contains("tomorrow") && others.status == 200,
        json!({ "ten": statuses, "over": over.to_string(), "others": others.status }),
    );

    // ---- done, open again, deleted
    let done = admin("PATCH", &format!("/{id}"), Some(&json!({ "resolved": true })))?;
    let again = admin("PATCH", &format!("/{id}"), Some(&json!({ "resolved": true })))?;
    let (open, all) = (listed(api, &operator, true)?, listed(api, &operator, false)?);
    s.ok(
        "an operator marks it done: when and by whom; it leaves the open list and stays in the whole; asked again, it changes nothing",
        done.status == 200
            && done.body["resolvedAt"].as_i64().is_some_and(|t| t > 0)
            && done.body["resolvedBy"].as_str().is_some_and(|b| !b.is_empty())
            && again.body["resolvedAt"] == done.body["resolvedAt"]
            && of(&open, &id).is_null()
            && of(&all, &id)["resolvedAt"] == done.body["resolvedAt"],
        json!({ "done": done.body, "again": again.body }),
    );
    let reopened = admin("PATCH", &format!("/{id}"), Some(&json!({ "resolved": false })))?;
    let bad = [admin("PATCH", &format!("/{id}"), Some(&json!({ "resolved": "yes" })))?, admin("PATCH", &format!("/{id}"), Some(&json!({ "resolved": true, "by": "me" })))?];
    let none = admin("PATCH", &format!("/{}", "0".repeat(32)), Some(&json!({ "resolved": true })))?;
    s.ok(
        "opened again, it is open; a body out of shape is 400, and no such feedback 404",
        reopened.status == 200 && reopened.body["resolvedAt"].is_null() && reopened.body["resolvedBy"].is_null() && bad.iter().all(|r| r.status == 400) && none.status == 404,
        json!([reopened.body, bad.iter().map(|r| r.status).collect::<Vec<_>>(), none.status]),
    );
    let held = blob_keys(api, &ivy_id, &id)?.len();
    let deleted = admin("DELETE", &format!("/{id}"), None)?;
    let gone = [admin("GET", &format!("/{id}/screenshots/0"), None)?, admin("DELETE", &format!("/{id}"), None)?];
    let all = listed(api, &operator, false)?;
    let bytes_gone = s.eventually(Duration::from_secs(15), || blob_keys(api, &ivy_id, &id).is_ok_and(|k| k.is_empty()));
    s.ok(
        "an operator deletes it, answered as it was: it is no longer listed or served, a delete asked again is 404, and its screenshots' bytes go (BLOBS holds none of its keys)",
        held == 2 && deleted.status == 200 && deleted.body["id"] == id.as_str() && of(&all, &id).is_null() && gone.iter().all(|r| r.status == 404) && bytes_gone,
        json!({ "held": held, "deleted": deleted.status, "gone": gone.iter().map(|r| r.status).collect::<Vec<_>>(), "bytesGone": bytes_gone }),
    );
    let log = api.signed(&operator, "GET", "/api/admin/log", None)?;
    let acts: Vec<String> = log.body["entries"].as_array().into_iter().flatten().filter(|e| e["target"] == id.as_str()).filter_map(|e| e["action"].as_str().map(str::to_string)).collect();
    s.ok("the log holds each, newest first, once: the delete, the reopen, the mark", acts == ["feedback-delete", "feedback-reopen", "feedback-done"], format!("{acts:?}"));

    // ---- who reaches the operators' side
    let stranger = api.person()?;
    let refused = [
        api.signed(&stranger, "GET", "/api/admin/feedback", None)?,
        api.signed(&stranger, "GET", &format!("/api/admin/feedback/{}/screenshots/0", r.body["id"].as_str().unwrap_or("")), None)?,
        api.signed(&stranger, "PATCH", &format!("/api/admin/feedback/{}", r.body["id"].as_str().unwrap_or("")), Some(&json!({ "resolved": true })))?,
        api.signed(&stranger, "DELETE", &format!("/api/admin/feedback/{}", r.body["id"].as_str().unwrap_or("")), None)?,
    ];
    s.ok("anyone but an operator reads, marks or deletes none (403)", refused.iter().all(|r| r.status == 403), json!(refused.iter().map(|r| r.status).collect::<Vec<_>>()));

    let failing = failures(s, api, &operator)?;
    let ui = shell_ui(s, api, &operator)?;
    // what this run sent goes, so a shared preview's operators read none of it
    let ours = listed(api, &operator, false)?.into_iter().filter(|f| f["person"] == ivy_id.as_str() || f["person"] == rex_id.as_str() || failing.as_deref().is_some_and(|p| f["person"] == p) || ui.as_deref().is_some_and(|e| f["email"] == e));
    for f in ours {
        admin("DELETE", &format!("/{}", f["id"].as_str().unwrap_or("")), None)?;
    }
    Ok(())
}

/// A send that fails part way, and a delete whose deletes fail, through the
/// local registry's hooks (a preview's are no route): its answer lost
/// after it acted (`dropFor`), or it refused before acting (`downFor`); a
/// run of the alarm's deletes failing (`feedbackSweepFails`), and the
/// registry restarted (`skewFor` from a release deployed before). Answers
/// the person it sent as, when it ran.
fn failures(s: &mut Suite, api: &Api, operator: &Keys) -> Result<Option<String>> {
    const LABEL: &str = "a send or delete that fails part way keeps a record of its screenshots' bytes until they are gone";
    if s.hosted() {
        s.skip(LABEL, "the registry's hooks are a local node's");
        return Ok(None);
    }
    let wait = Duration::from_secs(15);
    let admin = |method: &str, path: &str| api.signed(operator, method, &format!("/api/admin/feedback{path}"), None);
    let uma = api.person()?;
    let uma_id = api.identity(&uma)?;
    let two = |id: &str, message: &str| form(&[Field::Text("id", id), Field::Text("kind", "problem"), Field::Text("message", message), Field::File("screenshot", "image/png", PNG), Field::File("screenshot", "image/jpeg", JPEG)]);
    let mailed = |s: &Suite| s.mail.sent_to(crate::FEEDBACK_TO).len();
    let shots_whole = |id: &str| -> Result<bool> {
        let (a, b) = (admin("GET", &format!("/{id}/screenshots/0"))?, admin("GET", &format!("/{id}/screenshots/1"))?);
        Ok(a.status == 200 && a.bytes == PNG && b.status == 200 && b.bytes == JPEG)
    };

    // its answer lost once, after the registry kept it: the router asks
    // again, and it is the same send
    let lost_once = fresh_id();
    let before = mailed(s);
    hook(api, json!({ "dropFor": { "path": "/feedback/keep", "times": 1 } }))?;
    let r = send(api, Some(&uma), two(&lost_once, "Lost once."), FORM)?;
    s.ok(
        "a send whose answer is lost once after it was kept is asked again by the router: kept once, its screenshots whole, mailed once",
        r.status == 200 && r.body["id"] == lost_once.as_str() && shots_whole(&lost_once)? && mailed(s) - before == 1,
        json!({ "sent": r.body, "mailed": mailed(s) - before }),
    );

    // lost twice: its sender hears 503, yet it was kept; asked again with
    // its id, it is answered as kept, nothing put or mailed again
    let lost_twice = fresh_id();
    let before = mailed(s);
    hook(api, json!({ "dropFor": { "path": "/feedback/keep", "times": 2 } }))?;
    let failed = send(api, Some(&uma), two(&lost_twice, "Lost twice."), FORM)?;
    let kept_anyway = of(&listed(api, operator, false)?, &lost_twice);
    let shots_kept = shots_whole(&lost_twice)?;
    let again = send(api, Some(&uma), two(&lost_twice, "Lost twice."), FORM)?;
    let listed_once = listed(api, operator, false)?.iter().filter(|f| f["id"] == lost_twice.as_str()).count();
    s.ok(
        "a send whose answer is lost twice is 503 to its sender, yet kept with its screenshots whole (none deleted on a failure that may have acted); asked again with its id, it is answered as kept: listed once, mailed once",
        failed.status == 503 && kept_anyway["id"] == lost_twice.as_str() && shots_kept && again.status == 200 && again.body["id"] == lost_twice.as_str() && listed_once == 1 && shots_whole(&lost_twice)? && mailed(s) - before == 1,
        json!({ "failed": failed.to_string(), "keptAnyway": !kept_anyway.is_null(), "screenshots": shots_kept, "again": again.body, "listed": listed_once, "mailed": mailed(s) - before }),
    );

    // stopped before it was kept: its bytes are put and recorded (a row
    // still sending, not the operators'), and asked again it is finished,
    // as asked now (one screenshot: the second's bytes go)
    let stopped = fresh_id();
    hook(api, json!({ "downFor": { "path": "/feedback/keep", "times": 2 } }))?;
    let failed = send(api, Some(&uma), two(&stopped, "Stopped."), FORM)?;
    let (unlisted, put) = (of(&listed(api, operator, false)?, &stopped).is_null(), blob_keys(api, &uma_id, &stopped)?.len());
    let finished = send(api, Some(&uma), form(&[Field::Text("id", &stopped), Field::Text("kind", "problem"), Field::Text("message", "Stopped, then finished."), Field::File("screenshot", "image/png", PNG)]), FORM)?;
    let f = of(&listed(api, operator, false)?, &stopped);
    let left = blob_keys(api, &uma_id, &stopped)?;
    s.ok(
        "a send stopped before it was kept is 503, not the operators', its bytes recorded; asked again with its id it is finished as asked now: one screenshot kept, the earlier try's second deleted",
        failed.status == 503 && unlisted && put == 2 && finished.status == 200 && f["message"] == "Stopped, then finished." && f["screenshots"].as_array().map(Vec::len) == Some(1) && left.len() == 1,
        json!({ "failed": failed.status, "unlisted": unlisted, "put": put, "finished": finished.body, "kept": f, "left": left }),
    );

    // never finished: its record outlives a registry's restart and a failed
    // run of the alarm's deletes, and the next run deletes its bytes, then it
    let abandoned = fresh_id();
    hook(api, json!({ "downFor": { "path": "/feedback/keep", "times": 2 } }))?;
    let failed = send(api, Some(&uma), two(&abandoned, "Abandoned."), FORM)?;
    hook(api, json!({ "skewFor": { "path": "/feedback/list", "times": fragment_core::release::ASKS * 25, "deployed": 0 } }))?;
    let restarted = api.signed(operator, "GET", "/api/admin/feedback", None)?;
    let fresh = hook(api, json!({ "calls": null }))?.body["skewed"] == 0;
    hook(api, json!({ "feedbackSweepFails": 1 }))?;
    let aged = hook(api, json!({ "feedbackAge": abandoned }))?.body["aged"] == 1;
    let ran = s.eventually(wait, || hook(api, json!({ "feedbackSweepFails": null })).is_ok_and(|r| r.body["left"] == 0));
    let kept_through = blob_keys(api, &uma_id, &abandoned)?.len();
    hook(api, json!({ "signins": "sweep" }))?;
    let swept = s.eventually(wait, || blob_keys(api, &uma_id, &abandoned).is_ok_and(|k| k.is_empty()));
    // its record gone too: its id, sent again, is a new send (while the
    // record stood it would be 409, deleted)
    let anew = s.eventually(wait, || send(api, Some(&uma), form(&[Field::Text("id", &abandoned), Field::Text("kind", "message"), Field::Text("message", "Anew.")]), FORM).is_ok_and(|r| r.status == 200));
    s.ok(
        "a send never finished outlives a registry's restart and a failed run of the alarm's deletes, its bytes all there; the next run deletes them, then its record",
        failed.status == 503 && restarted.status == 200 && fresh && aged && ran && kept_through == 2 && swept && anew,
        json!({ "failed": failed.status, "restarted": restarted.status, "fresh": fresh, "aged": aged, "failedRun": ran, "keptThrough": kept_through, "swept": swept, "sentAnew": anew }),
    );

    // an operator's delete whose deletes fail: gone to the operators at once,
    // its bytes recorded until the next run deletes them
    let deleting = fresh_id();
    let sent = send(api, Some(&uma), two(&deleting, "Delete me."), FORM)?;
    hook(api, json!({ "feedbackSweepFails": 1 }))?;
    let deleted = admin("DELETE", &format!("/{deleting}"))?;
    let ran = s.eventually(wait, || hook(api, json!({ "feedbackSweepFails": null })).is_ok_and(|r| r.body["left"] == 0));
    let (shot, again, kept_through) = (admin("GET", &format!("/{deleting}/screenshots/0"))?.status, admin("DELETE", &format!("/{deleting}"))?.status, blob_keys(api, &uma_id, &deleting)?.len());
    hook(api, json!({ "signins": "sweep" }))?;
    let swept = s.eventually(wait, || blob_keys(api, &uma_id, &deleting).is_ok_and(|k| k.is_empty()));
    s.ok(
        "a delete whose bytes fail to delete is gone to the operators at once (404 served, 404 deleted again), its bytes recorded through the failed run, and the next run deletes them",
        sent.status == 200 && deleted.status == 200 && ran && shot == 404 && again == 404 && kept_through == 2 && swept,
        json!({ "deleted": deleted.status, "failedRun": ran, "served": shot, "deletedAgain": again, "keptThrough": kept_through, "swept": swept }),
    );
    Ok(Some(uma_id))
}

/// Settings' Help in Chrome: Report a problem opens the dialog, a
/// screenshot picked shows as a thumbnail, Send says thanks, and the
/// operators read it from `/settings`, with its screenshot. Answers the
/// email it signed in as, when it ran.
fn shell_ui(s: &mut Suite, api: &Api, operator: &Keys) -> Result<Option<String>> {
    const SENT: &str = "Settings' Help sends a problem with a screenshot picked: the dialog thanks them, and the operators read it, from /settings";
    if !s.runs("feedback", &[Need::Chrome]) {
        s.skip(SENT, "this run has no Chrome");
        return Ok(None);
    }
    let Some(mut b) = s.browser()? else {
        s.ok("Chrome is installed for Settings' Help (set CHROME_BIN)", false, "no Chrome found");
        return Ok(None);
    };
    let wait = std::time::Duration::from_secs(20);
    let email = format!("feedback-ui-{}@e2e.test", &Keys::generate().pubkey_hex()[..10]);
    let session = api.sign_in(&email)?;
    b.set_cookie(&format!("{}/", api.base), "fragment_session", &session)?;
    let page = b.open(&format!("{}/settings", api.base))?;
    b.viewport(&page, 1280, 800, false)?;
    let helped = b.until(&page, "!!document.querySelector('#settings-help [data-action=feedback-problem]') && [...document.querySelectorAll('.settings-nav-link')].some((b) => b.textContent === 'Help')", wait);
    b.eval(&page, "(document.querySelector('#settings-help [data-action=feedback-problem]').click(), true)")?;
    let opened = b.until(&page, "document.getElementById('feedback-dialog').open && document.getElementById('feedback-label').textContent === 'What happened?'", wait);
    let dir = s.dir("feedback-ui");
    let shot = dir.join("shot.png");
    std::fs::write(&shot, PNG)?;
    b.choose_files(&page, "#feedback-file", &[&shot])?;
    let thumbed = b.until(&page, "document.querySelectorAll('#feedback-shots .feedback-shot img').length === 1", wait);
    b.eval(&page, "(document.getElementById('feedback-message').value = 'The page went blank.', document.getElementById('feedback-send').click(), true)")?;
    let thanked = b.until(&page, "!document.getElementById('feedback-sent').hidden && document.getElementById('feedback-form').hidden", wait);
    let theirs = listed(api, operator, true)?.into_iter().find(|f| f["email"] == email.as_str()).unwrap_or(Value::Null);
    s.ok(
        SENT,
        helped && opened && thumbed && thanked && theirs["kind"] == "problem" && theirs["message"] == "The page went blank." && theirs["page"] == "/settings" && theirs["screenshots"].as_array().map(Vec::len) == Some(1),
        json!({ "help": helped, "opened": opened, "thumbnail": thumbed, "thanked": thanked, "kept": theirs, "said": b.eval(&page, "document.getElementById('feedback-error').textContent")? }),
    );

    // an answer to a dialog closed and opened again since: the page's sends
    // wait on the test (its fetch stands in), then answer A's send after B
    // is written, once a success and once an error; B stays as written
    b.eval(&page, "(() => { window.__fbReal = window.fetch; window.__fbWaiting = []; window.fetch = (u, o) => String(u).includes('/api/feedback') ? new Promise((r) => window.__fbWaiting.push({ r, body: o.body })) : window.__fbReal(u, o); return true })()")?;
    let mut stale = vec![];
    for (answer, status) in [(r#"{"id":"00000000000000000000000000000000","screenshots":0}"#, 200), (r#"{"error":"registry_unavailable","message":"A's error"}"#, 503)] {
        b.eval(&page, "(document.getElementById('feedback-done').click(), document.querySelector('#settings-help [data-action=feedback-problem]').click(), document.getElementById('feedback-message').value = 'A', document.getElementById('feedback-send').click(), true)")?;
        let pending = b.until(&page, "window.__fbWaiting.length === 1", wait);
        let a_id = b.eval(&page, "window.__fbWaiting[0].body.get('id')")?;
        b.eval(&page, "(document.getElementById('feedback-cancel').click(), document.querySelector('#settings-help [data-action=feedback-message]').click(), document.getElementById('feedback-message').value = 'B, written after', true)")?;
        b.choose_files(&page, "#feedback-file", &[&shot])?;
        let b_written = b.until(&page, "document.querySelectorAll('#feedback-shots .feedback-shot img').length === 1", wait);
        let answer = serde_json::to_string(answer)?;
        b.eval(&page, &format!("(async () => {{ window.__fbWaiting.shift().r(new Response({answer}, {{ status: {status}, headers: {{ 'content-type': 'application/json' }} }})); await new Promise((r) => setTimeout(r, 500)); return true }})()"))?;
        let b_now = b.eval(&page, "({ open: document.getElementById('feedback-dialog').open, form: !document.getElementById('feedback-form').hidden, thanked: !document.getElementById('feedback-sent').hidden, kind: document.getElementById('feedback-form').elements.kind.value, message: document.getElementById('feedback-message').value, shots: document.querySelectorAll('#feedback-shots .feedback-shot img').length, error: document.getElementById('feedback-error').hidden ? null : document.getElementById('feedback-error').textContent, send: !document.getElementById('feedback-send').disabled })")?;
        let b_id = b.eval(&page, "(document.getElementById('feedback-send').click(), new Promise((r) => setTimeout(() => r(window.__fbWaiting[0]?.body.get('id')), 300)))")?;
        b.eval(&page, "(window.__fbWaiting.splice(0).forEach((w) => w.r(new Response('{}', { status: 503 }))), true)")?;
        let unchanged = b_now == json!({ "open": true, "form": true, "thanked": false, "kind": "message", "message": "B, written after", "shots": 1, "error": null, "send": true });
        stale.push(json!({ "answer": status, "pending": pending, "written": b_written, "unchanged": unchanged, "b": b_now, "aId": a_id, "bId": b_id, "ownId": a_id.as_str().is_some_and(|a| a.len() == 32) && b_id.as_str().is_some_and(|b| b.len() == 32) && a_id != b_id }));
    }
    b.eval(&page, "(window.fetch = window.__fbReal, document.getElementById('feedback-dialog').close(), true)")?;
    s.ok(
        "an answer to a send of a dialog since closed and opened again, a success or an error, changes nothing of the new one: its kind, text and screenshot stay, no thanks, no error; each opening sends with an id of its own",
        stale.iter().all(|c| c["pending"] == true && c["written"] == true && c["unchanged"] == true && c["ownId"] == true),
        json!(stale),
    );
    Ok(Some(email))
}
