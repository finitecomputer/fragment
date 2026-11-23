//! The schedule (`Cards`): the newest live shot, failures retried, an editor's asks, the page's report.

use super::heard::Heard;
use super::limits::{AGAIN_EVERY_MS, ATTEMPTS_MAX, BYTES_MAX};
use fragment_proto::PageReport;
use serde::{Deserialize, Serialize};

/// A commit's id, as code.storage names one (a git SHA-1 or SHA-256, hex).
pub fn valid_live(s: &str) -> bool {
    (12..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// How a fragment's failed tries back off: the wait after the first
/// failure, doubling, never past the longest (the deployment's delivery
/// retry settings, `FRAGMENT_DELIVERY_RETRY_S` and `_MAX_S`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pace {
    pub first_ms: i64,
    pub max_ms: i64,
}

impl Pace {
    /// The wait after `failures` failed tries (1 or more).
    pub fn wait_ms(self, failures: u32) -> i64 {
        assert!(failures >= 1, "a wait follows a failure");
        assert!(self.first_ms >= 1 && self.max_ms >= self.first_ms, "a pace waits a positive time, its longest no shorter than its first");
        let doubled = self.first_ms.saturating_mul(1i64 << (failures - 1).min(30));
        doubled.min(self.max_ms)
    }
}

/// A fragment's card and its schedule, kept whole (cell/src/card.rs keeps
/// it as one JSON value). Keys it does not know are ignored: the `flight`
/// a cell before the alarm took its own shots kept is one, and so are
/// `keep_upload` and `asked_at` to the release before them, which reads
/// the rest, an upload as a card (`Card` and `Wanted` refuse keys they do
/// not know: new facts go here, never there).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cards {
    /// The card kept now.
    pub card: Option<Card>,
    /// The newest live with no card yet, and its tries so far.
    pub wanted: Option<Wanted>,
    /// What the page reported to the last try that ended its live's tries
    /// having opened it.
    pub page: Option<PageReport>,
    /// The card is an editor's upload, which shots leave (`upload`): they
    /// still open the page, for its report. Asking for a shot again
    /// (`again`) lets the next shot replace it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub keep_upload: bool,
    /// When a shot was last asked for again (`again`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asked_at: Option<i64>,
    /// The live whose content's change was shot already (`content_moved`):
    /// one such shot a live.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_shot: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Card {
    /// The image's blob (its SHA-256).
    pub blob: String,
    pub size: u64,
    /// The live commit it shows; empty for an upload, which shows none.
    pub live: String,
    /// The try that took it; 0 for an upload, which no try took.
    pub attempt: u32,
    pub at_ms: i64,
}

impl Card {
    /// Whether an editor uploaded it (`Cards::upload`) rather than a shot
    /// took it.
    pub fn uploaded(&self) -> bool {
        self.attempt == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wanted {
    pub live: String,
    /// When it was wanted: it names the card's tries apart from another
    /// time the same live is wanted.
    pub since: i64,
    /// Its tries so far, the one out (if any) among them: a try counts as
    /// failed from when it begins until it lands.
    pub failures: u32,
    /// When it is next tried.
    pub next_at: i64,
}

/// One try at a card: what it shows, and which try it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shot {
    pub live: String,
    pub since: i64,
    pub attempt: u32,
}

/// Input the schedule refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// A live that is no commit id.
    Live,
    /// A shot or an upload whose blob is no SHA-256, or whose size is none
    /// or over `BYTES_MAX`.
    Blob,
    /// A shot asked for again within `AGAIN_EVERY_MS` of the last ask; the
    /// next may be asked in `wait_ms`.
    TooSoon { wait_ms: i64 },
}

/// A shot asked for again (`Cards::again`): of which live, and its tries'
/// `since`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asked {
    pub live: String,
    pub since: i64,
    /// A shot of that live was wanted already (a deploy's, or an ask's):
    /// the ask is that one, and costs no other.
    pub joined: bool,
}

/// What the schedule asks for now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    /// Nothing to shoot now (`due_at` says when to look again).
    Idle,
    /// Take this shot now: it counts as a failed try until it lands.
    Shoot(Shot),
    /// The live's last try never landed (the object restarted under it):
    /// the live is let go, and the card before stays.
    GaveUp { failures: u32 },
}

/// How a try ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// An image, checked (`check_image`) and stored as this blob.
    Image { blob: String, size: u64 },
    /// No image; `retry` when another try may get one.
    Failed { retry: bool },
}

/// What a try's landing did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The shot is the card now.
    Kept,
    /// The card is an editor's upload, which shots leave: the shot ends
    /// its live's tries (what it heard is the page's report) and is no card.
    LeftUpload,
    /// The schedule moved on while the shot was out (a newer live, or the
    /// card's own live again): dropped.
    Stale,
    /// The try failed; the next waits until `at`.
    Retry { at: i64 },
    /// The last try failed: the live gets no card, and the one before stays.
    GaveUp { failures: u32 },
}

impl Cards {
    /// `live` moved to `live`: it is the one to shoot (newest wins), unless
    /// the card shows it already. Answers whether a shot is now wanted
    /// that was not; the same live again changes nothing.
    pub fn want(&mut self, live: &str, now: i64) -> Result<bool, Fault> {
        if !valid_live(live) {
            return Err(Fault::Live);
        }
        if self.card.as_ref().is_some_and(|c| c.live == live) {
            // back to the live the card shows: an older one out lands stale
            self.wanted = None;
            return Ok(false);
        }
        if self.wanted.as_ref().is_some_and(|w| w.live == live) {
            return Ok(false);
        }
        self.wanted = Some(Wanted { live: live.to_string(), since: now, failures: 0, next_at: now });
        Ok(true)
    }

    /// The app's files moved (main) while live stayed at `live`: the card,
    /// a shot of `live` taken before, no longer shows what the app shows
    /// (a brain's pages arriving after its first, empty shot: FIN-368). The
    /// same live is wanted again, at once, its first change after the shot
    /// alone: once a live (`content_shot`), so a busy app's later writes
    /// shoot nothing until its next deploy, and an editor's ask (`again`)
    /// stays for those. Nothing is wanted when a shot is wanted already (it
    /// shows the change), when there is no card or it shows another live
    /// (that live's own shot decides), or when an editor's upload is kept.
    /// Answers whether a shot is now wanted that was not.
    pub fn content_moved(&mut self, live: &str, now: i64) -> Result<bool, Fault> {
        if !valid_live(live) {
            return Err(Fault::Live);
        }
        let shows = self.card.as_ref().is_some_and(|c| c.live == live && !c.uploaded());
        if !shows || self.keep_upload || self.wanted.is_some() || self.content_shot.as_deref() == Some(live) {
            return Ok(false);
        }
        self.wanted = Some(Wanted { live: live.to_string(), since: now, failures: 0, next_at: now });
        self.content_shot = Some(live.to_string());
        Ok(true)
    }

    /// When the schedule next needs a look: the wanted live's next try.
    pub fn due_at(&self) -> Option<i64> {
        self.wanted.as_ref().map(|w| w.next_at)
    }

    /// The live a shot would be of now, without beginning it: the cell asks
    /// whether it is shot at all (`visitor`, the owner's ledger) first.
    pub fn due(&self, now: i64) -> Option<&Wanted> {
        self.wanted.as_ref().filter(|w| w.next_at <= now)
    }

    /// The wanted live gets no card (`Skip`): it is let go, and the card
    /// before stays.
    pub fn skip(&mut self) -> Option<Wanted> {
        self.wanted.take()
    }

    /// What to do now. A shot it answers is counted as a failed try at
    /// once, and the next waits as after a failure: a try lost with its
    /// object is tried again then, and a live whose last try was lost is
    /// let go.
    pub fn next(&mut self, now: i64, pace: Pace) -> Next {
        let Some(w) = self.wanted.as_mut().filter(|w| w.next_at <= now) else { return Next::Idle };
        if w.failures >= ATTEMPTS_MAX {
            let failures = w.failures;
            self.wanted = None;
            return Next::GaveUp { failures };
        }
        w.failures += 1;
        w.next_at = now + pace.wait_ms(w.failures);
        Next::Shoot(Shot { live: w.live.clone(), since: w.since, attempt: w.failures })
    }

    /// Whether `shot` is the try the schedule waits on: its live is still
    /// the newest, and no later try of it began.
    pub fn current(&self, shot: &Shot) -> bool {
        self.wanted.as_ref().is_some_and(|w| w.live == shot.live && w.since == shot.since && w.failures == shot.attempt)
    }

    /// A try landed. Only the current one counts (newest wins); a failure
    /// waits from now, not from when it began.
    pub fn landed(&mut self, shot: &Shot, outcome: Outcome, now: i64, pace: Pace) -> Result<Verdict, Fault> {
        if let Outcome::Image { blob, size } = &outcome {
            if !crate::blob::valid_sha(blob) || *size == 0 || *size > BYTES_MAX as u64 {
                return Err(Fault::Blob);
            }
        }
        if !self.current(shot) {
            return Ok(Verdict::Stale);
        }
        let w = self.wanted.as_mut().expect("a current shot has its live wanted");
        Ok(match outcome {
            Outcome::Image { .. } if self.keep_upload => {
                assert!(self.card.as_ref().is_some_and(Card::uploaded), "an upload kept is the card");
                self.wanted = None;
                Verdict::LeftUpload
            }
            Outcome::Image { blob, size } => {
                self.card = Some(Card { blob, size, live: shot.live.clone(), attempt: shot.attempt, at_ms: now });
                self.wanted = None;
                Verdict::Kept
            }
            Outcome::Failed { retry } if !retry || w.failures >= ATTEMPTS_MAX => {
                let failures = w.failures;
                self.wanted = None;
                Verdict::GaveUp { failures }
            }
            Outcome::Failed { .. } => {
                w.next_at = now + pace.wait_ms(w.failures);
                Verdict::Retry { at: w.next_at }
            }
        })
    }

    /// What a try that opened the page heard, once it has landed: the
    /// page's report when the try ended its live's tries (`Kept`,
    /// `LeftUpload`, `GaveUp`); a retry's and a stale try's are dropped.
    /// Answers the report it made.
    pub fn report(&mut self, verdict: &Verdict, live: &str, heard: Heard, now: i64) -> Option<&PageReport> {
        assert!(valid_live(live), "a shot names a commit");
        if !matches!(verdict, Verdict::Kept | Verdict::LeftUpload | Verdict::GaveUp { .. }) {
            return None;
        }
        self.page = Some(PageReport { live: live.to_string(), at: now, errors: heard.errors, dropped: heard.dropped });
        self.page.as_ref()
    }

    /// An editor asks for a shot of `live` (live as it is now) again: a
    /// card that looks wrong (shot as the page loaded, or of data since
    /// changed), or an upload they want a shot in place of. The next shot
    /// replaces the card, an upload too. A shot of `live` already wanted
    /// is the one (`joined`); else one is wanted now, from its first try,
    /// at most one ask each `AGAIN_EVERY_MS` (`TooSoon`, changing nothing).
    /// The cell asks first whether the fragment is shot at all
    /// (`visitor`, the owner's ledger).
    pub fn again(&mut self, live: &str, now: i64) -> Result<Asked, Fault> {
        if !valid_live(live) {
            return Err(Fault::Live);
        }
        if let Some(w) = self.wanted.as_ref().filter(|w| w.live == live) {
            let asked = Asked { live: w.live.clone(), since: w.since, joined: true };
            self.keep_upload = false;
            return Ok(asked);
        }
        // an ask a clock that went back puts in the future refuses none
        if let Some(wait_ms) = self.asked_at.map(|at| at + AGAIN_EVERY_MS - now).filter(|w| (1..=AGAIN_EVERY_MS).contains(w)) {
            return Err(Fault::TooSoon { wait_ms });
        }
        self.wanted = Some(Wanted { live: live.to_string(), since: now, failures: 0, next_at: now });
        self.keep_upload = false;
        self.asked_at = Some(now);
        Ok(Asked { live: live.to_string(), since: now, joined: false })
    }

    /// An editor's image is the card: checked (`check_image`) and stored as
    /// this blob. Shots leave it (`LeftUpload`), a deploy's and one out now
    /// alike, until a shot is asked for again (`again`) or another image is
    /// uploaded. Answers whether it changed anything: the same image again
    /// does not.
    pub fn upload(&mut self, blob: &str, size: u64, now: i64) -> Result<bool, Fault> {
        if !crate::blob::valid_sha(blob) || size == 0 || size > BYTES_MAX as u64 {
            return Err(Fault::Blob);
        }
        if self.keep_upload && self.card.as_ref().is_some_and(|c| c.blob == blob) {
            return Ok(false);
        }
        self.card = Some(Card { blob: blob.to_string(), size, live: String::new(), attempt: 0, at_ms: now });
        self.keep_upload = true;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::meter::meter_ref;
    use crate::card::testing::{blob, image, shoot, A, B, C, PACE, T0};

    /// Goal: a deploy is shot once, and its shot is the card. Method: want
    /// a live, begin its shot, land the image.
    #[test]
    fn a_deploy_is_shot_and_becomes_the_card() {
        let mut c = Cards::default();
        assert_eq!(c.next(T0, PACE), Next::Idle, "nothing deployed, nothing shot");
        assert_eq!(c.want(A, T0), Ok(true));
        assert_eq!(c.due_at(), Some(T0));
        let s = shoot(&mut c, T0);
        assert_eq!(s, Shot { live: A.into(), since: T0, attempt: 1 });
        assert_eq!(c.next(T0 + 1, PACE), Next::Idle, "a try out is not begun again before its wait");
        assert_eq!(c.landed(&s, image(1), T0 + 2_000, PACE), Ok(Verdict::Kept));
        assert_eq!(c.card, Some(Card { blob: blob(1), size: 40_000, live: A.into(), attempt: 1, at_ms: T0 + 2_000 }));
        assert_eq!((c.wanted.clone(), c.next(T0 + 3_000, PACE), c.due_at()), (None, Next::Idle, None));
    }

    /// Goal: the newest live wins. Method: two deploys before any shot
    /// (only the second is shot), then a deploy while a shot is out (the
    /// shot out lands stale, and the newest is shot next, from its first
    /// try).
    #[test]
    fn the_newest_live_wins() {
        let mut c = Cards::default();
        c.want(A, T0).unwrap();
        c.want(B, T0 + 1).unwrap();
        let s = shoot(&mut c, T0 + 2);
        assert_eq!(s.live, B, "a live replaced before its shot is never shot");
        c.want(C, T0 + 3).unwrap();
        assert_eq!(c.landed(&s, image(2), T0 + 5, PACE), Ok(Verdict::Stale), "a shot of a live no longer the newest is dropped");
        assert_eq!(c.card, None);
        let s = shoot(&mut c, T0 + 6);
        assert_eq!((s.live.as_str(), s.attempt), (C, 1));
        assert_eq!(c.landed(&s, image(3), T0 + 7, PACE), Ok(Verdict::Kept));
        assert_eq!(c.card.as_ref().map(|k| k.live.as_str()), Some(C));
        // a failure of a stale shot costs the newest no try
        c.want(A, T0 + 8).unwrap();
        let old = shoot(&mut c, T0 + 9);
        c.want(B, T0 + 10).unwrap();
        assert_eq!(c.landed(&old, Outcome::Failed { retry: true }, T0 + 11, PACE), Ok(Verdict::Stale));
        assert_eq!(c.wanted.as_ref().map(|w| (w.live.as_str(), w.failures, w.next_at)), Some((B, 0, T0 + 10)));
    }

    /// Goal: a second deploy replaces the card; the card before stays until
    /// the new one is kept. Method: two lives shot in turn.
    #[test]
    fn a_second_deploy_replaces_the_card() {
        let mut c = Cards::default();
        c.want(A, T0).unwrap();
        let s = shoot(&mut c, T0);
        c.landed(&s, image(1), T0 + 1, PACE).unwrap();
        c.want(B, T0 + 2).unwrap();
        let s = shoot(&mut c, T0 + 2);
        assert_eq!(c.card.as_ref().map(|k| k.blob.clone()), Some(blob(1)), "the card before serves while the next is shot");
        c.landed(&s, image(2), T0 + 3, PACE).unwrap();
        assert_eq!(c.card.as_ref().map(|k| (k.blob.clone(), k.live.as_str())), Some((blob(2), B)));
        // back to the live the card shows (a forced push of live): nothing to shoot
        c.want(C, T0 + 4).unwrap();
        let out = shoot(&mut c, T0 + 4);
        assert_eq!(c.want(B, T0 + 5), Ok(false));
        assert_eq!(c.wanted, None);
        assert_eq!(c.landed(&out, image(3), T0 + 6, PACE), Ok(Verdict::Stale), "the shot of C, out meanwhile, lands stale");
        assert_eq!(c.card.as_ref().map(|k| k.live.as_str()), Some(B));
    }

    /// Goal: a failure is tried again with a doubling wait, counted from
    /// when it landed, and after the last try the live is let go quietly,
    /// the card before kept. Method: fail every try, each landing a second
    /// after it began; then a failure that says not to retry.
    #[test]
    fn failures_retry_with_backoff_then_give_up() {
        let mut c = Cards::default();
        c.want(A, T0).unwrap();
        let s = shoot(&mut c, T0);
        c.landed(&s, image(1), T0, PACE).unwrap();
        c.want(B, T0 + 1).unwrap();
        let mut now = T0 + 1;
        let mut waits = vec![];
        for attempt in 1..=ATTEMPTS_MAX {
            let s = shoot(&mut c, now);
            assert_eq!(s.attempt, attempt);
            let landed_at = now + 1_000;
            match c.landed(&s, Outcome::Failed { retry: true }, landed_at, PACE).unwrap() {
                Verdict::Retry { at } => {
                    assert_eq!((c.next(at - 1, PACE), c.due_at()), (Next::Idle, Some(at)), "a retry waits its turn");
                    waits.push(at - landed_at);
                    now = at;
                }
                Verdict::GaveUp { failures } => assert_eq!((failures, attempt), (ATTEMPTS_MAX, ATTEMPTS_MAX)),
                v => panic!("{v:?}"),
            }
        }
        assert_eq!(waits, [10_000, 20_000, 40_000, 80_000]);
        assert_eq!((c.wanted.clone(), c.next(now + PACE.max_ms, PACE)), (None, Next::Idle), "given up, nothing more is tried");
        assert_eq!(c.card.as_ref().map(|k| k.live.as_str()), Some(A), "the card before stays");
        // a failure that no retry can mend gives up at once
        c.want(C, now).unwrap();
        let s = shoot(&mut c, now);
        assert_eq!(c.landed(&s, Outcome::Failed { retry: false }, now, PACE), Ok(Verdict::GaveUp { failures: 1 }));
        // the waits double to the longest, and stop there
        let pace = Pace { first_ms: 1_000, max_ms: 5_000 };
        assert_eq!((1..=6).map(|f| pace.wait_ms(f)).collect::<Vec<_>>(), [1_000, 2_000, 4_000, 5_000, 5_000, 5_000]);
        assert_eq!(Pace { first_ms: 1_000, max_ms: 1_000 }.wait_ms(u32::MAX), 1_000, "a huge count stays in bounds");
    }

    /// Goal: replays change nothing: the same live again, the same landing
    /// twice, a landing of a try a later one replaced. Method: each against
    /// a copy.
    #[test]
    fn replays_change_nothing() {
        let mut c = Cards::default();
        c.want(A, T0).unwrap();
        let before = c.clone();
        assert_eq!(c.want(A, T0 + 5), Ok(false), "the same live wanted again");
        assert_eq!(c, before);
        let s = shoot(&mut c, T0);
        c.landed(&s, image(1), T0 + 1, PACE).unwrap();
        let kept = c.clone();
        assert_eq!(c.landed(&s, image(2), T0 + 2, PACE), Ok(Verdict::Stale), "a landing twice");
        assert_eq!(c.want(A, T0 + 3), Ok(false), "the live the card shows");
        assert_eq!(c, kept);
        // a try a later one replaced, and another wanting of the same live, are not current
        c.want(B, T0 + 4).unwrap();
        let first = shoot(&mut c, T0 + 4);
        let second = shoot(&mut c, T0 + 4 + PACE.first_ms);
        let other = Shot { since: second.since + 1, ..second.clone() };
        let before = c.clone();
        assert_eq!(c.landed(&first, image(3), T0 + 5 + PACE.first_ms, PACE), Ok(Verdict::Stale));
        assert_eq!(c.landed(&other, image(3), T0 + 5 + PACE.first_ms, PACE), Ok(Verdict::Stale));
        assert_eq!(c, before);
        assert_eq!(c.landed(&second, image(4), T0 + 6 + PACE.first_ms, PACE), Ok(Verdict::Kept));
    }

    /// Goal: what the schedule refuses, it refuses typed and unchanged.
    /// Method: lives and blobs that are none.
    #[test]
    fn invalid_input_is_refused() {
        let mut c = Cards::default();
        for bad in ["", "abc", "ABCDEFABCDEFABCDEF", "g".repeat(40).as_str(), "a".repeat(65).as_str()] {
            assert_eq!(c.want(bad, T0), Err(Fault::Live), "{bad:?}");
        }
        assert_eq!(c, Cards::default());
        c.want(A, T0).unwrap();
        let s = shoot(&mut c, T0);
        let before = c.clone();
        for bad in [Outcome::Image { blob: "x".into(), size: 1 }, Outcome::Image { blob: blob(1), size: 0 }, Outcome::Image { blob: blob(1), size: BYTES_MAX as u64 + 1 }] {
            assert_eq!(c.landed(&s, bad, T0, PACE), Err(Fault::Blob));
        }
        assert_eq!(c, before, "a refused landing changes nothing");
        let decoded: Result<Cards, _> = serde_json::from_value(serde_json::json!({ "card": null, "wanted": { "live": A, "since": T0, "failures": 0, "next_at": T0, "extra": 1 } }));
        assert!(decoded.is_err(), "the kept state refuses fields it does not know");
    }

    /// Goal: the schedule survives a restart at any point: it is kept as
    /// JSON and read back the same, a try lost in the restart counts as
    /// failed and is tried again after its wait, and a live whose last try
    /// was lost is let go. Method: round-trip mid-shot; then lose every
    /// try. And the state a cell before this one kept (with its `flight`)
    /// reads back as its card and wanted live.
    #[test]
    fn a_restart_keeps_the_schedule_and_a_lost_try_counts() {
        let mut c = Cards::default();
        c.want(A, T0).unwrap();
        let s = shoot(&mut c, T0);
        let mut back: Cards = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
        assert_eq!(back.next(T0 + PACE.first_ms - 1, PACE), Next::Idle, "the lost try waits as a failure does");
        let again = shoot(&mut back, T0 + PACE.first_ms);
        assert_eq!((again.live.as_str(), again.since, again.attempt), (A, s.since, 2));
        assert_eq!(back.landed(&s, image(1), T0 + PACE.first_ms + 1, PACE), Ok(Verdict::Stale), "the lost try, should it land after all");
        // every try lost: the look after the last lets the live go
        let mut now = T0 + PACE.first_ms;
        for _ in 3..=ATTEMPTS_MAX {
            now = back.due_at().unwrap();
            shoot(&mut back, now);
        }
        assert_eq!(back.wanted.as_ref().map(|w| w.failures), Some(ATTEMPTS_MAX));
        assert_eq!(back.next(back.due_at().unwrap(), PACE), Next::GaveUp { failures: ATTEMPTS_MAX });
        assert_eq!((back.wanted.clone(), back.next(now + PACE.max_ms, PACE)), (None, Next::Idle));
        // what a cell before this one kept
        let old = serde_json::json!({
            "card": { "blob": blob(1), "size": 40_000, "live": A, "attempt": 1, "at_ms": T0 },
            "wanted": { "live": B, "since": T0, "failures": 1, "next_at": T0 },
            "flight": { "live": B, "since": T0, "attempt": 2, "until": T0 + 180_000 },
        });
        let mut old: Cards = serde_json::from_value(old).unwrap();
        assert_eq!(old.card.as_ref().map(|k| k.live.as_str()), Some(A));
        assert_eq!(shoot(&mut old, T0).attempt, 2, "its shot out was lost with the cut: tried again");
    }

    /// Goal: a live let go is let go: skipped, it is never shot, and the
    /// card before stays.
    #[test]
    fn a_skipped_live_is_never_shot() {
        let mut c = Cards::default();
        c.want(A, T0).unwrap();
        assert_eq!(c.due(T0).map(|w| w.live.as_str()), Some(A));
        assert_eq!(c.skip().map(|w| w.live), Some(A.to_string()));
        assert_eq!((c.next(T0, PACE), c.due(T0)), (Next::Idle, None));
    }

    /// A deploy of `live` shot and kept at `now`.
    fn shot_card(c: &mut Cards, live: &str, n: u8, now: i64) {
        c.want(live, now).unwrap();
        let s = shoot(c, now);
        assert_eq!(c.landed(&s, image(n), now, PACE), Ok(Verdict::Kept));
    }

    /// Goal: an editor's ask shoots the live the card shows again, as a
    /// shot of its own (its tries, its meter reference), which replaces
    /// the card; at most one ask a minute, a refused one changing nothing.
    /// Method: a deploy's card, an ask, its shot; asks within the minute
    /// and after it.
    #[test]
    fn asking_again_shoots_the_live_again() {
        let mut c = Cards::default();
        shot_card(&mut c, A, 1, T0);
        assert_eq!(c.want(A, T0 + 1), Ok(false), "a deploy of the live the card shows wants nothing");
        let t = T0 + 5_000;
        assert_eq!(c.again(A, t), Ok(Asked { live: A.into(), since: t, joined: false }));
        assert_eq!((c.due_at(), c.asked_at), (Some(t), Some(t)));
        let s = shoot(&mut c, t);
        assert_eq!(s, Shot { live: A.into(), since: t, attempt: 1 });
        assert_ne!(meter_ref("k", &s), meter_ref("k", &Shot { live: A.into(), since: T0, attempt: 1 }), "billed as a shot of its own");
        assert_eq!(c.landed(&s, image(2), t + 2_000, PACE), Ok(Verdict::Kept));
        assert_eq!(c.card, Some(Card { blob: blob(2), size: 40_000, live: A.into(), attempt: 1, at_ms: t + 2_000 }));
        // within the minute: refused, saying how long is left, nothing changed
        let before = c.clone();
        assert_eq!(c.again(A, t + 2_000), Err(Fault::TooSoon { wait_ms: AGAIN_EVERY_MS - 2_000 }));
        assert_eq!(c.again(A, t + AGAIN_EVERY_MS - 1), Err(Fault::TooSoon { wait_ms: 1 }));
        assert_eq!(c, before);
        // a minute on, again; and a deploy is never held to the minute
        assert_eq!(c.again(A, t + AGAIN_EVERY_MS).map(|a| a.joined), Ok(false));
        assert_eq!(c.want(B, t + AGAIN_EVERY_MS + 1), Ok(true), "a deploy meanwhile is the newest: it is shot");
        assert_eq!(c.wanted.as_ref().map(|w| w.live.as_str()), Some(B));
        // a deploy's card, then an ask at once: not held to the minute either
        let mut d = Cards::default();
        shot_card(&mut d, A, 1, T0);
        assert_eq!(d.again(A, T0 + 1).map(|a| a.joined), Ok(false));
    }

    /// Goal: an ask while a shot of the live is wanted (a deploy's, or an
    /// ask's, waiting or out) is that shot: no other is begun, the minute
    /// is not counted, and asking again changes nothing (a replay).
    /// Method: ask while a deploy's shot waits, while it is out, and twice.
    #[test]
    fn asking_again_joins_a_shot_wanted() {
        let mut c = Cards::default();
        c.want(A, T0).unwrap();
        let before = c.clone();
        assert_eq!(c.again(A, T0 + 1), Ok(Asked { live: A.into(), since: T0, joined: true }));
        assert_eq!(c.again(A, T0 + 2), Ok(Asked { live: A.into(), since: T0, joined: true }));
        assert_eq!(c, before, "joining changes nothing, nor counts toward the minute");
        let s = shoot(&mut c, T0 + 3);
        let out = c.clone();
        assert_eq!(c.again(A, T0 + 4).map(|a| (a.since, a.joined)), Ok((T0, true)), "the shot out is the one");
        assert_eq!(c, out);
        assert_eq!(c.landed(&s, image(1), T0 + 5, PACE), Ok(Verdict::Kept), "it lands as the deploy's");
        // an ask's shot, asked for again before it lands: the same
        let t = T0 + 10;
        let first = c.again(A, t).unwrap();
        assert_eq!(c.again(A, t + 1), Ok(Asked { joined: true, ..first }));
    }

    /// Goal: an upload is the card at once, and stays: a deploy is still
    /// shot (its report is the page's) and its shot leaves it, as does a
    /// shot out when it came; another upload replaces it; asking for a
    /// shot again lets the next shot replace it, the one asked for or a
    /// deploy's out then. Method: each in turn.
    #[test]
    fn an_upload_is_the_card_until_a_shot_is_asked_for_again() {
        let mut c = Cards::default();
        assert_eq!(c.upload(&blob(9), 30_000, T0), Ok(true), "a fragment with no card yet takes one");
        assert_eq!(c.card, Some(Card { blob: blob(9), size: 30_000, live: String::new(), attempt: 0, at_ms: T0 }));
        assert!(c.keep_upload && c.card.as_ref().is_some_and(Card::uploaded));
        // a deploy is shot, for its report, and leaves the upload
        assert_eq!(c.want(A, T0 + 1), Ok(true));
        let s = shoot(&mut c, T0 + 1);
        let v = c.landed(&s, image(1), T0 + 2, PACE).unwrap();
        assert_eq!(v, Verdict::LeftUpload);
        assert_eq!(c.report(&v, A, Heard::default(), T0 + 2).map(|p| p.live.as_str()), Some(A), "its report is the page's");
        assert_eq!((c.card.as_ref().map(|k| k.blob.clone()), c.wanted.clone()), (Some(blob(9)), None));
        // an upload while a deploy's shot is out: the shot lands and leaves it
        c.want(B, T0 + 3).unwrap();
        let s = shoot(&mut c, T0 + 3);
        assert_eq!(c.upload(&blob(8), 20_000, T0 + 4), Ok(true));
        assert_eq!(c.landed(&s, image(2), T0 + 5, PACE), Ok(Verdict::LeftUpload));
        assert_eq!(c.card.as_ref().map(|k| (k.blob.clone(), k.attempt)), Some((blob(8), 0)));
        // asked for again: its shot replaces the upload
        let asked = c.again(B, T0 + 6).unwrap();
        assert!(!asked.joined && !c.keep_upload);
        assert_eq!(c.card.as_ref().map(|k| k.blob.clone()), Some(blob(8)), "the upload serves until the shot lands");
        let s = shoot(&mut c, T0 + 6);
        assert_eq!(c.landed(&s, image(3), T0 + 7, PACE), Ok(Verdict::Kept));
        assert_eq!(c.card, Some(Card { blob: blob(3), size: 40_000, live: B.into(), attempt: 1, at_ms: T0 + 7 }));
        // and the next deploy's replaces that, as before any upload
        shot_card(&mut c, C, 4, T0 + 8);
        assert_eq!(c.card.as_ref().map(|k| k.live.as_str()), Some(C));
        // an ask while a deploy's shot is out over an upload joins it: that shot replaces it
        c.upload(&blob(7), 10_000, T0 + 9).unwrap();
        c.want(A, T0 + 10).unwrap();
        let s = shoot(&mut c, T0 + 10);
        assert_eq!(c.again(A, T0 + 11).map(|a| a.joined), Ok(true));
        assert_eq!(c.landed(&s, image(5), T0 + 12, PACE), Ok(Verdict::Kept));
        assert_eq!(c.card.as_ref().map(|k| (k.blob.clone(), k.live.as_str())), Some((blob(5), A)));
        // an ask whose shot is given up leaves the upload, which the next deploy's shot replaces
        c.upload(&blob(6), 10_000, T0 + 13).unwrap();
        let t = T0 + 20 + AGAIN_EVERY_MS;
        c.again(A, t).unwrap();
        let s = shoot(&mut c, t);
        assert_eq!(c.landed(&s, Outcome::Failed { retry: false }, t, PACE), Ok(Verdict::GaveUp { failures: 1 }));
        assert_eq!(c.card.as_ref().map(|k| k.blob.clone()), Some(blob(6)));
        shot_card(&mut c, B, 2, t + 1);
        assert_eq!(c.card.as_ref().map(|k| k.live.as_str()), Some(B));
    }

    /// Goal: what an ask or an upload refuses, it refuses typed and
    /// unchanged, and the same upload again changes nothing (a replay).
    /// Method: lives and blobs that are none; an ask the clock puts in the
    /// future; an upload twice.
    #[test]
    fn asks_and_uploads_refuse_what_is_none() {
        let mut c = Cards::default();
        shot_card(&mut c, A, 1, T0);
        let before = c.clone();
        for bad in ["", "abc", "g".repeat(40).as_str()] {
            assert_eq!(c.again(bad, T0), Err(Fault::Live), "{bad:?}");
        }
        for (b, size) in [("x".to_string(), 1), (blob(2), 0), (blob(2), BYTES_MAX as u64 + 1)] {
            assert_eq!(c.upload(&b, size, T0), Err(Fault::Blob), "{b} {size}");
        }
        assert_eq!(c, before, "a refusal changes nothing");
        assert_eq!(c.upload(&blob(2), BYTES_MAX as u64, T0 + 1), Ok(true), "exactly the cap is a card");
        let kept = c.clone();
        assert_eq!(c.upload(&blob(2), BYTES_MAX as u64, T0 + 2), Ok(false), "the same image again");
        assert_eq!(c, kept);
        // an ask the clock puts in the future (it went back) refuses none
        c.asked_at = Some(T0 + 10 * AGAIN_EVERY_MS);
        assert_eq!(c.again(A, T0 + 3).map(|a| a.joined), Ok(false));
    }

    /// Goal: an upload and an ask survive a restart, and the state reads
    /// both ways across the release that added them: a state from before
    /// reads as no upload kept and no ask; this one's reads in the release
    /// before (its `Card` and `Wanted` refuse keys they do not know), its
    /// upload as a card. Method: round-trips through JSON, and through the
    /// shape the release before kept.
    #[test]
    fn an_upload_and_an_ask_survive_a_restart_and_a_rollback() {
        /// The schedule as the release before this one read it.
        #[derive(Deserialize)]
        struct Before {
            card: Option<Card>,
            wanted: Option<Wanted>,
            #[allow(dead_code)]
            page: Option<PageReport>,
        }
        let mut c = Cards::default();
        c.upload(&blob(9), 30_000, T0).unwrap();
        c.want(A, T0 + 1).unwrap();
        let s = shoot(&mut c, T0 + 1);
        let text = serde_json::to_string(&c).unwrap();
        let mut back: Cards = serde_json::from_str(&text).unwrap();
        assert_eq!(back, c);
        assert_eq!(back.landed(&s, image(1), T0 + 2, PACE), Ok(Verdict::LeftUpload), "the upload is kept after the restart");
        let before: Before = serde_json::from_str(&text).unwrap();
        assert_eq!((before.card.map(|k| (k.blob, k.attempt)), before.wanted.map(|w| w.live)), (Some((blob(9), 0)), Some(A.to_string())));
        let t = T0 + 5;
        back.again(A, t).unwrap();
        let text = serde_json::to_string(&back).unwrap();
        let mut again: Cards = serde_json::from_str(&text).unwrap();
        assert_eq!(again, back);
        assert_eq!(again.again(A, t + 1).map(|a| a.joined), Ok(true), "the ask's shot is still the one");
        assert!(serde_json::from_str::<Before>(&text).is_ok());
        // a state from before reads as no upload kept and no ask, and one with neither writes as it did
        let old: Cards = serde_json::from_value(serde_json::json!({ "card": { "blob": blob(1), "size": 40_000, "live": A, "attempt": 1, "at_ms": T0 }, "wanted": null, "page": null })).unwrap();
        assert_eq!((old.keep_upload, old.asked_at), (false, None));
        assert_eq!(serde_json::to_value(&old).unwrap().as_object().map(|o| o.len()), Some(3));
    }

    /// Goal: an app's content changing after its live's shot (main moved,
    /// live stayed: a brain's pages after its empty first shot) is shot
    /// once more, of the same live, and then not again until the next
    /// deploy: a busy app is never shot over and over (FIN-368). Valid: the
    /// first change after a deploy's card; a new live's own first change.
    /// Invalid: a live that is no commit. Nothing is wanted for a second
    /// change, a change while a shot is wanted (it shows the change), a
    /// card of another live, no card, or an editor's upload kept. Replay:
    /// the same change again wants nothing. Restart: kept, and read by the
    /// release before as it was.
    #[test]
    fn content_after_a_shot_is_shot_once_a_live() {
        let mut c = Cards::default();
        assert_eq!(c.content_moved(A, T0), Ok(false), "no card: nothing to bring up to date");
        shot_card(&mut c, A, 1, T0);
        let t = T0 + 60_000;
        assert_eq!(c.content_moved(A, t), Ok(true));
        assert_eq!((c.due_at(), c.content_shot.as_deref()), (Some(t), Some(A)));
        assert_eq!(c.content_moved(A, t + 1), Ok(false), "a replay while it is wanted");
        let s = shoot(&mut c, t);
        assert_eq!(s, Shot { live: A.into(), since: t, attempt: 1 });
        assert_eq!(c.landed(&s, image(2), t + 2_000, PACE), Ok(Verdict::Kept));
        assert_eq!(c.card.as_ref().map(|k| (k.blob.clone(), k.live.as_str())), Some((blob(2), A)));
        // a busy app: its later changes shoot nothing until its next deploy
        for later in [t + 3_000, t + 3_600_000, t + 86_400_000] {
            assert_eq!(c.content_moved(A, later), Ok(false));
        }
        assert_eq!(c.wanted, None);
        // an editor's ask still shoots it (its own minute)
        assert_eq!(c.again(A, t + 86_400_000).map(|a| a.joined), Ok(false));
        let s = shoot(&mut c, t + 86_400_000);
        assert_eq!(c.landed(&s, image(3), t + 86_402_000, PACE), Ok(Verdict::Kept));
        // a deploy: its shot, then its own first change once more
        let u = t + 90_000_000;
        assert_eq!(c.want(B, u), Ok(true));
        assert_eq!(c.content_moved(B, u + 1), Ok(false), "a deploy's shot wanted already shows the change");
        let s = shoot(&mut c, u);
        assert_eq!(c.landed(&s, image(4), u + 2_000, PACE), Ok(Verdict::Kept));
        assert_eq!(c.content_moved(A, u + 3_000), Ok(false), "the card shows another live");
        assert_eq!(c.content_moved(B, u + 3_000), Ok(true));
        // an editor's upload kept: changes leave it
        let mut d = Cards::default();
        shot_card(&mut d, A, 1, T0);
        d.upload(&blob(9), 30_000, T0 + 1).unwrap();
        assert_eq!((d.content_moved(A, T0 + 2), d.wanted.clone()), (Ok(false), None));
        // invalid
        assert_eq!(Cards::default().content_moved("main", T0), Err(Fault::Live));
        // restart: kept; the release before reads the rest as it was
        let text = serde_json::to_string(&c).unwrap();
        let back: Cards = serde_json::from_str(&text).unwrap();
        assert_eq!(back, c);
        let before: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(before["content_shot"], B);
        #[derive(Deserialize)]
        struct Before {
            #[allow(dead_code)]
            card: Option<Card>,
            wanted: Option<Wanted>,
        }
        let read: Before = serde_json::from_str(&text).unwrap();
        assert_eq!(read.wanted.map(|w| w.live), Some(B.to_string()));
        // a state from before writes as it did
        let old: Cards = serde_json::from_value(serde_json::json!({ "card": null, "wanted": null, "page": null })).unwrap();
        assert_eq!((old.content_shot, serde_json::to_value(Cards::default()).unwrap().as_object().map(|o| o.len())), (None, Some(3)));
    }
}
