//! A save's records checked against R2's own copy (FIN-784; docs/computers.md,
//! "Every save and restore is checked end to end"). The Sandbox SDK's shim
//! hashes each archive as it makes it (a record's `sha256` is its producer's
//! own), but R2 checks a part's length alone, and bytes out of a container
//! have come the right length with the wrong contents (FIN-775). So once a
//! save's backups are all taken, each record is read back from R2 and
//! hashed (the cell's `stored`), and kept only when R2 holds it.

/// Tries of one directory's backup in all while R2's copy of it comes out
/// damaged: each moves its whole archive again, within the save's own
/// bound (the cell's `BACKUPS_MS`).
pub const SAVE_TRIES: u32 = 3;
const _: () = assert!(SAVE_TRIES >= 3, "damage on the way out is tried again twice at least");

/// What a read of R2's copy of a record found.
#[derive(Debug, PartialEq, Eq)]
pub enum Checked {
    /// The record's own size and SHA-256.
    Whole,
    /// Another size or SHA-256, or nothing: why, in words.
    Damaged(String),
    /// No read could be made (R2 failed it, or the save's time ran out).
    Unread(String),
}

/// What R2 holds of a record of `size` bytes and SHA-256 `sha256`: `held`,
/// the size and SHA-256 its read found, or None when nothing is there.
pub fn checked(size: u64, sha256: &str, held: Option<(u64, &str)>) -> Checked {
    match held {
        Some((n, sha)) if n == size && sha == sha256 => Checked::Whole,
        Some((n, sha)) => Checked::Damaged(format!("R2's copy holds {n} bytes of SHA-256 {sha}, not its record's {size} bytes of SHA-256 {sha256}")),
        None => Checked::Damaged(format!("R2's copy is gone, not its record's {size} bytes of SHA-256 {sha256}")),
    }
}

/// What a save does with a record so checked.
#[derive(Debug, PartialEq, Eq)]
pub enum AfterCheck {
    /// It is R2's: kept.
    Keep,
    /// It could not be read: kept unchecked, as before this check, so a
    /// save that fits its bounds never fails for want of its check.
    Unchecked(String),
    /// It came out damaged: let go of, and its directory backed up again.
    Again(String),
    /// It came out damaged, and is not backed up again here: the save
    /// fails, and its lifecycle tries it again.
    Fail(String),
}

/// A record's `tried`-th backup (from 1), `checked`. `again`: its save is
/// a sleep's own, still the one its lifecycle asked for, its guest held to
/// its end, so its directory may be backed up again as it was taken. An
/// awake save's is not: its hold is let go once its home is taken (a home
/// backed up after that could be torn as its guest writes), and its backup
/// could hold up the one an owner's sleep or restart asks meanwhile (the
/// container takes one backup at a time). It fails instead, tried again
/// later at no cost: its computer is awake, its last save kept.
pub fn after_check(checked: Checked, tried: u32, again: bool) -> AfterCheck {
    assert!(tried >= 1, "a record checked was backed up once at least");
    match checked {
        Checked::Whole => AfterCheck::Keep,
        Checked::Unread(why) => AfterCheck::Unchecked(why),
        Checked::Damaged(why) if !again => AfterCheck::Fail(format!("{why}; not backed up again: no sleep's own save")),
        Checked::Damaged(why) if tried >= SAVE_TRIES => AfterCheck::Fail(format!("{why} (it came out damaged {tried} times)")),
        Checked::Damaged(why) => AfterCheck::Again(why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OTHER: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    /// Goal: a record is R2's only at its own size and SHA-256. Valid (both),
    /// invalid (another SHA-256 at the same length, as FIN-775's damage came;
    /// another size; nothing there).
    #[test]
    fn a_record_is_r2s_at_its_own_size_and_sha256_alone() {
        assert_eq!(checked(100, SHA, Some((100, SHA))), Checked::Whole);
        assert_eq!(checked(100, SHA, Some((100, OTHER))), Checked::Damaged(format!("R2's copy holds 100 bytes of SHA-256 {OTHER}, not its record's 100 bytes of SHA-256 {SHA}")));
        assert!(matches!(checked(100, SHA, Some((99, SHA))), Checked::Damaged(why) if why.starts_with("R2's copy holds 99 bytes")));
        assert!(matches!(checked(100, SHA, None), Checked::Damaged(why) if why.starts_with("R2's copy is gone")));
    }

    /// Goal: a damaged copy is backed up again in a sleep's own save, a
    /// bounded number of times, then fails the save; in any other (an awake
    /// save's, or one a newer save overtook as it ran) it fails at once; one
    /// that could not be read is kept, never failing a save. Valid (whole),
    /// invalid (damaged, a sleep's or not), the bound (`SAVE_TRIES`), and
    /// replay (the same verdict at each try).
    #[test]
    fn a_damaged_copy_is_backed_up_again_a_bounded_number_of_times_in_a_sleeps_own_save() {
        let damaged = || Checked::Damaged("R2's copy holds …".into());
        assert_eq!(after_check(Checked::Whole, 1, true), AfterCheck::Keep);
        assert_eq!(after_check(Checked::Whole, SAVE_TRIES, false), AfterCheck::Keep);
        for tried in 1..SAVE_TRIES {
            assert_eq!(after_check(damaged(), tried, true), AfterCheck::Again("R2's copy holds …".into()), "try {tried}");
        }
        assert!(matches!(after_check(damaged(), SAVE_TRIES, true), AfterCheck::Fail(why) if why.ends_with(&format!("(it came out damaged {SAVE_TRIES} times)"))));
        assert!(matches!(after_check(damaged(), 1, false), AfterCheck::Fail(why) if why.contains("no sleep's own save")));
        for tried in 1..=SAVE_TRIES {
            assert_eq!(after_check(Checked::Unread("no answer within 10 ms".into()), tried, false), AfterCheck::Unchecked("no answer within 10 ms".into()));
        }
    }
}
