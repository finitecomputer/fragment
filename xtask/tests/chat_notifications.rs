//! Chat presence and notification selection, without browser push services.

#[test]
fn chat_notifications_follow_document_presence() {
    let root = fragment_devstack::repo_root();
    let node = fragment_devstack::node::locate(&root.join(fragment_devstack::TOOLS_DIR)).expect("the pinned Node");
    let output = node
        .command(&root.join(fragment_devstack::CACHE_DIR))
        .expect("Node command")
        .args(["--test", "xtask/tests/fixtures/chat-notifications.mjs"])
        .current_dir(root)
        .output()
        .expect("run chat notification tests");
    assert!(output.status.success(), "chat notification tests failed:\n{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    print!("{}", String::from_utf8_lossy(&output.stdout));
}
