//! Deterministic billing UI contract tests. The real billing module runs
//! on the pinned Node, with recorded element helpers and API answers;
//! no browser, server, Stripe, or other vendor is contacted.

#[test]
fn guest_billing_contract() {
    let root = fragment_devstack::repo_root();
    let node = fragment_devstack::node::locate(&root.join(fragment_devstack::TOOLS_DIR)).expect("the pinned Node");
    let output = node
        .command(&root.join(fragment_devstack::CACHE_DIR))
        .expect("Node command")
        .args(["--test", "crates/e2e/fixtures/billing-guest.mjs"])
        .current_dir(root)
        .output()
        .expect("run billing contract tests");
    assert!(output.status.success(), "billing contract tests failed:\n{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    print!("{}", String::from_utf8_lossy(&output.stdout));
}
