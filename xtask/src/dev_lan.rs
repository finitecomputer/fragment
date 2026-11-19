//! `cargo xtask dev --lan`: the dev stack as a self-hosted box serves it in
//! LAN mode (docs/self-host.md, seam 9): the platform at `https://<zone>`,
//! each fragment at `https://<name>.<zone>`, through `fragment-lan`'s door
//! on one address, the zone answered by its DNS, its certificates the
//! box's own root's. The node keeps its own port on loopback (8790), the
//! door's one upstream, and the CLI may still use it there.
//!
//! The settings, each with its default (`Settings::from_env`):
//! `FRAGMENT_LAN_ZONE` (`fragment.home.arpa`), `FRAGMENT_LAN_ADDRESS`
//! (`127.0.0.1`: a tailnet's or a LAN's address serves other devices),
//! `FRAGMENT_LAN_HTTPS_PORT` (8443), `FRAGMENT_LAN_HTTP_PORT` (8780),
//! `FRAGMENT_LAN_DNS_PORT` (8753) and `FRAGMENT_LAN_STATE`
//! (`target/devstack/lan`: the root, which `--clean` keeps, as devices
//! keep trusting it). Ports below 1024 need the right to bind them.

use std::net::IpAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use fragment_devstack as devstack;
use fragment_lan::zone::https_origin;

pub const DEFAULT_ZONE: &str = "fragment.home.arpa";
const DEFAULT_HTTPS_PORT: u16 = 8443;
const DEFAULT_HTTP_PORT: u16 = 8780;
const DEFAULT_DNS_PORT: u16 = 8753;

/// Where dev's LAN mode serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub zone: String,
    pub address: IpAddr,
    pub https_port: u16,
    pub http_port: u16,
    pub dns_port: u16,
    pub state: PathBuf,
}

impl Settings {
    /// The settings `vars` names (the environment's, in dev), each checked.
    pub fn from_vars(vars: impl Fn(&str) -> Option<String>) -> Result<Settings> {
        let var = |name: &str| vars(name).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let port = |name: &str, default: u16| -> Result<u16> {
            match var(name) {
                None => Ok(default),
                Some(v) => v.parse::<u16>().ok().filter(|p| *p != 0).with_context(|| format!("{name} is a port (1 to 65535), not {v:?}")),
            }
        };
        let settings = Settings {
            zone: var("FRAGMENT_LAN_ZONE").unwrap_or_else(|| DEFAULT_ZONE.into()),
            address: match var("FRAGMENT_LAN_ADDRESS") {
                None => IpAddr::from([127, 0, 0, 1]),
                Some(v) => v.parse().with_context(|| format!("FRAGMENT_LAN_ADDRESS is an IP address, not {v:?}"))?,
            },
            https_port: port("FRAGMENT_LAN_HTTPS_PORT", DEFAULT_HTTPS_PORT)?,
            http_port: port("FRAGMENT_LAN_HTTP_PORT", DEFAULT_HTTP_PORT)?,
            dns_port: port("FRAGMENT_LAN_DNS_PORT", DEFAULT_DNS_PORT)?,
            state: var("FRAGMENT_LAN_STATE").map(PathBuf::from).unwrap_or_else(|| devstack::repo_root().join("target/devstack/lan")),
        };
        // every field as the service checks it, before anything builds
        settings.config(0).check().context("dev's LAN mode")?;
        Ok(settings)
    }

    pub fn from_env() -> Result<Settings> {
        Settings::from_vars(|name| std::env::var(name).ok())
    }

    /// The platform's origin: the zone's apex behind the door.
    pub fn platform_url(&self) -> String {
        https_origin(&self.zone, self.https_port)
    }

    /// `fragment-lan`'s config in front of the node at `node_port`.
    pub fn config(&self, node_port: u16) -> fragment_lan::Config {
        fragment_lan::Config {
            zone: self.zone.clone(),
            address: self.address,
            https_port: self.https_port,
            http_port: Some(self.http_port),
            dns_port: Some(self.dns_port),
            node: std::net::SocketAddr::from(([127, 0, 0, 1], node_port)),
            state_dir: self.state.clone(),
        }
    }
}

/// Starts the service in front of the node at `node_port`, until dropped.
pub fn start(settings: &Settings, node_port: u16) -> Result<fragment_lan::Lan> {
    fragment_lan::Lan::start(settings.config(node_port)).context("start LAN mode (fragment-lan)")
}

/// What the banner says of it: where each thing is, and how a device on
/// this machine reaches it.
pub fn banner(settings: &Settings, lan: &fragment_lan::Lan) -> Vec<String> {
    let (zone, address) = (&settings.zone, settings.address);
    let root = settings.state.join(fragment_lan::ca::ROOT_PEM);
    vec![
        format!("  LAN mode:     {} through fragment-lan's door at {}; fragments at {}", settings.platform_url(), lan.https, https_origin(&format!("<name>.{zone}"), settings.https_port)),
        format!("  its DNS:      {zone} and the names under it at {} (udp and tcp; every other name refused)", lan.dns.map_or("none".into(), |a| a.to_string())),
        format!("  its root:     {} (http://{address}:{}/ca; SHA-256 {})", root.display(), settings.http_port, lan.fingerprint),
        format!("  on this Mac:  printf 'nameserver {address}\\nport {}\\n' | sudo tee /etc/resolver/{zone}; trust {} in Keychain Access", settings.dns_port, root.display()),
        format!("  check it:     curl --cacert {} {}/healthz", root.display(), settings.platform_url()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goal: dev's LAN mode defaults to loopback and high ports; each
    /// setting may be named; a bad one is refused before anything builds.
    #[test]
    fn dev_lan_mode_reads_its_settings() {
        let none = Settings::from_vars(|_| None).unwrap();
        assert_eq!((none.zone.as_str(), none.address, none.https_port, none.http_port, none.dns_port), (DEFAULT_ZONE, IpAddr::from([127, 0, 0, 1]), 8443, 8780, 8753));
        assert_eq!(none.platform_url(), "https://fragment.home.arpa:8443");
        assert!(none.state.ends_with("target/devstack/lan"));
        let named = Settings::from_vars(|n| match n {
            "FRAGMENT_LAN_ZONE" => Some("box.home.arpa".into()),
            "FRAGMENT_LAN_ADDRESS" => Some("100.64.0.7".into()),
            "FRAGMENT_LAN_HTTPS_PORT" => Some("443".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!((named.platform_url(), named.config(8790).node.port()), ("https://box.home.arpa".to_string(), 8790));
        for (name, value) in [("FRAGMENT_LAN_ZONE", "Box.home.arpa"), ("FRAGMENT_LAN_ADDRESS", "0.0.0.0"), ("FRAGMENT_LAN_ADDRESS", "box"), ("FRAGMENT_LAN_DNS_PORT", "0"), ("FRAGMENT_LAN_HTTP_PORT", "8443")] {
            let refused = Settings::from_vars(|n| (n == name).then(|| value.to_string()));
            assert!(refused.is_err(), "{name}={value}");
        }
    }
}
