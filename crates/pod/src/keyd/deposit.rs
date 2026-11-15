//! `POST /deposit`: the owner gives one boot of the key service a pod's
//! disk key and what a boot of that pod must be to receive it.
//!
//! The disk key is sealed (HPKE base, info "pod-keyd-deposit-v1", aad
//! owner ‖ key service boot key) to the boot key the owner just
//! attested. The owner signs SHA-256("pod-keyd-deposit-v1\0" ‖ that boot
//! key ‖ owner ‖ settings ‖ enc ‖ ct ‖ grant); the digest is the
//! record's id. The grant, SHA-256("pod-keyd-grant-v1\0" ‖ owner ‖ boot
//! key) signed by the owner, is what the key service later shows a pod
//! to prove the owner chose it.

use super::approve::{Approval, ApprovalWire};
use super::encode::Canon;
use super::{KeydError, DEPOSIT_DOMAIN, DEPOSIT_INFO, GRANT_DOMAIN, VERSION};
use crate::unlock::keys::{BootKey, DiskKey};
use crate::unlock::seal::{self, CT_LEN};
use crate::unlock::sign::{self, OwnerKey};
use crate::unlock::wire::{self, hex32, hex_into, WireError};
use serde::{Deserialize, Serialize};

/// 32 approvals of about 600 bytes each, and the rest.
pub const MAX_DEPOSIT_BODY: usize = 64 * 1024;
pub const MAX_APPROVALS: usize = 32;
pub const MAX_INSTANCE: usize = 256;
/// pod-verify names seven TCB statuses one may allow.
pub const MAX_TCB_STATUSES: usize = 8;
const MAX_TCB_NAME: usize = 64;

pub fn grant_digest(owner: &[u8; 32], keyd_pubkey: &[u8; 32]) -> [u8; 32] {
    let mut c = Canon::new(GRANT_DOMAIN);
    c.fixed(owner).fixed(keyd_pubkey);
    c.digest()
}

pub fn check_grant(owner: &[u8; 32], keyd_pubkey: &[u8; 32], grant: &[u8; 64]) -> Result<(), KeydError> {
    sign::verify(owner, &grant_digest(owner, keyd_pubkey), grant).map_err(|_| KeydError::Grant)
}

/// What a boot of the pod must be, and the release floor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// The id of the record this one replaces, None if there is none:
    /// a deposit is accepted only over the record it names, so an older
    /// deposit replayed later cannot put back what a newer one changed.
    pub replaces: Option<[u8; 32]>,
    /// `projects/<number>/zones/<zone>/instances/<id>`.
    pub gce_instance: String,
    pub allow_tcb_status: Vec<String>,
    /// No release below this seq is ever served.
    pub min_seq: u64,
    pub approvals: Vec<Approval>,
}

impl Settings {
    fn encode(&self, c: &mut Canon) {
        c.opt_fixed(self.replaces.as_ref().map(|r| r.as_slice())).str(&self.gce_instance);
        c.u64(self.allow_tcb_status.len() as u64);
        for s in &self.allow_tcb_status {
            c.str(s);
        }
        c.u64(self.min_seq).u64(self.approvals.len() as u64);
        for a in &self.approvals {
            a.encode(c);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deposit {
    pub owner: [u8; 32],
    pub settings: Settings,
    pub enc: [u8; 32],
    pub ct: [u8; CT_LEN],
    pub grant: [u8; 64],
    pub sig: [u8; 64],
}

fn aad(owner: &[u8; 32], keyd_pubkey: &[u8; 32]) -> Vec<u8> {
    [owner.as_slice(), keyd_pubkey].concat()
}

impl Deposit {
    pub fn make(owner: &OwnerKey, keyd_pubkey: &[u8; 32], key: &DiskKey, settings: Settings) -> Result<Self, KeydError> {
        let me = owner.public();
        let sealed = seal::seal_with(keyd_pubkey, None, DEPOSIT_INFO, &aad(&me, keyd_pubkey), key)?;
        let ct: [u8; CT_LEN] = sealed.ct.as_slice().try_into().map_err(|_| seal::SealError::Length(sealed.ct.len()))?;
        let grant = owner.sign(&grant_digest(&me, keyd_pubkey))?;
        let mut d = Self { owner: me, settings, enc: sealed.enc, ct, grant, sig: [0; 64] };
        d.sig = owner.sign(&d.id(keyd_pubkey))?;
        Ok(d)
    }

    /// What the owner signs, and the record's id once accepted.
    pub fn id(&self, keyd_pubkey: &[u8; 32]) -> [u8; 32] {
        let mut c = Canon::new(DEPOSIT_DOMAIN);
        c.fixed(keyd_pubkey).fixed(&self.owner);
        self.settings.encode(&mut c);
        c.fixed(&self.enc).fixed(&self.ct).fixed(&self.grant);
        c.digest()
    }

    /// The disk key, if the owner signed this for this boot of the key
    /// service, the grant names it, and the seal opens under its secret.
    pub fn open(&self, keyd: &BootKey) -> Result<DiskKey, KeydError> {
        let me = keyd.public();
        sign::verify(&self.owner, &self.id(&me), &self.sig).map_err(KeydError::Sign)?;
        check_grant(&self.owner, &me, &self.grant)?;
        Ok(seal::open_with(keyd, None, DEPOSIT_INFO, &aad(&self.owner, &me), &self.enc, &self.ct)?)
    }

    pub fn to_json(&self) -> Result<Vec<u8>, WireError> {
        let s = &self.settings;
        let w = DepositWire {
            version: VERSION,
            owner: hex::encode(self.owner),
            replaces: s.replaces.map(hex::encode),
            gce_instance: s.gce_instance.clone(),
            allow_tcb_status: s.allow_tcb_status.clone(),
            min_seq: s.min_seq,
            approvals: s.approvals.iter().map(Approval::to_wire).collect(),
            enc: hex::encode(self.enc),
            ct: hex::encode(self.ct),
            grant: hex::encode(self.grant),
            sig: hex::encode(self.sig),
        };
        serde_json::to_vec(&w).map_err(|e| WireError::Json(e.to_string()))
    }

    pub fn from_json(body: &[u8]) -> Result<Self, WireError> {
        if body.len() > MAX_DEPOSIT_BODY {
            return Err(WireError::TooLarge { len: body.len(), max: MAX_DEPOSIT_BODY });
        }
        let w: DepositWire = wire::json(body)?;
        if w.version != VERSION {
            return Err(WireError::Version(w.version));
        }
        let settings = settings(&w)?;
        let mut d = Self { owner: hex32("owner", &w.owner)?, settings, enc: [0; 32], ct: [0; CT_LEN], grant: [0; 64], sig: [0; 64] };
        hex_into("enc", &w.enc, &mut d.enc)?;
        hex_into("ct", &w.ct, &mut d.ct)?;
        hex_into("grant", &w.grant, &mut d.grant)?;
        hex_into("sig", &w.sig, &mut d.sig)?;
        Ok(d)
    }
}

fn field(field: &'static str, why: String) -> WireError {
    WireError::Field { field, why }
}

fn settings(w: &DepositWire) -> Result<Settings, WireError> {
    if w.gce_instance.is_empty() || w.gce_instance.len() > MAX_INSTANCE {
        return Err(field("gce_instance", format!("1 to {MAX_INSTANCE} bytes")));
    }
    let tcb_ok = w.allow_tcb_status.iter().all(|s| !s.is_empty() && s.len() <= MAX_TCB_NAME);
    if w.allow_tcb_status.len() > MAX_TCB_STATUSES || !tcb_ok {
        return Err(field("allow_tcb_status", format!("at most {MAX_TCB_STATUSES} names")));
    }
    if w.approvals.len() > MAX_APPROVALS {
        return Err(field("approvals", format!("{}; at most {MAX_APPROVALS}", w.approvals.len())));
    }
    Ok(Settings {
        replaces: w.replaces.as_deref().map(|r| hex32("replaces", r)).transpose()?,
        gce_instance: w.gce_instance.clone(),
        allow_tcb_status: w.allow_tcb_status.clone(),
        min_seq: w.min_seq,
        approvals: w.approvals.iter().cloned().map(Approval::from_wire).collect::<Result<_, _>>()?,
    })
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DepositWire {
    version: u32,
    owner: String,
    replaces: Option<String>,
    gce_instance: String,
    allow_tcb_status: Vec<String>,
    min_seq: u64,
    approvals: Vec<ApprovalWire>,
    enc: String,
    ct: String,
    grant: String,
    sig: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyd::approve::tests::approval;
    use crate::unlock::sign::SignError;

    fn settings() -> Settings {
        Settings {
            replaces: None,
            gce_instance: "projects/1/zones/us-central1-b/instances/2".into(),
            allow_tcb_status: vec![],
            min_seq: 1,
            approvals: vec![approval(1, 1), approval(2, 2)],
        }
    }

    #[test]
    fn opens_on_its_boot_of_the_key_service_and_round_trips() {
        let keyd = BootKey::generate().unwrap();
        let owner = OwnerKey::generate().unwrap();
        let key = DiskKey::random().unwrap();
        let d = Deposit::make(&owner, &keyd.public(), &key, settings()).unwrap();
        assert_eq!(d.open(&keyd).unwrap().as_bytes(), key.as_bytes());
        assert_eq!(check_grant(&owner.public(), &keyd.public(), &d.grant), Ok(()));
        let again = Deposit::from_json(&d.to_json().unwrap()).unwrap();
        assert_eq!(again, d);
        assert_eq!(again.id(&keyd.public()), d.id(&keyd.public()));
    }

    #[test]
    fn a_restarted_key_service_refuses_it_at_the_signature() {
        let first = BootKey::generate().unwrap();
        let d = Deposit::make(&OwnerKey::generate().unwrap(), &first.public(), &DiskKey::random().unwrap(), settings()).unwrap();
        let restarted = BootKey::generate().unwrap();
        assert_eq!(d.open(&restarted).unwrap_err(), KeydError::Sign(SignError::Signature));
    }

    #[test]
    fn every_setting_is_signed() {
        let keyd = BootKey::generate().unwrap();
        let d = Deposit::make(&OwnerKey::generate().unwrap(), &keyd.public(), &DiskKey::random().unwrap(), settings()).unwrap();
        let edits: [fn(&mut Settings); 5] = [
            |s| s.replaces = Some([1; 32]),
            |s| s.gce_instance.push('9'),
            |s| s.allow_tcb_status.push("OutOfDate".into()),
            |s| s.min_seq = 0,
            |s| s.approvals[1].expires = None,
        ];
        for (i, edit) in edits.iter().enumerate() {
            let mut forged = d.clone();
            edit(&mut forged.settings);
            assert_eq!(forged.open(&keyd).unwrap_err(), KeydError::Sign(SignError::Signature), "edit {i}");
        }
    }

    #[test]
    fn another_owner_cannot_resign_the_seal_or_reuse_the_grant() {
        let keyd = BootKey::generate().unwrap();
        let honest = Deposit::make(&OwnerKey::generate().unwrap(), &keyd.public(), &DiskKey::random().unwrap(), settings()).unwrap();
        let mallory = OwnerKey::generate().unwrap();
        let mut forged = Deposit { owner: mallory.public(), ..honest.clone() };
        forged.sig = mallory.sign(&forged.id(&keyd.public())).unwrap();
        assert_eq!(forged.open(&keyd).unwrap_err(), KeydError::Grant, "the honest owner's grant");
        forged.grant = mallory.sign(&grant_digest(&mallory.public(), &keyd.public())).unwrap();
        forged.sig = mallory.sign(&forged.id(&keyd.public())).unwrap();
        assert!(matches!(forged.open(&keyd).unwrap_err(), KeydError::Seal(_)), "the seal's aad names the honest owner");
    }

    #[test]
    fn refuses_too_many_approvals_odd_instances_and_large_bodies() {
        let keyd = BootKey::generate().unwrap();
        let owner = OwnerKey::generate().unwrap();
        let mut many = settings();
        many.approvals = (0..=MAX_APPROVALS as u64).map(|i| approval(i, 1)).collect();
        let d = Deposit::make(&owner, &keyd.public(), &DiskKey::random().unwrap(), many).unwrap();
        assert!(matches!(Deposit::from_json(&d.to_json().unwrap()), Err(WireError::Field { field: "approvals", .. })));
        let mut long = settings();
        long.gce_instance = "x".repeat(MAX_INSTANCE + 1);
        let d = Deposit::make(&owner, &keyd.public(), &DiskKey::random().unwrap(), long).unwrap();
        assert!(matches!(Deposit::from_json(&d.to_json().unwrap()), Err(WireError::Field { field: "gce_instance", .. })));
        let big = vec![b' '; MAX_DEPOSIT_BODY + 1];
        assert!(matches!(Deposit::from_json(&big), Err(WireError::TooLarge { .. })));
    }
}
