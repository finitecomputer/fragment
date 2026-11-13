//! The real quote from our c3-standard-8 TDX VM, verified offline with
//! the collateral Intel's PCS served for it (fetched once, saved under
//! testdata/collateral). Every test runs at a fixed time a minute after
//! that fetch, so they keep passing after the collateral expires.

use fragment_pod::attest::pzid::Instance;
use fragment_pod::attest::report::{self, attr};
use fragment_pod::verify::collateral::Collateral;
use fragment_pod::verify::failure::Failure;
use fragment_pod::verify::policy::{self, Policy, Status};
use fragment_pod::verify::tdx::{self, Verified};
use std::path::PathBuf;

fn testdata(name: &str) -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata")).join(name)
}

fn quote() -> Vec<u8> {
    std::fs::read(testdata("quote.bin")).unwrap()
}

fn nonce() -> Vec<u8> {
    std::fs::read(testdata("nonce.bin")).unwrap()
}

fn collateral() -> Collateral {
    Collateral::load(&testdata("collateral")).unwrap()
}

fn at() -> i64 {
    collateral().source.fetched_at + 60
}

fn with_nonce() -> Policy {
    Policy { nonce: Some(nonce()), ..Policy::default() }
}

fn verified() -> Verified {
    tdx::chain(&quote(), &collateral(), at()).unwrap()
}

fn first_failure(v: &Verified, p: &Policy) -> Option<Failure> {
    policy::first_failure(&policy::check(v, p)).cloned()
}

#[test]
fn the_real_quote_verifies_to_intels_root_and_carries_its_nonce() {
    let (v, outcomes) = tdx::verify(&quote(), &collateral(), at(), &with_nonce()).unwrap();
    assert_eq!(policy::first_failure(&outcomes), None);
    let nonce_check = outcomes.iter().find(|o| o.check == "nonce").unwrap();
    assert_eq!(nonce_check.status, Status::Pass);
    assert_eq!(v.tcb_status, "UpToDate");
    assert_eq!(v.fmspc, "00806F050000");
    assert_eq!(v.registers.td_attributes, "0000001000000000");
    assert!(v.registers.mrtd.starts_with("f21e6b48"));
    assert_eq!(v.registers.rtmr3, "00".repeat(48));
}

#[test]
fn one_flipped_byte_in_the_report_body_fails_the_chain() {
    for offset in [report::offset::MR_TD, report::offset::RTMR0 + 48 * 2, report::offset::REPORT_DATA] {
        let mut q = quote();
        q[report::HEADER_LEN + offset] ^= 0x01;
        let err = tdx::chain(&q, &collateral(), at()).unwrap_err();
        assert!(matches!(err, Failure::Chain { .. }), "offset {offset}: {err}");
    }
}

#[test]
fn one_flipped_byte_in_the_signature_fails_the_chain() {
    // After the 48-byte header, the 584-byte body and the 4-byte length
    // comes the 64-byte ECDSA signature over header and body.
    let sig = report::HEADER_LEN + report::TD10_BODY_LEN + 4;
    for i in [0, 31, 63] {
        let mut q = quote();
        q[sig + i] ^= 0x80;
        let err = tdx::chain(&q, &collateral(), at()).unwrap_err();
        assert!(matches!(err, Failure::Chain { .. }), "signature byte {i}: {err}");
    }
}

#[test]
fn the_wrong_nonce_fails() {
    let mut wrong = nonce();
    wrong[0] ^= 1;
    let p = Policy { nonce: Some(wrong), ..Policy::default() };
    assert!(matches!(first_failure(&verified(), &p), Some(Failure::Nonce { .. })));
    // A shorter nonce that is a prefix of REPORTDATA is still wrong: the
    // rest must be zero.
    let p = Policy { nonce: Some(nonce()[..32].to_vec()), ..Policy::default() };
    assert!(matches!(first_failure(&verified(), &p), Some(Failure::Nonce { .. })));
}

/// The honest test we can make: no Google TDX VM runs with DEBUG, and a
/// quote with the bit set cannot be signed without the platform's key.
/// So (1) setting the bit in the real quote breaks the signature, and
/// (2) the check we run after the chain refuses the real, verified
/// report once its bit is set. This does not prove an Intel-signed debug
/// quote reaches our check unaltered through dcap-qvl (we turn dcap-qvl's
/// own debug refusal off to type the error); that is reading, not test.
#[test]
fn a_debug_td_is_refused() {
    let mut q = quote();
    q[report::HEADER_LEN + report::offset::TD_ATTRIBUTES] |= attr::DEBUG as u8;
    assert!(matches!(tdx::chain(&q, &collateral(), at()), Err(Failure::Chain { .. })));

    let mut v = verified();
    v.report.td_attributes[0] |= attr::DEBUG as u8;
    assert!(matches!(first_failure(&v, &with_nonce()), Some(Failure::Debug { .. })));

    let mut v = verified();
    v.report.td_attributes[0] |= 0x10; // another "TD under debug" bit
    assert!(matches!(first_failure(&v, &with_nonce()), Some(Failure::TdUnderDebug { .. })));
}

#[test]
fn expired_collateral_fails() {
    let c = collateral();
    let after = c.next_update().unwrap() + 1;
    let err = tdx::chain(&quote(), &c, after).unwrap_err();
    assert!(matches!(err, Failure::CollateralExpired { .. }), "{err}");
    let before = c.source.fetched_at - 40 * 24 * 3600;
    let err = tdx::chain(&quote(), &c, before).unwrap_err();
    assert!(matches!(err, Failure::CollateralExpired { .. }), "{err}");
}

#[test]
fn dcap_qvl_refuses_expired_collateral_on_its_own_too() {
    use dcap_qvl::configs::RustCryptoConfig;
    use dcap_qvl::verify::QuoteVerifier;
    let c = collateral();
    let after = (c.next_update().unwrap() + 1) as u64;
    let verifier = QuoteVerifier::new_prod().with_config::<RustCryptoConfig>();
    assert!(verifier.verify(&quote(), &c.to_dcap().unwrap(), after).is_err());
    assert!(verifier.verify(&quote(), &c.to_dcap().unwrap(), at() as u64).is_ok());
}

#[test]
fn tampered_collateral_fails_the_chain() {
    let mut c = collateral();
    c.tcb_info = c.tcb_info.replacen("\"tcbLevels\"", "\"tcbLevels\" ", 1);
    assert!(matches!(tdx::chain(&quote(), &c, at()), Err(Failure::Chain { .. })));
    let mut c = collateral();
    let last = c.pck_crl.len() - 1;
    c.pck_crl[last] ^= 1;
    assert!(matches!(tdx::chain(&quote(), &c, at()), Err(Failure::Chain { .. })));
}

#[test]
fn only_named_tcb_statuses_beyond_up_to_date_pass() {
    let mut v = verified();
    v.tcb_status = "OutOfDate".into();
    assert!(matches!(first_failure(&v, &with_nonce()), Some(Failure::TcbStatus { .. })));
    let p = Policy { allowed_statuses: vec!["OutOfDate".into()], ..with_nonce() };
    assert_eq!(first_failure(&v, &p), None);
    assert!(policy::parse_status("Revoked").is_err());
}

#[test]
fn an_expected_register_must_match() {
    let v = verified();
    let mrtd = hex::decode(&v.registers.mrtd).unwrap();
    let p = Policy { expect: vec![("mrtd", mrtd.clone())], ..with_nonce() };
    assert_eq!(first_failure(&v, &p), None);
    let mut other = hex::decode(&v.registers.rtmr2).unwrap();
    other[47] ^= 1;
    let p = Policy { expect: vec![("mrtd", mrtd), ("rtmr2", other)], ..with_nonce() };
    assert!(matches!(first_failure(&v, &p), Some(Failure::Register { register, .. }) if register == "rtmr2"));
}

#[test]
fn another_instance_fails_the_mr_owner_check() {
    let i = Instance::parse("projects/123456789012/zones/us-central1-b/instances/112233445566778899").unwrap();
    let p = Policy { gce_instance: Some(i), ..with_nonce() };
    assert!(matches!(first_failure(&verified(), &p), Some(Failure::GceInstance { .. })));
}

/// The VM testdata/quote.bin came from. It is the VM, and the boot, of the
/// 2026-11-10-quote-live run, but a separate quote from that run's two.
const QUOTED_INSTANCE: &str = "projects/228527660165/zones/us-central1-b/instances/541049797945993118";

fn instance_failure(path: &str) -> Option<Failure> {
    let p = Policy { gce_instance: Some(Instance::parse(path).unwrap()), ..with_nonce() };
    first_failure(&verified(), &p)
}

#[test]
fn the_quotes_own_instance_passes_the_mr_owner_check() {
    assert_eq!(instance_failure(QUOTED_INSTANCE), None);
}

#[test]
fn the_quotes_instance_with_one_part_wrong_fails_the_mr_owner_check() {
    // Each differs from the quote's own VM in one part only: MR_OWNER
    // covers all three, so a neighbour in the same project is refused too.
    for path in [
        "projects/228527660165/zones/us-central1-b/instances/541049797945993119", // the instance
        "projects/228527660165/zones/us-central1-c/instances/541049797945993118", // the zone
        "projects/228527660166/zones/us-central1-b/instances/541049797945993118", // the project
    ] {
        assert!(matches!(instance_failure(path), Some(Failure::GceInstance { .. })), "{path} must be refused");
    }
}

#[test]
fn saved_collateral_reads_back_the_same() {
    let dir = std::env::temp_dir().join(format!("fragment-pod-collateral-{}", std::process::id()));
    collateral().save(&dir).unwrap();
    let back = Collateral::load(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(back.unwrap(), collateral());
}
