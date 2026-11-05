//! Goal: a page's modules all load where it is served. A template that
//! links another's site files (the Telegram connector's page is the chat's,
//! the Session Archive's renders as the chat does) serves every module those
//! import, so a module the chat gains (`control.js`) is never a page there
//! that does not start. Method: each relative import in each page file, a
//! script's or its page's, names a file the template holds; the platform's
//! own (`__*`) aside. A test, out of src/, as it reads import syntax from
//! strings (xtask/src/eval_sinks.rs reads src/ as runtime code).
#![cfg(feature = "platform")]

use std::path::Path;

/// The relative module specifiers `source` imports.
fn imports(source: &str) -> Vec<&str> {
    let mut found = Vec::new();
    for open in ["from \"", "from '", "import(\"", "import('", "import \"", "import '"] {
        let close = open.chars().last().unwrap();
        for (at, _) in source.match_indices(open) {
            let rest = &source[at + open.len()..];
            let spec = &rest[..rest.find(close).unwrap_or(0)];
            if spec.starts_with("./") || spec.starts_with("../") {
                found.push(spec);
            }
        }
    }
    found
}

#[test]
fn every_module_a_page_imports_is_its_own() {
    let templates = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../templates");
    let mut checked = 0;
    for t in fragment_templates::ALL {
        for f in t.files.iter().filter(|f| f.path.starts_with("site/") && [".js", ".mjs", ".html"].iter().any(|x| f.path.ends_with(x))) {
            let source = std::fs::read_to_string(templates.join(t.name).join(f.path)).unwrap_or_else(|e| panic!("{}/{}: {e}", t.name, f.path));
            for spec in imports(&source) {
                let mut path: Vec<&str> = f.path.split('/').collect();
                path.pop();
                for part in spec.split('/') {
                    match part {
                        "." => {}
                        ".." => assert!(path.pop().is_some(), "{}/{}: {spec} leaves its template", t.name, f.path),
                        part => path.push(part),
                    }
                }
                if path.last().is_some_and(|p| p.starts_with("__")) {
                    continue;
                }
                let path = path.join("/");
                assert!(t.file(&path).is_some(), "{}/{} imports {spec}, which {} does not serve: link it (as its other files are)", t.name, f.path, t.name);
                checked += 1;
            }
        }
    }
    assert!(checked > 20, "the pages' imports were read ({checked})");
}

/// The `href`s of a page's `<link rel="modulepreload">`s.
fn preloads(page: &str) -> Vec<&str> {
    let open = "<link rel=\"modulepreload\" href=\"";
    page.match_indices(open).map(|(at, _)| &page[at + open.len()..]).map(|rest| &rest[..rest.find('"').unwrap_or(0)]).collect()
}

/// Goal: the chat's modules all come at once, not a wave of imports after
/// each module that names them (three waves, each a round trip: the
/// chat-open bench, 2026-11-04). Method: the chat's page preloads
/// (`modulepreload`) each module it runs; the modules its page imports,
/// and theirs, read from the template's files (the platform's own, `__*`,
/// import nothing). Valid: the two are the same, none twice.
#[test]
fn the_chat_preloads_its_whole_module_graph() {
    let site = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../templates/chat/site");
    let page = std::fs::read_to_string(site.join("index.html")).expect("the chat's page");
    let preloaded = preloads(&page);
    let mut graph = std::collections::BTreeSet::new();
    let mut next: Vec<String> = imports(&page).into_iter().map(str::to_string).collect();
    while let Some(spec) = next.pop() {
        assert!(spec.starts_with("./") && !spec[2..].contains('/'), "the chat's modules sit beside its page: {spec}");
        if !graph.insert(spec.clone()) || spec.starts_with("./__") {
            continue;
        }
        let source = std::fs::read_to_string(site.join(&spec[2..])).unwrap_or_else(|e| panic!("{spec}: {e}"));
        next.extend(imports(&source).into_iter().map(str::to_string));
    }
    let listed: std::collections::BTreeSet<String> = preloaded.iter().map(|s| s.to_string()).collect();
    assert_eq!(listed.len(), preloaded.len(), "each module preloaded once: {preloaded:?}");
    assert!(graph.contains("./__markdown.js") && graph.len() > 8, "the chat's graph was read: {graph:?}");
    assert_eq!(listed, graph, "the chat's page preloads exactly the modules it runs");
}

/// Goal: the chat page's early socket, unclaimed, gives up when the
/// client's own first dial would (`OPEN_WAIT_MS` and its first backoff,
/// cell/client.mjs), never sooner. Method: the inline script's `WAIT`, and
/// the client's two, read from their files. Valid: equal.
#[test]
fn the_early_socket_waits_as_the_client_does() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let read = |p: &str| std::fs::read_to_string(root.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
    let number = |text: &str, after: &str| -> u64 {
        let at = text.find(after).unwrap_or_else(|| panic!("{after:?} is there")) + after.len();
        text[at..].chars().take_while(|c| c.is_ascii_digit() || *c == '_').filter(char::is_ascii_digit).collect::<String>().parse().expect("a number")
    };
    let page = read("templates/chat/site/index.html");
    let client = read("cell/client.mjs");
    let wait = number(&page, "const WAIT=");
    let (open, backoff) = (number(&client, "const OPEN_WAIT_MS = "), number(&client, "let backoff = "));
    assert_eq!(wait, open + backoff, "the early socket's deadline is the client's opening wait and its first backoff");
}
