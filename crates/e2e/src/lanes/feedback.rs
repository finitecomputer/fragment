//! Feedback (docs/api.md, Feedback): a person reports a problem or sends a
//! message to the deployment's operators, with up to three screenshots;
//! the operators read it, each screenshot as it came, are mailed it, mark
//! it done and open again, and delete it. Valid: a problem with its
//! screenshots, a message alone. Invalid: each way a form is out of shape,
//! nothing kept; past a person's day, 429; anyone but its person sends
//! none, and anyone but an operator reads none. Replay: a mark asked again
//! changes and logs nothing, a delete asked again is 404. And in Chrome,
//! Settings' Help sends one with a screenshot picked. What the run sent
//! is deleted as it ends, so a preview's operators read none of it.

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
    let deleted = admin("DELETE", &format!("/{id}"), None)?;
    let gone = [admin("GET", &format!("/{id}/screenshots/0"), None)?, admin("DELETE", &format!("/{id}"), None)?];
    let all = listed(api, &operator, false)?;
    s.ok(
        "an operator deletes it, answered as it was: it is no longer listed, its screenshots go, and a delete asked again is 404",
        deleted.status == 200 && deleted.body["id"] == id.as_str() && of(&all, &id).is_null() && gone.iter().all(|r| r.status == 404),
        json!({ "deleted": deleted.status, "gone": gone.iter().map(|r| r.status).collect::<Vec<_>>() }),
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

    let ui = shell_ui(s, api, &operator)?;
    // what this run sent goes, so a shared preview's operators read none of it
    let ours = listed(api, &operator, false)?.into_iter().filter(|f| f["person"] == ivy_id.as_str() || f["person"] == rex_id.as_str() || ui.as_deref().is_some_and(|e| f["email"] == e));
    for f in ours {
        admin("DELETE", &format!("/{}", f["id"].as_str().unwrap_or("")), None)?;
    }
    Ok(())
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
    Ok(Some(email))
}
