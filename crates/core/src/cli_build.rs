//! Which CLI build a platform ships (docs/api.md, "The CLI it ships").
//! The install line every guide gives fetches the latest release, which
//! production's deploy publishes whenever the CLI's code changed (ci.yml,
//! `cli-plan`), so a CLI downloaded before a deploy can lack what the
//! platform's guide now tells it to run.
//!
//! Both sides name a build by its content id (xtask/src/cli.rs: a hash of
//! what the CLI is built from, at one commit, never of the commit): a
//! release's binary carries its own (`FRAGMENT_CLI_ID` at build), and
//! production's Worker the one of the tree it was deployed from, in
//! `HEADER` on every answer of its host. They are compared, never versions:
//! a deploy that did not touch the CLI moves neither id, and a rollback
//! that brought older CLI code back is out of step with the newer release
//! even though its version is lower. A build without an id (a developer's,
//! a computer image's) and a platform that names none (a branch's, dev's)
//! are never out of step.

/// The header every answer of production's host names the CLI build it
/// ships in (its `FRAGMENT_CLI_ID`).
pub const HEADER: &str = "x-fragment-cli";

/// An id's length: the first 16 hex digits of a SHA-256.
pub const ID_HEX: usize = 16;

/// Whether `id` is a content id: `ID_HEX` lowercase hex digits.
pub fn valid(id: &str) -> bool {
    id.len() == ID_HEX && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Whether a CLI built as `own` is out of step with a platform that ships
/// `announced`: both are ids, and they differ. Anything else (no id on
/// either side, a malformed one) is not, so the CLI says nothing.
pub fn out_of_step(own: Option<&str>, announced: Option<&str>) -> bool {
    match (own, announced) {
        (Some(own), Some(announced)) => valid(own) && valid(announced) && own != announced,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OURS: &str = "b3f36423ad3f0625";
    const THEIRS: &str = "d2a41ec365eef89f";

    /// Valid: a release's id against a platform shipping another is out of
    /// step; against its own, not.
    #[test]
    fn two_ids_that_differ_are_out_of_step() {
        assert!(out_of_step(Some(OURS), Some(THEIRS)));
        assert!(out_of_step(Some(THEIRS), Some(OURS)), "whichever is newer: ids have no order");
        assert!(!out_of_step(Some(OURS), Some(OURS)));
    }

    /// No id on either side is never out of step: a developer's or a
    /// computer image's build, or a branch's or dev's platform.
    #[test]
    fn a_side_without_an_id_never_is() {
        assert!(!out_of_step(None, Some(THEIRS)));
        assert!(!out_of_step(Some(OURS), None));
        assert!(!out_of_step(None, None));
    }

    /// Invalid: a malformed id on either side (empty, short, long, upper
    /// case, not hex, padded) is no id, and says nothing.
    #[test]
    fn a_malformed_id_is_none() {
        for bad in ["", "b3f36423ad3f062", "b3f36423ad3f06250", "B3F36423AD3F0625", "g3f36423ad3f0625", " b3f36423ad3f062", "b3f36423-d3f0625"] {
            assert!(!valid(bad), "{bad:?}");
            assert!(!out_of_step(Some(OURS), Some(bad)), "announced {bad:?}");
            assert!(!out_of_step(Some(bad), Some(THEIRS)), "built {bad:?}");
        }
        assert!(valid(OURS) && valid(THEIRS) && valid("0123456789abcdef"));
    }
}
