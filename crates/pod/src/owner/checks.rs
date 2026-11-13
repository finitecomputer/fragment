//! The five checks before a key is released, as pure functions of what
//! was fetched: `quote`, `release`, `platform`, `google_instance`,
//! `owner`. If the quote (chain and binding) fails, nothing in /status
//! is trusted and the other four are skipped. All but `owner` are
//! pod-checks', which the key service runs too.

use super::policy::Policy;
use crate::checks;
use crate::checks::release::{self, Matched};
use crate::unlock::binding::{State, NONCE_LEN};
use crate::unlock::wire;
use crate::verify::failure::Failure;
use crate::verify::policy::Status;
use crate::verify::provenance::HostRegistry;
use crate::verify::tdx::Verified;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
}

impl Check {
    pub fn new(name: &'static str, r: Result<String, String>) -> Self {
        match r {
            Ok(detail) => Self { name, status: Status::Pass, detail },
            Err(detail) => Self { name, status: Status::Fail, detail },
        }
    }
    pub fn skipped(name: &'static str, why: &str) -> Self {
        Self { name, status: Status::Skipped, detail: why.into() }
    }
}

/// What the owner check accepts: init alone accepts a blank pod.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Status,
    Init,
    Unlock,
}

/// What one `/status` produced, verified or not.
pub struct Evidence<'a> {
    pub nonce: [u8; NONCE_LEN],
    pub status: &'a wire::Status,
    pub chain: Result<&'a Verified, &'a Failure>,
    pub registry: Result<&'a HostRegistry, &'a Failure>,
}

pub struct Expect<'a> {
    pub policy: &'a Policy,
    pub last_unlocked_seq: Option<u64>,
    pub me: Option<[u8; 32]>,
    pub mode: Mode,
}

pub struct Outcome {
    pub checks: Vec<Check>,
    pub release: Option<Matched>,
}

impl Outcome {
    pub fn all_pass(&self) -> bool {
        self.checks.iter().all(|c| c.status == Status::Pass)
    }
    pub fn any_fail(&self) -> bool {
        self.checks.iter().any(|c| c.status == Status::Fail)
    }
    pub fn first_not_passed(&self) -> Option<&Check> {
        self.checks.iter().find(|c| c.status != Status::Pass)
    }
}

/// REPORTDATA must be the binding of exactly these fields and our nonce.
pub fn check_binding(report_data: &[u8; 64], status: &wire::Status, nonce: [u8; NONCE_LEN]) -> Result<(), String> {
    checks::check_report_data(report_data, &status.binding(nonce).report_data())
}

pub fn check_owner(state: State, attested: Option<[u8; 32]>, me: Option<[u8; 32]>, mode: Mode) -> Check {
    let Some(me) = me else {
        return Check::skipped("owner", "no owner.key in --dir");
    };
    let r = match (state, attested, mode) {
        (State::Blank, _, Mode::Init) => Ok("blank: no owner yet".to_owned()),
        (State::Blank, _, Mode::Status) => return Check::skipped("owner", "blank: no owner yet"),
        (State::Blank, _, Mode::Unlock) => Err("the pod is blank; nothing to unlock".to_owned()),
        (_, Some(o), _) if o == me => Ok(format!("attested owner is this key ({})", hex::encode(me))),
        (_, Some(o), _) => Err(format!("attested owner {} is not this key {}", hex::encode(o), hex::encode(me))),
        (_, None, _) => Err(format!("the pod names no owner; this key is {}", hex::encode(me))),
    };
    Check::new("owner", r)
}

pub fn run(e: &Evidence<'_>, x: &Expect<'_>) -> Outcome {
    let v = match checks::quote(e.chain, &e.status.binding(e.nonce).report_data()) {
        Ok(v) => v,
        Err(why) => {
            let mut checks = vec![Check::new("quote", Err(why))];
            for name in ["release", "platform", "google_instance", "owner"] {
                checks.push(Check::skipped(name, "the quote did not verify, so nothing it carries is trusted"));
            }
            return Outcome { checks, release: None };
        }
    };
    let quote = format!("Intel root, collateral fresh; REPORTDATA binds nonce, boot key, state {}, owner, TLS root", e.status.state.as_str());
    let release = release::match_release(&x.policy.releases, &v.report, x.last_unlocked_seq);
    let checks = vec![
        Check::new("quote", Ok(quote)),
        Check::new("release", release.as_ref().map(Matched::detail).map_err(Clone::clone)),
        Check::new("platform", checks::platform(v, &x.policy.allow_tcb_status)),
        Check::new("google_instance", checks::google_instance(v, &x.policy.gce_instance, e.registry)),
        check_owner(e.status.state, e.status.owner, x.me, x.mode),
    ];
    Outcome { checks, release: release.ok() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unlock::binding::Binding;

    fn status() -> wire::Status {
        wire::Status { state: State::Locked, boot_pubkey: [2; 32], owner: Some([3; 32]), tls_root: Some([4; 32]), quote: vec![] }
    }

    fn report_data(nonce: [u8; 32]) -> [u8; 64] {
        Binding { nonce, boot_pubkey: [2; 32], state: State::Locked, owner: Some([3; 32]), tls_root: Some([4; 32]) }.report_data()
    }

    #[test]
    fn the_binding_must_match_every_field_and_the_nonce() {
        let rd = report_data([1; 32]);
        assert_eq!(check_binding(&rd, &status(), [1; 32]), Ok(()));
        assert!(check_binding(&rd, &status(), [9; 32]).is_err(), "another nonce: a replay");
        let edits: [fn(&mut wire::Status); 4] = [
            |s| s.state = State::Open,
            |s| s.boot_pubkey = [8; 32],
            |s| s.owner = None,
            |s| s.tls_root = None,
        ];
        for (i, edit) in edits.iter().enumerate() {
            let mut s = status();
            edit(&mut s);
            assert!(check_binding(&rd, &s, [1; 32]).is_err(), "edit {i}");
        }
        // A quote over a plain nonce (as testdata/quote.bin is) carries no binding.
        let mut plain = [0u8; 64];
        plain[..32].copy_from_slice(&[1; 32]);
        assert!(check_binding(&plain, &status(), [1; 32]).is_err());
    }

    #[test]
    fn the_owner_must_be_me_unless_init_finds_a_blank_pod() {
        let me = Some([3; 32]);
        let pass = |c: Check| c.status == Status::Pass;
        assert!(pass(check_owner(State::Locked, Some([3; 32]), me, Mode::Unlock)));
        assert!(pass(check_owner(State::Open, Some([3; 32]), me, Mode::Status)));
        assert!(pass(check_owner(State::Blank, None, me, Mode::Init)));
        assert_eq!(check_owner(State::Locked, Some([5; 32]), me, Mode::Unlock).status, Status::Fail);
        assert_eq!(check_owner(State::Locked, None, me, Mode::Unlock).status, Status::Fail);
        assert_eq!(check_owner(State::Blank, None, me, Mode::Unlock).status, Status::Fail);
        assert_eq!(check_owner(State::Locked, Some([3; 32]), me, Mode::Init).status, Status::Pass);
        assert_eq!(check_owner(State::Blank, None, me, Mode::Status).status, Status::Skipped);
        assert_eq!(check_owner(State::Locked, Some([3; 32]), None, Mode::Status).status, Status::Skipped);
    }
}
