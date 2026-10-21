//! Settings action completion races against navigation and identity changes.

#[test]
fn settings_actions_refresh_only_their_visit() {
    let root = fragment_devstack::repo_root();
    let node = fragment_devstack::node::locate(&root.join(fragment_devstack::TOOLS_DIR)).expect("the pinned Node");
    let output = node
        .command(&root.join(fragment_devstack::CACHE_DIR))
        .expect("Node command")
        .args(["--test", "xtask/tests/fixtures/settings-actions.mjs"])
        .current_dir(root)
        .output()
        .expect("run Settings action tests");
    assert!(output.status.success(), "Settings action tests failed:\n{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    print!("{}", String::from_utf8_lossy(&output.stdout));
}
