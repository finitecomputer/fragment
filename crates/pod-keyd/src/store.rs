//! What the key service holds, in memory only: per owner, the record
//! the owner's last deposit made. Nothing is written anywhere, on
//! purpose: a restarted key service holds nothing, and its pods wait
//! for their owners (or a new deposit). The disk key zeroizes when its
//! record is replaced or the process ends.

use crate::error::Refusal;
use crate::nonces::Spent;
use fragment_pod::attest::pzid::Instance;
use fragment_pod::keyd::approve::Approval;
use fragment_pod::keyd::deposit::{Settings, MAX_APPROVALS};
use fragment_pod::keyd::view::RecordView;
use fragment_pod::unlock::keys::DiskKey;
use fragment_pod::verify::policy::parse_status;
use std::collections::HashMap;

/// One record an owner, and the operator names no more owners than this
/// (args.rs), so the limit is never met; the store holds to it anyway.
pub const MAX_RECORDS: usize = 1024;

pub struct Record {
    pub owner: [u8; 32],
    /// The digest the owner signed for the deposit that made it.
    pub id: [u8; 32],
    pub instance: Instance,
    instance_text: String,
    pub allow_tcb_status: Vec<String>,
    pub disk_key: DiskKey,
    /// The owner's grant naming this boot of the key service.
    pub grant: [u8; 64],
    pub approvals: Vec<Approval>,
    pub min_seq: u64,
    pub last_release: Option<i64>,
}

impl Record {
    /// A record from a deposit already verified as the owner's. Its
    /// settings must make sense on their own.
    pub fn new(owner: [u8; 32], id: [u8; 32], s: &Settings, disk_key: DiskKey, grant: [u8; 64]) -> Result<Self, Refusal> {
        let instance = Instance::parse(&s.gce_instance).map_err(|e| Refusal::BadField(format!("gce_instance: {e}")))?;
        let allow_tcb_status = s.allow_tcb_status.iter().map(|t| parse_status(t)).collect::<Result<_, _>>().map_err(Refusal::BadField)?;
        let mut r = Self {
            owner,
            id,
            instance,
            instance_text: s.gce_instance.clone(),
            allow_tcb_status,
            disk_key,
            grant,
            approvals: Vec::new(),
            min_seq: s.min_seq,
            last_release: None,
        };
        for a in &s.approvals {
            r.approve(a.clone())?;
        }
        Ok(r)
    }

    /// Add one approval. Seqs are unique in a record, so a replayed
    /// approval adds nothing.
    pub fn approve(&mut self, a: Approval) -> Result<(), Refusal> {
        let seq = a.release.seq;
        if seq < self.min_seq {
            return Err(Refusal::BelowMinSeq { seq, min_seq: self.min_seq });
        }
        if self.approvals.iter().any(|x| x.release.seq == seq) {
            return Err(Refusal::DuplicateSeq(seq));
        }
        if self.approvals.len() >= MAX_APPROVALS {
            return Err(Refusal::TooManyApprovals);
        }
        self.approvals.push(a);
        Ok(())
    }

    pub fn view(&self) -> RecordView {
        RecordView {
            owner: self.owner,
            record: self.id,
            gce_instance: self.instance_text.clone(),
            allow_tcb_status: self.allow_tcb_status.clone(),
            min_seq: self.min_seq,
            approvals: self.approvals.clone(),
            last_release: self.last_release,
        }
    }
}

#[derive(Default)]
pub struct Store {
    pub records: HashMap<[u8; 32], Record>,
    pub spent: Spent,
}

impl Store {
    /// Put `r` in place of the owner's record, if `replaces` names the
    /// record there now (or there is none and it names none). It takes
    /// that record's min_seq if that is higher than its own: within a
    /// boot an owner's min_seq never goes down. A record keeps its id
    /// while each release it serves raises its min_seq, so a replacement
    /// signed before a release, and arriving after it, still names it,
    /// and would bring the older min_seq back.
    ///
    /// The higher one is kept here, not asked of the owner: the owner
    /// would learn it from an answer no one vouches for, and sign it.
    pub fn deposit(&mut self, mut r: Record, replaces: Option<[u8; 32]>) -> Result<&Record, Refusal> {
        match (self.records.get(&r.owner), replaces) {
            (None, None) if self.records.len() >= MAX_RECORDS => return Err(Refusal::TooManyRecords),
            (None, None) => {}
            (Some(current), Some(id)) if current.id == id => r.min_seq = r.min_seq.max(current.min_seq),
            _ => return Err(Refusal::StaleRecord),
        }
        let owner = r.owner;
        self.records.insert(owner, r);
        self.records.get(&owner).ok_or_else(|| Refusal::Internal("record vanished".into()))
    }

    pub fn by_id(&mut self, id: &[u8; 32]) -> Option<&mut Record> {
        self.records.values_mut().find(|r| &r.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fragment_pod::unlock::release::Release;

    fn approval(seq: u64) -> Approval {
        let mut registers = [None; 5];
        registers[2] = Some([seq as u8; 48]);
        Approval { release: Release { seq, name: format!("r{seq}"), registers }, not_before: None, expires: None }
    }

    fn settings(min_seq: u64, seqs: &[u64]) -> Settings {
        Settings {
            replaces: None,
            gce_instance: "projects/1/zones/us-central1-b/instances/2".into(),
            allow_tcb_status: vec![],
            min_seq,
            approvals: seqs.iter().map(|s| approval(*s)).collect(),
        }
    }

    fn record(owner: u8, id: u8, s: &Settings) -> Result<Record, Refusal> {
        Record::new([owner; 32], [id; 32], s, DiskKey::random().unwrap(), [0; 64])
    }

    #[test]
    fn a_record_refuses_approvals_below_min_seq_duplicates_and_past_the_limit() {
        assert!(matches!(record(1, 1, &settings(2, &[1])), Err(Refusal::BelowMinSeq { seq: 1, min_seq: 2 })));
        assert_eq!(record(1, 1, &settings(0, &[1, 1])).err(), Some(Refusal::DuplicateSeq(1)));
        let mut r = record(1, 1, &settings(0, &[])).unwrap();
        for seq in 0..MAX_APPROVALS as u64 {
            r.approve(approval(seq)).unwrap();
        }
        assert_eq!(r.approve(approval(99)), Err(Refusal::TooManyApprovals));
        let mut bad = settings(0, &[]);
        bad.gce_instance = "nope".into();
        assert!(matches!(record(1, 1, &bad), Err(Refusal::BadField(_))));
        bad = settings(0, &[]);
        bad.allow_tcb_status = vec!["Fine".into()];
        assert!(matches!(record(1, 1, &bad), Err(Refusal::BadField(_))));
    }

    #[test]
    fn a_deposit_replaces_only_the_record_it_names() {
        let mut s = Store::default();
        s.deposit(record(1, 10, &settings(0, &[])).unwrap(), None).unwrap();
        assert_eq!(s.deposit(record(1, 11, &settings(0, &[])).unwrap(), None).err(), Some(Refusal::StaleRecord));
        assert_eq!(s.deposit(record(1, 11, &settings(0, &[])).unwrap(), Some([9; 32])).err(), Some(Refusal::StaleRecord));
        assert_eq!(s.deposit(record(1, 11, &settings(0, &[])).unwrap(), Some([10; 32])).unwrap().id, [11; 32]);
        assert_eq!(s.deposit(record(2, 12, &settings(0, &[])).unwrap(), Some([11; 32])).err(), Some(Refusal::StaleRecord));
        assert!(s.by_id(&[10; 32]).is_none());
        assert!(s.by_id(&[11; 32]).is_some());
    }

    #[test]
    fn a_replacement_keeps_the_higher_min_seq() {
        let mut s = Store::default();
        s.deposit(record(1, 10, &settings(1, &[1, 2])).unwrap(), None).unwrap();
        // A release of seq 2 was served: the record is the same one, its floor higher.
        s.by_id(&[10; 32]).unwrap().min_seq = 2;
        let r = s.deposit(record(1, 11, &settings(1, &[1, 2])).unwrap(), Some([10; 32])).unwrap();
        assert_eq!((r.id, r.min_seq, r.approvals.len()), ([11; 32], 2, 2), "the new record, with the old one's floor");
        assert_eq!(s.deposit(record(1, 12, &settings(5, &[5])).unwrap(), Some([11; 32])).unwrap().min_seq, 5, "its own, when higher");
        assert_eq!(s.deposit(record(1, 13, &settings(0, &[])).unwrap(), Some([12; 32])).unwrap().min_seq, 5);
    }

    #[test]
    fn refuses_records_past_the_limit() {
        let mut s = Store::default();
        for i in 0..MAX_RECORDS {
            let mut owner = [0u8; 32];
            owner[..8].copy_from_slice(&(i as u64).to_be_bytes());
            let r = Record::new(owner, owner, &settings(0, &[]), DiskKey::random().unwrap(), [0; 64]).unwrap();
            s.deposit(r, None).unwrap();
        }
        assert_eq!(s.deposit(record(0xff, 1, &settings(0, &[])).unwrap(), None).err(), Some(Refusal::TooManyRecords));
    }
}
