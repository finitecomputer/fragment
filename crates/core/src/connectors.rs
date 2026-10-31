//! The deployment's connectors (docs/computers.md, "Connector policy"):
//! which doors an owner may open, and the exact TCP endpoints a computer's
//! relay carries for them. A connector's id is the same whatever carries
//! it (an image's adapter, a hosted fragment); its relay targets are only
//! for an image's. Live org policy will intersect here (`authorize`).

use fragment_proto::computer::RelayTarget;
use serde::{Deserialize, Serialize};

/// Connectors a deployment names, at most.
pub const CONNECTORS_MAX: usize = 16;
/// Relay targets one connector names, at most.
pub const TARGETS_MAX: usize = 32;

/// One connector the deployment allows, and the exact endpoints its
/// computer adapter may reach through the relay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Connector {
    pub id: String,
    #[serde(default)]
    pub relay_targets: Vec<RelayTarget>,
}

/// The deployment's connectors, checked (`new`). Absent from a config, it
/// is `default()`: SimpleX, with `relay::HOSTS` on 443, and Telegram (a
/// hosted connector: templates/telegram, no relay). `[]` allows none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy(Vec<Connector>);

/// What is asked of a connector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// An owner's link ticket.
    Link,
    /// A link's redemption, as the door's key or actor is registered.
    Redeem,
    /// A relayed record's check.
    Check,
    /// An owner's unlink: allowed whatever the policy says now.
    Unlink,
    /// A computer's relay to one of the connector's targets.
    Relay,
}

impl Default for Policy {
    fn default() -> Self {
        let targets = crate::relay::HOSTS.iter().map(|h| RelayTarget { host: (*h).into(), port: crate::relay::PORT }).collect();
        Policy::new(vec![Connector { id: "simplex".into(), relay_targets: targets }, Connector { id: "telegram".into(), relay_targets: vec![] }]).expect("the default policy is valid")
    }
}

impl Policy {
    /// `rows`, refused unless each id is a connector id (`id_ok`), named
    /// once, and each target an exact public DNS name (no wildcard, IP,
    /// local or internal name) on a nonzero port, named once.
    pub fn new(rows: Vec<Connector>) -> Result<Self, String> {
        if rows.len() > CONNECTORS_MAX {
            return Err(format!("{} connectors, more than {CONNECTORS_MAX}", rows.len()));
        }
        let mut ids = std::collections::BTreeSet::new();
        for row in &rows {
            if !id_ok(&row.id) || !ids.insert(&row.id) {
                return Err(format!("{:?} is no connector id (1 to 32 lower-case letters, digits and single dashes), or is named twice", row.id));
            }
            if row.relay_targets.len() > TARGETS_MAX {
                return Err(format!("{}: more than {TARGETS_MAX} relay targets", row.id));
            }
            let mut targets = std::collections::BTreeSet::new();
            for t in &row.relay_targets {
                let public = crate::egress::check(&format!("https://{}", t.host), false).is_ok_and(|u| matches!(u.host(), Some(url::Host::Domain(_))));
                if t.port == 0 || !crate::catalog::valid_host(&t.host) || !public || !targets.insert((&t.host, t.port)) {
                    return Err(format!("{}: {}:{} is no exact public DNS name on a nonzero port, or is named twice", row.id, t.host, t.port));
                }
            }
        }
        Ok(Policy(rows))
    }

    /// `FRAGMENT_CONNECTORS`, as a deploy writes it.
    pub fn parse(raw: &str) -> Result<Self, String> {
        Policy::new(serde_json::from_str(raw).map_err(|e| e.to_string())?)
    }

    pub fn entries(&self) -> &[Connector] {
        &self.0
    }

    /// Every connector's relay targets.
    pub fn targets(&self) -> impl Iterator<Item = &RelayTarget> {
        self.0.iter().flat_map(|c| &c.relay_targets)
    }

    /// Whether `owner`'s `agent` may do `action` through `connector`: the
    /// one decision every door and relay asks. An unlink is always allowed,
    /// so an owner can close a door the deployment no longer allows.
    pub fn authorize(&self, owner: &str, agent: &str, connector: &str, action: Action) -> Result<(), &'static str> {
        if owner.is_empty() || !fragment_proto::valid_fragment_name(agent) {
            return Err("names no owner or agent");
        }
        if !id_ok(connector) {
            return Err("names no connector");
        }
        if action == Action::Unlink || self.0.iter().any(|c| c.id == connector) {
            return Ok(());
        }
        Err("is a connector this deployment does not allow")
    }
}

/// A connector's id: 1 to 32 lower-case letters, digits and single inner
/// dashes, beginning with a letter.
pub fn id_ok(id: &str) -> bool {
    (1..=32).contains(&id.len())
        && id.as_bytes()[0].is_ascii_lowercase()
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !id.ends_with('-')
        && !id.contains("--")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goal: the default is today's SimpleX relay and Telegram; `[]` allows
    /// nothing but an unlink; an id the policy lacks is refused; bad ids are
    /// refused.
    #[test]
    fn the_default_is_simplex_and_an_empty_policy_allows_only_unlink() {
        let p = Policy::default();
        assert_eq!(p.targets().count(), crate::relay::HOSTS.len());
        assert!(p.targets().all(|t| t.port == crate::relay::PORT));
        let none = Policy::parse("[]").expect("[] is a policy");
        for a in [Action::Link, Action::Redeem, Action::Check, Action::Relay] {
            assert_eq!(p.authorize("npub1paul", "juniper--abcd", "simplex", a), Ok(()), "{a:?}");
            assert_eq!(p.authorize("npub1paul", "juniper--abcd", "telegram", a), Ok(()), "{a:?}");
            assert!(none.authorize("npub1paul", "juniper--abcd", "simplex", a).is_err(), "{a:?}");
            assert!(p.authorize("npub1paul", "juniper--abcd", "signal", a).is_err(), "{a:?}");
        }
        assert_eq!(none.authorize("npub1paul", "juniper--abcd", "simplex", Action::Unlink), Ok(()));
        assert!(p.authorize("", "juniper--abcd", "simplex", Action::Link).is_err());
        assert!(p.authorize("npub1paul", "../x", "simplex", Action::Link).is_err());
        assert!(none.authorize("npub1paul", "juniper--abcd", "../x", Action::Unlink).is_err());
        assert!(Policy::parse(r#"[{"id":"telegram"}]"#).is_ok(), "a hosted connector needs no relay");
        for id in ["", "../simplex", "SimpleX", "-a", "a-", "a--b", "a_b", "1a", &"a".repeat(33)] {
            assert!(!id_ok(id), "{id:?}");
        }
        for id in ["simplex", "telegram", "other-2", &"a".repeat(32)] {
            assert!(id_ok(id), "{id:?}");
        }
    }

    /// Goal: a policy names exact public DNS targets only, each once.
    #[test]
    fn targets_are_exact_public_names() {
        for host in ["*.example.com", "example.com.", "127.0.0.1", "8.8.8.8", "010.010.010.010", "10.0.0.1", "api.internal", "localhost", "host.local", "[::1]", "example.com:443", "Example.com"] {
            let v = serde_json::json!([{"id": "test", "relayTargets": [{"host": host, "port": 443}]}]);
            assert!(Policy::parse(&v.to_string()).is_err(), "{host}");
        }
        for bad in [
            r#"[{"id":"a","relayTargets":[{"host":"x.example.com","port":0}]}]"#,
            r#"[{"id":"a","relayTargets":[{"host":"x.example.com","port":443},{"host":"x.example.com","port":443}]}]"#,
            r#"[{"id":"a"},{"id":"a"}]"#,
            r#"[{"id":"a","extra":1}]"#,
        ] {
            assert!(Policy::parse(bad).is_err(), "{bad}");
        }
        let many: Vec<_> = (0..=CONNECTORS_MAX).map(|i| serde_json::json!({"id": format!("c{i}")})).collect();
        assert!(Policy::parse(&serde_json::Value::from(many).to_string()).is_err());
    }
}
