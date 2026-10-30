//! The CLI's releases (`cargo xtask cli id [<rev>]`, `cargo xtask cli
//! plan …`): which code a build of the CLI is, and the release production's
//! deploy publishes when that code changes (.github/workflows/ci.yml,
//! `cli-plan`; release.yml builds and publishes it).
//!
//! What a build is, is its content id: the first 16 hex digits of a
//! SHA-256 of what git holds at one commit for every path the CLI is built
//! from (`INPUTS`, and each template it scaffolds: templates/ less the
//! blessed ones, which it never embeds: crates/templates/build.rs), and of
//! the Cargo.lock entries its build can resolve: every package its
//! manifests reach by a normal or a build dependency, on any platform and
//! with any feature the lock holds (a superset of one target's build). No
//! commit, clock or build cache goes in: two commits whose CLI code is the
//! same have one id, and reverting a change brings back the id before it.
//! The blessed templates are today's list (fragment_templates::blessed),
//! read at every commit alike.
//!
//! A release's version is the next patch above every release's tag, or
//! cli/Cargo.toml's version when that is higher: a minor or a major is a
//! hand edit there, a patch never is. Versions only go up, when the code
//! goes back too (production rolled back to an older commit).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use fragment_devstack as devstack;

/// The paths the CLI is built from, besides its templates: the workspace's
/// manifest (its release profile) and cargo's config (whose flags would be
/// the build's), the CLI, and the workspace crates it depends on, as the
/// Hermes image copies them to build it (images/hermes/Dockerfile). `id`
/// refuses a CLI whose lock reaches a workspace crate not named here.
const INPUTS: &[&str] = &[".cargo", "Cargo.toml", "cli", "crates/core", "crates/nip98", "crates/proto", "crates/templates"];

/// The CLI's package, which the lock is walked from.
const PACKAGE: &str = "fragment-cli";

/// The hex digits of the hash an id keeps (as FRAGMENT_SCHEMA does).
const ID_HEX: usize = 16;

const USAGE: &str = "usage: cargo xtask cli id [<rev>] | cli plan [--at <rev>] [--latest <tag>] [--github-output <file>] [--summary <file>]";

pub(crate) fn cli(args: &[String]) -> Result<()> {
    let root = devstack::repo_root();
    match args.first().map(String::as_str) {
        Some("id") if args.len() <= 2 => {
            println!("{}", id(&root, args.get(1).map_or("HEAD", String::as_str))?);
            Ok(())
        }
        Some("plan") => plan_cmd(&root, &args[1..]),
        _ => bail!(USAGE),
    }
}

/// `plan`: what production's deploy of `--at` (default HEAD) publishes,
/// against the latest release (`--latest`, its tag: what the install line
/// fetches), printed, and written for the workflow: its step's outputs
/// (`release`, and with a release `tag` and `version`) and its summary.
fn plan_cmd(root: &Path, args: &[String]) -> Result<()> {
    let usage = || anyhow::anyhow!(USAGE);
    let (mut at, mut latest, mut output, mut summary) = ("HEAD".to_string(), None, None, None);
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--at" => at = it.next().ok_or_else(usage)?.clone(),
            "--latest" => latest = Some(it.next().ok_or_else(usage)?.clone()),
            "--github-output" => output = Some(PathBuf::from(it.next().ok_or_else(usage)?)),
            "--summary" => summary = Some(PathBuf::from(it.next().ok_or_else(usage)?)),
            _ => return Err(usage()),
        }
    }
    let plan = plan(root, &at, latest.as_deref())?;
    println!("{plan}");
    if let Some(path) = output {
        append(&path, &plan.outputs())?;
    }
    if let Some(path) = summary {
        append(&path, &format!("### The CLI's release\n\n{plan}\n"))?;
    }
    Ok(())
}

fn append(path: &Path, text: &str) -> Result<()> {
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path).with_context(|| format!("open {}", path.display()))?;
    file.write_all(text.as_bytes()).with_context(|| format!("write {}", path.display()))
}

/// What production's deploy of a commit publishes.
#[derive(Debug, PartialEq)]
struct Plan {
    /// The commit, and its CLI's id.
    at: String,
    id: String,
    /// The latest release's tag, and its CLI's id: none before the first.
    latest: Option<(String, String)>,
    /// The release to publish, when the CLI's code is not the latest's.
    release: Option<Version>,
}

impl Plan {
    /// The workflow step's outputs, `key=value` lines.
    fn outputs(&self) -> String {
        match self.release {
            Some(v) => format!("release=true\ntag=v{v}\nversion={v}\nid={}\n", self.id),
            None => format!("release=false\nid={}\n", self.id),
        }
    }
}

impl fmt::Display for Plan {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "The CLI at {} is {}.", self.at, self.id)?;
        match &self.latest {
            Some((tag, id)) => writeln!(f, "The latest release, {tag}, is {id}.")?,
            None => writeln!(f, "There is no release yet.")?,
        }
        match self.release {
            Some(v) => write!(f, "Its code changed: release v{v} of {}.", self.at),
            None => write!(f, "Its code is the latest release's: nothing to release."),
        }
    }
}

/// The plan for `at` against the release tagged `latest` (module docs):
/// that tag must be among this checkout's, as the next version is above
/// every one, so a checkout without them plans nothing.
fn plan(root: &Path, at: &str, latest: Option<&str>) -> Result<Plan> {
    let commit = commit_of(root, at)?;
    let ours = id(root, &commit)?;
    let tags = release_tags(root)?;
    let latest = match latest {
        Some(tag) => {
            anyhow::ensure!(tags.iter().any(|t| t == tag), "the latest release's tag {tag} is not in this checkout: fetch every tag (the next version is above them all)");
            Some((tag.to_string(), id(root, &format!("refs/tags/{tag}"))?))
        }
        None => None,
    };
    let release = match &latest {
        Some((_, theirs)) if *theirs == ours => None,
        _ => Some(next_version(base_version(root, &commit)?, &tags)),
    };
    Ok(Plan { at: commit, id: ours, latest, release })
}

/// A release's version: `<major>.<minor>.<patch>`, digits alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Version(u64, u64, u64);

impl Version {
    /// None for anything else, a pre-release's or a build's version too.
    fn parse(s: &str) -> Option<Version> {
        let digits = |p: &str| if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) { p.parse().ok() } else { None };
        let parts: Vec<u64> = s.split('.').map(digits).collect::<Option<_>>()?;
        match parts[..] {
            [major, minor, patch] => Some(Version(major, minor, patch)),
            _ => None,
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// The version a new release takes: the next patch above every release's
/// tag (`v<version>`; any other tag is no release's), or `base` (the CLI's
/// manifest's) when that is higher. Never one a tag holds.
fn next_version(base: Version, tags: &[String]) -> Version {
    match tags.iter().filter_map(|t| t.strip_prefix('v')).filter_map(Version::parse).max() {
        Some(top) if top >= base => Version(top.0, top.1, top.2 + 1),
        _ => base,
    }
}

/// cli/Cargo.toml's version at `commit`: the base a release's is at least.
fn base_version(root: &Path, commit: &str) -> Result<Version> {
    let manifest: toml::Table = show(root, commit, "cli/Cargo.toml")?.parse().context("cli/Cargo.toml")?;
    let version = manifest.get("package").and_then(|p| p.get("version")).and_then(|v| v.as_str()).context("cli/Cargo.toml names no version")?;
    Version::parse(version).with_context(|| format!("cli/Cargo.toml's version {version} is not <major>.<minor>.<patch>"))
}

/// This checkout's tags that start with `v`.
fn release_tags(root: &Path) -> Result<Vec<String>> {
    let out = git(root, &["tag", "--list", "v*"])?;
    Ok(String::from_utf8(out)?.lines().map(str::to_string).collect())
}

/// The CLI's content id at `rev` (module docs).
pub(crate) fn id(root: &Path, rev: &str) -> Result<String> {
    let commit = commit_of(root, rev)?;
    let paths = input_paths(root, &commit)?;
    let mut listed = vec!["ls-tree", "-rz", &commit, "--"];
    listed.extend(paths.iter().map(String::as_str));
    let mut hashed = git(root, &listed)?;
    links_stay_inside(root, &entries(&hashed)?, &paths)?;
    hashed.extend_from_slice(b"\0Cargo.lock\0");
    hashed.extend_from_slice(locked(root, &commit)?.join("\n").as_bytes());
    Ok(fragment_core::blob::sha256_hex(&hashed)[..ID_HEX].to_string())
}

/// The commit `rev` names, in full.
fn commit_of(root: &Path, rev: &str) -> Result<String> {
    anyhow::ensure!(!rev.is_empty() && !rev.starts_with('-'), "{rev:?} is no revision");
    let out = git(root, &["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")]).with_context(|| format!("{rev} is no commit here"))?;
    Ok(String::from_utf8(out)?.trim().to_string())
}

/// INPUTS and the templates the CLI scaffolds, as `commit` holds them:
/// one missing is refused, never hashed as nothing.
fn input_paths(root: &Path, commit: &str) -> Result<Vec<String>> {
    let mut paths: Vec<String> = INPUTS.iter().map(|p| p.to_string()).collect();
    for e in entries(&git(root, &["ls-tree", "-z", commit, "--", "templates/"])?)? {
        let name = e.path.strip_prefix("templates/").unwrap_or(&e.path);
        if e.mode == "040000" && !fragment_templates::blessed::BLESSED.contains(&name) {
            paths.push(e.path);
        }
    }
    let mut listed = vec!["ls-tree", "-z", commit, "--"];
    listed.extend(paths.iter().map(String::as_str));
    let found: BTreeSet<String> = entries(&git(root, &listed)?)?.into_iter().map(|e| e.path).collect();
    for path in &paths {
        anyhow::ensure!(found.contains(path), "{commit} holds no {path}, which the CLI is built from (xtask/src/cli.rs, INPUTS)");
    }
    Ok(paths)
}

/// One entry of `git ls-tree -z`.
struct Entry {
    mode: String,
    oid: String,
    path: String,
}

fn entries(listing: &[u8]) -> Result<Vec<Entry>> {
    let mut out = Vec::new();
    for record in listing.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let record = std::str::from_utf8(record).context("a path git lists is not UTF-8")?;
        let (meta, path) = record.split_once('\t').with_context(|| format!("git ls-tree said {record:?}"))?;
        let mut meta = meta.split(' ');
        let (Some(mode), Some(_kind), Some(oid)) = (meta.next(), meta.next(), meta.next()) else { bail!("git ls-tree said {record:?}") };
        out.push(Entry { mode: mode.to_string(), oid: oid.to_string(), path: path.to_string() });
    }
    Ok(out)
}

/// A symlink among the inputs is the CLI's through its target, whose bytes
/// the build embeds and git does not hash with the link: each must land
/// among the inputs too (the brain's link into the notes template is a
/// blessed template's, and none of the CLI's).
fn links_stay_inside(root: &Path, listed: &[Entry], paths: &[String]) -> Result<()> {
    for link in listed.iter().filter(|e| e.mode == "120000") {
        let target = String::from_utf8(git(root, &["cat-file", "blob", &link.oid])?)?;
        let from = Path::new(&link.path).parent().unwrap_or(Path::new(""));
        let landed = normalized(&from.join(&target));
        anyhow::ensure!(
            landed.as_deref().is_some_and(|l| paths.iter().any(|p| l == p || l.starts_with(&format!("{p}/")))),
            "{} links to {target}, which the CLI's id does not hash (xtask/src/cli.rs, INPUTS)",
            link.path
        );
    }
    Ok(())
}

/// A relative path with its `.` and `..` resolved; none when it leaves the
/// repo or is absolute.
fn normalized(path: &Path) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for c in path.components() {
        match c {
            Component::Normal(p) => parts.push(p.to_str()?),
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(parts.join("/"))
}

/// One `[[package]]` of Cargo.lock.
#[derive(serde::Deserialize)]
struct Locked {
    name: String,
    version: String,
    source: Option<String>,
    checksum: Option<String>,
    #[serde(default)]
    dependencies: Vec<String>,
}

#[derive(serde::Deserialize)]
struct Lock {
    package: Vec<Locked>,
}

/// The lock's entries the CLI's build can resolve, one line each, sorted:
/// walked from fragment-cli along every dependency of a registry's or a git
/// package (the lock holds their normal and build ones alone), and along
/// a workspace crate's normal and build dependencies, which its manifest
/// tells from the dev-dependencies its lock entry lists too. A workspace
/// crate is hashed by its tree instead, and must be one of INPUTS.
fn locked(root: &Path, commit: &str) -> Result<Vec<String>> {
    let lock: Lock = toml::from_str(&show(root, commit, "Cargo.lock")?).context("Cargo.lock")?;
    let members = members(root, commit)?;
    let start = lock.package.iter().position(|p| p.name == PACKAGE && p.source.is_none()).context("Cargo.lock has no fragment-cli")?;
    let (mut seen, mut queue, mut lines) = (BTreeSet::from([start]), VecDeque::from([start]), Vec::new());
    while let Some(i) = queue.pop_front() {
        let p = &lock.package[i];
        let built = match &p.source {
            Some(source) => {
                lines.push(format!("{} {} {source} {}", p.name, p.version, p.checksum.as_deref().unwrap_or("-")));
                None
            }
            None => {
                let dir = members.get(&p.name).with_context(|| format!("the CLI depends on {}, a path no workspace member is", p.name))?;
                anyhow::ensure!(INPUTS.contains(&dir.as_str()), "the CLI is built from {} ({dir}), which its id leaves out: name it in xtask/src/cli.rs, INPUTS", p.name);
                Some(built_deps(&show(root, commit, &format!("{dir}/Cargo.toml"))?).with_context(|| format!("{dir}/Cargo.toml"))?)
            }
        };
        for dep in &p.dependencies {
            let name = dep.split(' ').next().unwrap_or_default();
            if built.as_ref().is_some_and(|b| !b.contains(name)) {
                continue;
            }
            let j = resolve(&lock.package, dep)?;
            if seen.insert(j) {
                queue.push_back(j);
            }
        }
    }
    lines.sort();
    Ok(lines)
}

/// The packages a manifest's normal and build dependencies name, on every
/// platform (`[target.<cfg>.…]`), optional ones too, each by its package's
/// name (`package =` renames one).
fn built_deps(manifest: &str) -> Result<BTreeSet<String>> {
    let manifest: toml::Table = manifest.parse()?;
    let kinds = ["dependencies", "build-dependencies", "build_dependencies"];
    let mut tables: Vec<&toml::Value> = kinds.iter().filter_map(|k| manifest.get(*k)).collect();
    if let Some(targets) = manifest.get("target").and_then(|t| t.as_table()) {
        tables.extend(targets.values().flat_map(|t| kinds.iter().filter_map(|k| t.get(*k))));
    }
    let mut names = BTreeSet::new();
    for table in tables {
        for (key, spec) in table.as_table().context("a dependency table is a table")? {
            names.insert(spec.get("package").and_then(|p| p.as_str()).unwrap_or(key).to_string());
        }
    }
    Ok(names)
}

/// The workspace's members at `commit`: each package's name, to its
/// directory.
fn members(root: &Path, commit: &str) -> Result<BTreeMap<String, String>> {
    let workspace: toml::Table = show(root, commit, "Cargo.toml")?.parse().context("Cargo.toml")?;
    let dirs = workspace.get("workspace").and_then(|w| w.get("members")).and_then(|m| m.as_array()).context("Cargo.toml names no workspace members")?;
    let mut out = BTreeMap::new();
    for dir in dirs {
        let dir = dir.as_str().context("a workspace member is a path")?;
        anyhow::ensure!(!dir.contains('*'), "the workspace member {dir} is a glob: name each member");
        let manifest: toml::Table = show(root, commit, &format!("{dir}/Cargo.toml"))?.parse().with_context(|| format!("{dir}/Cargo.toml"))?;
        let name = manifest.get("package").and_then(|p| p.get("name")).and_then(|n| n.as_str()).with_context(|| format!("{dir}/Cargo.toml names no package"))?;
        out.insert(name.to_string(), dir.to_string());
    }
    Ok(out)
}

/// The entry a lock's dependency names: `name`, or `name version`, or
/// `name version (source)` where the name alone is ambiguous.
fn resolve(packages: &[Locked], dep: &str) -> Result<usize> {
    let mut parts = dep.splitn(3, ' ');
    let (name, version) = (parts.next().unwrap_or_default(), parts.next());
    let source = parts.next().map(|s| s.trim_start_matches('(').trim_end_matches(')'));
    let named = |p: &Locked| p.name == name && version.is_none_or(|v| v == p.version) && source.is_none_or(|s| p.source.as_deref() == Some(s));
    let found: Vec<usize> = packages.iter().enumerate().filter(|(_, p)| named(p)).map(|(i, _)| i).collect();
    match found[..] {
        [i] => Ok(i),
        _ => bail!("Cargo.lock's dependency {dep:?} names {} packages, not one", found.len()),
    }
}

/// A file as `commit` holds it.
fn show(root: &Path, commit: &str, path: &str) -> Result<String> {
    Ok(String::from_utf8(git(root, &["show", &format!("{commit}:{path}")])?)?)
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("git").args(args).current_dir(root).output().context("could not start git")?;
    anyhow::ensure!(out.status.success(), "git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    Ok(out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const LOCK: &str = r#"version = 4

[[package]]
name = "fragment-cli"
version = "0.14.1"
dependencies = ["fragment-core", "fragment-fakes", "serde", "socket2"]

[[package]]
name = "fragment-core"
version = "0.1.0"
dependencies = ["rusqlite", "serde"]

[[package]]
name = "fragment-fakes"
version = "0.1.0"
dependencies = ["tokio"]

[[package]]
name = "fragment-nip98"
version = "0.1.0"

[[package]]
name = "fragment-proto"
version = "0.1.0"

[[package]]
name = "fragment-templates"
version = "0.1.0"

[[package]]
name = "rusqlite"
version = "0.32.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "aa"

[[package]]
name = "serde"
version = "1.0.200"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "bb"
dependencies = ["serde_derive"]

[[package]]
name = "serde_derive"
version = "1.0.200"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "cc"

[[package]]
name = "socket2"
version = "0.6.5"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "dd"

[[package]]
name = "tokio"
version = "1.40.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "ee"
"#;

    /// A workspace shaped as the repo's, in a new git repository: the CLI
    /// (its dev-dependencies the fakes and socket2), its crates, the fakes,
    /// a template it scaffolds and a blessed one, and the cell.
    struct Repo(PathBuf);

    impl Repo {
        fn new() -> Repo {
            // the tests run at once, and a clock may tick in microseconds
            static MADE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let made = MADE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let at = std::env::temp_dir().join(format!("cli-id-{}-{made}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
            fs::create_dir_all(&at).unwrap();
            let repo = Repo(at);
            repo.git(&["init", "-q"]);
            repo.write("Cargo.toml", "[workspace]\nmembers = [\"cli\", \"crates/core\", \"crates/fakes\", \"crates/nip98\", \"crates/proto\", \"crates/templates\"]\n");
            repo.write(".cargo/config.toml", "[alias]\nxtask = \"run --package xtask --\"\n");
            repo.write("cli/Cargo.toml", "[package]\nname = \"fragment-cli\"\nversion = \"0.14.1\"\n\n[dependencies]\nfragment-core = { path = \"../crates/core\" }\nserde = \"1\"\n\n[dev-dependencies]\nfragment-fakes = { path = \"../crates/fakes\" }\nsocket2 = \"=0.6.5\"\n");
            repo.write("cli/src/main.rs", "fn main() {}");
            repo.write("crates/core/Cargo.toml", "[package]\nname = \"fragment-core\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1\"\n\n[dev-dependencies]\nrusqlite = \"0.32\"\n");
            repo.write("crates/fakes/Cargo.toml", "[package]\nname = \"fragment-fakes\"\nversion = \"0.1.0\"\n\n[dependencies]\ntokio = \"1\"\n");
            for (dir, name) in [("crates/nip98", "fragment-nip98"), ("crates/proto", "fragment-proto"), ("crates/templates", "fragment-templates")] {
                repo.write(&format!("{dir}/Cargo.toml"), &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n"));
            }
            repo.write("Cargo.lock", LOCK);
            repo.write("templates/todo/app.js", "todo");
            repo.write("templates/chat/index.html", "chat");
            repo.write("cell/src/lib.rs", "cell");
            repo.commit("first");
            repo
        }

        fn write(&self, path: &str, text: &str) {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        fn git(&self, args: &[&str]) {
            let out = Command::new("git").args(["-c", "user.name=Test", "-c", "user.email=test@example.com", "-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"]).args(args).current_dir(&self.0).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        }

        fn commit(&self, message: &str) {
            self.git(&["add", "-A"]);
            self.git(&["commit", "-qm", message]);
        }

        fn id(&self) -> Result<String> {
            id(&self.0, "HEAD")
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Valid and replayed: what the CLI is not built from (the cell, a
    /// blessed template, the fakes, a dev-dependency's lock entry, a new
    /// commit) leaves the id; what it is built from (its code, a crate of
    /// its, a template it scaffolds, a lock entry it resolves) moves it;
    /// and putting the code back brings the id before back.
    #[test]
    fn the_id_is_the_clis_code_alone() {
        let repo = Repo::new();
        let first = repo.id().unwrap();
        assert_eq!(first.len(), ID_HEX);
        assert_eq!(repo.id().unwrap(), first, "the same commit, asked again");
        repo.write("cell/src/lib.rs", "cell changed");
        repo.write("templates/chat/index.html", "chat changed");
        repo.write("crates/fakes/src/lib.rs", "fakes changed");
        repo.write("Cargo.lock", &LOCK.replace("1.40.0", "1.41.0").replace("0.32.0", "0.33.0").replace("\"dd\"", "\"d2\""));
        repo.commit("not the CLI's");
        assert_eq!(repo.id().unwrap(), first, "the cell, a blessed template, the fakes, tokio, rusqlite, socket2");
        for (path, text) in [("cli/src/main.rs", "fn main() { }"), ("crates/core/src/lib.rs", "core"), ("templates/todo/app.js", "todo changed"), ("Cargo.lock", &LOCK.replace("\"cc\"", "\"c2\""))] {
            repo.write(path, text);
            repo.commit(path);
            assert_ne!(repo.id().unwrap(), first, "{path}");
        }
        repo.git(&["revert", "--no-edit", "HEAD~4..HEAD"]);
        assert_eq!(repo.id().unwrap(), first, "the code put back");
    }

    /// Invalid: a workspace crate the CLI comes to build from that INPUTS
    /// leaves out, a link out of the inputs, a missing input, and a commit
    /// that is not there are refused, never hashed.
    #[test]
    fn what_the_id_cannot_cover_is_refused() {
        let repo = Repo::new();
        repo.write("cli/Cargo.toml", "[package]\nname = \"fragment-cli\"\nversion = \"0.14.1\"\n\n[dependencies]\nfragment-core = { path = \"../crates/core\" }\nfragment-fakes = { path = \"../crates/fakes\" }\nserde = \"1\"\n");
        repo.commit("the fakes, built");
        assert!(repo.id().unwrap_err().to_string().contains("INPUTS"));
        repo.git(&["revert", "--no-edit", "HEAD"]);
        assert!(repo.id().is_ok());
        std::os::unix::fs::symlink("../chat/index.html", repo.0.join("templates/todo/index.html")).unwrap();
        repo.commit("a link to a blessed template");
        assert!(repo.id().unwrap_err().to_string().contains("links to ../chat/index.html"));
        repo.git(&["revert", "--no-edit", "HEAD"]);
        repo.git(&["rm", "-rq", "crates/nip98"]);
        repo.commit("no nip98");
        assert!(repo.id().unwrap_err().to_string().contains("no crates/nip98"));
        assert!(id(&repo.0, "no-such-commit").is_err());
        assert!(id(&repo.0, "--all").is_err());
    }

    /// The next patch above every release, or the manifest's version when
    /// that is higher; a tag that is no release's version is passed over.
    #[test]
    fn versions_only_go_up() {
        let tags = |t: &[&str]| t.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let base = Version(0, 14, 1);
        assert_eq!(next_version(base, &tags(&[])), base, "the first release");
        assert_eq!(next_version(base, &tags(&["v0.14.0", "v0.14.1"])), Version(0, 14, 2));
        assert_eq!(next_version(base, &tags(&["v0.14.1", "v0.14.2", "v0.9.9"])), Version(0, 14, 3), "above every tag, not the latest's");
        assert_eq!(next_version(Version(0, 15, 0), &tags(&["v0.14.1"])), Version(0, 15, 0), "a hand bump");
        assert_eq!(next_version(Version(0, 15, 0), &tags(&["v0.15.0"])), Version(0, 15, 1));
        assert_eq!(next_version(base, &tags(&["v0.14.1", "v1.0.0-rc.1", "vnext", "v0.20", "celld-final"])), Version(0, 14, 2));
        for bad in ["", "1", "1.2", "1.2.3.4", "1.2.x", "1.2.3-rc.1", "v1.2.3", " 1.2.3"] {
            assert_eq!(Version::parse(bad), None, "{bad:?}");
        }
        assert_eq!(Version(0, 14, 2).to_string(), "0.14.2");
    }

    /// A deploy whose CLI is the latest release's publishes nothing; one
    /// whose CLI differs, the next version (as the workflow's outputs say);
    /// and a latest release this checkout has no tag of plans nothing.
    #[test]
    fn a_release_follows_a_changed_cli() {
        let repo = Repo::new();
        repo.git(&["tag", "v0.14.1"]);
        repo.write("cell/src/lib.rs", "a deploy of the cell alone");
        repo.commit("cell");
        let same = plan(&repo.0, "HEAD", Some("v0.14.1")).unwrap();
        assert_eq!(same.release, None);
        assert_eq!(same.outputs(), format!("release=false\nid={}\n", same.id));
        repo.write("cli/src/main.rs", "fn main() { println!() }");
        repo.commit("cli");
        let changed = plan(&repo.0, "HEAD", Some("v0.14.1")).unwrap();
        assert_eq!(changed.release, Some(Version(0, 14, 2)));
        assert_eq!(changed.outputs(), format!("release=true\ntag=v0.14.2\nversion=0.14.2\nid={}\n", changed.id));
        assert!(changed.to_string().contains("release v0.14.2 of"));
        assert_eq!(plan(&repo.0, "HEAD", None).unwrap().release, Some(Version(0, 14, 2)), "no latest release: above the tags all the same");
        assert!(plan(&repo.0, "HEAD", Some("v0.14.0")).unwrap_err().to_string().contains("fetch every tag"));
    }

    /// The repo's own tree: its lock and manifests walk, its inputs are all
    /// there, and what only a crate's tests or xtask build (fragment-core's
    /// dev-dependency tar, xtask's toml) is no part of the CLI's id.
    #[test]
    fn the_repos_cli_has_an_id() {
        let root = devstack::repo_root();
        assert_eq!(id(&root, "HEAD").unwrap().len(), ID_HEX);
        let lines = locked(&root, &commit_of(&root, "HEAD").unwrap()).unwrap();
        assert!(lines.iter().any(|l| l.starts_with("clap ")), "{lines:?}");
        assert!(!lines.iter().any(|l| l.starts_with("tar ") || l.starts_with("toml ")), "{lines:?}");
    }
}
