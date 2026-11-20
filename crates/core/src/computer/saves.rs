//! What the Computer DO knows of its saves (`Saves`): the saves of `/data` it
//! keeps, the snapshot that caches one, what each start restored, and the
//! records left to delete.

use fragment_proto::computer::{ComputerRestore, ComputerSave, LifeEnd, LossCause, RestoreSource};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::computer::disk::{disk_clear, disk_filling, ComputerDisk};
use crate::computer::lifecycle::Ended;
use crate::computer::limits::SAVES_KEPT;
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
}

impl Save {
    pub fn view(&self) -> ComputerSave {
        ComputerSave { number: self.number, id: self.id.clone(), at: self.at_ms, generation: self.generation, held: self.held, unusable: self.unusable }
    }
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
    /// The saves kept, newest first, at most `SAVES_KEPT`.
    #[serde(default)]
    saves: Vec<Save>,
    /// The last save's number.
    #[serde(default)]
    numbered: u64,
    /// The snapshot of the current save, when its sleep took one.
    snapshot: Option<Snapshot>,
    /// The start under way: an exit before it comes up is its own failure.
    starting: Option<Starting>,
    /// How the last life ended, until the next start that comes up reads it.
    ended: Option<Ended>,
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
    /// The saves kept, newest first.
    pub fn all(&self) -> &[Save] {
        &self.saves
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
        let kept_in = match self.saves.iter().find(|s| s.generation == e.generation) {
            Some(own) => Some(Some(own.id.as_str())),
            None => self.restored.as_ref().filter(|r| r.generation == e.generation).map(|r| r.save.as_deref()),
        };
        e.quiet && kept_in == Some(back_to.map(|s| s.id.as_str()))
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
                let newest_of_life = self.saves.iter().find(|s| s.generation == life).map(|s| s.id.as_str());
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
        let told = ended.filter(|e| rollback && !self.lost_nothing(e, restored_save));
        self.rollbacks += u64::from(rollback);
        self.restored = Some(restored.clone());
        if let Some(e) = told {
            self.went_back = Some(WentBack { life: e.generation, cause: loss_cause(&e), ended_at_ms: e.at_ms, restored: restored.clone(), save: restored_save.map(|s| s.number) });
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
    /// would use. The saves it pushed out of the newest `SAVES_KEPT` are
    /// let go of (`let_go`): answers what that pushed out unforgotten.
    pub fn saved(&mut self, generation: u64, records: Vec<Value>, seq: u64, at_ms: i64, held: bool) -> Vec<Value> {
        assert!(!records.is_empty() && records.len() <= SAVE_RECORDS_MAX, "a save is one record per directory, at most {SAVE_RECORDS_MAX}");
        let id = records[0]["id"].as_str().unwrap_or_default().to_string();
        assert!(!id.is_empty(), "a save has an id");
        let depth = |r: &Value| r["dir"].as_str().map_or(0, |d| d.matches('/').count());
        assert!(records.windows(2).all(|w| depth(&w[0]) <= depth(&w[1])), "a directory's record before any inside it: {records:?}");
        assert!(!self.saves.iter().any(|s| s.id == id), "a save is kept once");
        assert!(!self.overtaken(generation, seq), "never kept ahead of a newer save of its start");
        if self.snapshot.as_ref().is_some_and(|s| s.save != id) {
            self.snapshot = None;
        }
        self.numbered += 1;
        self.saves.insert(0, Save { number: self.numbered, id, generation, at_ms, seq: Some(seq), held, records, unusable: false });
        let mut dropped = if self.saves.len() > SAVES_KEPT { self.saves.split_off(SAVES_KEPT) } else { vec![] };
        // the save its running life came up from is kept past the newest: one
        // a start restored and came up from, the last to fall back to
        let booted = self.restored.as_ref().and_then(|r| r.save.as_deref());
        if let Some(at) = dropped.iter().position(|s| Some(s.id.as_str()) == booted && !s.unusable) {
            self.saves.push(dropped.remove(at));
        }
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
        assert!(!records.iter().any(|r| self.saves.iter().any(|s| s.records.contains(r))), "a record let go of is no kept save's");
        self.forgetting.extend(records);
        let over = self.forgetting.len().saturating_sub(FORGETTING_MAX);
        let lost: Vec<Value> = self.forgetting.drain(..over).collect();
        assert!(self.forgetting.len() <= FORGETTING_MAX, "at most FORGETTING_MAX records to delete");
        lost
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
    pub fn overtaken(&self, generation: u64, seq: u64) -> bool {
        self.saves.first().is_some_and(|s| s.generation == generation && s.seq.is_some_and(|newest| newest > seq))
    }

    /// Every record a wipe deletes (docs/api.md, Operators): each kept
    /// save's, newest first, then each let go of and not deleted yet. A
    /// record is a save's or waits to be deleted, never both.
    pub fn every_record(&self) -> Vec<Value> {
        let kept = self.saves.iter().flat_map(|s| s.records.iter());
        let out: Vec<Value> = kept.chain(self.forgetting.iter()).cloned().collect();
        assert!(out.len() <= (SAVES_KEPT + 1) * SAVE_RECORDS_MAX + FORGETTING_MAX, "a computer's records are bounded");
        out
    }

    /// Whether it keeps a snapshot's id (a cache of its current save).
    pub fn has_snapshot(&self) -> bool {
        self.snapshot.is_some()
    }

    /// Its disk was read (`df`, after a start came up or a save): kept
    /// when it is newer than the one kept (a late read of an earlier
    /// moment changes nothing). Answers whether it was kept.
    pub fn measured(&mut self, disk: ComputerDisk) -> bool {
        assert!(disk.total > 0 && disk.free <= disk.total, "a disk read is its size and what fits in it: {disk:?}");
        if self.disk.is_some_and(|d| d.at > disk.at) {
            return false;
        }
        self.disk_told = if self.disk_told { !disk_clear(disk.total, disk.free) } else { disk_filling(disk.total, disk.free) };
        self.disk = Some(disk);
        true
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

    /// The saves, as the computer's view shows them.
    pub fn views(&self) -> Vec<ComputerSave> {
        self.saves.iter().map(Save::view).collect()
    }
}

#[cfg(test)]
mod tests {
    use fragment_proto::computer::{ComputerSave, LifeEnd, RestoreSource};
    use serde_json::json;

    use super::*;
    use crate::computer::testing::{rec, Computer, IMAGE, T};
    use crate::computer::*;

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
        assert_eq!(views[2], ComputerSave { number: 2, id: "b".into(), at: T + 1, generation: 1, held: false, unusable: false });
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

    #[test]
    #[should_panic(expected = "never kept ahead of a newer save of its start")]
    fn an_overtaken_save_is_never_kept() {
        let mut s = Saves::default();
        s.saved(1, rec("b"), 2, T, true);
        s.saved(1, rec("a"), 1, T + 10, true);
    }

    #[test]
    #[should_panic(expected = "a directory's record before any inside it")]
    fn a_save_whose_records_are_out_of_order_is_a_bug() {
        let mut s = Saves::default();
        s.saved(1, vec![json!({ "id": "w", "dir": "/data/work" }), json!({ "id": "d", "dir": "/data" })], 0, T, true);
    }
}
