//! SimpleX on an always-on computer (docs/cloudflare-v1.md, decision 32):
//! one agent's link to its owner's SimpleX app, through SimpleX's own core,
//! `simplex-chat` (`VERSION`), run beside the bridge and driven over its
//! bot API (`-p`, on loopback only, one client: the bridge).
//!
//! - `wire`: that API, the commands sent and the frames read, typed from
//!   what the real core sent in the lab (tests/simplex.rs).
//! - `link`: the rules, pure: linking by a one-time invitation, the owner's
//!   contact alone, messages each way, and a rollback's cure.
//! - `run`: the bridge's I/O around them: each linked agent's core as a
//!   child process, only while the computer is always on, stopped under
//!   every hold, its link's file, a save asked when it changes.
//! - `http`: each agent's link on the screen port (`/simplex/…`).
//! - `door`: only its owner links or unlinks it (a ticket the platform
//!   signs), and only the link's door key speaks for them (Paul's design of
//!   2026-10-23).
//! - `socks`: the cores' way out: SimpleX's file servers through the
//!   platform's XFTP relay, everything else straight out of the computer.
//!
//! What it adds to the platform, entry by entry, is
//! docs/explorations/simplex-complexity.md.

pub mod door;
pub mod http;
pub mod link;
pub mod run;
pub mod socks;
pub mod wire;

/// The core's release, pinned: the latest stable on 2026-10-22, the one the
/// lab ran. Its Ubuntu 24.04 x86_64 binary runs on our Hermes image's base
/// (Debian trixie) with the libraries it already has.
pub const VERSION: &str = "v7.0.3";
/// That binary's SHA-256, the release's own (GitHub's asset digest).
pub const SHA256_X86_64: &str = "895fb14cfaa662d1c0947f2f871141fc89680fe3e70bff64c366cbe9e59aa4f0";
/// What the binary says it is (`--version`).
pub const REPORTS: &str = "SimpleX Chat v7.0.0.12";
