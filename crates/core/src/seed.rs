//! A fragment's making, as one machine (FIN-395): from a create's claim
//! on its name to its template landed, installed from its own live commit
//! (cell fragment.rs `create`, publish.rs `seed`).
//!
//! Its stage is read from the rows its making writes, and from nothing
//! else (`stage`): `claimed_at`, `created_at`, `template_pending`, `repo`,
//! the template's commit (`file_commits`, keyed by its life), `pin_live`,
//! `live_read_at` and `blessed`. Nothing of it lives in memory, so a
//! restart or an eviction finds it where its last step left it, and an
//! older release reading the same rows (a rollback) agrees: a stage stored
//! of its own would be a second truth.
//!
//! The stages, in order:
//!
//! - **Free**: no life, and no create holds its name.
//! - **Claimed**: a create holds its name while its making awaits (its
//!   maker's agent, its repo, its key, its template's manifest), until the
//!   claim lapses (`CLAIM_TTL_MS`: a create that crashed mid-flight).
//! - **Pending**: made (its owner and its agent members, its facts), its
//!   template still to land, at one of its seed's steps (`Step`): made, its
//!   repo made, its template committed, deployed, installed from live.
//!   Meanwhile a blessed template whose create answers first runs from the
//!   release (`release_installed`: a chat's).
//! - **Landed**: its template is in, or it had none.
//! - **Ended**: the life a step began in is over (deleted, and maybe made
//!   again): what that step does next is nothing.
//!
//! `step` takes a stage and what happened (`Event`) to the next stage and
//! what to do (`Effect`). The cell carries the effects out, then reads the
//! rows again, which must say the stage `step` said. Every pending stage
//! keeps the fragment busy, so its alarm is always due within the poll
//! interval (plane.rs `busy`), and each pass steps it on from the stage the
//! rows say: what a restart cut short, the next pass does.

/// How long a create in progress holds its name: past it, a claim whose
/// create crashed mid-flight lapses, and the name is free again.
pub const CLAIM_TTL_MS: i64 = 120_000;

/// The most steps one seed takes (publish.rs `seed`): its repo, its
/// commit, its deploy (its install with it), and its landing.
pub const SEED_STEPS_MAX: u32 = 4;

/// The rows a fragment's stage is read from, in one statement (cell
/// publish.rs `seed_rows`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rows {
    pub now_ms: i64,
    /// When a create claimed its name (`claimed_at`).
    pub claimed_at: Option<i64>,
    /// Its life (`created_at`): a fragment exists once it has one.
    pub created_at: Option<i64>,
    /// Its template is still to land (`template_pending`).
    pub template_pending: bool,
    /// Its repo is made (`repo`).
    pub repo: bool,
    /// Its template is main's commit (`file_commits`, `template:<life>`).
    pub committed: bool,
    /// Live's pin (`pin_live`).
    pub live: Option<String>,
    /// The live commit its code was last installed from (`live_read_at`).
    pub live_read_at: Option<String>,
    /// A blessed template's install stands (`blessed`): the release's
    /// manifest and code run it.
    pub blessed: bool,
}

/// How far a fragment's making has got (the module's doc).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    Free,
    Claimed { at: i64 },
    Pending { step: Step, release_installed: bool },
    Landed,
    Ended,
}

/// A pending template's seed, by the last step it took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Its facts are written; its repo is not made yet (a template that
    /// seeds after its create's answer, publish.rs `SEEDS_LATER`).
    Made,
    /// Its repo is made, its template not committed yet.
    RepoMade,
    /// Its template is main's commit, not deployed yet.
    Committed,
    /// Live is at `live`, and what follows it is not installed from it
    /// yet: deployed is not installed.
    Deployed { live: String },
    /// Installed from `live`: all that is left is to say it landed.
    Installed { live: String },
}

impl Step {
    /// Its place in the order the seed takes the steps.
    fn rank(&self) -> u8 {
        match self {
            Step::Made => 0,
            Step::RepoMade => 1,
            Step::Committed => 2,
            Step::Deployed { .. } => 3,
            Step::Installed { .. } => 4,
        }
    }
}

/// How a create makes its fragment, by its template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Making {
    /// No template: its repo is made before its facts are written, and
    /// nothing is to land.
    Plain,
    /// A template that lands before its create answers: its repo made
    /// before its facts, its seed in its create.
    SeedsNow,
    /// A blessed template its create answers before (publish.rs
    /// `SEEDS_LATER`): its repo is not made yet, it runs from the release,
    /// and its seed comes from its alarm, due at once.
    SeedsLater,
}

impl Making {
    /// Whether its repo is made before its facts are written (a create
    /// that fails to make it makes nothing).
    pub fn repo_first(self) -> bool {
        match self {
            Making::Plain | Making::SeedsNow => true,
            Making::SeedsLater => false,
        }
    }
}

/// What happened to a fragment's making.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A create asks for its name, at `at`.
    Claim { at: i64 },
    /// Its making is written, in one turn: its owner a member, its agent
    /// (`agent`: one is named) an editor, its facts, its pending template.
    Made { making: Making, agent: bool },
    /// The release's install of its template (`InstallFromRelease`) stands.
    ReleaseInstalled,
    /// The release's install failed: it waits for its seed's.
    ReleaseFailed,
    RepoMade,
    Committed,
    Deployed { live: String },
    /// What follows live is installed from `live`.
    Installed { live: String },
    /// Its pending template is cleared (`Land`).
    Landed,
    /// Its life ended.
    Delete,
    /// Its alarm's pass, or a writer that needs its template in first.
    Alarm,
    /// A new activation of its object: whatever the last one held in memory
    /// is gone.
    Activation,
}

/// What the cell does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Installs its template from the release, as its seed will commit it
    /// (plane.rs `install_pending`), under the plane's lock.
    InstallFromRelease,
    /// Makes its repo, or finds the one its owner's name made before.
    EnsureRepo,
    /// Commits its template to main, keyed by its life (so once).
    Commit,
    /// Moves live to main's tip, and installs from it.
    Deploy,
    /// Clears its pending template: it landed.
    Land,
    /// Sends its members' lists their rows.
    FlushIndex,
    /// Tells its agents' computers they joined (runs_on.rs `flush_joined`).
    TellJoined,
    /// Arms its alarm: `soon`, at once (its seed after its create's
    /// answer); else for its earliest due work.
    Arm { soon: bool },
}

impl Effect {
    /// Whether it is one of the seed's steps: taken one at a time, under
    /// its lock (publish.rs `seed`).
    pub fn is_seed_step(self) -> bool {
        matches!(self, Effect::EnsureRepo | Effect::Commit | Effect::Deploy | Effect::Land)
    }
}

/// Why `step` refused an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// Its name is a life's, or a create's whose claim holds.
    Taken,
    /// There is no life to act on.
    NoLife,
    /// `event` cannot happen at `stage`: a step before the one it follows
    /// (a commit before its repo, an install before its deploy), or a
    /// making written twice.
    OutOfOrder { stage: Stage, event: Event },
}

/// Rows that no making leaves: their fragment's stage cannot be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Contradiction(pub &'static str);

impl std::fmt::Display for Contradiction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// The stage its rows say.
pub fn stage(rows: &Rows) -> Result<Stage, Contradiction> {
    let Some(_life) = rows.created_at else {
        // a claim from the future (a clock that stepped back) holds, as a
        // fresh one does, until it lapses
        return Ok(match rows.claimed_at {
            Some(at) if rows.now_ms - at < CLAIM_TTL_MS => Stage::Claimed { at },
            _ => Stage::Free,
        });
    };
    if !rows.template_pending {
        return Ok(Stage::Landed);
    }
    // a commit and a pin are a repo's: the seed makes it first
    if !rows.repo && rows.committed {
        return Err(Contradiction("its template is committed, but it has no repo"));
    }
    if !rows.repo && rows.live.is_some() {
        return Err(Contradiction("live is pinned, but it has no repo"));
    }
    let step = match &rows.live {
        Some(live) if rows.live_read_at.as_deref() == Some(live.as_str()) => Step::Installed { live: live.clone() },
        Some(live) => Step::Deployed { live: live.clone() },
        None if rows.committed => Step::Committed,
        None if rows.repo => Step::RepoMade,
        None => Step::Made,
    };
    Ok(Stage::Pending { step, release_installed: rows.blessed })
}

/// The stage its rows say, for a step that began in `life`: `Ended` once
/// that life is not the fragment's (a delete, and the name made again,
/// between the step's awaits).
pub fn stage_in(rows: &Rows, life: i64) -> Result<Stage, Contradiction> {
    if rows.created_at == Some(life) {
        stage(rows)
    } else {
        Ok(Stage::Ended)
    }
}

impl Stage {
    /// Its template is still to land: a writer of its repo lands it first
    /// (plane.rs `repo_ready`; its repo alone is not enough, a commit or a
    /// deploy after it may have failed), its pins move by its seed alone
    /// (`ensure_pins` asks code.storage nothing), a platform deploy
    /// installs it from the release again (`blessed_current`), and its
    /// alarm is due within the poll interval (`busy`).
    pub fn waits_template(&self) -> bool {
        matches!(self, Stage::Pending { .. })
    }

    /// The release's install runs it until its template lands: its site is
    /// the release's, with no commit of its own (serve.rs, ref `""`),
    /// whether or not its repo is made, through a code.storage outage.
    pub fn runs_release(&self) -> bool {
        matches!(self, Stage::Pending { release_installed: true, .. })
    }

    /// Whether `self` is at the step `other` is, whatever runs meanwhile:
    /// `release_installed` is an install's to say (the create's from the
    /// release, a live one of a blessed template), never a seed step's, so
    /// a seed reading its rows back takes theirs.
    pub fn same_step(&self, other: &Stage) -> bool {
        match (self, other) {
            (Stage::Pending { step: mine, .. }, Stage::Pending { step: theirs, .. }) => mine == theirs,
            (mine, theirs) => mine == theirs,
        }
    }

    /// Its agents' joined notices wait: its template is still to land, and
    /// nothing is installed for them to follow. A computer told now would
    /// find no channel, take the notice as done, and look again only at
    /// its rediscovery. They stay in the outbox (a restart keeps them) and
    /// go once this ends.
    pub fn joins_wait(&self) -> bool {
        match self {
            Stage::Pending { step: Step::Installed { .. }, .. } => false,
            Stage::Pending { release_installed, .. } => !release_installed,
            Stage::Free | Stage::Claimed { .. } | Stage::Landed | Stage::Ended => false,
        }
    }
}

/// The next stage after `event` at `stage`, and what the cell does.
pub fn step(stage: &Stage, event: &Event) -> Result<(Stage, Vec<Effect>), Refused> {
    let out_of_order = || Refused::OutOfOrder { stage: stage.clone(), event: event.clone() };
    let next = match (stage, event) {
        // nothing of a making is in memory: an activation finds it where
        // the rows say
        (_, Event::Activation) => (stage.clone(), vec![]),
        // what a step of a life that ended does next is nothing
        (Stage::Ended, _) => (Stage::Ended, vec![]),
        (Stage::Free, Event::Claim { at }) => (Stage::Claimed { at: *at }, vec![]),
        (_, Event::Claim { .. }) => return Err(Refused::Taken),
        (Stage::Claimed { .. }, Event::Made { making, agent }) => made(*making, *agent),
        (Stage::Free | Stage::Claimed { .. }, Event::Delete) => return Err(Refused::NoLife),
        (_, Event::Delete) => (Stage::Ended, vec![]),
        // a pass finds it at a step, and takes the next
        (Stage::Pending { step, .. }, Event::Alarm) => (stage.clone(), vec![next_step(step)]),
        (Stage::Free | Stage::Claimed { .. } | Stage::Landed, Event::Alarm) => (stage.clone(), vec![]),
        (Stage::Pending { step, release_installed }, event) => pending(step, *release_installed, event).ok_or_else(out_of_order)?,
        // a step's outcome after it landed (a second seed, its install
        // again) changes nothing
        (
            Stage::Landed,
            Event::ReleaseInstalled
            | Event::ReleaseFailed
            | Event::RepoMade
            | Event::Committed
            | Event::Deployed { .. }
            | Event::Installed { .. }
            | Event::Landed,
        ) => (Stage::Landed, vec![]),
        (Stage::Free | Stage::Claimed { .. } | Stage::Landed, _) => return Err(out_of_order()),
    };
    // what an effect of a step tells, it tells once nothing waits for it
    assert!(!next.1.contains(&Effect::TellJoined) || !next.0.joins_wait(), "agents are told only once they have something to follow");
    assert!(next.1.iter().filter(|e| e.is_seed_step()).count() <= 1, "a step leads to one seed step at most");
    Ok(next)
}

/// Its making written, by how it is made: the stage it is at, and the
/// create's effects, in order. A list row goes only from here on, its
/// owner's and its agent's rows both written (#1041).
fn made(making: Making, agent: bool) -> (Stage, Vec<Effect>) {
    match making {
        Making::Plain => {
            let mut effects = vec![Effect::FlushIndex];
            if agent {
                effects.push(Effect::TellJoined);
            }
            effects.push(Effect::Arm { soon: false });
            (Stage::Landed, effects)
        }
        // its seed from its commit on, in its create; its agents are told
        // by the seed that lands it, whichever
        Making::SeedsNow => (Stage::Pending { step: Step::RepoMade, release_installed: false }, vec![Effect::FlushIndex, Effect::Commit, Effect::Arm { soon: false }]),
        // answered once it runs from the release; its seed is its alarm's,
        // due at once, which a restart keeps
        Making::SeedsLater => (
            Stage::Pending { step: Step::Made, release_installed: false },
            vec![Effect::InstallFromRelease, Effect::FlushIndex, Effect::Arm { soon: true }],
        ),
    }
}

/// The seed step a pending template takes next.
fn next_step(step: &Step) -> Effect {
    match step {
        Step::Made => Effect::EnsureRepo,
        Step::RepoMade => Effect::Commit,
        Step::Committed => Effect::Deploy,
        // deployed is not installed: the deploy again, from the same
        // commit (live is main's tip), installs it
        Step::Deployed { .. } => Effect::Deploy,
        Step::Installed { .. } => Effect::Land,
    }
}

/// `event` at a pending `step`; `None` when it cannot happen there.
fn pending(step: &Step, release_installed: bool, event: &Event) -> Option<(Stage, Vec<Effect>)> {
    let stay = |release_installed: bool| (Stage::Pending { step: step.clone(), release_installed }, vec![]);
    match event {
        // the release runs it until it deploys; over a live it installs nothing
        Event::ReleaseInstalled => Some(stay(release_installed || matches!(step, Step::Made | Step::RepoMade | Step::Committed))),
        Event::ReleaseFailed => Some(stay(release_installed)),
        Event::RepoMade => forward(step, Step::RepoMade, release_installed),
        Event::Committed => forward(step, Step::Committed, release_installed),
        Event::Deployed { live } => forward(step, Step::Deployed { live: live.clone() }, release_installed),
        Event::Installed { live } => match step {
            Step::Deployed { live: deployed } | Step::Installed { live: deployed } if deployed == live => forward(step, Step::Installed { live: live.clone() }, release_installed),
            // an install of a live it is not at changes nothing
            Step::Deployed { .. } | Step::Installed { .. } => Some(stay(release_installed)),
            Step::Made | Step::RepoMade | Step::Committed => None,
        },
        Event::Landed => match step {
            Step::Installed { .. } => Some((Stage::Landed, vec![Effect::TellJoined])),
            Step::Made | Step::RepoMade | Step::Committed | Step::Deployed { .. } => None,
        },
        Event::Claim { .. } | Event::Made { .. } | Event::Delete | Event::Alarm | Event::Activation => None,
    }
}

/// From `from` to `to`: the step after it advances, one it took already
/// changes nothing (a replay), and one that skips a step cannot happen.
fn forward(from: &Step, to: Step, release_installed: bool) -> Option<(Stage, Vec<Effect>)> {
    let (from_rank, to_rank) = (from.rank(), to.rank());
    if to_rank <= from_rank {
        return Some((Stage::Pending { step: from.clone(), release_installed }, vec![]));
    }
    if to_rank > from_rank + 1 {
        return None;
    }
    let effects = match &to {
        Step::RepoMade => vec![Effect::Commit],
        Step::Committed => vec![Effect::Deploy],
        // its install comes with the deploy's pin, or with the next deploy
        Step::Deployed { .. } => vec![],
        Step::Installed { .. } => vec![Effect::Land],
        Step::Made => unreachable!("no step comes before Made"),
    };
    Some((Stage::Pending { step: to, release_installed }, effects))
}

#[cfg(test)]
mod tests {
    //! The machine's table (engineering style §6): every forward step;
    //! the steps that cannot happen; each step replayed; and a restart
    //! after each step, the rows alone carrying it on to landed.
    //!
    //! `World` stands in for the cell: rows, and the effects as the cell
    //! carries them out on those rows, code.storage up or down.

    use super::*;

    const NOW: i64 = 1_000_000;
    const LIFE: i64 = 999_000;
    const LIVE: &str = "a1b2c3";

    fn pending(step: Step, release_installed: bool) -> Stage {
        Stage::Pending { step, release_installed }
    }

    fn live(sha: &str) -> String {
        sha.to_string()
    }

    /// The rows of a fragment just made, by how.
    fn made_rows(making: Making) -> Rows {
        Rows {
            now_ms: NOW,
            claimed_at: Some(LIFE),
            created_at: Some(LIFE),
            template_pending: making != Making::Plain,
            repo: making.repo_first(),
            ..Rows::default()
        }
    }

    /// The cell, as far as the machine sees it: its rows, and whether
    /// code.storage answers.
    struct World {
        rows: Rows,
        code_storage_up: bool,
        /// Its deploy installs what follows live (else it is left to the poll).
        installs: bool,
    }

    impl World {
        fn new(rows: Rows) -> World {
            World { rows, code_storage_up: true, installs: true }
        }

        fn stage(&self) -> Stage {
            stage(&self.rows).expect("a making's rows say its stage")
        }

        /// Carries out a seed step, as publish.rs does: the events it
        /// leads to, none when code.storage fails it.
        fn carry(&mut self, effect: Effect) -> Vec<Event> {
            match effect {
                Effect::InstallFromRelease => {
                    // the release, not code.storage
                    self.rows.blessed = true;
                    vec![Event::ReleaseInstalled]
                }
                Effect::EnsureRepo if self.code_storage_up => {
                    self.rows.repo = true;
                    vec![Event::RepoMade]
                }
                Effect::Commit if self.code_storage_up => {
                    self.rows.committed = true;
                    vec![Event::Committed]
                }
                Effect::Deploy if self.code_storage_up => {
                    self.rows.live = Some(live(LIVE));
                    let mut events = vec![Event::Deployed { live: live(LIVE) }];
                    if self.installs {
                        self.rows.live_read_at = Some(live(LIVE));
                        self.rows.blessed = true;
                        events.push(Event::Installed { live: live(LIVE) });
                    }
                    events
                }
                Effect::Land => {
                    self.rows.template_pending = false;
                    vec![Event::Landed]
                }
                Effect::EnsureRepo | Effect::Commit | Effect::Deploy => vec![],
                Effect::FlushIndex | Effect::TellJoined | Effect::Arm { .. } => vec![],
            }
        }

        /// One seed, as publish.rs `seed` runs it: from the stage the rows
        /// say, each step carried out, its events stepped, and the rows
        /// read back agreeing. Answers the effects left for after it.
        fn seed(&mut self) -> Vec<Effect> {
            let mut stage = self.stage();
            let (_, mut queue) = step(&stage, &Event::Alarm).expect("an alarm is never refused");
            let mut after = vec![];
            let mut steps = 0;
            while !queue.is_empty() {
                let effect = queue.remove(0);
                if !effect.is_seed_step() {
                    after.push(effect);
                    continue;
                }
                steps += 1;
                assert!(steps <= SEED_STEPS_MAX, "a seed takes at most {SEED_STEPS_MAX} steps");
                let events = self.carry(effect);
                if events.is_empty() {
                    break;
                }
                for event in &events {
                    let (next, more) = step(&stage, event).expect("a step's own outcome follows it");
                    stage = next;
                    queue.extend(more);
                }
                let read = self.stage();
                assert!(read.same_step(&stage), "the rows say the step the machine said, after {effect:?}: {read:?}, not {stage:?}");
                stage = read;
            }
            after
        }
    }

    /// Valid: every forward step, one event at a time, from a free name
    /// to landed, and the effects each asks for.
    #[test]
    fn every_forward_step() {
        let (s, e) = step(&Stage::Free, &Event::Claim { at: NOW }).unwrap();
        assert_eq!((s.clone(), e), (Stage::Claimed { at: NOW }, vec![]));
        let (s, e) = step(&s, &Event::Made { making: Making::SeedsLater, agent: true }).unwrap();
        assert_eq!(s, pending(Step::Made, false));
        assert_eq!(e, vec![Effect::InstallFromRelease, Effect::FlushIndex, Effect::Arm { soon: true }]);
        let (s, e) = step(&s, &Event::ReleaseInstalled).unwrap();
        assert_eq!((s.clone(), e), (pending(Step::Made, true), vec![]));
        assert_eq!(step(&s, &Event::Alarm).unwrap().1, vec![Effect::EnsureRepo]);
        let (s, e) = step(&s, &Event::RepoMade).unwrap();
        assert_eq!((s.clone(), e), (pending(Step::RepoMade, true), vec![Effect::Commit]));
        let (s, e) = step(&s, &Event::Committed).unwrap();
        assert_eq!((s.clone(), e), (pending(Step::Committed, true), vec![Effect::Deploy]));
        let (s, e) = step(&s, &Event::Deployed { live: live(LIVE) }).unwrap();
        assert_eq!((s.clone(), e), (pending(Step::Deployed { live: live(LIVE) }, true), vec![]));
        let (s, e) = step(&s, &Event::Installed { live: live(LIVE) }).unwrap();
        assert_eq!((s.clone(), e), (pending(Step::Installed { live: live(LIVE) }, true), vec![Effect::Land]));
        let (s, e) = step(&s, &Event::Landed).unwrap();
        assert_eq!((s.clone(), e), (Stage::Landed, vec![Effect::TellJoined]));
        assert_eq!(step(&s, &Event::Alarm).unwrap(), (Stage::Landed, vec![]));
        assert_eq!(step(&s, &Event::Delete).unwrap(), (Stage::Ended, vec![]));
    }

    /// Valid: each way of making, its create's effects in order; the
    /// stage the rows say once its making is written is the one `step`
    /// said.
    #[test]
    fn each_making() {
        let claimed = Stage::Claimed { at: NOW };
        for (making, agent, at, effects) in [
            (Making::Plain, false, Stage::Landed, vec![Effect::FlushIndex, Effect::Arm { soon: false }]),
            (Making::Plain, true, Stage::Landed, vec![Effect::FlushIndex, Effect::TellJoined, Effect::Arm { soon: false }]),
            (Making::SeedsNow, true, pending(Step::RepoMade, false), vec![Effect::FlushIndex, Effect::Commit, Effect::Arm { soon: false }]),
            (
                Making::SeedsLater,
                true,
                pending(Step::Made, false),
                vec![Effect::InstallFromRelease, Effect::FlushIndex, Effect::Arm { soon: true }],
            ),
        ] {
            assert_eq!(step(&claimed, &Event::Made { making, agent }).unwrap(), (at.clone(), effects), "{making:?} agent {agent}");
            assert_eq!(stage(&made_rows(making)).unwrap(), at, "{making:?}: the rows of its making say it");
        }
        // with or without its agent, a template's create tells no one
        // itself: the seed that lands it, or its alarm, does
        for making in [Making::SeedsNow, Making::SeedsLater] {
            assert_eq!(step(&claimed, &Event::Made { making, agent: true }).unwrap(), step(&claimed, &Event::Made { making, agent: false }).unwrap());
        }
    }

    /// Valid: the stage the rows say, row by row.
    #[test]
    fn stage_from_rows() {
        let mut rows = Rows { now_ms: NOW, ..Rows::default() };
        assert_eq!(stage(&rows), Ok(Stage::Free));
        rows.claimed_at = Some(NOW - CLAIM_TTL_MS + 1);
        assert_eq!(stage(&rows), Ok(Stage::Claimed { at: NOW - CLAIM_TTL_MS + 1 }));
        // a create that crashed mid-flight: its claim lapses
        rows.claimed_at = Some(NOW - CLAIM_TTL_MS);
        assert_eq!(stage(&rows), Ok(Stage::Free));
        rows.claimed_at = Some(NOW + 5);
        assert_eq!(stage(&rows), Ok(Stage::Claimed { at: NOW + 5 }), "a claim from a clock that stepped back holds");
        rows.created_at = Some(LIFE);
        assert_eq!(stage(&rows), Ok(Stage::Landed), "a life with nothing pending");
        rows.template_pending = true;
        assert_eq!(stage(&rows), Ok(pending(Step::Made, false)));
        rows.blessed = true;
        assert_eq!(stage(&rows), Ok(pending(Step::Made, true)));
        rows.repo = true;
        assert_eq!(stage(&rows), Ok(pending(Step::RepoMade, true)));
        rows.committed = true;
        assert_eq!(stage(&rows), Ok(pending(Step::Committed, true)));
        rows.live = Some(live(LIVE));
        rows.live_read_at = Some(live("older"));
        assert_eq!(stage(&rows), Ok(pending(Step::Deployed { live: live(LIVE) }, true)));
        // deployed, its commit's key trimmed since (`trim_writes`): still deployed
        rows.committed = false;
        assert_eq!(stage(&rows), Ok(pending(Step::Deployed { live: live(LIVE) }, true)));
        rows.live_read_at = Some(live(LIVE));
        assert_eq!(stage(&rows), Ok(pending(Step::Installed { live: live(LIVE) }, true)));
        rows.template_pending = false;
        assert_eq!(stage(&rows), Ok(Stage::Landed));
        assert_eq!(stage_in(&rows, LIFE), Ok(Stage::Landed));
        assert_eq!(stage_in(&rows, LIFE - 1), Ok(Stage::Ended), "another life's step finds its own ended");
        assert_eq!(stage_in(&Rows { now_ms: NOW, ..Rows::default() }, LIFE), Ok(Stage::Ended), "a life deleted, its name free");
    }

    /// Invalid: rows no making leaves.
    #[test]
    fn contradictions() {
        let base = Rows { now_ms: NOW, created_at: Some(LIFE), template_pending: true, ..Rows::default() };
        assert!(stage(&Rows { committed: true, ..base.clone() }).is_err(), "a commit with no repo");
        assert!(stage(&Rows { live: Some(live(LIVE)), ..base.clone() }).is_err(), "live with no repo");
        // no life: whatever else is there is not a fragment's
        assert_eq!(stage(&Rows { created_at: None, repo: true, ..base }), Ok(Stage::Free));
    }

    /// Invalid: a step before the one it follows, and the making written
    /// twice or before its claim.
    #[test]
    fn out_of_order() {
        let refused = |s: &Stage, e: Event| matches!(step(s, &e), Err(Refused::OutOfOrder { .. }));
        let made = pending(Step::Made, true);
        // a commit before its repo
        assert!(refused(&made, Event::Committed));
        assert!(refused(&made, Event::Deployed { live: live(LIVE) }));
        assert!(refused(&pending(Step::RepoMade, false), Event::Deployed { live: live(LIVE) }));
        // installed from a live it never deployed, and landed before it
        // was installed
        assert!(refused(&pending(Step::Committed, true), Event::Installed { live: live(LIVE) }));
        for s in [Step::Made, Step::RepoMade, Step::Committed, Step::Deployed { live: live(LIVE) }] {
            assert!(refused(&pending(s, true), Event::Landed));
        }
        // a step's outcome before its making is written
        for e in [Event::ReleaseInstalled, Event::RepoMade, Event::Committed, Event::Installed { live: live(LIVE) }, Event::Landed] {
            assert!(refused(&Stage::Free, e.clone()), "{e:?}");
            assert!(refused(&Stage::Claimed { at: NOW }, e.clone()), "{e:?}");
        }
        // the making, twice or unclaimed
        let made_event = Event::Made { making: Making::SeedsLater, agent: false };
        for s in [Stage::Free, made.clone(), Stage::Landed] {
            assert!(refused(&s, made_event.clone()), "{s:?}");
        }
        // a delete of no life
        assert_eq!(step(&Stage::Free, &Event::Delete), Err(Refused::NoLife));
        assert_eq!(step(&Stage::Claimed { at: NOW }, &Event::Delete), Err(Refused::NoLife));
    }

    /// Invalid (#1041): no list row before its making is written (its
    /// owner's and its agent's rows with it), and its agents told only
    /// once they have something to follow.
    #[test]
    fn nothing_announced_early() {
        let events = [
            Event::Claim { at: NOW },
            Event::ReleaseInstalled,
            Event::ReleaseFailed,
            Event::RepoMade,
            Event::Committed,
            Event::Deployed { live: live(LIVE) },
            Event::Installed { live: live(LIVE) },
            Event::Landed,
            Event::Alarm,
            Event::Activation,
        ];
        for s in [Stage::Free, Stage::Claimed { at: NOW }] {
            for e in &events {
                if let Ok((_, effects)) = step(&s, e) {
                    assert!(!effects.contains(&Effect::FlushIndex), "{s:?} {e:?}: a list row before its making");
                    assert!(!effects.contains(&Effect::TellJoined), "{s:?} {e:?}: told before its making");
                }
            }
        }
        // every pending stage with nothing installed: its agents wait, and
        // no step from it tells them
        for s in [Step::Made, Step::RepoMade, Step::Committed, Step::Deployed { live: live(LIVE) }] {
            let s = pending(s, false);
            assert!(s.joins_wait(), "{s:?}");
            for e in &events {
                if let Ok((next, effects)) = step(&s, e) {
                    assert!(!effects.contains(&Effect::TellJoined) || !next.joins_wait(), "{s:?} {e:?}");
                }
            }
        }
        // a chat runs its template from the release: its agents follow it
        // there before its first commit lands
        assert!(!pending(Step::Made, true).joins_wait());
        assert!(!pending(Step::Installed { live: live(LIVE) }, false).joins_wait());
        assert!(!Stage::Landed.joins_wait());
    }

    /// Invalid (`repo_ready`): a writer finds its template still to land at
    /// every pending step, its repo made or not, so it lands it first and
    /// never writes under it.
    #[test]
    fn writers_wait_for_the_template() {
        for s in [Step::Made, Step::RepoMade, Step::Committed, Step::Deployed { live: live(LIVE) }, Step::Installed { live: live(LIVE) }] {
            for release_installed in [false, true] {
                assert!(pending(s.clone(), release_installed).waits_template(), "{s:?}");
            }
        }
        for s in [Stage::Free, Stage::Claimed { at: NOW }, Stage::Landed, Stage::Ended] {
            assert!(!s.waits_template(), "{s:?}");
        }
    }

    /// Replay: a second seed, its install again, and a create of the same
    /// name again change nothing.
    #[test]
    fn replays() {
        // a second seed, run after the first landed it (the 412 race
        // publish.rs names): nothing to do
        assert_eq!(step(&Stage::Landed, &Event::Alarm), Ok((Stage::Landed, vec![])));
        let mut world = World::new(made_rows(Making::SeedsLater));
        assert_eq!(world.seed(), vec![Effect::TellJoined]);
        let rows = world.rows.clone();
        assert_eq!(world.seed(), vec![]);
        assert_eq!(world.rows, rows, "the second seed writes nothing");
        // each outcome again, at the stage it led to
        let installed = pending(Step::Installed { live: live(LIVE) }, true);
        assert_eq!(step(&installed, &Event::Installed { live: live(LIVE) }), Ok((installed.clone(), vec![])));
        assert_eq!(step(&installed, &Event::Deployed { live: live(LIVE) }), Ok((installed.clone(), vec![])));
        assert_eq!(step(&pending(Step::Committed, false), &Event::RepoMade), Ok((pending(Step::Committed, false), vec![])));
        assert_eq!(step(&pending(Step::RepoMade, false), &Event::RepoMade), Ok((pending(Step::RepoMade, false), vec![])));
        for e in [Event::Installed { live: live(LIVE) }, Event::Landed, Event::ReleaseInstalled, Event::Committed] {
            assert_eq!(step(&Stage::Landed, &e), Ok((Stage::Landed, vec![])), "{e:?}");
        }
        // an install over live installs nothing of the release's
        let deployed = pending(Step::Deployed { live: live(LIVE) }, false);
        assert_eq!(step(&deployed, &Event::ReleaseInstalled), Ok((deployed.clone(), vec![])));
        // a create retried under the same name, as it is made and after
        let retry = Event::Claim { at: NOW + 1 };
        for s in [Stage::Claimed { at: NOW }, pending(Step::Made, true), installed, Stage::Landed] {
            assert_eq!(step(&s, &retry), Err(Refused::Taken), "{s:?}");
        }
        // its claim lapsed with no life: free again
        let lapsed = Rows { now_ms: NOW + CLAIM_TTL_MS, claimed_at: Some(NOW), ..Rows::default() };
        assert_eq!(step(&stage(&lapsed).unwrap(), &retry).map(|(s, _)| s), Ok(Stage::Claimed { at: NOW + 1 }));
    }

    /// Restart: whatever memory held is dropped after each step (a fresh
    /// activation); the rows alone say where it is, an alarm is due, and
    /// alarm passes carry it on to landed. Each way of making, with
    /// code.storage down for its first passes, and a deploy whose install
    /// is left to the poll.
    #[test]
    fn restart_after_each_step() {
        for making in [Making::SeedsNow, Making::SeedsLater] {
            for outage_passes in 0..3 {
                for installs_on_first_deploy in [false, true] {
                    let mut world = World::new(made_rows(making));
                    if making == Making::SeedsLater {
                        world.carry(Effect::InstallFromRelease);
                    }
                    let mut told = 0;
                    for pass in 0..12 {
                        // a new activation: nothing but the rows
                        let at = world.stage();
                        assert_eq!(step(&at, &Event::Activation), Ok((at.clone(), vec![])));
                        if at == Stage::Landed {
                            break;
                        }
                        assert!(at.waits_template(), "a pending template keeps its alarm due (`busy`): {at:?}");
                        world.code_storage_up = pass >= outage_passes;
                        world.installs = installs_on_first_deploy || pass > outage_passes;
                        told += world.seed().iter().filter(|e| **e == Effect::TellJoined).count();
                    }
                    assert_eq!(world.stage(), Stage::Landed, "{making:?}, {outage_passes} passes down, installs {installs_on_first_deploy}");
                    assert_eq!(told, 1, "its agents are told once, by the seed that lands it");
                }
            }
        }
    }

    /// Restart: a step that began in a life that ended (deleted, and the
    /// name made again) changes nothing in the next.
    #[test]
    fn a_step_of_an_older_life() {
        let rows = Rows { now_ms: NOW, created_at: Some(LIFE + 1), template_pending: true, ..Rows::default() };
        let older = stage_in(&rows, LIFE).unwrap();
        assert_eq!(older, Stage::Ended);
        for e in [Event::Installed { live: live(LIVE) }, Event::RepoMade, Event::Committed, Event::Landed, Event::Alarm, Event::Delete] {
            assert_eq!(step(&older, &e), Ok((Stage::Ended, vec![])), "{e:?}");
        }
        // and the next life's own stage is its own
        assert_eq!(stage_in(&rows, LIFE + 1), Ok(pending(Step::Made, false)));
    }

    /// Restart (#1042, finding 3): a chat whose repo is made and whose
    /// first commit failed keeps serving from the release while
    /// code.storage is down, asking it nothing for its pins.
    #[test]
    fn a_pending_page_serves_through_an_outage() {
        let mut world = World::new(made_rows(Making::SeedsLater));
        world.carry(Effect::InstallFromRelease);
        world.carry(Effect::EnsureRepo);
        world.code_storage_up = false;
        for _ in 0..3 {
            world.seed();
            let at = world.stage();
            assert_eq!(at, pending(Step::RepoMade, true));
            assert!(at.runs_release() && at.waits_template() && !at.joins_wait(), "{at:?}");
        }
        // the release's install that failed leaves it waiting for its seed's
        let failed = World::new(made_rows(Making::SeedsLater)).stage();
        assert_eq!(step(&failed, &Event::ReleaseFailed), Ok((failed.clone(), vec![])));
        assert!(!failed.runs_release() && failed.joins_wait());
    }

    /// The bound: a seed takes each of its steps once.
    #[test]
    fn a_seed_is_bounded() {
        let mut world = World::new(made_rows(Making::SeedsLater));
        world.carry(Effect::InstallFromRelease);
        assert_eq!(world.seed(), vec![Effect::TellJoined]);
        assert_eq!(world.stage(), Stage::Landed);
    }
}
