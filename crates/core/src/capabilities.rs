//! Bounded, version-scoped image discovery. Never connector authorization.
use fragment_proto::computer::{ComputerCapabilities, valid_port_path};
use serde::{Deserialize, Serialize};

pub const REPORT_MAX_BYTES: usize = 16 * 1024;
pub const CHATS_MAX: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub version: String,
    pub capabilities: ComputerCapabilities,
}

impl Report {
    pub fn visible(&self, version_now: Option<&str>) -> Option<ComputerCapabilities> {
        (version_now == Some(self.version.as_str())).then(|| self.capabilities.clone())
    }
}

pub fn validate(c: &ComputerCapabilities) -> Result<(), &'static str> {
    if c.connectors.len() > crate::connectors::CONNECTORS_MAX || c.chats.len() > CHATS_MAX || serde_json::to_vec(c).map_err(|_| "capabilities must serialize")?.len() > REPORT_MAX_BYTES {
        return Err("capabilities exceed declaration bounds");
    }
    let mut ids = std::collections::BTreeSet::new();
    for p in &c.connectors {
        if !crate::door::id_ok(&p.id) || !ids.insert(&p.id) || p.title.trim().is_empty() || p.title.len() > 80 || p.title.chars().any(char::is_control) || p.port == 0 || !valid_port_path(&p.path) || ["%2e", "%2f", "%5c", "%25"].iter().any(|encoded| p.path.to_ascii_lowercase().contains(encoded)) {
            return Err("connector page needs a unique slug, title (1 to 80 bytes), port and local port path");
        }
    }
    let mut chats = std::collections::BTreeSet::new();
    for r in &c.chats {
        if !crate::door::id_ok(&r.connector) || !fragment_proto::valid_fragment_name(&r.agent) || !fragment_proto::valid_fragment_name(&r.chat) || !chats.insert((&r.connector, &r.agent, &r.chat)) {
            return Err("connector chat needs a valid connector, agent and chat, with no duplicates");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fragment_proto::computer::{ConnectorChat, ConnectorPage};
    fn declared() -> ComputerCapabilities {
        ComputerCapabilities { home_chat: true, connectors: vec![ConnectorPage { id: "other".into(), title: "Another connector".into(), port: 6080, path: "/connect/".into(), requires_always_on: true }], chats: vec![ConnectorChat { connector: "other".into(), agent: "juniper--abcd".into(), chat: "juniper-chat--abcd".into() }] }
    }
    #[test]
    fn declarations_are_bounded_and_transport_local() {
        assert!(validate(&declared()).is_ok());
        assert!(validate(&ComputerCapabilities::default()).is_ok(), "explicit empty declaration");
        for path in ["https://evil.test/", "//evil.test/", "/../private", "/%2e%2e/private", "/a#x", "/a\\b"] {
            let mut c = declared(); c.connectors[0].path = path.into(); assert!(validate(&c).is_err(), "{path}");
        }
        let mut c = declared(); c.connectors.push(c.connectors[0].clone()); assert!(validate(&c).is_err());
        let mut c = declared(); c.connectors[0].port = 0; assert!(validate(&c).is_err());
        let mut c = declared(); c.connectors[0].title = "a".repeat(81); assert!(validate(&c).is_err());
        let mut c = declared(); c.chats[0].agent = "../agent".into(); assert!(validate(&c).is_err());
        let mut c = declared(); c.chats = vec![c.chats[0].clone(); CHATS_MAX+1]; assert!(validate(&c).is_err());
    }
    #[test]
    fn reports_survive_restart_and_are_scoped_to_the_running_image() {
        let report = Report { version: "v1".into(), capabilities: declared() };
        let restored: Report = serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();
        assert_eq!(restored.visible(Some("v1")), Some(declared()));
        assert_eq!(restored.visible(Some("v2")), None, "upgrade hides stale discovery");
        assert_eq!(restored.visible(None), None, "no image version grants nothing");
        assert_eq!(restored.visible(Some("v1")), Some(declared()), "rollback to the same image keeps its declaration");
        let empty = Report { version: "v1".into(), capabilities: ComputerCapabilities::default() };
        assert_eq!(empty.visible(Some("v1")), Some(ComputerCapabilities::default()), "explicit empty wins over legacy discovery");
    }
}
