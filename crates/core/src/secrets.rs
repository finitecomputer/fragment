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
/// placeholder again: what goes back to a job of a fetch that carried
/// secrets (a failure's message, an answer that echoes its request). A
/// vendor may write a value back in an encoding of its own, so each is
/// named in every form below, each of its characters in any of them
/// (`spelled`, `char_forms`):
/// - as it is;
/// - its UTF-8 bytes percent-encoded, in either hex case, once or twice
///   (`%3A`, `%3a`, `%C3%A9`, `%253A`: a URL inside a URL's query), and a
///   space as `+` (a form's);
/// - as a JSON string escapes it (`\/`, `\"`, `\\`, `:`, `é`, a
///   surrogate pair past the first plane, in either hex case), once or
///   within another JSON string (`\\u003a`: an error that quotes JSON);
/// - as an HTML character reference (`&#58;`, `&#x3A;`, `&#233;`, `&amp;`);
///
/// and its base64 inside a longer blob, in either alphabet at any of its
/// three alignments (`base64_spellings`: an `Authorization: Basic` echo's),
/// itself in any of those forms (JSON's `\/` in a serialized blob), for a
/// value of 12 bytes or more, whose base64 is no chance match. A value that
/// comes back some other way (compressed, hashed, encrypted, cut in pieces
/// by the vendor) is past what matching text can name.
pub fn redact(text: String, opened: &std::collections::BTreeMap<String, String>) -> String {
    // each value, and its base64 cores: what a spelling may spell; longest
    // first, so a value inside another is named as the other
    let mut wanted: Vec<(&str, String)> = Vec::new();
    for (name, value) in opened.iter().filter(|(_, v)| !v.is_empty()) {
        wanted.push((name.as_str(), value.clone()));
        wanted.extend(base64_spellings(value.as_bytes()).into_iter().map(|core| (name.as_str(), core)));
    }
    if wanted.is_empty() {
        return text;
    }
    wanted.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
    let placeholder = |name: &str| format!("{{{{{name}}}}}");
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    // bounded by the text: each pass moves past one byte or one spelling
    'text: while i < bytes.len() {
        for (name, value) in &wanted {
            if let Some(end) = spelled(bytes, i, value) {
                out.extend_from_slice(placeholder(name).as_bytes());
                i = end;
                continue 'text;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    // a spelling starts at a character (a value's first byte is one, and
    // every other form is ASCII) and ends after one, so what is left is UTF-8
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// Where `value` ends if `text` spells it from `at`, each character in any
/// form `char_forms` knows (the longest such spelling), or none. The forms
/// in play at once are few (a character's spellings that start alike), so
/// the set of places a spelling may have reached stays small.
fn spelled(text: &[u8], at: usize, value: &str) -> Option<usize> {
    let mut reach = vec![at];
    for c in value.chars() {
        let mut next: Vec<usize> = Vec::new();
        for &p in &reach {
            for end in char_forms(text, p, c) {
                if !next.contains(&end) {
                    next.push(end);
                }
            }
        }
        if next.is_empty() {
            return None;
        }
        reach = next;
    }
    reach.into_iter().max()
}

/// Where each spelling of the character `c` that `text` holds at `at`
/// ends: its UTF-8 bytes, each in any form `byte_forms` knows; its JSON
/// escape (`json_escape`); its HTML character reference (`html_reference`).
fn char_forms(text: &[u8], at: usize, c: char) -> Vec<usize> {
    let mut buf = [0u8; 4];
    let mut reach = vec![at];
    for &b in c.encode_utf8(&mut buf).as_bytes() {
        let mut next: Vec<usize> = Vec::new();
        for &p in &reach {
            for n in byte_forms(&text[p..], b) {
                if !next.contains(&(p + n)) {
                    next.push(p + n);
                }
            }
        }
        reach = next;
        if reach.is_empty() {
            break;
        }
    }
    reach.extend(json_escape(&text[at..], c).map(|n| at + n));
    reach.extend(html_reference(&text[at..], c).map(|n| at + n));
    reach
}

/// The lengths of each spelling of the byte `b` that `text` starts with:
/// as it is, percent-encoded (once or twice), a form's `+` for a space,
/// and JSON's escapes of `/`, `"` and `\` (`redact`'s forms).
fn byte_forms(text: &[u8], b: u8) -> impl Iterator<Item = usize> {
    let is = |s: Option<&[u8]>, f: &dyn Fn(&[u8]) -> bool| s.is_some_and(f);
    let code = u32::from(b);
    // a JSON escape's backslashes, escaped once more within another JSON
    // string: `\/` is `\\/` or `\\\/` there, `\"` is `\\\"`, `\\` is `\\\\`
    let slashes = text.iter().take(4).take_while(|&&c| c == b'\\').count();
    [
        (text.first() == Some(&b)).then_some(1),
        (b == b' ' && text.first() == Some(&b'+')).then_some(1),
        is(text.get(..3), &|s| s[0] == b'%' && hex(&s[1..]) == Some(code)).then_some(3),
        is(text.get(..5), &|s| s.starts_with(b"%25") && hex(&s[3..]) == Some(code)).then_some(5),
        (b"/\"".contains(&b) && (1..=3).contains(&slashes) && text.get(slashes) == Some(&b)).then_some(slashes + 1),
        (b == b'\\' && (slashes == 2 || slashes == 4)).then_some(slashes),
    ]
    .into_iter()
    .flatten()
}

/// `s`'s value as hex digits, none for no digits or another character.
fn hex(s: &[u8]) -> Option<u32> {
    (!s.is_empty() && s.iter().all(u8::is_ascii_hexdigit)).then(|| std::str::from_utf8(s).ok().and_then(|s| u32::from_str_radix(s, 16).ok())).flatten()
}

/// The length of a JSON escape of `c` that `text` starts with: `\u` and
/// its UTF-16 code unit in either hex case, a surrogate pair for a
/// character past the first plane, each escape's backslash once or twice
/// (within another JSON string); none if it does not.
fn json_escape(text: &[u8], c: char) -> Option<usize> {
    let mut units = [0u16; 2];
    let units = c.encode_utf16(&mut units);
    let slashes = text.iter().take(3).take_while(|&&b| b == b'\\').count();
    if !(1..=2).contains(&slashes) {
        return None;
    }
    let mut at = 0;
    for &unit in units.iter() {
        let escape = text.get(at..at + slashes + 5)?;
        if !escape[..slashes].iter().all(|&b| b == b'\\') || escape[slashes] != b'u' || hex(&escape[slashes + 1..]) != Some(u32::from(unit)) {
            return None;
        }
        at += slashes + 5;
    }
    Some(at)
}

/// The length of an HTML character reference to `c` that `text` starts
/// with (`&#233;`, `&#xE9;`, or a named one of an ASCII character), if it
/// does.
fn html_reference(text: &[u8], c: char) -> Option<usize> {
    if text.first() != Some(&b'&') {
        return None;
    }
    // the longest reference read: `&#x`, seven digits (leading zeros), `;`
    let end = text.iter().take(12).position(|&b| b == b';')?;
    let digits = |s: &[u8], radix: u32| (!s.is_empty() && s.iter().all(|d| d.is_ascii_digit() || (radix == 16 && d.is_ascii_hexdigit()))).then(|| std::str::from_utf8(s).ok().and_then(|s| u32::from_str_radix(s, radix).ok())).flatten();
    let code = match &text[1..end] {
        [b'#', b'x' | b'X', hex @ ..] => digits(hex, 16)?,
        [b'#', dec @ ..] => digits(dec, 10)?,
        b"amp" => u32::from('&'),
        b"quot" => u32::from('"'),
        b"apos" => u32::from('\''),
        b"lt" => u32::from('<'),
        b"gt" => u32::from('>'),
        _ => return None,
    };
    (code == u32::from(c)).then_some(end + 1)
}

/// A base64 spelling shorter than this many characters is not named: a
/// blob may hold one by chance. Every value of 12 bytes or more is named at
/// every alignment (its fewest fixed characters, at an offset of one or two
/// bytes, are 15); tokens run longer.
const BASE64_MIN_CHARS: usize = 15;

/// A value's base64 inside a longer blob: at each of the three alignments
/// it may start at, the characters its own bytes fix (an edge's also hold
/// a neighbour's bits), in the standard alphabet and the URL-safe one.
fn base64_spellings(value: &[u8]) -> Vec<String> {
    use base64::Engine;
    let mut forms = Vec::new();
    for lead in 0..3usize {
        let mut bytes = vec![0u8; lead];
        bytes.extend_from_slice(value);
        let encoded = base64::engine::general_purpose::STANDARD_NO_PAD.encode(&bytes);
        let (first, last) = ((8 * lead).div_ceil(6), 8 * (lead + value.len()) / 6);
        if last < first + BASE64_MIN_CHARS {
            continue;
        }
        let core = &encoded[first..last];
        forms.push(core.to_string());
        forms.push(core.replace('+', "-").replace('/', "_"));
    }
    forms.dedup();
    forms
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

    /// A vendor's spellings of a value (`redact`'s forms), as a test writes them.
    mod spell {
        use base64::Engine;

        pub fn upper(v: &str) -> String {
            v.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
        }
        pub fn lower(v: &str) -> String {
            v.bytes().map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02x}") }).collect()
        }
        pub fn every_byte(v: &str) -> String {
            v.bytes().map(|b| format!("%{b:02X}")).collect()
        }
        pub fn twice(v: &str) -> String {
            upper(v).replace('%', "%25")
        }
        /// Each character but a letter or digit as `\u` and its UTF-16 code
        /// units (`/` as `\/`), as an ASCII-escaping serializer writes it.
        pub fn json(v: &str) -> String {
            v.chars()
                .map(|c| match c {
                    '/' => "\\/".to_string(),
                    c if c.is_ascii_alphanumeric() => c.to_string(),
                    c => c.encode_utf16(&mut [0; 2]).iter().map(|u| format!("\\u{u:04x}")).collect(),
                })
                .collect()
        }
        /// `json`, its escapes' hex in upper case.
        pub fn json_upper(v: &str) -> String {
            v.chars()
                .map(|c| match c {
                    '/' => "\\/".to_string(),
                    c if c.is_ascii_alphanumeric() => c.to_string(),
                    c => c.encode_utf16(&mut [0; 2]).iter().map(|u| format!("\\u{u:04X}")).collect(),
                })
                .collect()
        }
        /// JSON-escaped within another JSON string (an error that quotes JSON).
        pub fn json_twice(v: &str) -> String {
            json(v).replace('\\', "\\\\")
        }
        pub fn html(v: &str) -> String {
            v.chars().enumerate().map(|(i, c)| if c.is_ascii_alphanumeric() { c.to_string() } else if i % 2 == 0 { format!("&#{};", u32::from(c)) } else { format!("&#X{:x};", u32::from(c)) }).collect()
        }
        /// `v` inside a blob of `lead` bytes before it and some after.
        pub fn base64(v: &str, lead: usize, url_safe: bool) -> String {
            let blob = format!("{}{v}:after", &"user:"[..lead]);
            if url_safe {
                base64::engine::general_purpose::URL_SAFE.encode(blob)
            } else {
                base64::engine::general_purpose::STANDARD.encode(blob)
            }
        }
        /// That blob as a JSON serializer that escapes `/` writes it.
        pub fn base64_in_json(v: &str, lead: usize, url_safe: bool) -> String {
            base64(v, lead, url_safe).replace('/', "\\/")
        }
        /// `value` as an ASCII-escaping JSON serializer writes it inside an
        /// object (`ensure_ascii`): serde_json's text, every character past
        /// ASCII then escaped as `\u` code units.
        pub fn serialized(value: &str) -> String {
            serde_json::to_string(&serde_json::json!({ "echo": value })).unwrap().chars().map(|c| if c.is_ascii() { c.to_string() } else { c.encode_utf16(&mut [0; 2]).iter().map(|u| format!("\\u{u:04x}")).collect() }).collect()
        }
    }

    /// Goal (#814, a security bug): a secret a vendor writes back in an
    /// encoding of its own goes back to the job as its placeholder, one
    /// case a spelling: percent-encoded fully, in lower-case hex, every
    /// byte, twice; JSON-escaped; HTML-escaped; a form's `+`; base64 in a
    /// longer blob in both alphabets at each alignment. Invalid: a near
    /// miss, cut escapes and a short value's base64 stay as they are.
    /// Replay: named once, it is named the same again. A value inside
    /// another is named as the longer.
    #[test]
    fn a_secret_goes_back_as_its_placeholder_however_a_vendor_spells_it() {
        let token = "123456789:AAHdqTcv/CH1vGWJ+xfSeofSAs0K5=";
        let opened = std::collections::BTreeMap::from([("TOKEN".to_string(), token.to_string())]);
        let named = |text: String| redact(text, &opened);
        for (how, spelled) in [
            ("as it is", token.to_string()),
            ("as it went into the URL", url_encoded(token)),
            ("percent-encoded in upper-case hex", spell::upper(token)),
            ("percent-encoded in lower-case hex", spell::lower(token)),
            ("every byte percent-encoded", spell::every_byte(token)),
            ("percent-encoded twice", spell::twice(token)),
            ("JSON-escaped", spell::json(token)),
            ("JSON-escaped within JSON", spell::json_twice(token)),
            ("HTML-escaped", spell::html(token)),
        ] {
            assert_eq!(named(format!("{{\"url\":\"https://api.x/bot{spelled}/getMe\"}}")), "{\"url\":\"https://api.x/bot{{TOKEN}}/getMe\"}", "{how}: {spelled}");
        }
        for lead in 0..3 {
            for url_safe in [false, true] {
                let blob = spell::base64(token, lead, url_safe);
                let told = named(format!("Authorization: Basic {blob}"));
                assert!(told.contains("{{TOKEN}}") && !told.contains(&blob), "base64 at {lead}, url-safe {url_safe}: {blob} → {told}");
            }
        }
        // a form's space; a value inside another is named as the longer
        let spaced = std::collections::BTreeMap::from([("A".to_string(), "x y z".to_string()), ("B".to_string(), "x y z 2".to_string())]);
        assert_eq!(redact("q=x+y+z+2&r=x%20y%20z".to_string(), &spaced), "q={{B}}&r={{A}}");

        // invalid: a near miss, cut escapes and a short value's base64
        // Alex's review of #871: base64 inside JSON (its `/` escaped), and a
        // character past ASCII as a JSON or HTML escape, the first plane's
        // and a surrogate pair's alike, as serializers write them
        let named_one = |value: &str, text: String| redact(text, &std::collections::BTreeMap::from([("K".to_string(), value.to_string())]));
        assert_eq!(named_one("abc???def???ghi", r#"{"echo":"YWJjPz8\/ZGVmPz8\/Z2hp"}"#.to_string()), r#"{"echo":"{{K}}"}"#, "base64 in JSON");
        assert_eq!(named_one("clé-0123456789", r#"{"echo":"cl\u00e9-0123456789"}"#.to_string()), r#"{"echo":"{{K}}"}"#, "a JSON escape past ASCII");
        for value in ["clé-0123456789", "key😀0123456789", "中文-token-0123", "ß:/+=0123456789"] {
            for (how, spelled) in [
                ("as it is", value.to_string()),
                ("its UTF-8 percent-encoded", spell::upper(value)),
                ("JSON-escaped", spell::json(value)),
                ("upper-case JSON escapes", spell::json_upper(value)),
                ("JSON-escaped within JSON", spell::json_twice(value)),
                ("HTML references", spell::html(value)),
            ] {
                let told = named_one(value, format!("[{spelled}]"));
                assert_eq!(told, "[{{K}}]", "{value:?} {how}: {spelled}");
            }
            let serialized = spell::serialized(value);
            assert_eq!(named_one(value, serialized.clone()), r#"{"echo":"{{K}}"}"#, "{value:?} as an ASCII-escaping serializer writes it: {serialized}");
            for lead in 0..3 {
                for url_safe in [false, true] {
                    let blob = spell::base64_in_json(value, lead, url_safe);
                    let told = named_one(value, format!("{{\"auth\":\"{blob}\"}}"));
                    assert!(told.contains("{{K}}") && !told.contains(&blob), "{value:?} base64 in JSON at {lead}, url-safe {url_safe}: {blob} → {told}");
                }
            }
        }

        let near = token.replace('5', "6");
        assert_eq!(named(near.clone()), near, "a near miss is no secret");
        for cut in ["123456789%3", "123456789\\u00", "123456789&#5", "%", "&", "\\", "&#x;", "&#;", "&#xzz;"] {
            assert_eq!(named(cut.to_string()), cut, "{cut}");
        }
        let short = std::collections::BTreeMap::from([("S".to_string(), "short123".to_string())]);
        let blob = spell::base64("short123", 0, false);
        assert_eq!(redact(blob.clone(), &short), blob, "a short value's base64 may be chance");
        assert_eq!(redact("".to_string(), &opened), "");
        let empty = std::collections::BTreeMap::from([("E".to_string(), String::new())]);
        assert_eq!(redact("anything".to_string(), &empty), "anything", "an empty value names nothing");

        // replay: what is named stays named
        let once = named(format!("{token} {} {}", spell::lower(token), spell::base64(token, 1, false)));
        assert_eq!(named(once.clone()), once);
        assert!(!once.contains(token), "{once}");
    }

    /// Goal (#814): across many token shapes and every spelling, nothing of
    /// a value is left to read, and its placeholder takes its place.
    /// Method: tokens of 12 to 64 characters over a token's alphabet
    /// (letters, digits, `:/+=-_.~`, a space, and `é ß 中 😀` past ASCII), a
    /// deterministic generator.
    #[test]
    fn no_spelling_of_any_token_shape_is_left() {
        let alphabet: Vec<char> = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789:/+=-_.~ éß中😀".chars().collect();
        let mut seed: u64 = 0x5eed;
        let mut next = |n: usize| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as usize % n
        };
        for _ in 0..300 {
            let len = 12 + next(53);
            let token: String = (0..len).map(|_| alphabet[next(alphabet.len())]).collect();
            let opened = std::collections::BTreeMap::from([("T".to_string(), token.clone())]);
            let mut spellings = vec![token.clone(), url_encoded(&token), spell::upper(&token), spell::lower(&token), spell::every_byte(&token), spell::twice(&token), spell::json(&token), spell::json_twice(&token), spell::html(&token)];
            spellings.push(spell::serialized(&token));
            for lead in 0..3 {
                spellings.push(spell::base64(&token, lead, false));
                spellings.push(spell::base64(&token, lead, true));
                spellings.push(spell::base64_in_json(&token, lead, false));
            }
            for spelled in spellings {
                let told = redact(format!("before {spelled} after"), &opened);
                assert!(told.contains("{{T}}"), "{token:?} as {spelled}: {told}");
                assert!(!told.contains(&spelled) && !told.contains(token.trim()), "{token:?} as {spelled}: {told}");
            }
        }
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
