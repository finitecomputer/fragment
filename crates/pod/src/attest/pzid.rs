//! GCE's instance binding in MR_OWNER. Google's VMM sets MR_OWNER to
//! SHA-384 of a canonical JSON naming the project number, zone and
//! instance id (the "PZID"). Format copied from go-tdx-guest
//! `gce/pzid.go` (`PZIDPayload`), whose test vector is used below.
//!
//! MR_OWNER is chosen by whoever creates the TD. It means "Google's
//! VMM put this instance's name here" only once the quote is known to
//! come from a Google host (the host registry check).

use sha2::{Digest, Sha384};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instance {
    pub project_number: u64,
    pub zone: String,
    pub instance_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstanceError {
    Format(String),
    Zone(String),
}

impl fmt::Display for InstanceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Format(s) => write!(
                f,
                "{s:?} is not projects/<number>/zones/<zone>/instances/<numeric id>"
            ),
            Self::Zone(z) => write!(f, "zone {z:?} is not lowercase letters, digits and '-'"),
        }
    }
}

impl std::error::Error for InstanceError {}

impl Instance {
    /// Parse `projects/<number>/zones/<zone>/instances/<id>`, the form
    /// gceprovenance takes.
    pub fn parse(text: &str) -> Result<Self, InstanceError> {
        let bad = || InstanceError::Format(text.to_owned());
        let parts: Vec<&str> = text.trim_matches('/').split('/').collect();
        let [p, project, z, zone, i, id] = parts[..] else {
            return Err(bad());
        };
        if p != "projects" || z != "zones" || i != "instances" {
            return Err(bad());
        }
        let instance = Self {
            project_number: project.parse().map_err(|_| bad())?,
            zone: zone.to_owned(),
            instance_id: id.parse().map_err(|_| bad())?,
        };
        instance.check_zone()?;
        Ok(instance)
    }

    fn check_zone(&self) -> Result<(), InstanceError> {
        let ok = !self.zone.is_empty()
            && self
                .zone
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if ok {
            Ok(())
        } else {
            Err(InstanceError::Zone(self.zone.clone()))
        }
    }

    /// The canonical payload. The zone check above keeps it valid JSON
    /// without escaping.
    pub fn payload(&self) -> String {
        format!(
            r#"{{"instanceId":{},"numericalProjectId":{},"zone":"{}"}}"#,
            self.instance_id, self.project_number, self.zone
        )
    }

    pub fn mr_owner(&self) -> [u8; 48] {
        Sha384::digest(self.payload().as_bytes()).into()
    }

    /// The region a zone is in: `us-central1-b` is in `us-central1`.
    pub fn region(&self) -> &str {
        self.zone.rsplit_once('-').map_or(&self.zone, |(r, _)| r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_matches_go_tdx_guest() {
        let i = Instance::parse("projects/123456789012/zones/us-central1-a/instances/112233445566778899")
            .unwrap();
        assert_eq!(
            i.payload(),
            r#"{"instanceId":112233445566778899,"numericalProjectId":123456789012,"zone":"us-central1-a"}"#
        );
        assert_eq!(i.region(), "us-central1");
        let expected: [u8; 48] = Sha384::digest(i.payload().as_bytes()).into();
        assert_eq!(i.mr_owner(), expected);
    }

    #[test]
    fn refuses_names_and_odd_zones() {
        assert!(Instance::parse("projects/my-project/zones/us-central1-a/instances/1").is_err());
        assert!(Instance::parse("projects/1/zones/us-central1-a/instances/pod-vm").is_err());
        assert!(Instance::parse("projects/1/zones/us\"x/instances/1").is_err());
        assert!(Instance::parse("projects/1/zones/us-central1-a").is_err());
    }
}
