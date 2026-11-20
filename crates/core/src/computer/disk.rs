//! A computer's disk: read in its container, kept with its saves, and told to
//! its owner while it fills. What it is told is its room to restore
//! (docs/computers.md, "Room to restore"): a save is only good while a wake
//! can restore it, and a restore needs more of the disk than the running
//! computer used.

use fragment_proto::computer::DiskRoom;
use serde::{Deserialize, Serialize};

use crate::computer::held::LEFT_OUT_PATTERN_MAX_BYTES;
use crate::computer::lifecycle::{Event, Lifecycle, Phase};

/// A restore stops once its disk has less than this free (the Sandbox SDK's
/// `sandbox-shim` 1.0.0, `Extractor::check_floor`: a twentieth of the disk,
/// at most 256 MiB), checked every 8 MiB or 1,024 entries it writes: on a
/// computer's 20 GB disk, 256 MiB. A start whose restore stops so fails, and
/// so does each start after it (2026-11-20: a computer that ran with 2.1 GB
/// free could not restore its save). The shim's own number, never a file's
/// bound (proto's `limits`), whatever it happens to equal.
pub const RESTORE_FLOOR_MAX: u64 = 1 << 28;

/// The restore's floor on a disk of `total` bytes (`RESTORE_FLOOR_MAX`).
pub fn restore_floor(total: u64) -> u64 {
    (total / 20).min(RESTORE_FLOOR_MAX)
}

/// The most of a computer's disk an image may take, as a fresh computer's
/// disk reads it (`df` as it starts, before anything is restored): its
/// files, written to the disk as its runtime unpacks them, and the disk's
/// own keep. Every image is held to it (the Docker lane's
/// `the_hermes_image_fits_its_disk_budget`, images/bridge/tests/docker.rs),
/// so a computer's room (`room`) counts it, whichever image the deployment
/// moves it to (Paul, 2026-11-20: "our image update either never goes up
/// in size, or that it's safe for all existing users somehow"). Today's
/// image measured 2.53 to 2.67 GB of a fresh 20 GB btrfs disk with zstd in
/// five runs on a quiet box (0.55 GB of it the disk's own keep: its
/// metadata twice), and 2.99 GB once under heavy disk load, when btrfs
/// compresses less as it writes back in a hurry; production's empty
/// computers read about 2.6 GB used (2026-11-20). The budget is that plus
/// a little.
///
/// Raised only on Paul's word, recorded here: each GB raised is a GB less
/// of every person's files that restore.
pub const IMAGE_DISK_MAX: u64 = 3_000_000_000;

/// What a restore writes past `/data`'s size as `du` reads it, at most,
/// in hundredths: a big file's checksums and extents (a 17 GB restore
/// stopped 43 MB short of what `du` predicted, 0.26%).
pub const RESTORE_SLACK_PCT: u64 = 1;

/// What a restore of a `/data` of `data` bytes (`du`'s, `disk_of_sizes`)
/// writes, at most (`RESTORE_SLACK_PCT` more).
pub fn restore_writes(data: u64) -> u64 {
    data.saturating_add(data / 100 * RESTORE_SLACK_PCT)
}

/// Its owner and its agents are told its disk is filling once it has at
/// most this much room (`room`): on a 20 GB disk, at about 13.6 GB of a
/// person's files at their full size. Room enough that a save still restores, with
/// time to move big files to storage.
pub const ROOM_TOLD_MAX: u64 = 3_000_000_000;

/// Told, it is told no more only once a read finds at least this much room.
/// The gap above where it is told keeps a disk that hovers at the edge (a
/// read a few KB either side) from showing and clearing its notice in turn.
pub const ROOM_CLEAR_MIN: u64 = 4_000_000_000;
const _: () = assert!(ROOM_TOLD_MAX < ROOM_CLEAR_MIN && ROOM_TOLD_MAX > RESTORE_FLOOR_MAX, "it clears only past where it is told, and is told well before a restore's floor");

/// The room a computer's disk has left (docs/computers.md, "Room to
/// restore"): the least of what is free now (a write that fails fails now)
/// and what a restore of its `/data` as last read (`data`) would leave
/// beside the largest image (`IMAGE_DISK_MAX`; `restore_writes`), each
/// less a restore's floor (`restore_floor`). A read with no `data` (before
/// the first read after a save) is what is free alone. At 0, its newest
/// save may no longer restore.
pub fn room(d: &ComputerDisk) -> u64 {
    let floor = restore_floor(d.total);
    let now = d.free.saturating_sub(floor);
    match d.data {
        Some(data) => now.min(d.total.saturating_sub(IMAGE_DISK_MAX.saturating_add(floor).saturating_add(restore_writes(data)))),
        None => now,
    }
}

/// Whether a save of a `/data` as `d` last read (`data`) restores beside
/// the largest image the deployment may move it to (`IMAGE_DISK_MAX`),
/// with the restore's floor left: none read yet says yes, as before reads.
pub fn fits_any_image(d: &ComputerDisk) -> bool {
    d.data.is_none_or(|data| IMAGE_DISK_MAX.saturating_add(restore_floor(d.total)).saturating_add(restore_writes(data)) <= d.total)
}

/// Whether a disk as `d` read has at most `ROOM_TOLD_MAX` of room: its
/// owner and its agents are told.
pub fn disk_filling(d: &ComputerDisk) -> bool {
    room(d) <= ROOM_TOLD_MAX
}

/// Whether a disk told filling has clearly more room now: at least
/// `ROOM_CLEAR_MIN`.
pub fn disk_clear(d: &ComputerDisk) -> bool {
    room(d) >= ROOM_CLEAR_MIN
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

/// What reads a computer's disk in a start, run in its container
/// (`disk_of_df` reads its answer): POSIX `df` in bytes, which GNU's and
/// BusyBox's both print.
pub const DISK_READ: &str = "df -P -B1 /";

/// What reads a computer's disk and what its `/data` restores to after an
/// awake save, run in its container (`disk_of_sizes` reads its answer):
/// `DISK_READ`, then a line for each record its save takes, measured as
/// that save takes it (cell/src/computer.rs, `save`): `home` (`/data` but
/// its work, less what its held guest named: `left_out`, anchored to
/// `/data`) and `work` (`/data/work`, less what it named under its work:
/// `work_left_out`), each allocated (KiB) and apparent (bytes). Measured
/// apart, a file hard-linked between the two counts in each, as each record
/// restores its own copy, and a guest that names its whole work leaves out
/// nothing of it, as its work's record does (Codex on #1421). A left-out
/// path is left out of the walk as GNU's `du --exclude` does, so a file
/// linked from a cache into the work kept counts where it restores (Alex's
/// review of #1421). A `du` with no `--exclude` (BusyBox's: the stub) gives
/// `all`, every byte of `/data`. A step that fails prints its line with no
/// number, never another's (`disk_of_sizes` reads none then). A pattern
/// is a `du` pattern, anchored at `./`, `*` and `?` as in the save's (a `*`
/// may also cross a `/`: what it names counts as left out); one outside the
/// patterns' alphabet or with a `..` is not used, and what it names counts
/// as restored.
pub fn sizes_read(left_out: &[String]) -> String {
    let safe = |p: &&String| !p.is_empty() && p.len() <= LEFT_OUT_PATTERN_MAX_BYTES && p.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-/*?[]".contains(&b)) && !p.split('/').any(|s| s == "..");
    let exclude = |patterns: Vec<String>| -> String { patterns.iter().filter(safe).map(|p| p.trim_matches('/')).filter(|p| !p.is_empty()).map(|p| format!(" --exclude='./{p}'")).collect() };
    let home = exclude(std::iter::once("/work/".to_string()).chain(left_out.iter().cloned()).collect());
    let work = exclude(crate::computer::held::work_left_out(left_out));
    let measured = |ex: &str| format!("$(du -sxk{ex} . 2>/dev/null | cut -f1) $(du -sxb{ex} . 2>/dev/null | cut -f1)");
    format!(
        "{DISK_READ}; if du -s --exclude=x /dev/null > /dev/null 2>&1; then cd /data && echo \"home {}\"; {{ cd /data/work && echo \"work {}\"; }} || echo work; else cd /data && echo \"all {}\"; fi",
        measured(&home),
        measured(&work),
        measured("")
    )
}

/// A computer's disk from `DISK_READ`'s answer, read at `at_ms` in start
/// `generation`'s container: the line
/// after its header, its size and what is available (to the guest's user,
/// as a restore's floor counts it). None for an answer it cannot read.
pub fn disk_of_df(out: &str, at_ms: i64, generation: u64) -> Option<ComputerDisk> {
    let mut lines = out.lines().filter(|l| !l.trim().is_empty()).skip_while(|l| !l.starts_with("Filesystem"));
    lines.next()?;
    let cols: Vec<&str> = lines.next()?.split_whitespace().collect();
    let (total, free) = (cols.get(1)?.parse::<u64>().ok()?, cols.get(3)?.parse::<u64>().ok()?);
    (total > 0 && free <= total).then_some(ComputerDisk { total, free, at: at_ms, generation, data: None })
}

/// A computer's disk from `sizes_read`'s answer: its `df` (`disk_of_df`),
/// and what a restore of its `/data` writes (`data`): for each record, the
/// larger of its allocated and its apparent size (a restore writes each
/// file whole, a sparse one's holes too, and a small one in blocks of its
/// own), summed; or `all`'s. A record whose measure is missing leaves
/// `data` unread (the last one is kept: `Saves::measured`), never a part
/// of it. With no `df`, none.
pub fn disk_of_sizes(out: &str, at_ms: i64, generation: u64) -> Option<ComputerDisk> {
    let disk = disk_of_df(out, at_ms, generation)?;
    let record = |name: &str| -> Option<u64> {
        let line = out.lines().find_map(|l| l.strip_prefix(name)?.strip_prefix(' '))?;
        let mut n = line.split_whitespace().map(|w| w.parse::<u64>().ok());
        let (kib, bytes) = (n.next()??, n.next()??);
        Some(kib.saturating_mul(1024).max(bytes))
    };
    let data = match record("all") {
        Some(all) => Some(all),
        None => record("home").zip(record("work")).map(|(h, w)| h.saturating_add(w)),
    };
    Some(ComputerDisk { data, ..disk })
}

/// A computer's disk, as a read found it (`disk_of_df`, `disk_of_sizes`):
/// its size and what is free, in bytes; when it was read, and the start
/// whose container it read (0 in a read stored before it was kept); and
/// what a restore of its `/data` writes, as the newest read after a save
/// found it (`data`: a start's read keeps the one before, `Saves::measured`;
/// none before the first, or in a read stored before it was kept).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputerDisk {
    pub total: u64,
    pub free: u64,
    pub at: i64,
    #[serde(default)]
    pub generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<u64>,
}

impl ComputerDisk {
    /// As its view shows it, told filling or not (`filling`).
    pub fn view(&self, filling: bool) -> DiskRoom {
        DiskRoom { total: self.total, free: self.free, data: self.data, room: room(self), filling, at: self.at }
    }
}

#[cfg(test)]
mod tests {
    use fragment_proto::computer::ComputerNotice;

    use super::*;
    use crate::computer::testing::T;
    use crate::computer::*;

    const GB: u64 = 1_000_000_000;
    /// A computer's disk, as production's `df` reads it.
    const DISK: u64 = 20_000_002_048;
    const FLOOR: u64 = RESTORE_FLOOR_MAX;

    fn read(total: u64, free: u64, data: Option<u64>, at: i64) -> ComputerDisk {
        ComputerDisk { total, free, at, generation: 1, data }
    }

    /// Goal (2026-11-20: a computer ran with 2.1 GB free, saved, and its
    /// restore stopped at the shim's floor, every start after it too): the
    /// room a disk has is what a restore of its `/data` leaves beside the
    /// largest image, less the restore's floor, or what is free less the
    /// floor, whichever is less, never what `df` says alone. Valid (each
    /// term the least, exactly at the edges), the incident's numbers,
    /// invalid (more than the disk: no room, never an overflow), and a
    /// host's shared disk.
    #[test]
    fn room_is_what_a_restore_leaves() {
        assert_eq!(restore_floor(DISK), 1024 * 1024 * 256, "a computer's disk: 256 MiB");
        assert_eq!(restore_floor(2 * GB), 100_000_000, "a small disk: a twentieth");
        assert_eq!(restore_floor(0), 0);
        // no read of /data yet: what is free, less the floor
        assert_eq!(room(&read(DISK, 3 * GB, None, T)), 3 * GB - FLOOR);
        assert_eq!(room(&read(DISK, FLOOR - 1, None, T)), 0, "below the floor: none");
        // /data read: what its restore writes (1% past du) leaves beside the largest image
        assert_eq!(restore_writes(14 * GB), 14_140_000_000);
        assert_eq!(restore_writes(99), 99, "under 100 bytes, nothing past it");
        let left = DISK - IMAGE_DISK_MAX - FLOOR - 14_140_000_000;
        assert_eq!(room(&read(DISK, 10 * GB, Some(14 * GB), T)), left, "compressed while running, 10 GB free: a restore of 14 GB leaves 2.6 GB");
        assert_eq!(room(&read(DISK, left + FLOOR - 1, Some(14 * GB), T)), left - 1, "what is free now, when less");
        // the incident, as the bench made it (2026-11-20): 2.1 GB free
        // running, du 16.8 GB; its restore stopped 6.7 MB short of the floor
        let incident = read(DISK, 2_099_998_720, Some(16_812_892_160), T);
        assert_eq!(room(&incident), 0, "its save would not restore");
        assert!(disk_filling(&incident));
        assert_eq!(room(&read(DISK, 2_100_000_000, None, T)), 2_100_000_000 - FLOOR, "what df alone said: room it never had");
        // more than the disk holds: no room, no overflow
        assert_eq!(room(&read(DISK, DISK, Some(u64::MAX), T)), 0);
        assert_eq!(room(&read(u64::MAX, u64::MAX, Some(u64::MAX), T)), 0);
        // a host's shared disk (a dev box's, CI's): room to spare
        assert!(!disk_filling(&read(2_000 * GB, 300 * GB, Some(5 * GB), T)));
        // told at ROOM_TOLD_MAX, cleared at ROOM_CLEAR_MIN, exactly
        let at = |room: u64| read(DISK, room + FLOOR, None, T);
        assert!(disk_filling(&at(ROOM_TOLD_MAX)) && !disk_filling(&at(ROOM_TOLD_MAX + 1)), "told at 3 GB of room, a byte more is not");
        assert!(disk_clear(&at(ROOM_CLEAR_MIN)) && !disk_clear(&at(ROOM_CLEAR_MIN - 1)), "clear at 4 GB");
        // its view says the same
        let v = read(DISK, 10 * GB, Some(14 * GB), T).view(true);
        assert_eq!((v.room, v.filling, v.data, v.total, v.free), (left, true, Some(14 * GB), DISK, 10 * GB));
    }

    /// Goal (2026-11-03: a disk that filled left a computer that could not
    /// start; 2026-11-20: one whose save no longer fit): its owner is told
    /// its disk is filling once a read finds at most `ROOM_TOLD_MAX` of
    /// room, and no more only once a read finds at least `ROOM_CLEAR_MIN`:
    /// a disk hovering at either edge, reads a few KB apart, never shows and
    /// clears it in turn (Codex on #1015). A start's read (no `du`) keeps
    /// the newest read of `/data`. Valid (each edge, exactly), oscillation
    /// around both edges, replay and order (a read of an earlier moment
    /// changes nothing), invalid (more free than its size is refused),
    /// saves stored before (none told).
    #[test]
    fn a_disk_filling_is_told_until_a_read_finds_room() {
        const KB: u64 = 8_192;
        let told = |s: &Saves| notices(&Lifecycle::new(), s, &Images::default(), &Rules::default());
        // a run of reads, each a moment later, with the room each leaves
        // (what is free less the floor: a start's read), and whether it
        // is told after each
        let run = |rooms: &[u64]| -> Vec<bool> {
            let mut s = Saves::default();
            rooms.iter().enumerate().map(|(i, room)| {
                assert!(s.measured(read(DISK, room + FLOOR, None, T + i as i64)));
                matches!(told(&s).as_slice(), [ComputerNotice::DiskFilling { total, free, room: r }] if *total == DISK && *free == room + FLOOR && r == room)
            }).collect()
        };
        // around the told edge: shown, and it stays until 4 GB of room
        assert_eq!(run(&[3 * GB + KB, 3 * GB, 3 * GB + KB, 3 * GB, 3 * GB + 500_000_000, 4 * GB - 1, 4 * GB, 4 * GB - KB, 3 * GB + KB, 3 * GB]), [false, true, true, true, true, true, false, false, false, true]);
        // around the cleared edge: once clear, never told again until the told edge
        assert_eq!(run(&[3 * GB, 4 * GB, 4 * GB - KB, 4 * GB, 4 * GB - KB, 3 * GB + KB]), [true, false, false, false, false, false]);
        // a start's read keeps /data's size from the read after the save
        let mut s = Saves::default();
        assert!(s.measured(read(DISK, 10 * GB, Some(15 * GB), T)));
        assert!(s.measured(read(DISK, 11 * GB, None, T + 1)));
        assert_eq!(s.disk().map(|d| (d.free, d.data)), Some((11 * GB, Some(15 * GB))), "kept, its free from the new read");
        assert_eq!(told(&s), vec![ComputerNotice::DiskFilling { total: DISK, free: 11 * GB, room: DISK - IMAGE_DISK_MAX - FLOOR - restore_writes(15 * GB) }]);
        // replay and order
        let mut s = Saves::default();
        assert!(told(&s).is_empty() && s.disk().is_none(), "never read: nothing told");
        assert!(s.measured(read(DISK, 3 * GB, None, T + 10)));
        assert!(!s.measured(read(DISK, 15 * GB, None, T + 5)), "a late read of an earlier moment");
        assert_eq!(told(&s), vec![ComputerNotice::DiskFilling { total: DISK, free: 3 * GB, room: 3 * GB - FLOOR }]);
        assert!(s.measured(read(DISK, 3 * GB, None, T + 10)), "the same read again: the same");
        assert_eq!(told(&s).len(), 1);
        assert!(std::panic::catch_unwind(|| Saves::default().measured(read(GB, 2 * GB, None, T))).is_err(), "more free than its size");
        // a read stored by a release before reads named their start or /data's size: kept, its start 0
        let mut stored = serde_json::to_value(&s).unwrap();
        stored["disk"].as_object_mut().unwrap().remove("generation");
        let old: Saves = serde_json::from_value(stored).unwrap();
        assert_eq!(old.disk(), Some(ComputerDisk { total: DISK, free: 3 * GB, at: T + 10, generation: 0, data: None }));
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
        assert_eq!(disk_of_df(gnu, T, 3), Some(ComputerDisk { total: 1_998_232_485_888, free: 536_433_389_568, at: T, generation: 3, data: None }));
        assert_eq!(disk_of_df(busybox, T, 3).map(|d| d.free), Some(536_427_945_984));
        for bad in ["", "overlay 10 5 5 50% /", "Filesystem 1-blocks Used Available Capacity Mounted on", "Filesystem x\noverlay ten 5 five 50% /", "Filesystem x\noverlay 10 0 11 0% /", "Filesystem x\noverlay 0 0 0 0% /"] {
            assert_eq!(disk_of_df(bad, T, 3), None, "{bad:?}");
        }
    }

    /// Goal: what a restore of `/data` writes, read after a save as the
    /// Hermes image's GNU `du` and the stub's BusyBox `du` answer it (both
    /// answers the images' own, 2026-11-20, for an 8 MiB file hard-linked
    /// between /data/hermes and the work, a uv cache left out holding a
    /// 4 MiB file linked into a venv and a 2 MiB file of its own, and a
    /// copied state.db left out): each record as its save takes it, so the
    /// link across the records counts in each (each restores its copy) and
    /// the venv's link counts though its cache is left out (Codex and Alex
    /// on #1421), BusyBox all of it once. Valid, a sparse file and small
    /// files, a record unread (none of data, never a part), invalid (no
    /// `df`, an unsafe pattern, which is never used, a guest naming its
    /// whole work, which its work's record keeps).
    #[test]
    fn data_is_read_from_du() {
        let gnu = "Filesystem          1-blocks          Used    Available Capacity Mounted on\noverlay        1998232485888 1771345412096 205765574656      90% /\nhome 8192 8388608\nwork 12288 12582912\n";
        let busybox = "Filesystem           1-blocks       Used Available Capacity Mounted on\noverlay              1998232485888 1771345223680 205765894144  90% /\nall 14340 14680254\n";
        assert_eq!(disk_of_sizes(gnu, T, 2).and_then(|d| d.data), Some((8192 + 12288) * 1024), "the link in each record, the venv's link, not the cache");
        assert_eq!(disk_of_sizes(busybox, T, 2).and_then(|d| d.data), Some(14_680_254.max(14340 * 1024)), "all of it, once");
        assert_eq!(disk_of_sizes(gnu, T, 2).map(|d| (d.total, d.free, d.generation)), Some((1_998_232_485_888, 205_765_574_656, 2)));
        // a sparse file: apparent past allocated; small files: allocated past apparent
        let df = "Filesystem x\noverlay 100 50 50 50% /\n";
        assert_eq!(disk_of_sizes(&format!("{df}home 4 9000000\nwork 100 1000\n"), T, 2).and_then(|d| d.data), Some(9_000_000 + 102_400));
        // a record unread: no data (the last one is kept), never a part of it; no df: none
        for unread in ["home 4 10\n", "home 4 10\nwork\n", "home 4 10\nwork 1\n", "work 4 10\n", "home x 10\nwork 1 1\n", "all \n", ""] {
            assert_eq!(disk_of_sizes(&format!("{df}{unread}"), T, 2).map(|d| d.data), Some(None), "{unread:?}");
        }
        assert_eq!(disk_of_sizes("home 4 10\nwork 4 10\n", T, 2), None);
        assert_eq!(disk_of_sizes(&format!("noise\n{df}all 1 1\n"), T, 2).map(|d| d.data), Some(Some(1024)), "df's header anywhere");
        // the read itself: each record measured as its save takes it, BusyBox's all
        let read = |home: &str, work: &str| {
            let m = |ex: &str| format!("$(du -sxk{ex} . 2>/dev/null | cut -f1) $(du -sxb{ex} . 2>/dev/null | cut -f1)");
            format!("df -P -B1 /; if du -s --exclude=x /dev/null > /dev/null 2>&1; then cd /data && echo \"home {}\"; {{ cd /data/work && echo \"work {}\"; }} || echo work; else cd /data && echo \"all {}\"; fi", m(home), m(work), m(""))
        };
        assert_eq!(sizes_read(&[]), read(" --exclude='./work'", ""));
        assert_eq!(
            sizes_read(&["/work/*/home/.cache/uv/".into(), "/hermes/state.db".into(), "/../etc/".into(), "/a b/".into(), "/x;rm/".into(), "/x'y/".into(), String::new()]),
            read(" --exclude='./work' --exclude='./work/*/home/.cache/uv' --exclude='./hermes/state.db'", " --exclude='./*/home/.cache/uv'"),
            "the home leaves out its work and all named; the work what was named under it; never an unsafe pattern"
        );
        assert_eq!(sizes_read(&["/work/".into()]), read(" --exclude='./work' --exclude='./work'", ""), "a guest naming its whole work: its work's record keeps it all, and so does its measure");
    }
}
