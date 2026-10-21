//! Run the shell's actual build-section handler on the pinned Node with
//! recorded DOM/clipboard boundaries. No browser or clipboard is accessed.

#[test]
fn build_prompt_copy_contract() {
    let root = fragment_devstack::repo_root();
    let node = fragment_devstack::node::locate(&root.join(fragment_devstack::TOOLS_DIR)).expect("the pinned Node");
    let output = node
        .command(&root.join(fragment_devstack::CACHE_DIR))
        .expect("Node command")
        .args(["--test", "xtask/tests/fixtures/build-prompt.mjs"])
        .current_dir(root)
        .output()
        .expect("run build prompt tests");
    assert!(output.status.success(), "build prompt tests failed:\n{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    print!("{}", String::from_utf8_lossy(&output.stdout));
}
