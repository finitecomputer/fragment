// Generates OUT_DIR/templates.rs: the files the release serves from the
// cell's Static Assets, as an index of each one's path, SHA-256 and size
// (the cell reads a file's bytes by its hash: cell/src/assets.rs), and,
// with the `embed` feature, their bytes (the CLI, xtask, which writes them
// into the assets, and the e2e). They are one `Template` per directory
// under templates/ (every file but src/, which holds sources of committed
// bundles) and `ALL`; the shell's files (cell/shell/, `SHELL`); and the
// shared browser files (cell/, `BROWSER`), agent docs (cli/SKILL.md,
// cli/GUIDE.md) and an export's README (crates/core/src/export_readme.md). Each template's release, a
// hash of its paths and bytes, is made here, so a cell that never holds
// the bytes names it all the same.
//
// Without the `platform` feature (the CLI) it reads no blessed template and
// no shell: `ALL` is the templates a person or an agent scaffolds, and
// `SHELL` is empty, so neither is an input of the CLI's build.
//
// With `platform`, the pages `PAGES` names are built (bundle.rs), not held
// as written: the shell's and the chat, Telegram and Session Archive pages,
// their scripts bundled and every script and stylesheet of their own named
// by its hash. Each built
// file is a file of its template or the shell like any other, from OUT_DIR;
// the shell's sources its bundle holds are no file of the release (nothing
// asks for them), while a template's stay at their names.
//
// A template may hold another's files through a symlink (the chat's
// agent.png is the agent's, and so is the shell's): one asset, and
// embedded, its bytes are in a binary once. A symlink in a template that
// leaves templates/ fails the build.
use std::collections::BTreeMap;
use std::{env, fs, path::Path, path::PathBuf};

use sha2::{Digest, Sha256};

include!("src/blessed_names.rs");

#[cfg(feature = "platform")]
mod bundle;

/// The pages the release builds (bundle.rs), each where it sits: the
/// shell's (`shell`, cell/shell/), or a blessed template's site/, and how
/// it names a file beside it.
#[cfg(feature = "platform")]
const PAGES: &[(&str, &str)] = &[("chat", "./"), ("sessions", "./"), ("shell", "/__shell/"), ("telegram", "./")];

fn collect(dir: &Path, root: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let p = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "src" || name == ".DS_Store" {
            continue;
        }
        if p.is_dir() {
            collect(&p, root, out);
        } else {
            out.push((p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"), p));
        }
    }
}

/// The files the generated code names: each distinct real file once, as
/// the static that embeds it.
struct Statics {
    ids: BTreeMap<PathBuf, usize>,
}

impl Statics {
    fn id(&mut self, real: PathBuf) -> usize {
        let next = self.ids.len();
        *self.ids.entry(real).or_insert(next)
    }
}

/// One file's index entry (and, embedded, its bytes), as Rust.
fn entry(path: &str, bytes: &[u8], id: usize, embed: bool) -> String {
    let sha = hex::encode(Sha256::digest(bytes));
    let embedded = if embed { format!(", bytes: B{id}") } else { String::new() };
    format!("File {{ path: {path:?}, sha256: {sha:?}, size: {}{embedded} }}", bytes.len())
}

fn main() {
    let manifest = env::var("CARGO_MANIFEST_DIR").unwrap();
    let embed = env::var_os("CARGO_FEATURE_EMBED").is_some();
    // the platform's own files (the `platform` feature): the blessed
    // templates and the shell's; the CLI builds without, and reads neither
    let platform = env::var_os("CARGO_FEATURE_PLATFORM").is_some();
    let repo = fs::canonicalize(Path::new(&manifest).join("../..")).expect("the repo");
    let tdir = repo.join("templates");
    let shell = repo.join("cell/shell");
    let docs = [("SKILL_MD", repo.join("cli/SKILL.md")), ("GUIDE_MD", repo.join("cli/GUIDE.md")), ("EXPORT_README", repo.join("crates/core/src/export_readme.md"))];
    let browser = [
        ("CLIENT_JS", "client.mjs"),
        ("CLIENT_CSS", "fragment.css"),
        ("FILES_JS", "files.mjs"),
        ("FILES_CSS", "files.css"),
        ("MARKDOWN_JS", "markdown.mjs"),
        ("SW_JS", "sw.js"),
    ];
    let names_file = Path::new(&manifest).join("src/blessed_names.rs");
    let watched = [&tdir, &names_file].into_iter().chain(platform.then_some(&shell));
    for watched in watched.chain(docs.iter().map(|(_, p)| p)) {
        println!("cargo:rerun-if-changed={}", watched.display());
    }
    let mut names: Vec<String> = fs::read_dir(&tdir)
        .expect("templates/")
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| platform || !BLESSED.contains(&n.as_str()))
        .collect();
    names.sort();
    let mut statics = Statics { ids: BTreeMap::new() };
    let mut src = String::new();
    let out_dir = fs::canonicalize(env::var("OUT_DIR").unwrap()).expect("OUT_DIR");
    // the files a page's build made or named anew (bundle.rs): where, and
    // path; and the shell's sources its bundle holds, which the release
    // does not (none without `platform`, which builds no page)
    #[allow(unused_mut)]
    let mut built: Vec<(String, String)> = Vec::new();
    #[allow(unused_mut)]
    let mut shell_bundled: Vec<String> = Vec::new();
    #[cfg(feature = "platform")]
    let esbuild = bundle::Esbuild::installed(&repo);
    #[cfg(feature = "platform")]
    let page = |at: &str, dir: PathBuf| {
        let &(_, link) = PAGES.iter().find(|(p, _)| *p == at)?;
        Some(bundle::build(&esbuild, &bundle::Page { dir, link }, &repo, &out_dir.join("web").join(at)))
    };
    for name in &names {
        let mut files = Vec::new();
        collect(&tdir.join(name), &tdir.join(name), &mut files);
        // its page built (bundle.rs), in place of its own; its sources stay
        #[cfg(feature = "platform")]
        if let Some(made) = page(name, tdir.join(name).join("site")) {
            files.retain(|(rel, _)| rel != "site/index.html");
            for b in made.files {
                built.push((name.clone(), format!("site/{}", b.rel)));
                files.push((format!("site/{}", b.rel), b.path));
            }
        }
        files.sort();
        let mut release = Sha256::new();
        let mut entries = Vec::new();
        for (rel, path) in files {
            let real = fs::canonicalize(&path).unwrap_or_else(|e| panic!("templates/{name}/{rel}: {e}"));
            assert!(real.starts_with(&tdir) || real.starts_with(&out_dir), "templates/{name}/{rel} is a symlink out of templates/");
            let bytes = fs::read(&real).unwrap_or_else(|e| panic!("templates/{name}/{rel}: {e}"));
            // which release of the template a fragment runs (blessed.rs `release`)
            release.update(rel.as_bytes());
            release.update([0]);
            release.update(&bytes);
            release.update([0]);
            entries.push(entry(&rel, &bytes, statics.id(real), embed));
        }
        let release = hex::encode(&release.finalize()[..12]);
        src.push_str(&format!("pub static {}: Template = Template {{\n    name: {name:?},\n    release: {release:?},\n    files: &[\n", name.to_uppercase()));
        for e in entries {
            src.push_str(&format!("        {e},\n"));
        }
        src.push_str("    ],\n};\n");
    }
    src.push_str("pub static ALL: &[&Template] = &[\n");
    for name in &names {
        src.push_str(&format!("    &{},\n", name.to_uppercase()));
    }
    src.push_str("];\n");
    if platform {
        for (name, file) in browser {
            let path = repo.join("cell").join(file);
            println!("cargo:rerun-if-changed={}", path.display());
            let bytes = fs::read(&path).unwrap_or_else(|e| panic!("cell/{file}: {e}"));
            src.push_str(&format!(
                "pub static {name}: File = {};\n",
                entry(file, &bytes, statics.id(path), embed)
            ));
        }
        src.push_str("pub static BROWSER: &[&File] = &[&CLIENT_JS, &CLIENT_CSS, &FILES_JS, &FILES_CSS, &MARKDOWN_JS, &SW_JS];\n");
        let mut files = Vec::new();
        assert!(shell.is_dir(), "cell/shell/: the platform's files hold the shell's");
        collect(&shell, &shell, &mut files);
        // its sources are files too: the shell has no src/ to skip
        assert!(!shell.join("src").exists(), "cell/shell/src would be skipped");
        // its page built (bundle.rs), in place of its own: shell.js
        // bundled, its stylesheets by their hash, and the sources the
        // bundle holds no file of the release
        #[cfg(feature = "platform")]
        {
            let made = page("shell", shell.clone()).expect("the shell's page is built");
            files.retain(|(rel, path)| {
                let held = made.inputs.contains(&fs::canonicalize(path).unwrap_or_else(|e| panic!("cell/shell/{rel}: {e}")));
                if held {
                    shell_bundled.push(rel.clone());
                }
                rel != "index.html" && !held
            });
            for b in made.files {
                built.push(("shell".into(), b.rel.clone()));
                files.push((b.rel, b.path));
            }
        }
        files.sort();
        src.push_str("pub static SHELL: &[File] = &[\n");
        for (rel, path) in files {
            let real = fs::canonicalize(&path).unwrap_or_else(|e| panic!("cell/shell/{rel}: {e}"));
            assert!(real.starts_with(&repo) || real.starts_with(&out_dir), "cell/shell/{rel} is a symlink out of the repo");
            let bytes = fs::read(&real).unwrap_or_else(|e| panic!("cell/shell/{rel}: {e}"));
            src.push_str(&format!("    {},\n", entry(&rel, &bytes, statics.id(real), embed)));
        }
        src.push_str("];\n");
    }
    built.sort();
    shell_bundled.sort();
    src.push_str("/// The files a page's build made or named anew (build.rs, bundle.rs): where (`shell`, or a template), and path.\n");
    src.push_str("pub static BUILT: &[(&str, &str)] = &[\n");
    for (at, rel) in &built {
        src.push_str(&format!("    ({at:?}, {rel:?}),\n"));
    }
    src.push_str("];\n");
    src.push_str("/// The files of cell/shell/ its page's bundle holds, which the release does not.\n");
    src.push_str("pub static SHELL_BUNDLED: &[&str] = &[\n");
    for rel in &shell_bundled {
        src.push_str(&format!("    {rel:?},\n"));
    }
    src.push_str("];\n");
    for (name, path) in &docs {
        let bytes = fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let file = path.file_name().expect("a doc is a file").to_string_lossy().to_string();
        src.push_str(&format!("pub static {name}: File = {};\n", entry(&file, &bytes, statics.id(path.clone()), embed)));
    }
    if embed {
        for (real, id) in &statics.ids {
            src.push_str(&format!("static B{id}: &[u8] = include_bytes!({:?});\n", real.display().to_string()));
        }
    }
    fs::write(Path::new(&env::var("OUT_DIR").unwrap()).join("templates.rs"), src).expect("write templates.rs");
}
