//! Run the real Settings connection handlers and API wrapper with a recorded
//! DOM and synthetic HTTP responses; no browser or provider is contacted.

#[test]
fn connection_removal_feedback() {
    let root = fragment_devstack::repo_root();
    let node = fragment_devstack::node::locate(&root.join(fragment_devstack::TOOLS_DIR)).expect("the pinned Node");
    let output = node
        .command(&root.join(fragment_devstack::CACHE_DIR))
        .expect("Node command")
        .args(["--test", "xtask/tests/fixtures/connection-removal.mjs"])
        .current_dir(root)
        .output()
        .expect("run connection removal tests");
    assert!(output.status.success(), "connection removal tests failed:\n{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    print!("{}", String::from_utf8_lossy(&output.stdout));
}
