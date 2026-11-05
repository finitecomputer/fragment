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
        for f in t.files.iter().filter(|f| f.path.starts_with("site/") && !fragment_templates::built(t.name, f.path) && [".js", ".mjs", ".html"].iter().any(|x| f.path.ends_with(x))) {
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
#[cfg(feature = "embed")]
fn preloads(page: &str) -> Vec<&str> {
    let open = "<link rel=\"modulepreload\" href=\"";
    page.match_indices(open).map(|(at, _)| &page[at + open.len()..]).map(|rest| &rest[..rest.find('"').unwrap_or(0)]).collect()
}

/// Goal: a built page (crates/templates/bundle.rs) comes in one wave and
/// is kept: it names its bundle and stylesheets by their hash, each a file
/// its release holds, and preloads its bundle and the platform's modules
/// the bundle imports (their names fixed), nothing else; its bundle names
/// its map, whose sources are repo paths. Invalid: a page that names a
/// source module, or a file the release lacks.
#[cfg(feature = "embed")]
#[test]
fn a_built_page_names_its_hashed_files() {
    let chat = fragment_templates::blessed::template("chat").expect("chat is blessed");
    let page = std::str::from_utf8(chat.file("site/index.html").unwrap().bytes).unwrap();
    let preloaded = preloads(page);
    assert_eq!(preloaded.len(), 3, "the bundle and the two platform modules: {preloaded:?}");
    let bundle = preloaded[0].trim_start_matches("./");
    assert!(bundle.starts_with("chat.") && bundle.ends_with(".js") && bundle != "chat.js", "{bundle}");
    assert_eq!(&preloaded[1..], ["./__fragment.js", "./__markdown.js"]);
    assert!(page.contains(&format!("from \"./{bundle}\"")) && !page.contains("\"./chat.js\""), "the page runs its bundle");
    let js = std::str::from_utf8(chat.file(&format!("site/{bundle}")).expect("the release holds the bundle").bytes).unwrap();
    let map = js.rsplit("//# sourceMappingURL=").next().unwrap().trim();
    let map = chat.file(&format!("site/{map}")).expect("and its map");
    let sources: serde_json::Value = serde_json::from_slice(map.bytes).unwrap();
    assert!(sources["sources"].as_array().unwrap().iter().all(|s| s.as_str().unwrap().starts_with("templates/")), "{}", sources["sources"]);
    let css = page.split("<link rel=\"stylesheet\" href=\"./").nth(1).map(|r| &r[..r.find('"').unwrap()]);
    assert!(css.is_some_and(|c| c != "chat.css" && chat.file(&format!("site/{c}")).is_some()), "its stylesheet by its hash: {css:?}");
    let shell = std::str::from_utf8(fragment_templates::shell("index.html").unwrap().bytes).unwrap();
    for name in ["shell.js", "shell.css", "platform-pages.css"] {
        assert!(!shell.contains(&format!("/__shell/{name}\"")), "the shell's page names {name} by its hash");
    }
    let shell_bundle = preloads(shell)[0].trim_start_matches("/__shell/");
    assert!(fragment_templates::shell_built(shell_bundle) && fragment_templates::shell_published(shell_bundle).is_some(), "{shell_bundle}");
    assert!(!fragment_templates::shell_built("shell.js") && !fragment_templates::shell_built("index.html"));
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
