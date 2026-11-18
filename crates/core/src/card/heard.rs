//! What the page reported as it loaded, bounded as it is heard.

use super::limits::{ERRORS_MAX, ERROR_BYTES_MAX};
use super::schedule::valid_live;
use fragment_proto::{PageError, PageErrorKind, PageReport};
use serde_json::Value;

/// What one try heard the page report over CDP (`Runtime` and `Log`
/// enabled on its target), bounded as it is heard.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Heard {
    pub errors: Vec<PageError>,
    pub dropped: u32,
    /// The share link's token, when the shot holds it: taken out of all
    /// it keeps (`hiding`).
    hidden: Option<String>,
}

/// What a report says where the share link's token was.
pub const HIDDEN_TOKEN: &str = "[share link]";

impl Heard {
    /// What a shot that holds `view_token` (a link holder's) hears: the
    /// token taken out of every text and source it keeps, whatever the
    /// page logs, since every viewer reads the report (`status.page`, the
    /// `page.errors` event) and the token is its owner's alone. The shot
    /// never puts it in the page's URL; this is the second guard.
    pub fn hiding(view_token: Option<&str>) -> Heard {
        assert!(view_token.is_none_or(|t| !t.is_empty()), "a token hidden is a token");
        Heard { hidden: view_token.map(str::to_string), ..Heard::default() }
    }

    /// A message from the page's target, kept if it is one of the page's
    /// errors: `Runtime.exceptionThrown`, `Runtime.consoleAPICalled` of an
    /// error or a failed assert, `Log.entryAdded` of an error from the
    /// network or security. Anything else is not, nor a failed
    /// `/favicon.ico`: Chrome asks for it whether or not the page names it.
    pub fn hear(&mut self, msg: &Value) {
        let p = &msg["params"];
        let (kind, text, source) = match msg["method"].as_str() {
            Some("Runtime.exceptionThrown") => {
                let d = &p["exceptionDetails"];
                // the thrown value says it best ("TypeError: …" and its stack)
                let text = if d["exception"].is_object() { remote_text(&d["exception"]) } else { d["text"].as_str().unwrap_or_default().to_string() };
                (PageErrorKind::Exception, text, source_of(d))
            }
            Some("Runtime.consoleAPICalled") if matches!(p["type"].as_str(), Some("error" | "assert")) => {
                let args = p["args"].as_array().map(Vec::as_slice).unwrap_or_default();
                (PageErrorKind::Console, args.iter().map(remote_text).collect::<Vec<_>>().join(" "), source_of(&p["stackTrace"]["callFrames"][0]))
            }
            Some("Log.entryAdded") if p["entry"]["level"] == "error" => {
                let kind = match p["entry"]["source"].as_str() {
                    Some("network") if p["entry"]["url"].as_str().and_then(|u| url::Url::parse(u).ok()).is_some_and(|u| u.path() == "/favicon.ico") => return,
                    Some("network") => PageErrorKind::Network,
                    Some("security") => PageErrorKind::Security,
                    _ => return,
                };
                (kind, p["entry"]["text"].as_str().unwrap_or_default().to_string(), source_of(&p["entry"]))
            }
            _ => return,
        };
        self.keep(kind, &text, source.as_deref());
    }

    /// One error, the token taken out and then cut to size (so no cut
    /// leaves part of it); past `ERRORS_MAX`, only counted.
    pub fn keep(&mut self, kind: PageErrorKind, text: &str, source: Option<&str>) {
        if self.errors.len() >= ERRORS_MAX {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        let hide = |s: &str| match &self.hidden {
            Some(token) => s.replace(token.as_str(), HIDDEN_TOKEN),
            None => s.to_string(),
        };
        let error = PageError { kind, text: cut(&hide(text)), source: source.map(|s| cut(&hide(s))) };
        if let Some(token) = &self.hidden {
            assert!(!error.text.contains(token.as_str()) && !error.source.as_deref().is_some_and(|s| s.contains(token.as_str())), "a kept error holds no token");
        }
        self.errors.push(error);
    }
}

/// A CDP `RemoteObject` as the console prints it, near enough: a string
/// as itself, an object by its description (an `Error`'s is its message
/// and stack), another value as JSON, `undefined` by its type.
fn remote_text(o: &Value) -> String {
    match (&o["value"], o["description"].as_str().or(o["unserializableValue"].as_str())) {
        (Value::String(s), _) => s.clone(),
        (_, Some(d)) => d.to_string(),
        (Value::Null, None) if o["subtype"] != "null" => o["type"].as_str().unwrap_or("undefined").to_string(),
        (v, None) => v.to_string(),
    }
}

/// Where a CDP exception, stack frame, or log entry points: its URL, with
/// its line and column (1-based; CDP's are 0-based) when it has both.
fn source_of(at: &Value) -> Option<String> {
    let url = at["url"].as_str().filter(|u| !u.is_empty())?;
    Some(match (at["lineNumber"].as_u64(), at["columnNumber"].as_u64()) {
        (Some(line), Some(column)) => format!("{url}:{}:{}", line + 1, column + 1),
        _ => url.to_string(),
    })
}

/// `s` with no control character but a newline or a tab (any other is a
/// space), cut on a character to at most `ERROR_BYTES_MAX` bytes, ending
/// `…` when cut.
pub fn cut(s: &str) -> String {
    let clean = |c: char| if c.is_control() && c != '\n' && c != '\t' { ' ' } else { c };
    // a control character is never shorter than a space
    if s.len() <= ERROR_BYTES_MAX {
        return s.chars().map(clean).collect();
    }
    let mut out = String::with_capacity(ERROR_BYTES_MAX);
    for c in s.chars().map(clean) {
        if out.len() + c.len_utf8() > ERROR_BYTES_MAX - '…'.len_utf8() {
            break;
        }
        out.push(c);
    }
    out.push('…');
    out
}

/// A `page.errors` event's summary: how many, of which live, and the
/// first one's first line.
pub fn summary(report: &PageReport) -> String {
    assert!(valid_live(&report.live), "a report names a commit");
    let n = report.errors.len() as u64 + u64::from(report.dropped);
    let first = report.errors.first().and_then(|e| e.text.lines().next()).unwrap_or_default();
    format!("the page reported {n} error{} as it loaded (live {}): {first}", if n == 1 { "" } else { "s" }, &report.live[..12])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::schedule::{Cards, Outcome, Verdict};
    use crate::card::testing::{image, shoot, A, B, C, PACE, T0};

    fn heard(msgs: &[Value]) -> Heard {
        let mut h = Heard::default();
        msgs.iter().for_each(|m| h.hear(m));
        h
    }

    fn error(kind: PageErrorKind, text: &str, source: Option<&str>) -> PageError {
        PageError { kind, text: text.into(), source: source.map(str::to_string) }
    }

    /// Goal: what the page reports as it loads is heard as Chrome says it,
    /// and nothing else is. Method: each kind as CDP sends it, then the
    /// messages a load also sends that are no error.
    #[test]
    fn the_pages_errors_are_heard() {
        use serde_json::json;
        use PageErrorKind::*;
        let page = "http://a--ann.fragment.localhost:8790/";
        let h = heard(&[
            json!({ "method": "Runtime.exceptionThrown", "sessionId": "s", "params": { "exceptionDetails": {
                "text": "Uncaught", "lineNumber": 2, "columnNumber": 8, "url": page,
                "exception": { "type": "object", "subtype": "error", "className": "Error", "description": "Error: boom\n    at http://a/:3:9" } } } }),
            json!({ "method": "Runtime.exceptionThrown", "params": { "exceptionDetails": { "text": "Uncaught", "lineNumber": 0, "columnNumber": 0, "exception": { "type": "string", "value": "thrown" } } } }),
            json!({ "method": "Runtime.exceptionThrown", "params": { "exceptionDetails": { "text": "Uncaught SyntaxError: Unexpected token '}'", "lineNumber": 4, "columnNumber": 1, "url": "http://a/app.js" } } }),
            json!({ "method": "Runtime.consoleAPICalled", "params": { "type": "error", "args": [
                { "type": "string", "value": "failed:" }, { "type": "number", "value": 42, "description": "42" }, { "type": "boolean", "value": true },
                { "type": "object", "className": "Object", "description": "Object", "objectId": "1" }, { "type": "undefined" }, { "type": "object", "subtype": "null", "value": null },
                { "type": "number", "unserializableValue": "NaN" } ],
                "stackTrace": { "callFrames": [{ "functionName": "", "url": "http://a/app.js", "lineNumber": 9, "columnNumber": 4 }] } } }),
            json!({ "method": "Runtime.consoleAPICalled", "params": { "type": "assert", "args": [{ "type": "string", "value": "console.assert" }] } }),
            json!({ "method": "Log.entryAdded", "params": { "entry": { "source": "network", "level": "error", "text": "Failed to load resource: the server responded with a status of 404 (Not Found)", "url": "http://a/missing.js" } } }),
            json!({ "method": "Log.entryAdded", "params": { "entry": { "source": "security", "level": "error", "text": "Refused to load the image 'http://a/x.png' because it violates the following Content Security Policy directive: \"img-src 'none'\".", "url": page, "lineNumber": 3 } } }),
        ]);
        assert_eq!(
            h.errors,
            [
                error(Exception, "Error: boom\n    at http://a/:3:9", Some(&format!("{page}:3:9"))),
                error(Exception, "thrown", None),
                error(Exception, "Uncaught SyntaxError: Unexpected token '}'", Some("http://a/app.js:5:2")),
                error(Console, "failed: 42 true Object undefined null NaN", Some("http://a/app.js:10:5")),
                error(Console, "console.assert", None),
                error(Network, "Failed to load resource: the server responded with a status of 404 (Not Found)", Some("http://a/missing.js")),
                error(Security, "Refused to load the image 'http://a/x.png' because it violates the following Content Security Policy directive: \"img-src 'none'\".", Some(page)),
            ]
        );
        assert_eq!(h.dropped, 0);
        let quiet = heard(&[
            json!({ "method": "Runtime.consoleAPICalled", "params": { "type": "log", "args": [{ "type": "string", "value": "hi" }] } }),
            json!({ "method": "Runtime.consoleAPICalled", "params": { "type": "warning", "args": [] } }),
            json!({ "method": "Log.entryAdded", "params": { "entry": { "source": "network", "level": "warning", "text": "slow" } } }),
            json!({ "method": "Log.entryAdded", "params": { "entry": { "source": "deprecation", "level": "error", "text": "old" } } }),
            json!({ "method": "Log.entryAdded", "params": { "entry": { "source": "network", "level": "error", "text": "Failed to load resource: the server responded with a status of 404 (Not Found)", "url": "http://a--ann.fragment.localhost:8790/favicon.ico" } } }),
            json!({ "method": "Log.entryAdded", "params": { "entry": { "source": "security" } } }),
            json!({ "method": "Runtime.executionContextCreated", "params": { "context": {} } }),
            json!({ "method": "Page.loadEventFired", "params": { "timestamp": 1.0 } }),
            json!({ "id": 7, "result": { "exceptionDetails": { "text": "an answer, not an event" } } }),
            json!({ "method": 7 }),
            json!(null),
            json!("Runtime.exceptionThrown"),
        ]);
        assert_eq!(quiet, Heard::default(), "only errors are heard");
    }

    /// Goal: a link holder's shot reports no token, whatever the page logs
    /// or throws (the audit's M2: `console.error(location.href)`, or an
    /// uncaught error whose source is the page's URL). Method: the token
    /// in a console error's text, an exception's source, a network
    /// error's URL, and one long enough to be cut where the token is.
    #[test]
    fn a_link_holders_report_holds_no_token() {
        use serde_json::json;
        let token = "a1b2c3d4e5f6a7b8c9d0e1f2";
        let page = format!("http://a--ann.fragment.localhost:8790/?view={token}");
        let mut h = Heard::hiding(Some(token));
        h.hear(&json!({ "method": "Runtime.consoleAPICalled", "params": { "type": "error", "args": [{ "type": "string", "value": page }],
            "stackTrace": { "callFrames": [{ "url": page, "lineNumber": 0, "columnNumber": 0 }] } } }));
        h.hear(&json!({ "method": "Runtime.exceptionThrown", "params": { "exceptionDetails": { "text": "Uncaught", "url": page, "lineNumber": 1, "columnNumber": 2,
            "exception": { "type": "object", "description": format!("Error: at {page}") } } } }));
        h.hear(&json!({ "method": "Log.entryAdded", "params": { "entry": { "source": "network", "level": "error", "text": format!("GET {page} 500"), "url": page } } }));
        h.keep(PageErrorKind::Console, &format!("{}{token}", "x".repeat(ERROR_BYTES_MAX - 4)), None);
        assert_eq!(h.errors.len(), 4);
        for e in &h.errors {
            assert!(!e.text.contains(token) && !e.source.as_deref().unwrap_or("").contains(token), "{e:?}");
            assert!(!e.text.contains(&token[..12]), "no part of it: {e:?}");
        }
        assert_eq!(h.errors[0].text, format!("http://a--ann.fragment.localhost:8790/?view={HIDDEN_TOKEN}"));
        assert_eq!(h.errors[1].source.as_deref(), Some(format!("http://a--ann.fragment.localhost:8790/?view={HIDDEN_TOKEN}:2:3").as_str()));
        let mut anyone = Heard::hiding(None);
        anyone.keep(PageErrorKind::Console, &page, None);
        assert_eq!(anyone.errors[0].text, page, "a shot that holds no token hides nothing");
    }

    /// Goal: what a shot keeps of the page's errors is bounded: the first
    /// `ERRORS_MAX` (the rest counted), each text and source cut to
    /// `ERROR_BYTES_MAX` on a character, no control character but a
    /// newline or a tab; so a report at its largest is one event.
    #[test]
    fn what_a_shot_keeps_is_bounded() {
        let mut h = Heard::default();
        for i in 0..ERRORS_MAX + 3 {
            h.keep(PageErrorKind::Console, &format!("e{i}"), None);
        }
        assert_eq!((h.errors.len(), h.dropped, h.errors[ERRORS_MAX - 1].text.as_str()), (ERRORS_MAX, 3, "e9"), "the first are kept, the rest counted");
        let whole = "x".repeat(ERROR_BYTES_MAX);
        assert_eq!(cut(&whole), whole, "exactly the cap is kept whole");
        let over = cut(&format!("{whole}y"));
        assert!(over.len() <= ERROR_BYTES_MAX && over.ends_with('…') && over.starts_with("xxx"), "{}", over.len());
        let wide = cut(&"é".repeat(ERROR_BYTES_MAX));
        assert!(wide.len() <= ERROR_BYTES_MAX && wide.ends_with("é…"), "cut on a character: {}", wide.len());
        assert_eq!(cut("a\u{0}b\rc\u{1b}[31md\u{85}e\nf\tg"), "a b c [31md e\nf\tg");
        let mut h = Heard::default();
        h.keep(PageErrorKind::Network, "x", Some(&format!("http://a/{}", "p".repeat(2 * ERROR_BYTES_MAX))));
        assert!(h.errors[0].source.as_ref().is_some_and(|s| s.len() <= ERROR_BYTES_MAX && s.ends_with('…')), "a source is cut too");
        // the largest report: every error at its longest, every byte escaped
        let mut h = Heard { dropped: u32::MAX - 1, ..Heard::default() };
        for _ in 0..ERRORS_MAX + 1 {
            h.keep(PageErrorKind::Exception, &"\"".repeat(2 * ERROR_BYTES_MAX), Some(&"\\".repeat(2 * ERROR_BYTES_MAX)));
        }
        assert_eq!(h.dropped, u32::MAX, "the count saturates");
        let report = PageReport { live: "f".repeat(64), at: T0, errors: h.errors, dropped: h.dropped };
        let event = serde_json::json!({ "summary": summary(&report), "data": report });
        let size = serde_json::to_string(&event).unwrap().len();
        assert!(size <= fragment_proto::limits::RECORD_BODY_MAX_BYTES, "{size} bytes");
        let one = PageReport { live: A.into(), at: T0, errors: vec![error(PageErrorKind::Exception, "Error: boom\n    at x", None)], dropped: 0 };
        assert_eq!(summary(&one), "the page reported 1 error as it loaded (live aaaaaaaaaaaa): Error: boom");
    }

    /// Goal: the page's report is the one the try that ended its live's
    /// tries heard: kept or given up, not a retry's or a stale try's, and
    /// a replay changes nothing; it survives a restart, and a state kept
    /// without one reads back as none. Method: land tries with what they
    /// heard.
    #[test]
    fn the_report_is_the_ending_tries() {
        let boom = || Heard { errors: vec![error(PageErrorKind::Exception, "Error: boom", None)], ..Heard::default() };
        let mut c = Cards::default();
        c.want(A, T0).unwrap();
        let s = shoot(&mut c, T0);
        let v = c.landed(&s, Outcome::Failed { retry: true }, T0 + 1, PACE).unwrap();
        assert_eq!(c.report(&v, A, boom(), T0 + 1), None, "a retry's is dropped");
        assert_eq!(c.page, None);
        let s = shoot(&mut c, T0 + 1 + PACE.first_ms);
        let v = c.landed(&s, image(1), T0 + PACE.first_ms + 2, PACE).unwrap();
        let made = c.report(&v, A, boom(), T0 + PACE.first_ms + 2).cloned();
        let want = PageReport { live: A.into(), at: T0 + PACE.first_ms + 2, errors: boom().errors, dropped: 0 };
        assert_eq!((made, c.page.clone()), (Some(want.clone()), Some(want.clone())), "the kept try's is the report");
        // a replayed landing is stale: the report stays
        let v = c.landed(&s, image(1), T0 + PACE.first_ms + 3, PACE).unwrap();
        assert_eq!((v.clone(), c.report(&v, A, Heard::default(), T0 + PACE.first_ms + 3)), (Verdict::Stale, None));
        assert_eq!(c.page, Some(want.clone()));
        // a clean page next replaces it with no errors
        c.want(B, T0 + 100_000).unwrap();
        let s = shoot(&mut c, T0 + 100_000);
        let v = c.landed(&s, image(2), T0 + 100_001, PACE).unwrap();
        c.report(&v, B, Heard::default(), T0 + 100_001);
        assert_eq!(c.page, Some(PageReport { live: B.into(), at: T0 + 100_001, errors: vec![], dropped: 0 }));
        // a try given up reports what it heard
        c.want(C, T0 + 200_000).unwrap();
        let s = shoot(&mut c, T0 + 200_000);
        let v = c.landed(&s, Outcome::Failed { retry: false }, T0 + 200_001, PACE).unwrap();
        assert_eq!(c.report(&v, C, boom(), T0 + 200_001).map(|p| p.live.as_str()), Some(C));
        let back: Cards = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c, "the report survives a restart");
        let old: Cards = serde_json::from_value(serde_json::json!({ "card": null, "wanted": null })).unwrap();
        assert_eq!(old.page, None);
    }
}
