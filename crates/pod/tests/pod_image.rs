//! Against our own measured image A (2026-11-10): the event log and quote
//! from its first boot as a c3-standard-4 TDX VM, and the image's first 34
//! sectors. The UKI itself (56 MB) is not in testdata: the tests that need
//! it read the file `POD_IMAGE_A_UKI` (or `POD_IMAGE_D_UKI`) names, checked
//! against testdata's sha256sums, and are ignored unless asked for
//! (`cargo test -p fragment-pod --test pod_image -- --ignored`). The image
//! is built by the pod's image tooling, which is not on master yet.

use fragment_pod::attest::expect::{self, Origin};
use fragment_pod::attest::{eventlog, gpt, replay, report};
use sha2::{Digest, Sha256, Sha384};

fn testdata(name: &str) -> Vec<u8> {
    let path = format!("{}/testdata/pod-image-a/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The UKI the environment variable `var` names.
fn uki_from(var: &str) -> (String, Vec<u8>) {
    let path = std::env::var(var).unwrap_or_else(|_| panic!("set {var} to the image's pod.efi"));
    let uki = std::fs::read(&path).unwrap_or_else(|e| panic!("{var}={path}: {e}"));
    (path, uki)
}

fn uki() -> Vec<u8> {
    let (path, uki) = uki_from("POD_IMAGE_A_UKI");
    // the file this test was written against, not whatever was built since
    let sums = String::from_utf8(testdata("sha256sums")).unwrap();
    let want = sums.lines().find(|l| l.ends_with("pod.efi")).unwrap().split_whitespace().next().unwrap().to_owned();
    assert_eq!(hex::encode(Sha256::digest(&uki)), want, "{path} is not image A's UKI");
    uki
}

fn rtmr_events(log: &eventlog::EventLog, rtmr: usize) -> Vec<&eventlog::Event> {
    log.events.iter().filter(|e| e.rtmr() == Some(rtmr)).collect()
}

#[test]
fn the_log_replays_to_the_quote_and_rtmr2_and_rtmr3_are_empty() {
    let q = report::parse(&testdata("quote.bin")).unwrap();
    let log = eventlog::parse(&testdata("ccel.bin")).unwrap();
    assert_eq!(replay::replay(&log).unwrap(), q.rtmr);
    // systemd-stub 255 measures only into a TPM (here Google's vTPM, not
    // the TD's registers) when one is present
    assert!(rtmr_events(&log, 2).is_empty());
    assert_eq!(q.rtmr[2], [0; 48]);
    assert_eq!(q.rtmr[3], [0; 48]);
}

#[test]
fn the_gpt_event_is_computed_from_the_image_alone() {
    let log = eventlog::parse(&testdata("ccel.bin")).unwrap();
    let logged = rtmr_events(&log, 1)[2];
    assert_eq!(fragment_pod::attest::describe::type_name(logged.event_type), "EV_EFI_GPT_EVENT");
    let data = gpt::event_data(&testdata("gpt.bin"), 512).unwrap();
    // the firmware logged exactly the bytes we build, and hashed them
    assert_eq!(logged.data, data);
    assert_eq!(logged.sha384.unwrap(), <[u8; 48]>::from(Sha384::digest(&data)));
}

#[test]
fn a_changed_partition_table_changes_the_gpt_event() {
    let mut disk = testdata("gpt.bin");
    let a = gpt::event_data(&disk, 512).unwrap();
    // the root partition's UUID (the first half of the verity root hash)
    disk[1024 + 128 + 16] ^= 1;
    assert_ne!(gpt::event_data(&disk, 512).unwrap(), a);
}

#[test]
#[ignore = "needs image A's UKI at POD_IMAGE_A_UKI"]
fn rtmr1_is_computed_from_the_image_event_by_event() {
    let q = report::parse(&testdata("quote.bin")).unwrap();
    let log = eventlog::parse(&testdata("ccel.bin")).unwrap();
    let predicted = expect::rtmr1(&uki(), &testdata("gpt.bin")).unwrap();
    let logged = rtmr_events(&log, 1);
    assert_eq!(predicted.len(), logged.len());
    for (p, l) in predicted.iter().zip(&logged) {
        assert_eq!(Some(p.digest), l.sha384, "{}", p.what);
    }
    assert!(predicted.iter().all(|e| e.origin != Origin::Uki || e.rtmr == 1));
    assert_eq!(expect::fold(&predicted, 1), q.rtmr[1]);
}

#[test]
#[ignore = "needs image A's UKI at POD_IMAGE_A_UKI"]
fn rtmr2_is_predicted_empty_for_a_255_stub() {
    let uki = uki();
    assert_eq!(expect::stub_version(&uki), Some(255));
    assert!(expect::rtmr2(&uki).unwrap().is_empty());
}

#[test]
#[ignore = "needs image A's UKI at POD_IMAGE_A_UKI"]
fn one_changed_byte_of_the_uki_changes_the_prediction() {
    let mut uki = uki();
    let gpt = testdata("gpt.bin");
    let a = expect::fold(&expect::rtmr1(&uki, &gpt).unwrap(), 1);
    // a byte inside .cmdline: "console=ttyS0" becomes "console=ttyS1"
    let at = uki.windows(13).position(|w| w == b"console=ttyS0").unwrap() + 12;
    uki[at] = b'1';
    assert_ne!(expect::fold(&expect::rtmr1(&uki, &gpt).unwrap(), 1), a);
}

// Image D: image A with Debian trixie's systemd-stub 257 in place of
// Ubuntu's 255. Stub 257 measures the UKI's sections into RTMR2 too.

fn testdata_d(name: &str) -> Vec<u8> {
    let path = format!("{}/testdata/pod-image-d/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[test]
fn with_stub_257_rtmr2_holds_twelve_stub_events() {
    let q = report::parse(&testdata_d("quote.bin")).unwrap();
    let log = eventlog::parse(&testdata_d("ccel.bin")).unwrap();
    assert_eq!(replay::replay(&log).unwrap(), q.rtmr);
    let in_rtmr2 = rtmr_events(&log, 2);
    assert_eq!(in_rtmr2.len(), 12);
    // pairs: the section's name (with its NUL), then its contents
    let name: [u8; 48] = Sha384::digest(b".cmdline\0").into();
    assert_eq!(in_rtmr2[4].sha384, Some(name));
    assert!(fragment_pod::attest::describe::describe(in_rtmr2[4]).contains(".cmdline"));
}

#[test]
#[ignore = "needs image D's UKI at POD_IMAGE_D_UKI"]
fn with_stub_257_rtmr1_and_rtmr2_are_computed_from_the_image() {
    let (path, uki) = uki_from("POD_IMAGE_D_UKI");
    let sums = String::from_utf8(testdata_d("sha256sums")).unwrap();
    assert!(sums.contains(&hex::encode(Sha256::digest(&uki))), "{path} is not image D's UKI");
    assert_eq!(expect::stub_version(&uki), Some(257));
    let q = report::parse(&testdata_d("quote.bin")).unwrap();
    let log = eventlog::parse(&testdata_d("ccel.bin")).unwrap();
    let mut predicted = expect::rtmr1(&uki, &testdata_d("gpt.bin")).unwrap();
    predicted.extend(expect::rtmr2(&uki).unwrap());
    for rtmr in [1, 2] {
        let mine: Vec<_> = predicted.iter().filter(|e| e.rtmr == rtmr).collect();
        let logged = rtmr_events(&log, rtmr);
        assert_eq!(mine.len(), logged.len());
        for (p, l) in mine.iter().zip(&logged) {
            assert_eq!(Some(p.digest), l.sha384, "RTMR{rtmr}: {}", p.what);
        }
        assert_eq!(expect::fold(&predicted, rtmr), q.rtmr[rtmr]);
    }
}
