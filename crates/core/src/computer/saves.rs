//! What the Computer DO knows of its saves (`Saves`): the saves of `/data` it
//! keeps, the snapshot that caches one, what each start restored, and the
//! records left to delete.

use fragment_proto::computer::{ComputerRestore, ComputerSave, LifeEnd, LossCause, RestoreSource};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::computer::disk::{disk_clear, disk_filling, ComputerDisk};
use crate::computer::lifecycle::Ended;
use crate::computer::limits::{SAVES_KEPT, SET_ASIDE_KEPT_MS, SET_ASIDE_MAX};
use crate::computer::start_line::Full;

/// Records one save holds at most: one per directory it saves.
pub const SAVE_RECORDS_MAX: usize = 4;
/// Records let go of and not deleted yet, at most (`Saves::let_go`): a few
/// saves' worth, since deletes that keep failing while saves work are not
/// expected (both are R2's). Past it the oldest is no longer tried, its
/// archive left in R2, as the DO logs.
pub const FORGETTING_MAX: usize = 4 * SAVE_RECORDS_MAX;

/// A save of `/data`, as the Computer DO keeps it: the `DirectoryBackup`
/// records it was taken as (the authority on what a wake restores, handed
/// back to restore and to delete them), and what the platform knows of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Save {
    /// 1 for a computer's first save, one more for each after it.
    pub number: u64,
    /// Its first record's id (a UUID): what a snapshot names as its save,
    /// and what a wake says it restored.
    pub id: String,
    /// The start it is of.
    pub generation: u64,
    pub at_ms: i64,
    /// The number its lifecycle gave the save that took it (`Action::Save`'s
    /// `seq`, one more for each save asked, never reset while its saves are
    /// kept: both live in its DO alone): saves of one start that ran at once
    /// are kept in this order (`overtaken`), the order they were asked in,
    /// not the order they ended in, whatever their clocks. Unset in a save
    /// kept before it was stored (before #1378): one kept before this
    /// release's DO ran any save, so newer than none of its saves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    /// Whether the guest answered the hold before it was taken.
    pub held: bool,
    /// Its `DirectoryBackup` records, a directory's before any inside it
    /// (the order they restore in).
    pub records: Vec<Value>,
    /// Its restore failed as a whole save (an archive gone or corrupt), or
    /// the image's check of what it restored did: no start uses it again.
    #[serde(default)]
    pub unusable: bool,
    /// When it was set aside (ms; `Saves::choose`): kept, and its owner's
    /// export reads it, but no start restores it, and it is let go of
    /// `SET_ASIDE_KEPT_MS` after this. Set on the saves set aside alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_aside_ms: Option<i64>,
}

impl Save {
    pub fn view(&self) -> ComputerSave {
        ComputerSave { number: self.number, id: self.id.clone(), at: self.at_ms, generation: self.generation, held: self.held, unusable: self.unusable, set_aside: self.set_aside_ms }
    }
}

/// Why an operator's choice of a save (`Saves::choose`) was refused:
/// nothing changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotChosen {
    /// No save of that number is kept: these are, newest first.
    NotKept { kept: Vec<u64> },
    /// It was found unusable: no start restores it.
    Unusable,
}

/// A container snapshot: a cache of one save, for one image (docs/computers.md,
/// "Saves and what a wake restores"). A wake uses it only while `save` is
/// the current save and `image` the pinned image's reference, so it only
/// ever makes a wake faster, never different.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub id: String,
    /// The image's reference it was taken of (a name can stand for another
    /// image after a redeploy).
    pub image: String,
    /// The id of the save it was taken with, in the same sleep.
    pub save: String,
}

/// What a start restores, chosen as it begins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// The snapshot `id`, of the current save.
    Snapshot { id: String },
    /// The image, then the current save.
    Backup,
    /// The image, with an empty `/data`: there is no save.
    Nothing,
}

impl Plan {
    pub fn source(&self) -> RestoreSource {
        match self {
            Plan::Snapshot { .. } => RestoreSource::Snapshot,
            Plan::Backup => RestoreSource::Backup,
            Plan::Nothing => RestoreSource::Nothing,
        }
    }
}

/// The start under way, and what it restores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Starting {
    pub generation: u64,
    pub from: RestoreSource,
}

/// What the Computer DO knows of its saves, kept whole beside its
/// `Lifecycle`. Each method is one fact the DO learned, applied as it
/// happened; nothing here reaches the container. A record stored before it
/// kept several saves (its one `backup`) is not read: a hard cut, so a
/// computer saved before then wakes with an empty `/data`, its agents'
/// selves coming back from their repos.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Saves {
    /// The saves kept that a start may restore, newest first: at most
    /// `SAVES_KEPT` and the one its running life came up from.
    #[serde(default)]
    saves: Vec<Save>,
    /// The saves set aside (`choose`), newest first, at most
    /// `SET_ASIDE_MAX`: kept apart from `saves`, so no start restores one
    /// and no rotation of them lets one go, a release from before them
    /// included (it reads none, and restores the save chosen: Codex on
    /// this field's PR). Each until `SET_ASIDE_KEPT_MS` after it was set
    /// aside.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    aside: Vec<Save>,
    /// The last save's number.
    #[serde(default)]
    numbered: u64,
    /// The snapshot of the current save, when its sleep took one.
    snapshot: Option<Snapshot>,
    /// The start under way: an exit before it comes up is its own failure.
    starting: Option<Starting>,
    /// How the last life ended, until the next start that comes up reads it.
    ended: Option<Ended>,
    /// The save that held all the last life did as it ended (`ended`): read
    /// then, while its own save is still kept, since saves of starts that
    /// never came up may push it out before a start comes up. None when
    /// stored before it was kept, or not known: worked out as it is read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ended_in: Option<EndedIn>,
    /// What the last start that came up restored.
    restored: Option<ComputerRestore>,
    /// The image the last start that came up runs: what a snapshot its
    /// sleep takes is of (a pin while it runs changes the next start's).
    running: Option<Running>,
    /// Starts that went back in time.
    rollbacks: u64,
    /// Records no save holds now that the DO has not deleted yet, oldest
    /// first: each stays until its delete worked (`forgot`), so one that
    /// failed is tried again by the next.
    #[serde(default)]
    forgetting: Vec<Value>,
    /// The newest start that went back in time, kept for its owner's notice
    /// (`notices`) whatever starts come after, until they say they saw it.
    #[serde(default)]
    went_back: Option<WentBack>,
    /// The newest lost life its owner saw the notice of (`saw`): a
    /// `went_back` of it, or of an earlier life, is told no more.
    #[serde(default)]
    pub(super) seen: u64,
    /// Its owner restarted it: the next start is from the image and the
    /// save, never a snapshot (one could carry what broke it outside
    /// `/data`), and no snapshot is kept until a start comes up.
    #[serde(default)]
    fresh: bool,
    /// Its disk as last read (`measured`), and whether its owner is told it
    /// is filling (a latch: `disk_filling` sets it, `disk_clear` clears it):
    /// a cache, which a release from before it drops as it writes its saves
    /// back (the next read sets both again).
    #[serde(default)]
    pub(super) disk: Option<ComputerDisk>,
    #[serde(default)]
    pub(super) disk_told: bool,
}

/// The save that held all a life did as it ended (`Saves::ended`): `save`,
/// or none (an empty `/data`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndedIn {
    pub life: u64,
    #[serde(default)]
    pub save: Option<String>,
}

/// A start that went back in time, as its owner is told of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WentBack {
    /// The life whose work since the save restored is in no save.
    pub life: u64,
    pub cause: LossCause,
    /// When that life ended (0: before ends were timed).
    pub ended_at_ms: i64,
    /// What the start that went back restored.
    pub restored: ComputerRestore,
    /// That save's number (none: it restored nothing).
    pub save: Option<u64>,
    /// No turn ran and no port tab was used on the lost life after the save
    /// it went back to (`Saves::unchanged`). False when stored before this
    /// was kept: told as a change lost.
    #[serde(default)]
    pub unchanged: bool,
}

/// Why a life's work since its last save is in no save, from how it ended
/// (a start that went back after a sleep that saved fell back past a save
/// that would not restore).
pub(super) fn loss_cause(e: &Ended) -> LossCause {
    match (e.by, e.saved) {
        (LifeEnd::Exit, _) => LossCause::Crash,
        (LifeEnd::Sleep, false) if e.restart => LossCause::Restart,
        (LifeEnd::Sleep, false) => LossCause::Unsaved,
        (LifeEnd::Sleep, true) => LossCause::Unusable,
    }
}

/// A start that came up, and the image's reference it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Running {
    pub generation: u64,
    pub image: String,
}

impl Saves {
    /// The saves kept that a start may restore, newest first.
    pub fn all(&self) -> &[Save] {
        &self.saves
    }

    /// The saves set aside (`choose`), newest first.
    pub fn aside(&self) -> &[Save] {
        &self.aside
    }

    /// Every save kept, set aside or not, newest first: what its view
    /// lists and its owner's export reads.
    pub fn every_save(&self) -> Vec<&Save> {
        let mut all: Vec<&Save> = self.saves.iter().chain(&self.aside).collect();
        all.sort_by_key(|s| std::cmp::Reverse(s.number));
        all
    }

    /// The save a start restores: the newest one not found unusable (or,
    /// with every one found so, the newest, tried again as any start is).
    pub fn current(&self) -> Option<&Save> {
        self.saves.iter().find(|s| !s.unusable).or(self.saves.first())
    }

    /// What a start restores: the snapshot only when it caches the current
    /// save for the pinned image (`image`, its reference: `None` with
    /// snapshots off); otherwise the image and the current save, or
    /// nothing when there is no save.
    pub fn plan(&self, image: Option<&str>) -> Plan {
        let Some(save) = self.current() else { return Plan::Nothing };
        match (&self.snapshot, image) {
            (Some(s), Some(image)) if s.save == save.id && s.image == image && !save.unusable && !self.fresh => Plan::Snapshot { id: s.id.clone() },
            _ => Plan::Backup,
        }
    }

    /// Why `plan(image)` restores the save whole, when it does (a start's
    /// line: `start_line`): the first of these the DO's facts show, beside
    /// `plan`'s own test so the two agree. `fell_back`: this wake's start
    /// from the snapshot failed (`Lifecycle::fell_back`).
    pub fn full_why(&self, image: Option<&str>, fell_back: bool) -> Option<Full> {
        if self.plan(image) != Plan::Backup {
            return None;
        }
        let newest = self.saves.first().map(|s| s.id.as_str());
        Some(match (&self.snapshot, image) {
            (_, None) => Full::Off,
            _ if self.fresh => Full::Restart,
            _ if self.aside.first().is_some_and(|a| self.saves.first().is_none_or(|s| a.number > s.number)) => Full::Aside,
            _ if self.current().map(|s| s.id.as_str()) != newest => Full::Unusable,
            (Some(s), Some(image)) if s.image != image => Full::Image,
            (Some(_), _) => Full::Other,
            (None, _) if fell_back => Full::DeadSnapshot,
            (None, _) => match self.ended {
                Some(Ended { by: LifeEnd::Exit, .. }) => Full::Crash,
                Some(Ended { saved: false, restart: true, .. }) => Full::Restart,
                Some(Ended { saved: false, .. }) => Full::Unsaved,
                Some(Ended { saved: true, .. }) => Full::NoSnapshot,
                None => Full::Other,
            },
        })
    }

    /// The image's reference the last start that came up ran.
    pub fn last_image(&self) -> Option<&str> {
        self.running.as_ref().map(|r| r.image.as_str())
    }

    /// Its owner restarted it: the snapshot is forgotten (only a cache), and
    /// none is kept until a start comes up, so the restart's start is from
    /// the image and the newest good save (or, past one that fails its
    /// check, an older one). Again, the same.
    pub fn restart_fresh(&mut self) {
        self.snapshot = None;
        self.fresh = true;
    }

    /// Whether its next start is a restart's, fresh from the image.
    pub fn fresh(&self) -> bool {
        self.fresh
    }

    /// The newest start that went back in time and lost something, kept
    /// for its owner.
    pub fn went_back(&self) -> Option<&WentBack> {
        self.went_back.as_ref()
    }

    /// The newest lost life a start has yet to go back for: the last life
    /// ended by an exit, or by a sleep that did not save, and no start has
    /// come up since (it is asleep, or won't wake), unless going back to the
    /// current save, as its next start does, loses nothing of it.
    pub fn pending_loss(&self) -> Option<&Ended> {
        self.ended.as_ref().filter(|e| (e.by == LifeEnd::Exit || !e.saved) && !self.lost_nothing(e, self.current()))
    }

    /// Whether going back to `back_to` (none: an empty `/data`) after the
    /// life `e` ended lost nothing: nothing was done on that life since its
    /// newest save that worked (`Ended::quiet`), and `back_to` is that save,
    /// or, with none of its own, the one it came up from. Its owner is told
    /// nothing of such a start (`notices`); its guest still reads it as a
    /// rollback (`restored`).
    fn lost_nothing(&self, e: &Ended, back_to: Option<&Save>) -> bool {
        e.quiet && self.back_to_its_own(e, back_to)
    }

    /// Whether going back to `back_to` after the life `e` ended changed
    /// nothing the platform ran since: no turn ran and no port tab was used
    /// on that life since its newest save that worked (`Ended::unchanged`),
    /// and `back_to` is that save (or the one it came up from), as for
    /// `lost_nothing`. Its owner is told it went back, never that changes
    /// were lost.
    pub fn unchanged(&self, e: &Ended, back_to: Option<&Save>) -> bool {
        e.unchanged && self.back_to_its_own(e, back_to)
    }

    /// Whether `back_to` (none: an empty `/data`) is the save that holds
    /// what the life `e` did up to its newest save that worked: that save,
    /// or, with none of its own, the one it came up from.
    fn back_to_its_own(&self, e: &Ended, back_to: Option<&Save>) -> bool {
        let kept_in = match self.ended_in.as_ref().filter(|k| k.life == e.generation) {
            Some(k) => Some(k.save.as_deref()),
            None => self.kept_in(e.generation),
        };
        kept_in == Some(back_to.map(|s| s.id.as_str()))
    }

    /// The save that holds what life `generation` did up to its newest save
    /// that worked, as the saves kept now say, set aside or not: that save,
    /// or, with none of its own kept, the one it came up from (none: not
    /// known).
    fn kept_in(&self, generation: u64) -> Option<Option<&str>> {
        match self.every_save().into_iter().find(|s| s.generation == generation) {
            Some(own) => Some(Some(own.id.as_str())),
            None => self.restored.as_ref().filter(|r| r.generation == generation).map(|r| r.save.as_deref()),
        }
    }

    /// Its owner saw the notice of the lost life `life`: it, and any loss
    /// before it, is told no more. A notice read before a newer loss
    /// replaced it hides only itself (the newer is still told); a life past
    /// the newest loss told, or 0, is no notice they could have read, and is
    /// refused (`false`). Again, the same.
    pub fn saw(&mut self, life: u64) -> bool {
        let told = self.went_back.as_ref().map(|w| w.life).max(self.pending_loss().map(|e| e.generation));
        let read = life >= 1 && told.is_some_and(|t| life <= t);
        if read {
            self.seen = self.seen.max(life);
        }
        read
    }

    /// The start `generation` begins, restoring `from`. A start older than
    /// the one recorded was superseded before it began: it records nothing.
    pub fn starting(&mut self, generation: u64, from: RestoreSource) {
        if self.starting.is_none_or(|s| s.generation <= generation) {
            self.starting = Some(Starting { generation, from });
        }
    }

    /// What the start `generation` under way restores, if it is the one.
    pub fn start_from(&self, generation: u64) -> Option<RestoreSource> {
        self.starting.filter(|s| s.generation == generation).map(|s| s.from)
    }

    /// A start from the snapshot failed: the snapshot is forgotten (it is
    /// only a cache), so the next start is from the image and the save
    /// . Answers the snapshot forgotten.
    pub fn snapshot_failed(&mut self) -> Option<Snapshot> {
        self.snapshot.take()
    }

    /// The save `id` would not restore as a whole : no start restores
    /// it again, nor a snapshot of it. Answers whether a save is left to
    /// try, which is older (each newer one was found unusable first).
    pub fn unusable(&mut self, id: &str) -> bool {
        if let Some(s) = self.saves.iter_mut().find(|s| s.id == id) {
            s.unusable = true;
        }
        if self.snapshot.as_ref().is_some_and(|s| s.save == id) {
            self.snapshot = None;
        }
        let left = self.saves.iter().any(|s| !s.unusable);
        assert!(!left || self.current().is_some_and(|c| c.id != id), "the save found unusable is never the current one while another is left");
        left
    }

    /// A life ended (the lifecycle's `Step::ended`).
    pub fn ended(&mut self, ended: Ended) {
        assert!(self.ended.is_none_or(|e| e.generation < ended.generation), "a life ends once, after the lives before it");
        self.ended_in = self.kept_in(ended.generation).map(|save| EndedIn { life: ended.generation, save: save.map(str::to_string) });
        self.ended = Some(ended);
    }

    /// The start `generation` came up on `image` (its reference), having
    /// restored `from` (`save`: the id of the save it restored): what it
    /// restored, and whether that went back in time, which is counted.
    /// `None` when another start began meanwhile (this one was superseded:
    /// nothing is recorded).
    pub fn came_up(&mut self, generation: u64, from: RestoreSource, save: Option<&str>, image: Option<&str>, now_ms: i64) -> Option<ComputerRestore> {
        if self.starting != Some(Starting { generation, from }) {
            return None;
        }
        self.starting = None;
        self.running = image.map(|image| Running { generation, image: image.to_string() });
        assert_eq!(from == RestoreSource::Nothing, save.is_none(), "a start restores a save when there is one");
        let restored_save = save.and_then(|id| self.saves.iter().find(|b| b.id == id));
        let ended = self.ended.take();
        // a life's work since its last save is in no save unless its sleep
        // saved it, and this start restored that save (its life's newest):
        // a crash loses it, and so does a sleep that slept unsaved
        let rollback = match ended {
            None => false,
            Some(Ended { by: LifeEnd::Exit, .. }) => true,
            Some(Ended { by: LifeEnd::Sleep, generation: life, saved, .. }) => {
                // its newest save as it ended, when that was read then
                // (`ended_in`): one let go of since (set aside, then past its
                // time) still held its work (Codex on this PR)
                let newest_of_life = match self.ended_in.as_ref().filter(|k| k.life == life) {
                    Some(k) => k.save.as_deref(),
                    None => self.kept_in(life).flatten(),
                };
                !(saved && restored_save.is_some_and(|s| s.generation == life) && newest_of_life == save)
            }
        };
        let restored = ComputerRestore {
            generation,
            from,
            save: save.map(str::to_string),
            saved_at: restored_save.map(|s| s.at_ms),
            age_ms: restored_save.map(|s| now_ms.saturating_sub(s.at_ms).max(0)),
            after: ended.map(|e| e.by),
            rollback,
            at: now_ms,
        };
        // its owner is told of it until they say they saw it (`notices`),
        // unless it lost nothing (read before this start's `restored`
        // replaces the lost life's)
        let told = ended.filter(|e| rollback && !self.lost_nothing(e, restored_save)).map(|e| (e, self.unchanged(&e, restored_save)));
        self.rollbacks += u64::from(rollback);
        self.restored = Some(restored.clone());
        if let Some((e, unchanged)) = told {
            self.went_back = Some(WentBack { life: e.generation, cause: loss_cause(&e), ended_at_ms: e.at_ms, restored: restored.clone(), save: restored_save.map(|s| s.number), unchanged });
        }
        // a restart's start is fresh; the starts after it may use a snapshot again
        self.fresh = false;
        assert!(self.went_back.as_ref().is_none_or(|w| w.restored.rollback && w.life < w.restored.generation), "a start went back for an earlier life");
        Some(restored)
    }

    /// The start `generation` saved `/data` as `records`, its save `seq`,
    /// done at `at_ms` (`held`: its guest answered the hold), and not
    /// `overtaken`: the newest save, and the
    /// current one. A snapshot of any other save caches nothing a wake
    /// would use. The saves it pushed out of the newest `SAVES_KEPT`, and
    /// those set aside past their time (`trim_aside`), are let go of
    /// (`let_go`): answers what that pushed out unforgotten.
    pub fn saved(&mut self, generation: u64, records: Vec<Value>, seq: u64, at_ms: i64, held: bool) -> Vec<Value> {
        assert!(!records.is_empty() && records.len() <= SAVE_RECORDS_MAX, "a save is one record per directory, at most {SAVE_RECORDS_MAX}");
        let id = records[0]["id"].as_str().unwrap_or_default().to_string();
        assert!(!id.is_empty(), "a save has an id");
        let depth = |r: &Value| r["dir"].as_str().map_or(0, |d| d.matches('/').count());
        assert!(records.windows(2).all(|w| depth(&w[0]) <= depth(&w[1])), "a directory's record before any inside it: {records:?}");
        assert!(!self.every_save().iter().any(|s| s.id == id), "a save is kept once");
        assert!(!self.overtaken(generation, seq), "never kept ahead of a newer save of its start");
        if self.snapshot.as_ref().is_some_and(|s| s.save != id) {
            self.snapshot = None;
        }
        self.numbered += 1;
        self.saves.insert(0, Save { number: self.numbered, id, generation, at_ms, seq: Some(seq), held, records, unusable: false, set_aside_ms: None });
        let mut dropped = if self.saves.len() > SAVES_KEPT { self.saves.split_off(SAVES_KEPT) } else { vec![] };
        // the save its running life came up from is kept past the newest: one
        // a start restored and came up from, the last to fall back to
        let booted = self.restored.as_ref().and_then(|r| r.save.as_deref());
        if let Some(at) = dropped.iter().position(|s| Some(s.id.as_str()) == booted && !s.unusable) {
            self.saves.push(dropped.remove(at));
        }
        // those set aside are kept for their time, apart from the rotation
        dropped.extend(self.trim_aside(at_ms));
        assert!(self.saves.len() <= SAVES_KEPT + 1, "the newest, and the one it came up from");
        assert!(self.saves.windows(2).all(|w| w[0].number > w[1].number), "newest first");
        assert!(self.snapshot.as_ref().is_none_or(|s| self.saves[0].id == s.save), "a snapshot kept is of the current save");
        self.let_go(dropped.into_iter().flat_map(|s| s.records).collect())
    }

    /// `records` are no save's now (a save pushed out, or what a failed
    /// save took before it failed): the DO deletes each (`forgetting`),
    /// and each is kept here until that worked (`forgot`). Answers those
    /// pushed past `FORGETTING_MAX`, oldest first, which no one will delete.
    pub fn let_go(&mut self, records: Vec<Value>) -> Vec<Value> {
        assert!(records.iter().all(|r| r["id"].as_str().is_some_and(|id| !id.is_empty())), "a record has an id: {records:?}");
        assert!(!records.iter().any(|r| self.every_save().iter().any(|s| s.records.contains(r))), "a record let go of is no kept save's");
        self.forgetting.extend(records);
        let over = self.forgetting.len().saturating_sub(FORGETTING_MAX);
        let lost: Vec<Value> = self.forgetting.drain(..over).collect();
        assert!(self.forgetting.len() <= FORGETTING_MAX, "at most FORGETTING_MAX records to delete");
        lost
    }

    /// An operator's choice of save `number` for its next start (docs/api.md,
    /// Operators; docs/computers.md, "Saves and what a wake restores"),
    /// made while no container runs: each save newer than it is set aside
    /// (`aside`, at `now_ms`), and so is each older one past the newest
    /// `SAVES_KEPT` of them, so every save a start may fall back to is
    /// reached within its fallbacks (`STARTS_OWED_MAX`; Codex on this PR).
    /// It may be one set aside before, then restorable again; what an
    /// earlier choice set aside stays so for its time. One set aside is
    /// kept, and its owner's export reads it, but no start restores it,
    /// whatever fails. The snapshot goes unless it caches the save chosen.
    /// Answers what letting go of those set aside past their time or bound
    /// (`trim_aside`) pushed out unforgotten. Again, the same. Refused, with
    /// nothing changed, when no save of that number is kept or it was found
    /// unusable.
    pub fn choose(&mut self, number: u64, now_ms: i64) -> Result<Vec<Value>, NotChosen> {
        let Some(chosen) = self.every_save().into_iter().find(|s| s.number == number) else {
            return Err(NotChosen::NotKept { kept: self.every_save().iter().map(|s| s.number).collect() });
        };
        if chosen.unusable {
            return Err(NotChosen::Unusable);
        }
        let id = chosen.id.clone();
        let mut restorable: Vec<Save> = vec![];
        for mut s in std::mem::take(&mut self.saves).into_iter().chain(std::mem::take(&mut self.aside)) {
            let restored = match s.number.cmp(&number) {
                std::cmp::Ordering::Greater => false,
                std::cmp::Ordering::Equal => true,
                std::cmp::Ordering::Less => s.set_aside_ms.is_none(),
            };
            if restored {
                s.set_aside_ms = None;
                restorable.push(s);
            } else {
                s.set_aside_ms.get_or_insert(now_ms);
                self.aside.push(s);
            }
        }
        restorable.sort_by_key(|s| std::cmp::Reverse(s.number));
        for mut s in restorable.split_off(restorable.len().min(SAVES_KEPT + 1)) {
            s.set_aside_ms = Some(now_ms);
            self.aside.push(s);
        }
        self.saves = restorable;
        self.aside.sort_by_key(|s| std::cmp::Reverse(s.number));
        if self.snapshot.as_ref().is_some_and(|s| s.save != id) {
            self.snapshot = None;
        }
        let dropped = self.trim_aside(now_ms);
        assert_eq!(self.current().map(|s| s.id.as_str()), Some(id.as_str()), "its next start restores the save chosen");
        assert!(self.saves.len() <= SAVES_KEPT + 1 && self.aside.len() <= SET_ASIDE_MAX, "bounded");
        Ok(self.let_go(dropped.into_iter().flat_map(|s| s.records).collect()))
    }

    /// Takes out the saves set aside that are let go of at `now_ms`: each
    /// set aside `SET_ASIDE_KEPT_MS` ago or more, and the oldest past
    /// `SET_ASIDE_MAX`. Answers them.
    fn trim_aside(&mut self, now_ms: i64) -> Vec<Save> {
        let (mut kept, mut dropped): (Vec<Save>, Vec<Save>) = std::mem::take(&mut self.aside).into_iter().partition(|s| s.set_aside_ms.is_some_and(|at| now_ms < at.saturating_add(SET_ASIDE_KEPT_MS)));
        dropped.extend(kept.split_off(kept.len().min(SET_ASIDE_MAX)));
        self.aside = kept;
        dropped
    }

    /// The records let go of and not deleted yet, oldest first.
    pub fn forgetting(&self) -> &[Value] {
        &self.forgetting
    }

    /// Whether save `seq` of start `generation` was overtaken as it ran:
    /// the newest kept save is that start's and was asked after it (an
    /// owner's sleep's or restart's, asked once an awake save let go of its
    /// hold, ending first). The DO lets its records go: never
    /// kept ahead of a newer capture. An older one kept as it ran (ending
    /// first) overtakes nothing: the newer is kept ahead of it (alexlwn123
    /// on #1378). One only asked, not kept, overtakes nothing: a capture is
    /// kept until a newer one is (Codex on #1378). So saves of one start
    /// are kept in the order they were asked, whichever ends first: by
    /// their numbers, never their clocks, which two may share (Codex).
    /// That start's newest kept save, set aside or not: one set aside since
    /// (an operator's choice of an older save as it ran) still overtakes
    /// it, so it never becomes the current save ahead of that choice (Codex
    /// on this PR).
    pub fn overtaken(&self, generation: u64, seq: u64) -> bool {
        let newest = self.every_save().into_iter().find(|s| s.generation == generation);
        newest.is_some_and(|s| s.seq.is_some_and(|newest| newest > seq))
    }

    /// Every record a wipe deletes (docs/api.md, Operators): each kept
    /// save's, newest first, then each let go of and not deleted yet. A
    /// record is a save's or waits to be deleted, never both.
    pub fn every_record(&self) -> Vec<Value> {
        let kept = self.every_save().into_iter().flat_map(|s| s.records.iter());
        let out: Vec<Value> = kept.chain(self.forgetting.iter()).cloned().collect();
        assert!(out.len() <= (SAVES_KEPT + 1 + SET_ASIDE_MAX) * SAVE_RECORDS_MAX + FORGETTING_MAX, "a computer's records are bounded");
        out
    }

    /// Whether it keeps a snapshot's id (a cache of its current save).
    pub fn has_snapshot(&self) -> bool {
        self.snapshot.is_some()
    }

    /// Its disk was read (`df` in a start, `df` and `du` after an awake
    /// save): kept when it is newer than the one kept (a late read of an
    /// earlier moment changes nothing), with the newest read of what its
    /// `/data` restores to when it has none (a start's). Answers whether it
    /// was kept.
    pub fn measured(&mut self, disk: ComputerDisk) -> bool {
        assert!(disk.total > 0 && disk.free <= disk.total, "a disk read is its size and what fits in it: {disk:?}");
        if self.disk.is_some_and(|d| d.at > disk.at) {
            return false;
        }
        let disk = ComputerDisk { data: disk.data.or(self.disk.and_then(|d| d.data)), ..disk };
        self.disk_told = if self.disk_told { !disk_clear(&disk) } else { disk_filling(&disk) };
        self.disk = Some(disk);
        true
    }

    /// Its disk as its view shows it: as last read, and the room it has.
    pub fn disk_view(&self) -> Option<fragment_proto::computer::DiskRoom> {
        self.disk.map(|d| d.view(self.disk_told))
    }

    /// Its disk as last read.
    pub fn disk(&self) -> Option<ComputerDisk> {
        self.disk
    }

    /// The record `id` was deleted.
    pub fn forgot(&mut self, id: &str) {
        self.forgetting.retain(|r| r["id"] != id);
    }

    /// A sleep of `generation` could not save: whether that is news (no
    /// save of this start is kept). A slow sleep is asked for again
    /// (`SLEEP_DEADLINE_MS`), and the second ask's save fails once the
    /// first's destroy took the container: the first one saved.
    pub fn save_failed(&self, generation: u64) -> bool {
        !self.saves.iter().any(|b| b.generation == generation)
    }

    /// A sleep took snapshot `id` of `image` after its save `save`: kept
    /// when `save` is still the newest save (a second ask's save may have
    /// replaced it meanwhile). A sleep whose own save failed takes none,
    /// and a snapshot that fails forgets nothing: the one kept is still a
    /// cache of its own save .
    pub fn snapshotted(&mut self, save: &str, id: &str, image: &str) -> bool {
        // a restart's sleep keeps none: its start is fresh from the image
        let current = self.saves.first().is_some_and(|b| b.id == save) && !self.fresh;
        if current {
            self.snapshot = Some(Snapshot { id: id.to_string(), image: image.to_string(), save: save.to_string() });
        }
        current
    }

    /// The image's reference the start `generation` came up on, if it did.
    pub fn image_of(&self, generation: u64) -> Option<&str> {
        self.running.as_ref().filter(|r| r.generation == generation).map(|r| r.image.as_str())
    }

    pub fn restored(&self) -> Option<&ComputerRestore> {
        self.restored.as_ref()
    }

    pub fn rollbacks(&self) -> u64 {
        self.rollbacks
    }

    /// The saves, as the computer's view shows them: set aside or not,
    /// newest first.
    pub fn views(&self) -> Vec<ComputerSave> {
        self.every_save().into_iter().map(Save::view).collect()
    }
}

#[cfg(test)]
mod tests {
    use fragment_proto::computer::{ComputerNotice, ComputerSave, LifeEnd, RestoreSource};
    use serde_json::json;

    use super::*;
    use crate::computer::testing::{bare, rec, Computer, IMAGE, T};
    use crate::computer::*;

    /// Goal (the lifecycle's simulation, 2026-11-18): a life that ended
    /// after a save of its own, nothing done since, is in that save. Starts
    /// that never came up may push that save out before one comes up (one
    /// put to sleep as it started, its save failing, stays up and saves);
    /// going back past it, to the save the life came up from, lost its work,
    /// and is told. Read as it ended, not after (before, it was told
    /// nothing). Restart: the same once stored and read back.
    #[test]
    fn a_lifes_own_save_pushed_out_before_a_start_is_still_its_own() {
        let mut v = Saves::default();
        v.saved(1, rec("s0"), 0, T, true);
        v.starting(2, RestoreSource::Backup);
        v.came_up(2, RestoreSource::Backup, Some("s0"), Some(IMAGE), T + 1_000).expect("came up from s0");
        v.saved(2, rec("s1"), 0, T + 2_000, true);
        v.ended(Ended { generation: 2, by: LifeEnd::Exit, saved: false, at_ms: T + 3_000, restart: false, quiet: true, unchanged: true });
        assert!(v.pending_loss().is_none(), "s1 holds its work: back to s1 loses nothing, while s1 is kept");
        // three starts that never came up, each saving what will not restore
        for g in 3..=5 {
            v.saved(g, rec(&format!("x{g}")), 0, T + 10_000 * g as i64, false);
            assert!(v.unusable(&format!("x{g}")) || g == 5);
        }
        assert!(v.all().iter().all(|s| s.id != "s1"), "s1 pushed out: {:?}", v.all().iter().map(|s| s.id.clone()).collect::<Vec<_>>());
        assert_eq!(v.current().map(|s| s.id.as_str()), Some("s0"), "back to the save it came up from");
        let v: Saves = serde_json::from_str(&serde_json::to_string(&v).unwrap()).unwrap();
        let mut v = v;
        v.starting(6, RestoreSource::Backup);
        let r = v.came_up(6, RestoreSource::Backup, Some("s0"), Some(IMAGE), T + 90_000).expect("came up");
        assert!(r.rollback);
        assert!(v.went_back().is_some_and(|w| w.life == 2 && w.save == Some(1)), "its work since s0 is lost, and told: {:?}", v.went_back());
    }

    /// Goal (P3): a wake uses the snapshot only while it caches the current
    /// save for the pinned image; any other is the image and the save, and
    /// a computer never saved starts empty. Method: each way the two can
    /// disagree: another image (a pin, a redeploy), snapshots off, a newer
    /// save, a current save that is another than the snapshot's (the one
    /// it cached would not restore), a snapshot of a save that has gone.
    #[test]
    fn a_snapshot_is_used_only_for_its_save_and_image() {
        let mut s = Saves::default();
        assert_eq!(s.plan(Some(IMAGE)), Plan::Nothing);
        s.saved(1, rec("b0"), 0, T - 1, true);
        s.saved(1, rec("b1"), 0, T, true);
        assert!(s.snapshotted("b1", "s1", IMAGE));
        assert_eq!(s.plan(Some(IMAGE)), Plan::Snapshot { id: "s1".into() });
        assert_eq!(s.plan(Some("registry/stub@sha256:2")), Plan::Backup, "another image's");
        assert_eq!(s.plan(None), Plan::Backup, "snapshots off");
        let mut other = s.clone();
        assert!(other.unusable("b1"), "b0 is left");
        assert_eq!((other.plan(Some(IMAGE)), other.current().map(|c| c.id.as_str())), (Plan::Backup, Some("b0")), "the current save is another than the snapshot's");
        // the next sleep saves and takes no snapshot: the old one is dropped
        s.saved(2, rec("b2"), 0, T + 1, true);
        assert_eq!(s.plan(Some(IMAGE)), Plan::Backup);
        assert_eq!(s.snapshot, None);
        // a snapshot taken with a save that is no longer current is not kept
        assert!(!s.snapshotted("b1", "s1-late", IMAGE));
        assert_eq!(s.plan(Some(IMAGE)), Plan::Backup);
    }

    /// Goal: a wipe deletes every record a computer's saves name, the kept
    /// saves' and those let go of and not deleted yet, each once, and sees
    /// its snapshot. Method: saves past `SAVES_KEPT` (one let go of), a
    /// failed delete kept, a snapshot; then each record forgotten.
    #[test]
    fn a_wipe_finds_every_record_its_saves_name() {
        let mut s = Saves::default();
        assert!(s.every_record().is_empty() && !s.has_snapshot());
        for (n, id) in ["b0", "b1", "b2", "b3"].iter().enumerate() {
            s.saved(1, rec(id), 0, T + n as i64, true);
        }
        // b0 was pushed out of the newest three: let go of, not deleted yet
        let ids = |s: &Saves| s.every_record().iter().map(|r| r["id"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>();
        assert_eq!(ids(&s), ["b3", "b2", "b1", "b0"]);
        assert!(s.snapshotted("b3", "s3", IMAGE));
        assert!(s.has_snapshot());
        // once b0's delete worked it is not named again
        s.forgot("b0");
        assert_eq!(ids(&s), ["b3", "b2", "b1"]);
    }

    /// Goal (2026-11-03: a computer's three saves were all of one long
    /// life, none a start had come up from): beside its newest three, the
    /// save its running life came up from is kept, as the oldest, the last
    /// a start falls back to; once a start comes up from a newer one, it
    /// goes with the next save. One found unusable is not kept so. Valid,
    /// invalid (unusable), the bound (four at most). Method: a life's saves
    /// as the DO keeps them.
    #[test]
    fn the_save_a_life_came_up_from_is_kept_past_the_newest() {
        let mut c = Computer::new();
        c.wake();
        c.sleep(Some("b1"));
        let r = c.wake();
        assert_eq!(r.save.as_deref(), Some("b1"), "this life came up from b1");
        // the life saves three times, and a fourth: b1 stays, the oldest
        for id in ["b2", "b3", "b4", "b5"] {
            c.turn_saved(id);
        }
        let ids = |c: &Computer| c.saves.all().iter().map(|s| s.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&c), ["b5", "b4", "b3", "b1"], "the newest three, and the one it came up from");
        assert!(c.saves.forgetting().iter().any(|r| r["id"] == "b2"), "the others let go of");
        // its newer saves found unusable, a start falls back to b1 last
        for bad in ["b5", "b4", "b3"] {
            assert!(c.saves.unusable(bad));
        }
        assert_eq!(c.saves.current().map(|s| s.id.as_str()), Some("b1"));
        // a start comes up from a newer one: b1 goes with the next save
        let mut c = Computer::new();
        c.wake();
        c.sleep(Some("b1"));
        c.wake();
        for id in ["b2", "b3", "b4"] {
            c.turn_saved(id);
        }
        c.sleep(Some("b5"));
        let r = c.wake();
        assert_eq!(r.save.as_deref(), Some("b5"));
        c.turn_saved("b6");
        assert_eq!(ids(&c), ["b6", "b5", "b4"], "b5 is among the newest: nothing more kept");
        // one found unusable is not kept past the newest
        let mut c = Computer::new();
        c.wake();
        c.sleep(Some("b1"));
        c.wake();
        c.saves.unusable("b1");
        for id in ["b2", "b3", "b4"] {
            c.turn_saved(id);
        }
        assert_eq!(ids(&c), ["b4", "b3", "b2"]);
    }

    /// Goal (P3, decision 19): a snapshot is of the image its start ran, so
    /// an image pinned while it runs is taken at the next wake, from the
    /// save, never answered with a snapshot of the old image. On master the
    /// sleep named its snapshot's image by the pin at the time of the sleep.
    /// Method: a start on one image, a pin, its sleep, the next plan.
    #[test]
    fn a_pin_while_it_runs_wakes_on_the_new_image() {
        const NEXT: &str = "registry/stub@sha256:2";
        let mut s = Saves::default();
        s.starting(1, RestoreSource::Nothing);
        assert!(s.came_up(1, RestoreSource::Nothing, None, Some(IMAGE), T).is_some());
        // its owner pins NEXT while it runs; its sleep saves, then snapshots
        s.saved(1, rec("b1"), 0, T + 10, true);
        let of = s.image_of(1).expect("the image it runs").to_string();
        assert_eq!(of, IMAGE);
        assert!(s.snapshotted("b1", "s1", &of));
        assert_eq!(s.plan(Some(NEXT)), Plan::Backup, "the new image, and the save");
        assert_eq!(s.plan(Some(IMAGE)), Plan::Snapshot { id: "s1".into() }, "pinned back, the snapshot serves again");
        assert_eq!(s.image_of(2), None, "a start that has not come up runs nothing yet");
    }

    /// Goal (docs/computers.md, Lifecycle): the second ask of a
    /// slow sleep (`SLEEP_DEADLINE_MS`) never forgets the snapshot the first
    /// stored, nor notes a failed save the first one made. On master a
    /// snapshot that failed forgot the stored one (cell/src/computer.rs,
    /// `sleep`). Method: both asks of one start, in each order their saves
    /// can land.
    #[test]
    fn the_second_ask_of_a_slow_sleep_keeps_the_first_ones_snapshot() {
        let mut s = Saves::default();
        s.saved(3, rec("b-old"), 0, T, true);
        // the first ask saves and snapshots
        s.saved(4, rec("b1"), 0, T + 70_000, true);
        assert!(s.snapshotted("b1", "s1", IMAGE));
        // the second: its save fails once the first's destroy took the
        // container, so it takes no snapshot, and that is not news
        assert!(!s.save_failed(4), "the first ask saved this start");
        assert!(s.save_failed(5), "a start whose sleep saved nothing is news");
        assert_eq!(s.plan(Some(IMAGE)), Plan::Snapshot { id: "s1".into() });
        // the other order: the second's save lands while the first snapshots
        let mut s = Saves::default();
        s.saved(4, rec("b1"), 0, T, true);
        s.saved(4, rec("b2"), 0, T + 1_000, true);
        assert!(!s.snapshotted("b1", "s1", IMAGE), "the first's snapshot caches a save that is no longer current");
        assert_eq!(s.plan(Some(IMAGE)), Plan::Backup);
        assert!(s.snapshotted("b2", "s2", IMAGE));
        assert_eq!(s.plan(Some(IMAGE)), Plan::Snapshot { id: "s2".into() });
    }

    /// Goal (P7): each start that comes up says what it restored, how old
    /// that was, and how the life before it ended; a start that went back
    /// in time (after a crash, or a sleep that slept unsaved) is counted,
    /// and one after a sleep that saved is not. Method: one computer's
    /// history, read after each start.
    #[test]
    fn a_wake_says_what_it_restored() {
        let mut c = Computer::new();
        let r = c.wake();
        assert_eq!((r.from, r.save.clone(), r.after, r.rollback), (RestoreSource::Nothing, None, None, false), "its first start");
        c.sleep(Some("b1"));
        let saved_at = c.saves.current().map(|b| b.at_ms).expect("a save");
        let r = c.wake();
        assert_eq!((r.from, r.save.as_deref(), r.after, r.rollback), (RestoreSource::Backup, Some("b1"), Some(LifeEnd::Sleep), false), "{r:?}");
        assert_eq!((r.saved_at, r.age_ms), (Some(saved_at), Some(r.at - saved_at)));
        assert!(r.age_ms.is_some_and(|a| a > 0));
        // a crash: it comes back from the save before it, a rollback
        let g = c.life.generation();
        let s = c.apply(Event::Exited { generation: g });
        let s = c.owed(&s);
        let Some(Action::Start { generation }) = s.actions.iter().find(|a| matches!(a, Action::Start { .. })).cloned() else { panic!("{s:?}") };
        let r = c.come_up(generation);
        assert_eq!((r.from, r.after, r.rollback, c.saves.rollbacks()), (RestoreSource::Backup, Some(LifeEnd::Exit), true, 1), "{r:?}");
        // a sleep that slept unsaved: the next wake is a rollback too
        c.sleep(None);
        let r = c.wake();
        assert_eq!((r.save.as_deref(), r.after, r.rollback, c.saves.rollbacks()), (Some("b1"), Some(LifeEnd::Sleep), true, 2), "{r:?}");
        // a crash, then a start that fails, then one that comes up: counted once
        let g = c.life.generation();
        c.apply(Event::Exited { generation: g });
        c.apply(Event::StartFailed { generation: g + 1, why: "pull".into() });
        let s = c.apply(Event::Wake { why: Wake::Owner });
        let Some(Action::Start { generation }) = s.actions.first().cloned() else { panic!("{s:?}") };
        let r = c.come_up(generation);
        assert_eq!((r.rollback, c.saves.rollbacks()), (true, 3));
        // a sleep that saves: none
        c.sleep(Some("b2"));
        let r = c.wake();
        assert_eq!((r.save.as_deref(), r.rollback, c.saves.rollbacks()), (Some("b2"), false, 3));
        assert_eq!(c.saves.restored(), Some(&r));
        // a start superseded before it came up records nothing
        assert_eq!(c.saves.came_up(r.generation, r.from, Some("b2"), Some(IMAGE), c.now), None, "it came up once");
        // what it knows survives a restart of its Durable Object
        let back: Saves = serde_json::from_str(&serde_json::to_string(&c.saves).unwrap()).unwrap();
        assert_eq!(back, c.saves);
    }

    /// Goal: a restart's start is fresh from the image and the newest good
    /// save, never a snapshot (one could carry what broke it outside
    /// /data): the snapshot is forgotten, none is kept by the restart's own
    /// sleep, and once a start comes up snapshots are used again.
    #[test]
    fn a_restarts_start_is_fresh_from_the_image() {
        let mut c = Computer::new();
        c.wake();
        c.sleep(Some("b1"));
        assert!(c.saves.snapshotted("b1", "s1", IMAGE));
        assert_eq!(c.saves.plan(Some(IMAGE)), Plan::Snapshot { id: "s1".into() });
        c.saves.restart_fresh();
        c.saves.restart_fresh();
        assert!(c.saves.fresh() && !c.saves.has_snapshot(), "forgotten; again, the same");
        assert_eq!(c.saves.plan(Some(IMAGE)), Plan::Backup);
        // its own sleep's snapshot is not kept
        c.saves.saved(1, rec("b2"), 0, c.now, true);
        assert!(!c.saves.snapshotted("b2", "s2", IMAGE) && c.saves.plan(Some(IMAGE)) == Plan::Backup);
        let r = c.wake();
        assert_eq!((r.from, r.save.as_deref()), (RestoreSource::Backup, Some("b2")));
        assert!(!c.saves.fresh());
        c.sleep(Some("b3"));
        assert!(c.saves.snapshotted("b3", "s3", IMAGE) && c.saves.plan(Some(IMAGE)) == Plan::Snapshot { id: "s3".into() }, "snapshots again");
    }

    /// Goal: three saves are kept, newest first, each numbered; a fourth
    /// pushes out the oldest, whose records the DO deletes; a save says
    /// whether it was held, and the view shows them so.
    #[test]
    fn three_saves_are_kept_newest_first() {
        let mut s = Saves::default();
        assert!(s.saved(1, rec("a"), 0, T, true).is_empty());
        assert!(s.saved(1, rec("b"), 0, T + 1, false).is_empty());
        assert!(s.saved(2, rec("c"), 0, T + 2, true).is_empty());
        assert!(s.forgetting().is_empty());
        let lost = s.saved(2, vec![json!({ "id": "d", "dir": "/data" }), json!({ "id": "d-work", "dir": "/data/work" })], 0, T + 3, true);
        assert!(lost.is_empty());
        assert_eq!(s.forgetting(), rec("a").as_slice(), "the oldest's records, to delete");
        assert_eq!(s.all().iter().map(|s| (s.number, s.id.as_str())).collect::<Vec<_>>(), vec![(4, "d"), (3, "c"), (2, "b")]);
        assert_eq!(s.current().map(|c| c.records.len()), Some(2));
        let views = s.views();
        assert_eq!(views[2], ComputerSave { number: 2, id: "b".into(), at: T + 1, generation: 1, held: false, unusable: false, set_aside: None });
        let v = serde_json::to_value(&views[0]).unwrap();
        assert_eq!(v, json!({ "number": 4, "id": "d", "at": T + 3, "generation": 2, "held": true }));
        // a record stored before it kept several saves is not read: a hard cut
        let old: Saves = serde_json::from_value(json!({ "backup": { "id": "x", "generation": 1, "atMs": T }, "snapshot": null, "starting": null, "ended": null, "restored": null, "running": null, "rollbacks": 0 })).unwrap();
        assert_eq!((old.current(), old.plan(Some(IMAGE))), (None, Plan::Nothing));
    }

    /// Goal (#156, problem 10): a record let go of is kept until its delete
    /// worked, so one that failed is tried again by the next save's pass,
    /// across a restart of the DO (the saves are stored as JSON); a failed
    /// save's records are let go of the same way; past `FORGETTING_MAX` the
    /// oldest are answered as lost, never silently dropped.
    #[test]
    fn a_record_let_go_of_is_kept_until_its_delete_worked() {
        let ids = |s: &Saves| s.forgetting().iter().map(|r| r["id"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        let mut s = Saves::default();
        for (i, id) in ["a", "b", "c", "d"].into_iter().enumerate() {
            s.saved(1, rec(id), 0, T + i as i64, true);
        }
        assert_eq!(ids(&s), ["a"]);
        // a's delete failed: the next save's pass finds it still, beside b
        s.saved(1, rec("e"), 0, T + 10, true);
        assert_eq!(ids(&s), ["a", "b"]);
        // a restart reads them back; one stored before this field reads none
        let mut s: Saves = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(ids(&s), ["a", "b"]);
        let mut stored = serde_json::to_value(&s).unwrap();
        stored.as_object_mut().unwrap().remove("forgetting");
        assert!(serde_json::from_value::<Saves>(stored).unwrap().forgetting().is_empty());
        // b's delete worked, then a's; a delete answered twice changes nothing
        s.forgot("b");
        s.forgot("a");
        s.forgot("a");
        assert!(s.forgetting().is_empty());
        assert_eq!(s.all().iter().map(|k| k.id.as_str()).collect::<Vec<_>>(), ["e", "d", "c"], "the saves kept are untouched");
        // a failed save's records, taken before it failed
        assert!(s.let_go(vec![json!({ "id": "f", "dir": "/data" })]).is_empty());
        assert_eq!(ids(&s), ["f"]);
        // bounded: the oldest past the bound are answered, in order
        let many: Vec<Value> = (0..FORGETTING_MAX).map(|i| json!({ "id": format!("m{i}"), "dir": "/data" })).collect();
        let lost = s.let_go(many);
        assert_eq!(lost, vec![json!({ "id": "f", "dir": "/data" })]);
        assert_eq!(s.forgetting().len(), FORGETTING_MAX);
        assert_eq!(ids(&s)[0], "m0");
    }

    /// Invalid: a record a kept save holds is never let go of (its delete
    /// would take what a wake restores).
    #[test]
    #[should_panic(expected = "no kept save's")]
    fn a_kept_saves_record_is_never_let_go_of() {
        let mut s = Saves::default();
        s.saved(1, rec("a"), 0, T, true);
        s.let_go(rec("a"));
    }

    /// Goal (P9, Codex and alexlwn123 on #1378): a save that a newer save
    /// of its start overtook (kept, and asked after it) is never kept ahead
    /// of it; one only asked, an older one kept as it ran (asked before it,
    /// ended first), or another start's overtakes nothing, so a completed
    /// capture is kept. Restart: stored and read back, the same; a save
    /// stored before `seq` overtakes nothing. Invalid: keeping one ahead of
    /// a newer one is a bug (`an_overtaken_save_is_never_kept`).
    #[test]
    fn a_save_is_overtaken_only_by_a_newer_one_kept() {
        let mut s = Saves::default();
        assert!(!s.overtaken(1, 1), "none kept");
        s.saved(1, rec("a"), 2, T + 30, true);
        assert!(s.overtaken(1, 1), "a newer capture kept");
        assert!(!s.overtaken(1, 3), "an older one, kept as it ran");
        assert!(!s.overtaken(1, 2), "itself");
        assert!(!s.overtaken(2, 1), "another start's");
        let s: Saves = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert!(s.overtaken(1, 1) && !s.overtaken(1, 3));
        let mut v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["saves"][0]["seq"], 2, "stored: {v}");
        v["saves"][0].as_object_mut().unwrap().remove("seq");
        let old: Saves = serde_json::from_value(v).unwrap();
        assert!(!old.overtaken(1, 1) && old.all()[0].seq.is_none(), "a save stored before it overtakes nothing");
    }

    /// Goal (Codex on this PR): a save set aside still overtakes an older
    /// save of its start that lands after it. A life's awake save waits on
    /// its upload while its sleep's save, asked after it, is kept and its
    /// container stops; an operator then chooses an older save, setting
    /// the sleep's aside. The awake save that lands then is overtaken, so
    /// it never becomes the current save ahead of the operator's choice.
    /// Another start's, or one asked after it, is not.
    #[test]
    fn a_save_set_aside_still_overtakes_an_older_one_of_its_start() {
        let mut s = Saves::default();
        s.saved(1, rec("b1"), 1, T, true);
        s.saved(2, rec("b3"), 3, T + 20, true);
        assert!(s.overtaken(2, 2), "its sleep's save was asked after it");
        assert_eq!(s.choose(1, T + 30), Ok(vec![]));
        assert!(s.overtaken(2, 2), "set aside, it is still that start's newest");
        assert!(!s.overtaken(2, 4) && !s.overtaken(3, 2), "one asked after it, or another start's");
        assert_eq!(s.current().map(|c| c.id.as_str()), Some("b1"));
    }

    #[test]
    #[should_panic(expected = "never kept ahead of a newer save of its start")]
    fn an_overtaken_save_is_never_kept() {
        let mut s = Saves::default();
        s.saved(1, rec("b"), 2, T, true);
        s.saved(1, rec("a"), 1, T + 10, true);
    }

    /// The numbers of every save kept, newest first, and whether each is
    /// set aside.
    fn numbers(s: &Saves) -> Vec<(u64, bool)> {
        s.every_save().iter().map(|s| (s.number, s.set_aside_ms.is_some())).collect()
    }

    /// Goal (production, 2026-11-20: a computer whose newest save no longer
    /// fit its disk sat in `wont_wake`, each start failing its restore for
    /// room, a strike each, with an older save that fit and no way to it):
    /// an operator's choice of an older save makes it the one its next
    /// start restores. The newer are set aside: kept through the saves
    /// after it, outside the rotation, for `SET_ASIDE_KEPT_MS`, and never
    /// restored, even past a chosen save that will not restore. The start
    /// that comes up is a rollback its owner is told of, with where it went
    /// back to. Valid; invalid (no such save, one found unusable: nothing
    /// changed); replay (the same choice twice, its time kept; one set
    /// aside chosen in turn); restart (stored and read back; a release from
    /// before reads none set aside and restores the save chosen). Method:
    /// the lifecycle and its saves, as the DO drives them.
    #[test]
    fn an_operators_choice_starts_it_from_an_older_save() {
        let rules = Rules::default();
        let mut c = Computer::new();
        c.wake();
        c.sleep(Some("b1"));
        let r = c.wake();
        assert_eq!(r.save.as_deref(), Some("b1"), "its last good life came up from b1");
        c.turn_saved("b2");
        c.turn_saved("b3");
        c.sleep(Some("b4"));
        let life = c.life.generation();
        // b4 no longer fits its disk: each start fails its restore, a strike
        let s = c.apply(Event::Wake { why: Wake::Record });
        let mut next = s.actions.first().cloned();
        while let Some(Action::Start { generation }) = next {
            let s = c.apply(Event::StartFailed { generation, why: "restore stopped with ~203 MB free, below the 268435456 byte floor".into() });
            next = s.alarm_ms.and_then(|at| {
                c.now = at - 1_000;
                c.apply(Event::Alarm).actions.first().cloned()
            });
        }
        assert!(matches!(c.life.phase, Phase::Failed { .. }), "{:?}", c.life.phase);
        assert_eq!(c.saves.current().map(|s| s.id.as_str()), Some("b4"), "every start tries b4 again");
        assert_eq!(numbers(&c.saves), [(4, false), (3, false), (2, false), (1, false)], "b1 kept, the one its last life came up from");
        // invalid: refused, nothing changed
        let before = c.saves.clone();
        assert_eq!(c.saves.choose(9, c.now), Err(NotChosen::NotKept { kept: vec![4, 3, 2, 1] }));
        let mut found_bad = c.saves.clone();
        assert!(found_bad.unusable("b1"));
        let unusable = found_bad.clone();
        assert_eq!(found_bad.choose(1, c.now), Err(NotChosen::Unusable));
        assert_eq!((&c.saves, &found_bad), (&before, &unusable), "a refused choice changes nothing");
        // valid: b1 chosen; the newer set aside, the snapshot of b4 gone
        assert!(c.saves.snapshotted("b4", "s4", IMAGE), "its sleep's snapshot, of b4");
        let chose_at = c.now;
        assert_eq!(c.saves.choose(1, chose_at), Ok(vec![]));
        assert_eq!(numbers(&c.saves), [(4, true), (3, true), (2, true), (1, false)]);
        assert!(c.saves.views().iter().all(|v| v.set_aside == (v.number > 1).then_some(chose_at)), "its view says which, and when");
        assert_eq!((c.saves.current().map(|s| s.id.as_str()), c.saves.plan(Some(IMAGE)), c.saves.has_snapshot()), (Some("b1"), Plan::Backup, false));
        assert_eq!(c.saves.full_why(Some(IMAGE), false), Some(Full::Aside));
        assert_eq!(c.saves.every_record().len(), 4, "a wipe still finds them");
        // replay: the same, its time kept
        let once = c.saves.clone();
        assert_eq!(c.saves.choose(1, chose_at + 60_000), Ok(vec![]));
        assert_eq!(c.saves, once);
        // restart: read back the same; a release from before reads the
        // saves alone, none set aside, and restores the save chosen
        let back: Saves = serde_json::from_str(&serde_json::to_string(&c.saves).unwrap()).unwrap();
        assert_eq!(back, c.saves);
        let mut stored = serde_json::to_value(&c.saves).unwrap();
        assert_eq!(stored["aside"][0]["setAsideMs"], chose_at);
        assert_eq!(stored["saves"].as_array().map(Vec::len), Some(1));
        assert!(stored.as_object_mut().unwrap().remove("aside").is_some());
        let before_it: Saves = serde_json::from_value(stored).unwrap();
        assert_eq!((before_it.current().map(|s| s.id.as_str()), before_it.aside().len()), (Some("b1"), 0));
        // one set aside may be chosen in turn; the older stay set aside
        let mut undone = c.saves.clone();
        assert_eq!(undone.choose(4, chose_at + 1), Ok(vec![]));
        assert_eq!(undone.current().map(|s| s.id.as_str()), Some("b4"));
        assert_eq!(numbers(&undone), [(4, false), (3, true), (2, true), (1, false)]);
        // the chosen save would not restore: no save set aside is ever
        // tried, and it is a strike, as with no older save
        let mut failing = c.saves.clone();
        assert!(!failing.unusable("b1"), "nothing older to fall back to");
        assert_eq!(failing.current().map(|s| s.id.as_str()), Some("b1"), "tried again, never b4");
        // its strikes counted anew, a record's wake starts it, from b1
        c.apply(Event::SaveChosen);
        assert_eq!((bare(&c.life.phase), c.life.failures()), (Phase::ASLEEP, 0));
        let s = c.apply(Event::Wake { why: Wake::Record });
        let Some(Action::Start { generation }) = s.actions.first().cloned() else { panic!("a record wakes it: {s:?}") };
        let r = c.come_up(generation);
        assert_eq!((r.save.as_deref(), r.rollback), (Some("b1"), true), "{r:?}");
        let told = notices(&c.life, &c.saves, &c.images, &rules).into_iter().find(|n| matches!(n, ComputerNotice::WentBack { .. }));
        let b1_at = c.saves.all().iter().find(|s| s.id == "b1").map(|s| s.at_ms);
        assert!(
            matches!(told, Some(ComputerNotice::WentBack { life: l, pending: false, save: Some(1), saved_at, unchanged: false, .. }) if l == life && saved_at == b1_at),
            "its owner is told where it went back to: {told:?}"
        );
        // its saves after it: those set aside stay outside the rotation
        for id in ["c1", "c2", "c3", "c4"] {
            c.turn_saved(id);
        }
        assert_eq!(numbers(&c.saves), [(8, false), (7, false), (6, false), (4, true), (3, true), (2, true), (1, false)]);
        assert!(c.saves.forgetting().iter().any(|r| r["id"] == "c1") && !c.saves.forgetting().iter().any(|r| r["id"] == "b4"));
        // until their time: the first save after it lets them go
        let g = c.life.generation();
        c.saves.saved(g, rec("c5"), 0, chose_at + SET_ASIDE_KEPT_MS - 1, true);
        assert_eq!(c.saves.aside().len(), 3, "kept to the last ms");
        c.saves.saved(g, rec("c6"), 0, chose_at + SET_ASIDE_KEPT_MS, true);
        assert!(c.saves.aside().is_empty(), "{:?}", numbers(&c.saves));
        assert!(["b4", "b3", "b2"].iter().all(|id| c.saves.forgetting().iter().any(|r| r["id"] == *id)), "let go of, to delete");
    }

    /// Goal (Codex on this PR): a life's work is in the save it ended with,
    /// even once that save, set aside, is let go of before a start comes
    /// up (the same choice made again past its time): the start from an
    /// older save is a rollback its owner is told of, never taken for one
    /// that lost nothing. On the first cut the newest save of that life
    /// still kept (its awake save) was taken for its last.
    #[test]
    fn a_save_set_aside_and_let_go_of_still_held_its_lifes_work() {
        let mut c = Computer::new();
        c.wake();
        c.turn_saved("b1");
        c.sleep(Some("b2"));
        let life = c.life.generation();
        assert_eq!(c.saves.choose(1, c.now), Ok(vec![]));
        let lost = c.saves.choose(1, c.now + SET_ASIDE_KEPT_MS).expect("b1 kept");
        assert!(lost.is_empty() && c.saves.aside().is_empty() && c.saves.forgetting().iter().any(|r| r["id"] == "b2"), "b2 let go of");
        let r = c.wake();
        assert_eq!((r.save.as_deref(), r.rollback), (Some("b1"), true), "{r:?}");
        assert!(c.saves.went_back().is_some_and(|w| w.life == life && w.save == Some(1) && !w.unchanged), "{:?}", c.saves.went_back());
    }

    /// Goal: a later choice keeps what an earlier one set aside; saves set
    /// aside are bounded, `SET_ASIDE_MAX` at most (two choices' worth),
    /// past it the oldest let go of; and however choices run (Codex on
    /// this PR: each of six set aside chosen in turn, oldest first), the
    /// saves a start may restore stay within what its fallbacks reach (the
    /// one chosen, and `SAVES_KEPT` before it), the rest set aside. A
    /// computer's records stay bounded (`every_record`). Method: four rounds
    /// of three saves, each followed by a choice of the oldest of them, so
    /// each sets two more aside; then each set aside chosen, oldest first.
    #[test]
    fn saves_set_aside_are_bounded() {
        let mut s = Saves::default();
        let mut lost = vec![];
        for round in 0..4u64 {
            for i in 1..=3 {
                let n = round * 3 + i;
                s.saved(round + 1, rec(&format!("n{n}")), 0, T + n as i64, true);
            }
            lost.extend(s.choose(round * 3 + 1, T + 100 + round as i64).expect("a save kept"));
        }
        let aside: Vec<u64> = s.aside().iter().map(|s| s.number).collect();
        assert_eq!(aside, [12, 11, 9, 8, 6, 5], "the newest SET_ASIDE_MAX");
        assert_eq!(aside.len(), SET_ASIDE_MAX);
        assert!(lost.is_empty() && ["n3", "n2"].iter().all(|id| s.forgetting().iter().any(|r| r["id"] == *id)), "the oldest let go of, to delete");
        assert_eq!(s.current().map(|s| s.number), Some(10));
        for n in [5, 6, 8, 9, 11, 12] {
            assert!(s.choose(n, T + 200).is_ok());
            assert_eq!(s.current().map(|s| s.number), Some(n));
            assert!(s.all().len() <= SAVES_KEPT + 1, "chose {n}: {:?}", numbers(&s));
            assert!(s.every_record().len() <= (SAVES_KEPT + 1 + SET_ASIDE_MAX) * SAVE_RECORDS_MAX + FORGETTING_MAX);
        }
        assert_eq!(s.all().iter().map(|s| s.number).collect::<Vec<_>>(), [12, 11, 9, 8], "the one chosen, and SAVES_KEPT before it");
    }

    #[test]
    #[should_panic(expected = "a directory's record before any inside it")]
    fn a_save_whose_records_are_out_of_order_is_a_bug() {
        let mut s = Saves::default();
        s.saved(1, vec![json!({ "id": "w", "dir": "/data/work" }), json!({ "id": "d", "dir": "/data" })], 0, T, true);
    }
}
