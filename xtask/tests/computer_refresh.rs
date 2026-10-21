//! Execute the shell's refresh and identity boundaries with synthetic APIs.

#[test]
fn computer_refresh_preserves_confirmed_state() {
    let root = fragment_devstack::repo_root();
    let node = fragment_devstack::node::locate(&root.join(fragment_devstack::TOOLS_DIR)).expect("the pinned Node");
    let output = node
        .command(&root.join(fragment_devstack::CACHE_DIR))
        .expect("Node command")
        .args(["--test", "xtask/tests/fixtures/computer-refresh.mjs"])
        .current_dir(root)
        .output()
        .expect("run computer refresh tests");
    assert!(output.status.success(), "computer refresh tests failed:\n{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    print!("{}", String::from_utf8_lossy(&output.stdout));
}
