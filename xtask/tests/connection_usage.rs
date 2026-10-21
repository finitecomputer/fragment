//! Production Settings usage reads with synthetic DOM, HTTP and lifecycle events.

#[test]
fn connection_usage_read_states_and_retry() {
    let root = fragment_devstack::repo_root();
    let node = fragment_devstack::node::locate(&root.join(fragment_devstack::TOOLS_DIR)).expect("the pinned Node");
    let output = node
        .command(&root.join(fragment_devstack::CACHE_DIR))
        .expect("Node command")
        .args(["--test", "xtask/tests/fixtures/connection-usage.mjs"])
        .current_dir(root)
        .output()
        .expect("run connection usage tests");
    assert!(output.status.success(), "connection usage tests failed:\n{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    print!("{}", String::from_utf8_lossy(&output.stdout));
}
