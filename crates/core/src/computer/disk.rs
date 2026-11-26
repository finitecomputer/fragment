//! A computer's disk: read in its container, kept with its saves, and told to
//! its owner while it fills. What it is told is its room to restore
//! (docs/computers.md, "Room to restore"): a save is only good while a wake
//! can restore it, and a restore needs more of the disk than the running
//! computer used.

use fragment_proto::computer::{ComputerSizes, DiskRoom, KindSize, SizeEntry, StorageKind};
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

/// Whether a restore's failure `why` is for want of room on its disk, by
/// its words, and nothing else: the sandbox runtime's guard, which stops a
/// restore that would leave less than its floor free ("restore stopped
/// with ~203 MB free, below the 268435456 byte floor": production,
/// 2026-11-20), or its disk filling under it, `ENOSPC` (the cell's host
/// names a restore's `SandboxFileError` by its code) or the system's words
/// for it. A restore that failed any other way (a transfer, a timeout, an
/// archive gone) is not.
pub fn restore_found_no_room(why: &str) -> bool {
    let guard = why.contains("restore stopped with") && why.contains("free, below the") && why.contains("byte floor");
    guard || why.contains("ENOSPC") || why.contains("No space left on device")
}

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

/// `DISK_READ` on a computer's disk (docs/computers.md, "A disk"): its
/// `/data` is a filesystem of its own, not the container's.
pub const DISK_READ_ON_DISK: &str = "df -P -B1 /data";

/// A read (`DISK_READ`, or `sizes_read`'s, which runs it first) of a
/// computer on a disk: its `df` of `/data`, the rest the same.
pub fn read_on_disk(read: &str) -> String {
    let at = read.find(DISK_READ).expect("a disk read runs its df");
    let after = &read[at + DISK_READ.len()..];
    assert!(after.is_empty() || after.starts_with(';'), "its df's path ends there");
    format!("{}{DISK_READ_ON_DISK}{after}", &read[..at])
}

/// What reads a computer's disk and what its `/data` restores to after an
/// awake save, run in its container (`disk_of_sizes` reads its answer):
/// `DISK_READ`, then a line for each record its save takes, measured as
/// that save takes it (cell/src/computer.rs, `save`): `home` (`/data` but
/// its work, less what its held guest named outside it: `left_out`,
/// anchored to `/data`) and `work` (`/data/work`, less what it named under
/// its work: `work_left_out`), each allocated (KiB) and apparent (bytes).
/// Measured apart, a file hard-linked between the two counts in each, as
/// each record restores its own copy, and a guest that names its whole
/// work leaves out nothing of it, as its work's record does (Codex on
/// #1421). What a pattern names is left out of the walk (GNU's `du
/// --exclude`, each path that exists), so a file linked from a cache into
/// the work kept counts where it restores (Alex's review of #1421). The
/// shell finds those paths (`m`), where a `*` or a `?` never crosses a `/`
/// nor matches a name's leading `.`: the save's patterns are gitignore's,
/// whose `*` is one name too (GNU's own `--exclude` would let a `*` cross
/// a `/`, leaving out a folder the save keeps: Codex on #1421), and every
/// way the two differ leaves out less, counting more. Each pattern is in
/// the command once, so a guest's largest answer fits one exec's argument
/// (`EXEC_ARG_MAX_BYTES`). A `du` with no `--exclude` (BusyBox's: the stub)
/// gives `all`, every byte of `/data`. A step that fails prints its line
/// with no number, never another's (`disk_of_sizes` reads none then). A
/// pattern outside the patterns' alphabet or with a `..` is not used, and
/// what it names counts as restored.
///
/// After them, what its owner's Storage shows (`sizes_of`): the folders
/// of all of `/data`, its caches too, to `SIZES_DEPTH` below it, allocated
/// (KiB), biggest first (`/data`'s own, `.`, among them), and its files of
/// at least `SIZES_FILE_MIN_KIB` wherever they are, biggest first, each at
/// most as many as a summary holds, a line each: GNU's `du -0` frames each,
/// so a newline in a name reads as `?` and forges no line (Codex on
/// #1426); a line past its bound is dropped, in bytes; each walk ends by
/// `SIZES_LISTED_BY_S` from the read's start; its errors are dropped. The
/// df and records come first, so the first of each line is theirs, never
/// one the listing printed, and so the whole answer is within what the cell
/// keeps of it.
pub fn sizes_read(left_out: &[String]) -> String {
    let safe = |p: &&String| !p.is_empty() && p.len() <= LEFT_OUT_PATTERN_MAX_BYTES && p.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-/*?[]".contains(&b)) && !p.split('/').any(|s| s == "..");
    // each a word the shell expands as a glob (the alphabet holds no space,
    // quote or `$`), relative to the record's folder
    let globs = |patterns: Vec<String>| -> String { patterns.iter().filter(safe).map(|p| p.trim_matches('/')).filter(|p| !p.is_empty()).map(|p| format!(" ./{p}")).collect() };
    // the home's record leaves out its work whole: a pattern under it is the work's
    let home = globs(std::iter::once("/work/".to_string()).chain(left_out.iter().filter(|p| !p.starts_with("/work/")).cloned()).collect());
    let work = globs(crate::computer::held::work_left_out(left_out));
    // `m <paths>`: the folder's measure less each path that exists
    let m = "m() { n=$#; for p; do [ -e \"$p\" ] && set -- \"$@\" \"--exclude=$p\"; done; shift $n; echo \"$(du -sxk \"$@\" . 2>/dev/null | cut -f1) $(du -sxb \"$@\" . 2>/dev/null | cut -f1)\"; }";
    let records = format!(
        "{DISK_READ}; if du -s --exclude=x /dev/null > /dev/null 2>&1; then {m}; cd /data && echo \"home $(m{home})\"; {{ cd /data/work && echo \"work $(m{work})\"; }} || echo work; else cd /data && echo \"all $(du -sxk . 2>/dev/null | cut -f1) $(du -sxb . 2>/dev/null | cut -f1)\"; fi"
    );
    // what takes the disk: all of /data, its caches too; each record a line
    // (GNU's du -0, a newline in a name read as `?`: a name forges no line),
    // a line past its bound dropped (bytes: LC_ALL=C), each walk ending by
    // SIZES_LISTED_BY_S from the read's start (`s`) and within SIZES_WALK_S
    let walk = |what: &str| format!("{{ t=$(( {SIZES_LISTED_BY_S} - $(date +%s) + s )); [ $t -gt {SIZES_WALK_S} ] && t={SIZES_WALK_S}; [ $t -gt 0 ] && timeout $t {what} 2>/dev/null; }} | framed | LC_ALL=C awk 'length < {SIZE_LINE_MAX_BYTES}' | sort -rn");
    let listing = format!(
        "cd /data; if du -0 -s /dev/null > /dev/null 2>&1; then z=-0; framed() {{ tr '\\n\\0' '?\\n'; }}; else z=; framed() {{ cat; }}; fi; echo folders; {} | head -n {}; echo files; {} | head -n {SIZES_FILES_MAX}",
        walk(&format!("du $z -xk -d {SIZES_DEPTH} .")),
        SIZES_FOLDERS_MAX + 1,
        walk(&format!("find . -xdev -type f -size +{SIZES_FILE_MIN_KIB}k -exec du $z -k {{}} +"))
    );
    // its df and its records first (Codex on #1426: a line the listing
    // prints never passes for theirs, and its walks never cost them their read)
    format!("s=$(date +%s); {records}; {listing}")
}

/// How deep below `/data` a size summary lists its folders: an agent's
/// work (`work/<agent>`), its home and browser profile, and their caches
/// (`work/<agent>/home/.cache`).
pub const SIZES_DEPTH: u32 = 4;
/// Each walk of a size summary's listing ends within this many seconds
/// (`timeout`), and both by `SIZES_LISTED_BY_S` from the read's start, its
/// records and df read before them: the whole read ends within its bound
/// (`SIZES_READ_MS` in the cell, 30 s) whatever the listing does, and a
/// read whose records take the time lists nothing (Codex on #1426).
pub const SIZES_WALK_S: u32 = 8;
pub const SIZES_LISTED_BY_S: u32 = 24;
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
/// A listing's line the container drops past (`awk`, in bytes), its count,
/// a tab and `./` beside a path at most `SIZE_PATH_MAX_BYTES`; so
/// `sizes_read`'s whole answer is at most `SIZES_READ_MAX_BYTES`, which the
/// cell keeps of it (an exec keeps its answer's tail).
pub const SIZE_LINE_MAX_BYTES: usize = SIZE_PATH_MAX_BYTES + 24;
pub const SIZES_READ_MAX_BYTES: usize = 64 * 1024;
const _: () = assert!(1024 + (SIZES_FOLDERS_MAX + 1 + SIZES_FILES_MAX) * SIZE_LINE_MAX_BYTES <= SIZES_READ_MAX_BYTES, "a summary's whole answer is kept");

/// The `N\t./path` lines of `sizes_read`'s answer after `section`'s line,
/// up to the next section's: KiB and the path below `/data` (`.` its own).
/// Its df and records come before its first section, never among them. A
/// line is what ends in `\n`, a `\r` before it kept as the name's own.
fn listed<'a>(out: &'a str, section: &'static str) -> impl Iterator<Item = (u64, &'a str)> {
    // split on `\n` alone: `lines()` would strip a name's own trailing `\r`,
    // listing `a.zip\r` as `a.zip` (Codex on #1429); kept, it is a control
    // character, so the entry is left out
    out.split('\n').skip_while(move |l| l.trim() != section).skip(1).take_while(|l| !matches!(l.trim(), "folders" | "files" | "end")).filter_map(|l| {
        let (n, path) = l.split_once('\t')?;
        Some((n.trim().parse::<u64>().ok()?, path))
    })
}

/// What its owner's Storage shows, from `sizes_read`'s answer read at
/// `at_ms`: `/data`'s own total (GNU's `.`; BusyBox's `all` record, as a
/// name there could forge a `.`), its biggest folders below it and
/// its biggest files, each in bytes allocated, a path below `/data` a line,
/// printable and at most `SIZE_PATH_MAX_BYTES`, as many as
/// `SIZES_FOLDERS_MAX` and `SIZES_FILES_MAX`, their paths together at most
/// `SIZES_PATHS_MAX_BYTES`. None for an answer with no folders.
pub fn sizes_of(out: &str, at_ms: i64) -> Option<ComputerSizes> {
    if !out.lines().any(|l| l.trim() == "folders") {
        return None;
    }
    // BusyBox frames no name, so a newline in one could forge a `.` line:
    // its `all` record (before the listing) is its total; GNU's `.` is framed
    let all = out.lines().take_while(|l| l.trim() != "folders").find_map(|l| l.strip_prefix("all ")?.split_whitespace().next()?.parse::<u64>().ok());
    let total = all.or_else(|| listed(out, "folders").find(|(_, path)| *path == ".").map(|(kib, _)| kib)).map(|kib| kib.saturating_mul(1024));
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
    Some(ComputerSizes { at: at_ms, total, folders, files, kinds: vec![] })
}

/// What a person's Delete in Storage removes (docs/computers.md, "Room to
/// restore"): one regular file of its work, named by its path below
/// `/data` as its summary names it (`work/…`), refused outside `/data/work`
/// (a `..` or `.` or empty part, a control character, past
/// `SIZE_PATH_MAX_BYTES`). Answers the command its computer runs (as root):
/// the path quoted for `sh`, resolved (`readlink -f`, so a link to a folder
/// elsewhere leads nowhere) and held to `/data/work/` again, a regular file
/// that is no link itself; it exits 0 once that file is gone, and
/// `DELETE_REFUSED` for anything else (none there, a folder, a link, a
/// path that leads out), touching nothing.
pub fn delete_read(path: &str) -> Result<String, &'static str> {
    let parts: Vec<&str> = path.split('/').collect();
    let ok = path.len() <= SIZE_PATH_MAX_BYTES
        && parts.first() == Some(&"work")
        && parts.len() >= 2
        && parts.iter().all(|p| !p.is_empty() && *p != "." && *p != "..")
        && !path.chars().any(char::is_control)
        // GNU's listing reads a newline in a name as `?`: such a path may
        // name another file (Codex on #1429)
        && !path.contains('?');
    if !ok {
        return Err("a file is named by its path below /data, in its work (work/…), with no . or .. part and no ?");
    }
    let quoted = format!("'/data/{}'", path.replace('\'', "'\\''"));
    Ok(format!(
        "p={quoted}; r=$(readlink -f -- \"$p\") || exit {DELETE_REFUSED}; case \"$r\" in /data/work/*) ;; *) exit {DELETE_REFUSED} ;; esac; {{ [ -f \"$r\" ] && [ ! -L \"$p\" ]; }} || exit {DELETE_REFUSED}; rm -f -- \"$r\""
    ))
}

/// `delete_read`'s exit when it removed nothing.
pub const DELETE_REFUSED: i64 = 3;

/// What a summary's folders hold, by kind, the most first, none empty
/// (its owner's Storage). Its agents' own: all of `/data` (`total`) but its
/// work (the seam: docs/computers.md, "Data and the restore gate"), or,
/// in a summary read before its total was kept, the folders of `/data`
/// but its work (Codex on #1428: a file of `/data`'s own counts). In its
/// work, a folder named as a cache (`.cache`, `.npm`, `Cache`), a browser's
/// (`browser-profile`, `chromium`, `google-chrome`) or downloads
/// (`Downloads`, `downloads`) is of that kind, counted once at the
/// shallowest such folder listed; its projects are the rest of its work.
/// A summary lists folders a few levels deep, so a kind's folder deeper
/// than that counts as projects.
pub fn storage_kinds(sizes: &ComputerSizes) -> Vec<KindSize> {
    let kind_of = |name: &str| match name {
        ".cache" | ".npm" | "Cache" => Some(StorageKind::Caches),
        "browser-profile" | "chromium" | "google-chrome" => Some(StorageKind::Browser),
        "Downloads" | "downloads" => Some(StorageKind::Downloads),
        _ => None,
    };
    let mut bytes = std::collections::BTreeMap::<StorageKind, u64>::new();
    let mut counted: Vec<&str> = Vec::new();
    // shallowest first, so a kind's folder inside another is not counted twice
    let mut folders: Vec<&SizeEntry> = sizes.folders.iter().collect();
    folders.sort_by_key(|e| e.path.matches('/').count());
    let mut work = 0u64;
    let mut agents = 0u64;
    for e in folders {
        let path = e.path.as_str();
        if path == "work" {
            work = e.bytes;
            continue;
        }
        if !path.starts_with("work/") {
            if !path.contains('/') {
                agents = agents.saturating_add(e.bytes);
            }
            continue;
        }
        let Some(kind) = path.rsplit('/').next().and_then(kind_of) else { continue };
        if counted.iter().any(|c| path.starts_with(&format!("{c}/"))) {
            continue;
        }
        counted.push(path);
        *bytes.entry(kind).or_default() += e.bytes;
    }
    let in_work: u64 = [StorageKind::Caches, StorageKind::Browser, StorageKind::Downloads].iter().map(|k| bytes.get(k).copied().unwrap_or(0)).sum();
    bytes.insert(StorageKind::Projects, work.saturating_sub(in_work));
    bytes.insert(StorageKind::Agents, sizes.total.map_or(agents, |t| t.saturating_sub(work)));
    let mut kinds: Vec<KindSize> = bytes.into_iter().filter(|(_, b)| *b > 0).map(|(kind, bytes)| KindSize { kind, bytes }).collect();
    kinds.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.kind.cmp(&b.kind)));
    kinds
}

/// The most one argument of an exec may hold on Linux (`MAX_ARG_STRLEN`,
/// 32 pages of 4 KiB): `sizes_read` is one (`sh -c`), and a guest's largest
/// answer, each pattern once with `./` and a space, fits it with room to
/// spare (a command past it is never run, its `df` lost: Codex on #1421).
pub const EXEC_ARG_MAX_BYTES: usize = 128 * 1024;
const _: () = assert!(crate::computer::held::HELD_ANSWER_MAX_BYTES + crate::computer::held::LEFT_OUT_MAX * 3 + 4096 <= EXEC_ARG_MAX_BYTES, "a guest's largest answer fits sizes_read's one argument");

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

/// What a disk's sleep runs in its container to read its owner's Storage
/// (cell `measure_asleep`, which runs this and nothing else): one `df` of
/// its `/data`, through `sh -c` as every exec of the DO's is; read by
/// `disk_of_df_on_disk`.
pub const DISK_SLEEP_ARGV: [&str; 3] = ["sh", "-c", DISK_READ_ON_DISK];

/// A computer's disk on a disk (docs/computers.md, "A disk") from its
/// `df` of `/data` (`DISK_READ_ON_DISK`, alone or a sizes read's first
/// lines): its `/data` is a filesystem of its own, so what takes it is that
/// filesystem's used bytes, its own count (one `statvfs`), read at once
/// however many files it holds, where a walk of them grows with them: a
/// sleep's measure never walks. None for an answer it cannot read.
pub fn disk_of_df_on_disk(out: &str, at_ms: i64, generation: u64) -> Option<ComputerDisk> {
    let disk = disk_of_df(out, at_ms, generation)?;
    let row = out.lines().filter(|l| !l.trim().is_empty()).skip_while(|l| !l.starts_with("Filesystem")).nth(1)?;
    let used = row.split_whitespace().nth(2)?.parse::<u64>().ok()?;
    (used <= disk.total).then_some(ComputerDisk { data: Some(used), ..disk })
}

/// Whether a disk computer's breakdown (its last walk's, `sizes_of`: an
/// export's save or a file's delete) is as of its last read (`disk_at`):
/// its sleep reads its total alone (`disk_of_df_on_disk`), so a breakdown
/// older than that read says less than its total does, and its owner's
/// Storage shows the total and its time without it.
pub fn sizes_current(sizes_at: i64, disk_at: Option<i64>) -> bool {
    disk_at.is_none_or(|at| sizes_at >= at)
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

    /// Goal: a disk's data is its own filesystem's used bytes, from its
    /// `df` of `/data` alone or a sizes read's first lines (GNU's and
    /// BusyBox's), the walk's records never counted. Invalid: no `df`, used
    /// past its size, not a number.
    #[test]
    fn a_disks_data_is_its_filesystems_used_bytes() {
        let gnu = "Filesystem     1-blocks      Used   Available Capacity Mounted on\n/dev/loop0  20618805248 141561856 20477243392       1% /data\n";
        let busybox = "Filesystem           1-blocks       Used Available Capacity Mounted on\n/dev/loop0           20618805248  141561856 20477243392   1% /data\n";
        let want = ComputerDisk { total: 20_618_805_248, free: 20_477_243_392, at: T, generation: 4, data: Some(141_561_856) };
        assert_eq!(disk_of_df_on_disk(gnu, T, 4), Some(want));
        assert_eq!(disk_of_df_on_disk(busybox, T, 4), Some(want));
        // a sizes read's answer: its df first, its records never the data
        let sized = format!("{gnu}home 99999999 102399998976\nwork 1 1024\nfolders\n");
        assert_eq!(disk_of_df_on_disk(&sized, T, 4), Some(want));
        for bad in ["", "Filesystem x\n/dev/loop0 10 11 0 100% /data", "Filesystem x\n/dev/loop0 10 lots 5 50% /data", "Filesystem x\n/dev/loop0 10 5"] {
            assert_eq!(disk_of_df_on_disk(bad, T, 4), None, "{bad:?}");
        }
    }

    /// Goal: a disk's sleep runs one `df` of its `/data` (its filesystem's
    /// own count), never a walk of its files, which grows with them, and
    /// nothing beside it (cell `measure_asleep` runs this argv alone).
    #[test]
    fn a_disks_sleep_reads_one_df_and_walks_nothing() {
        assert_eq!(DISK_SLEEP_ARGV, ["sh", "-c", "df -P -B1 /data"]);
        assert_eq!(read_on_disk(DISK_READ), DISK_READ_ON_DISK);
        // one command: nothing rides along in its shell
        let script = DISK_SLEEP_ARGV[2];
        assert!(!script.contains([';', '&', '|', '$', '`', '\n']) && !script.split_whitespace().any(|w| ["du", "find", "ls"].contains(&w)), "{script:?}");
    }

    /// Goal: a disk's breakdown is shown only as of its last read: the one
    /// taken with it (an export's walk, its df the same moment) or after it
    /// (none read since). Invalid: a sleep's read after it, which says a
    /// total the breakdown does not.
    #[test]
    fn a_disks_breakdown_older_than_its_last_read_is_not_shown() {
        assert!(sizes_current(T, Some(T)));
        assert!(sizes_current(T, None));
        assert!(sizes_current(T + 5, Some(T)));
        assert!(!sizes_current(T, Some(T + 1)));
    }

    /// Goal: what a restore of `/data` writes, read after a save as the
    /// Hermes image's GNU `du` and the stub's BusyBox `du` answer it (both
    /// answers the images' own, 2026-11-20, for an 8 MiB file hard-linked
    /// between /data/hermes and the work, a uv cache left out holding a
    /// 4 MiB file linked into a venv and a 2 MiB file of its own, a 3 MiB
    /// file in a project's own `home/.cache/uv` (kept: the pattern's `*` is
    /// one name), a 1 MiB one in a dot-folder's (left out by the save,
    /// counted: the shell's `*` skips a leading `.`), and a copied state.db
    /// left out): each record as its save takes it, so the link across the
    /// records counts in each (each restores its copy) and the venv's link
    /// counts though its cache is left out (Codex and Alex on #1421), the
    /// project's cache counts (GNU's own `--exclude` read 12,288 KiB for the
    /// work, missing it: Codex on #1421), BusyBox all of it once. Valid, a
    /// sparse file and small files, a record unread (none of data, never a
    /// part), invalid (no `df`, an unsafe pattern, which is never used, a
    /// guest naming its whole work, which its work's record keeps), and a
    /// guest's largest answer, within one exec's argument.
    #[test]
    fn data_is_read_from_du() {
        let gnu = "Filesystem          1-blocks          Used    Available Capacity Mounted on\noverlay        1998232485888 1788574265344 188868038656      91% /\nhome 8192 8388608\nwork 16384 16777216\n";
        let busybox = "Filesystem           1-blocks       Used Available Capacity Mounted on\noverlay              1998232485888 1788574343168 188867993600  90% /\nall 18436 18874656\n";
        assert_eq!(disk_of_sizes(gnu, T, 2).and_then(|d| d.data), Some((8192 + 16384) * 1024), "the link in each record, the venv's link and the project's cache, not the agent's cache");
        assert_eq!(disk_of_sizes(busybox, T, 2).and_then(|d| d.data), Some(18436 * 1024), "all of it, once: allocated, past apparent");
        assert_eq!(disk_of_sizes(gnu, T, 2).map(|d| (d.total, d.free, d.generation)), Some((1_998_232_485_888, 188_868_038_656, 2)));
        // a sparse file: apparent past allocated; small files: allocated past apparent
        let df = "Filesystem x\noverlay 100 50 50 50% /\n";
        assert_eq!(disk_of_sizes(&format!("{df}home 4 9000000\nwork 100 1000\n"), T, 2).and_then(|d| d.data), Some(9_000_000 + 102_400));
        // a record unread: no data (the last one is kept), never a part of it; no df: none
        for unread in ["home 4 10\n", "home 4 10\nwork\n", "home 4 10\nwork 1\n", "work 4 10\n", "home x 10\nwork 1 1\n", "all \n", ""] {
            assert_eq!(disk_of_sizes(&format!("{df}{unread}"), T, 2).map(|d| d.data), Some(None), "{unread:?}");
        }
        assert_eq!(disk_of_sizes("home 4 10\nwork 4 10\n", T, 2), None);
        assert_eq!(disk_of_sizes(&format!("noise\n{df}all 1 1\n"), T, 2).map(|d| d.data), Some(Some(1024)), "df's header anywhere");
        // the read itself: each record measured as its save takes it (each
        // pattern a glob of the shell's, once), BusyBox's all
        let read = |home: &str, work: &str| {
            let m = "m() { n=$#; for p; do [ -e \"$p\" ] && set -- \"$@\" \"--exclude=$p\"; done; shift $n; echo \"$(du -sxk \"$@\" . 2>/dev/null | cut -f1) $(du -sxb \"$@\" . 2>/dev/null | cut -f1)\"; }";
            format!("s=$(date +%s); df -P -B1 /; if du -s --exclude=x /dev/null > /dev/null 2>&1; then {m}; cd /data && echo \"home $(m{home})\"; {{ cd /data/work && echo \"work $(m{work})\"; }} || echo work; else cd /data && echo \"all $(du -sxk . 2>/dev/null | cut -f1) $(du -sxb . 2>/dev/null | cut -f1)\"; fi; {LISTING}")
        };
        assert_eq!(sizes_read(&[]), read(" ./work", ""));
        // on a disk, its df is of its /data, and the rest is the same
        let read_all = sizes_read(&[]);
        assert_eq!(read_on_disk(&read_all), read_all.replacen("df -P -B1 /;", "df -P -B1 /data;", 1));
        assert!(read_on_disk(&read_all).contains("s=$(date +%s); df -P -B1 /data; if du"));
        assert_eq!(read_on_disk(DISK_READ), "df -P -B1 /data");
        assert_eq!(
            sizes_read(&["/work/*/home/.cache/uv/".into(), "/hermes/state.db".into(), "/../etc/".into(), "/a b/".into(), "/x;rm/".into(), "/x'y/".into(), "/x$(id)/".into(), String::new()]),
            read(" ./work ./hermes/state.db", " ./*/home/.cache/uv"),
            "the home leaves out its work and what was named outside it; the work what was named under it; never an unsafe pattern"
        );
        assert_eq!(sizes_read(&["/work/".into()]), read(" ./work", ""), "a guest naming its whole work: its work's record keeps it all, and so does its measure");
        // a guest's largest answer: within one exec's argument, each pattern once (Codex on #1421)
        let most: Vec<String> = (0..crate::computer::held::LEFT_OUT_MAX).map(|i| format!("/work/{i:03}{}", "a".repeat(crate::computer::held::HELD_ANSWER_MAX_BYTES / crate::computer::held::LEFT_OUT_MAX - 11))).collect();
        assert!(most.iter().map(|p| p.len() + 1).sum::<usize>() <= crate::computer::held::HELD_ANSWER_MAX_BYTES && most.iter().all(|p| p.len() <= LEFT_OUT_PATTERN_MAX_BYTES), "a valid answer");
        let read = sizes_read(&most);
        assert!(read.len() <= EXEC_ARG_MAX_BYTES, "{} bytes", read.len());
        assert_eq!(read.matches("/000aaa").count(), 1, "each pattern once");
    }

    /// The listing `sizes_read` begins with: all of `/data`, framed, bounded.
    const LISTING: &str = "cd /data; if du -0 -s /dev/null > /dev/null 2>&1; then z=-0; framed() { tr '\\n\\0' '?\\n'; }; else z=; framed() { cat; }; fi; echo folders; { t=$(( 24 - $(date +%s) + s )); [ $t -gt 8 ] && t=8; [ $t -gt 0 ] && timeout $t du $z -xk -d 4 . 2>/dev/null; } | framed | LC_ALL=C awk 'length < 424' | sort -rn | head -n 65; echo files; { t=$(( 24 - $(date +%s) + s )); [ $t -gt 8 ] && t=8; [ $t -gt 0 ] && timeout $t find . -xdev -type f -size +10240k -exec du $z -k {} + 2>/dev/null; } | framed | LC_ALL=C awk 'length < 424' | sort -rn | head -n 32";

    /// Goal: what takes the disk, read after a save as the Hermes image's GNU
    /// tools and the stub's BusyBox answer `sizes_read` (both answers the
    /// images' own, 2026-11-20: a 20 MiB video, a 12 MiB file in a uv cache
    /// linked into a venv, a 4 MiB file in that cache alone, and a folder
    /// whose name holds a newline and a forged line): `/data`'s total, its
    /// biggest folders (its caches too) and files, paths below `/data`; a
    /// newline in a name forges no line on GNU (Codex on #1426); its df and
    /// records before the listing, read as ever, a line in it never theirs. Valid, small files, invalid (no
    /// listing, a path past its bound, a control character, too many).
    #[test]
    fn what_takes_the_disk_is_listed() {
        let named = |l: &[SizeEntry]| l.iter().map(|e| (e.path.clone(), e.bytes)).collect::<Vec<_>>();
        let gnu = sizes_of(GNU_ANSWER, T).expect("a summary");
        assert_eq!(gnu.at, T);
        assert_eq!(gnu.total, Some(48_132 * 1024));
        let kib = |p: &str, k: u64| (p.to_string(), k * 1024);
        assert_eq!(named(&gnu.folders), vec![kib("work/juniper", 48_128), kib("work", 48_128), kib("work/juniper/home/.cache", 16_384), kib("work/juniper/home", 16_384), kib("hermes", 4), kib("work/juniper/proj/.venv", 0), kib("work/juniper/proj", 0)], "its caches too; the link counted once, where du found it first; the odd name (a tab in it) left out");
        assert_eq!(named(&gnu.files), vec![kib("work/juniper/talk.mp4", 20_480), kib("work/juniper/home/.cache/uv/pkg.bin", 12_288)], "a newline forges no line");
        assert_eq!(disk_of_sizes(GNU_ANSWER, T, 2).and_then(|d| d.data), Some(4096 + 44_032 * 1024), "its records after its listing: the work without its cache, the venv's link kept");
        let busybox = sizes_of(BUSYBOX_ANSWER, T).expect("a summary");
        assert_eq!(busybox.total, Some(48_132 * 1024), "its total its own record's, never a forged `.` line");
        assert!(named(&busybox.files).starts_with(&[kib("forged/clip.bin", 999_999), kib("work/juniper/talk.mp4", 20_480), kib("work/juniper/home/.cache/uv/pkg.bin", 12_288)]), "BusyBox frames no name: a newline forges a line (the stub's only)");
        assert_eq!(disk_of_sizes(BUSYBOX_ANSWER, T, 2).and_then(|d| d.data), Some(48_132 * 1024));
        // its total: `.`'s, or an `all` record's after `end`, never one before it
        assert_eq!(sizes_of("folders\n100\t.\nfiles\n", T).map(|s| (s.total, s.folders.len(), s.files.len())), Some((Some(102_400), 0, 0)));
        assert_eq!(sizes_of("all 9 1\nfolders\n100\t.\nall 7 1\nfiles\n", T).and_then(|s| s.total), Some(9 * 1024), "its record's, before the listing, never a line in it (Codex on #1426)");
        assert_eq!(sizes_of("folders\n5\t./a\nfiles\n", T).map(|s| s.total), Some(None), "neither: none");
        // no listing: no summary
        assert_eq!(sizes_of("Filesystem x\noverlay 100 50 50 50% /\nall 1 1\n", T), None);
        // bounds: a path too long, a control character, too many, all paths together
        let long = "a".repeat(SIZE_PATH_MAX_BYTES + 1);
        let odd = format!("folders\n9\t./{long}\n8\t./a\u{7}b\n7\t./ok\nfiles\nend\n");
        assert_eq!(sizes_of(&odd, T).map(|s| named(&s.folders)), Some(vec![("ok".to_string(), 7 * 1024)]));
        let many: String = (0..SIZES_FOLDERS_MAX + 10).map(|i| format!("{}\t./f{i}\n", 1000 - i)).collect();
        let many = sizes_of(&format!("folders\n{many}files\nend\n"), T).expect("a summary");
        assert_eq!((many.folders.len(), many.folders[0].path.as_str()), (SIZES_FOLDERS_MAX, "f0"), "the biggest, as many as it holds");
        let wide: String = (0..60).map(|i| format!("{}\t./{}{i:02}\n", 1000 - i, "d".repeat(SIZE_PATH_MAX_BYTES - 2))).collect();
        let wide = sizes_of(&format!("folders\n{wide}files\nend\n"), T).expect("a summary");
        assert!(wide.folders.iter().map(|e| e.path.len()).sum::<usize>() <= SIZES_PATHS_MAX_BYTES && wide.folders.len() == SIZES_PATHS_MAX_BYTES / SIZE_PATH_MAX_BYTES, "its paths within their bound: {}", wide.folders.len());
        // a df and a record forged in the listing (BusyBox frames no name) never pass for the real ones before it
        assert_eq!(disk_of_sizes("Filesystem x\noverlay 100 50 50 50% /\nall 9 1\nfolders\nall 1 1\nFilesystem y\noverlay 7 7 0 0% /\nfiles\n", T, 2).map(|d| (d.total, d.free, d.data)), Some((100, 50, Some(9 * 1024))), "Codex on #1426");
        // a name's own trailing `\r` is kept, so it never passes for another file's (Codex on #1429)
        let cr = sizes_of("folders\n9\t./work/a/report.zip\r\n8\t./work/a\nfiles\n7\t./work/a/report.zip\r\n6\t./work/a/ok.zip\n", T).expect("a summary");
        assert_eq!((named(&cr.folders), named(&cr.files)), (vec![("work/a".to_string(), 8 * 1024)], vec![("work/a/ok.zip".to_string(), 6 * 1024)]));
        // its df and records, before the listing, are no folder or file
        let after = sizes_of("Filesystem x\noverlay 100 50 50 50% /\nall 3 3\nfolders\n5\t./a\nfiles\n4\t./b\n", T).expect("a summary");
        assert_eq!((named(&after.folders), named(&after.files)), (vec![("a".to_string(), 5120)], vec![("b".to_string(), 4096)]));
    }

    /// Goal (Paul, 2026-11-20: Settings → Computer → Storage, in plain
    /// categories): a summary's folders by kind. Its agents' own is `/data`
    /// but its work (all of it, its total known); in its work, caches, browser data and downloads, each
    /// once at its shallowest folder listed (a cache inside a browser's
    /// profile is the browser's); projects the rest of its work. Valid, a
    /// kind nested in another, nothing in a kind (left out), no work,
    /// nothing at all.
    #[test]
    fn a_summary_is_told_by_kind() {
        const GB: u64 = 1_000_000_000;
        let entry = |path: &str, gb: f64| SizeEntry { path: path.into(), bytes: (gb * GB as f64) as u64 };
        let sizes = |folders: Vec<SizeEntry>| ComputerSizes { at: T, total: None, folders, files: vec![], kinds: vec![] };
        let told = |s: &ComputerSizes| storage_kinds(s).into_iter().map(|k| (k.kind, k.bytes)).collect::<Vec<_>>();
        let full = sizes(vec![
            entry("work", 20.0),
            entry("work/juniper", 18.0),
            entry("work/juniper/home", 5.0),
            entry("work/juniper/home/.cache", 2.0),
            entry("work/juniper/browser-profile", 3.0),
            entry("work/juniper/browser-profile/Cache", 1.0),
            entry("work/juniper/downloads", 4.0),
            entry("work/juniper/home/Downloads", 1.0),
            entry("work/juniper/proj", 6.0),
            entry("hermes", 1.5),
            entry("hermes/sessions", 1.0),
            entry("bridge", 0.1),
        ]);
        assert_eq!(
            told(&full),
            [(StorageKind::Projects, 10 * GB), (StorageKind::Downloads, 5 * GB), (StorageKind::Browser, 3 * GB), (StorageKind::Caches, 2 * GB), (StorageKind::Agents, (1.6 * GB as f64) as u64)],
            "the browser's own Cache is the browser's, once"
        );
        assert_eq!(told(&sizes(vec![entry("work", 2.0), entry("work/a/proj", 2.0)])), [(StorageKind::Projects, 2 * GB)], "nothing in a kind: left out");
        assert_eq!(told(&sizes(vec![entry("hermes", 1.0)])), [(StorageKind::Agents, GB)], "no work");
        assert_eq!(told(&sizes(vec![])), [], "nothing at all");
        // its total known: its agents' own is all of /data but its work, its own files too (Codex on #1428)
        let whole = ComputerSizes { total: Some(23 * GB), ..full.clone() };
        assert_eq!(told(&whole).iter().find(|k| k.0 == StorageKind::Agents), Some(&(StorageKind::Agents, 3 * GB)), "1.6 GB of folders and 1.4 GB of /data's own files");
        assert_eq!(told(&ComputerSizes { total: Some(GB), ..full.clone() }).iter().find(|k| k.0 == StorageKind::Agents), None, "a total under its work: none, never an overflow");
        // the Hermes image's real answer: its cache is a kind, its venv's link its project's
        let real = sizes_of(GNU_ANSWER, T).expect("a summary");
        assert_eq!(told(&real), [(StorageKind::Projects, 31_744 * 1024), (StorageKind::Caches, 16_384 * 1024), (StorageKind::Agents, 4 * 1024)]);
    }

    /// Goal (Paul, 2026-11-20: Delete, done by the platform in the
    /// container, never on a path outside /data/work): what a Delete runs
    /// for a path as its summary names it. Valid: a file of the work, a
    /// name with spaces and a quote, quoted for sh. Invalid: outside the
    /// work, the work or an agent's folder's parent itself, `..`, `.`, an
    /// empty part, an absolute path, a control character, past its bound.
    /// What the command does on both images (a file gone; a folder, a link,
    /// a link to a folder elsewhere and nothing there refused, untouched)
    /// is the Docker check in the PR that added it.
    #[test]
    fn a_delete_is_a_file_of_the_work_alone() {
        let ran = |path: &str| delete_read(path).expect("a file of the work");
        assert!(ran("work/juniper/downloads/talk.mp4").starts_with("p='/data/work/juniper/downloads/talk.mp4'; r=$(readlink -f -- \"$p\") || exit 3;"));
        assert!(ran("work/juniper/downloads/talk.mp4").ends_with("case \"$r\" in /data/work/*) ;; *) exit 3 ;; esac; { [ -f \"$r\" ] && [ ! -L \"$p\" ]; } || exit 3; rm -f -- \"$r\""));
        assert!(ran("work/a b/it's.mp4").starts_with("p='/data/work/a b/it'\\''s.mp4';"), "a quote closes, is escaped, and opens again");
        assert!(ran("work/x").starts_with("p='/data/work/x';"));
        for bad in ["", "work", "work/", "/work/x", "hermes/state.db", "x/work/y", "work/../hermes/state.db", "work/./x", "work//x", "work/x/", "work/a\nb", "work/a\u{0}b", &format!("work/{}", "a".repeat(SIZE_PATH_MAX_BYTES)), "work/a?b", "work/odd?999999\t./forged/clip.bin"] {
            assert!(delete_read(bad).is_err(), "{bad:?}");
        }
        assert_eq!(DELETE_REFUSED, 3);
    }

    /// `sizes_read`'s answer from the Hermes image (GNU), 2026-11-20 (its
    /// records the shell's globs: Codex on #1421).
    const GNU_ANSWER: &str = "Filesystem          1-blocks          Used    Available Capacity Mounted on\noverlay        1998232485888 1789511987200 187965280256      91% /\nhome 4 3\nwork 44032 45088768\nfolders\n48132\t.\n48128\t./work/juniper\n48128\t./work\n16384\t./work/juniper/home/.cache\n16384\t./work/juniper/home\n11264\t./work/juniper/odd?999999\t./forged\n11264\t./work/juniper/odd?999999\t.\n4\t./hermes\n0\t./work/juniper/proj/.venv\n0\t./work/juniper/proj\nfiles\n20480\t./work/juniper/talk.mp4\n12288\t./work/juniper/home/.cache/uv/pkg.bin\n11264\t./work/juniper/odd?999999\t./forged/clip.bin\n";
    /// The same from the stub (BusyBox).
    const BUSYBOX_ANSWER: &str = "Filesystem           1-blocks       Used Available Capacity Mounted on\noverlay              1998232485888 1789512024064 187965276160  90% /\nall 48132 49283285\nfolders\n999999\t./forged\n999999\t.\n48132\t.\n48128\t./work/juniper\n48128\t./work\n16384\t./work/juniper/home/.cache\n16384\t./work/juniper/home\n11264\t./work/juniper/odd\n11264\t./work/juniper/odd\n4\t./hermes\n0\t./work/juniper/proj/.venv\n0\t./work/juniper/proj\nfiles\n999999\t./forged/clip.bin\n20480\t./work/juniper/talk.mp4\n12288\t./work/juniper/home/.cache/uv/pkg.bin\n11264\t./work/juniper/odd\n";

    /// Goal (production, 2026-11-20): a restore that stopped for want of
    /// room is told from every other failure by its words alone, narrowly:
    /// the sandbox runtime's guard, as the cell hears it (alone, or under
    /// the host's `restore '<dir>':`, or the lever's), and a disk that
    /// filled (`ENOSPC`). Invalid: a transfer, a timeout, an archive gone,
    /// a near miss of the guard's words, and a disk read's own.
    #[test]
    fn a_restore_that_found_no_room_is_told_by_its_words() {
        let guard = "restore stopped with ~203 MB free, below the 268435456 byte floor";
        for yes in [
            guard.to_string(),
            format!("restore '/data/work': {guard}"),
            format!("{guard} (the test lever's fail-restore of save 2)"),
            "ENOSPC: restore '/data/work': write failed".to_string(),
            "restore '/data': No space left on device (os error 28)".to_string(),
        ] {
            assert!(restore_found_no_room(&yes), "{yes}");
        }
        for no in [
            "BACKUP_TRANSFER: the download stopped",
            "the restore of /data/work: no answer within 540000 ms",
            "BACKUP_NOT_FOUND: no object",
            "restore stopped with an error",
            "restore '/data': EXDEV: cross-device link",
            "the disk is 91% used",
            "",
        ] {
            assert!(!restore_found_no_room(no), "{no}");
        }
    }
}
