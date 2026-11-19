//! `GET /pods/{owner}`: what the key service holds for an owner, minus
//! every secret. The owner reads the record's id here before an
//! approval or a replacing deposit. Plain HTTP and unattested: a liar
//! here can only make the next signed statement name the wrong record,
//! which the key service then refuses.

use super::approve::{Approval, ApprovalWire};
use super::deposit::MAX_DEPOSIT_BODY;
use super::VERSION;
use crate::unlock::wire::{self, hex32, WireError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordView {
    pub owner: [u8; 32],
    pub record: [u8; 32],
    pub gce_instance: String,
    pub allow_tcb_status: Vec<String>,
    pub min_seq: u64,
    pub approvals: Vec<Approval>,
    /// When a boot of the pod last received the key (the key service's
    /// clock, unix seconds).
    pub last_release: Option<i64>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ViewWire {
    version: u32,
    owner: String,
    record: String,
    gce_instance: String,
    allow_tcb_status: Vec<String>,
    min_seq: u64,
    approvals: Vec<ApprovalWire>,
    last_release: Option<i64>,
}

impl RecordView {
    pub fn to_json(&self) -> Result<Vec<u8>, WireError> {
        let w = ViewWire {
            version: VERSION,
            owner: hex::encode(self.owner),
            record: hex::encode(self.record),
            gce_instance: self.gce_instance.clone(),
            allow_tcb_status: self.allow_tcb_status.clone(),
            min_seq: self.min_seq,
            approvals: self.approvals.iter().map(Approval::to_wire).collect(),
            last_release: self.last_release,
        };
        serde_json::to_vec(&w).map_err(|e| WireError::Json(e.to_string()))
    }

    pub fn from_json(body: &[u8]) -> Result<Self, WireError> {
        if body.len() > MAX_DEPOSIT_BODY {
            return Err(WireError::TooLarge { len: body.len(), max: MAX_DEPOSIT_BODY });
        }
        let w: ViewWire = wire::json(body)?;
        if w.version != VERSION {
            return Err(WireError::Version(w.version));
        }
        Ok(Self {
            owner: hex32("owner", &w.owner)?,
            record: hex32("record", &w.record)?,
            gce_instance: w.gce_instance,
            allow_tcb_status: w.allow_tcb_status,
            min_seq: w.min_seq,
            approvals: w.approvals.into_iter().map(Approval::from_wire).collect::<Result<_, _>>()?,
            last_release: w.last_release,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyd::approve::tests::approval;

    #[test]
    fn round_trips_and_has_no_key_fields() {
        let v = RecordView {
            owner: [1; 32],
            record: [2; 32],
            gce_instance: "projects/1/zones/z-1/instances/2".into(),
            allow_tcb_status: vec![],
            min_seq: 3,
            approvals: vec![approval(3, 1)],
            last_release: Some(100),
        };
        let json = v.to_json().unwrap();
        assert_eq!(RecordView::from_json(&json).unwrap(), v);
        let text = String::from_utf8(json).unwrap();
        for secret in ["enc", "ct", "key", "grant", "sig"] {
            assert!(!text.contains(&format!("\"{secret}\"")), "{secret} in {text}");
        }
    }
}
