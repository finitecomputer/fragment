//! An inbox's own check and answer (docs/api.md, Apps, `inbox`), less the
//! network: a webhook signed with an HMAC (`inbox.hmac`), and what a
//! fragment's `answer` query says back. The shape is a webhook's, never a
//! vendor's: a hex HMAC-SHA256, under one of the fragment's secrets, of a
//! string made from the raw body and some of the delivery's headers.

use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;

use crate::manifest::inbox_header_ok;

/// `signed`, at most this many bytes.
pub const SIGNED_MAX_BYTES: usize = 256;
/// `prefix`, at most this many bytes.
pub const PREFIX_MAX_BYTES: usize = 32;
/// `within_s`, at most: an hour.
pub const WITHIN_MAX_S: u64 = 3600;

/// `inbox.hmac`: a delivery is taken only when `header` holds `prefix`
/// and then the hex HMAC-SHA256, under the fragment's secret `secret`, of
/// `signed` with `{body}` the raw body and each `{<header>}` that header's
/// value (Slack's `v0:{x-slack-request-timestamp}:{body}`), compared in
/// constant time; with `fresh`, a header `signed` names, only while the
/// Unix seconds in it are at most `within_s` from now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HmacDecl {
    pub secret: String,
    pub header: String,
    pub prefix: String,
    pub signed: String,
    /// The header holding the delivery's time, and how far from now it may be.
    pub fresh: Option<(String, u64)>,
}

/// A piece of `signed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part<'a> {
    Text(&'a str),
    Header(&'a str),
    Body,
}

/// `signed`'s pieces: text, `{body}` once, and `{<header>}`s an inbox may
/// read. A `{` or `}` is only ever a placeholder's.
fn parts(signed: &str) -> Result<Vec<Part<'_>>, String> {
    let mut out = vec![];
    let mut rest = signed;
    // bounded: each turn takes at least one byte of `signed`
    while let Some(open) = rest.find(['{', '}']) {
        if rest.as_bytes()[open] == b'}' {
            return Err(format!("inbox.hmac.signed has a }} that closes nothing ({signed:?})"));
        }
        let close = rest[open..].find('}').map(|i| open + i).ok_or_else(|| format!("inbox.hmac.signed has a {{ that is never closed ({signed:?})"))?;
        if open > 0 {
            out.push(Part::Text(&rest[..open]));
        }
        out.push(match &rest[open + 1..close] {
            "body" => Part::Body,
            h if inbox_header_ok(h) => Part::Header(h),
            h => return Err(format!("inbox.hmac.signed names {{body}} and lower-case headers, not the platform's or a browser's ({h:?})")),
        });
        rest = &rest[close + 1..];
    }
    if !rest.is_empty() {
        out.push(Part::Text(rest));
    }
    if out.iter().filter(|p| **p == Part::Body).count() != 1 {
        return Err(format!("inbox.hmac.signed names {{body}} once ({signed:?})"));
    }
    Ok(out)
}

impl HmacDecl {
    /// `inbox.hmac`, as its code declares it: every key known, each of its shape.
    pub fn read(v: &Value) -> Result<HmacDecl, String> {
        let o = v.as_object().ok_or("inbox.hmac must be an object: {secret, header, prefix?, signed, fresh?, within_s?}")?;
        if let Some(k) = o.keys().find(|k| !matches!(k.as_str(), "secret" | "header" | "prefix" | "signed" | "fresh" | "within_s")) {
            return Err(format!("inbox.hmac.{k} means nothing: inbox.hmac is {{secret, header, prefix?, signed, fresh?, within_s?}}"));
        }
        let text = |k: &str| o.get(k).map(|v| v.as_str().ok_or_else(|| format!("inbox.hmac.{k} must be a string"))).transpose();
        let secret = text("secret")?.unwrap_or("");
        if !fragment_proto::valid_secret_name(secret) {
            return Err(format!("inbox.hmac.secret names one of its secrets (^[A-Z][A-Z0-9_]*$), not {secret:?}"));
        }
        let header = text("header")?.unwrap_or("");
        if !inbox_header_ok(header) {
            return Err(format!("inbox.hmac.header is a lower-case header name, not the platform's or a browser's ({header:?})"));
        }
        let prefix = text("prefix")?.unwrap_or("");
        if prefix.len() > PREFIX_MAX_BYTES || !prefix.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(format!("inbox.hmac.prefix is at most {PREFIX_MAX_BYTES} printable characters, no spaces ({prefix:?})"));
        }
        let signed = text("signed")?.ok_or("inbox.hmac needs signed: what is signed, naming {body}")?;
        if signed.len() > SIGNED_MAX_BYTES {
            return Err(format!("inbox.hmac.signed is at most {SIGNED_MAX_BYTES} bytes"));
        }
        let pieces = parts(signed)?;
        let fresh = match (text("fresh")?, o.get("within_s")) {
            (None, None) => None,
            (Some(h), Some(w)) => {
                if !inbox_header_ok(h) {
                    return Err(format!("inbox.hmac.fresh is a lower-case header name, not the platform's or a browser's ({h:?})"));
                }
                // an unsigned time is anyone's to set: a captured delivery, re-timed, would be new again
                if !pieces.contains(&Part::Header(h)) {
                    return Err(format!("inbox.hmac.signed names {{{h}}}, the time fresh reads, so its time is signed too"));
                }
                let w = w.as_u64().filter(|w| (1..=WITHIN_MAX_S).contains(w)).ok_or_else(|| format!("inbox.hmac.within_s is whole seconds, 1 to {WITHIN_MAX_S}"))?;
                Some((h.to_string(), w))
            }
            _ => return Err("inbox.hmac.fresh and within_s come together: the header holding its time, and how far from now it may be".into()),
        };
        Ok(HmacDecl { secret: secret.into(), header: header.into(), prefix: prefix.into(), signed: signed.into(), fresh })
    }

    /// As its code declared it (`read` reads it back the same).
    pub fn to_json(&self) -> Value {
        let mut v = json!({ "secret": self.secret, "header": self.header, "prefix": self.prefix, "signed": self.signed });
        if let Some((h, w)) = &self.fresh {
            v["fresh"] = h.as_str().into();
            v["within_s"] = (*w).into();
        }
        v
    }

    fn mac(&self, key: &[u8], header: &dyn Fn(&str) -> Option<String>, body: &[u8]) -> Result<Hmac<Sha256>, Refused> {
        let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes any key length");
        for part in parts(&self.signed).expect("a declared inbox.hmac.signed parses") {
            match part {
                Part::Text(t) => mac.update(t.as_bytes()),
                Part::Header(h) => mac.update(header(h).ok_or_else(|| Refused::Missing(h.into()))?.as_bytes()),
                Part::Body => mac.update(body),
            }
        }
        Ok(mac)
    }

    /// Whether a delivery is signed with `key`, now. `header` reads one of
    /// its headers by name.
    pub fn check(&self, key: &[u8], header: &dyn Fn(&str) -> Option<String>, body: &[u8], now_s: i64) -> Result<(), Refused> {
        let given = header(&self.header).ok_or_else(|| Refused::Missing(self.header.clone()))?;
        let sig = given.strip_prefix(self.prefix.as_str()).ok_or(Refused::Malformed)?;
        let sig = hex::decode(sig).map_err(|_| Refused::Malformed)?;
        if let Some((h, within)) = &self.fresh {
            let at: i64 = header(h).ok_or_else(|| Refused::Missing(h.clone()))?.trim().parse().map_err(|_| Refused::Malformed)?;
            if now_s.abs_diff(at) > *within {
                return Err(Refused::Stale);
            }
        }
        self.mac(key, header, body)?.verify_slice(&sig).map_err(|_| Refused::Mismatch)
    }

    /// The header value that signs `body` with `key` (the fakes', and tests').
    pub fn sign(&self, key: &[u8], header: &dyn Fn(&str) -> Option<String>, body: &[u8]) -> Option<String> {
        let mac = self.mac(key, header, body).ok()?;
        Some(format!("{}{}", self.prefix, hex::encode(mac.finalize().into_bytes())))
    }
}

/// Why a delivery's HMAC admits nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// A header it names is not there.
    Missing(String),
    /// The signature lacks its prefix or is not hex, or the time is not whole seconds.
    Malformed,
    /// Its time is farther from now than `within_s`.
    Stale,
    /// No HMAC of what it signs under the secret.
    Mismatch,
}

impl Refused {
    pub fn message(&self) -> String {
        match self {
            Refused::Missing(h) => format!("no {h} header"),
            Refused::Malformed => "a signature is its prefix then hex, and a time whole Unix seconds".into(),
            Refused::Stale => "its time is too far from now".into(),
            Refused::Mismatch => "its signature does not match".into(),
        }
    }
}

/// What an inbox's `answer` query said back, as its delivery's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// `null`: the platform's own, `{ok, seq, runs}`.
    Platform,
    /// `{body: "<text>"}`: that text, as `text/plain`.
    Text(String),
    /// `{body: <any other JSON>}`: that JSON.
    Json(String),
}

/// An `answer` query's result: `null`, or `{body}`. Its content type
/// follows its body, so no fragment answers HTML, or anything else a
/// browser runs, from the platform's origin.
pub fn answer(result: &Value) -> Result<Answer, String> {
    let shape = || "an inbox's answer is null or {body}".to_string();
    match result {
        Value::Null => Ok(Answer::Platform),
        Value::Object(o) if o.len() == 1 => match o.get("body").ok_or_else(shape)? {
            Value::String(s) => Ok(Answer::Text(s.clone())),
            v => Ok(Answer::Json(v.to_string())),
        },
        _ => Err(shape()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8] = b"8f742231b10e8888abcd99yyyzzz85a5";

    fn slack() -> HmacDecl {
        HmacDecl::read(&json!({ "secret": "SLACK_SIGNING_SECRET", "header": "x-slack-signature", "prefix": "v0=",
            "signed": "v0:{x-slack-request-timestamp}:{body}", "fresh": "x-slack-request-timestamp", "within_s": 300 }))
        .unwrap()
    }

    fn headers(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let pairs: Vec<(String, String)> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |name: &str| pairs.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
    }

    /// Goal: Slack's own published example verifies, so the template and
    /// the order of its pieces are Slack's (docs.slack.dev, "Verifying
    /// requests from Slack"). Method: its key, time, body and signature.
    #[test]
    fn slacks_published_example_verifies() {
        let body = b"token=xyzz0WbapA4vBCDEFasx0q6G&team_id=T1DC2JH3J&team_domain=testteamnow&channel_id=G8PSS9T3V&channel_name=foobar&user_id=U2CERLKJA&user_name=roadrunner&command=%2Fwebhook-collect&text=&response_url=https%3A%2F%2Fhooks.slack.com%2Fcommands%2FT1DC2JH3J%2F397700885554%2F96rGlfmibIGlgcZRskXaIFfN&trigger_id=398738663015.47445629121.803a0bc887a14d10d2c447fce8b6703c";
        let h = headers(&[("x-slack-request-timestamp", "1531420618"), ("x-slack-signature", "v0=a2114d57b48eac39b9ad189dd8316235a7b4a8d21a10bd27519666489c69b503")]);
        assert_eq!(slack().check(KEY, &h, body, 1531420618 + 299), Ok(()));
    }

    /// Goal: a delivery is taken only signed, now, by its secret, over
    /// these bytes; each refusal says why (valid, invalid). Method: one
    /// signed delivery, then each piece of it wrong in turn.
    #[test]
    fn a_delivery_is_taken_only_signed_now() {
        let d = slack();
        let (now, body) = (1_790_000_000_i64, br#"{"type":"event_callback"}"#.as_slice());
        let at = now.to_string();
        let sig = d.sign(KEY, &headers(&[("x-slack-request-timestamp", &at)]), body).unwrap();
        assert!(sig.starts_with("v0=") && sig.len() == 3 + 64, "{sig}");
        let signed = headers(&[("x-slack-request-timestamp", &at), ("x-slack-signature", &sig)]);
        assert_eq!(d.check(KEY, &signed, body, now), Ok(()), "good");
        assert_eq!(d.check(KEY, &headers(&[("x-slack-request-timestamp", &at), ("x-slack-signature", &sig.to_uppercase().replacen("V0=", "v0=", 1))]), body, now), Ok(()), "hex in either case");
        assert_eq!(d.check(b"another secret", &signed, body, now), Err(Refused::Mismatch), "another secret");
        assert_eq!(d.check(KEY, &signed, br#"{"type":"event_callback" }"#, now), Err(Refused::Mismatch), "other bytes");
        let flipped = format!("{}{}", &sig[..sig.len() - 1], if sig.ends_with('0') { '1' } else { '0' });
        assert_eq!(d.check(KEY, &headers(&[("x-slack-request-timestamp", &at), ("x-slack-signature", &flipped)]), body, now), Err(Refused::Mismatch), "a bad signature");
        let earlier = (now - 1).to_string();
        assert_eq!(d.check(KEY, &headers(&[("x-slack-request-timestamp", &earlier), ("x-slack-signature", &sig)]), body, now), Err(Refused::Mismatch), "the time is signed");
        assert_eq!(d.check(KEY, &signed, body, now + 300), Ok(()), "at the window's edge");
        assert_eq!(d.check(KEY, &signed, body, now + 301), Err(Refused::Stale), "stale");
        assert_eq!(d.check(KEY, &signed, body, now - 301), Err(Refused::Stale), "from the future");
        let v1 = sig.replacen("v0=", "v1=", 1);
        assert_eq!(d.check(KEY, &headers(&[("x-slack-request-timestamp", &at), ("x-slack-signature", &v1)]), body, now), Err(Refused::Malformed), "the wrong prefix");
        let bare = sig.trim_start_matches("v0=").to_string();
        assert_eq!(d.check(KEY, &headers(&[("x-slack-request-timestamp", &at), ("x-slack-signature", &bare)]), body, now), Err(Refused::Malformed), "no prefix");
        assert_eq!(d.check(KEY, &headers(&[("x-slack-request-timestamp", &at), ("x-slack-signature", "v0=zz")]), body, now), Err(Refused::Malformed), "not hex");
        assert_eq!(d.check(KEY, &headers(&[("x-slack-request-timestamp", "soon"), ("x-slack-signature", &sig)]), body, now), Err(Refused::Malformed), "a time not in seconds");
        assert_eq!(d.check(KEY, &headers(&[("x-slack-request-timestamp", &at)]), body, now), Err(Refused::Missing("x-slack-signature".into())), "no signature");
        assert_eq!(d.check(KEY, &headers(&[("x-slack-signature", &sig)]), body, now), Err(Refused::Missing("x-slack-request-timestamp".into())), "no time");
        assert_eq!(Refused::Missing("x-slack-signature".into()).message(), "no x-slack-signature header");
    }

    /// Goal: a webhook signing its body alone, with no time (GitHub's
    /// shape), is one too.
    #[test]
    fn a_body_alone_with_no_time() {
        let d = HmacDecl::read(&json!({ "secret": "GITHUB_WEBHOOK_SECRET", "header": "x-hub-signature-256", "prefix": "sha256=", "signed": "{body}" })).unwrap();
        assert_eq!(d.fresh, None);
        let sig = d.sign(b"It's a Secret to Everybody", &headers(&[]), b"Hello, World!").unwrap();
        // GitHub's published example (docs.github.com, "Validating webhook deliveries")
        assert_eq!(sig, "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17");
        assert_eq!(d.check(b"It's a Secret to Everybody", &headers(&[("x-hub-signature-256", &sig)]), b"Hello, World!", 0), Ok(()));
    }

    /// Goal: `inbox.hmac` reads only its own keys, each of its shape, and
    /// a `signed` naming `{body}` once and only headers an inbox reads;
    /// what it reads it writes back the same.
    #[test]
    fn its_declaration_is_checked() {
        let ok = json!({ "secret": "S", "header": "x-sig", "signed": "{x-time}.{body}" });
        let d = HmacDecl::read(&ok).unwrap();
        assert_eq!(d.prefix, "");
        assert_eq!(HmacDecl::read(&d.to_json()).unwrap(), d);
        assert_eq!(HmacDecl::read(&slack().to_json()).unwrap(), slack());
        let with = |k: &str, v: Value| {
            let mut o = ok.clone();
            o[k] = v;
            HmacDecl::read(&o)
        };
        let refused = [
            with("signed", json!("v0:{x-time}")),
            with("signed", json!("{body}{body}")),
            with("signed", json!("{body}}")),
            with("signed", json!("{body")),
            with("signed", json!("{x-fragment-inbox-token}{body}")),
            with("signed", json!("{cookie}{body}")),
            with("signed", json!("{X-Time}{body}")),
            with("signed", json!(format!("{}{{body}}", "a".repeat(SIGNED_MAX_BYTES)))),
            with("secret", json!("lower")),
            with("header", json!("authorization")),
            with("prefix", json!("v0 =")),
            with("prefix", json!("p".repeat(PREFIX_MAX_BYTES + 1))),
            with("fresh", json!("x-time")),
            with("within_s", json!(300)),
            with("also", json!(1)),
            HmacDecl::read(&json!({ "secret": "S", "header": "x-sig" })),
            HmacDecl::read(&json!("x")),
        ];
        for r in refused {
            assert!(r.is_err(), "{r:?}");
        }
        let fresh = |w: Value| HmacDecl::read(&json!({ "secret": "S", "header": "x-sig", "signed": "{x-time}.{body}", "fresh": "x-time", "within_s": w }));
        assert_eq!(fresh(json!(1)).unwrap().fresh, Some(("x-time".into(), 1)));
        assert_eq!(fresh(json!(WITHIN_MAX_S)).unwrap().fresh, Some(("x-time".into(), WITHIN_MAX_S)));
        for bad in [json!(0), json!(WITHIN_MAX_S + 1), json!(-1), json!(1.5), json!("300")] {
            assert!(fresh(bad.clone()).is_err(), "{bad}");
        }
        // its time is signed, or a captured delivery is new again with a new time
        for signed in ["{body}", "{x-other}.{body}"] {
            let e = HmacDecl::read(&json!({ "secret": "S", "header": "x-sig", "signed": signed, "fresh": "x-time", "within_s": 300 })).unwrap_err();
            assert!(e.contains("{x-time}") && e.contains("signed"), "{signed}: {e}");
        }
    }

    /// Goal: a captured delivery, its time header set to now, is refused:
    /// no declaration deploy takes leaves its time unsigned (valid,
    /// replay). Method: one signed a day ago, re-timed, under Slack's
    /// template and under one signing its body alone.
    #[test]
    fn a_captured_delivery_retimed_is_refused() {
        let (then, now) = (1_790_000_000_i64, 1_790_000_000 + 86_400);
        let body = br#"{"type":"event_callback"}"#.as_slice();
        let mut deployed = 0;
        for signed in ["v0:{x-time}:{body}", "{body}"] {
            let Ok(d) = HmacDecl::read(&json!({ "secret": "S", "header": "x-sig", "prefix": "v0=", "signed": signed, "fresh": "x-time", "within_s": 300 })) else { continue };
            deployed += 1;
            let sig = d.sign(KEY, &headers(&[("x-time", &then.to_string())]), body).unwrap();
            assert_eq!(d.check(KEY, &headers(&[("x-time", &then.to_string()), ("x-sig", &sig)]), body, then), Ok(()), "{signed}: taken when it came");
            let retimed = headers(&[("x-time", &now.to_string()), ("x-sig", &sig)]);
            assert_eq!(d.check(KEY, &retimed, body, now), Err(Refused::Mismatch), "{signed}: replayed with a new time");
        }
        assert_eq!(deployed, 1, "Slack's template deploys, and one leaving its time unsigned does not");
    }

    /// Goal: an answer is null or `{body}`, its type its body's.
    #[test]
    fn an_answer_is_null_or_a_body() {
        assert_eq!(answer(&Value::Null), Ok(Answer::Platform));
        assert_eq!(answer(&json!({ "body": "3eZbrw1aBm2rZgRNFdxV2595E9CY3gmdALWMmHkvFXO7tYXAYM8P" })), Ok(Answer::Text("3eZbrw1aBm2rZgRNFdxV2595E9CY3gmdALWMmHkvFXO7tYXAYM8P".into())));
        assert_eq!(answer(&json!({ "body": { "type": 1 } })), Ok(Answer::Json(r#"{"type":1}"#.into())));
        for bad in [json!({}), json!({ "body": "x", "type": "text/html" }), json!({ "status": 200 }), json!("x"), json!([])] {
            assert!(answer(&bad).is_err(), "{bad}");
        }
    }
}
