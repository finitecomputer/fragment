//! A computer's disk: read in its container, kept with its saves, and told to
//! its owner while it fills.

use serde::{Deserialize, Serialize};

use crate::computer::lifecycle::{Event, Lifecycle, Phase};

/// Its owner is told its disk is filling once this much of it is used
/// (`notices`, `disk_filling`): on a 20 GB disk, about 4 GB before it is
/// full, while a save still fits and there is room to tidy up. A disk that
/// filled made a computer's every later start fail (2026-11-03).
pub const DISK_TOLD_USED_PCT: u64 = 80;
const _: () = assert!(DISK_TOLD_USED_PCT > 0 && DISK_TOLD_USED_PCT < 100, "told before it is full, never always");

/// And only once at most this is free: a computer's own disk (20 GB) is
/// told at 4 GB, past `DISK_TOLD_USED_PCT`, while one on a host's shared
/// disk (a dev box's, CI's: terabytes, most of it others') is not told of
/// room it has plenty of.
pub const DISK_TOLD_FREE_MAX: u64 = 5_000_000_000;

/// Told, it is told no more only once a read finds clearly more room:
/// at most this much used, or at least `DISK_CLEAR_FREE_MIN` free. The gap
/// below where it is told keeps a disk that hovers at the edge (a read a
/// few KB either side) from showing and clearing its notice in turn.
pub const DISK_CLEAR_USED_PCT: u64 = 70;
pub const DISK_CLEAR_FREE_MIN: u64 = 6_000_000_000;
const _: () = assert!(DISK_CLEAR_USED_PCT < DISK_TOLD_USED_PCT && DISK_CLEAR_FREE_MIN > DISK_TOLD_FREE_MAX, "it clears only past where it is told");

/// Whether a disk of `total` bytes with `free` left is used past
/// `DISK_TOLD_USED_PCT`, with at most `DISK_TOLD_FREE_MAX` free: its owner
/// is told.
pub fn disk_filling(total: u64, free: u64) -> bool {
    assert!(free <= total, "free fits in its disk");
    (total - free) as u128 * 100 >= total as u128 * DISK_TOLD_USED_PCT as u128 && free <= DISK_TOLD_FREE_MAX
}

/// Whether a disk told filling has clearly more room now: at most
/// `DISK_CLEAR_USED_PCT` used, or at least `DISK_CLEAR_FREE_MIN` free.
pub fn disk_clear(total: u64, free: u64) -> bool {
    assert!(free <= total, "free fits in its disk");
    (total - free) as u128 * 100 <= total as u128 * DISK_CLEAR_USED_PCT as u128 || free >= DISK_CLEAR_FREE_MIN
}

/// The start whose disk to read once `event` was applied to `life`: an
/// awake one whose save just worked. Never a sleep's save, whose stop
/// follows at once (its next start reads it). A start reads its own disk
/// in its start, its restore in place and its guest not yet let go (the
/// cell's `try_start`, within the start's deadline): read once it is
/// ready, the read raced its guest's first seconds, and an owner's wake
/// answered after it (FIN-462).
pub fn reads_disk_after(event: &Event, life: &Lifecycle) -> Option<u64> {
    match (event, &life.phase) {
        (Event::Saved { generation: g, .. }, Phase::Awake { generation, .. }) if g == generation => Some(*g),
        _ => None,
    }
}

/// What reads a computer's disk, run in its container (`disk_of_df` reads
/// its answer): POSIX `df` in bytes, which GNU's and BusyBox's both print.
pub const DISK_READ: &str = "df -P -B1 /";

/// A computer's disk from `DISK_READ`'s answer, read at `at_ms` in start
/// `generation`'s container: the line
/// after its header, its size and what is available (to the guest's user,
/// as a restore's floor counts it). None for an answer it cannot read.
pub fn disk_of_df(out: &str, at_ms: i64, generation: u64) -> Option<ComputerDisk> {
    let mut lines = out.lines().filter(|l| !l.trim().is_empty());
    lines.next().filter(|h| h.starts_with("Filesystem"))?;
    let cols: Vec<&str> = lines.next()?.split_whitespace().collect();
    let (total, free) = (cols.get(1)?.parse::<u64>().ok()?, cols.get(3)?.parse::<u64>().ok()?);
    (total > 0 && free <= total).then_some(ComputerDisk { total, free, at: at_ms, generation })
}

/// A computer's disk, as a read found it (`disk_of_df`): its size and what
/// is free, in bytes; when it was read, and the start whose container it
/// read (0 in a read stored before it was kept).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerDisk {
    pub total: u64,
    pub free: u64,
    pub at: i64,
    #[serde(default)]
    pub generation: u64,
}

#[cfg(test)]
mod tests {
    use fragment_proto::computer::ComputerNotice;

    use super::*;
    use crate::computer::testing::T;
    use crate::computer::*;

    /// Goal (2026-11-03: a disk that filled left a computer that could not
    /// start): its owner is told its disk is filling once a read finds it
    /// used past `DISK_TOLD_USED_PCT` with at most `DISK_TOLD_FREE_MAX`
    /// free, and no more only once a read finds clearly more room
    /// (`disk_clear`): a disk hovering at either edge, reads a few KB
    /// apart, never shows and clears it in turn (Codex on #1015). Valid
    /// (each edge, exactly), oscillation around both edges, replay and
    /// order (a read of an earlier moment changes nothing), invalid (more
    /// free than its size is refused), saves stored before (none told).
    /// Method: the reads as the DO takes them, and what is told.
    #[test]
    fn a_disk_filling_is_told_until_a_read_finds_room() {
        const GB: u64 = 1_000_000_000;
        const KB: u64 = 8_192;
        let told = |s: &Saves| notices(&Lifecycle::new(), s, &Images::default(), &Rules::default());
        assert!(disk_filling(20 * GB, 4 * GB) && !disk_filling(20 * GB, 4 * GB + 1), "80% used exactly is told, a byte under is not");
        assert!(!disk_filling(2_000 * GB, 300 * GB), "a host's shared disk, 85% used with room to spare");
        assert!(disk_filling(2_000 * GB, 5 * GB) && !disk_filling(2_000 * GB, 5 * GB + 1), "told once at most DISK_TOLD_FREE_MAX is free");
        assert!(disk_clear(20 * GB, 6 * GB) && !disk_clear(20 * GB, 6 * GB - 1), "clear at 70% used, or 6 GB free");
        assert!(disk_clear(2_000 * GB, 6 * GB) && !disk_clear(2_000 * GB, 6 * GB - 1));
        // a run of reads, each a moment later, and whether it is told after each
        let run = |total: u64, frees: &[u64]| -> Vec<bool> {
            let mut s = Saves::default();
            frees.iter().enumerate().map(|(i, free)| {
                assert!(s.measured(ComputerDisk { total, free: *free, at: T + i as i64, generation: 1 }));
                matches!(told(&s).as_slice(), [ComputerNotice::DiskFilling { total: t, free: f }] if *t == total && *f == *free)
            }).collect()
        };
        // around the told edge of a computer's own disk: shown, and it stays until 6 GB free
        assert_eq!(run(20 * GB, &[4 * GB + KB, 4 * GB, 4 * GB + KB, 4 * GB, 5 * GB, 6 * GB - 1, 6 * GB, 6 * GB - KB, 4 * GB + KB, 4 * GB]), [false, true, true, true, true, true, false, false, false, true]);
        // around the cleared edge: once clear, never told again until the told edge
        assert_eq!(run(20 * GB, &[4 * GB, 6 * GB, 6 * GB - KB, 6 * GB, 6 * GB - KB, 4 * GB + KB]), [true, false, false, false, false, false]);
        // a host's shared disk, around 5 GB free
        assert_eq!(run(2_000 * GB, &[5 * GB + KB, 5 * GB, 5 * GB + KB, 6 * GB - KB, 6 * GB, 5 * GB + KB]), [false, true, true, true, false, false]);
        // replay and order
        let mut s = Saves::default();
        assert!(told(&s).is_empty() && s.disk().is_none(), "never read: nothing told");
        assert!(s.measured(ComputerDisk { total: 20 * GB, free: 3 * GB, at: T + 10, generation: 1 }));
        assert!(!s.measured(ComputerDisk { total: 20 * GB, free: 15 * GB, at: T + 5, generation: 1 }), "a late read of an earlier moment");
        assert_eq!(told(&s), vec![ComputerNotice::DiskFilling { total: 20 * GB, free: 3 * GB }]);
        assert!(s.measured(ComputerDisk { total: 20 * GB, free: 3 * GB, at: T + 10, generation: 1 }), "the same read again: the same");
        assert_eq!(told(&s).len(), 1);
        assert!(std::panic::catch_unwind(|| Saves::default().measured(ComputerDisk { total: GB, free: 2 * GB, at: T, generation: 1 })).is_err(), "more free than its size");
        // a read stored by a release before reads named their start: kept, its start 0
        let mut stored = serde_json::to_value(&s).unwrap();
        stored["disk"].as_object_mut().unwrap().remove("generation");
        let old: Saves = serde_json::from_value(stored).unwrap();
        assert_eq!(old.disk(), Some(ComputerDisk { total: 20 * GB, free: 3 * GB, at: T + 10, generation: 0 }));
        assert_eq!(told(&old), told(&s));
        // saves stored before the disk was read, or before the latch: none told
        let mut stored = serde_json::to_value(&s).unwrap();
        stored.as_object_mut().unwrap().remove("disk");
        stored.as_object_mut().unwrap().remove("diskTold");
        let old: Saves = serde_json::from_value(stored).unwrap();
        assert!(old.disk().is_none() && told(&old).is_empty());
    }

    /// Goal: after an event, a disk is read only once an awake save's
    /// `Saved` was applied; never after a start's `Ready` (a start reads its
    /// own in the start, before its guest runs: FIN-462), a late `Ready` of
    /// a start no longer under way, a sleep's save (its stop follows), nor
    /// any other event. Method: the lifecycle's steps, and what each says to
    /// read after.
    #[test]
    fn a_disk_is_read_after_an_awake_save_only() {
        let mut l = Lifecycle::new();
        l.apply(Event::Wake { why: Wake::Owner }, T);
        assert_eq!(reads_disk_after(&Event::Wake { why: Wake::Owner }, &l), None, "starting: nothing read");
        // the deadline passes first: the late Ready is no start's, and nothing is read
        let mut late = l.clone();
        late.apply(Event::Alarm, T + START_DEADLINE_MS);
        late.apply(Event::Ready { generation: 1 }, T + START_DEADLINE_MS + 1);
        assert_eq!(reads_disk_after(&Event::Ready { generation: 1 }, &late), None);
        // ready within it: awake, and nothing read after (its start read it)
        let s = l.apply(Event::Ready { generation: 1 }, T + START_DEADLINE_MS - 1);
        assert!(matches!(l.phase, Phase::Awake { generation: 1, .. }) && s.alarm_ms.is_some_and(|a| a > T + START_DEADLINE_MS - 1));
        assert_eq!(reads_disk_after(&Event::Ready { generation: 1 }, &l), None);
        // an awake save that worked: read after; another start's: not
        assert_eq!(reads_disk_after(&Event::Saved { generation: 1, seq: 1 }, &l), Some(1));
        assert_eq!(reads_disk_after(&Event::Saved { generation: 2, seq: 1 }, &l), None);
        // a sleep's save: its stop follows, nothing read
        l.apply(Event::Sleep, T + START_DEADLINE_MS + 10);
        assert_eq!(reads_disk_after(&Event::Saved { generation: 1, seq: 1 }, &l), None);
    }

    /// Goal: a disk read from `df -P -B1 /` as GNU's (the Hermes image) and
    /// BusyBox's (the stub) print it; anything else is no read. Valid,
    /// invalid (no header, no line, not numbers, free past its size, size 0).
    #[test]
    fn a_disk_is_read_from_df() {
        let gnu = "Filesystem          1-blocks          Used    Available Capacity Mounted on\noverlay        1998232485888 1450459369472 536433389568      74% /\n";
        let busybox = "Filesystem           1-blocks       Used Available Capacity Mounted on\noverlay              1998232485888 1450465140736 536427945984  73% /\n";
        assert_eq!(disk_of_df(gnu, T, 3), Some(ComputerDisk { total: 1_998_232_485_888, free: 536_433_389_568, at: T, generation: 3 }));
        assert_eq!(disk_of_df(busybox, T, 3).map(|d| d.free), Some(536_427_945_984));
        for bad in ["", "overlay 10 5 5 50% /", "Filesystem 1-blocks Used Available Capacity Mounted on", "Filesystem x\noverlay ten 5 five 50% /", "Filesystem x\noverlay 10 0 11 0% /", "Filesystem x\noverlay 0 0 0 0% /"] {
            assert_eq!(disk_of_df(bad, T, 3), None, "{bad:?}");
        }
    }
}
