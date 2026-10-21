//! Actual connection handlers with fake windows, HTTP and lifecycle delivery.

#[test]
fn connection_popups_keep_one_authorization_attempt() {
    let root = fragment_devstack::repo_root();
    let node = fragment_devstack::node::locate(&root.join(fragment_devstack::TOOLS_DIR)).expect("the pinned Node");
    let output = node
        .command(&root.join(fragment_devstack::CACHE_DIR))
        .expect("Node command")
        .args(["--test", "xtask/tests/fixtures/connection-popup.mjs"])
        .current_dir(root)
        .output()
        .expect("run connection popup tests");
    assert!(output.status.success(), "connection popup tests failed:\n{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    print!("{}", String::from_utf8_lossy(&output.stdout));
}
