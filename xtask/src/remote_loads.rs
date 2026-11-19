//! `cargo xtask check`'s remote-load pass: no page of this repo loads a
//! script, stylesheet or font, or any file, from another domain. "Not from
//! any external domain (like google fonts, or third party scripts. huge
//! security hole and privacy leak)" (Paul, 2026-11-19): another host's
//! script runs as our origin, and every load tells that host who opened
//! the page. We serve our own files from the release (crates/templates) on
//! the page's own origin, and loading one late or preloading it is fine.
//! Our origin differs per deployment (dev, preview, production), so a page
//! names its own files by a relative or root path: an absolute URL in a
//! load (`http:`, `https:` or `//`) is another domain's by definition, and
//! every one is refused.
//!
//! What a load is, by kind of file:
//! - a page (`.html`, `.htm`, `.svg`): an element's `src`, a `<link>`'s
//!   `href` (a stylesheet, preconnect, preload or icon: each loads or
//!   connects), and its inline styles' and scripts' loads below;
//! - a stylesheet (`.css`): `@import`, and `url()` (a font's `src`, an image);
//! - a script (`.js`, `.mjs`, `.cjs`): `import … from`, `import "…"`,
//!   `import()` and `importScripts()`;
//! - Rust that renders pages (`RENDERS_PAGES`): an element's `src`, a
//!   `<link>`'s `href`, and `@import` (not `url(`, a Rust function's name).
//!
//! Which files: each one git tracks or would add (`js_syntax::listed`) of
//! those kinds, less `js_syntax::VENDORED`, found in code and in comments
//! alike (so no exception hides in one). A person's own fragment is theirs
//! and may load what it likes: this pass reads this repo alone.
//!
//! An exception is a place that must load from another domain: `ALLOWED`,
//! each with why; there is none. As in html_sinks.rs, an entry that no
//! longer matches fails the check.
//!
//! What a text scan cannot see, and review must: a URL a script builds at
//! run time (`link.href = host + path`, `new Worker(url)`), and a load in a
//! kind of file not named above.

use std::path::Path;

use anyhow::{ensure, Context, Result};

/// Where Rust renders pages of its own: the cell's notices and views, and
/// the images' servers' pages.
const RENDERS_PAGES: [&str; 2] = ["cell/src/", "images/"];

/// The places that must load from another domain, each `(path, URL
/// prefix, why)`: none.
pub const ALLOWED: [(&str, &str, &str); 0] = [];

/// Files the walk must read: a walk that misses one is broken, not clean.
const MUST_READ: [&str; 6] = [
    "cell/fragment.css",
    "cell/shell/index.html",
    "cell/src/share.rs",
    "templates/chat/site/index.html",
    "templates/sessions/site/index.html",
    "images/bridge/src/simplex/page.html",
];

/// More files than this means the walk reached something it should not
/// (an unignored node_modules or target): the repo has about 1,000.
const FILES_MAX: usize = 5_000;

/// How a file loads, by its kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Page,
    Style,
    Script,
    Rust,
}

/// A file's kind, by its path: none for a file this pass does not read.
pub fn kind(path: &str) -> Option<Kind> {
    let name = path.rsplit('/').next()?;
    match name.rsplit_once('.')?.1 {
        "html" | "htm" | "svg" => Some(Kind::Page),
        "css" => Some(Kind::Style),
        "js" | "mjs" | "cjs" => Some(Kind::Script),
        "rs" if RENDERS_PAGES.iter().any(|p| path.starts_with(p)) => Some(Kind::Rust),
        _ => None,
    }
}

/// A file to read: its path, kind and text.
pub struct Source {
    pub path: String,
    pub kind: Kind,
    pub text: String,
}

/// A load from another domain: where, what loads, and its URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Load {
    pub path: String,
    pub line: usize,
    pub what: &'static str,
    pub url: String,
}

/// The repo's files of the kinds read, less the vendored.
pub fn sources(root: &Path) -> Result<Vec<Source>> {
    let mut out = vec![];
    for path in crate::js_syntax::listed(root)? {
        let Some(kind) = kind(&path) else { continue };
        if crate::js_syntax::vendored(&path) {
            continue;
        }
        let text = std::fs::read_to_string(root.join(&path)).with_context(|| format!("read {path}"))?;
        out.push(Source { path, kind, text });
        ensure!(out.len() <= FILES_MAX, "more than {FILES_MAX} files to read for remote loads: is node_modules or target no longer ignored?");
    }
    for path in MUST_READ {
        ensure!(out.iter().any(|s| s.path == path), "the walk did not read {path}: it is broken");
    }
    Ok(out)
}

/// Where the absolute URL `lower` names from `from` lies, past spaces, a
/// backslash (a Rust string's escaped quote) and a quote: `http:`,
/// `https:` or `//`, in any case (`lower` is lowercase). A relative or
/// root path, `data:` and a template's blank are none.
fn absolute(lower: &str, from: usize) -> Option<(usize, usize)> {
    let rest = &lower[from..];
    let start = from + rest.len() - rest.trim_start_matches([' ', '\t', '\n', '\r', '\\', '"', '\'', '`']).len();
    let rest = &lower[start..];
    if !(rest.starts_with("http:") || rest.starts_with("https:") || rest.starts_with("//")) {
        return None;
    }
    Some((start, rest.find(['"', '\'', '`', ')', '\\', ' ', '\t', '\n', '\r', '>']).map_or(lower.len(), |e| start + e)))
}

fn ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '$' | '-')
}

/// Where `word` starts a word in `lower`: no identifier character before it.
fn starts<'a>(lower: &'a str, word: &'a str) -> impl Iterator<Item = usize> + 'a {
    lower.match_indices(word).map(|(i, _)| i).filter(move |&i| lower[..i].chars().next_back().is_none_or(|c| !ident(c)))
}

/// Where `attr` is an attribute in `lower` (a tag, or a page): preceded
/// by space and followed by `=` (so `data-src` and `srcset` are no `src`,
/// nor code's `let src = …`); where its value starts.
fn attribute<'a>(lower: &'a str, attr: &'a str) -> impl Iterator<Item = usize> + 'a {
    lower.match_indices(attr).filter_map(move |(i, _)| {
        let spaced = lower[..i].chars().next_back().is_some_and(|c| c.is_ascii_whitespace());
        let value = lower[i + attr.len()..].strip_prefix('=')?;
        spaced.then_some(lower.len() - value.len())
    })
}

/// A page's inline scripts' bodies, where each starts and ends.
fn script_bodies(lower: &str) -> Vec<(usize, usize)> {
    let mut out = vec![];
    let mut at = 0;
    // each pass moves `at` past a script's body, or stops
    while let Some(open) = lower[at..].find("<script").map(|i| at + i) {
        let Some(body) = lower[open..].find('>').map(|i| open + i + 1) else { break };
        let end = lower[body..].find("</script").map_or(lower.len(), |i| body + i);
        out.push((body, end));
        at = end;
    }
    out
}

/// Where a module's specifier starts after `from`, `import` or `import(`
/// at `at`, when the word stands alone (`important`, `fromage` and
/// `importScripts` are none) and a string follows (so `from https://…` in
/// prose is none).
fn specifier(lower: &str, at: usize) -> Option<usize> {
    let word = if lower[at..].starts_with("from") { "from" } else { "import" };
    let after = &lower[at + word.len()..];
    if after.chars().next().is_some_and(ident) {
        return None;
    }
    let spaced = |s: &str| s.trim_start_matches([' ', '\t', '\n', '\r']).len();
    let mut rest = &after[after.len() - spaced(after)..];
    if word == "import" {
        if let Some(call) = rest.strip_prefix('(') {
            rest = &call[call.len() - spaced(call)..];
        }
    }
    rest.starts_with(['"', '\'', '`']).then_some(lower.len() - rest.len())
}

/// Every load from another domain in `source`, in the order found.
pub fn loads(source: &Source) -> Vec<Load> {
    // ASCII lowercase keeps byte offsets: they index `text` as they index `lower`
    let text = source.text.as_str();
    let lower = text.to_ascii_lowercase();
    let markup = matches!(source.kind, Kind::Page | Kind::Rust);
    let style = matches!(source.kind, Kind::Page | Kind::Style);
    // a page's script is its inline scripts: elsewhere its words are prose
    let bodies = if source.kind == Kind::Page { script_bodies(&lower) } else { vec![] };
    let script = |i: &usize| source.kind == Kind::Script || bodies.iter().any(|&(a, b)| a <= *i && *i < b);
    // (where the load is, what loads, where its URL is named from)
    let mut found: Vec<(usize, &'static str, usize)> = vec![];
    if markup {
        found.extend(attribute(&lower, "src").map(|v| (v, "an element's src", v)));
        for (open, _) in lower.match_indices("<link") {
            let end = lower[open..].find('>').map_or(lower.len(), |e| open + e);
            found.extend(attribute(&lower[..end], "href").filter(|&v| v > open).map(|v| (v, "a <link>'s href", v)));
        }
    }
    if markup || style {
        for i in starts(&lower, "@import") {
            let after = &lower[i + "@import".len()..];
            let after = after.trim_start_matches([' ', '\t', '\n', '\r']);
            let after = after.strip_prefix("url(").unwrap_or(after);
            found.push((i, "a stylesheet's @import", lower.len() - after.len()));
        }
    }
    if style {
        found.extend(starts(&lower, "url(").map(|i| (i, "a stylesheet's url()", i + "url(".len())));
    }
    found.extend(starts(&lower, "from").chain(starts(&lower, "import")).filter(script).filter_map(|i| Some((i, "a module's import", specifier(&lower, i)?))));
    found.extend(starts(&lower, "importscripts(").filter(script).map(|i| (i, "a worker's importScripts()", i + "importscripts(".len())));
    found.sort();
    let mut named = std::collections::BTreeSet::new();
    found
        .into_iter()
        .filter_map(|(at, what, from)| {
            let (start, end) = absolute(&lower, from)?;
            // a URL is one load: an `@import url(…)`'s is the import's, found first
            named.insert(start).then(|| Load { path: source.path.clone(), line: text[..at].matches('\n').count() + 1, what, url: text[start..end].to_string() })
        })
        .collect()
}

/// Fails on every load from another domain `allowed` does not name, and
/// on an `allowed` entry that names nothing found.
pub fn check(sources: &[Source], allowed: &[(&str, &str, &str)]) -> Result<()> {
    let mut used = vec![false; allowed.len()];
    let mut refused = vec![];
    for load in sources.iter().flat_map(loads) {
        match allowed.iter().position(|(path, prefix, _)| *path == load.path && load.url.starts_with(prefix)) {
            Some(i) => used[i] = true,
            None => refused.push(load),
        }
    }
    let listed: Vec<String> = refused.iter().map(|l| format!("{}:{}: {} loads {}", l.path, l.line, l.what, l.url)).collect();
    ensure!(
        listed.is_empty(),
        "a page of this repo loads a script, stylesheet or font from another domain (xtask/src/remote_loads.rs), {} place(s): a security hole and a privacy leak (Paul, 2026-11-19). Serve it ourselves, in the release's files, by a path of the page's own origin (async if it helps), or name the place in remote_loads::ALLOWED with why it must:\n{}",
        listed.len(),
        listed.join("\n")
    );
    for (i, (path, prefix, why)) in allowed.iter().enumerate() {
        ensure!(used[i], "remote_loads::ALLOWED names {prefix} in {path} ({why}), and it is not there: remove the entry");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fragment_devstack as devstack;

    fn source(path: &str, text: &str) -> Source {
        Source { path: path.to_string(), kind: kind(path).expect("a kind this pass reads"), text: text.to_string() }
    }

    /// Goal: each kind of load from another domain fails the check, named
    /// by its file, line, what loads and its URL. Method: one planted load
    /// of each form, in each kind of file it is a load in.
    #[test]
    fn a_planted_remote_load_fails_the_check_naming_its_line() {
        let planted = [
            ("a.html", "<!doctype html>\n<link rel=\"stylesheet\" href=\"https://fonts.googleapis.com/css2?family=X\">\n", 2, "a <link>'s href", "https://fonts.googleapis.com/css2?family=X"),
            ("b.html", "<link rel=preconnect href=https://fonts.gstatic.com crossorigin>\n", 1, "a <link>'s href", "https://fonts.gstatic.com"),
            ("c.html", "<LINK\n  rel=\"preload\" as=\"font\"\n  HREF='//cdn.example/f.woff2'>\n", 3, "a <link>'s href", "//cdn.example/f.woff2"),
            ("d.html", "<head>\n<script defer src=\"https://cdn.jsdelivr.net/npm/x@1/x.js\"></script>\n", 2, "an element's src", "https://cdn.jsdelivr.net/npm/x@1/x.js"),
            ("e.htm", "<img alt=\"\" src=HTTP://tracker.example/p.gif>\n", 1, "an element's src", "HTTP://tracker.example/p.gif"),
            ("f.html", "<style>\n  @import url(\"https://fonts.googleapis.com/css2?family=Y\");\n</style>\n", 2, "a stylesheet's @import", "https://fonts.googleapis.com/css2?family=Y"),
            ("g.css", "a { color: red }\n@import \"http://cdn.example/x.css\";\n", 2, "a stylesheet's @import", "http://cdn.example/x.css"),
            ("h.css", "@font-face { font-family: Z; src: url(https://fonts.gstatic.com/z.woff2) format(\"woff2\"); }\n", 1, "a stylesheet's url()", "https://fonts.gstatic.com/z.woff2"),
            ("i.svg", "<svg><style>.a { background: URL('//img.example/a.png') }</style></svg>\n", 1, "a stylesheet's url()", "//img.example/a.png"),
            ("j.mjs", "import { a } from \"https://esm.sh/a@1\";\n", 1, "a module's import", "https://esm.sh/a@1"),
            ("k.js", "const m = await import( 'https://unpkg.com/m@2/m.js');\n", 1, "a module's import", "https://unpkg.com/m@2/m.js"),
            ("l.cjs", "\nimport \"https://cdn.example/side-effect.js\";\n", 2, "a module's import", "https://cdn.example/side-effect.js"),
            ("m.js", "importScripts(\"https://cdn.example/sw-lib.js\");\n", 1, "a worker's importScripts()", "https://cdn.example/sw-lib.js"),
            ("n.html", "<script type=\"module\">\nexport * from 'https://cdn.example/all.js';\n</script>\n", 2, "a module's import", "https://cdn.example/all.js"),
            ("cell/src/page.rs", "let page = format!(\"<head><script src=\\\"https://cdn.example/x.js\\\"></script>\");\n", 1, "an element's src", "https://cdn.example/x.js"),
            ("images/bridge/src/x.rs", "const PAGE: &str = \"<link rel=stylesheet href='https://fonts.example/s.css'>\";\n", 1, "a <link>'s href", "https://fonts.example/s.css"),
        ];
        for (path, text, line, what, url) in planted {
            let s = source(path, text);
            assert_eq!(loads(&s), [Load { path: path.into(), line, what, url: url.into() }], "{path}");
            let err = check(&[s], &[]).expect_err(path).to_string();
            assert!(err.contains(&format!("{path}:{line}: {what} loads {url}")), "{err}");
            assert!(err.contains("another domain") && err.contains("privacy leak"), "it says why: {err}");
        }
    }

    /// Goal: a load from the page's own origin passes, however it loads
    /// (late, preloaded, imported), and so does an absolute URL that is no
    /// load. Method: each form with a relative or root path, a template's
    /// blank, `data:`, and links, prose and API URLs that load nothing.
    #[test]
    fn own_origin_loads_and_non_loads_pass() {
        let clean = [
            ("a.html", "<link rel=\"stylesheet\" href=\"./__fonts/fonts.css\">\n<link rel=\"preload\" as=\"font\" href=\"/__shell/jetbrains-mono-400.woff2\" crossorigin>\n<link rel=\"stylesheet\" href=\"{{sheet}}\">\n<script type=\"module\" src=\"./chat.js\"></script>\n<img src=\"data:image/png;base64,AA\">\n<a href=\"https://github.com/finitecomputer/fragment\">source</a>\n<p>It came from \"https://example.com\" once.</p>\n"),
            ("b.css", "@import url(\"__fonts/fonts.css\");\n@font-face { font-family: F; src: url(\"funnel-sans-400.ttf\") format(\"truetype\"); }\n.a { background: url(platform-landscape.jpg) }\n"),
            ("c.mjs", "import { a } from \"./__fragment.js\";\nconst m = await import(\"./__markdown.js\");\nconst u = new URL(\"https://api.example.com/v1\");\nfetch(\"https://api.example.com/v1\");\n// an important note, imported from https://example.com\nconst link = 'https://example.com/a';\n"),
            ("cell/src/page.rs", "let u = url(\"https://api.example.com/v1\");\nlet h = format!(\"<link rel=\\\"stylesheet\\\" href=\\\"{}\\\">\", versioned(\"sheet.css\"));\nlet data_src = \"<div data-src=\\\"https://x.example\\\">\";\nlet src = \"https://api.example.com/v1\";\n"),
        ];
        for (path, text) in clean {
            let s = source(path, text);
            assert_eq!(loads(&s), [], "{path}");
        }
        assert_eq!(kind("docs/api.md"), None, "prose is read for no load");
        assert_eq!(kind("crates/core/src/secrets.rs"), None, "Rust that renders no page is not read");
        assert_eq!(kind("cell/src/share.rs"), Some(Kind::Rust));
    }

    /// Goal: an exception exempts its file's URL and nothing else, and a
    /// stale one fails. Method: the same load allowed, allowed in another
    /// file, and an entry that matches nothing.
    #[test]
    fn an_exception_is_exact_and_never_stale() {
        let probe = || source("fixtures/probe.html", "<script src=\"https://cdn.example/x.js\"></script>\n");
        check(&[probe()], &[("fixtures/probe.html", "https://cdn.example/", "a probe")]).expect("allowed");
        assert!(check(&[probe()], &[("fixtures/other.html", "https://cdn.example/", "elsewhere")]).is_err(), "another file's entry exempts nothing");
        assert!(check(&[probe()], &[("fixtures/probe.html", "https://other.example/", "another host")]).is_err(), "another host's entry exempts nothing");
        let err = check(&[source("a.html", "<p>ok</p>\n")], &[("fixtures/probe.html", "https://cdn.example/", "a probe")]).expect_err("stale").to_string();
        assert!(err.contains("ALLOWED names https://cdn.example/ in fixtures/probe.html"), "{err}");
    }

    /// Goal: the repo itself loads nothing from another domain, and the
    /// walk reads the shell, the platform's stylesheet, the templates'
    /// pages and the images' pages.
    #[test]
    fn the_repo_loads_nothing_from_another_domain() {
        let root = devstack::repo_root();
        let found = sources(&root).expect("the repo's files");
        for path in ["cell/fragment.css", "cell/shell/index.html", "cell/shell/shell.css", "templates/chat/site/index.html", "templates/doc/applib/notes.mjs", "images/bridge/src/simplex/page.html"] {
            assert!(found.iter().any(|s| s.path == path), "{path} is read");
        }
        check(&found, &ALLOWED).expect("no page of this repo loads from another domain");
    }
}
