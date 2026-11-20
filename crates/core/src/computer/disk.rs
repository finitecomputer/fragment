//! A computer's disk: read in its container, kept with its saves, and told to
//! its owner while it fills. What it is told is its room to restore
//! (docs/computers.md, "Room to restore"): a save is only good while a wake
//! can restore it, and a restore needs more of the disk than the running
//! computer used.

use fragment_proto::computer::{ComputerSizes, DiskRoom, SizeEntry};
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

/// The most of a computer's disk its image takes, as a fresh computer's
/// disk reads it (`df` as it starts, before anything is restored): its
/// files, written to the disk as its runtime unpacks them, and the disk's
/// own keep. An empty 20 GB btrfs disk keeps 0.55 GB for itself (its
/// metadata twice), and our image written to it with zstd took 2.41 GB
/// more: 2.99 GB in all (2026-11-20). Production's empty computers read
/// about 17.4 GB free, 2.6 GB used; the larger is counted. A restore of
/// `/data` comes on top of it.
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
/// `DISK_READ`, then `du` of `/data`, allocated (KiB) and apparent (bytes),
/// each line named, leaving out what that save left out (`left_out`, the
/// held guest's patterns, anchored to `/data`) as GNU's `du --exclude`
/// does: what is kept is measured, so a file hard-linked between a cache
/// left out and the work kept (uv's and pnpm's caches link into a project)
/// counts once, where it restores (Alex's review of #1421: subtracting the
/// caches' own `du` dropped it). A `du` with no `--exclude` (BusyBox's: the
/// stub) counts all of `/data`, more than restores. A pattern is a `du`
/// pattern here, anchored at `./`, `*` and `?` as in the save's (a `*` may
/// also cross a `/`: what it names counts as left out); one outside the
/// patterns' alphabet or with a `..` is not used, and what it names counts
/// as restored.
///
/// With them, what its owner's Storage shows (`sizes_of`): the folders it
/// keeps to `SIZES_DEPTH` below `/data`, allocated (KiB), biggest first
/// (the first is `/data`'s own, its total kept), and its files of at least
/// `SIZES_FILE_MIN_KIB` wherever they are (a cache's too), biggest first,
/// each at most as many as a summary holds and a line each. Three walks of
/// `/data`, after its save's backups read it all.
pub fn sizes_read(left_out: &[String]) -> String {
    let safe = |p: &&String| !p.is_empty() && p.len() <= LEFT_OUT_PATTERN_MAX_BYTES && p.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-/*?[]".contains(&b)) && !p.split('/').any(|s| s == "..");
    let exclude: String = left_out.iter().filter(safe).map(|p| p.trim_matches('/')).filter(|p| !p.is_empty()).map(|p| format!(" --exclude='./{p}'")).collect();
    let read = |ex: &str| {
        format!(
            "printf 'apparent ' && du -sxb{ex} . && echo folders && du -xk -d {SIZES_DEPTH}{ex} . | awk 'length < {line}' | sort -rn | head -n {} && echo files && find . -xdev -type f -size +{SIZES_FILE_MIN_KIB}k -exec du -k {{}} + 2>/dev/null | awk 'length < {line}' | sort -rn | head -n {SIZES_FILES_MAX}",
            SIZES_FOLDERS_MAX + 1,
            line = SIZE_LINE_MAX_BYTES
        )
    };
    if exclude.is_empty() {
        return format!("{DISK_READ} && cd /data && {}", read(""));
    }
    format!("{DISK_READ} && cd /data && if du -s --exclude=x /dev/null > /dev/null 2>&1; then {}; else {}; fi", read(&exclude), read(""))
}

/// How deep below `/data` a size summary lists its folders: an agent's
/// work (`work/<agent>`), its home and browser profile, and their caches
/// (`work/<agent>/home/.cache`).
pub const SIZES_DEPTH: u32 = 4;
/// The folders and the files a size summary lists at most, the biggest
/// first, and the least a file it lists holds (10 MiB, in KiB).
pub const SIZES_FOLDERS_MAX: usize = 64;
pub const SIZES_FILES_MAX: usize = 32;
pub const SIZES_FILE_MIN_KIB: u64 = 10 * 1024;
/// A path a size summary lists is at most this long (one longer is left
/// out), and its paths together at most this (the rest left out, smallest
/// first): its owner's view carries it whole.
pub const SIZE_PATH_MAX_BYTES: usize = 400;
pub const SIZES_PATHS_MAX_BYTES: usize = 16 * 1024;
/// A listing's line the container drops past (`awk`), its count, a tab
/// and `./` beside a path at most `SIZE_PATH_MAX_BYTES`; so `sizes_read`'s
/// whole answer is at most `SIZES_READ_MAX_BYTES`, which the cell keeps
/// of it (an exec keeps its answer's tail).
pub const SIZE_LINE_MAX_BYTES: usize = SIZE_PATH_MAX_BYTES + 24;
pub const SIZES_READ_MAX_BYTES: usize = 64 * 1024;
const _: () = assert!(512 + (SIZES_FOLDERS_MAX + 1 + SIZES_FILES_MAX) * SIZE_LINE_MAX_BYTES <= SIZES_READ_MAX_BYTES, "a summary's whole answer is kept");

/// The `N\t./path` lines of `sizes_read`'s answer after `section`'s line,
/// up to the next section's: KiB and the path below `/data` (`.` its own).
fn listed<'a>(out: &'a str, section: &'static str) -> impl Iterator<Item = (u64, &'a str)> {
    out.lines().skip_while(move |l| l.trim() != section).skip(1).take_while(|l| !matches!(l.trim(), "folders" | "files")).filter_map(|l| {
        let (n, path) = l.split_once('\t')?;
        Some((n.trim().parse::<u64>().ok()?, path))
    })
}

/// What its owner's Storage shows, from `sizes_read`'s answer read at
/// `at_ms`: its biggest folders below `/data` (its own total left out:
/// `disk_of_sizes`) and its biggest files, each in bytes allocated, a path
/// below `/data` a line, printable and at most `SIZE_PATH_MAX_BYTES`, as
/// many as `SIZES_FOLDERS_MAX` and `SIZES_FILES_MAX`, their paths
/// together at most `SIZES_PATHS_MAX_BYTES`. None for an answer with no
/// folders.
pub fn sizes_of(out: &str, at_ms: i64) -> Option<ComputerSizes> {
    if !out.lines().any(|l| l.trim() == "folders") {
        return None;
    }
    let mut room = SIZES_PATHS_MAX_BYTES;
    let mut take = |section: &'static str, max: usize| -> Vec<SizeEntry> {
        let mut got = Vec::new();
        for (kib, path) in listed(out, section) {
            let path = path.strip_prefix("./").unwrap_or(path);
            if path == "." || path.is_empty() || path.len() > SIZE_PATH_MAX_BYTES || path.chars().any(char::is_control) || got.len() == max || path.len() > room {
                continue;
            }
            room -= path.len();
            got.push(SizeEntry { path: path.to_string(), bytes: kib.saturating_mul(1024) });
        }
        got
    };
    let folders = take("folders", SIZES_FOLDERS_MAX);
    let files = take("files", SIZES_FILES_MAX);
    Some(ComputerSizes { at: at_ms, folders, files })
}

/// A computer's disk from `DISK_READ`'s answer, read at `at_ms` in start
/// `generation`'s container: the line
/// after its header, its size and what is available (to the guest's user,
/// as a restore's floor counts it). None for an answer it cannot read.
pub fn disk_of_df(out: &str, at_ms: i64, generation: u64) -> Option<ComputerDisk> {
    let mut lines = out.lines().filter(|l| !l.trim().is_empty());
    lines.next().filter(|h| h.starts_with("Filesystem"))?;
    let cols: Vec<&str> = lines.next()?.split_whitespace().collect();
    let (total, free) = (cols.get(1)?.parse::<u64>().ok()?, cols.get(3)?.parse::<u64>().ok()?);
    (total > 0 && free <= total).then_some(ComputerDisk { total, free, at: at_ms, generation, data: None })
}

/// A computer's disk from `sizes_read`'s answer: its `df` (`disk_of_df`),
/// and what a restore of its `/data` writes (`data`): the larger of what
/// it keeps allocated and apparent (a restore writes each file whole, a
/// sparse one's holes too, and a small one in blocks of its own). With no
/// `du` it reads, the `df` alone; with no `df`, none.
pub fn disk_of_sizes(out: &str, at_ms: i64, generation: u64) -> Option<ComputerDisk> {
    let disk = disk_of_df(out, at_ms, generation)?;
    let apparent = out.lines().find_map(|l| l.strip_prefix("apparent ")?.split_whitespace().next()?.parse::<u64>().ok());
    // `/data`'s own allocated total: the `.` of its folders
    let alloc_kib = listed(out, "folders").find(|(_, path)| *path == ".").map(|(kib, _)| kib);
    let (Some(alloc_kib), Some(apparent)) = (alloc_kib, apparent) else { return Some(disk) };
    Some(ComputerDisk { data: Some(alloc_kib.saturating_mul(1024).max(apparent)), ..disk })
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

    /// Goal: what a restore of `/data` writes, and what takes the disk,
    /// read after a save as the Hermes image's GNU `du` and the stub's
    /// BusyBox `du` answer `sizes_read` (both answers the images' own,
    /// 2026-11-20: a 20 MiB video, a 12 MiB file in a uv cache left out and
    /// linked into a venv, a 4 MiB file in that cache alone): the larger of
    /// allocated and apparent of what the save keeps, GNU's `--exclude`
    /// counting the venv's link and not the cache (Alex's review of
    /// #1421), BusyBox counting all; the biggest folders and files, paths
    /// below `/data`. Valid, small files, invalid (no `du`, no `df`, an
    /// unsafe pattern, which is never used, a path past its bound, a
    /// control character, too many).
    #[test]
    fn data_is_read_from_du() {
        let gnu = GNU_ANSWER;
        let busybox = BUSYBOX_ANSWER;
        assert_eq!(disk_of_sizes(gnu, T, 2).and_then(|d| d.data), Some(32_772 * 1024), "the venv's 12 MiB and the video, not the cache");
        assert_eq!(disk_of_sizes(busybox, T, 2).and_then(|d| d.data), Some(36_868 * 1024), "all of it");
        assert_eq!(disk_of_sizes(gnu, T, 2).map(|d| (d.total, d.free, d.generation)), Some((1_998_232_485_888, 237_187_936_256, 2)));
        let sizes = sizes_of(gnu, T).expect("a summary");
        let named = |l: &[SizeEntry]| l.iter().map(|e| (e.path.clone(), e.bytes)).collect::<Vec<_>>();
        assert_eq!(sizes.at, T);
        assert_eq!(named(&sizes.folders)[..4], [("work/juniper".to_string(), 32_768 * 1024), ("work".to_string(), 32_768 * 1024), ("work/juniper/proj/.venv".to_string(), 12_288 * 1024), ("work/juniper/proj".to_string(), 12_288 * 1024)]);
        assert_eq!(sizes.folders.len(), 7, "/data's own total is the disk's, not a folder");
        assert_eq!(named(&sizes.files), [("work/juniper/talk.mp4".to_string(), 20_480 * 1024), ("work/juniper/home/.cache/uv/pkg.bin".to_string(), 12_288 * 1024)], "a cache's big file is listed too");
        // small files: allocated is more than apparent
        let small = "Filesystem x\noverlay 100 50 50 50% /\napparent 1000\t.\nfolders\n100\t.\nfiles\n";
        assert_eq!(disk_of_sizes(small, T, 2).and_then(|d| d.data), Some(102_400));
        assert_eq!(sizes_of(small, T).map(|s| (s.folders.len(), s.files.len())), Some((0, 0)));
        // no du: the df alone, and no summary; no df: none
        assert_eq!(disk_of_sizes("Filesystem x\noverlay 100 50 50 50% /\n", T, 2).map(|d| d.data), Some(None));
        assert_eq!(sizes_of("Filesystem x\noverlay 100 50 50 50% /\n", T), None);
        assert_eq!(disk_of_sizes("Filesystem x\noverlay 100 50 50 50% /\napparent 10\t.\nfolders\nx\t.\nfiles\n", T, 2).map(|d| d.data), Some(None));
        assert_eq!(disk_of_sizes("apparent 10\t.\nfolders\n1\t.\n", T, 2), None);
        // bounds: a path too long, a control character, too many, all paths together
        let long = "a".repeat(SIZE_PATH_MAX_BYTES + 1);
        let odd = format!("Filesystem x\noverlay 100 50 50 50% /\napparent 1\t.\nfolders\n9\t./{long}\n8\t./a\u{7}b\n7\t./ok\nfiles\n");
        assert_eq!(sizes_of(&odd, T).map(|s| named(&s.folders)), Some(vec![("ok".to_string(), 7 * 1024)]));
        let many: String = (0..SIZES_FOLDERS_MAX + 10).map(|i| format!("{}\t./f{i}\n", 1000 - i)).collect();
        let many = sizes_of(&format!("folders\n{many}files\n"), T).expect("a summary");
        assert_eq!((many.folders.len(), many.folders[0].path.as_str()), (SIZES_FOLDERS_MAX, "f0"), "the biggest, as many as it holds");
        let wide: String = (0..60).map(|i| format!("{}\t./{}{i:02}\n", 1000 - i, "d".repeat(SIZE_PATH_MAX_BYTES - 2))).collect();
        let wide = sizes_of(&format!("folders\n{wide}files\n"), T).expect("a summary");
        assert!(wide.folders.iter().map(|e| e.path.len()).sum::<usize>() <= SIZES_PATHS_MAX_BYTES && wide.folders.len() == SIZES_PATHS_MAX_BYTES / SIZE_PATH_MAX_BYTES, "its paths within their bound: {}", wide.folders.len());
        // the read itself: nothing left out, all of /data
        let all = "printf 'apparent ' && du -sxb . && echo folders && du -xk -d 4 . | awk 'length < 424' | sort -rn | head -n 65 && echo files && find . -xdev -type f -size +10240k -exec du -k {} + 2>/dev/null | awk 'length < 424' | sort -rn | head -n 32";
        assert_eq!(sizes_read(&[]), format!("df -P -B1 / && cd /data && {all}"));
        // the save's patterns as du's excludes, anchored, quoted; never an unsafe one; BusyBox's du counts all
        let read = sizes_read(&["/work/*/home/.cache/uv/".into(), "/hermes/state.db".into(), "/../etc/".into(), "/a b/".into(), "/x;rm/".into(), "/x'y/".into(), "/".into(), String::new()]);
        let ex = " --exclude='./work/*/home/.cache/uv' --exclude='./hermes/state.db'";
        let kept = format!("printf 'apparent ' && du -sxb{ex} . && echo folders && du -xk -d 4{ex} . | awk 'length < 424' | sort -rn | head -n 65 && echo files && find . -xdev -type f -size +10240k -exec du -k {{}} + 2>/dev/null | awk 'length < 424' | sort -rn | head -n 32");
        assert_eq!(read, format!("df -P -B1 / && cd /data && if du -s --exclude=x /dev/null > /dev/null 2>&1; then {kept}; else {all}; fi"));
        assert_eq!(sizes_read(&["/../etc/".into(), "/".into()]), format!("df -P -B1 / && cd /data && {all}"), "no pattern it uses: all");
    }

    /// `sizes_read`'s answer from the Hermes image (GNU), 2026-11-20.
    const GNU_ANSWER: &str = "Filesystem          1-blocks          Used    Available Capacity Mounted on\noverlay        1998232485888 1739559915520 237187936256      89% /\napparent 33554435\t.\nfolders\n32772\t.\n32768\t./work/juniper\n32768\t./work\n12288\t./work/juniper/proj/.venv\n12288\t./work/juniper/proj\n4\t./hermes\n0\t./work/juniper/home/.cache\n0\t./work/juniper/home\nfiles\n20480\t./work/juniper/talk.mp4\n12288\t./work/juniper/home/.cache/uv/pkg.bin\n";
    /// The same from the stub (BusyBox).
    const BUSYBOX_ANSWER: &str = "Filesystem           1-blocks       Used Available Capacity Mounted on\noverlay              1998232485888 1739560046592 237187936256  88% /\napparent 37748903\t.\nfolders\n36868\t.\n36864\t./work/juniper\n36864\t./work\n16384\t./work/juniper/home/.cache\n16384\t./work/juniper/home\n4\t./hermes\n0\t./work/juniper/proj/.venv\n0\t./work/juniper/proj\nfiles\n20480\t./work/juniper/talk.mp4\n12288\t./work/juniper/home/.cache/uv/pkg.bin\n";
}
