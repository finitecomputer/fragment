//! Which of a CI run's cached builds cargo may take as built
//! (.github/workflows/ci.yml: e2e-shard restores and records them, deploy
//! restores them). Cargo takes a crate of ours as built when none of its
//! sources is newer than its last build's start (the mtime it gives the
//! build's dep-info), and every file of a fresh checkout is newer than
//! every cached build. So the builds are cached with a record of the tree
//! they were built from, each entry of `git ls-tree -r -t` (file, symlink
//! or directory: its mode, object and path) with the time since which it
//! has held that object, the start of the first build in the cache's line
//! that saw it so ("since"). Restoring:
//!
//! - every entry of the checkout is touched now, after the cache is in
//!   place: newer than any build in it, whenever that cache was saved (a
//!   build saved after this checkout was made, and restored by it, is not
//!   newer than a source it lacks);
//! - then each entry the record holds as the checkout does is given its
//!   since. A build made at or after that time, in the line of builds that
//!   saved the cache one after another, was made from this very object,
//!   and stays built; one made before it (another feature set's, kept in
//!   the target directory from an older tree) was not, and is rebuilt. A
//!   directory's object moves with anything under it, which a build script
//!   watching it (crates/templates/build.rs) needs.
//!
//! A since is the build's, not the record's: one time for the whole tree
//! (the newest build's start) would take every crate that build did not
//! itself rebuild for stale. Content decides, never a commit's date, which
//! can be older than builds that lack its change.
//!
//! Why keep this (tooling review, 2026-10-30): git-restore-mtime has no
//! Python dependencies, but restores commit dates, not the content/build
//! relationship above; a change committed before a cached build can look
//! fresh without having been built. Swatinem/rust-cache is already used;
//! caching workspace crates there does not repair checkout mtimes. sccache
//! 0.19.0 with only its GHA backend has 292 normal transitive packages and
//! cannot cache linker-invoking crates (our CLI, e2e and cell cdylib). Using
//! dependency caches alone is correct, but gives up first-party build reuse:
//! a shard has already exhausted CI's 20-minute budget (lanes/mod.rs).
//! Retain this until a simpler cache's PR run proves it fits that budget.
//!
//! Std alone, no crate: CI compiles this file by itself with `rustc`
//! before its first cargo build, as no cargo build is to be trusted before
//! it has run. xtask holds it for its tests alone.
//!
//!   stamp restore <record> <state dir>   after the cache is restored
//!   stamp record <record> <state dir>    after a build that succeeded

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::io;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// The state dir's files, from `restore` to `record`: the record restored
/// (empty when there was none) and this build's start (seconds).
const PREVIOUS: &str = "previous";
const START: &str = "start";
/// Paths per `touch`, within any argument list's limit.
const TOUCH_BATCH: usize = 500;

#[derive(Debug)]
pub enum StampError {
    Usage,
    /// A command that did not run, or ran and failed.
    Command(String),
    /// The checkout is not HEAD's tree: the record would not say what was built.
    Dirty,
    /// A listing, record or start that does not parse.
    Malformed(String),
    Io(String, io::Error),
}

impl fmt::Display for StampError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StampError::Usage => write!(f, "usage: stamp restore|record <record> <state dir>"),
            StampError::Command(what) => write!(f, "{what}"),
            StampError::Dirty => write!(f, "the checkout is not HEAD's tree, which the record describes"),
            StampError::Malformed(what) => write!(f, "malformed {what}"),
            StampError::Io(what, e) => write!(f, "{what}: {e}"),
        }
    }
}

// CI's entry point; xtask compiles this file for its tests, which run `run`.
#[cfg_attr(test, allow(dead_code))]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = run(&args) {
        eprintln!("stamp: {e}");
        std::process::exit(1);
    }
}

fn run(args: &[String]) -> Result<(), StampError> {
    let root = Path::new(".");
    match args {
        [cmd, record, state] if cmd == "restore" => {
            let r = restore(root, Path::new(record), Path::new(state))?;
            if r.recorded {
                println!("{} of {} files and directories as the cached builds' tree held them; the rest are new to them", r.unchanged, r.entries);
            } else {
                println!("no record of the cached builds' tree: all {} files and directories are new to them", r.entries);
            }
            Ok(())
        }
        [cmd, record, state] if cmd == "record" => {
            let n = record_tree(root, Path::new(record), Path::new(state))?;
            println!("recorded the tree of this build, {n} files and directories");
            Ok(())
        }
        _ => Err(StampError::Usage),
    }
}

pub struct Restored {
    pub entries: usize,
    pub unchanged: usize,
    pub recorded: bool,
}

/// After the cache is restored, before any cargo build: the checkout's
/// mtimes, as the module says. The record is moved into `state`, with this
/// build's start, for `record_tree`: a record left beside builds made after
/// it would say what they were not built from.
pub fn restore(root: &Path, record: &Path, state: &Path) -> Result<Restored, StampError> {
    clean(root)?;
    let current = entries(&git(root, &["ls-tree", "-r", "-t", "-z", "HEAD"])?)?;
    let kept = match std::fs::read(record) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(StampError::Io(format!("read {}", record.display()), e)),
    };
    let since = parse_record(kept.as_deref().unwrap_or_default())?;
    let paths: Vec<&str> = current.iter().map(|e| path_of(e)).collect();
    touch(root, None, &paths)?;
    let unchanged = unchanged(&since, &current);
    for (at, paths) in &unchanged {
        touch(root, Some(*at), paths)?;
    }
    let start = now()?;
    std::fs::create_dir_all(state).map_err(|e| StampError::Io(format!("create {}", state.display()), e))?;
    write(&state.join(PREVIOUS), kept.as_deref().unwrap_or_default())?;
    write(&state.join(START), start.to_string().as_bytes())?;
    if kept.is_some() {
        std::fs::remove_file(record).map_err(|e| StampError::Io(format!("remove {}", record.display()), e))?;
    }
    Ok(Restored { entries: current.len(), unchanged: unchanged.values().map(Vec::len).sum(), recorded: kept.is_some() })
}

/// After a build that succeeded, before its cache is saved: the record of
/// the tree it was built from, each entry's since kept from the restored
/// record where it held the same object, else this build's start.
pub fn record_tree(root: &Path, record: &Path, state: &Path) -> Result<usize, StampError> {
    clean(root)?;
    let current = entries(&git(root, &["ls-tree", "-r", "-t", "-z", "HEAD"])?)?;
    let previous = read(&state.join(PREVIOUS))?;
    let start = read(&state.join(START))?;
    let start: u64 = std::str::from_utf8(&start).ok().and_then(|s| s.trim().parse().ok()).ok_or_else(|| StampError::Malformed(format!("start in {}", state.display())))?;
    write(record, &next_record(&parse_record(&previous)?, &current, start))?;
    Ok(current.len())
}

/// The entries of `git ls-tree -r -t -z`: `<mode> <type> <object>\t<path>`.
fn entries(listing: &[u8]) -> Result<Vec<String>, StampError> {
    listing.split(|b| *b == 0).filter(|e| !e.is_empty()).map(entry).collect()
}

fn entry(bytes: &[u8]) -> Result<String, StampError> {
    let text = std::str::from_utf8(bytes).map_err(|_| StampError::Malformed("entry: a path that is not UTF-8".into()))?;
    let well_formed = text.split_once('\t').is_some_and(|(meta, path)| meta.split(' ').count() == 3 && !path.is_empty());
    if !well_formed {
        return Err(StampError::Malformed(format!("entry {text:?}")));
    }
    Ok(text.to_string())
}

fn path_of(entry: &str) -> &str {
    entry.split_once('\t').map(|(_, path)| path).expect("an entry names its path")
}

/// A record: `<since> <entry>` for each entry, NUL after each.
fn parse_record(bytes: &[u8]) -> Result<HashMap<String, u64>, StampError> {
    let mut since = HashMap::new();
    // bounded: one turn per entry
    for line in bytes.split(|b| *b == 0).filter(|l| !l.is_empty()) {
        let bad = || StampError::Malformed(format!("record line {:?}", String::from_utf8_lossy(line)));
        let space = line.iter().position(|b| *b == b' ').ok_or_else(bad)?;
        let at = std::str::from_utf8(&line[..space]).ok().and_then(|s| s.parse().ok()).ok_or_else(bad)?;
        since.insert(entry(&line[space + 1..])?, at);
    }
    Ok(since)
}

/// The paths the record holds as the checkout does, by their since.
fn unchanged<'a>(since: &HashMap<String, u64>, current: &'a [String]) -> BTreeMap<u64, Vec<&'a str>> {
    let mut by_time: BTreeMap<u64, Vec<&str>> = BTreeMap::new();
    for e in current {
        if let Some(at) = since.get(e) {
            by_time.entry(*at).or_default().push(path_of(e));
        }
    }
    by_time
}

fn next_record(previous: &HashMap<String, u64>, current: &[String], start: u64) -> Vec<u8> {
    let mut out = Vec::new();
    for e in current {
        let at = previous.get(e).copied().unwrap_or(start);
        out.extend_from_slice(format!("{at} {e}").as_bytes());
        out.push(0);
    }
    out
}

/// Touches `paths` (under `root`; a symlink itself, not its target) now, or
/// at `at`, in seconds since the epoch. `touch -t` and `-h` are POSIX: GNU's
/// and BSD's (a Mac's) take them alike, read in UTC here.
fn touch(root: &Path, at: Option<u64>, paths: &[&str]) -> Result<(), StampError> {
    for batch in paths.chunks(TOUCH_BATCH) {
        let mut cmd = Command::new("touch");
        cmd.current_dir(root).env("TZ", "UTC").arg("-h");
        if let Some(at) = at {
            cmd.arg("-t").arg(touch_time(at));
        }
        let status = cmd.arg("--").args(batch).status().map_err(|e| StampError::Io("run touch".into(), e))?;
        if !status.success() {
            return Err(StampError::Command(format!("touch {}: {status}", batch.len())));
        }
    }
    Ok(())
}

/// `CCYYMMDDhhmm.SS`, UTC, for `touch -t`.
fn touch_time(secs: u64) -> String {
    let (y, m, d) = civil((secs / 86_400) as i64);
    let s = secs % 86_400;
    format!("{y:04}{m:02}{d:02}{:02}{:02}.{:02}", s / 3600, s % 3600 / 60, s % 60)
}

/// The proleptic Gregorian date of a day counted from 1970-01-01 (Howard
/// Hinnant's `civil_from_days`).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn clean(root: &Path) -> Result<(), StampError> {
    let status = Command::new("git").current_dir(root).args(["diff", "--quiet", "HEAD", "--"]).status().map_err(|e| StampError::Io("run git diff".into(), e))?;
    match status.code() {
        Some(0) => Ok(()),
        Some(1) => Err(StampError::Dirty),
        _ => Err(StampError::Command(format!("git diff --quiet HEAD: {status}"))),
    }
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>, StampError> {
    let out = Command::new("git").current_dir(root).args(args).output().map_err(|e| StampError::Io("run git".into(), e))?;
    if !out.status.success() {
        return Err(StampError::Command(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim())));
    }
    Ok(out.stdout)
}

fn now() -> Result<u64, StampError> {
    let since_epoch = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| StampError::Malformed("clock: before 1970".into()))?;
    Ok(since_epoch.as_secs())
}

fn read(path: &Path) -> Result<Vec<u8>, StampError> {
    std::fs::read(path).map_err(|e| StampError::Io(format!("read {}", path.display()), e))
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), StampError> {
    std::fs::write(path, bytes).map_err(|e| StampError::Io(format!("write {}", path.display()), e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    #[test]
    fn touch_times_are_utc_dates() {
        assert_eq!(touch_time(0), "197001010000.00");
        assert_eq!(touch_time(946_684_800), "200001010000.00");
        assert_eq!(touch_time(951_782_400), "200002290000.00", "a leap day");
        assert_eq!(touch_time(1_792_762_509), "202610231335.09");
        assert_eq!(touch_time(4_102_444_799), "209912312359.59");
    }

    /// Goal: an entry keeps its since while it holds its object, and takes
    /// the build's start when it moves or is new; a record that does not
    /// parse is refused. Method: the pure steps, on listings.
    #[test]
    fn a_since_is_kept_while_the_object_is() {
        let a = "100644 blob aaaa\tsrc/a.rs".to_string();
        let b1 = "100644 blob bbbb\tsrc/b.rs".to_string();
        let b2 = "100644 blob cccc\tsrc/b.rs".to_string();
        let dir1 = "040000 tree dddd\tsrc".to_string();
        let dir2 = "040000 tree eeee\tsrc".to_string();
        let first = next_record(&HashMap::new(), &[a.clone(), b1.clone(), dir1.clone()], 100);
        let first = parse_record(&first).unwrap();
        assert_eq!(first.values().collect::<Vec<_>>(), [&100, &100, &100]);
        let second = parse_record(&next_record(&first, &[a.clone(), b2.clone(), dir2.clone()], 200)).unwrap();
        assert_eq!((second[&a], second[&b2], second[&dir2]), (100, 200, 200));
        assert!(!second.contains_key(&b1) && !second.contains_key(&dir1), "what the tree no longer holds goes");
        let checkout = [a.clone(), b1.clone(), dir2.clone()];
        let plan = unchanged(&second, &checkout);
        assert_eq!(plan, BTreeMap::from([(100, vec!["src/a.rs"]), (200, vec!["src"])]), "b.rs moved back: new to the builds");
        for bad in [&b"100 no-tab"[..], b"x 100644 blob a\tp", b"100644 blob a\tp", b"100 100644 blob\tp"] {
            assert!(matches!(parse_record(bad), Err(StampError::Malformed(_))), "{}", String::from_utf8_lossy(bad));
        }
        assert!(matches!(run(&["nothing".into()]), Err(StampError::Usage)));
    }

    /// A git repository in a directory of its own, with cargo builds in it,
    /// each a CI run's: the checkout made fresh (every entry touched now, or
    /// at a time given), the record restored, the build, the record written.
    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Fixture {
            let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
            let root = std::env::temp_dir().join(format!("xtask-stamp-{name}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&root).unwrap();
            let f = Fixture { root };
            f.git(&["init", "-q"]);
            // as the repo's: the builds and the state are no part of the tree
            f.write(".gitignore", "/target/\n/state/\n");
            f
        }

        fn write(&self, path: &str, text: &str) {
            let path = self.root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        fn git(&self, args: &[&str]) {
            let status = Command::new("git")
                .current_dir(&self.root)
                .args(["-c", "user.name=stamp", "-c", "user.email=stamp@example.invalid", "-c", "commit.gpgsign=false"])
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        }

        fn commit(&self) {
            self.git(&["add", "-A"]);
            self.git(&["commit", "-q", "--allow-empty", "-m", "a build"]);
        }

        /// A CI run up to its build: a checkout's mtimes (now, or `at`),
        /// then the record restored. Runs are seconds apart, as a since is.
        fn checkout_and_restore(&self, at: Option<u64>) -> Restored {
            std::thread::sleep(Duration::from_millis(1100));
            let listing = entries(&git(&self.root, &["ls-tree", "-r", "-t", "-z", "HEAD"]).unwrap()).unwrap();
            let paths: Vec<&str> = listing.iter().map(|e| path_of(e)).collect();
            touch(&self.root, at, &paths).unwrap();
            restore(&self.root, &self.root.join("target/built-tree"), &self.root.join("state")).unwrap()
        }

        /// `cargo run` here: what the build, as cargo took it, prints; and
        /// its verbose log (cargo's Fresh and Compiling lines).
        fn cargo_run(&self, args: &[&str]) -> (String, String) {
            let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
            let out = Command::new(cargo)
                .current_dir(&self.root)
                .env_remove("CARGO_TARGET_DIR")
                .env_remove("RUSTFLAGS")
                .env_remove("CARGO_ENCODED_RUSTFLAGS")
                // CI's `always` would split "Fresh lib" with escapes
                .env("CARGO_TERM_COLOR", "never")
                .args(["run", "-v", "--offline", "--target-dir", "target"])
                .args(args)
                .output()
                .unwrap();
            let log = String::from_utf8_lossy(&out.stderr).into_owned();
            assert!(out.status.success(), "cargo run {args:?}:\n{log}");
            (String::from_utf8_lossy(&out.stdout).into_owned(), log)
        }

        fn record(&self) {
            record_tree(&self.root, &self.root.join("target/built-tree"), &self.root.join("state")).unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    const PACKAGE: &str = "[package]\nname = \"fx\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\nmembers = [\"lib\"]\n\n[dependencies]\nlib = { path = \"lib\" }\n";

    /// Goal (review of #558, P1): a feature set's build kept from an older
    /// tree is not taken as built for this one. Method: A builds source A
    /// with `extra` off; B changes the source and turns `extra` on; C turns
    /// it off with the source unchanged. C's build of the `extra`-off
    /// variant, A's in the target directory, must be rebuilt: it prints B.
    /// And a crate no build changed (`lib`, built once in A) stays built
    /// across all three, B's record taking nothing from it.
    #[test]
    fn a_kept_feature_variant_is_rebuilt_from_this_tree() {
        let f = Fixture::new("variant");
        let off = format!("{PACKAGE}\n[features]\ndefault = []\nextra = []\n");
        let on = format!("{PACKAGE}\n[features]\ndefault = [\"extra\"]\nextra = []\n");
        f.write("Cargo.toml", &off);
        f.write("lib/Cargo.toml", "[package]\nname = \"lib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n");
        f.write("lib/src/lib.rs", "pub fn word() -> &'static str { \"from\" }\n");
        f.write("src/main.rs", "fn main() { print!(\"{} {}\", lib::word(), include_str!(\"../msg.txt\")); }\n");
        f.write("msg.txt", "A");
        f.commit();
        f.checkout_and_restore(None);
        assert_eq!(f.cargo_run(&[]).0, "from A");
        f.record();

        f.write("Cargo.toml", &on);
        f.write("msg.txt", "B");
        f.commit();
        f.checkout_and_restore(None);
        let (out, log) = f.cargo_run(&[]);
        assert_eq!(out, "from B");
        assert!(log.contains("Fresh lib"), "lib, unchanged, stays built:\n{log}");
        f.record();

        f.write("Cargo.toml", &off);
        f.commit();
        let restored = f.checkout_and_restore(None);
        assert!(restored.recorded && restored.unchanged < restored.entries);
        let (out, log) = f.cargo_run(&[]);
        assert_eq!(out, "from B", "A's extra-off build is not this tree's:\n{log}");
        assert!(log.contains("Fresh lib"), "lib, unchanged since A, stays built:\n{log}");
    }

    /// Goal (review of #558, P1): a checkout made before the cache it
    /// restores was built (a newer master's build, saved meanwhile) still
    /// rebuilds what it changed. Method: A built and recorded; then B,
    /// checked out with mtimes older than A's build, restoring A's cache:
    /// it prints B. Unchanged, a fresh checkout's build takes all as built.
    #[test]
    fn a_checkout_older_than_the_cache_rebuilds_its_change() {
        let f = Fixture::new("older");
        f.write("Cargo.toml", PACKAGE);
        f.write("lib/Cargo.toml", "[package]\nname = \"lib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n");
        f.write("lib/src/lib.rs", "pub fn word() -> &'static str { \"from\" }\n");
        f.write("src/main.rs", "fn main() { print!(\"{} {}\", lib::word(), include_str!(\"../msg.txt\")); }\n");
        f.write("msg.txt", "A");
        f.commit();
        let before_a = now().unwrap() - 60;
        f.checkout_and_restore(None);
        assert_eq!(f.cargo_run(&[]).0, "from A");
        f.record();

        f.checkout_and_restore(None);
        let (out, log) = f.cargo_run(&[]);
        assert_eq!(out, "from A");
        assert!(log.contains("Fresh fx") && log.contains("Fresh lib"), "nothing changed, nothing rebuilt:\n{log}");
        f.record();

        f.write("msg.txt", "B");
        f.write("lib/src/new.rs", "");
        f.commit();
        f.checkout_and_restore(Some(before_a));
        let changed_dir = std::fs::symlink_metadata(f.root.join("lib/src")).unwrap().modified().unwrap();
        assert!(changed_dir > UNIX_EPOCH + Duration::from_secs(before_a + 60), "a changed directory is new to the builds too");
        let (out, log) = f.cargo_run(&[]);
        assert_eq!(out, "from B", "B's change is newer than A's build whatever its checkout's time:\n{log}");
    }

    /// Goal (review of #558, P1): crates/templates/build.rs, which watches
    /// templates/ (a directory), with `platform` on, then off across a
    /// template's edit, then on again: the `platform` build kept from
    /// before the edit is not taken as built, and its index names the
    /// edited file. Method: the repo's build.rs and blessed names, in a
    /// package of their own with the release's inputs it reads (a template,
    /// the shell, the agent docs), its index printed by template.
    #[test]
    fn the_templates_build_script_reruns_for_a_kept_feature_variant() {
        let f = Fixture::new("templates");
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        // its build dependencies at the versions the repo's lock names, which
        // the registry holds wherever the workspace was built (offline); the
        // lock is no part of the tree, as cargo prunes it at the first build
        f.write(".gitignore", "/target/\n/state/\n/crates/templates/Cargo.lock\n");
        std::fs::create_dir_all(f.root.join("crates/templates")).unwrap();
        std::fs::copy(repo.join("Cargo.lock"), f.root.join("crates/templates/Cargo.lock")).unwrap();
        f.write(
            "crates/templates/Cargo.toml",
            "[package]\nname = \"templates\"\nversion = \"0.1.0\"\nedition = \"2021\"\nbuild = \"build.rs\"\n\n[workspace]\n\n[features]\nplatform = []\n\n[build-dependencies]\nsha2 = \"0.10\"\nhex = \"0.4\"\n",
        );
        f.write("crates/templates/build.rs", include_str!("../../crates/templates/build.rs"));
        f.write("crates/templates/src/blessed_names.rs", include_str!("../../crates/templates/src/blessed_names.rs"));
        f.write(
            "crates/templates/src/main.rs",
            "#![allow(dead_code)]\nstruct File { path: &'static str, sha256: &'static str, size: u64 }\nstruct Template { name: &'static str, release: &'static str, files: &'static [File] }\ninclude!(concat!(env!(\"OUT_DIR\"), \"/templates.rs\"));\nfn main() { for t in ALL { println!(\"{} {}\", t.name, t.release); } }\n",
        );
        f.write("templates/todo/app.mjs", "one\n");
        f.write("cell/shell/index.html", "<!doctype html>\n");
        for file in ["client.mjs", "fragment.css", "files.mjs", "files.css", "markdown.mjs", "sw.js"] {
            f.write(&format!("cell/{file}"), "browser file\n");
        }
        f.write("cli/SKILL.md", "skill\n");
        f.write("cli/GUIDE.md", "guide\n");
        let manifest = ["--manifest-path", "crates/templates/Cargo.toml"];
        let platform = ["--manifest-path", "crates/templates/Cargo.toml", "--features", "platform"];
        f.commit();
        f.checkout_and_restore(None);
        let one = f.cargo_run(&platform).0;
        f.record();

        f.write("templates/todo/app.mjs", "two\n");
        f.commit();
        f.checkout_and_restore(None);
        let two = f.cargo_run(&manifest).0;
        assert_ne!(one, two, "the edit moves todo's release");
        f.record();

        f.commit();
        f.checkout_and_restore(None);
        let (again, log) = f.cargo_run(&platform);
        assert_eq!(again, two, "the platform build names the edited template:\n{log}");
    }
}
