//! Shell/chat visibility protocol with synthetic windows, presence and push.

#[test]
fn chat_frames_only_suppress_notifications_while_shown() {
    let root = fragment_devstack::repo_root();
    let node = fragment_devstack::node::locate(&root.join(fragment_devstack::TOOLS_DIR)).expect("the pinned Node");
    let output = node
        .command(&root.join(fragment_devstack::CACHE_DIR))
        .expect("Node command")
        .args(["--test", "xtask/tests/fixtures/chat-frame-visibility.mjs"])
        .current_dir(root)
        .output()
        .expect("run chat frame visibility tests");
    assert!(output.status.success(), "chat frame visibility tests failed:\n{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    print!("{}", String::from_utf8_lossy(&output.stdout));
}
