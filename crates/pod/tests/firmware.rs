//! Google's launch endorsement for our VM's firmware, fetched once from
//! gs://gce_tcb_integrity/ovmf_x64_csm/tdx/<MRTD>.binarypb.

use fragment_pod::attest::report;
use fragment_pod::verify::failure::Failure;
use fragment_pod::verify::firmware;

fn read(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/testdata/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// 2026-11-10, inside the signer certificate's validity (2026-07-07 to 2031-07-07).
const AT: i64 = 1_794_351_347;

#[test]
fn google_endorses_the_real_quotes_mrtd() {
    let e = firmware::verify(&read("google/mrtd-endorsement.binarypb"), AT).unwrap();
    let q = report::parse(&read("quote.bin")).unwrap();
    assert!(e.signer.contains("CN=GCE-uefi-signer"));
    let m = e.covers(&q.mr_td).expect("endorsed");
    assert_eq!(m.ram_gib, 0);
}

#[test]
fn a_changed_byte_in_the_endorsement_fails() {
    let good = read("google/mrtd-endorsement.binarypb");
    // A byte inside the signed golden measurement, and one in the signature.
    for at in [200, good.len() - 10] {
        let mut bad = good.clone();
        bad[at] ^= 1;
        assert!(matches!(firmware::verify(&bad, AT), Err(Failure::Chain { .. })), "byte {at}");
    }
}

#[test]
fn the_signer_certificate_must_be_valid_at_the_time() {
    let e = firmware::verify(&read("google/mrtd-endorsement.binarypb"), 1_700_000_000);
    assert!(matches!(e, Err(Failure::Chain { detail }) if detail.contains("valid")));
}
