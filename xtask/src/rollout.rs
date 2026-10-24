//! A deploy from `wrangler deploy` on, and how it ends: all on the new
//! release, all back on the one before, or, when neither can be had,
//! failed saying where it stands. Never a Worker of one release left in
//! front of objects of another without the job failing.
//!
//! What it answers (2026-10-24, production's promotion of bfc85d43, run
//! 40126945229): `wrangler deploy` uploads and deploys the Worker, and
//! then reads the version back to apply the Durable Object-managed
//! Container application's settings, then deploys the triggers. From
//! about 21:00 to 21:15 UTC Cloudflare's API answered that read with "not
//! ready" (code 100146, asked five times 500 ms apart: every try failed
//! 2.7 s after its upload, on production and the preview alike, whatever
//! the image), so wrangler failed with the Worker live. xtask ran it again
//! twice, each run another upload and deploy that met the same, and ended
//! red; meanwhile the objects Cloudflare kept busy ran the release before
//! for five minutes (the Registry, three computers, their agents'
//! fragments, a ledger), refusing the new Worker's calls (6,726 refusals).
//!
//! So, in order:
//!
//! 1. The release live now is read from the Worker's deployments (its
//!    version at 100%, and that version's `FRAGMENT_DEPLOY_ID` and
//!    `FRAGMENT_SCHEMA`): the one a failure goes back to.
//! 2. `wrangler deploy`, again only for an image's push that timed out
//!    (the hand deploys' 4 GB layer did), which is before any upload.
//! 3. Whether the Worker changed is read from its deployments, never from
//!    wrangler's words: when it failed and nothing changed, the deploy ends
//!    so, nothing to undo. When it failed with the Worker live, the deploy
//!    is finished, not run again (each run is another upload): the version
//!    read back, the Computer's container application checked as wrangler
//!    checks it, and the triggers deployed (`wrangler triggers deploy`),
//!    asked again with backoff for `FINISH_LIMIT`.
//! 4. Verified: the platform's `/healthz/objects` asked until every object
//!    class answers the new release (docs/api.md, Health), for
//!    `VERIFY_LIMIT`, longer than the five minutes Cloudflare holds a busy
//!    object on its old code (measured 2026-10-21, and again 2026-10-24 on
//!    a throwaway Worker: a busy object took a deploy's code 300 s after
//!    it, whatever `code_update_strategy` asked).
//! 5. When it cannot be finished or verified, the release before is
//!    deployed again (`wrangler rollback` to its version), and verified
//!    the same way; unless the new release's schema is not the one before
//!    (an object that ran a newer step refuses older code: docs/MODEL.md,
//!    Schemas), or there was none: then it fails, forward only, saying so.

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::Result;
use serde_json::Value;

use crate::prepare::Clock;

/// How many times `wrangler deploy` runs at most: again only after a push
/// that timed out.
pub(crate) const DEPLOY_TRIES: u32 = 3;
/// The pause before each of its tries after the first.
pub(crate) const DEPLOY_PAUSE: Duration = Duration::from_secs(10);
/// How long a deploy whose Worker is live is asked to finish, its pauses
/// doubling from `FINISH_PAUSE_FIRST` to `FINISH_PAUSE_MAX`.
pub(crate) const FINISH_LIMIT: Duration = Duration::from_secs(10 * 60);
const FINISH_PAUSE_FIRST: Duration = Duration::from_secs(10);
const FINISH_PAUSE_MAX: Duration = Duration::from_secs(60);
/// How long the platform has to answer a release at every object class:
/// past Cloudflare's five minutes of a busy object's old code.
pub(crate) const VERIFY_LIMIT: Duration = Duration::from_secs(7 * 60);
/// The pause between two probes, and how often a wait says where it is.
const PROBE_PAUSE: Duration = Duration::from_secs(5);
const SAY_EVERY: Duration = Duration::from_secs(30);

/// A release the Worker has deployed: its version, and the release and
/// schema that version names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Live {
    /// The Worker version's id (Cloudflare's).
    pub version: String,
    /// Its `FRAGMENT_DEPLOY_ID`.
    pub release: String,
    /// Its `FRAGMENT_SCHEMA` (deploy.rs, `schema_id`): `None` for a
    /// release from before it was named, whose schema is unknown.
    pub schema: Option<String>,
}

/// How one `wrangler deploy` ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Wrangled {
    Done,
    /// An image's push timed out (before any upload): worth running again.
    PushTimedOut,
    /// Anything else, as wrangler said it: whether the Worker changed is
    /// read from its deployments.
    Failed(String),
}

/// What the platform answered a probe (`/healthz/objects`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Probed {
    /// Every object class admitted this release (200, naming it).
    Release(String),
    /// Not yet: what it said (which class refused, and by which release).
    Not(String),
    /// No such route: a release from before it, which `/healthz` alone
    /// names (`x-fragment-deploy`).
    NoRoute(Option<String>),
}

/// What a deploy does to the platform, each step one call: the real one
/// (deploy.rs, `Cloudflare`) or a test's.
pub(crate) trait Platform {
    /// The release live now, or `None`: no Worker yet.
    fn live(&mut self) -> Result<Option<Live>>;
    /// The version live now, its deployments alone read (a version just
    /// uploaded may not read back yet: that is `finish`'s).
    fn version(&mut self) -> Result<Option<String>>;
    /// `wrangler deploy`, once.
    fn deploy(&mut self) -> Result<Wrangled>;
    /// The rest of a deploy whose Worker is live (`version`): read back,
    /// its container application checked, its triggers deployed.
    fn finish(&mut self, version: &str) -> Result<()>;
    /// `GET /healthz/objects`.
    fn probe(&mut self) -> Result<Probed>;
    /// The release `to` deployed again (`wrangler rollback`).
    fn rollback(&mut self, to: &Live) -> Result<()>;
}

/// How a deploy ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Every object class answers the new release.
    Deployed,
    /// The Worker never changed: the deploy failed before its upload.
    Unchanged(String),
    /// The release before is back, verified, and why the new one is not.
    RolledBack { to: String, why: String },
    /// Neither: where it stands, for a person to act on.
    Stuck(String),
}

impl Outcome {
    /// The deploy's result: only `Deployed` succeeds.
    pub(crate) fn result(self) -> Result<()> {
        match self {
            Outcome::Deployed => Ok(()),
            Outcome::Unchanged(why) => anyhow::bail!("deploy: nothing changed: {why}"),
            Outcome::RolledBack { to, why } => anyhow::bail!("deploy: rolled back to {to}, which every object class answers again: {why}"),
            Outcome::Stuck(why) => anyhow::bail!("deploy: STUCK, a person must act: {why}"),
        }
    }
}

/// The deploy of release `new` (its schema `schema`) on `p`, from
/// `wrangler deploy` on (the steps above), timed by `clock`.
pub(crate) fn run(p: &mut impl Platform, clock: &mut impl Clock, new: &str, schema: &str) -> Result<Outcome> {
    let before = p.live()?;
    if let Some(b) = &before {
        println!("deploy: live now: {} (version {})", b.release, b.version);
    }
    let wrangled = deployed(p, clock)?;
    let now = p.version()?;
    let changed = now.as_deref() != before.as_ref().map(|l| l.version.as_str());
    let unfinished = match (&wrangled, changed) {
        (Wrangled::Done, _) => None,
        (failed, false) => return Ok(Outcome::Unchanged(said(failed))),
        (failed, true) => Some(said(failed)),
    };
    let why = match (unfinished, now) {
        (Some(failed), Some(version)) => {
            println!("deploy: wrangler failed with version {version} live ({failed}): finishing it, not running it again");
            finished(p, clock, &version).err().map(|e| format!("{failed}; and finishing it failed for {} (the limit): {e:#}", minutes(FINISH_LIMIT)))
        }
        (Some(failed), None) => Some(format!("{failed}; and the Worker has no deployment")),
        (None, _) => None,
    };
    let why = match why {
        Some(why) => why,
        None => match verified(p, clock, new, false)? {
            Ok(()) => return Ok(Outcome::Deployed),
            Err(why) => why,
        },
    };
    back(p, clock, before, schema, why)
}

/// `wrangler deploy` until it is done, fails other than by a push that
/// timed out, or has run `DEPLOY_TRIES` times.
fn deployed(p: &mut impl Platform, clock: &mut impl Clock) -> Result<Wrangled> {
    for tries in 1..=DEPLOY_TRIES {
        match p.deploy()? {
            Wrangled::PushTimedOut if tries < DEPLOY_TRIES => {
                eprintln!("deploy: an image's push timed out (try {tries} of {DEPLOY_TRIES}); running wrangler deploy again in {}", minutes(DEPLOY_PAUSE));
                clock.sleep(DEPLOY_PAUSE);
            }
            wrangled => return Ok(wrangled),
        }
    }
    unreachable!("the last try returns")
}

/// The finish of a deploy whose Worker (`version`) is live, asked again
/// with backoff until it is done or `FINISH_LIMIT` passes: the last
/// failure then.
fn finished(p: &mut impl Platform, clock: &mut impl Clock, version: &str) -> Result<()> {
    let start = clock.now();
    let mut pause = FINISH_PAUSE_FIRST;
    loop {
        match p.finish(version) {
            Ok(()) => {
                println!("deploy: finished (version {version}) after {}", minutes(clock.now() - start));
                return Ok(());
            }
            Err(e) if clock.now() - start + pause > FINISH_LIMIT => return Err(e),
            Err(e) => {
                eprintln!("deploy: finishing version {version} failed ({e:#}); asking again in {}", minutes(pause));
                clock.sleep(pause);
                pause = (pause * 2).min(FINISH_PAUSE_MAX);
            }
        }
    }
}

/// The platform probed until every object class answers `release`, for
/// `VERIFY_LIMIT`: `Err` saying what it answered last, past it. A release
/// with no probe route (`old`: one gone back to, from before it) is
/// verified by `/healthz` naming it.
fn verified(p: &mut impl Platform, clock: &mut impl Clock, release: &str, old: bool) -> Result<Result<(), String>> {
    let start = clock.now();
    let mut said = start;
    loop {
        let last = match p.probe() {
            Ok(Probed::Release(r)) if r == release => {
                println!("deploy: every object class answers {release} ({} after the deploy)", minutes(clock.now() - start));
                return Ok(Ok(()));
            }
            Ok(Probed::NoRoute(Some(r))) if old && r == release => {
                println!("deploy: the Worker answers {release}, a release with no probe of its objects");
                return Ok(Ok(()));
            }
            Ok(Probed::Release(other)) => format!("every object class answers {other}, not {release}"),
            Ok(Probed::Not(why)) => why,
            Ok(Probed::NoRoute(named)) => format!("no /healthz/objects, and /healthz names {}", named.as_deref().unwrap_or("no release")),
            Err(e) => format!("{e:#}"),
        };
        let waited = clock.now() - start;
        if waited >= VERIFY_LIMIT {
            return Ok(Err(format!("{release} not answered by every object class after {} (the limit): {last}", minutes(waited))));
        }
        if clock.now() - said >= SAY_EVERY {
            said = clock.now();
            println!("deploy: waiting for every object class to answer {release} ({} of {}): {last}", minutes(waited), minutes(VERIFY_LIMIT));
        }
        clock.sleep(PROBE_PAUSE);
    }
}

/// The release `before` deployed again, after `why`, when it may be: the
/// outcome either way.
fn back(p: &mut impl Platform, clock: &mut impl Clock, before: Option<Live>, schema: &str, why: String) -> Result<Outcome> {
    let Some(before) = before else {
        return Ok(Outcome::Stuck(format!("{why}; and there is no release before to go back to (the Worker's first deploy)")));
    };
    match before.schema.as_deref() {
        Some(s) if s == schema => {}
        Some(s) => return Ok(Outcome::Stuck(format!("{why}; and it is not rolled back: its schema ({schema}) is not {}'s ({s}), whose code an object that ran the newer steps refuses (forward only: fix it forward)", before.release))),
        None => return Ok(Outcome::Stuck(format!("{why}; and it is not rolled back: {} names no schema, so whether its code takes these objects is unknown (forward only: fix it forward)", before.release))),
    }
    eprintln!("deploy: {why}; rolling back to {} (version {})", before.release, before.version);
    if let Err(e) = p.rollback(&before) {
        return Ok(Outcome::Stuck(format!("{why}; and the rollback to {} failed: {e:#}", before.release)));
    }
    Ok(match verified(p, clock, &before.release, true)? {
        Ok(()) => Outcome::RolledBack { to: before.release, why },
        Err(also) => Outcome::Stuck(format!("{why}; and after the rollback, {also}")),
    })
}

/// What a failed `wrangler deploy` said, for the outcome.
fn said(w: &Wrangled) -> String {
    match w {
        Wrangled::Done => "done".into(),
        Wrangled::PushTimedOut => "an image's push to the registry timed out, three times".into(),
        Wrangled::Failed(why) => why.clone(),
    }
}

/// A length as people read it: `2m05s`, `40s`.
fn minutes(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 { format!("{s}s") } else { format!("{}m{:02}s", s / 60, s % 60) }
}

/// `/healthz/objects`' answer read (cell/src/routed.rs, `objects`): its
/// status, its JSON, and the release `/healthz` names (`x-fragment-deploy`).
pub(crate) fn probe_of(status: u16, body: &Value, named: Option<&str>) -> Probed {
    match status {
        404 => Probed::NoRoute(named.map(str::to_string)),
        200 => match (body["release"].as_str(), named) {
            (Some(r), Some(n)) if r == n => Probed::Release(r.to_string()),
            (r, n) => Probed::Not(format!("the probe names {} and the Worker {}", r.unwrap_or("no release"), n.unwrap_or("no release"))),
        },
        _ => {
            let objects: BTreeMap<String, Value> = body["objects"].as_object().map(|o| o.clone().into_iter().collect()).unwrap_or_default();
            let not: Vec<String> = objects
                .iter()
                .filter(|(_, v)| v.get("release").is_none())
                .map(|(class, v)| match (v["refusedBy"].as_str(), v["error"].as_str()) {
                    (Some(by), _) => format!("{class} runs {by}"),
                    (_, Some(e)) => format!("{class}: {e}"),
                    _ => format!("{class}: {v}"),
                })
                .collect();
            if not.is_empty() { Probed::Not(format!("the probe answered {status}")) } else { Probed::Not(not.join(", ")) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prepare::tests::Fake;

    /// The platform as a deploy sees it: the Worker's release and its
    /// objects', each step scripted. The objects take the Worker's release
    /// `lag` probes after it changes (`None`: never, as a busy object kept
    /// on old code would be, without end).
    struct World {
        versions: Vec<Live>,
        worker: Live,
        objects: String,
        lag: Option<u32>,
        probes_since: u32,
        deploys: Vec<Wrangled>,
        /// Whether a failed deploy uploaded first (the Worker changed).
        uploads_before_failing: bool,
        finish_fails: u32,
        rollback_lag: Option<u32>,
        /// A release whose objects fail the probe whatever they run (a
        /// class that cannot start its code).
        broken: Option<String>,
        calls: Vec<String>,
        new: Live,
    }

    impl World {
        fn new(before: &str, new: &str, schema_before: Option<&str>) -> World {
            let old = Live { version: "v-old".into(), release: before.into(), schema: schema_before.map(str::to_string) };
            World {
                versions: vec![old.clone()],
                worker: old,
                objects: before.into(),
                lag: Some(0),
                probes_since: 0,
                deploys: vec![Wrangled::Done],
                uploads_before_failing: false,
                finish_fails: 0,
                rollback_lag: Some(0),
                broken: None,
                calls: vec![],
                new: Live { version: "v-new".into(), release: new.into(), schema: Some(SCHEMA.into()) },
            }
        }
        fn first_deploy(new: &str) -> World {
            let mut w = World::new("none", new, None);
            w.versions.clear();
            w
        }
        /// Each side's release when the deploy ended.
        fn ends(&self) -> (String, String) {
            (self.worker.release.clone(), self.objects.clone())
        }
        fn upload(&mut self) {
            self.worker = self.new.clone();
            self.versions.push(self.new.clone());
            self.probes_since = 0;
        }
    }

    impl Platform for World {
        fn live(&mut self) -> Result<Option<Live>> {
            self.calls.push("live".into());
            Ok(self.versions.last().map(|_| self.worker.clone()))
        }
        fn version(&mut self) -> Result<Option<String>> {
            self.calls.push("version".into());
            Ok(self.versions.last().map(|_| self.worker.version.clone()))
        }
        fn deploy(&mut self) -> Result<Wrangled> {
            self.calls.push("deploy".into());
            let next = if self.deploys.len() > 1 { self.deploys.remove(0) } else { self.deploys[0].clone() };
            match &next {
                Wrangled::Done => self.upload(),
                Wrangled::Failed(_) if self.uploads_before_failing => self.upload(),
                _ => {}
            }
            Ok(next)
        }
        fn finish(&mut self, version: &str) -> Result<()> {
            self.calls.push(format!("finish {version}"));
            assert_eq!(version, self.worker.version, "it finishes the version live");
            if self.finish_fails > 0 {
                self.finish_fails -= 1;
                anyhow::bail!("the version is not ready (100146)");
            }
            Ok(())
        }
        fn probe(&mut self) -> Result<Probed> {
            self.calls.push("probe".into());
            if self.lag.is_some_and(|lag| self.probes_since >= lag) {
                self.objects = self.worker.release.clone();
            }
            self.probes_since += 1;
            if self.broken.as_ref() == Some(&self.objects) {
                return Ok(Probed::Not("Ledger: 503 schema_refused: its steps failed".into()));
            }
            Ok(if self.objects == self.worker.release {
                Probed::Release(self.worker.release.clone())
            } else {
                Probed::Not(format!("Registry runs {}", self.objects))
            })
        }
        fn rollback(&mut self, to: &Live) -> Result<()> {
            self.calls.push(format!("rollback {}", to.version));
            self.worker = to.clone();
            self.versions.push(to.clone());
            self.probes_since = 0;
            self.lag = self.rollback_lag;
            Ok(())
        }
    }

    const SCHEMA: &str = "5c1e3b0f2a9d4e61";
    const OLD: &str = "0e9bc7b36e5c";
    const NEW: &str = "bfc85d431653";

    fn deploy(w: &mut World) -> Outcome {
        run(w, &mut Fake::default(), NEW, SCHEMA).unwrap()
    }

    fn count(w: &World, call: &str) -> usize {
        w.calls.iter().filter(|c| c.starts_with(call)).count()
    }

    #[test]
    fn a_deploy_that_verifies_ends_all_new() {
        let mut w = World::new(OLD, NEW, Some(SCHEMA));
        assert_eq!(deploy(&mut w), Outcome::Deployed);
        assert_eq!(w.ends(), (NEW.into(), NEW.into()));
        assert_eq!((count(&w, "deploy"), count(&w, "finish"), count(&w, "rollback")), (1, 0, 0));
    }

    #[test]
    fn a_deploy_that_fails_before_its_upload_changes_nothing() {
        // an image not ready (prepare.rs refuses before wrangler runs at
        // all), or one wrangler gives up on, or a config Cloudflare refuses
        let mut w = World::new(OLD, NEW, Some(SCHEMA));
        w.deploys = vec![Wrangled::Failed("Timed out while preparing the container image on Cloudflare's network.".into())];
        let out = deploy(&mut w);
        assert!(matches!(&out, Outcome::Unchanged(why) if why.contains("Timed out while preparing")), "{out:?}");
        assert_eq!(w.ends(), (OLD.into(), OLD.into()));
        assert_eq!((count(&w, "deploy"), count(&w, "probe"), count(&w, "rollback")), (1, 0, 0), "not run again, nothing to verify or undo");
        assert!(out.result().unwrap_err().to_string().contains("nothing changed"));
    }

    #[test]
    fn a_push_that_timed_out_is_run_again_and_no_more_than_its_tries() {
        let mut w = World::new(OLD, NEW, Some(SCHEMA));
        w.deploys = vec![Wrangled::PushTimedOut, Wrangled::PushTimedOut, Wrangled::Done];
        assert_eq!(deploy(&mut w), Outcome::Deployed);
        assert_eq!(count(&w, "deploy"), 3);

        let mut w = World::new(OLD, NEW, Some(SCHEMA));
        w.deploys = vec![Wrangled::PushTimedOut];
        assert!(matches!(deploy(&mut w), Outcome::Unchanged(_)));
        assert_eq!((count(&w, "deploy"), w.ends()), (DEPLOY_TRIES as usize, (OLD.into(), OLD.into())));
    }

    #[test]
    fn a_worker_live_whose_settings_were_not_applied_is_finished_not_uploaded_again() {
        // 2026-10-24: "The Worker version was deployed, but Wrangler could
        // not finish applying its Durable Object-managed Container
        // application settings", for minutes
        let mut w = World::new(OLD, NEW, Some(SCHEMA));
        w.deploys = vec![Wrangled::Failed("could not finish applying its Durable Object-managed Container application settings".into())];
        w.uploads_before_failing = true;
        w.finish_fails = 4;
        assert_eq!(deploy(&mut w), Outcome::Deployed);
        assert_eq!(w.ends(), (NEW.into(), NEW.into()));
        assert_eq!((count(&w, "deploy"), count(&w, "finish v-new"), count(&w, "rollback")), (1, 5, 0), "one upload; finished on its fifth ask");
    }

    #[test]
    fn a_worker_live_that_cannot_be_finished_is_rolled_back_all_old() {
        let mut w = World::new(OLD, NEW, Some(SCHEMA));
        w.deploys = vec![Wrangled::Failed("could not finish applying".into())];
        w.uploads_before_failing = true;
        w.finish_fails = u32::MAX;
        let out = deploy(&mut w);
        assert!(matches!(&out, Outcome::RolledBack { to, why } if to == OLD && why.contains("finishing it failed") && why.contains("100146")), "{out:?}");
        assert_eq!(w.ends(), (OLD.into(), OLD.into()));
        assert_eq!(count(&w, "deploy"), 1);
        assert!(out.result().unwrap_err().to_string().contains("rolled back"));
    }

    #[test]
    fn objects_held_on_old_code_within_the_limit_end_all_new() {
        // Cloudflare's five minutes: 60 probes 5 s apart
        let mut w = World::new(OLD, NEW, Some(SCHEMA));
        w.lag = Some(60);
        assert_eq!(deploy(&mut w), Outcome::Deployed);
        assert_eq!(w.ends(), (NEW.into(), NEW.into()));
        assert_eq!(count(&w, "probe"), 61);
    }

    #[test]
    fn objects_that_never_take_the_new_release_are_rolled_back_all_old() {
        let mut w = World::new(OLD, NEW, Some(SCHEMA));
        w.lag = None;
        let out = deploy(&mut w);
        assert!(matches!(&out, Outcome::RolledBack { to, why } if to == OLD && why.contains("Registry runs 0e9bc7b36e5c") && why.contains("7m00s")), "{out:?}");
        assert_eq!(w.ends(), (OLD.into(), OLD.into()), "the objects held on the old release meet it again");
        assert_eq!(count(&w, "probe") as u128, VERIFY_LIMIT.as_millis() / PROBE_PAUSE.as_millis() + 2, "bounded: the limit's probes, and the rollback's one");
    }

    #[test]
    fn a_release_whose_schema_moved_is_never_rolled_back() {
        for schema_before in [Some("0000000000000000"), None] {
            let mut w = World::new(OLD, NEW, schema_before);
            w.lag = None;
            let out = deploy(&mut w);
            assert!(matches!(&out, Outcome::Stuck(why) if why.contains("forward only")), "{out:?}");
            assert_eq!(count(&w, "rollback"), 0);
            assert!(out.result().unwrap_err().to_string().contains("STUCK"), "the job fails loudly");
        }
    }

    #[test]
    fn a_first_deploy_has_nothing_to_go_back_to() {
        let mut w = World::first_deploy(NEW);
        w.lag = None;
        let out = deploy(&mut w);
        assert!(matches!(&out, Outcome::Stuck(why) if why.contains("no release before")), "{out:?}");
        assert_eq!(count(&w, "rollback"), 0);
    }

    #[test]
    fn a_release_that_cannot_run_is_rolled_back_all_old() {
        // its objects took it, and one class fails: the release before is back
        let mut w = World::new(OLD, NEW, Some(SCHEMA));
        w.broken = Some(NEW.into());
        let out = deploy(&mut w);
        assert!(matches!(&out, Outcome::RolledBack { to, why } if to == OLD && why.contains("schema_refused")), "{out:?}");
        assert_eq!(w.ends(), (OLD.into(), OLD.into()));
    }

    #[test]
    fn a_rollback_that_does_not_verify_is_stuck_and_says_both() {
        // the objects took the broken release and Cloudflare holds them there
        let mut w = World::new(OLD, NEW, Some(SCHEMA));
        w.broken = Some(NEW.into());
        w.rollback_lag = None;
        let out = deploy(&mut w);
        assert!(matches!(&out, Outcome::Stuck(why) if why.contains("after the rollback") && why.contains(NEW) && why.contains(OLD)), "{out:?}");
        assert_eq!(count(&w, "rollback"), 1, "rolled back once, not again");
    }

    #[test]
    fn the_probe_reads_as_the_cell_answers_it() {
        let ok = serde_json::json!({ "release": NEW, "objects": { "Registry": { "release": NEW } } });
        assert_eq!(probe_of(200, &ok, Some(NEW)), Probed::Release(NEW.into()));
        assert!(matches!(probe_of(200, &ok, Some(OLD)), Probed::Not(_)), "a probe and a Worker that disagree are no answer");
        let skewed = serde_json::json!({ "release": NEW, "objects": { "Registry": { "refusedBy": OLD }, "Ledger": { "release": NEW }, "Computer": { "error": "503 schema_refused: newer" } } });
        assert_eq!(probe_of(503, &skewed, Some(NEW)), Probed::Not(format!("Computer: 503 schema_refused: newer, Registry runs {OLD}")));
        assert_eq!(probe_of(404, &Value::Null, Some(OLD)), Probed::NoRoute(Some(OLD.into())));
        assert_eq!(probe_of(522, &Value::Null, None), Probed::Not("the probe answered 522".into()));
    }
}
