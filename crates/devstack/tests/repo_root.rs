//! `repo_root` in a build kept from another checkout. Cargo takes a crate
//! as built wherever its checkout is, so a target directory cloned into a
//! new worktree (or restored at another mount, or shared by two checkouts)
//! holds this crate with the first checkout's path compiled in; the xtask
//! linked with it then checked, built and deployed that checkout from the
//! new one. The root is found as the process runs instead.
//!
//! Method: this test binary run again as a child that prints the root it
//! finds, (a) told by cargo's own variable that its package sits in
//! another checkout, as `cargo run` and `cargo test` say it, and (b)
//! copied into another checkout's `target/`, with no word from cargo, as a
//! binary run by hand is. A checkout here is a folder holding
//! `crates/devstack/Cargo.toml`.

use std::path::{Path, PathBuf};
use std::process::Command;

use fragment_devstack as devstack;

/// Set for the child: it prints its root and does nothing else.
const CHILD: &str = "FRAGMENT_REPO_ROOT_CHILD";
const SAID: &str = "the repo root: ";

/// The child's half, and nothing when cargo runs it as a test.
#[test]
fn child() {
    if std::env::var_os(CHILD).is_some() {
        println!("{SAID}{}", devstack::repo_root().display());
    }
}

/// A folder of its own, gone when dropped.
struct Temp(PathBuf);

impl Temp {
    fn new() -> Temp {
        // the real path: macOS's temporary folder is reached through a link,
        // which a process's own path (`current_exe`) does not keep
        let base = std::env::temp_dir().canonicalize().expect("the temporary folder");
        let dir = base.join(format!("repo-root-{}", devstack::random_hex(6)));
        std::fs::create_dir_all(&dir).expect("a folder of the test's own");
        Temp(dir)
    }

    /// Another checkout: all `repo_root` asks of one.
    fn checkout(&self) -> PathBuf {
        let root = self.0.join("checkout");
        std::fs::create_dir_all(root.join("crates/devstack")).expect("its folders");
        std::fs::write(root.join("crates/devstack/Cargo.toml"), "").expect("its manifest");
        root
    }

    /// This test binary, copied to `to` under the folder.
    fn copy_of_this_binary(&self, to: &str) -> PathBuf {
        let to = self.0.join(to);
        std::fs::create_dir_all(to.parent().expect("a file's folder")).expect("its folder");
        std::fs::copy(std::env::current_exe().expect("the test binary"), &to).expect("the copy");
        to
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The root the child at `exe` finds, told `manifest_dir` by cargo (or
/// nothing).
fn root_found_by(exe: &Path, manifest_dir: Option<&Path>) -> PathBuf {
    let mut cmd = Command::new(exe);
    cmd.args(["--exact", "child", "--nocapture"]).env(CHILD, "1").env_remove("CARGO_MANIFEST_DIR");
    if let Some(dir) = manifest_dir {
        cmd.env("CARGO_MANIFEST_DIR", dir);
    }
    let out = cmd.output().expect("the child runs");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "the child failed: {text}{}", String::from_utf8_lossy(&out.stderr));
    let said = text.lines().find_map(|l| l.strip_prefix(SAID)).unwrap_or_else(|| panic!("the child named no root: {text}"));
    PathBuf::from(said)
}

/// Where this crate really is: the checkout this test was built in and
/// runs in.
fn this_checkout() -> PathBuf {
    let root = std::env::current_dir().expect("cargo runs a package's tests in its folder").join("../..").canonicalize().expect("the repo root");
    assert!(root.join("crates/devstack/tests/repo_root.rs").is_file(), "{} is this checkout", root.display());
    root
}

/// Goal: a process cargo runs for another checkout works in that checkout,
/// whichever checkout built it. Valid: cargo's variable names a package of
/// the other checkout (xtask's folder, as `cargo xtask` has it, and a
/// crate two levels down, as `cargo test` has it). Before, the root was
/// the one compiled in.
#[test]
fn a_build_cargo_runs_for_another_checkout_works_in_that_checkout() {
    let t = Temp::new();
    let other = t.checkout();
    let this = std::env::current_exe().expect("the test binary");
    assert_eq!(root_found_by(&this, Some(&other.join("xtask"))), other);
    assert_eq!(root_found_by(&this, Some(&other.join("crates/devstack"))), other);
}

/// Goal: a binary run by hand works in the checkout whose `target/` holds
/// it. Valid: a copy of this binary under another checkout's `target/`,
/// with nothing from cargo, as a cloned target directory's binaries are.
/// Before, the root was the one compiled in.
#[test]
fn a_build_kept_in_another_checkouts_target_works_in_that_checkout() {
    let t = Temp::new();
    let other = t.checkout();
    let kept = t.copy_of_this_binary("checkout/target/debug/deps/repo_root-kept");
    assert_eq!(root_found_by(&kept, None), other);
}

/// Goal: cargo's word comes first, and a word that names no checkout is
/// passed over. Valid: a binary in one checkout's `target/`, told by cargo
/// that it runs for another (a target directory two checkouts share).
/// Invalid: cargo's variable naming a folder outside any checkout (set by
/// a cargo that ran something else, and inherited): the binary's own
/// place decides.
#[test]
fn cargos_word_comes_before_where_the_binary_is() {
    let t = Temp::new();
    let other = t.checkout();
    let kept = t.copy_of_this_binary("checkout/target/debug/deps/repo_root-kept");
    assert_eq!(root_found_by(&kept, Some(&this_checkout().join("xtask"))).canonicalize().expect("a root that is there"), this_checkout());
    assert_eq!(root_found_by(&kept, Some(&t.0.join("elsewhere/package"))), other);
}

/// Goal: a binary outside any checkout, with nothing from cargo, still
/// works in the checkout that built it, as before. Valid: a copy of this
/// binary in a bare folder.
#[test]
fn a_build_outside_any_checkout_works_where_it_was_built() {
    let t = Temp::new();
    let stray = t.copy_of_this_binary("bin/repo_root-stray");
    assert_eq!(root_found_by(&stray, None).canonicalize().expect("a root that is there"), this_checkout());
}
