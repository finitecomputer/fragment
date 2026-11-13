//! Against the quote, nonce and CCEL captured from a c3-standard-8 TDX VM
//! (stock Ubuntu 24.04, kernel 7.0.0-1017-gcp) through configfs-tsm.

use fragment_pod::attest::{describe, eventlog, nonce, replay, report};

fn testdata(name: &str) -> Vec<u8> {
    let path = format!("{}/testdata/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[test]
fn the_real_quote_carries_the_nonce_and_known_fields() {
    let r = report::parse(&testdata("quote.bin")).unwrap();
    let nonce = testdata("nonce.bin");
    assert_eq!(r.report_data, nonce::report_data(&nonce).unwrap());
    assert_eq!(r.quote_version, 4);
    assert_eq!(hex::encode(r.td_attributes), "0000001000000000");
    assert_eq!(r.td_attributes_u64() & report::attr::DEBUG, 0);
    assert!(hex::encode(r.mr_td).starts_with("f21e6b48"));
    assert_eq!(r.rtmr[3], [0; 48]);
}

#[test]
fn replaying_the_real_log_gives_the_real_rtmrs() {
    let r = report::parse(&testdata("quote.bin")).unwrap();
    let log = eventlog::parse(&testdata("ccel.bin")).unwrap();
    assert_eq!(log.events.len(), 111);
    assert_eq!(replay::replay(&log).unwrap(), r.rtmr);
}

#[test]
fn the_real_log_measures_the_kernel_and_command_line_into_rtmr2() {
    let log = eventlog::parse(&testdata("ccel.bin")).unwrap();
    let in_rtmr2: Vec<String> = log
        .events
        .iter()
        .filter(|e| e.rtmr() == Some(2))
        .map(describe::describe)
        .collect();
    assert!(in_rtmr2.iter().any(|d| d.starts_with("file GRUB read: /vmlinuz-7.0.0-1017-gcp")));
    let cmdline = log
        .events
        .iter()
        .find(|e| describe::describe(e).starts_with("kernel command line:"))
        .unwrap();
    assert_eq!(cmdline.rtmr(), Some(2));
    assert_eq!(describe::coverage(cmdline), describe::Coverage::GrubText);
}

#[test]
fn one_changed_event_byte_breaks_the_replay() {
    let r = report::parse(&testdata("quote.bin")).unwrap();
    let mut log = eventlog::parse(&testdata("ccel.bin")).unwrap();
    let last_rtmr2 = log.events.iter().rposition(|e| e.rtmr() == Some(2)).unwrap();
    log.events[last_rtmr2].sha384.as_mut().unwrap()[0] ^= 1;
    let replayed = replay::replay(&log).unwrap();
    assert_ne!(replayed[2], r.rtmr[2]);
    assert_eq!(replayed[0], r.rtmr[0]);
}
