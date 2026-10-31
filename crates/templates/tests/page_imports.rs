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
