//! `fragment private-home`: what the owner of a private home says about it,
//! signed with this machine's nostr key (the key the CLI signs its requests
//! with), for the home's key service (sandcastle's docs/nitro-keys.md). It
//! prints the signed statement; it sends nothing.
//!
//! A statement is a nostr event (NIP-01) of kind 30078 (NIP-78), its `d`
//! tag `finite.home/<home>/<slot>`, its content one of `Says` as JSON. A
//! newer statement in the same slot replaces the older one.

use anyhow::{bail, Result};
use clap::Subcommand;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::auth::Identity;

pub const KIND: u64 = 30078;
pub const D_PREFIX: &str = "finite.home/";
/// A repair grant's length, at most, in hours (the key service's own bound).
pub const REPAIR_HOURS_MAX: u64 = 24;
const HOME_BYTES_MAX: usize = 64;

#[derive(Subcommand)]
pub enum Cmd {
    /// The default, signed once: release the home's key to an image Finite
    /// published in the public log, a day after it was published
    AcceptReleases {
        #[arg(long)]
        home: String,
        /// How long a published image waits before it is accepted
        #[arg(long, default_value_t = 24)]
        after_hours: u64,
    },
    /// Release the home's key to one image, by its PCR0, now
    Release {
        #[arg(long)]
        home: String,
        #[arg(long)]
        pcr0: String,
    },
    /// Let one person repair the home for a while (at most a day)
    Repair {
        #[arg(long)]
        home: String,
        /// Their npub (or 64-hex key)
        #[arg(long)]
        to: String,
        #[arg(long)]
        hours: u64,
    },
    /// Withdraw a statement: `releases`, `release/<pcr0>` or `repair/<key>`
    Withdraw {
        #[arg(long)]
        home: String,
        #[arg(long)]
        slot: String,
    },
}

/// What a statement says. Its JSON is the key service's (`says` names the
/// kind, kebab-case).
#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(tag = "says", rename_all = "kebab-case")]
pub enum Says {
    AcceptPublishedReleases { after_s: u64 },
    Release { pcr0: String },
    Repair { to: String, until: u64 },
    Withdraw,
}

#[derive(Serialize, Debug)]
pub struct Event {
    pub id: String,
    pub pubkey: String,
    pub created_at: u64,
    pub kind: u64,
    pub tags: Vec<Vec<String>>,
    pub content: String,
    pub sig: String,
}

fn valid_home(home: &str) -> bool {
    !home.is_empty() && home.len() <= HOME_BYTES_MAX && home.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn now_s() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("the clock is past 1970").as_secs()
}

/// `says` about `home` in `slot`, signed by `identity` at `created_at`.
pub fn sign(identity: &Identity, home: &str, slot: &str, says: &Says, created_at: u64) -> Event {
    let pubkey = identity.pubkey_hex().to_string();
    let tags = vec![vec!["d".to_string(), format!("{D_PREFIX}{home}/{slot}")]];
    let content = serde_json::to_string(says).expect("a statement serializes");
    // NIP-01: the id is the hash of [0, pubkey, created_at, kind, tags, content]
    let serialized = serde_json::to_string(&(0, &pubkey, created_at, KIND, &tags, &content)).expect("an event serializes");
    let id: [u8; 32] = Sha256::digest(serialized.as_bytes()).into();
    let sig = identity.sign_digest(&id);
    Event { id: hex::encode(id), pubkey, created_at, kind: KIND, tags, content, sig }
}

/// The statement a command makes, and its slot.
pub fn statement(cmd: Cmd, now: u64) -> Result<(String, String, Says)> {
    let (home, slot, says) = match cmd {
        Cmd::AcceptReleases { home, after_hours } => (home, "releases".to_string(), Says::AcceptPublishedReleases { after_s: after_hours * 3600 }),
        Cmd::Release { home, pcr0 } => {
            if pcr0.len() != 96 || !pcr0.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
                bail!("a PCR0 is 96 lowercase hex digits");
            }
            (home, format!("release/{pcr0}"), Says::Release { pcr0 })
        }
        Cmd::Repair { home, to, hours } => {
            let Some(to) = fragment_core::npub::parse(&to) else {
                bail!("--to is a person's npub or 64-hex key");
            };
            if hours == 0 || hours > REPAIR_HOURS_MAX {
                bail!("a repair is granted for 1 to {REPAIR_HOURS_MAX} hours");
            }
            (home, format!("repair/{to}"), Says::Repair { to, until: now + hours * 3600 })
        }
        Cmd::Withdraw { home, slot } => (home, slot, Says::Withdraw),
    };
    if !valid_home(&home) {
        bail!("--home is [A-Za-z0-9_-], 1 to {HOME_BYTES_MAX} characters");
    }
    Ok((home, slot, says))
}

/// The command: its statement, signed now, as JSON on stdout.
pub fn run(cmd: Cmd, identity: &Identity) -> Result<()> {
    let now = now_s();
    let (home, slot, says) = statement(cmd, now)?;
    let event = sign(identity, &home, &slot, &says, now);
    println!("{}", serde_json::to_string(&event).expect("an event serializes"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use k256::schnorr::{Signature, VerifyingKey};

    fn verifies(event: &Event) -> bool {
        let serialized = serde_json::to_string(&(0, &event.pubkey, event.created_at, event.kind, &event.tags, &event.content)).unwrap();
        let id: [u8; 32] = Sha256::digest(serialized.as_bytes()).into();
        let key = VerifyingKey::from_bytes(&hex::decode(&event.pubkey).unwrap()).unwrap();
        let sig = Signature::try_from(hex::decode(&event.sig).unwrap().as_slice()).unwrap();
        hex::encode(id) == event.id && key.verify_raw(&id, &sig).is_ok()
    }

    #[test]
    fn a_statement_is_a_signed_nostr_event_the_service_reads() {
        let me = crate::auth::fixed(1);
        let (home, slot, says) = statement(Cmd::AcceptReleases { home: "home-1".into(), after_hours: 24 }, 1_000).unwrap();
        let event = sign(&me, &home, &slot, &says, 1_000);
        assert!(verifies(&event));
        assert_eq!(event.kind, KIND);
        assert_eq!(event.tags, vec![vec!["d".to_string(), "finite.home/home-1/releases".to_string()]]);
        assert_eq!(event.content, r#"{"says":"accept-published-releases","after_s":86400}"#);
    }

    #[test]
    fn a_repair_grant_names_a_person_and_ends() {
        let npub = crate::auth::fixed(2).npub();
        let (_, slot, says) = statement(Cmd::Repair { home: "h".into(), to: npub, hours: 2 }, 1_000).unwrap();
        let hex = crate::auth::fixed(2).pubkey_hex().to_string();
        assert_eq!(slot, format!("repair/{hex}"));
        assert_eq!(says, Says::Repair { to: hex, until: 1_000 + 7_200 });
        assert!(statement(Cmd::Repair { home: "h".into(), to: "nobody".into(), hours: 2 }, 0).is_err());
        assert!(statement(Cmd::Repair { home: "h".into(), to: crate::auth::fixed(2).npub(), hours: 25 }, 0).is_err());
    }

    #[test]
    fn shapes_are_checked_before_signing() {
        assert!(statement(Cmd::Release { home: "h".into(), pcr0: "AB".repeat(48) }, 0).is_err());
        assert!(statement(Cmd::Release { home: "h".into(), pcr0: "ab".repeat(48) }, 0).is_ok());
        assert!(statement(Cmd::AcceptReleases { home: "a home".into(), after_hours: 1 }, 0).is_err());
    }
}
