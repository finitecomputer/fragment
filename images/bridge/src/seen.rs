//! Whether a runtime's memory of a chat is behind (M17; FIN-530, FIN-543).
//!
//! The journal says which turns the agent took in a chat (`work`). This
//! marker says which turn its runtime was last handed there: one small file per chat, `fragment-seen/<chat>`, holding
//! that turn's id, in the directory where the runtime keeps the agent's
//! memory of its chats (`Runtime::memory`: Hermes' profile, beside its
//! `state.db`; Claude Code's project, beside its transcripts). So it goes
//! back in time with that memory and is lost with it: a computer that
//! never ran the agent has none, a rollback took it back, a profile lost
//! alone took it. A turn of the agent's that ran after the marker's (or
//! with none) means the runtime is behind (note.rs `behind`), and the turn
//! is told the chat (driver.rs `note_for`, note.rs `recap`); once the turn
//! is handed, the marker names it.
//!
//! One format for every runtime, written by the bridge, never by the
//! runtime. In the end state the agent fragment compares, and the runtime's
//! host keeps the marker as it is (Finite Agent phase 1b, "What stays
//! hard").

use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::records;

/// The marker's directory, in the runtime's memory of an agent.
const DIR: &str = "fragment-seen";

fn file(memory: &Path, chat: &str) -> Option<PathBuf> {
    records::fragment_name_ok(chat).then(|| memory.join(DIR).join(chat))
}

/// The turn the runtime was last handed in `chat`: `None` when no marker
/// is there (or it is not a turn's id).
pub fn read(memory: &Path, chat: &str) -> Option<String> {
    let mut text = String::new();
    std::fs::File::open(file(memory, chat)?).ok()?.take(129).read_to_string(&mut text).ok()?;
    records::valid_post_id(&text).then_some(text)
}

/// The marker names `turn` now: written whole and renamed into place. The
/// directories it needs are made as their parent's owner, so a runtime that
/// has not yet made its memory's directory (Claude Code makes its project's
/// at its first turn) can still write there.
pub fn write(memory: &Path, chat: &str, turn: &str) -> std::io::Result<()> {
    let invalid = |what: &str| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{what} is not a marker's"));
    let path = file(memory, chat).ok_or_else(|| invalid("the chat's name"))?;
    if !records::valid_post_id(turn) {
        return Err(invalid("the turn's id"));
    }
    let dir = path.parent().expect("in its directory");
    made(dir)?;
    let tmp = dir.join(format!(".{chat}.tmp"));
    std::fs::write(&tmp, turn)?;
    std::fs::rename(&tmp, &path)
}

/// `dir`, made with any parent it lacks, each as its own parent's owner.
fn made(dir: &Path) -> std::io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    let parent = dir.parent().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no directory above"))?;
    made(parent)?;
    let owner = std::fs::metadata(parent)?;
    match std::fs::create_dir(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(e),
        _ => {}
    }
    let made = std::fs::metadata(dir)?;
    if (made.uid(), made.gid()) != (owner.uid(), owner.gid()) {
        std::os::unix::fs::chown(dir, Some(owner.uid()), Some(owner.gid()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goal: a marker reads back what was written, per chat; none is
    /// `None`; a name that is no chat's, or a file that is no turn's id, is
    /// none too; the runtime's memory's own directory is made when missing.
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
        std::fs::remove_dir_all(&root).unwrap();
    }
}
