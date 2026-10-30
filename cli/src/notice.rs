//! The line that says this CLI is not the one its platform ships
//! (fragment_core::cli_build; docs/api.md, "The CLI it ships"): production
//! names its CLI's content id on every answer, and a release's binary
//! carries its own, so a CLI downloaded before a deploy that changed the
//! CLI hears it out of step, says so once a run on stderr, with the
//! install line that fetches the matching one, and goes on: stdout (a
//! `--json` envelope, `fragment mcp`'s messages) never carries it, and
//! nothing waits on it.

use std::sync::atomic::{AtomicBool, Ordering};

/// This build's content id: a release's (`FRAGMENT_CLI_ID`, which the
/// release workflow and the e2e set at build: xtask/src/cli.rs), else none,
/// and a build without one is never out of step (a developer's, or a
/// computer image's, built from the tree its agent's platform runs).
pub const CLI_ID: Option<&str> = match option_env!("FRAGMENT_CLI_ID") {
    Some(id) if !id.is_empty() => Some(id),
    _ => None,
};

/// Whether this run has said it.
static TOLD: AtomicBool = AtomicBool::new(false);

/// An answer's headers, heard: its `x-fragment-cli`, when it names a CLI
/// other than this one, is said on stderr, the first time this run hears it.
pub fn heard(headers: &reqwest::header::HeaderMap) {
    let announced = headers.get(fragment_core::cli_build::HEADER).and_then(|v| v.to_str().ok());
    if let Some(line) = notice(CLI_ID, announced, &TOLD) {
        eprintln!("{line}");
    }
}

/// The line a CLI built as `own` says on hearing `announced`, when the two
/// are out of step and `told` was not yet set (it sets it); else none.
fn notice(own: Option<&str>, announced: Option<&str>, told: &AtomicBool) -> Option<String> {
    if !fragment_core::cli_build::out_of_step(own, announced) || told.swap(true, Ordering::Relaxed) {
        return None;
    }
    Some(format!("fragment: a CLI matching this platform is available (this one is {}, built from other code); install it: {}", crate::VERSION, install()))
}

/// The one-line install, as cli/SKILL.md gives it (`cargo xtask check`
/// holds it to the release's assets).
fn install() -> &'static str {
    crate::SKILL.lines().map(str::trim).find(|l| l.contains("/releases/latest/download/")).unwrap_or("https://github.com/finitecomputer/fragment/releases/latest")
}

#[cfg(test)]
mod tests {
    use super::*;

    const OURS: &str = "b3f36423ad3f0625";
    const THEIRS: &str = "d2a41ec365eef89f";

    /// Valid, replayed and restarted: out of step, the first answer of a
    /// run says it, naming this version and the install line; every later
    /// one says nothing; a new run (a new flag) says it again.
    #[test]
    fn out_of_step_is_said_once_a_run() {
        let told = AtomicBool::new(false);
        let line = notice(Some(OURS), Some(THEIRS), &told).expect("said");
        assert!(line.starts_with("fragment: a CLI matching this platform is available"), "{line}");
        assert!(line.contains(crate::VERSION) && line.contains(install()) && !line.contains('\n'), "{line}");
        assert!(install().contains("releases/latest/download/fragment-$(uname -s)-$(uname -m).tar.gz"), "{}", install());
        assert_eq!(notice(Some(OURS), Some(THEIRS), &told), None, "said once");
        assert_eq!(notice(Some(OURS), Some("0123456789abcdef"), &told), None, "once, whatever answers next");
        assert!(notice(Some(OURS), Some(THEIRS), &AtomicBool::new(false)).is_some(), "the next run says it again");
    }

    /// In step, or with no id on a side, or a malformed one: nothing is
    /// said, and nothing is spent, so a later answer out of step still is.
    #[test]
    fn nothing_else_is_said() {
        let told = AtomicBool::new(false);
        for (own, announced) in [(Some(OURS), Some(OURS)), (None, Some(THEIRS)), (Some(OURS), None), (Some(OURS), Some("D2A41EC365EEF89F")), (Some(OURS), Some("")), (Some("not-an-id"), Some(THEIRS))] {
            assert_eq!(notice(own, announced, &told), None, "{own:?} {announced:?}");
        }
        assert!(notice(Some(OURS), Some(THEIRS), &told).is_some());
    }

    /// A test build carries no id, as a developer's does: it is never told.
    #[test]
    fn a_build_without_an_id_is_never_told() {
        if option_env!("FRAGMENT_CLI_ID").is_none() {
            assert_eq!(CLI_ID, None);
            assert_eq!(notice(CLI_ID, Some(THEIRS), &AtomicBool::new(false)), None);
        }
    }
}
