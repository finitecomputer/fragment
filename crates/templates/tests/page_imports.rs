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

/// A built name's own (`chat.3f9a2c1b4d5e.js` → `chat.js`): its hash out.
#[cfg(feature = "embed")]
fn unhashed(name: &str) -> String {
    name.split('.').filter(|p| !(p.len() == 12 && p.bytes().all(|b| b.is_ascii_hexdigit()))).collect::<Vec<_>>().join(".")
}

/// Goal: a built page (crates/templates/bundle.rs) comes in one wave and
/// is kept for good, and is otherwise its page as written (the chat's
/// early socket, the shell's metas). Method: each page build.rs builds,
/// from its release. Valid: each script and stylesheet of its own it
/// names is named by its hash (`site::hashed`, kept for good) and held by
/// its release; it preloads its bundles and the platform's modules they
/// import, nothing else; each bundle names its map, held too, whose
/// sources are repo paths; its hashes and preloads taken out, it is its
/// page as the repo holds it. Invalid: a page naming a source module, or
/// a file its release lacks.
#[cfg(feature = "embed")]
#[test]
fn a_built_page_names_its_hashed_files() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let pages: Vec<(&str, &str)> = fragment_templates::BUILT.iter().copied().filter(|(_, p)| p.ends_with("index.html")).collect();
    assert_eq!(pages.len(), 4, "the shell's page and the chat's three: {pages:?}");
    for (at, _) in pages {
        let (file, link, dir): (Box<dyn Fn(&str) -> Option<&'static fragment_templates::File>>, &str, String) = if at == "shell" {
            (Box::new(fragment_templates::shell), "/__shell/", "cell/shell/".into())
        } else {
            let t = fragment_templates::blessed::template(at).expect("a built template is blessed");
            (Box::new(move |p: &str| t.file(&format!("site/{p}"))), "./", format!("templates/{at}/site/"))
        };
        let page = std::str::from_utf8(file("index.html").expect("its page").bytes).unwrap();
        let open = format!("\"{link}");
        let named: Vec<&str> = page.match_indices(&open).map(|(i, _)| &page[i + open.len()..]).map(|r| &r[..r.find('"').unwrap()]).collect();
        let mut own: Vec<&str> = Vec::new();
        for n in named.iter().copied().filter(|n| !n.starts_with("__") && (n.ends_with(".js") || n.ends_with(".css"))) {
            if !own.contains(&n) {
                own.push(n);
            }
        }
        assert!(own.iter().any(|n| n.ends_with(".js")), "{at}: its page runs a bundle");
        for name in &own {
            assert!(fragment_core::site::hashed(name) && file(name).is_some(), "{at}: names {name}, which is no hashed file of its release");
        }
        let preload = format!("  <link rel=\"modulepreload\" href=\"{link}");
        let preloads: Vec<&str> = page.lines().filter_map(|l| l.strip_prefix(&preload)).map(|r| &r[..r.find('"').unwrap()]).collect();
        let bundles: Vec<&str> = own.iter().copied().filter(|n| n.ends_with(".js")).collect();
        assert_eq!(preloads.iter().filter(|p| !p.starts_with("__")).copied().collect::<Vec<_>>(), bundles, "{at}: preloads its bundles");
        let mut scripts = String::new();
        for bundle in &bundles {
            let js = std::str::from_utf8(file(bundle).unwrap().bytes).unwrap();
            scripts.push_str(js);
            let map = js.trim_end().rsplit("//# sourceMappingURL=").next().unwrap();
            let map: serde_json::Value = serde_json::from_slice(file(map).unwrap_or_else(|| panic!("{at}: {bundle}'s map {map}")).bytes).unwrap();
            let sources = map["sources"].as_array().unwrap();
            assert!(!sources.is_empty() && sources.iter().all(|s| s.as_str().is_some_and(|s| root.join(s).is_file())), "{at}: {bundle}'s sources: {sources:?}");
        }
        for platform in preloads.iter().filter(|p| p.starts_with("__")) {
            assert!(scripts.contains(&format!("\"./{platform}\"")), "{at}: preloads {platform}, which no bundle imports");
        }
        let mut written = page.lines().filter(|l| !l.starts_with(&preload)).map(|l| format!("{l}\n")).collect::<String>();
        for name in &own {
            written = written.replace(&format!("{open}{name}\""), &format!("{open}{}\"", unhashed(name)));
        }
        let source = std::fs::read_to_string(root.join(&dir).join("index.html")).unwrap();
        assert_eq!(written, source, "{at}: its built page is its page, but for its hashes and preloads");
    }
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
