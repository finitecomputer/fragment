// The pages the release builds rather than serves as written (build.rs
// `PAGES`): the shell's (cell/shell/index.html), and the blessed chat's and
// the two that run the chat's modules, the Telegram connector's and the
// Session Archive's (templates/{chat,telegram,sessions}/site/index.html).
// Nothing here is any one page's: each script a page names of its own is
// bundled by esbuild into one minified module with its source map beside
// it, and each script and stylesheet it names is named by its
// bytes' hash (`chat.3f9a2c1b4d5e.js`), so a browser keeps it for good and
// never asks again: a hashed name is immutable (crates/core/src/site.rs
// `cache_control`, cell/src/shell.rs `asset`). The page is rewritten to
// name them, and preloads its bundles and the platform's own modules they
// import (`./__fragment.js`, `./__markdown.js`), which keep their fixed
// names and caching: people's own pages import them
// (docs/published-fragments.md).
//
// Why a build step (AGENTS.md welcomes one that buys a simplification): a
// cold open fetched a page's modules a wave of imports at a time, and every
// reload asked again for each of them; bundled and hashed, a shell reload
// asks the network 23 times, not 70 (the bench in the pull request that
// made this, 2026-11-04). It replaced a hand-kept preload list.
//
// esbuild is the one package.json pins, installed by the pinned npm
// (docs/pinned-node.md): this build fetches the pinned Node and runs its
// `npm ci` when node_modules is missing or stale, as xtask does
// (fragment_devstack::node), so a fresh clone builds with no step first.
// Its native binary runs directly, its version checked against the pin.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

use sha2::{Digest, Sha256};

/// A page the build makes: its scripts and stylesheets are the files of
/// its own it names, each as `"{link}<name>.js"` or `"{link}<name>.css"`
/// (a module it imports or loads, a stylesheet it links), read from the
/// page, so a page is built by naming it alone (build.rs `PAGES`). The
/// platform's modules (`__*`) are not its own: they keep their names.
pub struct Page {
    /// Where it and its files sit (cell/shell, templates/chat/site).
    pub dir: PathBuf,
    /// How the page names a file beside it (`/__shell/`, `./`).
    pub link: &'static str,
}

/// A file a page's build made, or names anew: its path beside the page,
/// and where its bytes are.
pub struct Built {
    pub rel: String,
    pub path: PathBuf,
}

/// What a page's build made.
pub struct Made {
    /// Its page, bundles and maps, and its stylesheets' hashed names.
    pub files: Vec<Built>,
    /// Each script it names, and its bundle's name.
    bundles: Vec<(String, String)>,
    /// Every source its bundles hold, by its real path.
    pub inputs: BTreeSet<PathBuf>,
}

/// esbuild, as the pinned npm installed it from package.json's pin.
pub struct Esbuild {
    bin: PathBuf,
}

/// The most a page's bundles may weigh, all of them, raw: the shell's was
/// 240 KB when this was made. Past it, the build fails, and a bigger page
/// is a choice made here.
const PAGE_BUNDLES_MAX_BYTES: usize = 512 * 1024;

impl Esbuild {
    /// The pinned Node (fetched on first use), node_modules (its `npm ci`
    /// when missing or stale), and esbuild's native binary for the machine
    /// this build runs on, its version package.json's.
    pub fn installed(repo: &Path) -> Esbuild {
        for f in ["package.json", "package-lock.json"] {
            println!("cargo:rerun-if-changed={}", repo.join(f).display());
        }
        let tools = repo.join(fragment_devstack::TOOLS_DIR);
        let node = fragment_devstack::node::locate(&tools).unwrap_or_else(|e| panic!("the pinned Node, whose npm installs esbuild: {e}"));
        fragment_devstack::node::ensure_modules(&node, repo, &tools, &repo.join(fragment_devstack::CACHE_DIR))
            .unwrap_or_else(|e| panic!("node_modules, which holds esbuild: {e}"));
        let platform = match (env::consts::OS, env::consts::ARCH) {
            ("linux", "x86_64") => "linux-x64",
            ("linux", "aarch64") => "linux-arm64",
            ("macos", "aarch64") => "darwin-arm64",
            ("macos", "x86_64") => "darwin-x64",
            (os, arch) => panic!("no esbuild for {os}/{arch}: the platform's pages are built with it"),
        };
        let bin = repo.join("node_modules/@esbuild").join(platform).join("bin/esbuild");
        println!("cargo:rerun-if-changed={}", bin.display());
        let package = fs::read_to_string(repo.join("package.json")).expect("package.json");
        let pin = package.split("\"esbuild\": \"").nth(1).and_then(|r| r.split('"').next()).expect("package.json pins esbuild");
        let out = Command::new(&bin).arg("--version").output().unwrap_or_else(|e| panic!("run {} ({e})", bin.display()));
        let version = String::from_utf8_lossy(&out.stdout);
        assert_eq!(version.trim(), pin, "{} is esbuild {}, not package.json's {pin}", bin.display(), version.trim());
        Esbuild { bin }
    }
}

/// A name with its bytes' hash in it: `shell.js` → `shell.<12 hex>.js`,
/// more digits only where the 12 lack a digit or a letter: the platform's
/// own form, which crates/core/src/site.rs `hashed` keeps for good.
fn hashed_name(name: &str, bytes: &[u8]) -> String {
    let hex = hex::encode(Sha256::digest(bytes));
    let fits = |h: &str| h.bytes().any(|b| b.is_ascii_digit()) && h.bytes().any(|b| b.is_ascii_alphabetic());
    let hash = (12..=hex.len()).map(|n| &hex[..n]).find(|h| fits(h)).expect("a SHA-256 holds a digit and a letter");
    match name.split_once('.') {
        Some((stem, ext)) => format!("{stem}.{hash}.{ext}"),
        None => format!("{name}.{hash}"),
    }
}

/// A path esbuild names, relative to the repo (its working directory),
/// as its real path.
fn real(repo: &Path, path: &str) -> PathBuf {
    fs::canonicalize(repo.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// A source map's sources named by their path in the repo
/// (`cell/shell/shell.js`), not from wherever the build wrote it: the same
/// bytes from every build directory (the cell's index and xtask's assets
/// are two builds, which must name one file).
fn repo_sources(map: &[u8], at: &Path, repo: &Path) -> Vec<u8> {
    let mut map: serde_json::Value = serde_json::from_slice(map).expect("esbuild's source map is JSON");
    let sources = map["sources"].as_array_mut().expect("a source map names its sources");
    for s in sources.iter_mut() {
        let path = fs::canonicalize(at.join(s.as_str().expect("a source is a path"))).expect("a source of the map");
        let rel = path.strip_prefix(repo).unwrap_or_else(|_| panic!("{} is outside the repo", path.display()));
        *s = rel.to_string_lossy().replace('\\', "/").into();
    }
    serde_json::to_vec(&map).expect("a source map serializes")
}

/// The files of its own `page` names, each once, in the order it first
/// names them: `"{link}<name>"`, a name of no folder ending in `.js` or
/// `.css`, not the platform's (`__*`).
fn own_files(text: &str, link: &str) -> (Vec<String>, Vec<String>) {
    let open = format!("\"{link}");
    let (mut scripts, mut styles) = (Vec::new(), Vec::new());
    for (at, _) in text.match_indices(&open) {
        let rest = &text[at + open.len()..];
        let Some(name) = rest.find('"').map(|end| &rest[..end]) else { continue };
        if name.starts_with("__") || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)) {
            continue;
        }
        let list = if name.ends_with(".js") {
            &mut scripts
        } else if name.ends_with(".css") {
            &mut styles
        } else {
            continue;
        };
        if !list.iter().any(|n| n == name) {
            list.push(name.to_string());
        }
    }
    (scripts, styles)
}

/// Builds `page` into `out`: a bundle and its map for each script it
/// names, its stylesheets by their hashed names (their bytes as written),
/// and its page naming them.
pub fn build(esbuild: &Esbuild, page: &Page, repo: &Path, out: &Path) -> Made {
    let html = page.dir.join("index.html");
    let what = html.display().to_string();
    let mut text = fs::read_to_string(&html).unwrap_or_else(|e| panic!("{what}: {e}"));
    let (entries, styles) = own_files(&text, page.link);
    assert!(!entries.is_empty(), "{what} names no script of its own to bundle");
    let _ = fs::remove_dir_all(out);
    fs::create_dir_all(out).expect("the build's directory");
    let meta_path = out.join("meta.json");
    let status = Command::new(&esbuild.bin)
        .current_dir(repo)
        .args(entries.iter().map(|e| page.dir.join(e)))
        .args(["--bundle", "--format=esm", "--minify", "--sourcemap=external", "--charset=utf8", "--log-level=warning", "--external:./__*", "--entry-names=[name]"])
        .arg(format!("--outdir={}", out.join("raw").display()))
        .arg(format!("--metafile={}", meta_path.display()))
        .status()
        .unwrap_or_else(|e| panic!("run {}: {e}", esbuild.bin.display()));
    assert!(status.success(), "esbuild, {what}'s scripts: {status}");
    let meta: serde_json::Value = serde_json::from_slice(&fs::read(&meta_path).expect("esbuild's metafile")).expect("its metafile is JSON");
    let mut made = Made { files: vec![], bundles: vec![], inputs: BTreeSet::new() };
    let mut externals = BTreeSet::new();
    let mut weight = 0;
    for entry in &entries {
        let stem = entry.strip_suffix(".js").expect("an entry is a .js file");
        let raw = out.join("raw").join(format!("{stem}.js"));
        let raw_real = fs::canonicalize(&raw).expect("esbuild's bundle");
        let (_, output) = meta["outputs"]
            .as_object()
            .expect("its outputs")
            .iter()
            .find(|(path, _)| real(repo, path) == raw_real)
            .unwrap_or_else(|| panic!("esbuild's metafile names {entry}'s bundle"));
        let inputs: BTreeSet<PathBuf> = output["inputs"].as_object().expect("a bundle's inputs").keys().map(|p| real(repo, p)).collect();
        if let Some(shared) = made.inputs.intersection(&inputs).next() {
            panic!("{what}'s scripts both hold {}: each bundle would run its own copy (one entry, importing both, runs one)", shared.display());
        }
        made.inputs.extend(inputs);
        for import in output["imports"].as_array().into_iter().flatten().filter(|i| i["external"] == true) {
            externals.insert(import["path"].as_str().expect("an import names its path").to_string());
        }
        let map = repo_sources(&fs::read(out.join("raw").join(format!("{stem}.js.map"))).expect("esbuild's map"), &out.join("raw"), repo);
        let map_name = hashed_name(&format!("{entry}.map"), &map);
        let mut js = fs::read_to_string(&raw).expect("esbuild's bundle");
        js.push_str(&format!("//# sourceMappingURL={map_name}\n"));
        weight += js.len();
        let js_name = hashed_name(entry, js.as_bytes());
        for (name, bytes) in [(&map_name, map), (&js_name, js.into_bytes())] {
            let path = out.join(name);
            fs::write(&path, bytes).expect("write a built file");
            made.files.push(Built { rel: name.clone(), path });
        }
        made.bundles.push((entry.to_string(), js_name));
    }
    let _ = fs::remove_dir_all(out.join("raw"));
    assert!(weight <= PAGE_BUNDLES_MAX_BYTES, "{what}'s bundles are {weight} bytes, past {PAGE_BUNDLES_MAX_BYTES}");
    assert!(externals.iter().all(|e| e.starts_with("./__") && !e[2..].contains('/')), "a bundle imports nothing of its own outside it: {externals:?}");
    let mut renamed: Vec<(String, String)> = made.bundles.clone();
    for style in &styles {
        let source = page.dir.join(style);
        let name = hashed_name(style, &fs::read(&source).unwrap_or_else(|e| panic!("{}: {e}", source.display())));
        made.files.push(Built { rel: name.clone(), path: source });
        renamed.push((style.clone(), name));
    }
    for (from, to) in &renamed {
        text = text.replace(&format!("\"{}{from}\"", page.link), &format!("\"{}{to}\"", page.link));
    }
    // its bundles first, then the platform's modules they import, all asked at once
    let preloads: String = made
        .bundles
        .iter()
        .map(|(_, b)| b.clone())
        .chain(externals.iter().map(|e| e.trim_start_matches("./").to_string()))
        .map(|name| format!("  <link rel=\"modulepreload\" href=\"{}{name}\">\n", page.link))
        .collect();
    assert_eq!(text.matches("</head>").count(), 1, "{what} has one head");
    text = text.replacen("</head>", &format!("{preloads}</head>"), 1);
    let page_out = out.join("index.html");
    fs::write(&page_out, text).expect("write the built page");
    made.files.push(Built { rel: "index.html".into(), path: page_out });
    made.files.sort_by(|a, b| a.rel.cmp(&b.rel));
    made
}
