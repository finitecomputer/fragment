//! Whether a runtime's memory of a chat is behind (M17; FIN-530, FIN-543).
//!
//! The journal says which turns the agent took in a chat (`work`). This
//! marker says which turn its runtime last took there: one small file per
//! chat, `fragment-seen/<chat>`, holding that turn's id, in the directory
//! where the runtime keeps the agent's memory of its chats
//! (`Runtime::memory`: Hermes' profile, beside its `state.db`; Claude
//! Code's project, beside its transcripts). So it goes back in time with
//! that memory and is lost with it: a computer that never ran the agent
//! has none, a rollback took it back, a profile lost alone took it. A turn
//! of the agent's that ran after the marker's (or with none) means the
//! runtime is behind (note.rs `behind`), and the turn is told the chat
//! (driver.rs `note_for`, note.rs `recap`); once the runtime has the turn
//! (`Event::Taken`), the marker names it.
//!
//! One format for every runtime, written by the bridge, never by the
//! runtime. In the end state the agent fragment compares, and the runtime's
//! host keeps the marker as it is (Finite Agent phase 1b, "What stays
//! hard").
//!
//! The runtime's user, and so its agent's tools, may change anything in
//! that directory, and the bridge is root. Each directory is opened once,
//! never through a link, and everything in it is done relative to it: a
//! directory swapped for another after it was opened changes nothing. Only
//! a plain file is read, and only a file of its own making is written.

use std::ffi::{CString, OsStr};
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::records;

/// The marker's directory, in the runtime's memory of an agent.
const DIR: &str = "fragment-seen";

/// The turn the runtime last took in `chat`: `None` when no marker is
/// there (or it is not a plain file holding a turn's id).
pub fn read(memory: &Path, chat: &str) -> Option<String> {
    if !records::fragment_name_ok(chat) {
        return None;
    }
    let seen = open_at(Some(&open_at(None, memory.as_os_str(), libc::O_DIRECTORY, 0).ok()?), DIR.as_ref(), libc::O_DIRECTORY, 0).ok()?;
    // a pipe opened so never waits for a writer, and is not read
    let f = open_at(Some(&seen), chat.as_ref(), libc::O_NONBLOCK, 0).ok()?;
    if !f.metadata().ok()?.is_file() {
        return None;
    }
    let mut text = String::new();
    f.take(129).read_to_string(&mut text).ok()?;
    records::valid_post_id(&text).then_some(text)
}

/// The marker names `turn` now: written whole and renamed into place. The
/// directories it needs are made as their parent's owner, so a runtime that
/// has not yet made its memory's directory (Claude Code makes its project's
/// at its first turn) can still write there.
pub fn write(memory: &Path, chat: &str, turn: &str) -> std::io::Result<()> {
    let invalid = |what: &str| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{what} is not a marker's"));
    if !records::fragment_name_ok(chat) {
        return Err(invalid("the chat's name"));
    }
    if !records::valid_post_id(turn) {
        return Err(invalid("the turn's id"));
    }
    let seen = made(&memory.join(DIR))?;
    // a file of its own: whatever is at the temporary name goes (a link
    // itself, never what it names), and the new one is made, not opened
    let tmp = format!(".{chat}.tmp");
    let _ = at(&seen, tmp.as_ref(), |d, t| unsafe { libc::unlinkat(d, t, 0) });
    open_at(Some(&seen), tmp.as_ref(), libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL, 0o644)?.write_all(turn.as_bytes())?;
    let to = name(chat.as_ref())?;
    at(&seen, tmp.as_ref(), |d, t| unsafe { libc::renameat(d, t, d, to.as_ptr()) })
}

/// `dir`, opened (never through a link at its own name), and made first
/// with any parent it lacks, each made in its opened parent as that
/// parent's owner.
fn made(dir: &Path) -> std::io::Result<File> {
    match open_at(None, dir.as_os_str(), libc::O_DIRECTORY, 0) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        opened => return opened,
    }
    let (Some(parent), Some(own)) = (dir.parent(), dir.file_name()) else { return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no directory above")) };
    let parent = made(parent)?;
    let owner = parent.metadata()?;
    match at(&parent, own, |d, n| unsafe { libc::mkdirat(d, n, 0o755) }) {
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(e),
        _ => {}
    }
    at(&parent, own, |d, n| unsafe { libc::fchownat(d, n, owner.uid(), owner.gid(), libc::AT_SYMLINK_NOFOLLOW) })?;
    open_at(Some(&parent), own, libc::O_DIRECTORY, 0)
}

fn name(n: &OsStr) -> std::io::Result<CString> {
    CString::new(n.as_bytes()).map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "a name with a NUL"))
}

/// `call` on the name `n` in the directory `dir`, as the libc call it is.
fn at(dir: &File, n: &OsStr, call: impl FnOnce(libc::c_int, *const libc::c_char) -> libc::c_int) -> std::io::Result<()> {
    let n = name(n)?;
    if call(dir.as_raw_fd(), n.as_ptr()) < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// `n` in `dir` (none: from the working directory, a path), opened with
/// `flags`, never through a link at `n` itself.
fn open_at(dir: Option<&File>, n: &OsStr, flags: libc::c_int, mode: libc::c_uint) -> std::io::Result<File> {
    let c = name(n)?;
    let fd = unsafe { libc::openat(dir.map_or(libc::AT_FDCWD, |d| d.as_raw_fd()), c.as_ptr(), flags | libc::O_NOFOLLOW | libc::O_CLOEXEC, mode) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: openat returned a new descriptor, owned here alone
    Ok(File::from(unsafe { OwnedFd::from_raw_fd(fd) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goal: a marker reads back what was written, per chat; none is
    /// `None`; a name that is no chat's, or a file that is no turn's id, is
    /// none too; the runtime's memory's own directory is made when missing.
    /// What the runtime's user may leave there is never followed: a link
    /// as the marker, its temporary file, its directory or the memory's; a
    /// pipe is never waited on.
    #[test]
    fn a_marker_names_the_turn_last_handed() {
        let root = std::env::temp_dir().join(format!("seen-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let memory = root.join("projects/-data-work-juniper--k3x9");
        assert_eq!(read(&memory, "talk--k3x9"), None, "none yet");
        write(&memory, "talk--k3x9", "aa11").unwrap();
        write(&memory, "talk--k3x9", "bb22").unwrap();
        write(&memory, "other--k3x9", "cc33").unwrap();
        assert_eq!((read(&memory, "talk--k3x9").as_deref(), read(&memory, "other--k3x9").as_deref()), (Some("bb22"), Some("cc33")));
        assert!(write(&memory, "../up", "dd44").is_err() && read(&memory, "../up").is_none(), "a chat's name only");
        assert!(write(&memory, "talk--k3x9", "a b").is_err(), "a turn's id only");
        std::fs::write(memory.join(DIR).join("odd--k3x9"), "not a turn\n").unwrap();
        assert_eq!(read(&memory, "odd--k3x9"), None);
        let outside = root.join("outside");
        std::fs::write(&outside, "aa11").unwrap();
        std::os::unix::fs::symlink(&outside, memory.join(DIR).join("linked--k3x9")).unwrap();
        assert_eq!(read(&memory, "linked--k3x9"), None, "a link to a turn's id is no marker");
        std::os::unix::fs::symlink(&outside, memory.join(DIR).join(".linked--k3x9.tmp")).unwrap();
        write(&memory, "linked--k3x9", "ee55").unwrap();
        assert_eq!((std::fs::read_to_string(&outside).unwrap().as_str(), read(&memory, "linked--k3x9").as_deref()), ("aa11", Some("ee55")), "written as a file of its own");
        assert!(std::process::Command::new("mkfifo").arg(memory.join(DIR).join("piped--k3x9")).status().unwrap().success());
        assert_eq!(read(&memory, "piped--k3x9"), None, "a pipe: never waited on");
        // a link as the marker's directory, or as the memory itself
        let elsewhere = root.join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("talk--k3x9"), "ff66").unwrap();
        let swapped = root.join("swapped");
        std::fs::create_dir(&swapped).unwrap();
        std::os::unix::fs::symlink(&elsewhere, swapped.join(DIR)).unwrap();
        assert_eq!(read(&swapped, "talk--k3x9"), None, "a linked directory is not read through");
        assert!(write(&swapped, "talk--k3x9", "aa77").is_err(), "nor written through");
        let linked = root.join("linked-memory");
        std::os::unix::fs::symlink(&swapped, &linked).unwrap();
        assert!(write(&linked, "talk--k3x9", "aa88").is_err() && read(&linked, "talk--k3x9").is_none(), "a memory that is a link is never used");
        assert_eq!(std::fs::read_to_string(elsewhere.join("talk--k3x9")).unwrap(), "ff66", "nothing outside was touched");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
