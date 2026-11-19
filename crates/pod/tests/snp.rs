//! A real SEV-SNP report from a stock Ubuntu 24.04 n2d-standard-4 in
//! us-central1-b (2026-11-11), fetched through configfs-tsm with a 64-byte
//! nonce; the VCEK, ASK and ARK the host returned with it (auxblob); the VCEK
//! of the second SNP VM's chip; and Google's SNP launch endorsement for the
//! report's measurement (gs://gce_tcb_integrity/ovmf_x64_csm/sevsnp/).

use fragment_pod::verify::failure::Failure;
use fragment_pod::verify::{firmware, snp};

fn read(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/testdata/snp/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

const MEASUREMENT: &str =
    "2279d213af547f03377a189d6e0c949eef1c2d1796eb276f2f75d3cbd97979a97d3884e01d9596b1c9303528c0de9f3b";

/// 2026-11-11, inside every certificate's validity.
const AT: i64 = 1_794_400_000;

fn certs() -> snp::Certs {
    snp::Certs { vek: read("vcek.der"), ask: read("ask.der"), ark: read("ark.der") }
}

#[test]
fn the_real_report_verifies_to_amds_milan_root_with_its_vcek() {
    let s = snp::verify(&read("report.bin"), &certs(), Some(&read("nonce.bin")), AT).unwrap();
    assert_eq!(s.measurement, MEASUREMENT);
    assert_eq!(s.signing_key, "VCEK");
    assert_eq!(s.product, "Milan-B0");
    assert!(!s.policy.debug_allowed);
    assert!(!s.policy.migrate_ma_allowed);
    assert!(s.policy.smt_allowed);
    assert_eq!(s.vmpl, 0);
    assert_eq!(s.host_data, "00".repeat(32));
    assert_eq!(s.id_key_digest, "00".repeat(48));
    // The VCEK names this chip and this TCB.
    assert_eq!(s.chip_id, s.vek_hw_id.clone().unwrap());
    assert_eq!(s.reported_tcb, s.vek_tcb.clone().unwrap());
}

#[test]
fn another_nonce_fails() {
    let mut other = read("nonce.bin");
    other[0] ^= 1;
    let e = snp::verify(&read("report.bin"), &certs(), Some(&other), AT).unwrap_err();
    assert!(matches!(e, Failure::Nonce { .. }), "{e}");
}

#[test]
fn a_changed_byte_in_the_signed_report_fails() {
    let mut bad = read("report.bin");
    bad[0x90] ^= 1; // the measurement
    let e = snp::verify(&bad, &certs(), Some(&read("nonce.bin")), AT).unwrap_err();
    assert!(matches!(e, Failure::Chain { .. }), "{e}");
}

#[test]
fn another_chips_vcek_fails() {
    let c = snp::Certs { vek: read("vcek-other-chip.der"), ..certs() };
    let e = snp::verify(&read("report.bin"), &c, Some(&read("nonce.bin")), AT).unwrap_err();
    assert!(matches!(e, Failure::Chain { .. }), "{e}");
}

#[test]
fn an_ark_other_than_amds_pinned_milan_root_fails() {
    // The ASK in the ARK's place: not self-signed, and not the pinned root.
    let c = snp::Certs { ark: read("ask.der"), ..certs() };
    let e = snp::verify(&read("report.bin"), &c, Some(&read("nonce.bin")), AT).unwrap_err();
    assert!(matches!(e, Failure::Chain { .. }), "{e}");
}

#[test]
fn a_debug_policy_is_refused() {
    let mut r = snp::parse(&read("report.bin")).unwrap();
    r.policy.set_debug_allowed(true);
    let e = snp::check_policy(&r).unwrap_err();
    assert!(matches!(e, Failure::Debug { .. }), "{e}");
}

#[test]
fn google_endorses_the_measurement_for_four_vcpus() {
    let e = firmware::verify_snp(&read("endorsement.binarypb"), AT).unwrap();
    assert!(e.signer.contains("CN=GCE-uefi-signer"));
    let hit = e.covers(MEASUREMENT).expect("endorsed");
    assert_eq!(hit.vcpus, 4);
    // One measurement per vCPU count: they all differ.
    let mut all: Vec<_> = e.measurements.iter().map(|m| m.measurement.clone()).collect();
    all.sort();
    all.dedup();
    assert_eq!(all.len(), e.measurements.len());
    assert!(e.measurements.len() > 1);
}

#[test]
fn a_changed_byte_in_the_snp_endorsement_fails() {
    let mut bad = read("endorsement.binarypb");
    bad[200] ^= 1;
    assert!(matches!(firmware::verify_snp(&bad, AT), Err(Failure::Chain { .. })));
}
