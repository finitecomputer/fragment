//! A released CLI warns once, on stderr, only when production advertises
//! a newer published CLI for its inputs. Platform commits are independent;
//! missing headers, local builds and other hosts are silent. No extra call.
use std::sync::atomic::{AtomicBool, Ordering};
use fragment_core::release::cli_version;

const RELEASED_VERSION: Option<&str> = option_env!("FRAGMENT_CLI_VERSION");
static SAID: AtomicBool = AtomicBool::new(false);

fn newer(host: Option<&str>, own: Option<&str>, available: Option<&str>) -> bool {
    host == crate::DEFAULT_HOST.strip_prefix("https://")
        && matches!((own.and_then(cli_version), available.and_then(cli_version)), (Some(own), Some(available)) if available > own)
}

fn install(version: &str) -> String {
    crate::SKILL.lines().find(|l| l.contains("releases/latest/download"))
        .unwrap_or("see https://finite.chat/llms.txt").trim()
        .replace("releases/latest/download", &format!("releases/download/v{version}"))
}

/// Each answer's host and `x-fragment-cli`, as it arrives. The header
/// names a release already published and verified before this deploy.
pub fn heard(host: Option<&str>, available: Option<&str>) {
    if newer(host, RELEASED_VERSION, available) && !SAID.swap(true, Ordering::Relaxed) {
        let version = available.expect("newer requires a version");
        eprintln!("fragment: CLI {version} matching {} is out; update it with:\n  {}", crate::DEFAULT_HOST, install(version));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_newer_published_version_on_production_warns() {
        let prod = Some("finite.chat");
        assert!(newer(prod, Some("0.15.9"), Some("0.15.10")));
        assert!(newer(prod, Some("0.15.10"), Some("0.16.0")));
        assert!(newer(prod, Some("0.99.99"), Some("1.0.0")));
        for available in [None, Some(""), Some("0.15.10"), Some("0.15.9"), Some("0.15.11-rc1"), Some("v0.16.0"), Some("bogus"), Some("0.15.💥")] {
            assert!(!newer(prod, Some("0.15.10"), available), "{available:?}");
        }
        for own in [None, Some(""), Some("dev")] { assert!(!newer(prod, own, Some("0.15.10"))); }
        for host in [None, Some("preview.finite.build"), Some("localhost"), Some("finite.chat.attacker.test")] {
            assert!(!newer(host, Some("0.15.9"), Some("0.15.10")));
        }
    }

    #[test]
    fn the_install_fetches_the_advertised_release_even_after_a_rollback() {
        let command = install("0.15.1");
        assert!(command.contains("releases/download/v0.15.1/fragment-$(uname -s)-$(uname -m).tar.gz"));
        assert!(!command.contains("releases/latest"));
    }
}
