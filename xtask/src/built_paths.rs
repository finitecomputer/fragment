//! `cargo xtask check`'s built-path pass: nothing in the workspace compiles
//! in where its checkout is.
//!
//! Cargo takes a crate as built wherever its checkout sits: what it
//! compares leaves out the paths it hands rustc, and the variables it sets
//! itself are dropped from what a crate is rebuilt for. So a target
//! directory kept from another checkout (cloned into a new worktree,
//! restored at another mount, shared through `CARGO_TARGET_DIR`) holds
//! every unchanged crate with the first checkout's paths in it, and cargo
//! says `Fresh`. `env!("CARGO_MANIFEST_DIR")` then reads, writes and tests
//! that other checkout (xtask checked, built and deployed it; a test read
//! its fixtures and passed), and `env!("CARGO_BIN_EXE_…")` runs its binary.
//!
//! What to write instead:
//! - the repo root is `devstack::repo_root()`, found as the process runs;
//! - a test of a crate that has no devstack names its files relative to
//!   its package's folder, where cargo runs it;
//! - a test's binary is `std::env::var_os("CARGO_BIN_EXE_…")`, which cargo
//!   sets for the test as it runs.
//!
//! Which files: each `.rs` git tracks or would add (`js_syntax::listed`)
//! under the workspace's members, comments included, so no exception hides
//! in one. `images/` and `bench/` are workspaces of their own, built in
//! target directories nothing hands to another checkout, and `cell/` reads
//! no file as it runs.
//!
//! The one exception is `ALLOWED`: `devstack::repo_root`'s last resort,
//! for a binary run from outside any checkout.

use std::path::Path;

use anyhow::{ensure, Result};

/// The workspace's members' folders (Cargo.toml).
const MEMBERS: [&str; 3] = ["cli/", "crates/", "xtask/"];
/// What compiles a checkout's path in.
const COMPILED_IN: [&str; 2] = ["env!(\"CARGO_MANIFEST_DIR\")", "env!(\"CARGO_BIN_EXE_"];
/// The file that may, and how many times.
pub const ALLOWED: (&str, usize) = ("crates/devstack/src/lib.rs", 1);
/// This file, which names them to refuse them.
const THIS: &str = "xtask/src/built_paths.rs";

/// The workspace's Rust, by path, with its text.
pub fn sources(root: &Path) -> Result<Vec<(String, String)>> {
    let mut sources = vec![];
    for path in crate::js_syntax::listed(root)? {
        let read = path.ends_with(".rs") && path != THIS && MEMBERS.iter().any(|m| path.starts_with(m));
        if read {
            let text = std::fs::read_to_string(root.join(&path))?;
            sources.push((path, text));
        }
    }
    ensure!(sources.iter().any(|(path, _)| path == ALLOWED.0), "the workspace's Rust was found: {} is not among {} files", ALLOWED.0, sources.len());
    Ok(sources)
}

/// Fails on a compiled-in checkout path outside `allowed`, each named by
/// its file and line, and on an `allowed` that no longer matches.
pub fn check(sources: &[(String, String)], allowed: (&str, usize)) -> Result<()> {
    let mut found = vec![];
    let mut allowed_seen = 0;
    for (path, text) in sources {
        for (i, line) in text.lines().enumerate() {
            let Some(what) = COMPILED_IN.iter().find(|w| line.contains(**w)) else { continue };
            if path == allowed.0 {
                allowed_seen += 1;
            } else {
                found.push(format!("{path}:{}: {what}", i + 1));
            }
        }
    }
    ensure!(
        found.is_empty(),
        "a checkout's path is compiled in, which a build kept from another checkout keeps (xtask/src/built_paths.rs says what to write instead):\n{}",
        found.join("\n")
    );
    ensure!(allowed_seen == allowed.1, "{} compiles a checkout's path in {allowed_seen} times, not the {} allowed: update ALLOWED", allowed.0, allowed.1);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(path: &str, text: &str) -> (String, String) {
        (path.to_string(), text.to_string())
    }

    /// Goal: each way of compiling a checkout's path in fails the check,
    /// named by its file and line. Method: one planted use of each, in
    /// code, in a `concat!`, and in a comment.
    #[test]
    fn a_planted_compiled_in_path_fails_the_check_naming_its_line() {
        let planted = [
            ("crates/core/src/x.rs", "fn root() -> &'static str {\n    env!(\"CARGO_MANIFEST_DIR\")\n}\n", 2),
            ("cli/tests/x.rs", "const F: &str = concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/fixtures\");\n", 1),
            ("cli/tests/y.rs", "\n\nlet c = Command::new(env!(\"CARGO_BIN_EXE_fragment\"));\n", 3),
            ("xtask/src/x.rs", "// was: option_env!(\"CARGO_MANIFEST_DIR\")\n", 1),
        ];
        for (path, text, line) in planted {
            let said = check(&[source(path, text)], ("crates/devstack/src/lib.rs", 0)).expect_err(path).to_string();
            assert!(said.contains(&format!("{path}:{line}: ")), "{said}");
        }
    }

    /// Goal: what is read as the process runs passes, and the allowed
    /// file passes only at its count. Valid: cargo's variables read at run
    /// time, and a build script's own. Invalid: the allowed file with a
    /// second use, and with none (a stale entry).
    #[test]
    fn what_is_read_at_run_time_passes_and_the_exception_is_exact() {
        let fine = [
            source("crates/x/src/lib.rs", "let dir = std::env::var_os(\"CARGO_MANIFEST_DIR\");\n"),
            source("cli/tests/x.rs", "let bin = std::env::var_os(\"CARGO_BIN_EXE_fragment\");\n"),
            source("crates/x/build.rs", "let manifest = env::var(\"CARGO_MANIFEST_DIR\").unwrap();\n"),
            source("crates/x/src/v.rs", "const V: &str = env!(\"CARGO_PKG_VERSION\");\n"),
        ];
        check(&fine, ("crates/devstack/src/lib.rs", 0)).unwrap();
        let once = source("crates/devstack/src/lib.rs", "let built = env!(\"CARGO_MANIFEST_DIR\");\n");
        check(std::slice::from_ref(&once), ("crates/devstack/src/lib.rs", 1)).unwrap();
        let twice = source("crates/devstack/src/lib.rs", "let a = env!(\"CARGO_MANIFEST_DIR\");\nlet b = env!(\"CARGO_MANIFEST_DIR\");\n");
        assert!(check(&[twice], ("crates/devstack/src/lib.rs", 1)).unwrap_err().to_string().contains("2 times, not the 1 allowed"));
        assert!(check(&fine, ("crates/devstack/src/lib.rs", 1)).unwrap_err().to_string().contains("0 times, not the 1 allowed"));
    }

    /// Goal: the repo holds none but the one allowed. Method: the repo's
    /// own sources, read as the check reads them.
    #[test]
    fn the_repo_compiles_in_no_checkout_path_but_the_one_allowed() {
        let sources = sources(&crate::devstack::repo_root()).unwrap();
        assert!(sources.len() > 100, "the workspace's Rust: {} files", sources.len());
        check(&sources, ALLOWED).unwrap();
    }
}
