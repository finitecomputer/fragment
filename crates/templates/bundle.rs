// The pages the release builds rather than serves as written: the shell's
// (cell/shell/index.html) and the blessed chat's
// (templates/chat/site/index.html). Each page's script is bundled by
// esbuild into one minified module, beside its source map, and every file
// the page links is named by its bytes' hash (`chat.3f9a2c1b4d5e.js`), so
// a browser keeps it for good and never asks again (a hashed name is
// immutable: crates/core/src/site.rs `cache_control`, cell/src/shell.rs
// `asset`). The page itself is rewritten to name them, and preloads the
// bundle and the platform's own modules it imports (`__fragment.js`,
// `__markdown.js`, kept at their fixed names: people's own pages import
// them, docs/published-fragments.md).
//
// Why a build step (AGENTS.md welcomes one that buys simplification): a
// cold open fetched a page's modules a wave of imports at a time, and every
// reload asked again for each of them (the chat-open bench, 2026-11-04).
// A bundle is one request, and a hashed name none.
//
// esbuild is the one package.json pins, as the pinned npm installed it
// (docs/pinned-node.md): its native binary, run directly (it needs no
// Node). Its version is checked against the pin.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

use sha2::{Digest, Sha256};

/// A page the build makes.
pub struct Page {
    /// Where its files sit (cell/shell, templates/chat/site).
    pub dir: PathBuf,
    /// How the page names a file beside it (`/__shell/`, `./`).
    pub link: &'static str,
    /// Its script, bundled; the page names it once, as `{link}{entry}"`.
    pub entry: &'static str,
    /// Its stylesheets, as written but named by their hash; the page names
    /// each once, as `{link}{style}"`.
    pub styles: &'static [&'static str],
}

/// A file a page's build made, or names anew: its path beside the page,
/// and where its bytes are.
pub struct Built {
    pub rel: String,
    pub path: PathBuf,
}

/// A name with its bytes' hash in it: `shell.js` → `shell.<12 hex>.js`
/// (crates/core/src/site.rs `hashed` takes 8 hex digits or more).
fn hashed_name(name: &str, bytes: &[u8]) -> String {
    let hash = &hex::encode(Sha256::digest(bytes))[..12];
    match name.split_once('.') {
        Some((stem, ext)) => format!("{stem}.{hash}.{ext}"),
        None => format!("{name}.{hash}"),
    }
}

/// esbuild as npm installed it from package.json's pin: its native binary
/// for the machine this build runs on, its version checked.
fn esbuild(repo: &Path) -> PathBuf {
    let platform = match (env::consts::OS, env::consts::ARCH) {
        ("linux", "x86_64") => "linux-x64",
        ("linux", "aarch64") => "linux-arm64",
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", "x86_64") => "darwin-x64",
        (os, arch) => panic!("no esbuild for {os}/{arch}: the shell's and the chat's pages are built with it"),
    };
    let bin = repo.join("node_modules/@esbuild").join(platform).join("bin/esbuild");
    println!("cargo:rerun-if-changed={}", bin.display());
    println!("cargo:rerun-if-changed={}", repo.join("package.json").display());
    let pin = fs::read_to_string(repo.join("package.json")).expect("package.json");
    let pin = pin.split("\"esbuild\": \"").nth(1).and_then(|r| r.split('"').next()).expect("package.json pins esbuild");
    let out = Command::new(&bin).arg("--version").output().unwrap_or_else(|e| {
        panic!("no esbuild at {} ({e}): `cargo xtask build` (or dev, e2e) installs node_modules with the pinned npm", bin.display())
    });
    let version = String::from_utf8_lossy(&out.stdout);
    assert_eq!(version.trim(), pin, "{} is esbuild {}, not package.json's {pin}: node_modules is stale (the next xtask run installs it again)", bin.display(), version.trim());
    bin
}

/// A source map's sources named by their path in the repo
/// (`cell/shell/shell.js`), not from wherever the build wrote it: the same
/// bytes from every build directory (the cell's index and xtask's assets
/// are two builds, which must name one file).
fn repo_sources(map: &[u8], at: &Path, repo: &Path) -> Vec<u8> {
    let mut map: serde_json::Value = serde_json::from_slice(map).expect("esbuild's source map is JSON");
    let sources = map["sources"].as_array_mut().expect("a source map names its sources");
    for s in sources.iter_mut() {
        let rel = s.as_str().expect("a source is a path");
        let real = fs::canonicalize(at.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
        let path = real.strip_prefix(repo).unwrap_or_else(|_| panic!("{} is outside the repo", real.display()));
        *s = path.to_string_lossy().replace('\\', "/").into();
    }
    serde_json::to_vec(&map).expect("a source map serializes")
}

/// The relative imports of the platform's own modules (`./__*.js`) a
/// minified bundle keeps: what its page preloads beside it.
fn platform_imports(js: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for open in ["from\"./__", "import\"./__"] {
        for (at, _) in js.match_indices(open) {
            let rest = &js[at + open.len() - 4..];
            let spec = &rest[..rest.find('"').expect("an import's specifier ends")];
            if !found.iter().any(|f| f == spec) {
                found.push(spec.to_string());
            }
        }
    }
    found.sort();
    found
}

/// `text` with `from` replaced by `to`, where `from` is in it exactly once.
fn replace_once(text: &str, from: &str, to: &str, what: &str) -> String {
    assert_eq!(text.matches(from).count(), 1, "{what} names {from:?} once");
    text.replacen(from, to, 1)
}

/// Builds `page` into `out`: its page, its bundle and its map, and its
/// stylesheets by their hashed names (their bytes as written).
pub fn build(page: &Page, repo: &Path, out: &Path) -> Vec<Built> {
    let esbuild = esbuild(repo);
    fs::create_dir_all(out).expect("the build's directory");
    let entry = page.dir.join(page.entry);
    let raw = out.join(page.entry);
    let status = Command::new(&esbuild)
        .arg(&entry)
        .args(["--bundle", "--format=esm", "--minify", "--sourcemap=external", "--charset=utf8", "--log-level=warning", "--external:./__*"])
        .arg(format!("--outfile={}", raw.display()))
        .status()
        .unwrap_or_else(|e| panic!("run {}: {e}", esbuild.display()));
    assert!(status.success(), "esbuild {}: {status}", entry.display());
    let map = repo_sources(&fs::read(raw.with_extension("js.map")).expect("esbuild's map"), out, repo);
    let map_name = hashed_name(&format!("{}.map", page.entry), &map);
    let mut js = fs::read_to_string(&raw).expect("esbuild's bundle");
    js.push_str(&format!("//# sourceMappingURL={map_name}\n"));
    let js_name = hashed_name(page.entry, js.as_bytes());
    let mut built = Vec::new();
    for (name, bytes) in [(&map_name, map), (&js_name, js.clone().into_bytes())] {
        let path = out.join(name);
        fs::write(&path, bytes).expect("write a built file");
        built.push(Built { rel: name.clone(), path });
    }
    let html = page.dir.join("index.html");
    let mut text = fs::read_to_string(&html).unwrap_or_else(|e| panic!("{}: {e}", html.display()));
    let what = html.display().to_string();
    text = replace_once(&text, &format!("{}{}\"", page.link, page.entry), &format!("{}{js_name}\"", page.link), &what);
    for style in page.styles {
        let source = page.dir.join(style);
        let name = hashed_name(style, &fs::read(&source).unwrap_or_else(|e| panic!("{}: {e}", source.display())));
        text = replace_once(&text, &format!("{}{style}\"", page.link), &format!("{}{name}\"", page.link), &what);
        built.push(Built { rel: name, path: source });
    }
    let preloads: String = [js_name.clone()]
        .into_iter()
        .chain(platform_imports(&js).into_iter().map(|s| s.trim_start_matches("./").to_string()))
        .map(|name| format!("  <link rel=\"modulepreload\" href=\"{}{name}\">\n", page.link))
        .collect();
    text = replace_once(&text, "</head>", &format!("{preloads}</head>"), &what);
    let page_out = out.join("index.html");
    fs::write(&page_out, text).expect("write the built page");
    built.push(Built { rel: "index.html".into(), path: page_out });
    fs::remove_file(&raw).ok();
    fs::remove_file(raw.with_extension("js.map")).ok();
    built.sort_by(|a, b| a.rel.cmp(&b.rel));
    built
}
