//! LAN mode (docs/self-host.md, seam 9): a self-hosted box's own DNS
//! zone, a TLS front door on the one address its operator names, and the
//! box's own certificate authority, constrained to the zone. The platform
//! is at `https://<zone>`, each fragment at `https://<name>.<zone>`, its
//! own origin, as on Cloudflare: so a box on a tailnet or a LAN gets what
//! only an https origin has (`__Host-` cookies, service workers, the
//! clipboard), with no public DNS name and no public CA.
//!
//! One process (`fragment-lan --config <file>`) serves all three:
//!
//! - `dns`: authoritative for the zone alone, every other name refused;
//! - `door`: TLS for the zone in front of the node's own port, the edge
//!   that says who asked (`CF-Connecting-IP`), and the root's page over
//!   plain http;
//! - `ca`: the root, made once and kept, and the zone's certificate, made
//!   anew at each start.
//!
//! It needs nothing of the runtime's: the node behind it is wrangler's
//! workerd or celld alike, and the cell has no code of its own for it
//! (its configuration names `https://<zone>` as its platform and the zone
//! as its fragments' suffix).

pub mod ca;
pub mod config;
pub mod dns;
pub mod door;
mod serve;
pub mod zone;

pub use config::{Config, ConfigError};
pub use serve::{Lan, StartError};
pub use zone::Zone;
