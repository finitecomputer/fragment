//! Secrets at the egress point (docs/secrets.md); sealing and opening are
//! `seal.rs`.

use fragment_proto::limits;
use url::Url;

/// A host refused for a secret: not a host as a URL names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotHost(pub String);

impl std::fmt::Display for NotHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?} is not a host as a URL names it (api.example.com: no scheme, port, path, or wildcard)", self.0)
    }
}

/// A host a secret may go to, as a URL names it (`Url::host_str`): a DNS
/// name or an IP literal (`[::1]`), with no scheme, port, path, wildcard,
/// or trailing dot, in lowercase. A host is exact: `api.example.com` is
/// not `example.com`, nor `eu.api.example.com`; any port of it is it.
pub fn secret_host(given: &str) -> Result<String, NotHost> {
    let refused = || NotHost(given.to_string());
    if given.is_empty() || given.len() > limits::SECRET_HOST_MAX_BYTES || given.contains('*') || given.ends_with('.') {
        return Err(refused());
    }
    let lower = given.to_ascii_lowercase();
    let url = Url::parse(&format!("https://{lower}/")).map_err(|_| refused())?;
    // all of it is the host: a port, a path, credentials, a query, or a
    // name the URL rewrote (unicode to punycode) is not
    let whole = url.port().is_none() && url.path() == "/" && url.username().is_empty() && url.password().is_none() && url.query().is_none() && url.fragment().is_none();
    if whole && url.host_str() == Some(lower.as_str()) {
        Ok(lower)
    } else {
        Err(refused())
    }
}

/// Why a placeholder is not filled. No refusal names a host: a secret's
/// hosts are its editors' to know (`GET secrets`), while a run's error is
/// every viewer's, and a hint naming the request's host would invite
/// pasting the very host a job chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unfilled {
    /// The secret names no host: set before secrets named them.
    NoHosts,
    /// The request's host is none of the secret's.
    OtherHost,
    /// One of its hosts, but not over https.
    Cleartext,
}

impl Unfilled {
    pub fn message(self, name: &str) -> String {
        match self {
            Unfilled::NoHosts => format!("secret {name} names no host it may go to; set it again with `fragment secret set <fragment> {name} --to <the API's host>`"),
            Unfilled::OtherHost => format!("secret {name} may not go to this request's host: a secret goes only to the hosts it was set for"),
            Unfilled::Cleartext => format!("secret {name} goes over https only"),
        }
    }
}

/// Whether a secret that may go to `hosts` fills a placeholder in a
/// request to `url`: the request's host is one of them (on any port),
/// over https. A fleet that allows local egress (dev and the e2e) also
/// fills over http toward a local address, where its fakes listen.
pub fn may_fill(hosts: &[String], url: &Url, allow_local: bool) -> Result<(), Unfilled> {
    let host = url.host_str().unwrap_or("").trim_end_matches('.');
    if hosts.is_empty() {
        return Err(Unfilled::NoHosts);
    }
    if !hosts.iter().any(|h| h == host) {
        return Err(Unfilled::OtherHost);
    }
    let https = url.scheme() == "https";
    let local_fake = allow_local && crate::egress::local(url);
    if https || local_fake {
        Ok(())
    } else {
        Err(Unfilled::Cleartext)
    }
}

/// Headers that would send a request somewhere its URL does not say (a
/// proxy's or a virtual host's): a request carrying a secret sets none,
/// so its URL's host, the one `may_fill` checked, is where it goes.
pub const ROUTING_HEADERS: [&str; 3] = ["host", "forwarded", "x-forwarded-host"];

/// The first of `names` that is a routing header (`ROUTING_HEADERS`), in
/// any case.
pub fn routing_header<'a>(names: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    names.into_iter().find(|n| ROUTING_HEADERS.iter().any(|r| n.eq_ignore_ascii_case(r)))
}

/// The secret names a header value refers to as `{{NAME}}` (a job's
/// fetch; the cell substitutes them at its egress point).
pub fn placeholders(v: &str) -> Vec<&str> {
    let mut names = vec![];
    let mut rest = v;
    while let Some(start) = rest.find("{{") {
        let Some(end) = rest[start + 2..].find("}}") else { break };
        names.push(&rest[start + 2..start + 2 + end]);
        rest = &rest[start + 2 + end + 2..];
    }
    names
}

/// A URL's part a secret may fill: its path and query, after its scheme,
/// host and port (`url::Position::BeforePath`) and before its fragment,
/// which is the client's and never sent, with `{{` and
/// `}}` as the parser kept them (raw in a query, `%7B%7B` in a path) read
/// as braces, so a placeholder is found wherever the job wrote it.
fn fillable(url: &Url) -> String {
    let tail = &url[url::Position::BeforePath..url::Position::AfterQuery];
    ["%7B", "%7b"].iter().fold(tail.to_string(), |t, b| t.replace(b, "{")).replace("%7D", "}").replace("%7d", "}")
}

/// The secret names a URL refers to as `{{NAME}}` in its path or query
/// (a job's fetch: `https://api.telegram.org/bot{{TOKEN}}/sendMessage`).
/// Never in its scheme, host or port, nor what is no secret's name
/// (`{{id}}`, a template's, an app's own): those stay as written, as they
/// went before URLs held secrets.
pub fn url_placeholders(url: &Url) -> Vec<String> {
    placeholders(&fillable(url)).into_iter().filter(|n| fragment_proto::valid_secret_name(n)).map(str::to_string).collect()
}

/// A secret's value as it goes into a URL: every byte but the unreserved
/// ones and `:` (a bot token's) percent-encoded, so it is one path
/// segment's or one query value's bytes and nothing more: no `/`, `?`,
/// `#`, `&`, `=` or `%` of its own.
pub fn url_encoded(value: &str) -> String {
    value.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~:".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// Why a URL's placeholders were not filled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlUnfilled {
    /// The value asked for the name could not be had (its message).
    Value(String),
    /// Filled, it would go elsewhere: another scheme, host or port, or
    /// another number of path segments or query parameters (a value of
    /// `..`, say).
    Moved,
}

/// `url` with each `{{NAME}}` in its path and query replaced by
/// `value(NAME)`, `url_encoded`, at the egress point: the request goes to
/// the same scheme, host and port, in as many path segments and query
/// parameters, or it is not made (`Moved`). Host checks (`may_fill`) are
/// the caller's, on `url` before this.
pub fn fill_url(url: &Url, mut value: impl FnMut(&str) -> Result<String, String>) -> Result<Url, UrlUnfilled> {
    let mut tail = fillable(url);
    for name in url_placeholders(url) {
        let v = value(&name).map_err(UrlUnfilled::Value)?;
        tail = tail.replace(&format!("{{{{{name}}}}}"), &url_encoded(&v));
    }
    let filled = Url::parse(&format!("{}{tail}{}", &url[..url::Position::BeforePath], &url[url::Position::AfterQuery..])).map_err(|_| UrlUnfilled::Moved)?;
    let segments = |u: &Url| u.path_segments().map_or(0, Iterator::count);
    let pairs = |u: &Url| u.query_pairs().count();
    let same = filled.scheme() == url.scheme() && filled.host() == url.host() && filled.port_or_known_default() == url.port_or_known_default() && segments(&filled) == segments(url) && pairs(&filled) == pairs(url);
    if !same {
        return Err(UrlUnfilled::Moved);
    }
    Ok(filled)
}

/// `text` with each opened secret (by name: its value) named as its
/// placeholder again, as it went into a URL (`url_encoded`) and as it is:
/// what goes back to a job of a fetch whose URL held secrets (a failure's
/// message, an answer that echoes its URL).
pub fn redact(text: String, opened: &std::collections::BTreeMap<String, String>) -> String {
    opened.iter().fold(text, |t, (name, secret)| t.replace(&url_encoded(secret), &format!("{{{{{name}}}}}")).replace(secret.as_str(), &format!("{{{{{name}}}}}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts(of: &[&str]) -> Vec<String> {
        of.iter().map(|h| h.to_string()).collect()
    }

    fn url(raw: &str) -> Url {
        Url::parse(raw).unwrap()
    }

    /// Goal: a host a secret names is a URL's host and nothing more.
    /// Method: the shapes people type, kept or refused.
    #[test]
    fn a_secret_names_exact_hosts() {
        for (given, kept) in [("api.example.com", "api.example.com"), ("API.Example.com", "api.example.com"), ("1.1.1.1", "1.1.1.1"), ("[2606:4700::1111]", "[2606:4700::1111]"), ("127.0.0.1", "127.0.0.1")] {
            assert_eq!(secret_host(given).as_deref(), Ok(kept), "{given}");
        }
        let long = format!("{}.com", "a".repeat(limits::SECRET_HOST_MAX_BYTES));
        for bad in ["", "https://api.example.com", "api.example.com:8443", "api.example.com/v1", "user@api.example.com", "*.example.com", "api.example.com.", "a b.com", "api.example.com?x", "münchen.de", long.as_str()] {
            assert_eq!(secret_host(bad), Err(NotHost(bad.to_string())), "{bad}");
        }
    }

    /// Goal: a placeholder fills only toward one of its secret's hosts,
    /// over https (the audit's M3), and no refusal names a host. Method:
    /// one secret, the request varied by host, scheme, and whether local
    /// egress is allowed (dev's fakes).
    #[test]
    fn a_secret_fills_toward_its_hosts_over_https_only() {
        let api = hosts(&["api.example.com"]);
        assert_eq!(may_fill(&api, &url("https://api.example.com/v1"), false), Ok(()));
        assert_eq!(may_fill(&api, &url("https://api.example.com:8443/v1"), false), Ok(()), "the host is what is named, on any port");
        assert_eq!(may_fill(&api, &url("https://API.example.com./v1"), false), Ok(()), "a URL's host is lowercase; a trailing dot is the same name");
        for to in ["https://evil.example/collect", "https://example.com/", "https://eu.api.example.com/"] {
            assert_eq!(may_fill(&api, &url(to), false), Err(Unfilled::OtherHost), "{to}");
        }
        assert_eq!(may_fill(&api, &url("http://api.example.com/v1"), false), Err(Unfilled::Cleartext));
        assert_eq!(may_fill(&api, &url("http://api.example.com/v1"), true), Err(Unfilled::Cleartext), "local egress is no reason to send a public host cleartext");
        let fake = hosts(&["127.0.0.1"]);
        assert_eq!(may_fill(&fake, &url("http://127.0.0.1:9999/data"), true), Ok(()), "dev and the e2e fill toward their fakes");
        assert_eq!(may_fill(&fake, &url("http://127.0.0.1:9999/data"), false), Err(Unfilled::Cleartext));
        assert_eq!(may_fill(&[], &url("https://api.example.com/"), false), Err(Unfilled::NoHosts));
        for why in [Unfilled::NoHosts, Unfilled::OtherHost, Unfilled::Cleartext] {
            let said = why.message("K");
            assert!(said.contains("secret K") && !said.contains("example"), "{said}");
        }
        assert!(Unfilled::NoHosts.message("K").contains("--to <the API's host>"));
    }

    /// Goal: a request carrying a secret names no other host in a header.
    /// Method: each routing header, in any case, among ordinary ones.
    #[test]
    fn a_secrets_request_sets_no_routing_header() {
        assert_eq!(routing_header(["authorization", "content-type"]), None);
        assert_eq!(routing_header(["authorization", "Host"]), Some("Host"));
        assert_eq!(routing_header(["X-Forwarded-Host"]), Some("X-Forwarded-Host"));
        assert_eq!(routing_header(["forwarded", "accept"]), Some("forwarded"));
        assert_eq!(routing_header(["x-forwarded-for", "hostname"]), None);
    }

    #[test]
    fn header_placeholders() {
        assert_eq!(placeholders("Bearer {{API_KEY}}"), vec!["API_KEY"]);
        assert_eq!(placeholders("{{A}}:{{B}}"), vec!["A", "B"]);
        assert!(placeholders("no {{ end").is_empty());
        assert!(placeholders("plain").is_empty());
    }

    /// Goal: a URL's placeholders fill its path and query alone, the value
    /// encoded so the request goes where its URL said, in as many segments
    /// and parameters; a value that would move it is refused.
    #[test]
    fn a_secret_fills_a_urls_path_and_query_and_moves_nothing() {
        let given = url("https://api.telegram.org/bot{{TELEGRAM_BOT_TOKEN}}/setWebhook?url=https%3A%2F%2Fx.example%2Fin&secret_token={{HOOK}}");
        assert_eq!(url_placeholders(&given), ["TELEGRAM_BOT_TOKEN", "HOOK"]);
        let values = |n: &str| Ok(match n { "TELEGRAM_BOT_TOKEN" => "123456:AA-bb_cc".to_string(), _ => "0f1e".to_string() });
        let filled = fill_url(&given, values).unwrap();
        assert_eq!(filled.as_str(), "https://api.telegram.org/bot123456:AA-bb_cc/setWebhook?url=https%3A%2F%2Fx.example%2Fin&secret_token=0f1e");
        // a value with a URL's own characters is one segment's, one value's bytes
        let odd = fill_url(&given, |_| Ok("a/b?c#d&e=f%g h".to_string())).unwrap();
        assert_eq!(odd.host_str(), Some("api.telegram.org"));
        assert_eq!(odd.path_segments().unwrap().count(), 2, "{odd}");
        assert_eq!(odd.query_pairs().count(), 2, "{odd}");
        assert!(odd.fragment().is_none(), "{odd}");
        assert_eq!(odd.query_pairs().find(|(k, _)| k == "secret_token").unwrap().1, "a/b?c#d&e=f%g h", "the server reads the value as it was");
        // a whole segment of `..` would drop one: refused, as is an empty value
        let dots = url("https://api.example.com/v1/{{S}}/x");
        assert_eq!(fill_url(&dots, |_| Ok("..".to_string())), Err(UrlUnfilled::Moved));
        assert_eq!(fill_url(&dots, |_| Ok(".".to_string())), Err(UrlUnfilled::Moved));
        assert_eq!(fill_url(&dots, |_| Err("no secret named S".to_string())), Err(UrlUnfilled::Value("no secret named S".to_string())));
        // no placeholders: the same URL; none is read from the host
        let plain = url("https://api.example.com/v1/x?k=v");
        assert!(url_placeholders(&plain).is_empty());
        assert_eq!(fill_url(&plain, |_| unreachable!()).unwrap(), plain);
        // what is no secret's name stays as written; a fragment is never filled
        let own = url("https://api.example.com/items/{{id}}?q=%7B%7Bx%7D%7D#{{TOKEN}}");
        assert!(url_placeholders(&own).is_empty(), "{:?}", url_placeholders(&own));
        let kept = fill_url(&url("https://api.example.com/{{S}}/x#frag"), |_| Ok("v".to_string())).unwrap();
        assert_eq!(kept.as_str(), "https://api.example.com/v/x#frag");
        // a placeholder before the path is no placeholder: the host stays as written
        let host = url("https://{{HOST}}.example.com/x");
        assert!(url_placeholders(&host).is_empty());
        assert_eq!(fill_url(&host, |_| unreachable!()).unwrap(), host);
    }

    /// Goal: what goes back to a job names a URL's secret as its placeholder,
    /// whether the answer or the failure echoes it encoded or as it is.
    #[test]
    fn a_urls_secret_goes_back_as_its_placeholder() {
        let opened = std::collections::BTreeMap::from([("TOKEN".to_string(), "123:a/b c".to_string())]);
        let encoded = url_encoded("123:a/b c");
        assert_eq!(encoded, "123:a%2Fb%20c");
        let failed = format!("api.telegram.org: fetch failed: TypeError: Fetch API cannot load: https://api.telegram.org/bot{encoded}/getMe");
        assert_eq!(redact(failed, &opened), "api.telegram.org: fetch failed: TypeError: Fetch API cannot load: https://api.telegram.org/bot{{TOKEN}}/getMe");
        assert_eq!(redact("location: /bot123:a/b c/x".to_string(), &opened), "location: /bot{{TOKEN}}/x");
        assert_eq!(redact("nothing here".to_string(), &opened), "nothing here");
        assert_eq!(redact("123:a/b c".to_string(), &std::collections::BTreeMap::new()), "123:a/b c", "nothing opened, nothing named");
    }
}
