//! Execute the Settings loader against deterministic navigation/identity races.

#[test]
fn settings_responses_stay_with_their_view() {
    let root = fragment_devstack::repo_root();
    let node = fragment_devstack::node::locate(&root.join(fragment_devstack::TOOLS_DIR)).expect("the pinned Node");
    let output = node
        .command(&root.join(fragment_devstack::CACHE_DIR))
        .expect("Node command")
        .args(["--test", "xtask/tests/fixtures/settings-responses.mjs"])
        .current_dir(root)
        .output()
        .expect("run Settings response tests");
    assert!(output.status.success(), "Settings response tests failed:\n{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    print!("{}", String::from_utf8_lossy(&output.stdout));
}
