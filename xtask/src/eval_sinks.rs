//! `cargo xtask check`'s eval pass: no runtime code runs a string as
//! script. "A check would be good so eval doesn't land in our runtime code"
//! (Paul, 2026-10-20). Today none does: the preview cards drive Chrome over
//! DevTools only to open, size and capture a page (cell/src/card.rs), and the
//! platform's pages and a fragment's Worker run only the code deployed with
//! them. The e2e evaluates in Chrome freely (crates/e2e/src/browser.rs); it is
//! not runtime code.
//!
//! Which files: runtime code is what ships or runs for people (`RUNTIME`: the
//! cell, the templates, the computer images, the CLI, and the crates the cell
//! and the CLI are built of), each file there git tracks or would add, less
//! tests (a `tests` directory), `js_syntax::VENDORED` and prose. Each is read
//! as its language reads it (`Lang`): Rust, JavaScript, a page's inline
//! scripts, or a `#`-commented script (Python, shell, a Dockerfile). Every
//! other directory is in `NOT_RUNTIME`, with why; a tracked file in neither
//! fails the check, so a new crate or directory is placed before its code can
//! go unread.
//!
//! What is refused (`REFUSED`), each found where its language has code, never
//! in a comment (so one may say what the rule is), and in a string only where
//! a string is the danger:
//! - DevTools: `Runtime.evaluate`, `Runtime.callFunctionOn`,
//!   `Runtime.compileScript`, `Page.addScriptToEvaluateOnNewDocument`, and any
//!   method of the `Debugger.` domain, in code and strings alike (a method is a
//!   string on the wire) and in every language. The middle three by their
//!   method's own name, so one whose domain is joined on at run time
//!   (`format!("{domain}.callFunctionOn")`) is found too; `evaluate` alone is
//!   too common a word, so `Runtime.evaluate` is found whole.
//! - A page driver's evaluators, called as a method: Puppeteer's and
//!   Playwright's `.evaluate(`, `.evaluateHandle(`, `.evaluateOnNewDocument(`,
//!   `.addInitScript(`, `.$eval(` and `.$$eval(` (Browser Rendering offers a
//!   Worker either), and chromiumoxide's and headless_chrome's `.evaluate(`: a
//!   driver names no DevTools method in our source. XPath's
//!   `document.evaluate(` is found too.
//! - JavaScript's own, in its code: the identifier `eval`, wherever it stands
//!   (a module is strict, so `eval` is the global's or a property's name:
//!   `(0, eval)(s)` and `globalThis.eval(s)` are found, not only `eval(s)`);
//!   `Function` called, with `new` or without (only called: `instanceof
//!   Function` and `Function.prototype` are ordinary code); `setTimeout` or
//!   `setInterval` whose first argument is a string literal; `import(` of
//!   anything but one string literal, and of a `data:` or `blob:` one (code
//!   from a string, by its URL); and a string that is `eval` or `Function`
//!   alone, the name to reach one by (`globalThis["eval"]`).
//! - The JavaScript a Rust string carries (the cell's pages and their scripts
//!   are Rust strings: cell/src/share.rs, cell/src/auth.rs): each string's
//!   value, read as JavaScript under the same rules; and js_sys's evaluators
//!   in Rust code, `eval(` and `Function::new_no_args` or `new_with_args`.
//!
//! What a text scan cannot see, and review must: a timer handed a variable
//! that holds a string; a name made at run time (`globalThis[name]`, a method
//! named apart from its domain's `Runtime`); a constructor reached through
//! another (`(async () => {}).constructor`); a `Worker` or `importScripts` of
//! a `blob:` URL; a `<script>` or an event handler a page makes
//! (html_sinks.rs refuses those); and what this repo does not write: Hermes'
//! own browser tools drive a browser on a computer (images/hermes/Dockerfile
//! installs the agent-browser they run), as Hermes' code.
//!
//! An exception is a place that must run a string as script: `ALLOWED`, each
//! with why, empty today. Like html_sinks' list, an entry that no longer
//! matches fails the check, so a stale one never waves a new use through.

use std::collections::BTreeSet;
use std::ops::Range;
use std::path::Path;

use anyhow::{ensure, Context, Result};

/// Where runtime code is: what ships or runs for people.
pub const RUNTIME: [(&str, &str); 8] = [
    ("cell/", "the Worker: its Rust, entry.mjs and the platform's JavaScript, and the shell"),
    ("templates/", "the templates' apps, sites and skills, which fragments and computers run"),
    ("images/", "the computer images: the bridge, Hermes' boot, plugins and hooks, the screen pages"),
    ("cli/", "the CLI"),
    ("crates/core/", "the cell's pure logic"),
    ("crates/proto/", "the wire types"),
    ("crates/templates/", "the release's files, embedded in the cell and the CLI"),
    ("crates/nip98/", "NIP-98 signatures, the cell's and the CLI's"),
];

/// The repo's other directories, each with why its code is not runtime code.
pub const NOT_RUNTIME: [(&str, &str); 9] = [
    ("bench/", "benchmarks, run by hand on a machine of one's own"),
    ("crates/e2e/", "the e2e, which drives Chrome by Runtime.evaluate (crates/e2e/src/browser.rs)"),
    ("crates/fakes/", "the vendors' fakes, for dev and the e2e"),
    ("crates/devstack/", "the dev stack, for dev and the e2e"),
    ("xtask/", "the repo's tooling"),
    ("docs/", "prose"),
    ("deploy/", "a deployment's example config"),
    (".github/", "CI's workflows"),
    (".cargo/", "cargo's config"),
];

/// What runs a string as script, each with what it does: the name a failure
/// reports, and an `ALLOWED` entry names.
pub const REFUSED: [(&str, &str); 17] = [
    ("Runtime.evaluate", "DevTools: runs a string as script in a page"),
    ("callFunctionOn", "DevTools' Runtime.callFunctionOn: runs a function's source in a page"),
    ("compileScript", "DevTools' Runtime.compileScript: compiles a string as script, for Runtime.runScript"),
    ("addScriptToEvaluateOnNewDocument", "DevTools' Page.addScriptToEvaluateOnNewDocument: runs a string in each document a page opens"),
    ("Debugger.", "DevTools' Debugger domain: evaluates on a paused frame, rewrites a script's source"),
    (".evaluate(", "a page driver's (Puppeteer, Playwright, chromiumoxide, headless_chrome): runs a function's source or a string in a page"),
    (".evaluateHandle(", "Puppeteer's and Playwright's: runs a function's source in a page"),
    (".evaluateOnNewDocument(", "Puppeteer's: runs a function's source in each document a page opens"),
    (".addInitScript(", "Playwright's: runs a function's source or a string in each document a page opens"),
    (".$eval(", "Puppeteer's and Playwright's: runs a function's source on a page's element"),
    (".$$eval(", "Puppeteer's and Playwright's: runs a function's source on a page's elements"),
    ("eval", "runs a string as code"),
    ("Function(", "makes a function of a string"),
    ("setTimeout", "runs its string first argument as code"),
    ("setInterval", "runs its string first argument as code"),
    ("import(", "loads a module by a URL made at run time, or a data: or blob: URL's (code from a string)"),
    ("js_sys::Function", "Function::new_no_args and new_with_args make a function of a string"),
];

/// The DevTools names, found in code and strings.
const DEVTOOLS: [&str; 5] = ["Runtime.evaluate", "callFunctionOn", "compileScript", "addScriptToEvaluateOnNewDocument", "Debugger."];
/// A page driver's evaluators, found as a method called in code.
const DRIVERS: [&str; 6] = [".evaluate(", ".evaluateHandle(", ".evaluateOnNewDocument(", ".addInitScript(", ".$eval(", ".$$eval("];

/// The places that must, each `(path, name, why)`.
pub const ALLOWED: [(&str, &str, &str); 0] = [];

/// More files than this means the walk reached something it should not: the
/// repo's runtime code is about 230.
pub const FILES_MAX: usize = 2_000;
/// The file whose DevTools calls this pass exists to hold: a walk that misses
/// it is broken, not clean.
const CARDS: &str = "cell/src/card.rs";

/// How a file is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    JavaScript,
    /// a page: its inline scripts, as JavaScript
    Html,
    /// `#` comments: Python, a shell script, a Dockerfile
    Hash,
}

/// How a repo path is read, by its name; `None` when it holds no code.
pub fn lang(path: &str) -> Option<Lang> {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rsplit_once('.').map(|(_, ext)| ext) {
        Some("rs") => Some(Lang::Rust),
        Some("js" | "mjs" | "cjs") => Some(Lang::JavaScript),
        Some("html") => Some(Lang::Html),
        Some("py" | "sh") => Some(Lang::Hash),
        _ if name == "Dockerfile" => Some(Lang::Hash),
        _ => None,
    }
}

/// A runtime file, read.
pub struct File {
    pub path: String,
    pub lang: Lang,
    pub text: String,
}

/// A refused use found: where, and which.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Hit {
    pub path: String,
    pub line: usize,
    pub name: &'static str,
}

/// The repo's runtime code, read: each tracked file placed first
/// (`placed`), then what `select` takes of what git tracks or would add.
pub fn sources(root: &Path) -> Result<Vec<File>> {
    placed(&crate::js_syntax::tracked(root)?)?;
    let mut out = vec![];
    for (path, lang) in select(crate::js_syntax::listed(root)?.into_iter())? {
        let text = std::fs::read_to_string(root.join(&path)).with_context(|| format!("read {path}"))?;
        out.push(File { path, lang, text });
    }
    Ok(out)
}

/// Fails on a tracked file in a directory neither `RUNTIME` nor
/// `NOT_RUNTIME` names (a file at the top is config or prose), and on an
/// entry of either that names no tracked file.
pub fn placed(tracked: &BTreeSet<String>) -> Result<()> {
    let lists = RUNTIME.iter().chain(&NOT_RUNTIME);
    let unplaced: Vec<&String> = tracked.iter().filter(|p| p.contains('/') && !lists.clone().any(|(dir, _)| p.starts_with(dir))).collect();
    ensure!(
        unplaced.is_empty(),
        "tracked files in a directory xtask/src/eval_sinks.rs does not place: name it in RUNTIME (shipped or run for people, so read) or NOT_RUNTIME (with why not):\n{}",
        unplaced.iter().map(|p| p.as_str()).collect::<Vec<_>>().join("\n")
    );
    for (dir, why) in lists {
        ensure!(tracked.iter().any(|p| p.starts_with(dir)), "eval_sinks names {dir} ({why}), and git tracks nothing there: remove or move the entry");
    }
    Ok(())
}

/// The runtime code among `paths`, sorted as they come, with how each is
/// read: under `RUNTIME`, in no `tests` directory, not vendored, and code.
pub fn select(paths: impl Iterator<Item = String>) -> Result<Vec<(String, Lang)>> {
    let mut found = vec![];
    for path in paths {
        let runtime = RUNTIME.iter().any(|(dir, _)| path.starts_with(dir));
        let test = path.split('/').any(|c| c == "tests");
        let Some(lang) = lang(&path).filter(|_| runtime && !test && !crate::js_syntax::vendored(&path)) else { continue };
        found.push((path, lang));
        ensure!(found.len() <= FILES_MAX, "more than {FILES_MAX} runtime files to read: is node_modules or target no longer ignored?");
    }
    ensure!(found.iter().any(|(p, _)| p == CARDS), "the walk did not find {CARDS}: it is broken");
    Ok(found)
}

/// Every refused use in `file`, by line, each once.
pub fn hits(file: &File) -> Vec<Hit> {
    let b = file.text.as_bytes();
    let mut at = vec![];
    match file.lang {
        Lang::JavaScript => {
            let parts = lex_js(b);
            devtools(b, &parts, &mut at);
            drivers(b, &parts, &mut at);
            javascript(b, &parts, &mut at);
        }
        Lang::Html => {
            for script in scripts(b) {
                let s = &b[script.clone()];
                let parts = lex_js(s);
                let mut found = vec![];
                devtools(s, &parts, &mut found);
                drivers(s, &parts, &mut found);
                javascript(s, &parts, &mut found);
                at.extend(found.into_iter().map(|(i, name)| (script.start + i, name)));
            }
        }
        Lang::Hash => {
            let parts = lex_hash(b);
            devtools(b, &parts, &mut at);
            drivers(b, &parts, &mut at);
        }
        Lang::Rust => {
            let parts = lex_rust(b);
            devtools(b, &parts, &mut at);
            drivers(b, &parts, &mut at);
            for (word, name) in [("eval", "eval"), ("new_no_args", "js_sys::Function"), ("new_with_args", "js_sys::Function")] {
                at.extend(words(b, &parts, word, code).filter(|&i| call_args(b, &parts, i + word.len()).is_some()).map(|i| (i, name)));
            }
            // the JavaScript each string carries (a page's scripts, when it
            // holds a page), found at its byte in the file
            for run in literals(&parts) {
                let Some((value, from)) = rust_string(b, run) else { continue };
                let whole = 0..value.len();
                let pieces = match value.to_ascii_lowercase().windows(7).any(|w| w == b"<script") {
                    true => scripts(&value),
                    false => vec![whole],
                };
                for piece in pieces {
                    let s = &value[piece.clone()];
                    let parts = lex_js(s);
                    let mut found = vec![];
                    drivers(s, &parts, &mut found);
                    javascript(s, &parts, &mut found);
                    at.extend(found.into_iter().map(|(i, name)| (from[piece.start + i], name)));
                }
            }
        }
    }
    let lines: BTreeSet<(usize, &'static str)> = at.into_iter().map(|(i, name)| (line_of(b, i), name)).collect();
    lines.into_iter().map(|(line, name)| Hit { path: file.path.clone(), line, name }).collect()
}

/// Fails on every refused use `allowed` does not name, and on an `allowed`
/// entry that names nothing found.
pub fn check(files: &[File], allowed: &[(&str, &str, &str)]) -> Result<()> {
    let mut used = vec![false; allowed.len()];
    let mut refused = vec![];
    for file in files {
        for hit in hits(file) {
            match allowed.iter().position(|(path, name, _)| *path == hit.path && *name == hit.name) {
                Some(i) => used[i] = true,
                None => refused.push(hit),
            }
        }
    }
    let why = |name: &str| REFUSED.iter().find(|(n, _)| *n == name).map_or("", |(_, why)| *why);
    let listed: Vec<String> = refused.iter().map(|h| format!("{}:{}: {} ({})", h.path, h.line, h.name, why(h.name))).collect();
    ensure!(
        listed.is_empty(),
        "runtime code runs a string as script (xtask/src/eval_sinks.rs), {} place(s): \"a check would be good so eval doesn't land in our runtime code\" (Paul, 2026-10-20), so do without it, or name the place in eval_sinks::ALLOWED with why it must:\n{}",
        listed.len(),
        listed.join("\n")
    );
    for (i, (path, name, why)) in allowed.iter().enumerate() {
        ensure!(used[i], "eval_sinks::ALLOWED names {name} in {path} ({why}), and it is not there: remove the entry");
    }
    Ok(())
}

// --- the rules, each over a source's bytes and what each byte is, adding (offset, name)

type Found = Vec<(usize, &'static str)>;

fn code(p: Part) -> bool {
    p == Part::Code
}

/// The DevTools names, in code and strings: a method is a string on the wire.
fn devtools(b: &[u8], parts: &[Part], out: &mut Found) {
    for name in DEVTOOLS {
        let hits = words(b, parts, name, |p| p != Part::Comment);
        // `Debugger.` is a domain: a method's name follows
        out.extend(hits.filter(|&i| !name.ends_with('.') || b.get(i + name.len()).is_some_and(|&c| is_ident(c))).map(|i| (i, name)));
    }
}

/// A page driver's evaluators, each a method called in code.
fn drivers(b: &[u8], parts: &[Part], out: &mut Found) {
    for name in DRIVERS {
        let method = name.trim_end_matches('(');
        out.extend(words(b, parts, method, code).filter(|&i| call_args(b, parts, i + method.len()).is_some()).map(|i| (i, name)));
    }
}

/// JavaScript's own ways to run a string, in its code.
fn javascript(b: &[u8], parts: &[Part], out: &mut Found) {
    out.extend(words(b, parts, "eval", code).map(|i| (i, "eval")));
    out.extend(words(b, parts, "Function", code).filter(|&i| call_args(b, parts, i + "Function".len()).is_some()).map(|i| (i, "Function(")));
    for timer in ["setTimeout", "setInterval"] {
        let string_first = |i: usize| call_args(b, parts, i + timer.len()).is_some_and(|p| string_at(b, parts, p).is_some());
        out.extend(words(b, parts, timer, code).filter(|&i| string_first(i)).map(|i| (i, timer)));
    }
    for i in words(b, parts, "import", code) {
        // `x.import(` is a method's; `import x from`, `import.meta` take no `(`
        let Some(p) = call_args(b, parts, i + "import".len()).filter(|_| before(b, parts, i) != Some(b'.')) else { continue };
        let fixed = string_at(b, parts, p).is_some_and(|s| {
            let alone = matches!(b.get(skip_space(b, parts, s.end)), Some(b')' | b','));
            let url = string_body(b, &s).to_ascii_lowercase();
            alone && !url.starts_with(b"data:") && !url.starts_with(b"blob:")
        });
        if !fixed {
            out.push((i, "import("));
        }
    }
    // a name to reach one by: `globalThis["eval"]`
    for run in literals(parts).filter(|r| matches!(b[r.start], b'"' | b'\'' | b'`')) {
        match string_body(b, &run) {
            b"eval" => out.push((run.start, "eval")),
            b"Function" => out.push((run.start, "Function(")),
            _ => {}
        }
    }
}

// --- reading: what each byte of a source is, as its language reads it

/// The text of each `<script>` in a page, or in a piece of one a Rust string
/// holds: to its `</script>`, or to the end when it does not close there. A
/// script by `src` has none.
fn scripts(page: &[u8]) -> Vec<Range<usize>> {
    // ASCII lowercase keeps byte offsets: they index `page` as they index `lower`
    let lower = page.to_ascii_lowercase();
    let mut out = vec![];
    let mut at = 0;
    // bounded: each pass moves `at` past a tag
    while let Some(open) = find(&lower, at, b"<script") {
        at = open + b"<script".len();
        // `<scripts-list>` is another element
        if lower.get(at).is_some_and(|&c| !(c == b'>' || c == b'/' || c.is_ascii_whitespace())) {
            continue;
        }
        let Some(body) = find(&lower, at, b">").map(|e| e + 1) else { break };
        let end = find(&lower, body, b"</script").unwrap_or(page.len());
        out.push(body..end);
        at = end;
    }
    out
}

/// What a byte of a source is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Code,
    /// a string's, a template's text, a regular expression's, a character's
    Literal,
    Comment,
}

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$' || c >= 0x80
}

/// Where `word` stands whole in `b`, each of its bytes in a part `keep`
/// takes: no identifier byte before it when it starts with one, nor after it
/// when it ends with one.
fn words<'a>(b: &'a [u8], parts: &'a [Part], word: &'a str, keep: fn(Part) -> bool) -> impl Iterator<Item = usize> + 'a {
    let w = word.as_bytes();
    let (starts, ends) = (is_ident(w[0]), is_ident(w[w.len() - 1]));
    // bounded: one pass over `b`
    (0..(b.len() + 1).saturating_sub(w.len())).filter(move |&i| {
        &b[i..i + w.len()] == w
            && (!starts || i == 0 || !is_ident(b[i - 1]))
            && (!ends || b.get(i + w.len()).is_none_or(|&c| !is_ident(c)))
            && parts[i..i + w.len()].iter().all(|&p| keep(p))
    })
}

/// The first byte from `i` on that is not a comment or code's space.
fn skip_space(b: &[u8], parts: &[Part], mut i: usize) -> usize {
    while i < b.len() && (parts[i] == Part::Comment || (parts[i] == Part::Code && b[i].is_ascii_whitespace())) {
        i += 1;
    }
    i
}

/// The code byte before `i`, past comments and space.
fn before(b: &[u8], parts: &[Part], i: usize) -> Option<u8> {
    (0..i).rev().find(|&k| parts[k] == Part::Literal || (parts[k] == Part::Code && !b[k].is_ascii_whitespace())).map(|k| b[k])
}

/// Where a call's first argument starts, when `(` follows `at` in code.
fn call_args(b: &[u8], parts: &[Part], at: usize) -> Option<usize> {
    let p = skip_space(b, parts, at);
    (b.get(p) == Some(&b'(') && parts[p] == Part::Code).then(|| skip_space(b, parts, p + 1))
}

/// The string literal that starts at `p`, if one does: up to a template's
/// first `${`.
fn string_at(b: &[u8], parts: &[Part], p: usize) -> Option<Range<usize>> {
    let quoted = p < b.len() && parts[p] == Part::Literal && matches!(b[p], b'"' | b'\'' | b'`');
    quoted.then(|| p..(p..b.len()).find(|&k| parts[k] != Part::Literal).unwrap_or(b.len()))
}

/// A quoted literal's text, between its quotes.
fn string_body<'a>(b: &'a [u8], run: &Range<usize>) -> &'a [u8] {
    let end = if run.end > run.start + 1 && b[run.end - 1] == b[run.start] { run.end - 1 } else { run.end };
    &b[run.start + 1..end]
}

/// Each run of literal bytes.
fn literals(parts: &[Part]) -> impl Iterator<Item = Range<usize>> + '_ {
    let starts = (0..parts.len()).filter(|&i| parts[i] == Part::Literal && (i == 0 || parts[i - 1] != Part::Literal));
    starts.map(|s| s..(s..parts.len()).find(|&k| parts[k] != Part::Literal).unwrap_or(parts.len()))
}

fn line_of(b: &[u8], i: usize) -> usize {
    1 + b[..i].iter().filter(|&&c| c == b'\n').count()
}

fn mark(parts: &mut [Part], range: Range<usize>, part: Part) {
    parts[range].fill(part);
}

/// The end of the line `i` is on (its newline is not included).
fn line_end(b: &[u8], i: usize) -> usize {
    b[i..].iter().position(|&c| c == b'\n').map_or(b.len(), |n| i + n)
}

fn find(b: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    b.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|n| from + n)
}

/// The end of a block comment opened at `i`; Rust's nest.
fn block_end(b: &[u8], i: usize, nests: bool) -> usize {
    let mut depth = 0usize;
    let mut j = i;
    // bounded: each pass moves `j` forward
    while j + 1 < b.len() {
        match (b[j], b[j + 1]) {
            (b'/', b'*') if nests || depth == 0 => {
                depth += 1;
                j += 2;
            }
            (b'*', b'/') => {
                depth -= 1;
                j += 2;
                if depth == 0 {
                    return j;
                }
            }
            _ => j += 1,
        }
    }
    b.len()
}

/// The end of a string opened by the quote at `i`: past its closing quote, or
/// at the newline that ends an unclosed one where a line ends strings.
fn quoted_end(b: &[u8], i: usize, quote: u8, ends_at_newline: bool) -> usize {
    let mut j = i + 1;
    // bounded: each pass moves `j` forward
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            c if c == quote => return j + 1,
            b'\n' if ends_at_newline => return j,
            _ => j += 1,
        }
    }
    b.len()
}

/// JavaScript: `//` and `/* */` comments; strings, templates (their `${…}`
/// code is code) and regular expressions as literals, a `/` taken for a
/// regular expression's where a value cannot stand before it.
///
/// Why keep this lexer (tooling review, 2026-10-30): Acorn 8.19.0 is maintained
/// and has zero transitive npm dependencies (ESLint 10.14.0 has 74), but we
/// also scan every decoded Rust string, including text and incomplete JS.
/// Strict parsing/tokenization rejects those; acorn-loose 8.5.2 (one
/// dependency, Acorn) guesses an AST and can discard a forbidden use: for
/// `/\neval(source);` it returns no eval identifier, while this scan finds it
/// after the broken regex's newline. A parser-only swap would lose that
/// conservative cross-language check; a fallback would retain this lexer
/// plus the Node adapter. The JS lexer and its two helpers are about 107
/// lines, not the whole mixed-language scanner. Replace when the adapter
/// shrinks that code and preserves the embedded-source fixtures too.
fn lex_js(b: &[u8]) -> Vec<Part> {
    let mut parts = vec![Part::Code; b.len()];
    // the brace depth to go back to as each open template's `${…}` closes
    let mut templates: Vec<usize> = vec![];
    let mut depth = 0usize;
    // a value just ended, so a `/` divides
    let mut value = false;
    let mut i = 0;
    // bounded: each pass moves `i` forward
    while i < b.len() {
        let (c, next) = (b[i], b.get(i + 1).copied());
        let start = i;
        if c == b'/' && next == Some(b'/') {
            i = line_end(b, i);
            mark(&mut parts, start..i, Part::Comment);
        } else if c == b'/' && next == Some(b'*') {
            i = block_end(b, i, false);
            mark(&mut parts, start..i, Part::Comment);
        } else if c == b'"' || c == b'\'' {
            i = quoted_end(b, i, c, true);
            mark(&mut parts, start..i, Part::Literal);
            value = true;
        } else if c == b'`' || (c == b'}' && depth == 0 && !templates.is_empty()) {
            if c == b'`' {
                parts[i] = Part::Literal;
            } else {
                depth = templates.pop().expect("a template is open");
            }
            let (end, opened) = template_text(b, i + 1, &mut parts);
            if opened {
                templates.push(depth);
                depth = 0;
            }
            value = !opened;
            i = end;
        } else if let Some(end) = (c == b'/' && !value).then(|| regex_end(b, i)).flatten() {
            mark(&mut parts, start..end, Part::Literal);
            value = true;
            i = end;
        } else if is_ident(c) {
            i = (i..b.len()).find(|&k| !is_ident(b[k])).unwrap_or(b.len());
            value = !REGEX_AFTER.contains(&&b[start..i]);
        } else {
            match c {
                b'{' => depth += 1,
                b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
            if !c.is_ascii_whitespace() {
                value = matches!(c, b')' | b']');
            }
            i += 1;
        }
    }
    parts
}

/// The words after which a `/` starts a regular expression.
const REGEX_AFTER: [&[u8]; 14] = [b"return", b"typeof", b"instanceof", b"in", b"of", b"new", b"delete", b"void", b"throw", b"case", b"do", b"else", b"yield", b"await"];

/// A template's text from `i`, marked: to past its closing backtick
/// (`false`), or past a `${` that opens code (`true`).
fn template_text(b: &[u8], mut i: usize, parts: &mut [Part]) -> (usize, bool) {
    // bounded: each pass moves `i` forward
    while i < b.len() {
        match b[i] {
            b'\\' => {
                let end = (i + 2).min(b.len());
                mark(parts, i..end, Part::Literal);
                i = end;
            }
            b'`' => {
                parts[i] = Part::Literal;
                return (i + 1, false);
            }
            b'$' if b.get(i + 1) == Some(&b'{') => return (i + 2, true),
            _ => {
                parts[i] = Part::Literal;
                i += 1;
            }
        }
    }
    (i, false)
}

/// The end of a regular expression opened at `i`, flags and all; `None` when
/// the line ends first (so it was a division).
fn regex_end(b: &[u8], i: usize) -> Option<usize> {
    let mut class = false;
    let mut j = i + 1;
    // bounded: each pass moves `j` forward
    while j < b.len() {
        match b[j] {
            b'\n' => return None,
            b'\\' => j += 1,
            b'[' => class = true,
            b']' => class = false,
            b'/' if !class => return Some((j + 1..b.len()).find(|&k| !is_ident(b[k])).unwrap_or(b.len())),
            _ => {}
        }
        j += 1;
    }
    None
}

/// Rust: `//` comments (doc comments too) and nested `/* */`; strings (raw,
/// byte and C strings too) and characters as literals, a lifetime as code.
fn lex_rust(b: &[u8]) -> Vec<Part> {
    let mut parts = vec![Part::Code; b.len()];
    let mut i = 0;
    // bounded: each pass moves `i` forward
    while i < b.len() {
        let (c, next) = (b[i], b.get(i + 1).copied());
        let start = i;
        if c == b'/' && next == Some(b'/') {
            i = line_end(b, i);
            mark(&mut parts, start..i, Part::Comment);
        } else if c == b'/' && next == Some(b'*') {
            i = block_end(b, i, true);
            mark(&mut parts, start..i, Part::Comment);
        } else if c == b'"' {
            i = quoted_end(b, i, b'"', false);
            mark(&mut parts, start..i, Part::Literal);
        } else if c == b'\'' {
            match char_end(b, i) {
                Some(end) => {
                    mark(&mut parts, start..end, Part::Literal);
                    i = end;
                }
                None => i += 1,
            }
        } else if is_ident(c) {
            let word_end = (i..b.len()).find(|&k| !is_ident(b[k])).unwrap_or(b.len());
            i = match prefixed_end(b, &b[start..word_end], word_end) {
                Some(end) => {
                    mark(&mut parts, start..end, Part::Literal);
                    end
                }
                None => word_end,
            };
        } else {
            i += 1;
        }
    }
    parts
}

/// A literal a word at its start prefixes, ending where it does: `r"…"`,
/// `r#"…"#`, `b"…"`, `br"…"`, `c"…"`, `cr"…"`, `b'…'`; `None` for a word
/// (`r#type` is a raw identifier).
fn prefixed_end(b: &[u8], word: &[u8], at: usize) -> Option<usize> {
    match word {
        b"r" | b"br" | b"cr" => {
            let hashes = b[at..].iter().take_while(|&&c| c == b'#').count();
            (b.get(at + hashes) == Some(&b'"')).then(|| {
                let close: Vec<u8> = std::iter::once(b'"').chain(std::iter::repeat_n(b'#', hashes)).collect();
                find(b, at + hashes + 1, &close).map_or(b.len(), |e| e + close.len())
            })
        }
        b"b" | b"c" if b.get(at) == Some(&b'"') => Some(quoted_end(b, at, b'"', false)),
        b"b" if b.get(at) == Some(&b'\'') => char_end(b, at),
        _ => None,
    }
}

/// The end of a character literal at `i` (`'a'`, `'"'`, `'\''`,
/// `'\u{1F600}'`); `None` for a lifetime or a label.
fn char_end(b: &[u8], i: usize) -> Option<usize> {
    let first = *b.get(i + 1)?;
    if first == b'\\' {
        // the escaped character, then up to `\u{10FFFF}`'s rest, to the quote
        return b.get(i + 3..)?.iter().take(10).position(|&c| c == b'\'').map(|n| i + 3 + n + 1);
    }
    let len = match first {
        0..0x80 => 1,
        0x80..0xE0 => 2,
        0xE0..0xF0 => 3,
        _ => 4,
    };
    (b.get(i + 1 + len) == Some(&b'\'')).then_some(i + 2 + len)
}

/// A Rust string literal's value, each byte with the offset in `b` it comes
/// from; `None` for a character literal.
fn rust_string(b: &[u8], run: Range<usize>) -> Option<(Vec<u8>, Vec<usize>)> {
    let lit = &b[run.clone()];
    let prefix = lit.iter().take_while(|c| c.is_ascii_alphabetic()).count();
    let hashes = lit[prefix..].iter().take_while(|&&c| c == b'#').count();
    if lit.get(prefix + hashes) != Some(&b'"') {
        return None;
    }
    let tail: Vec<u8> = std::iter::once(b'"').chain(std::iter::repeat_n(b'#', hashes)).collect();
    // an unclosed one runs to the end of the file
    let closed = lit.len() >= prefix + 2 * tail.len() && lit.ends_with(&tail);
    let body = run.start + prefix + tail.len()..if closed { run.end - tail.len() } else { run.end };
    if lit[..prefix].contains(&b'r') {
        return Some((b[body.clone()].to_vec(), body.collect()));
    }
    let (mut value, mut from) = (vec![], vec![]);
    let mut j = body.start;
    // bounded: each pass moves `j` forward
    while j < body.end {
        let (c, at) = (b[j], j);
        j += 1;
        if c != b'\\' || j >= body.end {
            value.push(c);
            from.push(at);
            continue;
        }
        let e = b[j];
        j += 1;
        let decoded: Vec<u8> = match e {
            b'n' => vec![b'\n'],
            b't' => vec![b'\t'],
            b'r' => vec![b'\r'],
            b'0' => vec![0],
            b'x' => {
                let hex = std::str::from_utf8(&b[j..(j + 2).min(body.end)]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
                j = (j + 2).min(body.end);
                hex.map_or(vec![], |h| vec![h])
            }
            b'u' => {
                let end = b[j..body.end].iter().position(|&c| c == b'}').map_or(body.end, |n| j + n + 1);
                let hex = std::str::from_utf8(&b[j..end]).ok().map(|h| h.trim_matches(['{', '}']));
                let ch = hex.and_then(|h| u32::from_str_radix(h, 16).ok()).and_then(char::from_u32);
                j = end;
                ch.map_or(vec![], |ch| ch.to_string().into_bytes())
            }
            // a line continued: its newline and the next line's indent are no part of the value
            b'\n' => {
                j = (j..body.end).find(|&k| !b[k].is_ascii_whitespace()).unwrap_or(body.end);
                vec![]
            }
            other => vec![other],
        };
        from.extend(std::iter::repeat_n(at, decoded.len()));
        value.extend(decoded);
    }
    Some((value, from))
}

/// `#` comments, and strings (Python's triple-quoted too) as literals:
/// Python, a shell script, a Dockerfile.
fn lex_hash(b: &[u8]) -> Vec<Part> {
    let mut parts = vec![Part::Code; b.len()];
    let mut i = 0;
    // bounded: each pass moves `i` forward
    while i < b.len() {
        let c = b[i];
        let start = i;
        if c == b'#' {
            i = line_end(b, i);
            mark(&mut parts, start..i, Part::Comment);
        } else if c == b'"' || c == b'\'' {
            let triple = [c; 3];
            i = match b[i..].starts_with(&triple) {
                true => find(b, i + 3, &triple).map_or(b.len(), |e| e + 3),
                false => quoted_end(b, i, c, true),
            };
            mark(&mut parts, start..i, Part::Literal);
        } else {
            i += 1;
        }
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use fragment_devstack as devstack;

    fn file(path: &str, text: &str) -> File {
        File { path: path.to_string(), lang: lang(path).expect("code"), text: text.to_string() }
    }

    fn found(path: &str, text: &str) -> Vec<(usize, &'static str)> {
        hits(&file(path, text)).into_iter().map(|h| (h.line, h.name)).collect()
    }

    /// Goal: a planted use in each category fails the check, named by its
    /// file, line and name, in each language it can be written in. Method:
    /// each beside clean code, alone and in a check of both.
    #[test]
    fn a_planted_eval_fails_the_check_naming_its_line() {
        let clean = file("cell/clean.mjs", "export const ok = 1;\n");
        let planted: [(&str, &str, usize, &str); 38] = [
            // DevTools, as a method on the wire and as a client's call
            ("cell/src/card.rs", "async fn f() {\n    cdp.call(\"Runtime.evaluate\", json!({ \"expression\": js }), deadline).await?;\n}\n", 2, "Runtime.evaluate"),
            ("crates/core/src/a.rs", "const M: &str = \"Runtime.callFunctionOn\";\n", 1, "callFunctionOn"),
            ("cell/src/b.rs", "let m = format!(\"{domain}.callFunctionOn\");\n", 1, "callFunctionOn"),
            ("cell/c.mjs", "await send(\"Runtime.compileScript\", { expression });\n", 1, "compileScript"),
            ("images/hermes/d.py", "ws.send(json.dumps({\"method\": \"Page.addScriptToEvaluateOnNewDocument\"}))\n", 1, "addScriptToEvaluateOnNewDocument"),
            ("images/bridge/src/e.rs", "let _ = \"Debugger.enable\";\n", 1, "Debugger."),
            ("cell/f.mjs", "await client.Debugger.evaluateOnCallFrame({ expression });\n", 1, "Debugger."),
            ("images/hermes/Dockerfile", "FROM x\nRUN python3 -c 'send(\"Runtime.evaluate\")'\n", 2, "Runtime.evaluate"),
            // a page driver's
            ("cell/g.mjs", "const title = await page.evaluate(() => document.title);\n", 1, ".evaluate("),
            ("cell/src/h.rs", "let v = tab.evaluate(\"1 + 1\", false)?;\n", 1, ".evaluate("),
            ("images/hermes/i.py", "page.evaluate(\"document.title\")\n", 1, ".evaluate("),
            ("templates/j.js", "await page\n  .evaluateOnNewDocument(f);\n", 2, ".evaluateOnNewDocument("),
            ("templates/k.js", "await context.addInitScript({ content });\n", 1, ".addInitScript("),
            ("templates/l.js", "const t = await page.$eval(\"h1\", (h) => h.textContent);\n", 1, ".$eval("),
            ("templates/m.js", "await page.$$eval (\"a\", f);\n", 1, ".$$eval("),
            ("templates/n.js", "await page.evaluateHandle(() => window);\n", 1, ".evaluateHandle("),
            // JavaScript's own
            ("cell/o.mjs", "ok();\neval(text);\n", 2, "eval"),
            ("cell/p.mjs", "eval (text);\n", 1, "eval"),
            ("cell/q.mjs", "(0, eval)(text);\n", 1, "eval"),
            ("cell/r.mjs", "globalThis.eval(text);\n", 1, "eval"),
            ("cell/s.mjs", "globalThis[\"eval\"](text);\n", 1, "eval"),
            ("cell/t.mjs", "const run = new Function(\"return 1\");\n", 1, "Function("),
            ("cell/u.mjs", "const self_ = Function(\"return this\")();\n", 1, "Function("),
            ("cell/v.mjs", "const run = new Function\n  (\"a\", body);\n", 1, "Function("),
            ("cell/w.mjs", "setTimeout(\"tick()\", 10);\n", 1, "setTimeout"),
            ("cell/x.mjs", "window.setInterval(\n  `tick()`, 10);\n", 1, "setInterval"),
            ("cell/y.mjs", "await import(name);\n", 1, "import("),
            ("cell/z.mjs", "await import(\"./\" + name);\n", 1, "import("),
            ("cell/aa.mjs", "await import(`./${name}.js`);\n", 1, "import("),
            ("cell/ab.mjs", "await import(\"data:text/javascript,export default 1\");\n", 1, "import("),
            ("cell/ac.mjs", "await import(URL.createObjectURL(new Blob([code])));\n", 1, "import("),
            ("cell/ad.mjs", "const s = `${eval(text)}`;\n", 1, "eval"),
            // a page's inline script, at its line in the page
            ("templates/x/site/index.html", "<!doctype html>\n<p>eval(x) is a word here</p>\n<script type=\"module\">\n  const a = 1;\n  eval(a);\n</script>\n", 5, "eval"),
            // the JavaScript a Rust string carries, and js_sys's evaluators
            ("cell/src/ae.rs", "const SCRIPT: &str = r#\"<script>\n  setTimeout(\"go()\", 1);\n</script>\"#;\n", 2, "setTimeout"),
            ("cell/src/af.rs", "let page = format!(\"<p>{x}</p>\\\n    <script>eval(\\\"{y}\\\")</script>\");\n", 2, "eval"),
            ("cell/src/ag.rs", "let v = js_sys::eval(&source)?;\n", 1, "eval"),
            ("cell/src/ah.rs", "let f = js_sys::Function::new_no_args(&body);\n", 1, "js_sys::Function"),
            // An arbitrary Rust string need not be valid JS. Acorn's strict
            // parser rejects this; its loose parser discards the eval.
            ("cell/src/ai.rs", "let fragment = r#\"/\neval(source);\"#;\n", 2, "eval"),
        ];
        check(std::slice::from_ref(&clean), &[]).expect("clean code passes");
        let names: BTreeSet<&str> = planted.iter().map(|(_, _, _, name)| *name).collect();
        assert_eq!(names, REFUSED.iter().map(|(name, _)| *name).collect(), "each refused name is planted, and each found is one REFUSED says why of");
        for (path, text, line, name) in planted {
            assert_eq!(found(path, text), [(line, name)], "{path}");
            let err = check(&[file("cell/clean.mjs", &clean.text), file(path, text)], &[]).expect_err(path).to_string();
            assert!(err.contains(&format!("{path}:{line}: {name} (")), "{err}");
            assert!(!err.contains("cell/clean.mjs"), "only the planted file is named: {err}");
        }
    }

    /// Goal: what only looks like a refused use passes: identifiers that
    /// contain one, comments that say the rule, the DevTools the cards use,
    /// strings that are data, and a literal module. Method: one file per
    /// language, each read as its language reads it.
    #[test]
    fn look_alikes_pass() {
        let clean = [
            (
                "cell/src/card.rs",
                "//! never Runtime.evaluate, Runtime.callFunctionOn or Debugger.enable\n\
                 /// the card refuses `page.evaluate(` and eval(\n\
                 /* a block /* nested: Runtime.evaluate */ still a comment: eval( */\n\
                 fn evaluate_rule<'a>(x: &'a str) -> &'a str { x }\n\
                 fn f() {\n\
                 \x20   let runtime_evaluate = 1;\n\
                 \x20   let q = '\"'; let r = \"Runtime.exceptionThrown\";\n\
                 \x20   for domain in [\"Page\", \"Runtime\", \"Log\"] { cdp.call(&format!(\"{domain}.enable\"), json!({})); }\n\
                 \x20   cdp.call(\"Page.navigate\", json!({ \"url\": u })); cdp.call(\"Page.captureScreenshot\", json!({}));\n\
                 \x20   let f: js_sys::Function = Reflect::get(obj, &method.into())?.into();\n\
                 \x20   let e = \"retrieval and medieval\"; let g = r#type; let h = \"https://a//b\";\n\
                 \x20   let i = MyDebugger.x; let j = self.evaluated(); let k = rule.evaluate_all(x);\n\
                 }\n",
            ),
            (
                "cell/shell/clean.js",
                "// never eval(text) or new Function(body), nor setTimeout(\"code\")\n\
                 /* nor import(name), nor Runtime.evaluate */\n\
                 import { a } from \"./a.mjs\";\n\
                 import \"./side.mjs\";\n\
                 const m = await import(\"./lazy.mjs\");\n\
                 const n = await import('./lazy.mjs', { with: { type: \"json\" } });\n\
                 const u = import.meta.url;\n\
                 retrieval(1); isFunction(x); evaluate_(y); const evaluator = 1;\n\
                 setTimeout(() => tick(), 10); setInterval(tick, 1000);\n\
                 const f = function (x) { return x; };\n\
                 if (x instanceof Function) Function.prototype.call.call(f);\n\
                 const s = \"eval(x) is only text\"; const t = 'new Function(\"a\")';\n\
                 const re = /\"eval\\(/g; const half = a / b / c; const url = \"https://x\"; // eval(\n\
                 const tpl = `eval(${a}) is text, ${\"Function(\" + b}`;\n\
                 x.import(name); obj.$evaluate(1); page.evaluated(); sum.evaluateLater;\n",
            ),
            ("templates/a/site/index.html", "<!doctype html>\n<!-- eval(x) -->\n<p>Runtime.evaluate</p>\n<script src=\"./app.js\"></script>\n<script>\n  // eval(x)\n  const a = 1;\n</script>\n"),
            ("images/hermes/clean.py", "# never Runtime.evaluate, page.evaluate( or eval(\nimport os\nprint(\"evaluate later\")\nx = \"#\" + 'no # comment'\n\"\"\"Docs: Debugger. ends a sentence.\"\"\"\n"),
        ];
        for (path, text) in clean {
            assert_eq!(found(path, text), [], "{path}");
        }
    }

    /// Goal: the walk reads what ships and nothing else, places every
    /// tracked directory, and refuses a stale entry or an unplaced one.
    /// Method: listed paths of each kind, then the lists checked against a
    /// tracked set with each entry, one missing, and one more directory.
    #[test]
    fn the_walk_reads_runtime_code_and_places_every_directory() {
        let paths = [
            "cell/src/card.rs",
            "cell/shell/shell.js",
            "cell/shell/vendor/split-grid.js",
            "cell/shell/style.css",
            "templates/notes/site/index.html",
            "images/hermes/Dockerfile",
            "images/hermes/preload.py",
            "images/bridge/tests/docker.rs",
            "cli/tests/mcp.rs",
            "crates/core/src/lib.rs",
            "crates/e2e/src/browser.rs",
            "xtask/src/main.rs",
            "docs/MODEL.md",
            "README.md",
        ];
        let selected = select(paths.iter().map(|p| p.to_string())).expect("a walk");
        let read: Vec<(&str, Lang)> = selected.iter().map(|(p, l)| (p.as_str(), *l)).collect();
        assert_eq!(
            read,
            [
                ("cell/src/card.rs", Lang::Rust),
                ("cell/shell/shell.js", Lang::JavaScript),
                ("templates/notes/site/index.html", Lang::Html),
                ("images/hermes/Dockerfile", Lang::Hash),
                ("images/hermes/preload.py", Lang::Hash),
                ("crates/core/src/lib.rs", Lang::Rust),
            ]
        );
        assert!(select(["cell/src/lib.rs".to_string()].into_iter()).is_err(), "a walk without the cards is broken");

        let every: BTreeSet<String> = RUNTIME.iter().chain(&NOT_RUNTIME).map(|(dir, _)| format!("{dir}x")).chain(["Cargo.toml".to_string()]).collect();
        placed(&every).expect("each entry names a tracked file, and every file is placed");
        let without = |gone: &str| every.iter().filter(|p| !p.starts_with(gone)).cloned().collect::<BTreeSet<_>>();
        assert!(placed(&without("crates/nip98/")).is_err(), "a RUNTIME entry with nothing tracked is stale");
        assert!(placed(&without(".cargo/")).is_err(), "a NOT_RUNTIME entry with nothing tracked is stale");
        for new in ["crates/new/src/lib.rs", "agent/index.mjs"] {
            let err = placed(&every.iter().cloned().chain([new.to_string()]).collect()).expect_err(new).to_string();
            assert!(err.contains(new), "{err}");
        }
    }

    /// Goal: an exception exempts its file's name and nothing else, and a
    /// stale one fails.
    #[test]
    fn an_exception_is_exact_and_never_stale() {
        let planted = || file("cell/src/card.rs", "let _ = \"Runtime.evaluate\";\n");
        check(&[planted()], &[("cell/src/card.rs", "Runtime.evaluate", "a reason")]).expect("allowed");
        assert!(check(&[planted()], &[("cell/src/card.rs", "callFunctionOn", "another name")]).is_err(), "another name's entry exempts nothing");
        assert!(check(&[planted()], &[("cell/src/other.rs", "Runtime.evaluate", "elsewhere")]).is_err(), "another file's entry exempts nothing");
        let err = check(&[file("cell/src/card.rs", "fn f() {}\n")], &[("cell/src/card.rs", "Runtime.evaluate", "a reason")]).expect_err("stale").to_string();
        assert!(err.contains("ALLOWED names Runtime.evaluate in cell/src/card.rs"), "{err}");
    }

    /// Goal: the repo's runtime code runs no string as script, and the walk
    /// reaches the cards, the Worker's entry, the shell, the templates, the
    /// images' bridge, boot, screens and Python, and the CLI; while the e2e's
    /// harness, which the walk leaves out, would be refused (the rules find
    /// its real uses).
    #[test]
    fn the_repo_runs_no_string_as_script() {
        let root = devstack::repo_root();
        let files = sources(&root).expect("the repo's runtime code");
        for path in [
            CARDS,
            "crates/core/src/card.rs",
            "cell/entry.mjs",
            "cell/shell/shell.js",
            "cell/src/share.rs",
            "templates/todo/app.mjs",
            "templates/notes/site/index.html",
            "images/bridge/src/main.rs",
            "images/hermes/boot/src/main.rs",
            "images/hermes/screen/index.html",
            "images/hermes/preload.py",
            "images/hermes/Dockerfile",
            "cli/src/main.rs",
        ] {
            assert!(files.iter().any(|f| f.path == path), "{path} is read");
        }
        assert!(files.iter().all(|f| !f.path.starts_with("crates/e2e/") && !f.path.contains("/tests/") && !crate::js_syntax::vendored(&f.path)));
        check(&files, &ALLOWED).expect("no runtime code runs a string as script");

        let harness = "crates/e2e/src/browser.rs";
        let text = std::fs::read_to_string(root.join(harness)).expect("the e2e's browser");
        let harness = hits(&File { path: harness.into(), lang: Lang::Rust, text });
        assert!(harness.iter().any(|h| h.name == "Runtime.evaluate"), "the e2e's own Runtime.evaluate is found: {harness:?}");
    }
}
