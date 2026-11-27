//! A computer's disk (docs/computers.md, "A disk"): `/data` as an FPAC
//! volume, a lazy disk whose blocks live in R2 as write-once objects
//! under `disks/<computer>/`, served to the guest's daemon by the cell's
//! `fpac` egress route (FPAC's contract, docs/contract.md in
//! finitecomputer/fpac at v0.1.1). The platform's part, here as pure rules:
//!
//! - **The epoch** is the computer's start generation: minted at each
//!   start, larger than every earlier one, so a container from an older
//!   start can write only under its old epoch.
//! - **The head** names the checkpoint a disk opens from. The Computer DO
//!   keeps it as typed columns and moves it only by `decide`, the same
//!   rules as FPAC's own (`fpac_store::head::decide` at v0.1.1, which its
//!   conformance suite holds every route to; the cases below are its).
//! - **Object names** are `e<epoch>/<rest>`; the route reads and writes
//!   nothing else, and writes only under the running life's epoch.

use serde::{Deserialize, Serialize};

use crate::computer::limits::{DISK_EXIT_WAIT_MS, STOP_LOOK_MS};

/// A disk's volume, as FPAC sees it: the computer's 20 GB (decision 18's
/// disk, docs/computers.md "An image's share of the disk"), a whole number
/// of FPAC's 64 KiB extents.
pub const DISK_VOLUME_BYTES: u64 = 20_000 << 20;

/// An object's largest body: a pack's (96 MiB), under the intercept's
/// 100 MiB request body limit. FPAC's `OBJECT_BYTES_MAX`.
pub const OBJECT_BYTES_MAX: u64 = 96 << 20;

/// An object's name, at most: `e` + 20 digits + `/` + a kind and a number,
/// with room. FPAC's `NAME_BYTES_MAX`.
pub const NAME_BYTES_MAX: usize = 96;

/// A head's line, at most: two 16-digit numbers, a sequence, 64 hex digits
/// and ` clean`, with room. A longer post is refused unread.
pub const HEAD_LINE_MAX_BYTES: usize = 200;

/// A head's numbers, at most: 2^53 - 1, so every reader (the DO's SQLite,
/// a JavaScript number) holds them exactly. FPAC's `HEAD_NUMBER_MAX`.
pub const HEAD_NUMBER_MAX: u64 = (1 << 53) - 1;

/// The checkpoint a head names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointRef {
    pub epoch: u64,
    pub seq: u64,
    /// Its object's SHA-256.
    pub sum: [u8; 32],
}

/// A disk's head: the life that posted it, the checkpoint it opens from
/// (none before its first), and whether that life slept cleanly (its
/// checkpoint covers everything it shipped, so the next open replays
/// nothing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Head {
    pub epoch: u64,
    pub checkpoint: Option<CheckpointRef>,
    pub clean: bool,
}

impl Head {
    /// Its one spelling: `<epoch> <checkpoint epoch> <checkpoint seq>
    /// <sha256 hex>`, or `<epoch> - - -` before a first checkpoint, and
    /// ` clean` after it for a clean sleep.
    pub fn encode(&self) -> String {
        let line = match self.checkpoint {
            Some(c) => format!("{} {} {} {}", self.epoch, c.epoch, c.seq, hex::encode(c.sum)),
            None => format!("{} - - -", self.epoch),
        };
        if self.clean {
            line + " clean"
        } else {
            line
        }
    }

    /// A head from its line: only the form `encode` writes (no sign, no
    /// leading zero, lowercase hex, numbers within `HEAD_NUMBER_MAX`), so
    /// a head has one spelling; anything else is None.
    pub fn decode(line: &str) -> Option<Head> {
        if line.len() > HEAD_LINE_MAX_BYTES {
            return None;
        }
        let parts: Vec<&str> = line.split(' ').collect();
        let (epoch, ck_epoch, ck_seq, sum, clean) = match parts.as_slice() {
            [e, ce, cs, sum] => (*e, *ce, *cs, *sum, false),
            [e, ce, cs, sum, "clean"] => (*e, *ce, *cs, *sum, true),
            _ => return None,
        };
        let epoch = head_number(epoch)?;
        let checkpoint = if ck_epoch == "-" && ck_seq == "-" && sum == "-" {
            None
        } else {
            let mut s = [0u8; 32];
            if sum.len() != 64 || sum.bytes().any(|c| c.is_ascii_uppercase()) || hex::decode_to_slice(sum, &mut s).is_err() {
                return None;
            }
            Some(CheckpointRef { epoch: head_number(ck_epoch)?, seq: head_number(ck_seq)?, sum: s })
        };
        let head = Head { epoch, checkpoint, clean };
        if head.encode() == line {
            Some(head)
        } else {
            None
        }
    }
}

/// A head's number: decimal, at least 1, at most `HEAD_NUMBER_MAX`, with
/// no sign or leading zero.
fn head_number(s: &str) -> Option<u64> {
    let digits = !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()) && !s.starts_with('0');
    if digits {
        s.parse::<u64>().ok().filter(|n| *n <= HEAD_NUMBER_MAX)
    } else {
        None
    }
}

/// What a disk's boot found of the head it was handed and could not mount
/// (its `POST /unusable`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Unopened {
    /// FPAC's own word: the head will not open (what it names gone or
    /// corrupt).
    #[default]
    Unusable,
    /// The device blank: its head a claim, nothing of it replayed or mapped
    /// (nothing of it ever shipped, so nothing was kept).
    Blank,
    /// The device opened, what shipped replayed, but its filesystem would
    /// not mount: a format cut short (FPAC ships from before `mke2fs`).
    Unmountable,
}

impl Unopened {
    /// As the platform's lines and levers say it.
    pub fn name(self) -> &'static str {
        match self {
            Unopened::Unusable => "unusable",
            Unopened::Blank => "blank",
            Unopened::Unmountable => "unmountable",
        }
    }
}

/// Where a disk's start goes past a head its guest found would not open
/// (`head`, the one it was handed, and `back`, the one before it). A disk
/// none of whose lives came up (`came_up`: its guest ran, after a new
/// disk's format had shipped) holds nothing of anyone's, so whatever its
/// boot found, a new disk again; so too a blank device none of whose heads
/// names a checkpoint. A filesystem that would not mount on a disk that
/// came up is tried again as it is (a mount's error may pass), never rolled
/// back (Codex on #1552). Else the one before it; else nowhere, a failed
/// start, whatever it kept left as it is for an operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fallback {
    To(Head),
    Fresh,
    Nowhere,
}

pub fn fallback(head: Option<&Head>, back: Option<&Head>, found: Unopened, came_up: bool) -> Fallback {
    let checkpointless = |h: Option<&Head>| h.is_none_or(|h| h.checkpoint.is_none());
    let fresh = !came_up || (found == Unopened::Blank && checkpointless(head) && checkpointless(back));
    match (head, back) {
        (Some(_), _) if fresh => Fallback::Fresh,
        _ if found == Unopened::Unmountable => Fallback::Nowhere,
        (_, Some(b)) => Fallback::To(*b),
        _ => Fallback::Nowhere,
    }
}

/// The longest a disk's sleep keeps its container for its final flush,
/// whatever the deployment's unsaved bound says: its guest's wait
/// (`guest_flush_ms`) and the runtime's idle stop moved for it fit past it
/// within Cloudflare's own cap on an idle timeout (six hours).
pub const FLUSH_KEPT_MAX_MS: i64 = 5 * 3_600_000;
/// Cloudflare's cap on a container's idle timeout (`setInactivityTimeout`).
pub const IDLE_TIMEOUT_MAX_MS: i64 = 6 * 3_600_000;
const _: () = assert!(FLUSH_KEPT_MAX_MS + 2 * STOP_LOOK_MS + DISK_EXIT_WAIT_MS <= IDLE_TIMEOUT_MAX_MS, "the guest's wait fits the runtime's idle cap");

/// How long a disk's sleep keeps its container for its final flush, from
/// its first signal (FIN-783): until that flush lands, within the unsaved
/// bound its start was given (`unsaved_max_ms`, the time a sleep that
/// cannot save stays awake and tries again), never under its exit wait
/// (`DISK_EXIT_WAIT_MS`, a flush's own time) nor past `FLUSH_KEPT_MAX_MS`.
/// Past it, the container goes and that life ends unsaved, saying so.
pub fn flush_kept_ms(unsaved_max_ms: i64) -> i64 {
    unsaved_max_ms.clamp(DISK_EXIT_WAIT_MS, FLUSH_KEPT_MAX_MS)
}

/// How long a stop waits for its guest's exit now: its exit wait, never
/// past what it keeps (`kept_ms` from `since_ms`); 0 once that passed.
pub fn flush_wait_ms(since_ms: i64, now_ms: i64, kept_ms: i64) -> i64 {
    since_ms.saturating_add(kept_ms).saturating_sub(now_ms).clamp(0, DISK_EXIT_WAIT_MS)
}

/// How long a disk's guest waits for its final flush, at most
/// (`FRAGMENT_DISK_FLUSH_MS`): past what the platform keeps (`kept_ms`)
/// by a look at it and its destroy, so the platform's destroy, never the
/// guest, ends a flush that cannot land; the runtime's idle stop is moved
/// as far at its signal.
pub fn guest_flush_ms(kept_ms: i64) -> i64 {
    assert!((DISK_EXIT_WAIT_MS..=FLUSH_KEPT_MAX_MS).contains(&kept_ms), "a kept bound is `flush_kept_ms`'s");
    kept_ms + 2 * STOP_LOOK_MS + DISK_EXIT_WAIT_MS
}

/// Whether a guest's read of its view is its running life's, and so heard
/// and, on a disk, keeps it as come up: on a disk, only a read that names
/// the life that runs (its start's generation, its route's); one in flight
/// from a start that ended names that start (Codex on #1552). Off a disk,
/// every read (its route names no life).
pub fn read_is_running_lifes(on_disk: bool, read: Option<u64>, running: Option<u64>) -> bool {
    !on_disk || (read.is_some() && read == running)
}

/// How long after a wiped computer's container was gone its disk's objects
/// are listed once more before the wipe is done: past any put its route
/// took before then (R2 lands a put whose body it had within its own
/// request, seconds in practice; it promises no bound), so one that landed
/// after an earlier list, or whose own delete failed, is deleted too
/// (Codex on #1552).
pub const WIPE_SETTLE_MS: i64 = 120_000;

/// How long from `now_ms` until a list may be a wipe's last of its disk,
/// its container gone at `gone_ms` (`WIPE_SETTLE_MS`): 0 once it may. The
/// wipe's call waits that out within its own time (cell/src/wipe.rs), so
/// an operator's wipe goes on as its disk settles.
pub fn wipe_settle_left(gone_ms: i64, now_ms: i64) -> i64 {
    gone_ms.saturating_add(WIPE_SETTLE_MS).saturating_sub(now_ms).max(0)
}

/// The platform's answer to a head a life posts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Take,
    /// The same head again: an earlier post whose answer was lost.
    Same,
    Refuse(&'static str),
}

/// Whether the platform takes `proposed` over `current`, from the life
/// `running` (the start generation it minted). Only the running life may
/// move the head; within a life the checkpoint only moves forward, and the
/// same checkpoint may gain the clean mark of a sleep but never lose it; a
/// new life may name any checkpoint (it opened from the old head, and is
/// the only writer).
pub fn decide(current: Option<&Head>, running: u64, proposed: &Head) -> Decision {
    if proposed.epoch != running {
        return Decision::Refuse("only the running life posts a head");
    }
    if proposed.checkpoint.is_some_and(|c| c.epoch > proposed.epoch) {
        return Decision::Refuse("a checkpoint from a later life");
    }
    let Some(cur) = current else { return Decision::Take };
    if cur == proposed {
        return Decision::Same;
    }
    if cur.epoch > proposed.epoch {
        return Decision::Refuse("an earlier life's head");
    }
    if cur.epoch < proposed.epoch {
        return Decision::Take;
    }
    // the same life: a later checkpoint of its own, only
    match (cur.checkpoint, proposed.checkpoint) {
        // a life that shipped nothing sleeps cleanly
        (None, None) if proposed.clean => Decision::Take,
        (_, None) => Decision::Refuse("a life's head loses its checkpoint"),
        (None, Some(p)) => {
            if p.epoch == proposed.epoch {
                Decision::Take
            } else {
                Decision::Refuse("a life's head goes back to another life's checkpoint")
            }
        }
        (Some(c), Some(p)) => {
            if p.epoch != proposed.epoch && p != c {
                Decision::Refuse("a life's head goes back to another life's checkpoint")
            } else if p == c {
                // the same checkpoint: only the clean mark changed
                if proposed.clean {
                    Decision::Take
                } else {
                    Decision::Refuse("a clean head loses its mark")
                }
            } else if p.epoch == c.epoch && p.seq == c.seq {
                Decision::Refuse("the same checkpoint with another body")
            } else if p.epoch == c.epoch && p.seq < c.seq {
                Decision::Refuse("a checkpoint older than the head's")
            } else {
                Decision::Take
            }
        }
    }
}

/// An object's name, checked: `e<epoch>/<rest>`, the epoch a number with
/// no leading zero, at least 1 and at most 20 digits, the rest a lowercase
/// letter then lowercase letters, digits, `.` and `-`, at most
/// `NAME_BYTES_MAX` in all. Answers its epoch.
pub fn object_epoch(name: &str) -> Option<u64> {
    if name.len() > NAME_BYTES_MAX {
        return None;
    }
    let (e, rest) = name.strip_prefix('e')?.split_once('/')?;
    let digits = !e.is_empty() && e.len() <= 20 && e.bytes().all(|c| c.is_ascii_digit()) && !e.starts_with('0');
    let r = rest.as_bytes();
    let rest_ok = !r.is_empty() && r[0].is_ascii_lowercase() && r.iter().all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'.' || c == b'-');
    if digits && rest_ok {
        e.parse().ok()
    } else {
        None
    }
}

/// A volume's unit: what the daemon counts its extents in (FPAC's
/// `EXTENT_BYTES`).
pub const EXTENT_BYTES: u64 = 64 << 10;
const _: () = assert!(DISK_VOLUME_BYTES.is_multiple_of(EXTENT_BYTES), "a disk's volume is whole extents");

/// What a disk's daemon says of itself (`fpac status`, its
/// `/run/fpac/status.json` at v0.1.1), as a sleep reads it: the bytes it
/// has not shipped (written and not cut, or cut and not landed), the
/// extents a read fetched, and the fetches it made again (a checksum
/// that did not match, or the remote unavailable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub unshipped_bytes: u64,
    pub misses: u64,
    pub refetches: u64,
}

impl Status {
    /// None: an answer that is not its status (unread, or another
    /// release's).
    pub fn read(answer: &str) -> Option<Status> {
        let v: serde_json::Value = serde_json::from_str(answer.trim()).ok()?;
        let n = |key: &str| v.get(key).and_then(serde_json::Value::as_u64);
        Some(Status { unshipped_bytes: n("unshipped_extents")?.checked_mul(EXTENT_BYTES)?, misses: n("misses")?, refetches: n("fetch_retries")? })
    }
}

/// Where a computer's disk lives in the bucket: `disks/<its id's hex>/`,
/// every object of every life under it (a wipe deletes it whole).
pub fn disk_prefix(computer: &str) -> Option<String> {
    let hex = computer.strip_prefix("computer:")?;
    if hex.len() == 24 && hex.bytes().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) {
        Some(format!("disks/{hex}/"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(epoch: u64, ck: Option<(u64, u64, u8)>, clean: bool) -> Head {
        Head { epoch, checkpoint: ck.map(|(e, s, b)| CheckpointRef { epoch: e, seq: s, sum: [b; 32] }), clean }
    }

    // Goal: a head has one spelling, and every other spelling is refused.
    // Method: each form encoded and decoded back; signs, leading zeros,
    // uppercase hex, short sums, extra words and numbers past the bound.
    #[test]
    fn a_head_has_one_spelling() {
        for h in [head(7, None, false), head(7, None, true), head(9, Some((7, 3, 0xab)), false), head(9, Some((9, 1, 0x01)), true)] {
            assert_eq!(Head::decode(&h.encode()), Some(h), "{}", h.encode());
        }
        let sum = "ab".repeat(32);
        for bad in [
            "".to_string(),
            "0 - - -".into(),
            "07 - - -".into(),
            "+7 - - -".into(),
            "7 - - - dirty".into(),
            "7 - -".into(),
            format!("9 7 3 {}", sum.to_uppercase()),
            format!("9 7 3 {}", &sum[..62]),
            format!("9 07 3 {sum}"),
            format!("9 7 3 {sum}  clean"),
            format!("{} - - -", HEAD_NUMBER_MAX + 1),
            format!("9 - 3 {sum}"),
        ] {
            assert_eq!(Head::decode(&bad), None, "{bad:?}");
        }
        assert!(Head::decode(&format!("{HEAD_NUMBER_MAX} - - -")).is_some(), "the largest number is a number");
    }

    // Goal: the platform moves the head only by the head's rules: FPAC's
    // conformance cases 13-19, and every refusal `decide` names. Method:
    // each case against the head it follows.
    #[test]
    fn the_head_moves_only_by_its_rules() {
        // 13: this life's first head is taken
        assert_eq!(decide(None, 5, &head(5, None, false)), Decision::Take);
        // 14: the same head again is the same
        assert_eq!(decide(Some(&head(5, None, false)), 5, &head(5, None, false)), Decision::Same);
        // 15: a later checkpoint of this life is taken
        let first = head(5, Some((5, 1, 1)), false);
        assert_eq!(decide(Some(&head(5, None, false)), 5, &first), Decision::Take);
        assert_eq!(decide(Some(&first), 5, &head(5, Some((5, 2, 2)), false)), Decision::Take);
        // 16: the same checkpoint with another body is refused
        assert_eq!(decide(Some(&first), 5, &head(5, Some((5, 1, 9)), false)), Decision::Refuse("the same checkpoint with another body"));
        // 17: the same checkpoint marked clean (a sleep) is taken
        assert_eq!(decide(Some(&first), 5, &head(5, Some((5, 1, 1)), true)), Decision::Take);
        // 18: a clean head never loses its mark
        assert_eq!(decide(Some(&head(5, Some((5, 1, 1)), true)), 5, &first), Decision::Refuse("a clean head loses its mark"));
        // 19: a head from a life that isn't running is refused
        assert_eq!(decide(None, 6, &head(5, None, false)), Decision::Refuse("only the running life posts a head"));
        // and the rest of its refusals
        assert_eq!(decide(None, 5, &head(5, Some((6, 1, 1)), false)), Decision::Refuse("a checkpoint from a later life"));
        assert_eq!(decide(Some(&head(6, None, false)), 5, &head(5, None, false)), Decision::Refuse("an earlier life's head"));
        assert_eq!(decide(Some(&first), 5, &head(5, None, false)), Decision::Refuse("a life's head loses its checkpoint"));
        assert_eq!(decide(Some(&head(5, None, false)), 5, &head(5, Some((4, 1, 1)), false)), Decision::Refuse("a life's head goes back to another life's checkpoint"));
        assert_eq!(decide(Some(&head(5, Some((5, 2, 2)), false)), 5, &first), Decision::Refuse("a checkpoint older than the head's"));
        // a new life may name any checkpoint, its old head's or none
        assert_eq!(decide(Some(&head(5, Some((5, 2, 2)), true)), 6, &head(6, Some((5, 2, 2)), false)), Decision::Take);
        // a life that shipped nothing sleeps cleanly
        assert_eq!(decide(Some(&head(6, None, false)), 6, &head(6, None, true)), Decision::Take);
    }

    // Goal: the route names only FPAC's objects, and knows each one's life.
    // Method: valid names, and each way a name breaks the grammar.
    #[test]
    fn an_object_name_is_its_grammar() {
        assert_eq!(object_epoch("e1/s1"), Some(1));
        assert_eq!(object_epoch("e123/m4-0.x"), Some(123));
        for bad in ["", "e0/s1", "e01/s1", "e1", "e1/", "e1/S1", "e1/1s", "x1/s1", "e1/s_1", "e1/s1/x", "e123456789012345678901/s1"] {
            assert_eq!(object_epoch(bad), None, "{bad:?}");
        }
        assert_eq!(object_epoch(&format!("e1/s{}", "1".repeat(NAME_BYTES_MAX))), None, "past its length");
    }

    // Goal: a disk's prefix is its computer's own, and nothing else names
    // one. Method: a computer id, and ids that are not.
    #[test]
    fn a_status_is_read_as_its_daemon_writes_it() {
        // a v0.1.0 daemon's line, cut to what the sleep reads and a few beside
        let line = r#"{"epoch":7,"dirty_extents":2,"unshipped_extents":3,"misses":41,"fetch_retries":1,"fenced":false}"#;
        assert_eq!(Status::read(line), Some(Status { unshipped_bytes: 3 * EXTENT_BYTES, misses: 41, refetches: 1 }));
        assert_eq!(Status::read(&format!("{line}\n")), Status::read(line), "as `fpac status` prints it");
        for bad in ["", "fpac status: No such file or directory", r#"{"misses":1,"fetch_retries":0}"#, r#"{"unshipped_extents":-1,"misses":1,"fetch_retries":0}"#, r#"{"unshipped_extents":18446744073709551615,"misses":1,"fetch_retries":0}"#] {
            assert_eq!(Status::read(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_head_that_will_not_open_falls_back_to_the_one_before_or_a_new_disk() {
        let ck = |e, s| Some(CheckpointRef { epoch: e, seq: s, sum: [7; 32] });
        let older = Head { epoch: 3, checkpoint: ck(3, 9), clean: true };
        let newer = Head { epoch: 4, checkpoint: ck(4, 2), clean: false };
        let claim = Head { epoch: 1, checkpoint: None, clean: false };
        use Unopened::*;
        // a disk a life came up on: what shipped is someone's
        assert_eq!(fallback(Some(&newer), Some(&older), Unusable, true), Fallback::To(older), "the head before it");
        assert_eq!(fallback(Some(&newer), None, Unusable, true), Fallback::Nowhere, "a checkpointed head with none before: a failed start, its files kept");
        assert_eq!(fallback(Some(&claim), None, Blank, true), Fallback::Fresh, "a first life's claim, its device blank: a new disk again");
        assert_eq!(fallback(Some(&claim), Some(&Head { epoch: 0, checkpoint: None, clean: false }), Blank, true), Fallback::Fresh, "blank, and no head it had a checkpoint: new again at once");
        assert_eq!(fallback(Some(&claim), None, Unusable, true), Fallback::Nowhere, "a claim whose segments would not open (corrupt, not blank): never a new disk over what shipped");
        assert_eq!(fallback(Some(&newer), None, Blank, true), Fallback::Nowhere, "a checkpointed head is never blank: its files kept");
        assert_eq!(fallback(Some(&claim), None, Unmountable, true), Fallback::Nowhere, "a filesystem that would not mount on a disk that came up: its files kept");
        assert_eq!(fallback(Some(&newer), Some(&older), Unmountable, true), Fallback::Nowhere, "never rolled back past: a mount's error may pass, and the newest head is someone's (Codex on #1552)");
        assert_eq!(fallback(Some(&Head { epoch: 5, checkpoint: None, clean: false }), Some(&older), Unusable, true), Fallback::To(older), "a claim with a head before it: that one");
        assert_eq!(fallback(None, None, Unusable, true), Fallback::Nowhere, "no head: a new disk that would not make, a failed start");
        // a disk no life came up on holds nothing of anyone's: a format
        // cut short (Codex on #1552), or any head of it that will not open,
        // is a new disk again, its open's own checkpoint whatever it names
        assert_eq!(fallback(Some(&claim), None, Unmountable, false), Fallback::Fresh, "a format cut short: a new disk");
        assert_eq!(fallback(Some(&Head { epoch: 2, checkpoint: ck(2, 1), clean: false }), Some(&claim), Unmountable, false), Fallback::Fresh, "its replay's checkpoint, still nothing of anyone's");
        assert_eq!(fallback(Some(&claim), None, Unusable, false), Fallback::Fresh);
        assert_eq!(fallback(Some(&claim), None, Blank, false), Fallback::Fresh);
        assert_eq!(fallback(None, None, Unmountable, false), Fallback::Nowhere, "no head: nothing to start over from");
    }

    #[test]
    fn a_disks_sleep_keeps_its_container_for_its_final_flush_within_the_unsaved_bound() {
        // the bound: the unsaved one, never under its exit wait nor past the cap
        assert_eq!(flush_kept_ms(30 * 60_000), 30 * 60_000, "valid: the deployment's");
        assert_eq!(flush_kept_ms(45_000), DISK_EXIT_WAIT_MS, "a short bound keeps its exit wait (a flush's own time)");
        assert_eq!(flush_kept_ms(0), DISK_EXIT_WAIT_MS);
        assert_eq!(flush_kept_ms(i64::MAX), FLUSH_KEPT_MAX_MS, "past the cap: the cap");
        // each wait: its exit wait, never past what is kept
        let kept = 30 * 60_000;
        assert_eq!(flush_wait_ms(1_000, 1_000, kept), DISK_EXIT_WAIT_MS, "the first, its whole exit wait");
        assert_eq!(flush_wait_ms(1_000, 1_000 + kept - 10_000, kept), 10_000, "near the end, only what is left");
        assert_eq!(flush_wait_ms(1_000, 1_000 + kept, kept), 0, "past it: no wait, it goes");
        assert_eq!(flush_wait_ms(1_000, i64::MAX, kept), 0, "no overflow");
        assert_eq!(flush_wait_ms(i64::MAX, i64::MAX, kept), 0, "saturates, never overflows");
        // the guest outwaits the platform's look and destroy, within the idle cap
        assert!(guest_flush_ms(kept) > kept + STOP_LOOK_MS + DISK_EXIT_WAIT_MS);
        assert!(guest_flush_ms(flush_kept_ms(i64::MAX)) <= IDLE_TIMEOUT_MAX_MS);
    }

    #[test]
    #[should_panic(expected = "a kept bound")]
    fn a_guests_wait_is_of_a_kept_bound_alone() {
        guest_flush_ms(1_000);
    }

    #[test]
    fn on_a_disk_only_the_running_lifes_read_is_heard() {
        assert!(read_is_running_lifes(true, Some(3), Some(3)), "valid: the life that runs");
        assert!(!read_is_running_lifes(true, Some(2), Some(3)), "a read in flight from the start before");
        assert!(!read_is_running_lifes(true, Some(3), None), "none runs");
        assert!(!read_is_running_lifes(true, None, Some(3)), "a read that names no life");
        assert!(!read_is_running_lifes(true, None, None));
        assert!(read_is_running_lifes(false, None, Some(3)), "off a disk, every read");
        assert!(read_is_running_lifes(false, None, None));
    }

    #[test]
    fn a_wipe_lists_its_disk_again_once_any_put_taken_has_landed() {
        assert_eq!(wipe_settle_left(1_000, 1_000), WIPE_SETTLE_MS);
        assert_eq!(wipe_settle_left(1_000, 1_000 + WIPE_SETTLE_MS - 1), 1);
        assert_eq!(wipe_settle_left(1_000, 1_000 + WIPE_SETTLE_MS), 0);
        assert_eq!(wipe_settle_left(1_000, i64::MAX), 0, "long past");
        assert!(wipe_settle_left(i64::MAX, i64::MAX - 1) > 0, "no overflow");
    }

    #[test]
    fn a_disk_lives_under_its_computer() {
        assert_eq!(disk_prefix("computer:17c33cdf88c6d1d8f58b7a0f").as_deref(), Some("disks/17c33cdf88c6d1d8f58b7a0f/"));
        for bad in ["17c33cdf88c6d1d8f58b7a0f", "computer:17C33CDF88C6D1D8F58B7A0F", "computer:17c33", "computer:../17c33cdf88c6d1d8f58b7a"] {
            assert_eq!(disk_prefix(bad), None, "{bad:?}");
        }
    }
}
