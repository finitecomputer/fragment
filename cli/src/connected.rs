//! `fragment api`: one call to a connected service's API, as the agent.
//!
//! On a computer an agent holds no credential: each connection it may use
//! is a placeholder in an environment variable (`fcx_<provider>_<tag>`,
//! docs/computers.md, "Connections and operator keys"), and the computer's
//! intercept swaps in a short-lived token on the way to that provider's own
//! hosts. This command is the one caller every connection's skill shares
//! (FIN-509): it finds the provider's placeholder, sends it as a bearer
//! token, prints the answer or saves it whole, and says a refusal one way,
//! with what the provider answered and what to tell the owner. A skill is
//! then its provider's paths and quirks, and no code.
//!
//! It knows no provider: a name, a URL and a body are all it is given, and
//! what a provider answers is passed on as it came, read by no rule of ours.

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
/// file named, so one call never fills an agent's context (64 KiB of JSON
/// is some sixteen thousand tokens).
pub const PRINT_MAX_BYTES: usize = 64 << 10;
/// The most of an answer taken at all. It goes to its file as it comes, so
/// this bounds the disk, never memory.
const ANSWER_MAX_BYTES: u64 = 512 << 20;
/// The most of a GraphQL answer, which is held whole to read its `errors`.
const GRAPHQL_MAX_BYTES: usize = 16 << 20;
/// The most of a refusal's body read, and said back.
const REFUSAL_MAX_BYTES: usize = 16 << 10;
/// The most redirects one call follows.
const REDIRECTS_MAX: usize = 10;

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
    /// What the provider answered, as it came: its JSON, or its text. None
    /// where it said nothing, or the refusal is this command's own.
    pub body: Option<Value>,
}

impl Refused {
    fn new(status: u16, code: &str, message: impl Into<String>) -> Refused {
        Refused { status, code: code.into(), message: message.into(), body: None }
    }

    /// The line an agent reads: the refusal, what to do about it, and what
    /// the provider answered.
    pub fn said(&self, provider: &str) -> Value {
        let mut said = json!({ "error": self.code, "status": self.status, "message": self.message, "hint": hint(&self.code, self.status, provider) });
        if let Some(body) = &self.body {
            said["body"] = body.clone();
        }
        said
    }
}

/// What to do about a refusal: the computer's own (`not_connected`,
/// `forbidden`), or the provider's by its status.
fn hint(code: &str, status: u16, provider: &str) -> String {
    match (code, status) {
        ("not_connected", _) => format!("your owner has not connected {provider} (or must again), or left it out of what you may use: ask them to connect it in Settings, under Connections"),
        ("forbidden", _) => format!("your owner narrowed your connections and left {provider} out, connected it read only (a change refused while reads work: they allow changes in Settings, under Connections), or the call went to a host that is not {provider}'s"),
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

/// A refusal, from its answer's body. Its code is the body's own `error`
/// where that is a word, as the computer's is (`{error: "not_connected" |
/// "forbidden", message}`) and OAuth's (`{error, error_description}`, RFC
/// 6749); its words are the body's `message` or `error_description`, else
/// its status. No provider's own shape is read for either: the body is
/// passed on as it came (its JSON, else its text, cut at
/// `REFUSAL_MAX_BYTES`), for the agent to read.
pub fn refusal(status: u16, text: &str) -> Refused {
    let parsed: Option<Value> = serde_json::from_str(text).ok();
    let word = |key: &str| parsed.as_ref().and_then(|v| v.get(key)).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
    let code = word("error").unwrap_or_default();
    let message = word("message").or_else(|| word("error_description")).unwrap_or_else(|| format!("HTTP {status}"));
    let body = if text.trim().is_empty() {
        None
    } else if text.len() > REFUSAL_MAX_BYTES {
        let mut end = REFUSAL_MAX_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        Some(Value::String(format!("{}… (cut: {} bytes in all)", &text[..end], text.len())))
    } else {
        Some(parsed.unwrap_or_else(|| Value::String(text.to_string())))
    };
    Refused { status, code, message, body }
}

/// A good answer, and where its body went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Body,
}

/// An answer's body: short enough to print, or saved whole to a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    Printed(Vec<u8>),
    /// `--out`'s file, or (`spilled`) one made for an answer too long to
    /// print.
    Saved { path: PathBuf, bytes: u64, spilled: bool },
}

/// What a body that is not as long as it stated (or is over the most taken)
/// says.
fn short(bytes: u64, stated: Option<u64>) -> String {
    format!("the answer is {bytes} bytes of the {} it states (or over {ANSWER_MAX_BYTES}): nothing was kept", stated.map(|n| n.to_string()).unwrap_or_else(|| "?".into()))
}

/// Sends the call and takes its answer whole. A good answer short enough
/// to print is held; a longer one, or one a file was asked for, goes to its
/// file as it comes (`save`), held nowhere. A body cut short of its stated
/// length is no answer (`incomplete`), so half a file is never taken for
/// one. A redirect is followed, and the token stays with the host it was
/// sent to. `spill` is where an answer too long to print is saved.
pub fn send(call: &Call, vars: &[(String, String)], stdin: &mut dyn Read, spill: &Path) -> Result<Result<Answer, Refused>> {
    let method = reqwest::Method::from_bytes(call.method.to_ascii_uppercase().as_bytes()).ok().filter(|m| ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"].contains(&m.as_str())).ok_or_else(|| anyhow!("{:?} is no method: GET, POST, PUT, PATCH, DELETE or HEAD", call.method))?;
    let graphql = call.query.is_some();
    if graphql && method != reqwest::Method::POST {
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
    // A redirect is followed, and the token is not sent on: reqwest drops
    // Authorization when a redirect leaves the host or the port
    // (redirect.rs, `remove_sensitive_headers`). So a download that answers
    // a signed address on a storage host is fetched there without it, and a
    // placeholder is worth nothing off this computer's egress in any case.
    let client = reqwest::blocking::Client::builder().user_agent(concat!("fragment-cli/", env!("CARGO_PKG_VERSION"))).redirect(reqwest::redirect::Policy::limited(REDIRECTS_MAX)).timeout(std::time::Duration::from_secs(timeout)).build()?;
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
        Err(e) => {
            let code = if e.is_timeout() {
                "timed_out"
            } else if e.is_redirect() {
                "redirected"
            } else {
                "unreachable"
            };
            return Ok(Err(Refused::new(0, code, format!("{}", e.without_url()))));
        }
    };
    let status = response.status().as_u16();
    // a refusal's code where its body has none (RFC 6750: `WWW-Authenticate:
    // Bearer error="insufficient_scope"`), and how long to wait
    let header = |name: &str| response.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
    let challenge = |key: &str| header("www-authenticate").and_then(|c| c.split_once(&format!("{key}=\"")).and_then(|(_, rest)| rest.split_once('"')).map(|(v, _)| v.to_string()));
    let (challenged, described, retry_after) = (challenge("error"), challenge("error_description"), header("retry-after"));
    let stated = response.content_length();
    let content_type = header("content-type");
    let mut rest = response.take(ANSWER_MAX_BYTES + 1);
    if !(200..300).contains(&status) {
        // a refusal is short: no more of it is read than is said back
        let mut bytes = vec![];
        let _ = rest.by_ref().take(REFUSAL_MAX_BYTES as u64 + 1).read_to_end(&mut bytes);
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
    // What is held: enough to print, or a GraphQL answer whole, to read its
    // errors. A file asked for is written as it comes, nothing of it held.
    let hold = match (graphql, &call.out) {
        (true, _) => GRAPHQL_MAX_BYTES,
        (false, Some(_)) => 0,
        (false, None) => PRINT_MAX_BYTES,
    };
    let mut held = vec![];
    if let Err(e) = rest.by_ref().take(hold as u64 + 1).read_to_end(&mut held) {
        return Ok(Err(Refused::new(status, "incomplete", format!("the answer stopped after {} bytes: {e}", held.len()))));
    }
    // the body ended within what is held
    let whole = held.len() <= hold;
    if graphql {
        if !whole {
            return Ok(Err(Refused::new(status, "too_long", format!("a GraphQL answer over {GRAPHQL_MAX_BYTES} bytes: ask for fewer fields, or fewer at a time"))));
        }
        // GraphQL answers 200 with what it refused: `errors`, each with a
        // `message` (the GraphQL specification, 7.1.2)
        if let Ok(v) = serde_json::from_slice::<Value>(&held) {
            if let Some(first) = v.get("errors").and_then(Value::as_array).and_then(|e| e.first()) {
                let message = first.get("message").and_then(Value::as_str).unwrap_or("the answer names errors").to_string();
                return Ok(Err(Refused { status, code: "graphql_errors".into(), message, body: refusal(status, &String::from_utf8_lossy(&held)).body }));
            }
        }
    }
    let to = match (&call.out, whole && held.len() <= PRINT_MAX_BYTES) {
        (Some(out), _) => Some((out.as_path(), false)),
        (None, false) => Some((spill, true)),
        (None, true) => None,
    };
    let body = match to {
        None if stated.is_some_and(|n| n != held.len() as u64) => return Ok(Err(Refused::new(status, "incomplete", short(held.len() as u64, stated)))),
        None => Body::Printed(held),
        Some((path, spilled)) => match save(path, &held, &mut rest, stated)? {
            Ok(bytes) => Body::Saved { path: path.to_path_buf(), bytes, spilled },
            Err(why) => return Ok(Err(Refused::new(status, "incomplete", why))),
        },
    };
    Ok(Ok(Answer { status, content_type, body }))
}

/// Saves an answer to `path` whole or not at all: `head` (what is already
/// read of it), then all `rest` gives, written beside `path` first and
/// renamed over it only when the body ended as long as it stated. So a file
/// already at `path` is kept unless the whole answer came, and half a file
/// is never left for one. Its bytes; or, where the body was cut short, why.
/// An error is the file's own (a folder that is not there): the caller's
/// mistake.
fn save(path: &Path, head: &[u8], rest: &mut dyn Read, stated: Option<u64>) -> Result<Result<u64, String>> {
    let name = path.file_name().ok_or_else(|| anyhow!("--out names a file, not {}", path.display()))?;
    let part = path.with_file_name(format!(".{}.part", name.to_string_lossy()));
    let failed = |e: std::io::Error| anyhow!("{}: {e}", path.display());
    let written = (|| -> Result<Result<u64, String>> {
        let mut file = std::fs::File::create(&part).map_err(failed)?;
        file.write_all(head).map_err(failed)?;
        let mut bytes = head.len() as u64;
        let mut buffer = vec![0u8; 64 << 10];
        loop {
            match rest.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    file.write_all(&buffer[..n]).map_err(failed)?;
                    bytes += n as u64;
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Ok(Err(format!("the answer stopped after {bytes} bytes: {e}"))),
            }
        }
        if bytes > ANSWER_MAX_BYTES || stated.is_some_and(|n| n != bytes) {
            return Ok(Err(short(bytes, stated)));
        }
        file.sync_all().map_err(failed)?;
        std::fs::rename(&part, path).map_err(failed)?;
        Ok(Ok(bytes))
    })();
    if !matches!(written, Ok(Ok(_))) {
        let _ = std::fs::remove_file(&part);
    }
    written
}

/// What a good answer says on stdout: its body, or, saved, one line naming
/// the file.
pub fn deliver(answer: &Answer) -> Vec<u8> {
    match &answer.body {
        Body::Printed(body) => {
            let mut body = body.clone();
            if !body.is_empty() && !body.ends_with(b"\n") {
                body.push(b'\n');
            }
            body
        }
        Body::Saved { path, bytes, spilled } => {
            let mut line = json!({ "status": answer.status, "saved": path.display().to_string(), "bytes": bytes, "contentType": answer.content_type });
            if *spilled {
                line["note"] = json!("the answer is too long to print, so it is saved: read the part you need, or ask the provider for less (fields, a page size)");
            }
            format!("{line}\n").into_bytes()
        }
    }
}

/// `fragment api`: the answer on stdout and 0, or a refusal on stderr and 1.
/// A mistake in how it was called is the caller's error (exit 2).
pub fn run(call: Call) -> Result<i32> {
    let vars: Vec<(String, String)> = std::env::vars_os().filter_map(|(n, v)| Some((n.into_string().ok()?, v.into_string().ok()?))).collect();
    let spill = std::env::temp_dir().join(format!("fragment-api-{:08x}", rand::random::<u32>()));
    match send(&call, &vars, &mut std::io::stdin().lock(), &spill)? {
        Ok(answer) => {
            std::io::stdout().write_all(&deliver(&answer))?;
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

    /// Asked with nowhere to spill: an answer too long to print is a test's
    /// own to ask for (`send`, with a file).
    fn asked(c: &Call) -> Result<Answer, Refused> {
        send(c, &held(), &mut std::io::empty(), Path::new("/nonexistent/fragment-api/never")).unwrap()
    }

    fn printed(answer: &Answer) -> &[u8] {
        match &answer.body {
            Body::Printed(body) => body,
            saved => panic!("printed, not {saved:?}"),
        }
    }

    /// A folder of one test's own.
    fn folder(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fragment-api-test-{what}-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
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
        assert_eq!((answer.status, printed(&answer)), (200, br#"{"ok":true}"#.as_slice()));
        let seen = seen.lock().unwrap();
        let r = &seen[0];
        assert_eq!((r.method.as_str(), r.path.as_str()), ("POST", "/v1/issues"));
        assert_eq!(r.header("authorization"), Some(format!("Bearer fcx_linear_{TAG}").as_str()));
        assert_eq!((r.header("content-type"), r.header("x-version"), r.header("accept")), (Some("application/json"), Some("2"), Some("application/json")));
        assert_eq!((r.query.get("team").map(String::as_str), r.query.get("q").map(String::as_str), r.query.get("n").map(String::as_str)), (Some("ENG"), Some("SELECT Id FROM Account"), Some("5")));
        assert_eq!(r.body, br#"{"title": "A"}"#);
        assert_eq!(deliver(&answer), b"{\"ok\":true}\n");
    }

    /// A GraphQL document is a POST of `{query, variables}`, and an answer
    /// of 200 that names errors is a refusal: its first error's message,
    /// and the whole answer as the provider wrote it.
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
        let refused = asked(&c).unwrap_err();
        assert_eq!((refused.status, refused.code.as_str(), refused.message.as_str()), (200, "graphql_errors", "Cannot query field"));
        let body = refused.body.expect("the provider's answer, as it came");
        assert_eq!((&body["errors"][0]["extensions"]["code"], &body["errors"][0]["extensions"]["userPresentableMessage"]), (&json!("GRAPHQL_VALIDATION_FAILED"), &json!("No such field.")));
        c.method = "GET".into();
        assert!(send(&c, &held(), &mut std::io::empty(), Path::new("/nonexistent/never")).is_err(), "a query is a POST");
    }

    /// A redirect is followed, and the token stays with the host it was
    /// sent to: a download's signed address on another host is fetched
    /// without it, and saved whole; the same host is asked with it.
    #[test]
    fn a_redirect_is_followed_and_the_token_stays_with_its_host() {
        let (elsewhere, seen_elsewhere) = provider(|_| Response::bytes(200, "application/pdf", vec![7u8; 1000]));
        let to = format!("{}/signed?sig=abc", elsewhere.url);
        let (server, seen) = provider(move |r| match r.path.as_str() {
            "/file" => Response::bytes(302, "text/plain", vec![]).with_header("location", &to),
            "/moved" => Response::bytes(301, "text/plain", vec![]).with_header("location", "/here"),
            _ => Response::json(200, &json!({ "here": true })),
        });
        let bearer = format!("Bearer fcx_linear_{TAG}");
        let dir = folder("redirect");
        let out = dir.join("plan.pdf");
        let saved = asked(&Call { out: Some(out.clone()), ..call(&format!("{}/file", server.url)) }).unwrap();
        assert_eq!(saved.body, Body::Saved { path: out.clone(), bytes: 1000, spilled: false });
        assert_eq!(std::fs::read(&out).unwrap(), vec![7u8; 1000], "a download through its redirect, whole");
        assert_eq!(seen.lock().unwrap()[0].header("authorization"), Some(bearer.as_str()));
        {
            let there = seen_elsewhere.lock().unwrap();
            assert_eq!((there.len(), there[0].path.as_str(), there[0].header("authorization")), (1, "/signed", None), "the token went to no other host");
        }
        assert_eq!(printed(&asked(&call(&format!("{}/moved", server.url))).unwrap()), br#"{"here":true}"#.as_slice());
        let seen = seen.lock().unwrap();
        assert_eq!((seen[2].path.as_str(), seen[2].header("authorization")), ("/here", Some(bearer.as_str())), "its own host is asked with it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Each refusal is said one way. The computer's own and OAuth's are
    /// known by their `error`, and say what to tell the owner. Whatever
    /// else a provider answers is read by no rule of ours: no code is
    /// guessed from its shape, and its body is passed on whole.
    #[test]
    fn a_refusal_is_said_one_way_and_a_providers_answer_is_passed_on() {
        for (status, body, code, message) in [
            (403, json!({ "error": "not_connected", "message": "linear is not connected" }), "not_connected", "linear is not connected"),
            (403, json!({ "error": "forbidden", "message": "read only" }), "forbidden", "read only"),
            (400, json!({ "error": "invalid_grant", "error_description": "expired" }), "invalid_grant", "expired"),
            (422, json!({ "message": "Validation Failed", "errors": [{ "resource": "Issue", "field": "title", "code": "missing_field" }] }), "", "Validation Failed"),
            (400, json!([{ "errorCode": "MALFORMED_QUERY", "message": "unexpected token" }]), "", "HTTP 400"),
            (404, json!({ "error": { "code": "itemNotFound", "message": "No such item" } }), "", "HTTP 404"),
            (409, json!({ "error_summary": "path/not_found/..", "error": { ".tag": "path" } }), "", "HTTP 409"),
        ] {
            let said = body.to_string();
            let (server, _) = provider(move |_| Response::bytes(status, "application/json", said.clone().into_bytes()));
            let refused = asked(&call(&format!("{}/x", server.url))).unwrap_err();
            assert_eq!(refused, Refused { status, code: code.into(), message: message.into(), body: Some(body.clone()) }, "{body}");
        }
        // what the agent reads of a provider's refusal: all of it
        let github = refusal(422, r#"{"message": "Validation Failed", "errors": [{"resource": "Issue", "field": "title", "code": "missing_field"}]}"#).said("github");
        assert_eq!((&github["message"], &github["body"]["errors"][0]["field"], &github["error"]), (&json!("Validation Failed"), &json!("title"), &json!("")));
        // no JSON: its text; nothing: its status alone; too long to say back: cut, on a character
        assert_eq!(refusal(502, "<html>Bad Gateway</html>"), Refused { status: 502, code: String::new(), message: "HTTP 502".into(), body: Some(json!("<html>Bad Gateway</html>")) });
        assert_eq!(refusal(500, ""), Refused::new(500, "", "HTTP 500"));
        let long = refusal(500, &format!("a{}", "é".repeat(REFUSAL_MAX_BYTES)));
        assert!(long.body.as_ref().and_then(Value::as_str).is_some_and(|b| b.len() < REFUSAL_MAX_BYTES + 64 && b.ends_with("bytes in all)")), "{long:?}");
        let said = Refused::new(403, "not_connected", "x").said("linear");
        assert!(said["hint"].as_str().unwrap().contains("Settings, under Connections"), "{said}");
        assert!(said.get("body").is_none(), "no body where the provider said nothing: {said}");
        assert!(Refused::new(401, "", "x").said("linear")["hint"].as_str().unwrap().contains("reconnect linear"));
        assert_eq!(Refused::new(404, "", "x").said("linear")["hint"], "");
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
            assert!(send(&Call { timeout: Some(timeout), ..call("https://api.example/x") }, &held(), &mut std::io::empty(), Path::new("/nonexistent/never")).is_err());
        }
    }

    /// Not connected reaches no one; a token that is no placeholder goes
    /// over HTTPS alone; and a call written wrongly is the caller's mistake.
    #[test]
    fn what_is_never_sent() {
        let nowhere = Path::new("/nonexistent/never");
        let (server, seen) = provider(|_| Response::json(200, &json!({})));
        let mut c = call(&format!("{}/x", server.url));
        c.provider = "salesforce".into();
        assert_eq!(asked(&c).unwrap_err().code, "not_connected");
        let own = vars(&[("T", "real-token")]);
        c.env = Some("T".into());
        assert!(send(&c, &own, &mut std::io::empty(), nowhere).unwrap_err().to_string().contains("https alone"));
        for wrong in [
            Call { method: "FETCH".into(), ..call("https://api.example/x") },
            Call { headers: vec!["Authorization: Bearer mine".into()], ..call("https://api.example/x") },
            Call { body: Some("not json".into()), ..call("https://api.example/x") },
            Call { params: vec!["novalue".into()], ..call("https://api.example/x") },
            Call { variables: Some("{}".into()), ..call("https://api.example/x") },
            call("https://user:pass@api.example/x"),
            call("api.example/x"),
        ] {
            assert!(send(&wrong, &held(), &mut std::io::empty(), nowhere).is_err(), "{wrong:?}");
        }
        assert!(seen.lock().unwrap().is_empty(), "none of them reached the provider");
        let text = Call { body: Some("a,b\n1,2\n".into()), content_type: Some("text/csv".into()), ..call("https://api.example/x") };
        assert_eq!(body(&text, &mut std::io::empty()).unwrap(), Some((b"a,b\n1,2\n".to_vec(), "text/csv".into())), "another kind of body, as written");
        let piped = Call { body: Some("-".into()), ..call("https://api.example/x") };
        assert_eq!(body(&piped, &mut &b"{\"a\":1}"[..]).unwrap().unwrap().0, b"{\"a\":1}");
    }

    /// An answer is saved whole or not at all: `--out` writes the file as
    /// the answer comes and names it; one too long to print is saved
    /// beside, and said; and one cut short of its stated length keeps
    /// nothing, and leaves a file already there as it was.
    #[test]
    fn an_answer_is_saved_whole_or_not_at_all() {
        let dir = folder("saved");
        let (server, _) = provider(|r| match r.path.as_str() {
            "/file" => Response::bytes(200, "application/pdf", vec![7u8; 1000]),
            "/empty" => Response::bytes(200, "text/plain", vec![]),
            "/most" => Response::bytes(200, "application/json", vec![b'1'; PRINT_MAX_BYTES]),
            _ => Response::bytes(200, "application/json", vec![b'1'; PRINT_MAX_BYTES + 1]),
        });
        let out = dir.join("report.pdf");
        let answer = asked(&Call { out: Some(out.clone()), ..call(&format!("{}/file", server.url)) }).unwrap();
        let said: Value = serde_json::from_slice(&deliver(&answer)).unwrap();
        assert_eq!(said, json!({ "status": 200, "saved": out.display().to_string(), "bytes": 1000, "contentType": "application/pdf" }));
        assert_eq!(std::fs::read(&out).unwrap(), vec![7u8; 1000]);
        assert!(!dir.join(".report.pdf.part").exists(), "nothing left beside it");
        // the most printed is printed; a byte more is saved beside, and said
        assert_eq!(printed(&asked(&call(&format!("{}/most", server.url))).unwrap()).len(), PRINT_MAX_BYTES);
        let spill = dir.join("spill");
        let long = send(&call(&format!("{}/long", server.url)), &held(), &mut std::io::empty(), &spill).unwrap().unwrap();
        let said: Value = serde_json::from_slice(&deliver(&long)).unwrap();
        assert_eq!((said["saved"].as_str(), said["bytes"].as_u64()), (spill.to_str(), Some(PRINT_MAX_BYTES as u64 + 1)));
        assert!(said["note"].as_str().unwrap().contains("too long to print"));
        assert_eq!(std::fs::read(&spill).unwrap().len(), PRINT_MAX_BYTES + 1);
        // an answer of nothing is an empty file
        let empty = dir.join("empty");
        asked(&Call { out: Some(empty.clone()), ..call(&format!("{}/empty", server.url)) }).unwrap();
        assert_eq!(std::fs::read(&empty).unwrap(), b"");
        // a file that can't be written is the caller's mistake, said
        assert!(send(&Call { out: Some(dir.join("no-such-dir/x")), ..call(&format!("{}/file", server.url)) }, &held(), &mut std::io::empty(), &spill).is_err());

        // a body that stops short of its content-length, to a file and to print
        std::fs::write(&out, b"the one before").unwrap();
        for to in [Some(out.clone()), None] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/cut", listener.local_addr().unwrap());
            let served = std::thread::spawn(move || {
                let (mut conn, _) = listener.accept().unwrap();
                let mut request = [0u8; 2048];
                let _ = conn.read(&mut request).unwrap();
                conn.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 1000\r\nconnection: close\r\n\r\nonly this").unwrap();
            });
            let cut = asked(&Call { out: to, ..call(&url) }).unwrap_err();
            assert_eq!(cut.code, "incomplete", "{cut:?}");
            served.join().unwrap();
        }
        assert_eq!(std::fs::read(&out).unwrap(), b"the one before", "the file already there is kept");
        assert!(!dir.join(".report.pdf.part").exists(), "and nothing is left beside it");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
