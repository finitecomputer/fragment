//! Deployment connector authorization, independent of its transport or UI.
use fragment_proto::computer::RelayTarget;
use serde::{Deserialize, Serialize};

pub const DEFAULT_POLICY: &str = include_str!("../../../deploy/computer-connectors.json");
pub const CONNECTORS_MAX: usize = 16;
pub const TARGETS_MAX: usize = 32;
pub const POLICY_MAX_BYTES: usize = 5000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectorPolicy {
    pub id: String,
    #[serde(default)]
    pub relay_targets: Vec<RelayTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputerConnectorPolicy(Vec<ConnectorPolicy>);

/// Owner is the authority from which live org policy will be resolved. No
/// cached org membership or image declaration can grant connector permission.
pub struct Context<'a> {
    pub owner: &'a str,
    pub agent: &'a str,
}

/// No org policy revision exists yet. Later policy decisions carry the
/// live revision used to fence pending consent and queued outbound work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authorization {
    pub allowed: bool,
    pub reason: &'static str,
    pub revision: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Link,
    Redeem,
    Check,
    Unlink,
    Relay,
    Export,
    Credential,
}

impl Default for ComputerConnectorPolicy {
    fn default() -> Self {
        Self::parse(DEFAULT_POLICY).expect("checked-in connector policy")
    }
}

impl ComputerConnectorPolicy {
    pub fn parse(raw: &str) -> Result<Self, String> {
        if raw.len() > POLICY_MAX_BYTES {
            return Err("connector policy exceeds 5000 bytes".into());
        }
        let rows: Vec<ConnectorPolicy> = serde_json::from_str(raw).map_err(|e| e.to_string())?;
        if rows.len() > CONNECTORS_MAX {
            return Err("too many connectors".into());
        }
        let mut ids = std::collections::BTreeSet::new();
        for row in &rows {
            if !id_ok(&row.id) || !ids.insert(&row.id) {
                return Err("invalid or duplicate connector id".into());
            }
            if row.relay_targets.len() > TARGETS_MAX {
                return Err("too many relay targets".into());
            }
            let mut targets = std::collections::BTreeSet::new();
            for t in &row.relay_targets {
                if t.port == 0
                    || !crate::catalog::valid_host(&t.host)
                    || t.host.parse::<std::net::IpAddr>().is_ok()
                    || crate::egress::check(&format!("https://{}", t.host), false).map_or(true, |u| !matches!(u.host(), Some(url::Host::Domain(_))))
                    || !targets.insert((&t.host, t.port))
                {
                    return Err(
                        "relay targets must be unique exact public DNS hosts and nonzero ports"
                            .into(),
                    );
                }
            }
        }
        Ok(Self(rows))
    }

    pub fn entries(&self) -> &[ConnectorPolicy] {
        &self.0
    }
    pub fn targets(&self) -> impl Iterator<Item = &RelayTarget> {
        self.0.iter().flat_map(|c| &c.relay_targets)
    }

    /// The single authorization seam for links, inbound records and outbound
    /// export/credential/relay use. Future org policy intersects here, never
    /// in a connector's image. Unlink stays possible after deconfiguration.
    pub fn authorize_connector(&self, context: &Context<'_>, connector: &str, action: Action, _bound_scope: Option<&str>) -> Authorization {
        let (allowed, reason) = if context.owner.is_empty() || !fragment_proto::valid_fragment_name(context.agent) {
            (false, "invalid owner or agent")
        } else if !id_ok(connector) {
            (false, "invalid connector")
        } else if action == Action::Unlink {
            (true, "unlink remains allowed")
        } else if self.0.iter().any(|c| c.id == connector) {
            (true, "deployment allows connector")
        } else {
            (false, "deployment does not allow connector")
        };
        Authorization { allowed, reason, revision: None }
    }
}

pub fn id_ok(id: &str) -> bool {
    (1..=32).contains(&id.len())
        && id.as_bytes()[0].is_ascii_lowercase()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !id.ends_with('-')
        && !id.contains("--")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_and_explicit_disable() {
        let p = ComputerConnectorPolicy::default();
        assert_eq!(p.targets().count(), 12);
        let c = Context {
            owner: "owner",
            agent: "juniper--abcd",
        };
        for a in [
            Action::Link,
            Action::Redeem,
            Action::Check,
            Action::Relay,
            Action::Export,
            Action::Credential,
        ] {
            assert!(p.authorize_connector(&c, "simplex", a, None).allowed);
            assert!(!ComputerConnectorPolicy::parse("[]")
                .unwrap()
                .authorize_connector(&c, "simplex", a, None).allowed);
        }
        assert!(ComputerConnectorPolicy::parse("[]").unwrap().authorize_connector(&c, "simplex", Action::Unlink, Some("old-org")).allowed);
        let denied = p.authorize_connector(&c, "telegram", Action::Export, None);
        assert!(!denied.allowed && denied.revision.is_none() && !denied.reason.is_empty());

        for host in [
            "*.example.com",
            "example.com.",
            "127.0.0.1", "8.8.8.8", "010.010.010.010",
            "10.0.0.1",
            "api.internal",
            "localhost",
            "host.local",
            "[::1]",
            "example.com:443",
        ] {
            let v = serde_json::json!([{"id":"test", "relayTargets":[{"host":host,"port":443}]}]);
            assert!(
                ComputerConnectorPolicy::parse(&v.to_string()).is_err(),
                "{host}"
            );
        }
        assert!(
            ComputerConnectorPolicy::parse(r#"[{"id":"telegram"}]"#).is_ok(),
            "no computer transport required"
        );
    }
}
