//! The sections, in the order they run.

mod admin;
mod agent_app;
mod agent_ask;
mod agent_restart;
mod agent_sharing;
mod agent_smoke;
mod ai_tools;
mod app;
mod appfiles;
mod blobs;
mod board;
mod brain;
mod chat;
mod chat_seed;
mod author;
mod billing;
mod checkout;
mod computers;
mod control;
mod credentials;
mod connections;
mod delegation;
mod export;
mod faces;
mod deliver;
mod doc;
mod doc_for;
mod door;
mod finite_private;
mod search;
mod search_across;
mod search_bench;
mod import_release;
mod governance;
mod frames;
mod icons;
pub mod hermes;
mod home;
mod identities;
mod isolation;
pub(crate) mod jobs;
mod keys;
mod levers;
mod ledger;
mod limits;
mod members;
mod migrations;
mod moves;
mod native;
mod notes;
mod pipe_live;
mod offload;
mod orgs;
mod plane;
mod posts;
mod schedules;
mod restart;
mod sessions;
mod share;
pub(crate) mod simplex_door;
mod shell;
mod signin;
mod simplex_settings;
pub(crate) mod simplex_setup;
mod site;
mod slides;
mod sync;
mod slack;
mod telegram_settings;
mod templates;
mod trials;
mod vault;
mod wipe;

use std::panic::{self, AssertUnwindSafe};
use std::time::Instant;

use anyhow::Result;

use crate::api::Api;
use crate::Suite;

/// A lane: it asks `s.section` whether its section runs, then checks.
type Lane = fn(&mut Suite, &Api) -> Result<()>;

/// The lanes, in the order they run.
const LANES: &[Lane] = &[
    control::auth,
    control::create,
    chat_seed::chat_seed,
    control::lockdown,
    keys::keys,
    members::members,
    identities::identities,
    signin::signin,
    door::door,
    native::native,
    connections::connections,
    vault::vault,
    levers::levers,
    members::secrets,
    members::secrets_page,
    members::reach,
    delegation::delegation,
    plane::files,
    plane::deploy,
    templates::templates,
    share::share,
    faces::faces,
    agent_sharing::agent_sharing,
    isolation::isolation,
    frames::frames,
    app::ops,
    app::public,
    app::effects,
    // after the last node restart before the triggers section: the cron
    // fragment's first minute passes while the sections between run (not
    // a section: its outcome waits for triggers, which reports it)
    |s, api| {
        s.cron = Some(jobs::cron(s, api).map_err(|e| format!("{e:#}")));
        Ok(())
    },
    limits::facet_cap,
    limits::capability_cost,
    // by name: its stack builds the door's image
    simplex_door::door,
    limits::facet_restart,
    limits::lockdown,
    site::site,
    site::watch,
    icons::icons,
    author::schemas,
    author::channels,
    author::live,
    author::routes,
    author::cli,
    author::browser,
    jobs::jobs,
    |s, api| {
        let cron = s.cron.take();
        jobs::triggers(s, api, cron)
    },
    appfiles::appfiles,
    blobs::blobs,
    sessions::sessions,
    sessions::sessions_ui,
    schedules::schedules_ui,
    home::home,
    offload::offload,
    notes::notes,
    brain::brain,
    brain::brain_release,
    doc::doc,
    doc_for::doc_for,
    slides::slides,
    board::board,
    deliver::push,
    deliver::ai,
    ledger::ledger_lane,
    ledger::transcribe_lane,
    finite_private::finite_private,
    orgs::orgs,
    billing::billing,
    governance::governance,
    billing::billing_page,
    trials::trials,
    trials::trial_seats,
    admin::admin,
    admin::admin_page,
    shell::shell_platform,
    computers::computers,
    computers::computer_room,
    export::export,
    search::search,
    search_across::search_across,
    search_bench::search_bench,
    import_release::import_release,
    chat::chat,
    chat::chat_history,
    shell::shell_ui,
    wipe::wipe,
    hermes::hermes,
    agent_smoke::agent_smoke,
    agent_ask::agent_ask,
    agent_app::agent_app,
    agent_restart::agent_restart,
    pipe_live::pipe_live,
    checkout::checkout,
    sync::folder_sync,
    migrations::migrations,
    moves::moves,
    moves::move_import,
    moves::move_big,
    moves::move_ui,
    moves::move_comp,
    restart::restart,
];

/// The suite split for CI (`--shard k/n`): each shard runs on a runner of
/// its own, with its own build and node, the sections it lists in the
/// lanes' order. Every section is in exactly one shard (a test below), so
/// the shards together run what one whole run does. `hermes` runs only by
/// name, and `agent-smoke`, `agent-ask`, `agent-app`, `agent-restart` and `checkout` only
/// on a preview, so their shard reports each as the whole run does: one
/// skip.
///
/// Balanced by each section's median time in the seven master pushes of
/// 2026-11-22 and 23 that ran every shard (d1e171ee1 to 9094a72c5): 50
/// minutes in all. `computers` (997 s) is a shard alone and the run's
/// longest, so no split runs faster than it. The sections that make or
/// wipe a computer, or use this machine's Docker (their needs and uses:
/// needs.rs, `Suite::section_using`), are the second and third shards'
/// alone: `shell-ui` (554 s) and three more (639 s), then `chat` and the
/// rest (453 s with sections that make none). The last two hold only
/// sections that make no computer (453 s each), so their nodes run no
/// container and build no image. Before, the first shard held `computers`
/// and 48 s more, and the others 414 to 554 s.
/// Shard 4 holds `triggers` and the sections from the cron
/// fragment's deploy (after `effects`) up to it, so its first cron minute
/// passes while they run, as in a whole run.
pub const SHARDS: [&[&str]; 5] = [
    &["computers", "agent-app", "agent-ask", "agent-restart", "agent-smoke", "checkout", "hermes", "import-release", "pipe-live", "search-bench"],
    &["shell-ui", "export", "computer-room", "faces"],
    &[
        "chat", "billing-page", "move-ui", "restart", "admin", "sessions-ui", "billing", "wipe", "governance", "migrations", "move-import", "move",
        "agent-sharing", "templates", "admin-page", "move-comp", "shell", "deploy", "blobs", "create", "sessions", "native", "reach", "auth", "secrets-page",
        "home",
    ],
    &[
        "facet-cap", "capability-cost", "simplex-door", "facet-restart", "app-lockdown", "site", "watch", "icons", "schemas", "channels", "live", "routes",
        "cli", "browser", "jobs", "triggers", "finite-private", "brain", "search", "frames", "isolation", "members", "effects", "door", "trials",
        "chat-history", "transcribe", "notes", "public", "files", "ops", "delegation", "secrets", "doc-for", "lockdown", "trial-seats",
    ],
    &[
        "ledger", "chat-seed", "ai", "share", "signin", "sync", "slides", "push", "move-big", "connections", "board", "doc", "appfiles", "vault",
        "brain-release", "levers", "schedules-ui", "search-across", "identities", "orgs", "keys", "offload",
    ],
];

/// Whether shard `k` (from 1) of the table runs the section `name`.
pub fn shard_runs(k: u32, name: &str) -> bool {
    assert!((1..=SHARDS.len() as u32).contains(&k), "a shard parsed is one of the table's: {k}");
    SHARDS[k as usize - 1].contains(&name)
}

/// Runs every lane, each with an API of its own. A lane that returns an
/// error or panics is one FAIL, and the lanes after it still run: a red run
/// reports every failure at once.
pub fn run(s: &mut Suite) {
    for lane in LANES {
        run_one(s, *lane);
    }
}

/// Runs one lane, and prints how long its section took (what a rebalance
/// of `SHARDS` reads).
pub fn run_one(s: &mut Suite, lane: impl FnOnce(&mut Suite, &Api) -> Result<()>) {
    let api = s.api();
    let before = s.ran.len();
    let t0 = Instant::now();
    let stopped = match panic::catch_unwind(AssertUnwindSafe(|| lane(s, &api))) {
        Ok(Ok(())) => None,
        Ok(Err(e)) => Some(format!("{e:#}")),
        Err(panic) => Some(format!("it panicked: {}", panic_message(panic.as_ref()))),
    };
    // every lane asks for its section before anything else
    assert!(s.ran.len() <= before + 1, "a lane runs one section");
    if let Some(why) = stopped {
        assert!(s.ran.len() == before + 1, "a lane stops early only once its section runs: {why}");
        s.stopped_early(&why);
    }
    if s.ran.len() == before + 1 {
        println!("      ({} in {:.1?})", s.ran[before], t0.elapsed());
    }
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> &str {
    match (panic.downcast_ref::<&str>(), panic.downcast_ref::<String>()) {
        (Some(s), _) => s,
        (_, Some(s)) => s,
        _ => "(no message)",
    }
}

#[cfg(test)]
mod tests {
    //! The shard table against the lanes themselves: every section a lane
    //! asks for (a dry run's plan, which asks each lane without a node) is
    //! in exactly one shard, and each shard runs exactly its own.
    use super::*;

    /// Every section the lanes ask for, in their order.
    fn sections() -> Vec<String> {
        let args = crate::hosted::parse(&["--hosted", "--zone", "finite.place", "--branch", "p5", "--dry-run"].map(String::from)).unwrap();
        let (plan, unknown) = crate::hosted::plan(None, vec![], &args.hosted.expect("a hosted dry run"));
        assert!(unknown.is_empty());
        plan.into_iter().map(|p| p.section).collect()
    }

    #[test]
    fn every_section_is_in_exactly_one_shard() {
        let sections = sections();
        assert!(sections.len() >= 45, "the plan asks every lane: {} sections", sections.len());
        for name in &sections {
            let holders: Vec<usize> = (1..=SHARDS.len()).filter(|k| SHARDS[k - 1].contains(&name.as_str())).collect();
            assert_eq!(holders.len(), 1, "{name} is in shards {holders:?}: each section is in exactly one");
        }
        for (i, shard) in SHARDS.iter().enumerate() {
            assert!(!shard.is_empty(), "shard {} runs something", i + 1);
            for name in *shard {
                assert!(sections.iter().any(|s| s == name), "shard {} names {name}, which no lane asks for", i + 1);
            }
        }
        let listed: usize = SHARDS.iter().map(|s| s.len()).sum();
        assert_eq!(listed, sections.len(), "the table names each section once");
    }

    /// `shard_runs`, which a sharded run's selection asks, follows the
    /// table: shard k runs its own sections and no other's.
    #[test]
    fn a_shard_runs_its_own_sections_alone() {
        for name in sections() {
            let runs: Vec<u32> = (1..=SHARDS.len() as u32).filter(|k| shard_runs(*k, &name)).collect();
            assert_eq!(runs.len(), 1, "{name} runs in shards {runs:?}");
        }
        assert!(!(1..=SHARDS.len() as u32).any(|k| shard_runs(k, "nonesuch")));
    }

    /// The cron fragment is deployed (after `effects`) in the shard that
    /// runs `triggers`, and every section from there to `triggers` is in
    /// that shard: its first minute passes while they run, as in a whole run.
    #[test]
    fn the_triggers_shard_holds_the_sections_from_the_cron_deploy() {
        let sections = sections();
        let at = |name: &str| sections.iter().position(|s| s == name).unwrap_or_else(|| panic!("{name} is a section"));
        let (from, to) = (at("effects") + 1, at("triggers"));
        let shard = (1..=SHARDS.len() as u32).find(|k| shard_runs(*k, "triggers")).expect("a shard runs triggers");
        for name in &sections[from..=to] {
            assert!(shard_runs(shard, name), "{name} runs between the cron deploy and triggers, so in shard {shard}");
        }
    }
}
