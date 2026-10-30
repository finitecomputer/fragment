//! A released CLI out of step with production says so, once, on stderr. Its
//! build names the commit it was built from (`FRAGMENT_CLI_COMMIT`,
//! .github/workflows/release.yml), and production names its own on every
//! answer (`x-fragment-deploy`); production's deploy releases a CLI of its
//! commit (ci.yml's `cli-release`), so a difference means a matching one is
//! out. A local build names no commit, and another host (a preview, a dev
//! stack) says nothing: both are silent.
use std::sync::atomic::{AtomicBool, Ordering};

const BUILT_FROM: Option<&str> = option_env!("FRAGMENT_CLI_COMMIT");
static SAID: AtomicBool = AtomicBool::new(false);

/// The commits compared: `x-fragment-deploy`'s first 12 hex digits.
fn short(commit: &str) -> &str {
    &commit[..commit.len().min(12)]
}

fn out_of_step(own: Option<&str>, deployed: Option<&str>) -> bool {
    matches!((own, deployed), (Some(own), Some(deployed)) if !own.is_empty() && !deployed.is_empty() && short(own) != short(deployed))
}

/// Each answer's host and `x-fragment-deploy`, as it arrives.
pub fn heard(host: Option<&str>, deployed: Option<&str>) {
    let production = crate::DEFAULT_HOST.strip_prefix("https://");
    if host == production && out_of_step(BUILT_FROM, deployed) && !SAID.swap(true, Ordering::Relaxed) {
        let install = crate::SKILL.lines().find(|l| l.contains("releases/latest/download")).unwrap_or("see https://finite.chat/llms.txt");
        eprintln!("fragment: a CLI matching {} is out; update it with:\n  {}", crate::DEFAULT_HOST, install.trim());
    }
}

#[cfg(test)]
mod tests {
    use super::out_of_step;

    #[test]
    fn says_so_only_when_both_commits_are_known_and_differ() {
        assert!(out_of_step(Some("9ef22ed57c5a"), Some("cc15f8350000")));
        assert!(out_of_step(Some("9ef22ed57c5a6dca"), Some("cc15f8350000")));
        assert!(!out_of_step(Some("9ef22ed57c5a6dca"), Some("9ef22ed57c5a")));
        assert!(!out_of_step(None, Some("9ef22ed57c5a")));
        assert!(!out_of_step(Some("9ef22ed57c5a"), None));
        assert!(!out_of_step(Some(""), Some("9ef22ed57c5a")));
    }
}
