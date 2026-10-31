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
//!    wrangler's words (one that could not even run to its end included),
//!    asked again with backoff for `OBSERVE_LIMIT` while they cannot be
//!    read, and failing as stuck, naming the release before, if they never
//!    are: when it failed and nothing changed, the deploy ends
//!    so, nothing to undo. When it failed with the Worker live, the deploy
//!    is finished, not run again (each run is another upload): the version
//!    read back, the Computer's container application checked as wrangler
//!    checks it, and the triggers deployed (`wrangler triggers deploy`),
//!    asked again with backoff for `FINISH_LIMIT`.
//! 4. Verified: the platform's `/healthz/objects` asked until every object
//!    class answers the new release (docs/api.md, Health), for
//!    `VERIFY_LIMIT`. Every object takes a deploy's code at once (the
//!    Worker asks for `immediate`: cell/wrangler.jsonc), so the limit is
//!    far past what a deploy takes (0 s on each of the preview's first ten
//!    deploys under it) and short of the five minutes Cloudflare's default
//!    held a busy object on its old code: should Cloudflare hold one again,
//!    the deploy is rolled back, red, never silently refused for minutes.
//! 5. When it cannot be finished or verified, the release before is
//!    deployed again (`wrangler rollback` to its version), and verified
//!    the same way; unless the new release's schema is not the one before
//!    (an object that ran a newer step refuses older code: docs/MODEL.md,
//!    Schemas), or there was none, or the deploy changed what the release
//!    before cannot take back (a branch's Stripe endpoint, made anew: its
//!    old signing secret signs for an endpoint deleted): then it fails,
//!    forward only, saying so.

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
/// How long the Worker's deployments are asked, after a failed deploy,
/// whether it changed the Worker, with the same backoff.
pub(crate) const OBSERVE_LIMIT: Duration = Duration::from_secs(3 * 60);
const FINISH_PAUSE_FIRST: Duration = Duration::from_secs(10);
const FINISH_PAUSE_MAX: Duration = Duration::from_secs(60);
/// How long the platform has to answer a release at every object class.
/// Objects take a deploy's code at once (`immediate`, cell/wrangler.jsonc):
/// each of the preview's first ten deploys under it answered in 0 s, each
/// deploy before it that was not held in 11 s at most (2026-10-27 to 31).
pub(crate) const VERIFY_LIMIT: Duration = Duration::from_secs(90);
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
/// `rollback` is `Err` saying why the release before cannot come back
/// whatever its schema (deploy.rs: a branch whose Stripe endpoint this
/// deploy made anew). Only reading the release live first may fail the
/// run (`Err`): from `wrangler deploy` on, every failure is an outcome.
pub(crate) fn run(p: &mut impl Platform, clock: &mut impl Clock, new: &str, schema: &str, rollback: Result<(), String>) -> Result<Outcome> {
    let before = p.live()?;
    if let Some(b) = &before {
        println!("deploy: live now: {} (version {})", b.release, b.version);
    }
    let why = match deployed(p, clock) {
        Wrangled::Done => None,
        failed => {
            let failed = said(&failed);
            match observed(p, clock, before.as_ref()) {
                Observed::Unchanged => return Ok(Outcome::Unchanged(failed)),
                Observed::Unknown(e) => {
                    let was = before.as_ref().map_or("none".to_string(), |b| format!("{} (version {})", b.release, b.version));
                    // the advice a person follows is held to the deploy's own rule
                    let advice = match eligible(before.as_ref(), schema, &rollback) {
                        Ok(()) => format!(
                            "ask /healthz/objects, and roll back by hand (`wrangler rollback {}`) if it is live and does not answer",
                            before.as_ref().expect("an eligible rollback has a release before").version
                        ),
                        Err(cannot) => format!("ask /healthz/objects, and do not roll back by hand: {cannot}"),
                    };
                    return Ok(Outcome::Stuck(format!(
                        "{failed}; and whether the Worker changed could not be read for {} ({e}). The release before is {was}; {new} may be live: {advice}",
                        minutes(OBSERVE_LIMIT)
                    )));
                }
                Observed::Live(version) => {
                    println!("deploy: wrangler failed with version {version} live ({failed}): finishing it, not running it again");
                    finished(p, clock, &version).err().map(|e| format!("{failed}; and finishing it failed for {} (the limit): {e:#}", minutes(FINISH_LIMIT)))
                }
            }
        }
    };
    let why = match why {
        Some(why) => why,
        None => match verified(p, clock, new, false)? {
            Ok(()) => return Ok(Outcome::Deployed),
            Err(why) => why,
        },
    };
    back(p, clock, before, schema, rollback, why)
}

/// `wrangler deploy` until it is done, fails other than by a push that
/// timed out, or has run `DEPLOY_TRIES` times. One that could not run to
/// its end (it could not start, or its output could not be read) is a
/// failure like any other: whether it changed the Worker is read after.
fn deployed(p: &mut impl Platform, clock: &mut impl Clock) -> Wrangled {
    for tries in 1..=DEPLOY_TRIES {
        match p.deploy() {
            Ok(Wrangled::PushTimedOut) if tries < DEPLOY_TRIES => {
                eprintln!("deploy: an image's push timed out (try {tries} of {DEPLOY_TRIES}); running wrangler deploy again in {}", minutes(DEPLOY_PAUSE));
                clock.sleep(DEPLOY_PAUSE);
            }
            Ok(wrangled) => return wrangled,
            Err(e) => return Wrangled::Failed(format!("wrangler deploy did not run to its end: {e:#}")),
        }
    }
    unreachable!("the last try returns")
}

/// Whether a failed deploy changed the Worker, as its deployments say.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Observed {
    /// Its version is the one before (or there is none, as before).
    Unchanged,
    /// This version is live now, another than the one before.
    Live(String),
    /// Not known: the deployments could not be read for `OBSERVE_LIMIT`.
    Unknown(String),
}

/// The Worker's deployments read, after a failed `wrangler deploy`, asked
/// again with backoff for `OBSERVE_LIMIT` while they cannot be: never
/// uploaded again meanwhile.
fn observed(p: &mut impl Platform, clock: &mut impl Clock, before: Option<&Live>) -> Observed {
    let start = clock.now();
    let mut pause = FINISH_PAUSE_FIRST;
    loop {
        let read = p.version();
        match read {
            Ok(now) if now.as_deref() == before.map(|b| b.version.as_str()) => return Observed::Unchanged,
            Ok(Some(version)) => return Observed::Live(version),
            Ok(None) => return Observed::Unknown("the Worker has no deployment, and had one".into()),
            Err(e) if clock.now() - start + pause > OBSERVE_LIMIT => return Observed::Unknown(format!("{e:#}")),
            Err(e) => {
                eprintln!("deploy: the Worker's deployments could not be read ({e:#}); asking again in {}", minutes(pause));
                clock.sleep(pause);
                pause = (pause * 2).min(FINISH_PAUSE_MAX);
            }
        }
    }
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
fn back(p: &mut impl Platform, clock: &mut impl Clock, before: Option<Live>, schema: &str, rollback: Result<(), String>, why: String) -> Result<Outcome> {
    if let Err(cannot) = eligible(before.as_ref(), schema, &rollback) {
        return Ok(Outcome::Stuck(format!("{why}; and {cannot}")));
    }
    let before = before.expect("an eligible rollback has a release before");
    eprintln!("deploy: {why}; rolling back to {} (version {})", before.release, before.version);
    if let Err(e) = p.rollback(&before) {
        return Ok(Outcome::Stuck(format!("{why}; and the rollback to {} failed: {e:#}", before.release)));
    }
    Ok(match verified(p, clock, &before.release, true)? {
        Ok(()) => Outcome::RolledBack { to: before.release, why },
        Err(also) => Outcome::Stuck(format!("{why}; and after the rollback, {also}")),
    })
}

/// Whether the release `before` may come back, by this deploy's or a
/// person's hand (a stuck outcome's advice): `Err` saying why not. Not
/// when there is none, when the deploy changed what it cannot take back
/// (`rollback`), or when its schema is not this release's or unknown.
fn eligible(before: Option<&Live>, schema: &str, rollback: &Result<(), String>) -> Result<(), String> {
    let Some(before) = before else {
        return Err("there is no release before to go back to (the Worker's first deploy)".into());
    };
    if let Err(cannot) = rollback {
        return Err(format!("it is not rolled back to {}: {cannot} (forward only: fix it forward)", before.release));
    }
    match before.schema.as_deref() {
        Some(s) if s == schema => Ok(()),
        Some(s) => Err(format!("it is not rolled back: its schema ({schema}) is not {}'s ({s}), whose code an object that ran the newer steps refuses (forward only: fix it forward)", before.release)),
        None => Err(format!("it is not rolled back: {} names no schema, so whether its code takes these objects is unknown (forward only: fix it forward)", before.release)),
    }
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
        /// How many reads of the deployments fail (503, a timeout).
        version_fails: u32,
        /// Whether `wrangler deploy` cannot run to its end (its output
        /// lost), after its upload when `uploads_before_failing`.
        deploy_breaks: bool,
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
                version_fails: 0,
                deploy_breaks: false,
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
            if self.version_fails > 0 {
                self.version_fails -= 1;
                anyhow::bail!("the Worker's deployments: 503");
            }
            Ok(self.versions.last().map(|_| self.worker.version.clone()))
        }
        fn deploy(&mut self) -> Result<Wrangled> {
            self.calls.push("deploy".into());
            if self.deploy_breaks {
                if self.uploads_before_failing {
                    self.upload();
                }
                anyhow::bail!("reading wrangler's output: broken pipe");
            }
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
        run(w, &mut Fake::default(), NEW, SCHEMA, Ok(())).unwrap()
    }

    fn count(w: &World, call: &str) -> usize {
        w.calls.iter().filter(|c| c.starts_with(call)).count()
    }

    /// A Worker left live by a deploy that failed after its upload.
    fn live_unfinished() -> World {
        let mut w = World::new(OLD, NEW, Some(SCHEMA));
        w.deploys = vec![Wrangled::Failed("could not finish applying its Durable Object-managed Container application settings".into())];
        w.uploads_before_failing = true;
        w
    }

    #[test]
    fn deployments_that_cannot_be_read_at_once_are_read_again_never_uploaded_again() {
        // review of #605: a 503 reading them after the Worker went live
        let mut w = live_unfinished();
        w.version_fails = 3;
        assert_eq!(deploy(&mut w), Outcome::Deployed);
        assert_eq!(w.ends(), (NEW.into(), NEW.into()));
        assert_eq!((count(&w, "deploy"), count(&w, "version"), count(&w, "finish")), (1, 4, 1));
    }

    #[test]
    fn deployments_never_read_leave_it_stuck_naming_the_release_before() {
        let mut w = live_unfinished();
        w.version_fails = u32::MAX;
        let out = deploy(&mut w);
        assert!(matches!(&out, Outcome::Stuck(why) if why.contains("could not be read") && why.contains(OLD) && why.contains("`wrangler rollback v-old`") && why.contains(NEW)), "{out:?}");
        assert_eq!((count(&w, "deploy"), count(&w, "finish"), count(&w, "rollback")), (1, 0, 0), "nothing more done blind");
        assert!(count(&w, "version") > 1 && count(&w, "version") < 12, "asked again with backoff, bounded: {}", count(&w, "version"));

        // review of #605: its advice is held to the rule a rollback is:
        // never by hand beneath another schema, an unknown one, or over a
        // branch's new Stripe endpoint
        for (schema_before, rollback, said) in [
            (Some("0000000000000000"), Ok(()), "is not"),
            (None, Ok(()), "names no schema"),
            (Some(SCHEMA), Err("its Stripe webhook endpoint was made anew".to_string()), "Stripe"),
        ] {
            let mut w = World::new(OLD, NEW, schema_before);
            w.deploys = vec![Wrangled::Failed("could not finish applying".into())];
            w.uploads_before_failing = true;
            w.version_fails = u32::MAX;
            let out = run(&mut w, &mut Fake::default(), NEW, SCHEMA, rollback).unwrap();
            assert!(
                matches!(&out, Outcome::Stuck(why) if why.contains("do not roll back by hand") && why.contains("forward only") && why.contains(said) && !why.contains("wrangler rollback")),
                "{out:?}"
            );
        }
    }

    #[test]
    fn a_deploy_that_could_not_run_to_its_end_is_read_as_any_failure() {
        // its output lost after its upload: the Worker is live, and finished
        let mut w = live_unfinished();
        w.deploy_breaks = true;
        assert_eq!(deploy(&mut w), Outcome::Deployed);
        assert_eq!((count(&w, "deploy"), count(&w, "finish v-new")), (1, 1));
        // and before it: nothing changed
        let mut w = World::new(OLD, NEW, Some(SCHEMA));
        w.deploy_breaks = true;
        let out = deploy(&mut w);
        assert!(matches!(&out, Outcome::Unchanged(why) if why.contains("did not run to its end")), "{out:?}");
        assert_eq!(w.ends(), (OLD.into(), OLD.into()));
    }

    #[test]
    fn a_deploy_that_changed_what_the_release_before_cannot_take_back_is_not_rolled_back() {
        // review of #605: a branch's Stripe endpoint made anew, its old one deleted
        let mut w = World::new(OLD, NEW, Some(SCHEMA));
        w.lag = None;
        let out = run(&mut w, &mut Fake::default(), NEW, SCHEMA, Err("its Stripe webhook endpoint was made anew".into())).unwrap();
        assert!(matches!(&out, Outcome::Stuck(why) if why.contains("not rolled back") && why.contains("Stripe")), "{out:?}");
        assert_eq!(count(&w, "rollback"), 0);
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
        // a minute: 12 probes 5 s apart
        let mut w = World::new(OLD, NEW, Some(SCHEMA));
        w.lag = Some(12);
        assert_eq!(deploy(&mut w), Outcome::Deployed);
        assert_eq!(w.ends(), (NEW.into(), NEW.into()));
        assert_eq!(count(&w, "probe"), 13);
    }

    /// Cloudflare's deferred five minutes (60 probes 5 s apart), should it
    /// hold a busy object on its old code again whatever `immediate` asks:
    /// past the limit, so the deploy is rolled back and says so, never
    /// green over minutes of refusals.
    #[test]
    fn objects_held_for_cloudflares_five_minutes_are_rolled_back() {
        let mut w = World::new(OLD, NEW, Some(SCHEMA));
        w.lag = Some(60);
        let out = deploy(&mut w);
        assert!(matches!(&out, Outcome::RolledBack { to, .. } if to == OLD), "{out:?}");
        assert_eq!(w.ends(), (OLD.into(), OLD.into()));
    }

    #[test]
    fn objects_that_never_take_the_new_release_are_rolled_back_all_old() {
        let mut w = World::new(OLD, NEW, Some(SCHEMA));
        w.lag = None;
        let out = deploy(&mut w);
        assert!(matches!(&out, Outcome::RolledBack { to, why } if to == OLD && why.contains("Registry runs 0e9bc7b36e5c") && why.contains("1m30s")), "{out:?}");
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
