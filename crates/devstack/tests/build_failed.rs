//! A node whose Worker does not build: its entry imports a file that is
//! not there, as an import of a file the node's copy of cell/ lacked did
//! (runs 41655792118 and 41655789661). wrangler dev says its build failed
//! and waits for a file to change, neither ready nor gone; the start fails
//! on its words, with esbuild's error, instead of waiting out
//! `READY_TIMEOUT`.
//!
//! It boots wrangler dev on the pinned Node, so it runs by name:
//! `cargo test -p fragment-devstack --test build_failed -- --ignored`.

use std::time::Instant;

use fragment_devstack as devstack;

#[test]
#[ignore = "boots wrangler dev on the pinned Node: run by name with --ignored"]
fn a_node_whose_worker_does_not_build_fails_its_start_with_the_build_error() {
    let tools = devstack::Tools::locate().expect("the pinned Node and wrangler");
    let project = devstack::repo_root().join(format!("target/build-failed/{}", devstack::random_hex(4)));
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("worker.js"), "import { said } from \"./missing.mjs\";\nexport default { fetch() { return new Response(said); } };\n").unwrap();
    std::fs::write(devstack::local_config(&project), r#"{ "name": "build-failed", "main": "worker.js", "compatibility_date": "2026-01-01" }"#).unwrap();
    let opts = devstack::NodeOptions { project: project.clone(), listener: devstack::hold_port(0).unwrap(), log_dir: project.clone(), own_group: true };
    let t0 = Instant::now();
    let Err(e) = devstack::Node::start(&tools, &opts) else { panic!("a Worker that imports a missing file started") };
    let said = format!("{e:#}");
    println!("the start failed in {:.1?}", t0.elapsed());
    assert!(said.starts_with("wrangler dev's build failed"), "the start fails on the build's failure, not a wait run out: {said}");
    assert!(said.contains("Could not resolve \"./missing.mjs\""), "with esbuild's error: {said}");
    let _ = std::fs::remove_dir_all(&project);
}
