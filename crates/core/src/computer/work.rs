//! A save keeps its work's last record when nothing in the work changed
//! (P9): `/data/work` is most of a person's disk, and copying it whole at
//! every save, changed or not, was most of a sleep (4.3 GB in 36–57 s on
//! preview, 2026-11-18).
//!
//! The DO stamps each work record in its container: a file named by the
//! record's id in `/data/.work-stamps/`, whose modification time is the
//! record's backup start (`STAMP_NEXT`, renamed to the id by `STAMP_TAKEN`
//! once the save is kept) or its restore's end (`STAMP_RESTORED`). A save
//! asks whether anything under the work changed after the stamp of the
//! work's last record in that container (`WORK_CHANGED`): a change time
//! (ctime, which no tool can set) newer than the stamp's. Only a clean "no"
//! keeps the record; an answer, an error, a stamp of another record, no
//! stamp, or the bound running out all copy the work, as before.
//!
//! Why it is exact:
//! - The stamp is on the work's own filesystem (`/data`, of which the work
//!   is a directory, never a mount: a restore swaps it in by a rename), so
//!   its time and the work's change times are of one clock at one
//!   granularity, and the comparison is strict.
//! - After each stamp the DO waits for that clock to tick past it before
//!   the backup reads anything, or before the guest is let go: every write
//!   in the stamp's own tick came before the record was taken, so it is in
//!   the record (a restore's), or the backup read it (a backup's), and every
//!   write after has a change time newer than the stamp's. A restore's
//!   stamp gets its name only once that tick passed.
//! - A backup's stamp then writes the filesystem's dirty pages back
//!   (`sync -f`), which write-protects every shared writable mapping of a
//!   file: a store through one after the stamp faults and changes its
//!   file's ctime, as a write does (Codex on #1379: a page dirtied before
//!   the stamp took later stores silently). A restore's needs none: no
//!   process of the new start has mapped anything yet.
//! - A write during the backup is after its stamp: the next save copies.
//! - A delete or a rename changes its directory; an edit, a chmod, a
//!   `touch -d` or a copy that keeps old times changes its file's ctime.
//! - A record is kept only by a save that leaves out what that record left
//!   out (`work_name`: the patterns' digest in the record's name), so a
//!   cache the guest names no more, or a save its guest did not answer, is
//!   copied (Codex on #1379).
//! - What the work's record leaves out (the caches its guest names) is
//!   walked too: a change there copies, erring toward saving.
//! - No stamp is made, and none is trusted, while anything is mounted under
//!   the work (an unmount could show what lay under it unchanged), or when
//!   the mounts cannot be read.
//!
//! What it trusts: the container's clock going forward (a root guest that
//! sets it back can hide its own changes from its own next save; a stamp
//! still in the future is not trusted). A write to a file the walk passed
//! already, as it walks, is the next save's, as one to a file a copy read
//! already is.

use serde_json::Value;

/// Where the stamps are kept, relative to `/data`: root's alone (a tool run
/// as the guest's user cannot touch one), and left out of the home's
/// record, so a restore of it brings back none.
pub const WORK_STAMPS: &str = "/.work-stamps/";

/// The longest a check of the work (`WORK_CHANGED`) is given: a walk of
/// 195,514 files took 0.6 s warm on preview, and an `lstat` walk of as many
/// 4.6–9.5 s cold (11-18). Past it, it copies.
pub const WORK_CHECK_MS: i64 = 6_000;
/// The longest a backup's stamp (`STAMP_NEXT`: its tick and its writeback)
/// is given. Past it, no stamp: the next save copies.
pub const WORK_STAMP_MS: i64 = 4_000;

/// Each script runs as `sh -c <script> sh <data> <args>…` (`$1` is `/data`,
/// a test's own directory in tests). First, nothing may be mounted under
/// the work, and the mounts must read (`MOUNTINFO`, the process's own
/// unless a test names another).
macro_rules! no_mount {
    () => {
        "m=$(cat \"${MOUNTINFO:-/proc/self/mountinfo}\") || exit 3\ncase \"$m\" in *\" $1/work \"*|*\" $1/work/\"*) exit 3;; esac\n"
    };
}

/// The tick wait after a stamp: until a probe touched now is newer than
/// it, at most about 3 s (a filesystem that keeps whole seconds waits up
/// to one), else it failed.
macro_rules! tick_after {
    ($stamp:literal) => {
        concat!(
            "i=0\nuntil touch ./.probe && [ -n \"$(find ./.probe -newer \"./",
            $stamp,
            "\")\" ]; do i=$((i+1)); [ \"$i\" -le 300 ] || exit 1; sleep 0.01; done\nrm -f ./.probe\n"
        )
    };
}

/// Before the work's backup (`$2`: the save's number): its stamp, the
/// tick past it, then the writeback. Exit 2: this image's `find` cannot
/// compare change times (`-newercm`, GNU findutils'), so no save of it
/// keeps its work; exit 3: a mount under the work, or none readable.
pub const STAMP_NEXT: &str = concat!(
    no_mount!(),
    "cd \"$1\" && mkdir -p -m 700 .work-stamps && cd .work-stamps && find . -maxdepth 0 -newercm . >/dev/null 2>&1 || exit 2\n",
    "rm -f \"./next-$2\" && touch \"./next-$2\" || exit 1\n",
    tick_after!("next-$2"),
    "sync -f . || exit 1\n",
);

/// Once the save `$2` is kept with the new work record `$3`: its stamp is
/// that record's, the only one (another save's under way, ended first,
/// loses its own, and copies next time).
pub const STAMP_TAKEN: &str = "cd \"$1/.work-stamps\" && test -f \"./next-$2\" || exit 1\nfor f in *; do [ \"$f\" = \"next-$2\" ] || rm -f \"./$f\"; done\nmv \"./next-$2\" \"./$3\"\n";

/// Once a start restored its save, the work's record `$2` among it, and
/// before its check and its gate: that record's stamp, the only one, named
/// so only once the tick past it passed.
pub const STAMP_RESTORED: &str = concat!(
    no_mount!(),
    "cd \"$1\" && rm -rf .work-stamps && mkdir -m 700 .work-stamps && cd .work-stamps && find . -maxdepth 0 -newercm . >/dev/null 2>&1 || exit 2\n",
    "touch ./.pending || exit 1\n",
    tick_after!(".pending"),
    "mv ./.pending \"./$2\"\n",
);

/// Whether anything under the work changed since the stamp of record
/// `$2`: the first path that did, or nothing. Exit 1 with no such stamp,
/// one still in the future, or a mount under the work (or none readable).
pub const WORK_CHANGED: &str = concat!(
    "s=\"$1/.work-stamps/$2\"\ntest -f \"$s\" || exit 1\n",
    no_mount!(),
    "[ -z \"$(find \"$s\" -newermt now)\" ] || exit 1\n",
    "find \"$1/work\" -newercm \"$s\" -print -quit\n",
);

/// Whether `WORK_CHANGED`'s run says the work is unchanged: it exited 0
/// and printed nothing. Anything else (a path, an error, no answer within
/// its bound: `None`) copies it.
pub fn work_unchanged(exit: Option<i64>, output: &str) -> bool {
    exit == Some(0) && output.trim().is_empty()
}

/// Whether `id` names a stamp: a record's id as `DirectoryBackup` makes
/// them (a UUID), never a path nor an option.
pub fn stamp_name_ok(id: &str) -> bool {
    id.len() <= 64 && id.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// The name a work's record of start `generation` is taken with, saying
/// what it leaves out (`exclude`, its patterns' digest): a save keeps a
/// record only when it leaves out the same (`kept_for`).
pub fn work_name(generation: u64, exclude: &[String]) -> String {
    format!("generation {generation} work {}", crate::blob::sha256_hex(exclude.join("\n").as_bytes()))
}

/// Whether a save that leaves out `exclude` may keep `record` (a work's,
/// named by `work_name` as it was taken). A record named otherwise (taken
/// before records said so) is never kept.
pub fn kept_for(record: &Value, exclude: &[String]) -> bool {
    let digest = crate::blob::sha256_hex(exclude.join("\n").as_bytes());
    record["name"].as_str().and_then(|n| n.rsplit_once(" work ")).is_some_and(|(_, d)| d == digest)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::*;

    /// A data directory of its own, with a work in it.
    fn data(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fragment-work-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for p in ["work/a/home/.config", "work/a/project/src", "work/b"] {
            std::fs::create_dir_all(d.join(p)).unwrap();
        }
        for (p, text) in [("work/a/project/src/main.rs", "fn main() {}"), ("work/a/home/.config/app.toml", "x = 1"), ("work/b/notes.txt", "hello")] {
            std::fs::write(d.join(p), text).unwrap();
        }
        d
    }

    /// `script` run as the DO runs it, on `data`: its exit and its output.
    fn run(script: &str, data: &Path, args: &[&str]) -> (Option<i64>, String) {
        run_with_path(script, data, args, None)
    }

    fn run_with_path(script: &str, data: &Path, args: &[&str], path: Option<&str>) -> (Option<i64>, String) {
        run_env(script, data, args, &path.map(|p| vec![("PATH", p)]).unwrap_or_default())
    }

    fn run_env(script: &str, data: &Path, args: &[&str], env: &[(&str, &str)]) -> (Option<i64>, String) {
        let mut c = Command::new("sh");
        c.arg("-c").arg(script).arg("sh").arg(data).args(args);
        for (k, v) in env {
            c.env(k, v);
        }
        let out = c.output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
        (out.status.code().map(i64::from), text)
    }

    fn unchanged(data: &Path, id: &str) -> bool {
        let (exit, out) = run(WORK_CHANGED, data, &[id]);
        work_unchanged(exit, &out)
    }

    const ID: &str = "6f1c1a52-1b6e-4c3e-9a51-0c2f6e0d7e11";

    /// Valid: a work restored and stamped, then left alone, is unchanged,
    /// though the restore swapped its directory in (a rename) and wrote
    /// its files just before the stamp; its stamp is the only one.
    #[test]
    fn a_work_left_alone_is_unchanged() {
        let d = data("alone");
        // as a restore does: extracted beside, old times kept, swapped in
        std::fs::rename(d.join("work"), d.join("work.new")).unwrap();
        assert!(Command::new("touch").args(["-d", "2020-01-01"]).arg(d.join("work.new/b/notes.txt")).status().unwrap().success());
        std::fs::create_dir_all(d.join(".work-stamps")).unwrap();
        std::fs::write(d.join(".work-stamps/old-record"), "").unwrap();
        std::fs::rename(d.join("work.new"), d.join("work")).unwrap();
        assert_eq!(run(STAMP_RESTORED, &d, &[ID]).0, Some(0));
        let stamps: Vec<_> = std::fs::read_dir(d.join(".work-stamps")).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(stamps, vec![std::ffi::OsString::from(ID)], "its record's stamp alone");
        assert!(unchanged(&d, ID));
        assert!(unchanged(&d, ID), "asked again, the same");
        // reading changes nothing
        assert_eq!(std::fs::read_to_string(d.join("work/b/notes.txt")).unwrap(), "hello");
        assert!(unchanged(&d, ID));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Each change after the stamp, at once (in the stamp's own tick if
    /// the clock allows), is seen: a new file, an edit, a delete, a rename,
    /// a move across directories, a chmod, old times put back on an edited
    /// file, a new empty directory, a link.
    #[test]
    fn every_change_after_the_stamp_is_seen() {
        type Change = (&'static str, fn(&Path));
        let changes: [Change; 10] = [
            ("a new file", |w| std::fs::write(w.join("b/new.txt"), "n").unwrap()),
            ("an edit", |w| {
                use std::io::Write;
                std::fs::OpenOptions::new().append(true).open(w.join("b/notes.txt")).unwrap().write_all(b"!").unwrap();
            }),
            ("a delete", |w| std::fs::remove_file(w.join("b/notes.txt")).unwrap()),
            ("a rename", |w| std::fs::rename(w.join("b/notes.txt"), w.join("b/renamed.txt")).unwrap()),
            ("a move across directories", |w| std::fs::rename(w.join("a/project/src/main.rs"), w.join("b/main.rs")).unwrap()),
            ("a chmod", |w| {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(w.join("b/notes.txt"), std::fs::Permissions::from_mode(0o600)).unwrap();
            }),
            ("an edit with its old times put back", |w| {
                let f = w.join("a/home/.config/app.toml");
                std::fs::write(&f, "x = 2").unwrap();
                assert!(Command::new("touch").args(["-d", "2020-01-01"]).arg(&f).status().unwrap().success());
            }),
            ("a new empty directory", |w| std::fs::create_dir(w.join("a/empty")).unwrap()),
            ("a link", |w| std::os::unix::fs::symlink("notes.txt", w.join("b/link")).unwrap()),
            ("a deep write", |w| std::fs::write(w.join("a/project/src/main.rs"), "fn main() { loop {} }").unwrap()),
        ];
        for (what, change) in changes {
            let d = data("change");
            assert_eq!(run(STAMP_RESTORED, &d, &[ID]).0, Some(0));
            assert!(unchanged(&d, ID), "{what}: unchanged before it");
            change(&d.join("work"));
            let (exit, out) = run(WORK_CHANGED, &d, &[ID]);
            assert!(exit == Some(0) && !out.trim().is_empty(), "{what} is seen: {exit:?} {out:?}");
            let _ = std::fs::remove_dir_all(&d);
        }
    }

    /// A backup's stamp: what changes once `STAMP_NEXT` returned (the
    /// backup then starts) is seen by the next save, what came before it is
    /// not; `STAMP_TAKEN` makes it the record's, the only stamp. Replay: a
    /// second `STAMP_TAKEN` finds no stamp to take and changes nothing
    /// more; a later save's stamp takes the place of the first.
    #[test]
    fn a_backups_stamp_counts_from_its_start() {
        let d = data("backup");
        std::fs::write(d.join("work/b/before.txt"), "before").unwrap();
        assert_eq!(run(STAMP_NEXT, &d, &["7"]).0, Some(0));
        assert_eq!(run(STAMP_TAKEN, &d, &["7", ID]).0, Some(0));
        assert!(unchanged(&d, ID), "what came before the stamp is the record's");
        assert_eq!(run(STAMP_TAKEN, &d, &["7", ID]).0, Some(1), "taken once");
        assert!(unchanged(&d, ID), "a replay changes nothing");
        // a write right after the stamp (the backup may have read past it)
        assert_eq!(run(STAMP_NEXT, &d, &["8"]).0, Some(0));
        std::fs::write(d.join("work/b/during.txt"), "during").unwrap();
        const NEXT: &str = "0d6b3c35-7a73-4d4e-8a4f-9e1d2f3a4b5c";
        assert_eq!(run(STAMP_TAKEN, &d, &["8", NEXT]).0, Some(0));
        assert!(!unchanged(&d, NEXT), "a write during the backup is seen");
        assert!(!unchanged(&d, ID), "the older record's stamp is gone");
        let stamps: Vec<_> = std::fs::read_dir(d.join(".work-stamps")).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(stamps, vec![std::ffi::OsString::from(NEXT)]);
        // restart: a save whose isolate went between its stamp and its
        // record leaves the last record's stamp as it was, which still
        // speaks for that record; the next save's taken stamp is the only one
        let _ = std::fs::remove_dir_all(&d);
        let d = data("restart");
        assert_eq!(run(STAMP_RESTORED, &d, &[ID]).0, Some(0));
        assert_eq!(run(STAMP_NEXT, &d, &["11"]).0, Some(0));
        assert!(unchanged(&d, ID), "nothing changed: the restored record still stands");
        std::fs::write(d.join("work/b/after.txt"), "after").unwrap();
        assert!(!unchanged(&d, ID), "a change after it is seen");
        assert_eq!(run(STAMP_NEXT, &d, &["12"]).0, Some(0));
        assert_eq!(run(STAMP_TAKEN, &d, &["12", NEXT]).0, Some(0));
        let stamps: Vec<_> = std::fs::read_dir(d.join(".work-stamps")).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(stamps, vec![std::ffi::OsString::from(NEXT)], "the cut save's stamp and the old record's gone");
        assert!(unchanged(&d, NEXT));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Invalid: no stamp, another record's stamp, a work that is gone, or a `find` that cannot compare change
    /// times (busybox's): each copies. No answer (its bound ran out) copies.
    #[test]
    fn a_check_that_cannot_tell_copies() {
        let d = data("cannot");
        assert!(!unchanged(&d, ID), "no stamp");
        assert_eq!(run(STAMP_RESTORED, &d, &[ID]).0, Some(0));
        assert!(!unchanged(&d, "another-record"), "another record's stamp");
        std::fs::rename(d.join("work"), d.join("gone")).unwrap();
        assert!(!unchanged(&d, ID), "no work");
        std::fs::rename(d.join("gone"), d.join("work")).unwrap();
        assert!(!unchanged(&d, ID), "a work moved away and back changed (its own ctime)");
        assert_eq!(run(STAMP_RESTORED, &d, &[ID]).0, Some(0));
        assert!(unchanged(&d, ID));
        // a find without -newercm, as busybox's
        let bin = d.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("find"), "#!/bin/sh\necho \"find: unrecognized: $*\" >&2\nexit 1\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(bin.join("find"), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
        let (exit, out) = run_with_path(WORK_CHANGED, &d, &[ID], Some(&path));
        assert!(!work_unchanged(exit, &out), "{exit:?} {out}");
        assert_eq!(run_with_path(STAMP_NEXT, &d, &["10"], Some(&path)).0, Some(2), "no stamp from such an image");
        assert_eq!(run_with_path(STAMP_RESTORED, &d, &[ID], Some(&path)).0, Some(2));
        // (that restore's stamp went before its find was asked: none left)
        assert!(!unchanged(&d, ID));
        assert_eq!(run(STAMP_RESTORED, &d, &[ID]).0, Some(0));
        // a mount under the work, or mounts that do not read: no stamp made,
        // none trusted
        let mounts = d.join("mountinfo");
        std::fs::write(&mounts, format!("36 35 98:0 / {}/work/b rw - fuse.r2 r2 rw\n", d.display())).unwrap();
        let fake = [("MOUNTINFO", mounts.to_str().unwrap())];
        assert_eq!(run_env(WORK_CHANGED, &d, &[ID], &fake).0, Some(3));
        assert_eq!(run_env(STAMP_NEXT, &d, &["11"], &fake).0, Some(3));
        assert_eq!(run_env(STAMP_RESTORED, &d, &[ID], &fake).0, Some(3));
        assert!(d.join(format!(".work-stamps/{ID}")).exists(), "a refused restore's stamp touched nothing");
        let none = [("MOUNTINFO", "/nonexistent/mountinfo")];
        assert_eq!(run_env(WORK_CHANGED, &d, &[ID], &none).0, Some(3));
        assert!(unchanged(&d, ID), "with the process's own mounts, as it was");
        // a stamp still in the future (a clock set back) is not trusted
        assert!(Command::new("touch").args(["-d", "tomorrow"]).arg(d.join(format!(".work-stamps/{ID}"))).status().unwrap().success());
        assert!(!unchanged(&d, ID));
        // a restore's stamp whose tick never came (a `find` that sees none
        // newer, a `sleep` at once) is never named: no record's stamp
        std::fs::write(bin.join("find"), "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::write(bin.join("sleep"), "#!/bin/sh\nexit 0\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(bin.join("sleep"), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(run_with_path(STAMP_RESTORED, &d, &[ID], Some(&path)).0, Some(1));
        assert!(!d.join(format!(".work-stamps/{ID}")).exists() && !unchanged(&d, ID), "no tick, no stamp");
        assert!(!work_unchanged(None, ""), "no answer within its bound");
        assert!(!work_unchanged(Some(1), ""));
        assert!(!work_unchanged(Some(0), "/data/work/b"));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A stamp is named by a record's id, never a path.
    #[test]
    fn a_stamp_is_named_by_an_id() {
        assert!(stamp_name_ok(ID));
        for bad in ["", "../x", "a/b", "next 1", "-rf", &"a".repeat(65)] {
            assert!(!stamp_name_ok(bad), "{bad:?}");
        }
    }

    /// A record is kept only by a save that leaves out what it left out:
    /// the same patterns keep it; others, none (a hold unanswered), or a
    /// record named before records said so, copy.
    #[test]
    fn a_record_is_kept_only_by_a_save_that_leaves_out_the_same() {
        let caches = vec!["/*/home/.cache/pip/".to_string(), "/*/browser-profile/*/Cache/".to_string()];
        let record = |name: &str| serde_json::json!({ "id": ID, "dir": "/data/work", "name": name });
        let taken = record(&work_name(4, &caches));
        assert!(kept_for(&taken, &caches));
        assert!(kept_for(&record(&work_name(9, &caches)), &caches), "another start's, the same patterns");
        assert!(!kept_for(&taken, &[]), "a save its guest did not answer leaves out nothing");
        assert!(!kept_for(&taken, &caches[..1]), "a cache named no more");
        assert!(!kept_for(&record(&work_name(4, &[])), &caches));
        assert!(!kept_for(&record("generation 4"), &caches), "named before records said what they leave out");
        assert!(!kept_for(&serde_json::json!({ "id": ID }), &[]));
    }

    /// A store through a shared writable mapping, to a page it dirtied
    /// before a backup's stamp, after that stamp, is seen: the stamp's
    /// writeback makes it fault and change its file's ctime (Codex on
    /// #1379). On a filesystem with writeback (tmpfs has none, so not
    /// `/tmp` here): beside the build's own target.
    #[test]
    fn a_store_through_a_mapping_after_the_stamp_is_seen() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/work-tests");
        std::fs::create_dir_all(&root).unwrap();
        let d = root.join(format!("mapped-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("work")).unwrap();
        let f = d.join("work/mapped.bin");
        std::fs::write(&f, vec![0u8; 4096]).unwrap();
        // maps it, stores (the page dirty), says so, waits for a word, stores again
        let mut py = Command::new("python3")
            .args(["-c", "import mmap,sys\nf=open(sys.argv[1],'r+b')\nm=mmap.mmap(f.fileno(),4096)\nm[0:1]=b'a'\nprint('dirty',flush=True)\nsys.stdin.readline()\nm[1:2]=b'b'\nprint('stored',flush=True)\nsys.stdin.readline()\n"])
            .arg(&f)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("python3 runs");
        use std::io::{BufRead, Write};
        let mut out = std::io::BufReader::new(py.stdout.take().unwrap());
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), "dirty");
        assert_eq!(run(STAMP_NEXT, &d, &["1"]).0, Some(0));
        assert_eq!(run(STAMP_TAKEN, &d, &["1", ID]).0, Some(0));
        assert!(unchanged(&d, ID), "nothing since the stamp");
        let mut input = py.stdin.take().unwrap();
        writeln!(input).unwrap();
        line.clear();
        out.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), "stored");
        let (exit, said) = run(WORK_CHANGED, &d, &[ID]);
        assert!(exit == Some(0) && said.trim().ends_with("mapped.bin"), "the store is seen: {exit:?} {said:?}");
        writeln!(input).unwrap();
        py.wait().unwrap();
        let _ = std::fs::remove_dir_all(&d);
    }
}
