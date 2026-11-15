//! pod-keyd's command line. No config file: its limits are constants,
//! the owners it serves are named here, and everything else it knows,
//! they tell it.

use crate::store::MAX_RECORDS;
use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;

pub const DEFAULT_PORT: u16 = 7100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub bind: IpAddr,
    pub port: u16,
    pub configfs: PathBuf,
    pub ccel: PathBuf,
    /// The owners it takes deposits from; one at least.
    pub owners: HashSet<[u8; 32]>,
}

pub const USAGE: &str = "usage: pod-keyd --owner <key> [--owner <key>]... [--bind <addr>] [--port <port>] [--configfs <dir>] [--ccel <file>]
  --owner     an owner it takes deposits from: their public key, 64 hex digits; up to 1024
  --bind      address to listen on (default 0.0.0.0)
  --port      port (default 7100)
  --configfs  report directory root (default /sys/kernel/config/tsm/report)
  --ccel      event log (default /sys/firmware/acpi/tables/data/CCEL)";

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut out = Args {
        bind: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        port: DEFAULT_PORT,
        configfs: PathBuf::from(fragment_pod_init::tsm::CONFIGFS_REPORT),
        ccel: PathBuf::from(fragment_pod_init::tsm::CCEL_DATA),
        owners: HashSet::new(),
    };
    let mut it = args.into_iter();
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--bind" => {
                let v = value()?;
                out.bind = v.parse().map_err(|_| format!("--bind {v:?} is not an IP address"))?;
            }
            "--port" => {
                let v = value()?;
                out.port = v.parse().ok().filter(|p| *p != 0).ok_or(format!("--port {v:?} is not a port"))?;
            }
            "--owner" => {
                let v = value()?;
                let mut key = [0u8; 32];
                hex::decode_to_slice(&v, &mut key).map_err(|_| format!("--owner {v:?} is not a public key: 64 hex digits"))?;
                if !out.owners.insert(key) {
                    return Err(format!("--owner {v} is named twice"));
                }
            }
            "--configfs" => out.configfs = PathBuf::from(value()?),
            "--ccel" => out.ccel = PathBuf::from(value()?),
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    if out.owners.is_empty() {
        return Err("name at least one --owner: pod-keyd takes deposits from no one else".into());
    }
    if out.owners.len() > MAX_RECORDS {
        return Err(format!("{} owners; pod-keyd serves at most {MAX_RECORDS}", out.owners.len()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(n: u8) -> String {
        hex::encode([n; 32])
    }

    fn with(rest: &[&str]) -> Result<Args, String> {
        parse(["--owner".to_owned(), owner(1)].into_iter().chain(rest.iter().map(|s| (*s).to_owned())))
    }

    #[test]
    fn defaults_to_all_ipv4_on_7100() {
        let a = with(&[]).unwrap();
        assert_eq!((a.bind, a.port), (IpAddr::V4(Ipv4Addr::UNSPECIFIED), 7100));
        assert_eq!(with(&["--port", "7200"]).unwrap().port, 7200);
        assert!(with(&["--port", "0"]).is_err());
        assert!(with(&["--config", "x"]).is_err());
    }

    #[test]
    fn serves_the_owners_it_is_given_and_needs_one() {
        assert!(parse(Vec::new()).unwrap_err().contains("at least one --owner"));
        let a = with(&["--owner", &owner(2)]).unwrap();
        assert_eq!(a.owners, HashSet::from([[1; 32], [2; 32]]));
        assert!(with(&["--owner", &owner(1)]).unwrap_err().contains("twice"));
        for bad in ["", "00", "npub1xyz", &"zz".repeat(32), &"00".repeat(33)] {
            assert!(with(&["--owner", bad]).unwrap_err().contains("64 hex digits"), "{bad:?}");
        }
        assert!(with(&["--owner"]).unwrap_err().contains("needs a value"));
        let mut many = Vec::new();
        for i in 0..=MAX_RECORDS as u16 {
            let mut key = [0u8; 32];
            key[..2].copy_from_slice(&i.to_be_bytes());
            many.extend(["--owner".to_owned(), hex::encode(key)]);
        }
        assert!(parse(many).unwrap_err().contains("at most 1024"));
    }
}
