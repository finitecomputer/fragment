//! policy.json: the releases the owner trusts (registers by release,
//! each with a sequence number), extra TCB statuses to accept, and the
//! GCE instance the pod must be. A key service reads the same shape.

use anyhow::{Context, Result};
use crate::attest::pzid::Instance;
use crate::unlock::release::{Release, ReleaseWire};
use crate::verify::policy::parse_status;
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyFile {
    releases: Vec<ReleaseWire>,
    #[serde(default)]
    allow_tcb_status: Vec<String>,
    gce_instance: String,
}

#[derive(Debug, Clone)]
pub struct Policy {
    pub releases: Vec<Release>,
    pub allow_tcb_status: Vec<String>,
    pub gce_instance: Instance,
    /// As written, for statements that name the instance.
    pub gce_instance_text: String,
}

impl Policy {
    pub fn parse(json: &[u8]) -> Result<Self> {
        let f: PolicyFile = serde_json::from_slice(json)?;
        let mut releases = Vec::new();
        for r in f.releases {
            let name = r.name.clone();
            releases.push(Release::from_wire(r).with_context(|| format!("release {name:?}"))?);
        }
        let allow_tcb_status = f.allow_tcb_status.iter().map(|s| parse_status(s)).collect::<Result<_, _>>().map_err(anyhow::Error::msg)?;
        let gce_instance = Instance::parse(&f.gce_instance)?;
        Ok(Self { releases, allow_tcb_status, gce_instance, gce_instance_text: f.gce_instance })
    }

    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&bytes).with_context(|| format!("{}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_policy_file() {
        let json = format!(
            r#"{{"releases":[{{"seq":1,"name":"a","rtmr1":"{}"}}],"allow_tcb_status":["OutOfDate"],
            "gce_instance":"projects/1/zones/us-central1-b/instances/2"}}"#,
            "02".repeat(48)
        );
        let p = Policy::parse(json.as_bytes()).unwrap();
        assert_eq!(p.releases[0].registers[2], Some([2; 48]));
        assert_eq!(p.allow_tcb_status, ["OutOfDate"]);
        assert_eq!(p.gce_instance_text, "projects/1/zones/us-central1-b/instances/2");
        assert!(Policy::parse(json.replace("OutOfDate", "Fine").as_bytes()).is_err());
        assert!(Policy::parse(json.replace(&"02".repeat(48), "02").as_bytes()).is_err());
        assert!(Policy::parse(br#"{"releases":[],"gce_instance":"nope"}"#).is_err());
        let empty = br#"{"releases":[{"seq":1,"name":"any"}],"gce_instance":"projects/1/zones/z-1/instances/2"}"#;
        assert!(format!("{:#}", Policy::parse(empty).unwrap_err()).contains("names no register"));
    }
}
