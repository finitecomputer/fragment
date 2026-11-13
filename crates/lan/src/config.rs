//! `fragment-lan`'s config: a JSON file its operator writes, checked before
//! anything is made or bound.
//!
//! ```json
//! {
//!   "zone": "fragment.home.arpa",
//!   "address": "100.101.102.103",
//!   "https_port": 443,
//!   "http_port": 80,
//!   "dns_port": 53,
//!   "node": "127.0.0.1:8790",
//!   "state_dir": "/var/lib/fragment-lan"
//! }
//! ```
//!
//! - `address` is the one address every listener binds and the zone's names
//!   answer: a tailnet's, a LAN's. Never an unspecified one (`0.0.0.0`,
//!   `::`), so nothing listens where its operator did not say.
//! - `http_port` (the root's page) and `dns_port` may be left out (none).
//!   A port of 0 picks a free one (tests and dev).
//! - `node` is the node's own port on this box (loopback): the door's one
//!   upstream, plain http beside it.
//! - `state_dir` holds the root (`ca.rs`), 0700.

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::zone::{Zone, ZoneError};

/// A config file at most.
pub const CONFIG_BYTES_MAX: u64 = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub zone: String,
    pub address: IpAddr,
    pub https_port: u16,
    #[serde(default)]
    pub http_port: Option<u16>,
    #[serde(default)]
    pub dns_port: Option<u16>,
    pub node: SocketAddr,
    pub state_dir: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{path}: {source}")]
    Read { path: PathBuf, source: std::io::Error },
    #[error("{path} is past {CONFIG_BYTES_MAX} bytes")]
    TooLong { path: PathBuf },
    #[error("{path}: {source}")]
    Json { path: PathBuf, source: serde_json::Error },
    #[error(transparent)]
    Zone(#[from] ZoneError),
    #[error("address {0} is {1}: name the one address to listen on (a tailnet's, a LAN's)")]
    Address(IpAddr, &'static str),
    #[error("node {0} is not on this box's loopback: the door speaks plain http to it")]
    Node(SocketAddr),
    #[error("{0} and {1} are the same port: each listener has its own")]
    SamePort(&'static str, &'static str),
}

impl Config {
    /// The config in `path`, read and checked.
    pub fn read(path: &Path) -> Result<Config, ConfigError> {
        let read = |source| ConfigError::Read { path: path.to_path_buf(), source };
        let len = std::fs::metadata(path).map_err(read)?.len();
        if len > CONFIG_BYTES_MAX {
            return Err(ConfigError::TooLong { path: path.to_path_buf() });
        }
        let text = std::fs::read_to_string(path).map_err(read)?;
        let config: Config = serde_json::from_str(&text).map_err(|source| ConfigError::Json { path: path.to_path_buf(), source })?;
        config.check()?;
        Ok(config)
    }

    /// The zone, once every field is one this service can serve.
    pub fn check(&self) -> Result<Zone, ConfigError> {
        let zone = Zone::parse(&self.zone)?;
        let refused = match self.address {
            a if a.is_unspecified() => Some("unspecified: it would listen on every address"),
            a if a.is_multicast() => Some("multicast"),
            IpAddr::V4(a) if a.is_broadcast() => Some("the broadcast address"),
            _ => None,
        };
        if let Some(why) = refused {
            return Err(ConfigError::Address(self.address, why));
        }
        if !self.node.ip().is_loopback() {
            return Err(ConfigError::Node(self.node));
        }
        let ports = [("https_port", Some(self.https_port)), ("http_port", self.http_port), ("dns_port", self.dns_port)];
        for (i, (a, pa)) in ports.iter().enumerate() {
            for (b, pb) in &ports[i + 1..] {
                if pa.is_some_and(|p| p != 0) && pa == pb {
                    return Err(ConfigError::SamePort(a, b));
                }
            }
        }
        Ok(zone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config {
            zone: "fragment.home.arpa".into(),
            address: "100.64.0.7".parse().unwrap(),
            https_port: 443,
            http_port: Some(80),
            dns_port: Some(53),
            node: "127.0.0.1:8790".parse().unwrap(),
            state_dir: "/var/lib/fragment-lan".into(),
        }
    }

    /// Goal: a config as an operator writes it reads back the same, and the
    /// optional listeners may be left out.
    #[test]
    fn a_config_reads_as_written() {
        let ok = config();
        assert_eq!(ok.check().unwrap().name(), "fragment.home.arpa");
        let text = serde_json::to_string(&ok).unwrap();
        assert_eq!(serde_json::from_str::<Config>(&text).unwrap(), ok);
        let bare: Config = serde_json::from_str(r#"{"zone":"a.b","address":"10.0.0.2","https_port":8443,"node":"127.0.0.1:8790","state_dir":"/x"}"#).unwrap();
        assert_eq!((bare.http_port, bare.dns_port), (None, None));
        bare.check().unwrap();
    }

    /// Goal (invalid): every address, a node off the box, a shared port, a
    /// bad zone and an unknown field are refused before anything binds.
    #[test]
    fn what_would_listen_elsewhere_is_refused() {
        for (address, why) in [("0.0.0.0", "unspecified"), ("::", "unspecified"), ("224.0.0.1", "multicast"), ("255.255.255.255", "broadcast")] {
            let e = Config { address: address.parse().unwrap(), ..config() }.check().unwrap_err();
            assert!(matches!(e, ConfigError::Address(..)) && e.to_string().contains(why), "{address}: {e}");
        }
        assert!(matches!(Config { node: "10.0.0.2:8790".parse().unwrap(), ..config() }.check(), Err(ConfigError::Node(_))));
        assert!(matches!(Config { http_port: Some(443), ..config() }.check(), Err(ConfigError::SamePort("https_port", "http_port"))));
        assert!(Config { https_port: 0, http_port: Some(0), dns_port: Some(0), ..config() }.check().is_ok(), "ports picked are each their own");
        assert!(matches!(Config { zone: "Fragment.home.arpa".into(), ..config() }.check(), Err(ConfigError::Zone(_))));
        let text = serde_json::to_string(&config()).unwrap().replacen('{', "{\"listen_all\":true,", 1);
        assert!(serde_json::from_str::<Config>(&text).is_err(), "an unknown field");
    }
}
