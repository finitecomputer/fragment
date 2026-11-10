//! What `fragment home files` brings of an agent's own files, and what it
//! leaves behind and why (FIN-463 §4, "The files cap"): the one rule the
//! survey shows and the import follows.
//!
//! 1. **Never brought:** dependencies and caches, build output beside the
//!    project file that builds it, hidden folders, compiled code,
//!    archives, installers and model weights, and what a project's
//!    `.gitignore` ignores.
//! 2. **Left behind, listed:** a file past a blob's most
//!    (`BLOB_MAX_BYTES`), or whose name can't be a file's here.
//! 3. **Brought first:** photos, video, audio and documents, newest first;
//!    then everything else (source and data), newest first, while the
//!    cap holds (`FILES_MAX_BYTES`, and `FILES_MAX` files).

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use anyhow::Result;
use fragment_proto::limits::BLOB_MAX_BYTES;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde::Serialize;

use super::layout::Layout;
use super::rules::{self, Kind};
use super::source::{Entry, Source};

/// The bytes of files one agent's import brings, at most (Paul, FIN-463):
/// what a person's export keeps room for in one fragment.
pub const FILES_MAX_BYTES: u64 = fragment_core::export::limits::HOME_FILES_MAX_BYTES;
/// The files it brings, at most: what a person's export takes of one
/// fragment (`export::limits::FILES_MAX`), less room for the fragment's
/// own (its manifest, `left-behind.md`).
pub const FILES_MAX: usize = fragment_core::export::limits::FILES_MAX - FILES_ROOM;
const FILES_ROOM: usize = 16;
/// Where the files go in the fragment they are brought to.
pub const PREFIX: &str = "files";
/// A `.gitignore` read, at most, and how many one plan reads.
const GITIGNORE_MAX_BYTES: u64 = 64 * 1024;
const GITIGNORES_MAX: usize = 1_000;

/// Why a file, or a folder of them, stays behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Why {
    /// Dependencies or a cache (`node_modules`, `.venv`).
    Dependencies,
    /// Built beside the project file that builds it (`dist` by `package.json`).
    BuildOutput,
    /// A hidden folder (`.obsidian`).
    Hidden,
    /// Compiled code, an archive, an installer or model weights.
    Never,
    /// A project's `.gitignore` ignores it.
    Ignored,
    /// Its name says it may hold a secret.
    Secret,
    /// Larger than a file here may be (`BLOB_MAX_BYTES`).
    TooLarge,
    /// Its path can't be a file's here.
    Name,
    /// Past the cap's bytes.
    OverCap,
    /// Past the cap's count of files.
    OverCount,
}

impl Why {
    pub fn said(self) -> &'static str {
        match self {
            Why::Dependencies => "dependencies or a cache, made again by the tools that made them",
            Why::BuildOutput => "build output, made again from its project",
            Why::Hidden => "a hidden folder",
            Why::Never => "compiled code, an archive, an installer or model weights",
            Why::Ignored => "its project's .gitignore leaves it out",
            Why::Secret => "its name says it may hold a password or key",
            Why::TooLarge => "larger than one file here may be (256 MiB)",
            Why::Name => "its name can't be a file's name here",
            Why::OverCap => "past the 10 GiB brought",
            Why::OverCount => "past the number of files brought",
        }
    }
}

/// A file, or a folder of them (its path ends in `/`), left behind.
#[derive(Debug, Clone, Serialize)]
pub struct Left {
    /// Relative to the work folder.
    pub path: String,
    pub files: usize,
    pub bytes: u64,
    pub why: Why,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<Kind>,
}

/// A file brought.
#[derive(Debug, Clone, Serialize)]
pub struct Picked {
    /// Its path in the source.
    pub from: String,
    /// Its path in the fragment.
    pub to: String,
    pub bytes: u64,
    pub kind: Kind,
}

#[derive(Debug, Default)]
pub struct Plan {
    /// The work folder, if the layout names one the source has.
    pub root: Option<String>,
    /// In the order they are brought.
    pub picked: Vec<Picked>,
    /// Folders first (by path), then files, the largest first.
    pub left: Vec<Left>,
}

/// The plan for `src`'s work folder (the layout's), within `cap_bytes`
/// and `cap_files`.
pub fn plan(src: &Source, layout: &Layout, cap_bytes: u64, cap_files: usize) -> Result<Plan> {
    let Some(work) = layout.work.as_deref().filter(|w| *w == "." || src.has_folder(w)) else { return Ok(Plan::default()) };
    let skip = if work == "." { 0 } else { work.len() + 1 };
    let files: Vec<(&Entry, &str)> = src.under(work).map(|e| (e, &e.path[skip..])).collect();
    // the layout's parts are brought their own way: not these files
    let parts: Vec<String> = layout.parts.values().filter_map(|p| if work == "." { Some(format!("{p}/")) } else { p.strip_prefix(&format!("{work}/")).map(|r| format!("{r}/")) }).collect();
    let parent = |rel: &str| rel.rsplit_once('/').map_or("", |(d, _)| d).to_string();
    let projects: HashSet<String> = files.iter().filter(|(_, rel)| rules::project_file(name_of(rel))).map(|(_, rel)| parent(rel)).collect();
    let ignores = gitignores(src, &files);

    let mut folders: BTreeMap<String, Left> = BTreeMap::new();
    // what the source left out unread for its name (`home::open`), named
    let mut left: Vec<Left> = src
        .skipped
        .iter()
        .filter_map(|e| Some((if skip == 0 { e.path.as_str() } else { e.path.strip_prefix(&format!("{work}/"))? }, e.size)))
        .map(|(rel, bytes)| Left { path: rel.to_string(), files: 1, bytes, why: Why::Secret, kind: None })
        .collect();
    let mut candidates: Vec<(&Entry, &str, Kind)> = Vec::new();
    for (e, rel) in &files {
        if parts.iter().any(|p| rel.starts_with(p.as_str())) {
            continue;
        }
        if let Some((folder, why)) = left_folder(rel, &projects) {
            let l = folders.entry(folder.clone()).or_insert(Left { path: folder, files: 0, bytes: 0, why, kind: None });
            (l.files, l.bytes) = (l.files + 1, l.bytes + e.size);
            continue;
        }
        let kind = Kind::of(name_of(rel));
        let why = if rules::secret_name(name_of(rel)) {
            Some(Why::Secret)
        } else if kind.never() {
            Some(Why::Never)
        } else if ignored(&ignores, rel) {
            Some(Why::Ignored)
        } else if e.size > BLOB_MAX_BYTES {
            Some(Why::TooLarge)
        } else if !fragment_proto::valid_repo_path(&format!("{PREFIX}/{rel}")) {
            Some(Why::Name)
        } else {
            None
        };
        match why {
            Some(why) => left.push(Left { path: rel.to_string(), files: 1, bytes: e.size, why, kind: Some(kind) }),
            None => candidates.push((e, rel, kind)),
        }
    }
    candidates.sort_by_key(|(e, rel, kind)| (!kind.first(), Reverse(e.mtime), *rel));
    let (mut bytes, mut picked) = (0u64, Vec::new());
    for (e, rel, kind) in candidates {
        let why = if picked.len() >= cap_files {
            Some(Why::OverCount)
        } else if bytes + e.size > cap_bytes {
            Some(Why::OverCap)
        } else {
            None
        };
        match why {
            Some(why) => left.push(Left { path: rel.to_string(), files: 1, bytes: e.size, why, kind: Some(kind) }),
            None => {
                bytes += e.size;
                picked.push(Picked { from: e.path.clone(), to: format!("{PREFIX}/{rel}"), bytes: e.size, kind });
            }
        }
    }
    left.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path)));
    Ok(Plan { root: Some(work.to_string()), picked, left: folders.into_values().chain(left).collect() })
}

fn name_of(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

/// The outermost folder on `rel`'s way that is left behind whole, and why.
fn left_folder(rel: &str, projects: &HashSet<String>) -> Option<(String, Why)> {
    let dirs: Vec<&str> = rel.split('/').collect();
    let dirs = &dirs[..dirs.len() - 1];
    for (i, name) in dirs.iter().enumerate() {
        let why = if rules::dependency_dir(name) {
            Why::Dependencies
        } else if name.starts_with('.') {
            Why::Hidden
        } else if rules::build_dir(name) && projects.contains(&dirs[..i].join("/")) {
            Why::BuildOutput
        } else {
            continue;
        };
        return Some((format!("{}/", dirs[..=i].join("/")), why));
    }
    None
}

/// Each `.gitignore` among `files` (outside folders left behind), as a
/// matcher of paths relative to its folder, by its folder.
fn gitignores(src: &Source, files: &[(&Entry, &str)]) -> Vec<(String, Gitignore)> {
    let mut out = Vec::new();
    for (e, rel) in files {
        if name_of(rel) != ".gitignore" || e.size > GITIGNORE_MAX_BYTES || out.len() >= GITIGNORES_MAX || left_folder(rel, &HashSet::new()).is_some() {
            continue;
        }
        let Ok(Some(text)) = src.text(&e.path) else { continue };
        let mut b = GitignoreBuilder::new(Path::new("."));
        for line in text.lines() {
            let _ = b.add_line(None, line);
        }
        if let Ok(g) = b.build() {
            let dir = rel.rsplit_once('/').map_or(String::new(), |(d, _)| d.to_string());
            out.push((dir, g));
        }
    }
    // the deepest first: a nearer .gitignore decides before a farther one
    out.sort_by_key(|(d, _)| Reverse(if d.is_empty() { 0 } else { d.split('/').count() }));
    out
}

/// What the `.gitignore`s say of `rel` (a folder, `is_dir`) by itself:
/// the nearest that names it decides, and `None` when none does.
fn decides(ignores: &[(String, Gitignore)], rel: &str, is_dir: bool) -> Option<bool> {
    for (dir, g) in ignores {
        let sub = if dir.is_empty() { Some(rel) } else { rel.strip_prefix(&format!("{dir}/")) };
        let Some(sub) = sub else { continue };
        let m = g.matched(Path::new(sub), is_dir);
        if m.is_ignore() {
            return Some(true);
        }
        if m.is_whitelist() {
            return Some(false);
        }
    }
    None
}

/// Whether git would ignore `rel`, as git decides: a file under a folder
/// that is ignored stays ignored whatever a later `!` says of the file
/// (git never looks inside an ignored folder); else what the nearest
/// `.gitignore` naming the file says.
fn ignored(ignores: &[(String, Gitignore)], rel: &str) -> bool {
    let parts: Vec<&str> = rel.split('/').collect();
    (1..parts.len()).any(|n| decides(ignores, &parts[..n].join("/"), true) == Some(true)) || decides(ignores, rel, false) == Some(true)
}

/// What a plan brings and leaves, counted: by kind, and by why.
#[derive(Debug, Serialize)]
pub struct Counted {
    pub root: Option<String>,
    pub cap_bytes: u64,
    pub cap_files: usize,
    pub brought: Tally,
    pub left: Tally,
    /// Each reason: its count, and its largest (at most `EXAMPLES`).
    pub left_by_why: BTreeMap<Why, Reason>,
}

#[derive(Debug, Default, Serialize)]
pub struct Tally {
    pub files: usize,
    pub bytes: u64,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub by_kind: BTreeMap<Kind, Count>,
}

#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct Count {
    pub files: usize,
    pub bytes: u64,
}

#[derive(Debug, Default, Serialize)]
pub struct Reason {
    pub files: usize,
    pub bytes: u64,
    pub largest: Vec<Left>,
}

/// The largest of a reason the survey names.
const EXAMPLES: usize = 10;

impl Plan {
    pub fn counted(&self, cap_bytes: u64, cap_files: usize) -> Counted {
        let mut brought = Tally::default();
        for p in &self.picked {
            brought.files += 1;
            brought.bytes += p.bytes;
            let c = brought.by_kind.entry(p.kind).or_default();
            (c.files, c.bytes) = (c.files + 1, c.bytes + p.bytes);
        }
        let mut left = Tally::default();
        let mut by_why: BTreeMap<Why, Reason> = BTreeMap::new();
        for l in &self.left {
            left.files += l.files;
            left.bytes += l.bytes;
            if let Some(k) = l.kind {
                let c = left.by_kind.entry(k).or_default();
                (c.files, c.bytes) = (c.files + l.files, c.bytes + l.bytes);
            }
            let r = by_why.entry(l.why).or_default();
            (r.files, r.bytes) = (r.files + l.files, r.bytes + l.bytes);
            r.largest.push(l.clone());
        }
        for r in by_why.values_mut() {
            r.largest.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path)));
            r.largest.truncate(EXAMPLES);
        }
        Counted { root: self.root.clone(), cap_bytes, cap_files, brought, left, left_by_why: by_why }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::home::source::tests::Temp;
    use fragment_core::export::limits::GIB;

    fn put(root: &Path, files: &[(&str, usize, i64)]) {
        for (p, size, age) in files {
            let path = root.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, vec![b'x'; *size]).unwrap();
            let t = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_790_000_000 - *age as u64);
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(t).unwrap();
        }
    }

    fn layout() -> Layout {
        Layout { work: Some("work".into()), parts: [("brains".to_string(), "work/brains".to_string())].into(), ..Layout::hermes() }
    }

    #[test]
    fn media_first_newest_first_within_the_cap() {
        let t = Temp::new();
        put(&t.0, &[
            ("work/notes/old.md", 10, 500),
            ("work/notes/new.md", 10, 100),
            ("work/photos/a.jpg", 40, 900),
            ("work/code/main.py", 10, 1),
            ("work/code/data.csv", 30, 2),
            ("work/brains/b1/wiki/index.md", 10, 1),
            ("home/SOUL.md", 5, 1),
        ]);
        let src = Source::open(&t.0).unwrap();
        // the cap: 70 bytes. Photos and documents go first, newest first (60
        // bytes), then main.py (newest of the rest); data.csv is past it
        let p = plan(&src, &layout(), 70, 100).unwrap();
        let picked: Vec<&str> = p.picked.iter().map(|p| p.to.as_str()).collect();
        assert_eq!(picked, ["files/notes/new.md", "files/notes/old.md", "files/photos/a.jpg", "files/code/main.py"]);
        let left: Vec<(&str, Why)> = p.left.iter().map(|l| (l.path.as_str(), l.why)).collect();
        assert_eq!(left, [("code/data.csv", Why::OverCap)], "the brains part is brought its own way, and the home is no work");
        let p = plan(&src, &layout(), 1_000, 2).unwrap();
        assert_eq!(p.picked.len(), 2);
        assert!(p.left.iter().all(|l| l.why == Why::OverCount));
        assert!(plan(&src, &Layout::hermes(), 1_000, 100).unwrap().picked.is_empty(), "no work, no files");
    }

    /// As git decides: a file under a folder a `.gitignore` ignores stays
    /// ignored, whatever a later `!` says of the file (git never looks in
    /// an ignored folder); a nearer `.gitignore` overrides a farther one.
    #[test]
    fn an_ignored_folder_keeps_its_files_ignored() {
        let t = Temp::new();
        put(&t.0, &[("work/p/.gitignore", 0, 1), ("work/p/private/keep.txt", 3, 1), ("work/p/private/other.txt", 3, 1), ("work/p/a.log", 3, 1), ("work/p/sub/.gitignore", 0, 1), ("work/p/sub/b.log", 3, 1), ("work/p/sub/c.log", 3, 1)]);
        std::fs::write(t.0.join("work/p/.gitignore"), "private/\n!private/keep.txt\n*.log\n").unwrap();
        std::fs::write(t.0.join("work/p/sub/.gitignore"), "!b.log\n").unwrap();
        let src = Source::open(&t.0).unwrap();
        let p = plan(&src, &layout(), GIB, 100).unwrap();
        let mut picked: Vec<&str> = p.picked.iter().map(|p| p.from.as_str()).collect();
        picked.sort();
        assert_eq!(picked, ["work/p/.gitignore", "work/p/sub/.gitignore", "work/p/sub/b.log"]);
        let mut ignored: Vec<&str> = p.left.iter().filter(|l| l.why == Why::Ignored).map(|l| l.path.as_str()).collect();
        ignored.sort();
        assert_eq!(ignored, ["p/a.log", "p/private/keep.txt", "p/private/other.txt", "p/sub/c.log"]);
    }

    #[test]
    fn what_never_comes() {
        let t = Temp::new();
        put(&t.0, &[
            ("work/app/package.json", 10, 1),
            ("work/app/src/index.js", 10, 1),
            ("work/app/dist/bundle.js", 50, 1),
            ("work/app/node_modules/x/index.js", 70, 1),
            ("work/app/node_modules/y/index.js", 30, 1),
            ("work/notes/build/plan.md", 10, 1),
            ("work/.obsidian/config", 10, 1),
            ("work/models/llama.gguf", 90, 1),
            ("work/backup.zip", 20, 1),
            ("work/py/.gitignore", 0, 1),
            ("work/py/out.log", 10, 1),
            ("work/py/keep.log", 10, 1),
            ("work/py/main.py", 10, 1),
            ("work/server.pem", 5, 1),
        ]);
        std::fs::write(t.0.join("work/py/.gitignore"), "*.log\n!keep.log\n").unwrap();
        let src = Source::open(&t.0).unwrap();
        let p = plan(&src, &layout(), GIB, 100).unwrap();
        let mut picked: Vec<&str> = p.picked.iter().map(|p| p.from.as_str()).collect();
        picked.sort();
        assert_eq!(picked, ["work/app/package.json", "work/app/src/index.js", "work/notes/build/plan.md", "work/py/.gitignore", "work/py/keep.log", "work/py/main.py"]);
        let left: Vec<(&str, usize, Why)> = p.left.iter().map(|l| (l.path.as_str(), l.files, l.why)).collect();
        assert_eq!(left, [
            (".obsidian/", 1, Why::Hidden),
            ("app/dist/", 1, Why::BuildOutput),
            ("app/node_modules/", 2, Why::Dependencies),
            ("models/llama.gguf", 1, Why::Never),
            ("backup.zip", 1, Why::Never),
            ("py/out.log", 1, Why::Ignored),
            ("server.pem", 1, Why::Secret),
        ]);
        let c = p.counted(GIB, 100);
        assert_eq!((c.left.files, c.left_by_why[&Why::Dependencies].bytes), (8, 100));
        assert_eq!(c.brought.by_kind[&Kind::Document].files, 1);
    }
}
