//! Execute the export UI with synthetic DOM, HTTP and lifecycle callbacks.

#[test]
fn export_status_retry_and_cleanup() {
    let root = fragment_devstack::repo_root();
    let node = fragment_devstack::node::locate(&root.join(fragment_devstack::TOOLS_DIR)).expect("the pinned Node");
    let output = node
        .command(&root.join(fragment_devstack::CACHE_DIR))
        .expect("Node command")
        .args(["--test", "xtask/tests/fixtures/export-status.mjs"])
        .current_dir(root)
        .output()
        .expect("run export status tests");
    assert!(output.status.success(), "export status tests failed:\n{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    print!("{}", String::from_utf8_lossy(&output.stdout));
}
