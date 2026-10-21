//! Execute the complete production service worker with synthetic event,
//! notification and window-client APIs. No browser or push service runs.

#[test]
fn service_worker_notification_contract() {
    let root = fragment_devstack::repo_root();
    let node = fragment_devstack::node::locate(&root.join(fragment_devstack::TOOLS_DIR)).expect("the pinned Node");
    let output = node
        .command(&root.join(fragment_devstack::CACHE_DIR))
        .expect("Node command")
        .args(["--test", "xtask/tests/fixtures/service-worker.mjs"])
        .current_dir(root)
        .output()
        .expect("run service worker tests");
    assert!(output.status.success(), "service worker tests failed:\n{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    print!("{}", String::from_utf8_lossy(&output.stdout));
}
