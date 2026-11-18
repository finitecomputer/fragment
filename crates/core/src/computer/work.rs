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
//!   write after has a change time newer than the stamp's.
//! - A write during the backup is after its stamp: the next save copies.
//! - A delete or a rename changes its directory; an edit, a chmod, a
//!   `touch -d` or a copy that keeps old times changes its file's ctime.
//! - What the work's record leaves out (the caches its guest names) is
//!   walked too: a change there copies, erring toward saving.
//!
//! What it trusts: the container's clock going forward (a root guest that
//! sets it back can hide its own changes from its own next save), and a
//! filesystem mounted under the work having none (one found copies).

/// Where the stamps are kept, relative to `/data`: root's alone (a tool run
/// as the guest's user cannot touch one), and left out of the home's
/// record, so a restore of it brings back none.
pub const WORK_STAMPS: &str = "/.work-stamps/";

/// The longest a check of the work (`WORK_CHANGED`) is given: a walk of
/// 195,000 files took 1.1–9.5 s (preview, 2026-11-18). Past it, it copies.
pub const WORK_CHECK_MS: i64 = 30_000;

/// Each script runs as `sh -c <script> sh <data> <args>…` (`$1` is `/data`,
/// a test's own directory in tests). The tick wait after a stamp `$s`:
/// until a probe touched now is newer than it, at most about 3 s (a
/// filesystem that keeps whole seconds waits up to one), else it failed.
macro_rules! tick_after {
    ($stamp:literal) => {
        concat!(
            "i=0\nuntil touch .probe && [ -n \"$(find .probe -newer \"",
            $stamp,
            "\")\" ]; do i=$((i+1)); [ \"$i\" -le 300 ] || exit 1; sleep 0.01; done\nrm -f .probe\n"
        )
    };
}

/// Before the work's backup (`$2`: the save's number): its stamp, then
/// the tick past it. Exit 2: this image's `find` cannot compare change
/// times (`-newercm`, GNU findutils'), so no save of it keeps its work.
pub const STAMP_NEXT: &str = concat!(
    "cd \"$1\" && mkdir -p -m 700 .work-stamps && cd .work-stamps && find . -maxdepth 0 -newercm . >/dev/null 2>&1 || exit 2\n",
    "rm -f \"next-$2\" && touch \"next-$2\" || exit 1\n",
    tick_after!("next-$2"),
);

/// Once the save `$2` is kept with the new work record `$3`: its stamp is
/// that record's, the only one (another save's under way, ended first,
/// loses its own, and copies next time).
pub const STAMP_TAKEN: &str = "cd \"$1/.work-stamps\" && test -f \"next-$2\" || exit 1\nfor f in *; do [ \"$f\" = \"next-$2\" ] || rm -f \"$f\"; done\nmv \"next-$2\" \"$3\"\n";

/// Once a start restored its save, the work's record `$2` among it, and
/// before its check and its gate: that record's stamp, the only one, then
/// the tick past it.
pub const STAMP_RESTORED: &str = concat!(
    "cd \"$1\" && rm -rf .work-stamps && mkdir -m 700 .work-stamps && cd .work-stamps && find . -maxdepth 0 -newercm . >/dev/null 2>&1 || exit 2\n",
    "touch \"$2\" || exit 1\n",
    tick_after!("$2"),
);

/// Whether anything under the work changed since the stamp of record
/// `$2`: the first path that did, or nothing. Exit 1 with no such stamp,
/// or a mount under the work.
pub const WORK_CHANGED: &str =
    "test -f \"$1/.work-stamps/$2\" || exit 1\n! grep -qs \" $1/work[ /]\" /proc/self/mountinfo || exit 1\nfind \"$1/work\" -newercm \"$1/.work-stamps/$2\" -print -quit\n";

/// Whether `WORK_CHANGED`'s run says the work is unchanged: it exited 0
/// and printed nothing. Anything else (a path, an error, no answer within
/// its bound: `None`) copies it.
pub fn work_unchanged(exit: Option<i64>, output: &str) -> bool {
    exit == Some(0) && output.trim().is_empty()
}

/// Whether `id` names a stamp: a record's id as `DirectoryBackup` makes
/// them (a UUID), never a path.
pub fn stamp_name_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
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
        let mut c = Command::new("sh");
        c.arg("-c").arg(script).arg("sh").arg(data).args(args);
        if let Some(p) = path {
            c.env("PATH", p);
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
        assert!(!work_unchanged(None, ""), "no answer within its bound");
        assert!(!work_unchanged(Some(1), ""));
        assert!(!work_unchanged(Some(0), "/data/work/b"));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A stamp is named by a record's id, never a path.
    #[test]
    fn a_stamp_is_named_by_an_id() {
        assert!(stamp_name_ok(ID));
        for bad in ["", "../x", "a/b", "next 1", &"a".repeat(65)] {
            assert!(!stamp_name_ok(bad), "{bad:?}");
        }
    }
}
