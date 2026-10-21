//! Shared Hermes/runtime and browser model-display fixtures.

#[test]
fn model_display_parity_fixtures() {
    let root = fragment_devstack::repo_root();
    let node = fragment_devstack::node::locate(&root.join(fragment_devstack::TOOLS_DIR)).expect("the pinned Node");
    let output = node
        .command(&root.join(fragment_devstack::CACHE_DIR))
        .expect("Node command")
        .args(["--test", "xtask/tests/fixtures/model-display-parity.mjs"])
        .current_dir(root)
        .output()
        .expect("run model display parity tests");
    assert!(output.status.success(), "model display parity tests failed:\n{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    print!("{}", String::from_utf8_lossy(&output.stdout));
}
