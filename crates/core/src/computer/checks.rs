//! A save's records checked against R2's own copy (FIN-784; docs/computers.md,
//! "Every save and restore is checked end to end"). The Sandbox SDK's shim
//! hashes each archive as it makes it (a record's `sha256` is its producer's
//! own), but R2 checks a part's length alone, and bytes out of a container
//! have come the right length with the wrong contents (FIN-775). So once a
//! save is kept, each record is read back from R2 and hashed (the cell's
//! `stored`) while nobody waits on it, and a save whose copy is not R2's is
//! unusable, failed, and taken again.

/// What a read of R2's copy of a record found.
#[derive(Debug, PartialEq, Eq)]
pub enum Checked {
    /// The record's own size and SHA-256.
    Whole,
    /// Another size or SHA-256, or nothing: why, in words.
    Damaged(String),
    /// No read could be made, or it was given up (R2 failed it, the save's
    /// time ran out, or someone waits on the save): kept unchecked.
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
}
