//! `fragment api`: one call to a connected service's API, as the agent.
//!
//! On a computer an agent holds no credential: each connection it may use
//! is a placeholder in an environment variable (`fcx_<provider>_<tag>`,
//! docs/computers.md, "Connections and operator keys"), and the computer's
//! intercept swaps in a short-lived token on the way to that provider's own
//! hosts. This command is the one caller every connection's skill shares
//! (FIN-509): it finds the provider's placeholder, sends it as a bearer
//! token, never follows a redirect with it, prints the answer, and says a
//! refusal one way, with what to tell the owner. A skill is then its
//! provider's paths and quirks, and no code.
//!
//! It knows no provider: a name, a URL and a body are all it is given.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

/// A connection's placeholder begins so, a key's so (`fragment_core::swap`).
const PLACEHOLDER_PREFIXES: [&str; 2] = [fragment_core::swap::CONNECTION_PREFIX, fragment_core::swap::KEY_PREFIX];
/// How long one call may take, unless it says (`--timeout`, for a large
/// file), and the longest it may say.
const TIMEOUT_S: u64 = 120;
const TIMEOUT_MAX_S: u64 = 3600;
/// The most of an answer printed: a longer one is saved to a file, and the
/// file named, so an agent's context is never filled by one call.
pub const PRINT_MAX_BYTES: usize = 256 << 10;
/// The most of an answer read at all.
const ANSWER_MAX_BYTES: u64 = 512 << 20;

/// What `fragment api` was asked.
#[derive(Debug, Clone, Default)]
pub struct Call {
    pub provider: String,
    pub method: String,
    pub url: String,
    /// `name=value`, each added to the URL's query, encoded.
    pub params: Vec<String>,
    /// The body: JSON as written, `@file`, or `-` (stdin).
    pub body: Option<String>,
    /// A GraphQL document: the body is `{query, variables}`, and an answer
    /// that names `errors` is a failure.
    pub query: Option<String>,
    pub variables: Option<String>,
    pub content_type: Option<String>,
    /// `Name: value`, each sent as written.
    pub headers: Vec<String>,
    /// Where the answer's body is saved whole, instead of printed.
    pub out: Option<PathBuf>,
    /// The environment variable that holds the token, over the provider's
    /// placeholder (off a computer: any token of the provider's).
    pub env: Option<String>,
    /// Seconds the call may take, over `TIMEOUT_S`.
    pub timeout: Option<u64>,
}

/// A call that did not answer as asked: what `fragment api` says on stderr,
/// one line of JSON, and exits 1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub status: u16,
    pub code: String,
    pub message: String,
}

impl Refused {
    fn new(status: u16, code: &str, message: impl Into<String>) -> Refused {
        Refused { status, code: code.into(), message: message.into() }
    }

    /// The line an agent reads: the refusal, and what to do about it.
    pub fn said(&self, provider: &str) -> Value {
        json!({ "error": self.code, "status": self.status, "message": self.message, "hint": hint(&self.code, self.status, provider) })
    }
}

/// What to do about a refusal: the computer's own (`not_connected`,
/// `forbidden`), or the provider's by its status.
fn hint(code: &str, status: u16, provider: &str) -> String {
    match (code, status) {
        ("not_connected", _) => format!("your owner has not connected {provider} (or must again), or left it out of what you may use: ask them to connect it in Settings, under Connections"),
        ("forbidden", _) => format!("your owner narrowed your connections and left {provider} out, connected it read only (a change refused while reads work: they allow changes in Settings, under Connections), or the call went to a host that is not {provider}'s"),
        ("redirected", _) => "call the address it names yourself, if it is the provider's own: a redirect is never followed with the token".into(),
        ("insufficient_scope", _) => format!("the connection lacks this permission: your owner connected {provider} read only, or left this part out (they allow it in Settings, under Connections)"),
        (_, 401) => format!("{provider} refused the connection's token: ask your owner to reconnect {provider} in Settings, under Connections"),
        (_, 403) => format!("{provider} refused it: the connected account may not do this, or your owner left this permission out (they add it in Settings, under Connections, with a reconnect)"),
        (_, 429) => "the provider's rate limit: wait, then ask for less at a time".into(),
        _ => String::new(),
    }
}

/// The provider's token for this agent: the variable `env` names, else the
/// one whose value is the provider's placeholder. The second is whether it
/// is a placeholder (worthless anywhere but this computer's egress).
pub fn token(provider: &str, env: Option<&str>, vars: &[(String, String)]) -> Result<(String, bool), Refused> {
    let placeholder = |v: &str| PLACEHOLDER_PREFIXES.iter().any(|p| v.strip_prefix(p).and_then(|rest| rest.strip_prefix(provider)).and_then(|rest| rest.strip_prefix('_')).is_some_and(|tag| !tag.is_empty() && tag.bytes().all(|b| b.is_ascii_hexdigit())));
    if let Some(name) = env {
        let value = vars.iter().find(|(n, _)| n == name).map(|(_, v)| v.trim().to_string()).filter(|v| !v.is_empty());
        return value.map(|v| (v.clone(), placeholder(&v))).ok_or_else(|| Refused::new(0, "not_connected", format!("{name} is not set")));
    }
    // sorted, so two variables of one provider answer the same each time
    let mut held: Vec<&(String, String)> = vars.iter().filter(|(_, v)| placeholder(v.trim())).collect();
    held.sort();
    held.first().map(|(_, v)| (v.trim().to_string(), true)).ok_or_else(|| Refused::new(0, "not_connected", format!("no variable here holds {provider}'s placeholder: it is not connected for you")))
}

/// The address called: `url` with each of `params` added to its query.
/// HTTPS, or (a placeholder alone, which is no secret off this computer)
/// HTTP; never one that carries a user or a password.
pub fn address(url: &str, params: &[String], placeholder: bool) -> Result<reqwest::Url> {
    let mut u = reqwest::Url::parse(url).map_err(|e| anyhow!("{url:?} is no URL ({e}): name it whole, https://<the provider's host>/<path>"))?;
    if u.scheme() != "https" && !(placeholder && u.scheme() == "http") {
        return Err(anyhow!("{url:?}: a token goes over https alone"));
    }
    if u.host_str().is_none() || !u.username().is_empty() || u.password().is_some() {
        return Err(anyhow!("{url:?}: a host, and no user or password in the address"));
    }
    for p in params {
        let (name, value) = p.split_once('=').filter(|(n, _)| !n.is_empty()).ok_or_else(|| anyhow!("--param {p:?}: name=value"))?;
        u.query_pairs_mut().append_pair(name, value);
    }
    Ok(u)
}

/// The body sent and its content type: none, `--body`'s (JSON unless
/// `--content-type` says otherwise, and then held to be JSON), or a GraphQL
/// document's `{query, variables}`.
pub fn body(call: &Call, stdin: &mut dyn Read) -> Result<Option<(Vec<u8>, String)>> {
    if let Some(document) = &call.query {
        if call.body.is_some() {
            return Err(anyhow!("--query makes the body: name it or --body, not both"));
        }
        let variables: Value = match &call.variables {
            Some(v) => serde_json::from_str(v).ok().filter(Value::is_object).ok_or_else(|| anyhow!("--variables is a JSON object"))?,
            None => json!({}),
        };
        return Ok(Some((serde_json::to_vec(&json!({ "query": document, "variables": variables }))?, "application/json".into())));
    }
    if call.variables.is_some() {
        return Err(anyhow!("--variables go with --query"));
    }
    let Some(given) = &call.body else { return Ok(None) };
    let bytes = match given.as_str() {
        "-" => {
            let mut b = vec![];
            stdin.read_to_end(&mut b)?;
            b
        }
        g => match g.strip_prefix('@') {
            Some(file) => std::fs::read(file).map_err(|e| anyhow!("--body @{file}: {e}"))?,
            None => g.as_bytes().to_vec(),
        },
    };
    let content_type = call.content_type.clone().unwrap_or_else(|| "application/json".into());
    if content_type.starts_with("application/json") && serde_json::from_slice::<Value>(&bytes).is_err() {
        return Err(anyhow!("--body is not JSON (another kind of body names its --content-type)"));
    }
    Ok(Some((bytes, content_type)))
}

/// `--header`'s, each `Name: value`: never the token's own.
fn headers(given: &[String]) -> Result<Vec<(String, String)>> {
    given
        .iter()
        .map(|h| {
            let (name, value) = h.split_once(':').map(|(n, v)| (n.trim(), v.trim())).filter(|(n, _)| !n.is_empty()).ok_or_else(|| anyhow!("--header {h:?}: Name: value"))?;
            if name.eq_ignore_ascii_case("authorization") {
                return Err(anyhow!("--header: the token is sent for you, as Authorization"));
            }
            Ok((name.to_string(), value.to_string()))
        })
        .collect()
}

/// A refusal's code and words, from its body: the computer's own
/// (`{error: "not_connected" | "forbidden", message}`), OAuth's (`{error,
/// error_description}`), a list of errors (`[{errorCode | code, message}]`,
/// `{errors: [{message, extensions: {code}}]}`), or `{message}` / `{error:
/// {code | status, message}}`; else the start of what was said.
pub fn refusal(status: u16, text: &str) -> Refused {
    // a code in words, where the body has one, over a number
    let word = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(|s| s.to_lowercase().replace(' ', "_"));
    let code_of = |o: &Value| -> Option<String> {
        const KEYS: [&str; 8] = ["errorCode", "code", "category", "status", "error", "type", ".tag", "errorType"];
        let extensions = |o: &Value| o.get("extensions").and_then(|x| x.get("code").or(x.get("type"))).and_then(word);
        KEYS.iter().find_map(|k| o.get(k).and_then(word)).or_else(|| extensions(o)).or_else(|| KEYS.iter().find_map(|k| o.get(k).and_then(Value::as_i64)).map(|n| n.to_string()))
    };
    // what the provider wrote for a person, where it did, over its own words
    let message_of = |o: &Value| -> Option<String> {
        let said = |k: &str| o.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
        o.get("extensions").and_then(|x| x.get("userPresentableMessage")).and_then(Value::as_str).map(str::to_string).or_else(|| ["message", "error_description", "error_summary", "detail", "summary"].iter().find_map(|k| said(k))).or_else(|| o.get("errorMessages").and_then(|l| l.get(0)).and_then(Value::as_str).map(str::to_string))
    };
    let from = |o: &Value, around: Option<&Value>| -> Option<Refused> {
        let (code, message) = (code_of(o), message_of(o).or_else(|| around.and_then(message_of)));
        (code.is_some() || message.is_some()).then(|| Refused::new(status, &code.unwrap_or_default(), message.unwrap_or_else(|| "the provider refused the request".into())))
    };
    let parsed: Option<Value> = serde_json::from_str(text).ok();
    let found = parsed.as_ref().and_then(|v| match v {
        Value::Array(list) => list.first().and_then(|e| from(e, None)),
        Value::Object(o) => match (o.get("error"), o.get("errors")) {
            (Some(Value::String(code)), _) => Some(Refused::new(status, code, o.get("message").or(o.get("error_description")).and_then(Value::as_str).unwrap_or(code))),
            (Some(e @ Value::Object(_)), _) => from(e, Some(v)),
            (_, Some(Value::Array(list))) if !list.is_empty() => list.first().and_then(|e| from(e, None)),
            _ => from(v, None),
        },
        _ => None,
    });
    found.unwrap_or_else(|| Refused::new(status, "", if text.trim().is_empty() { format!("HTTP {status}") } else { text.chars().take(500).collect() }))
}

/// An answer taken whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

/// Sends the call and reads its answer whole: a body cut short of its
/// stated length is no answer (`incomplete`), so half a file is never
/// taken for one. A redirect is answered, never followed.
pub fn send(call: &Call, vars: &[(String, String)], stdin: &mut dyn Read) -> Result<Result<Answer, Refused>> {
    let method = reqwest::Method::from_bytes(call.method.to_ascii_uppercase().as_bytes()).ok().filter(|m| ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"].contains(&m.as_str())).ok_or_else(|| anyhow!("{:?} is no method: GET, POST, PUT, PATCH, DELETE or HEAD", call.method))?;
    if call.query.is_some() && method != reqwest::Method::POST {
        return Err(anyhow!("a GraphQL --query is a POST"));
    }
    let more = headers(&call.headers)?;
    let sent = body(call, stdin)?;
    let (token, placeholder) = match token(&call.provider, call.env.as_deref(), vars) {
        Ok(t) => t,
        Err(refused) => return Ok(Err(refused)),
    };
    let url = address(&call.url, &call.params, placeholder)?;
    let timeout = call.timeout.unwrap_or(TIMEOUT_S);
    if !(1..=TIMEOUT_MAX_S).contains(&timeout) {
        return Err(anyhow!("--timeout is 1 to {TIMEOUT_MAX_S} seconds"));
    }
    let client = reqwest::blocking::Client::builder().user_agent(concat!("fragment-cli/", env!("CARGO_PKG_VERSION"))).redirect(reqwest::redirect::Policy::none()).timeout(std::time::Duration::from_secs(timeout)).build()?;
    let mut request = client.request(method, url).bearer_auth(&token);
    // JSON is asked for unless the call says what it takes (a download)
    if !more.iter().any(|(name, _)| name.eq_ignore_ascii_case("accept")) {
        request = request.header("accept", "application/json");
    }
    for (name, value) in more {
        request = request.header(name, value);
    }
    if let Some((bytes, content_type)) = sent {
        request = request.header("content-type", content_type).body(bytes);
    }
    let response = match request.send() {
        Ok(r) => r,
        Err(e) => return Ok(Err(Refused::new(0, if e.is_timeout() { "timed_out" } else { "unreachable" }, format!("{}", e.without_url())))),
    };
    let status = response.status().as_u16();
    if (300..400).contains(&status) {
        let to = response.headers().get("location").and_then(|l| l.to_str().ok()).unwrap_or("?").to_string();
        return Ok(Err(Refused::new(status, "redirected", format!("the answer is a redirect to {to}, which is never followed with the token"))));
    }
    // a refusal's code where its body has none (RFC 6750: `WWW-Authenticate:
    // Bearer error="insufficient_scope"`), and how long to wait
    let header = |name: &str| response.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
    let challenge = |key: &str| header("www-authenticate").and_then(|c| c.split_once(&format!("{key}=\"")).and_then(|(_, rest)| rest.split_once('"')).map(|(v, _)| v.to_string()));
    let (challenged, described, retry_after) = (challenge("error"), challenge("error_description"), header("retry-after"));
    let stated = response.content_length();
    let content_type = response.headers().get("content-type").and_then(|c| c.to_str().ok()).map(str::to_string);
    let mut bytes = vec![];
    if let Err(e) = response.take(ANSWER_MAX_BYTES + 1).read_to_end(&mut bytes) {
        return Ok(Err(Refused::new(status, "incomplete", format!("the answer stopped after {} bytes: {e}", bytes.len()))));
    }
    if bytes.len() as u64 > ANSWER_MAX_BYTES || stated.is_some_and(|n| n != bytes.len() as u64) {
        return Ok(Err(Refused::new(status, "incomplete", format!("the answer is {} bytes of the {} it states (or over {ANSWER_MAX_BYTES}): nothing was kept", bytes.len(), stated.map(|n| n.to_string()).unwrap_or_else(|| "?".into())))));
    }
    if !(200..300).contains(&status) {
        let mut refused = refusal(status, &String::from_utf8_lossy(&bytes));
        if refused.code.is_empty() {
            if let Some(code) = challenged {
                refused.message = described.filter(|_| bytes.is_empty()).unwrap_or(refused.message);
                refused.code = code;
            }
        }
        if let (429, Some(wait)) = (status, retry_after) {
            refused.message = format!("{} (Retry-After: {wait})", refused.message);
        }
        return Ok(Err(refused));
    }
    // GraphQL answers 200 with what it refused
    if call.query.is_some() {
        if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
            if v.get("errors").and_then(Value::as_array).is_some_and(|e| !e.is_empty()) {
                return Ok(Err(refusal(status, &String::from_utf8_lossy(&bytes))));
            }
        }
    }
    Ok(Ok(Answer { status, content_type, body: bytes }))
}

/// Writes `bytes` to `path` whole or not at all: beside it first, then
/// renamed over it.
pub fn save(path: &Path, bytes: &[u8]) -> Result<()> {
    let name = path.file_name().ok_or_else(|| anyhow!("--out names a file, not {}", path.display()))?;
    let part = path.with_file_name(format!(".{}.part", name.to_string_lossy()));
    let written = std::fs::File::create(&part).and_then(|mut f| f.write_all(bytes).and_then(|_| f.sync_all())).and_then(|_| std::fs::rename(&part, path));
    written.map_err(|e| {
        let _ = std::fs::remove_file(&part);
        anyhow!("{}: {e}", path.display())
    })
}

/// What a good answer says on stdout: its body, or, saved (`--out`, or over
/// `PRINT_MAX_BYTES`, into `spill`), one line naming the file.
pub fn deliver(answer: &Answer, out: Option<&Path>, spill: &Path) -> Result<Vec<u8>> {
    let saved = |path: &Path, why: Option<&str>| -> Result<Vec<u8>> {
        save(path, &answer.body)?;
        let mut said = json!({ "status": answer.status, "saved": path.display().to_string(), "bytes": answer.body.len(), "contentType": answer.content_type });
        if let Some(why) = why {
            said["note"] = json!(why);
        }
        Ok(format!("{said}\n").into_bytes())
    };
    match out {
        Some(path) => saved(path, None),
        None if answer.body.len() > PRINT_MAX_BYTES => saved(spill, Some("the answer is too long to print, so it is saved: read the part you need, or ask the provider for less (fields, a page size)")),
        None => {
            let mut body = answer.body.clone();
            if !body.is_empty() && !body.ends_with(b"\n") {
                body.push(b'\n');
            }
            Ok(body)
        }
    }
}

/// `fragment api`: the answer on stdout and 0, or a refusal on stderr and 1.
/// A mistake in how it was called is the caller's error (exit 2).
pub fn run(call: Call) -> Result<i32> {
    let vars: Vec<(String, String)> = std::env::vars_os().filter_map(|(n, v)| Some((n.into_string().ok()?, v.into_string().ok()?))).collect();
    match send(&call, &vars, &mut std::io::stdin().lock())? {
        Ok(answer) => {
            let spill = std::env::temp_dir().join(format!("fragment-api-{}-{:08x}", call.provider, rand::random::<u32>()));
            std::io::stdout().write_all(&deliver(&answer, call.out.as_deref(), &spill)?)?;
            Ok(0)
        }
        Err(refused) => {
            eprintln!("{}", refused.said(&call.provider));
            Ok(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fragment_fakes::http::{Request, Response, Server};
    use std::sync::{Arc, Mutex};

    const TAG: &str = "0123456789abcdef0123456789abcdef";

    fn vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect()
    }

    fn held() -> Vec<(String, String)> {
        vars(&[("PATH", "/bin"), ("LINEAR_OAUTH_ACCESS_TOKEN", &format!("fcx_linear_{TAG}")), ("GOOGLE_PLACES_API_KEY", &format!("fck_google-places_{TAG}"))])
    }

    /// What a provider was asked: the parts a test reads.
    struct Seen {
        method: String,
        path: String,
        query: std::collections::HashMap<String, String>,
        headers: Vec<(&'static str, Option<String>)>,
        body: Vec<u8>,
    }

    impl Seen {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers.iter().find(|(n, _)| *n == name).and_then(|(_, v)| v.as_deref())
        }
    }

    /// A provider that keeps what it was asked and answers as `answer` says.
    fn provider(answer: impl Fn(&Request) -> Response + Send + Sync + 'static) -> (Server, Arc<Mutex<Vec<Seen>>>) {
        let seen = Arc::new(Mutex::new(vec![]));
        let kept = Arc::clone(&seen);
        let server = Server::start(
            0,
            Arc::new(move |r: &Request| {
                let headers = ["authorization", "content-type", "x-version", "accept"].map(|n| (n, r.header(n).map(str::to_string))).to_vec();
                kept.lock().unwrap().push(Seen { method: r.method.clone(), path: r.path.clone(), query: r.query.clone(), headers, body: r.body.clone() });
                answer(r)
            }),
        )
        .unwrap();
        (server, seen)
    }

    fn call(url: &str) -> Call {
        Call { provider: "linear".into(), method: "GET".into(), url: url.into(), ..Call::default() }
    }

    fn asked(c: &Call) -> Result<Answer, Refused> {
        send(c, &held(), &mut std::io::empty()).unwrap()
    }

    /// The token is the provider's own placeholder, found by its shape: a
    /// variable of another provider's, or one that only begins like it, is
    /// never sent; none is `not_connected`; `--env` names any variable.
    #[test]
    fn the_token_is_the_providers_placeholder() {
        assert_eq!(token("linear", None, &held()), Ok((format!("fcx_linear_{TAG}"), true)));
        assert_eq!(token("google-places", None, &held()), Ok((format!("fck_google-places_{TAG}"), true)), "a key's, and a name with a hyphen");
        for other in ["google", "line", "linear-app"] {
            assert_eq!(token(other, None, &held()).unwrap_err().code, "not_connected", "{other}");
        }
        let near = vars(&[("A", "fcx_linear_"), ("B", "fcx_linear_nothex"), ("C", "xfcx_linear_00")]);
        assert_eq!(token("linear", None, &near).unwrap_err().code, "not_connected");
        let own = vars(&[("MY_TOKEN", " lin_oauth_abc \n")]);
        assert_eq!(token("linear", Some("MY_TOKEN"), &own), Ok(("lin_oauth_abc".into(), false)), "any token, trimmed");
        assert_eq!(token("linear", Some("NOPE"), &own).unwrap_err().message, "NOPE is not set");
    }

    /// A call is sent as asked: the placeholder as a bearer token, its
    /// parameters encoded into the query, its JSON body and headers as
    /// written, and the answer's body printed as it came.
    #[test]
    fn a_call_reaches_the_provider_as_the_agent() {
        let (server, seen) = provider(|_| Response::json(200, &json!({ "ok": true })));
        let mut c = call(&format!("{}/v1/issues?team=ENG", server.url));
        c.method = "post".into();
        c.params = vec!["q=SELECT Id FROM Account".into(), "n=5".into()];
        c.body = Some(r#"{"title": "A"}"#.into());
        c.headers = vec!["X-Version: 2".into()];
        let answer = asked(&c).unwrap();
        assert_eq!((answer.status, answer.body.as_slice()), (200, br#"{"ok":true}"#.as_slice()));
        let seen = seen.lock().unwrap();
        let r = &seen[0];
        assert_eq!((r.method.as_str(), r.path.as_str()), ("POST", "/v1/issues"));
        assert_eq!(r.header("authorization"), Some(format!("Bearer fcx_linear_{TAG}").as_str()));
        assert_eq!((r.header("content-type"), r.header("x-version"), r.header("accept")), (Some("application/json"), Some("2"), Some("application/json")));
        assert_eq!((r.query.get("team").map(String::as_str), r.query.get("q").map(String::as_str), r.query.get("n").map(String::as_str)), (Some("ENG"), Some("SELECT Id FROM Account"), Some("5")));
        assert_eq!(r.body, br#"{"title": "A"}"#);
        assert_eq!(deliver(&answer, None, Path::new("/nonexistent/never")).unwrap(), b"{\"ok\":true}\n");
    }

    /// A GraphQL document is a POST of `{query, variables}`, and an answer
    /// of 200 that names errors is a refusal, in the provider's words.
    #[test]
    fn a_graphql_document_is_sent_and_its_errors_are_a_refusal() {
        let (server, seen) = provider(|r| match String::from_utf8_lossy(&r.body).contains("viewer") {
            true => Response::json(200, &json!({ "data": { "viewer": { "id": "u1" } } })),
            false => Response::json(200, &json!({ "errors": [{ "message": "Cannot query field", "extensions": { "code": "GRAPHQL_VALIDATION_FAILED", "userPresentableMessage": "No such field." } }] })),
        });
        let mut c = call(&format!("{}/graphql", server.url));
        c.method = "POST".into();
        c.query = Some("query($n: Int) { viewer { id } }".into());
        c.variables = Some(r#"{"n": 1}"#.into());
        assert_eq!(asked(&c).unwrap().status, 200);
        let sent: Value = serde_json::from_slice(&seen.lock().unwrap()[0].body).unwrap();
        assert_eq!(sent, json!({ "query": "query($n: Int) { viewer { id } }", "variables": { "n": 1 } }));
        c.query = Some("query { nope }".into());
        assert_eq!(asked(&c).unwrap_err(), Refused::new(200, "graphql_validation_failed", "No such field."));
        c.method = "GET".into();
        assert!(send(&c, &held(), &mut std::io::empty()).is_err(), "a query is a POST");
    }

    /// A redirect is answered and never followed: the token reaches the
    /// first host alone.
    #[test]
    fn a_redirect_is_never_followed() {
        let (elsewhere, seen_elsewhere) = provider(|_| Response::json(200, &json!({})));
        let to = format!("{}/taken", elsewhere.url);
        let (server, _) = provider(move |_| Response::bytes(302, "text/plain", vec![]).with_header("location", &to));
        let refused = asked(&call(&format!("{}/file", server.url))).unwrap_err();
        assert_eq!((refused.status, refused.code.as_str()), (302, "redirected"));
        assert!(refused.message.contains("/taken"), "{refused:?}");
        assert!(seen_elsewhere.lock().unwrap().is_empty(), "the token went nowhere else");
    }

    /// Each refusal is said one way: the computer's own with what to tell
    /// the owner, and a provider's in its own code and words.
    #[test]
    fn a_refusal_is_said_one_way() {
        for (status, body, code, message) in [
            (401, json!({ "error": "not_connected", "message": "linear is not connected" }), "not_connected", "linear is not connected"),
            (403, json!({ "error": "forbidden", "message": "read only" }), "forbidden", "read only"),
            (400, json!({ "error": "invalid_grant", "error_description": "expired" }), "invalid_grant", "expired"),
            (400, json!([{ "errorCode": "MALFORMED_QUERY", "message": "unexpected token" }]), "malformed_query", "unexpected token"),
            (404, json!({ "error": { "code": "itemNotFound", "message": "No such item" } }), "itemnotfound", "No such item"),
            (403, json!({ "error": { "code": 403, "status": "PERMISSION_DENIED", "message": "Denied" } }), "permission_denied", "Denied"),
            (403, json!({ "status": "error", "message": "Scopes are required.", "category": "MISSING_SCOPES" }), "missing_scopes", "Scopes are required."),
            (409, json!({ "error_summary": "path/not_found/..", "error": { ".tag": "path" } }), "path", "path/not_found/.."),
            (404, json!({ "errorMessages": ["Issue does not exist"], "errors": {} }), "", "Issue does not exist"),
            (500, json!({ "error": { "code": 500 } }), "500", "the provider refused the request"),
            (422, json!({ "message": "Validation Failed" }), "", "Validation Failed"),
            (400, json!({ "errors": [{ "message": "Bad", "extensions": { "code": "INVALID_INPUT" } }] }), "invalid_input", "Bad"),
        ] {
            let said = body.to_string();
            let (server, _) = provider(move |_| Response::bytes(status, "application/json", said.clone().into_bytes()));
            assert_eq!(asked(&call(&format!("{}/x", server.url))).unwrap_err(), Refused::new(status, code, message), "{body}");
        }
        assert_eq!(refusal(502, "<html>Bad Gateway</html>").message, "<html>Bad Gateway</html>");
        assert_eq!(refusal(500, "").message, "HTTP 500");
        let said = Refused::new(401, "not_connected", "x").said("linear");
        assert!(said["hint"].as_str().unwrap().contains("Settings, under Connections"), "{said}");
        assert!(Refused::new(401, "", "x").said("linear")["hint"].as_str().unwrap().contains("reconnect linear"));
        assert_eq!(Refused::new(404, "not_found", "x").said("linear")["hint"], "");
        // a refusal with no body: its code from the challenge (RFC 6750), and how long to wait
        let (server, seen) = provider(|r| match r.path.as_str() {
            "/scope" => Response::bytes(403, "text/plain", vec![]).with_header("www-authenticate", "Bearer realm=\"api\", error=\"insufficient_scope\", error_description=\"needs root_readwrite\""),
            "/slow" => Response::json(429, &json!({ "message": "Too many" })).with_header("retry-after", "30"),
            _ => Response::bytes(200, "application/pdf", vec![1]),
        });
        let scope = asked(&call(&format!("{}/scope", server.url))).unwrap_err();
        assert_eq!(scope, Refused::new(403, "insufficient_scope", "needs root_readwrite"));
        assert!(scope.said("box")["hint"].as_str().unwrap().contains("read only"));
        assert_eq!(asked(&call(&format!("{}/slow", server.url))).unwrap_err().message, "Too many (Retry-After: 30)");
        // JSON is asked for, unless the call says what it takes; and a call may take only so long
        let download = Call { headers: vec!["Accept: */*".into()], ..call(&format!("{}/file", server.url)) };
        asked(&download).unwrap();
        assert_eq!(seen.lock().unwrap().last().unwrap().header("accept"), Some("*/*"));
        for timeout in [0, TIMEOUT_MAX_S + 1] {
            assert!(send(&Call { timeout: Some(timeout), ..call("https://api.example/x") }, &held(), &mut std::io::empty()).is_err());
        }
    }

    /// Not connected reaches no one; a token that is no placeholder goes
    /// over HTTPS alone; and a call written wrongly is the caller's mistake.
    #[test]
    fn what_is_never_sent() {
        let (server, seen) = provider(|_| Response::json(200, &json!({})));
        let mut c = call(&format!("{}/x", server.url));
        c.provider = "salesforce".into();
        assert_eq!(asked(&c).unwrap_err().code, "not_connected");
        let own = vars(&[("T", "real-token")]);
        c.env = Some("T".into());
        assert!(send(&c, &own, &mut std::io::empty()).unwrap_err().to_string().contains("https alone"));
        for wrong in [
            Call { method: "FETCH".into(), ..call("https://api.example/x") },
            Call { headers: vec!["Authorization: Bearer mine".into()], ..call("https://api.example/x") },
            Call { body: Some("not json".into()), ..call("https://api.example/x") },
            Call { params: vec!["novalue".into()], ..call("https://api.example/x") },
            Call { variables: Some("{}".into()), ..call("https://api.example/x") },
            call("https://user:pass@api.example/x"),
            call("api.example/x"),
        ] {
            assert!(send(&wrong, &held(), &mut std::io::empty()).is_err(), "{wrong:?}");
        }
        assert!(seen.lock().unwrap().is_empty(), "none of them reached the provider");
        let text = Call { body: Some("a,b\n1,2\n".into()), content_type: Some("text/csv".into()), ..call("https://api.example/x") };
        assert_eq!(body(&text, &mut std::io::empty()).unwrap(), Some((b"a,b\n1,2\n".to_vec(), "text/csv".into())), "another kind of body, as written");
        let piped = Call { body: Some("-".into()), ..call("https://api.example/x") };
        assert_eq!(body(&piped, &mut &b"{\"a\":1}"[..]).unwrap().unwrap().0, b"{\"a\":1}");
    }

    /// An answer is saved whole or not at all: `--out` writes the file and
    /// names it; one too long to print is saved beside, and said; and one
    /// cut short of its stated length keeps nothing.
    #[test]
    fn an_answer_is_saved_whole_or_not_at_all() {
        let dir = std::env::temp_dir().join(format!("fragment-api-test-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        let (server, _) = provider(|r| match r.path.as_str() {
            "/file" => Response::bytes(200, "application/pdf", vec![7u8; 1000]),
            _ => Response::bytes(200, "application/json", vec![b'1'; PRINT_MAX_BYTES + 1]),
        });
        let answer = asked(&call(&format!("{}/file", server.url))).unwrap();
        let out = dir.join("report.pdf");
        let said: Value = serde_json::from_slice(&deliver(&answer, Some(&out), &dir.join("spill")).unwrap()).unwrap();
        assert_eq!(said, json!({ "status": 200, "saved": out.display().to_string(), "bytes": 1000, "contentType": "application/pdf" }));
        assert_eq!(std::fs::read(&out).unwrap(), vec![7u8; 1000]);
        assert!(!dir.join(".report.pdf.part").exists(), "nothing left beside it");
        let long = asked(&call(&format!("{}/long", server.url))).unwrap();
        let said: Value = serde_json::from_slice(&deliver(&long, None, &dir.join("spill")).unwrap()).unwrap();
        assert_eq!((said["saved"].as_str(), said["bytes"].as_u64()), (dir.join("spill").to_str(), Some(PRINT_MAX_BYTES as u64 + 1)));
        assert!(said["note"].as_str().unwrap().contains("too long to print"));
        assert!(deliver(&answer, Some(&dir.join("no-such-dir/x")), &dir.join("spill")).is_err(), "a file that can't be written is said");

        // a body that stops short of its content-length
        use std::io::Read as _;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/cut", listener.local_addr().unwrap());
        let served = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut asked = [0u8; 2048];
            let _ = conn.read(&mut asked).unwrap();
            conn.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 1000\r\nconnection: close\r\n\r\nonly this").unwrap();
        });
        let cut = asked(&call(&url)).unwrap_err();
        assert_eq!(cut.code, "incomplete", "{cut:?}");
        served.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
