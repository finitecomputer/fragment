//! The guest's word on a hold (docs/computers.md, "The hold"): what its
//! answer leaves out of a save, and what it says when it has not answered.

use crate::search::cut;

/// A guest's answer to the hold is at most this long, and names at most
/// `LEFT_OUT_MAX` patterns of at most `LEFT_OUT_PATTERN_MAX_BYTES` each:
/// room for an image that names each file it copied (ours: four lines a
/// database, for up to 128 databases).
pub const HELD_ANSWER_MAX_BYTES: usize = 64 * 1024;
pub const LEFT_OUT_MAX: usize = 512;
pub const LEFT_OUT_PATTERN_MAX_BYTES: usize = 256;
const _: () = assert!(LEFT_OUT_MAX * 2 <= HELD_ANSWER_MAX_BYTES, "the answer's bound fits its patterns' count");

/// What a save saves (docs/computers.md, "Data and the restore gate"):
/// `/data/work`, what the guest's tools write, and the rest of `/data`,
/// the guest's own state, each a record of its own and restored together,
/// the first first (a restore replaces its directory whole, so `/data`'s
/// record goes in before the work's). `/data`'s record leaves the work out
/// (the cell's `WORK_LEFT_OUT`), and what a held guest names; the work's
/// record leaves out only what it named under `/work/` (`work_left_out`:
/// its agents' caches). The platform's own paths, which a save's failure
/// names (`glance::why` keeps them).
pub const SAVED_DIRS: [&str; 2] = ["/data", "/data/work"];

/// What a guest's answer to the hold (`/run/computer/held`, docs/computers.md)
/// names as left out of its save: one gitignore pattern a line, relative to
/// `/data`, of letters, digits and `._-/*?[]` (empty lines skipped). These
/// are files the guest copied under the hold to names the save keeps (our
/// Hermes image: its SQLite databases, by SQLite's online backup), so a
/// hot copy of them, which could tear, is never what a wake restores. An
/// answer past its bounds, or with any other line, is refused: the guest is
/// then saved whole, losing nothing.
pub fn left_out(answer: &str) -> Result<Vec<String>, String> {
    if answer.len() > HELD_ANSWER_MAX_BYTES {
        return Err(format!("an answer of {} bytes, past {HELD_ANSWER_MAX_BYTES}", answer.len()));
    }
    let mut patterns = Vec::new();
    for line in answer.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let ok = line.len() <= LEFT_OUT_PATTERN_MAX_BYTES && line.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-/*?[]".contains(&b));
        if !ok {
            return Err(format!("{line:?} is no pattern of letters, digits and ._-/*?[] of at most {LEFT_OUT_PATTERN_MAX_BYTES} bytes"));
        }
        patterns.push(line.to_string());
    }
    if patterns.len() > LEFT_OUT_MAX {
        return Err(format!("{} patterns, past {LEFT_OUT_MAX}", patterns.len()));
    }
    Ok(patterns)
}

/// What the DO keeps of a guest's word on a hold it has not answered.
/// The patterns of a held guest's answer (`left_out`, anchored to `/data`)
/// that its work's own save leaves out, relative to its work (`/data/work`):
/// those under `/work/`, that prefix cut (a cache under an agent's home or
/// its browser's profile). `/data`'s own save leaves its work out whole, so
/// they change nothing there. A pattern of `**` at its top, which could
/// reach anything, is never one: the work's save is the person's.
pub fn work_left_out(left_out: &[String]) -> Vec<String> {
    left_out.iter().filter_map(|p| p.strip_prefix("/work/")).filter(|p| !p.is_empty() && !p.starts_with("**")).map(|p| format!("/{p}")).collect()
}

pub const UNHELD_MAX_BYTES: usize = 1024;

/// What a guest says of a hold it has not answered (`/run/computer/unheld`,
/// docs/computers.md, "The hold"), as the Computer DO logs it once the
/// hold's wait runs out: its text, trimmed, its control characters but
/// newlines as spaces, at most `UNHELD_MAX_BYTES` (cut at a character);
/// `None` for none. Only a log line: nothing is decided by it.
pub fn unheld(text: &str) -> Option<String> {
    let clean: String = text.trim().chars().map(|c| if c.is_control() && c != '\n' { ' ' } else { c }).collect();
    let said = cut(&clean, UNHELD_MAX_BYTES).trim_end();
    (!said.is_empty()).then(|| said.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goal (2026-11-03: saves grew past a computer's disk, caches in its
    /// agents' homes among them): the work's own save leaves out what the
    /// held guest named under `/work/`, relative to the work; nothing else
    /// it named, and never a pattern that could reach all of it.
    #[test]
    fn the_work_leaves_out_what_its_guest_named_in_it() {
        let named: Vec<String> = ["/hermes/state.db", "/work/*/home/.cache/pip/", "/work/*/browser-profile/**/Code?Cache/", "/work/", "/work/**/x", "/workshop/a", "/hermes/.cache/uv/"].iter().map(|s| s.to_string()).collect();
        assert_eq!(work_left_out(&named), ["/*/home/.cache/pip/", "/*/browser-profile/**/Code?Cache/"]);
        assert!(work_left_out(&[]).is_empty());
    }

    /// Valid and invalid answers to the hold: the patterns a guest names
    /// (an empty answer names none), and every answer the platform refuses,
    /// which saves the guest whole.
    #[test]
    fn a_guest_names_what_its_save_leaves_out() {
        assert_eq!(left_out(""), Ok(vec![]));
        assert_eq!(left_out("*.db\n*.db-wal\n\n  *.db-shm\n*.db-journal\n"), Ok(vec!["*.db".into(), "*.db-wal".into(), "*.db-shm".into(), "*.db-journal".into()]));
        assert_eq!(left_out("/work/\nprofiles/*/cache/[ab]?.tmp"), Ok(vec!["/work/".into(), "profiles/*/cache/[ab]?.tmp".into()]));
        let too_long = "x".repeat(LEFT_OUT_PATTERN_MAX_BYTES + 1);
        let too_many = "a\n".repeat(LEFT_OUT_MAX + 1);
        let too_big = "a\n".repeat(HELD_ANSWER_MAX_BYTES);
        for bad in ["!keep.db", "a b", "$(rm -rf /)", "*.db;rm", "é", too_long.as_str(), too_many.as_str(), too_big.as_str()] {
            assert!(left_out(bad).is_err(), "{bad:?}");
        }
    }

    /// A guest's word on a hold it has not answered, as logged: none for
    /// nothing said, its text trimmed, its control characters spaces, and
    /// never past its bound, cut before the character that crosses it (of
    /// each width).
    #[test]
    fn a_guest_says_why_it_has_not_answered_within_a_bound() {
        assert_eq!(unheld(""), None);
        assert_eq!(unheld(" \n\t"), None);
        assert_eq!(unheld("copy refused: copying /data/a.db: file is not a database\n"), Some("copy refused: copying /data/a.db: file is not a database".into()));
        assert_eq!(unheld("a\u{1b}[31mb\nc"), Some("a [31mb\nc".into()));
        for c in ['é', '€', '𝄞'] {
            let long = format!("{}{c}{}", "x".repeat(UNHELD_MAX_BYTES - 1), "y".repeat(100));
            assert_eq!(unheld(&long), Some("x".repeat(UNHELD_MAX_BYTES - 1)), "{c:?}");
        }
        assert_eq!(unheld(&"x".repeat(UNHELD_MAX_BYTES + 1)), Some("x".repeat(UNHELD_MAX_BYTES)), "at its bound, whole");
    }
}
