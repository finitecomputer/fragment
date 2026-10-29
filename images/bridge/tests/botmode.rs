/// The image's Python keeper uses Hermes' own APIs; its pure journal and
/// retry paths run on the host with those imports stubbed.
#[test]
fn keeper_reconciles_journal_and_retries_notifications() {
    let out = std::process::Command::new("python3")
        .args(["-B", "tests/botmode.py"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("python3 runs the image's keeper tests");
    assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
}
