//! A computer's paused operator keys (docs/ledger.md, "Shared-key guard";
//! FIN-448), told its owner as notices (`ComputerNotice::KeyPaused`). Each
//! refusal the swap hears comes from its owner's ledger, the authority,
//! with the cap it paused at, the one tap it offers and its deadline; the
//! latest replaces the notice. A cap change the ledger applied ends it.
//! Otherwise it ends at its deadline (`resumes_at`). A call's success is no
//! event, and the computer keeps no copy of any cap: nothing it holds can
//! go stale against the ledger (Alex and Codex on #1117).
//!
//! A refusal heard before a lift can arrive after it, and tell a pause
//! again for a while; its button asks to raise the cap to at least what it
//! offers (`SetKeyCap::at_least`), so pressing it never lowers a cap: the
//! ledger leaves a higher one as it is, and the notice goes.
//!
//! Kept by the Computer DO as text (`text`, `read`), a line a pause, `p
//! <provider> <spent> <cap> <allow> <resumes at>` (its `allow` 0 when there
//! is none): the cell's Wasm carries no decoder of its own for it.
//! Bounded: a line a provider at most.

use fragment_proto::computer::ComputerNotice;

/// A pause told again says nothing new unless its cap changed or it ends
/// at least this much later: each refused call reports one.
pub const NEWS_MS: i64 = 60_000;

/// One key paused, as the ledger said it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyPause {
    pub provider: String,
    /// What the key was charged this last hour, and the cap it was paused at.
    pub spent_micros: i64,
    pub cap_micros: i64,
    /// The cap its owner's one tap allows (`ledger::guard::next_lift`):
    /// none at the most a person may allow.
    pub allow_micros: Option<i64>,
    /// When it resumes as the hour passes, as the ledger said.
    pub resumes_at: i64,
}

impl KeyPause {
    /// Its notice for its owner.
    pub fn notice(&self) -> ComputerNotice {
        ComputerNotice::KeyPaused {
            provider: self.provider.clone(),
            spent_micros: self.spent_micros,
            cap_micros: self.cap_micros,
            allow_micros: self.allow_micros,
            resumes_at: self.resumes_at,
        }
    }

    /// Its line (`p …`), as the swap sends it and the DO keeps it.
    pub fn line(&self) -> String {
        format!("p {} {} {} {} {}", self.provider, self.spent_micros, self.cap_micros, self.allow_micros.unwrap_or(0), self.resumes_at)
    }

    /// A line `line` wrote; none for any other.
    pub fn of_line(line: &str) -> Option<KeyPause> {
        let mut w = line.strip_prefix("p ")?.split_whitespace();
        let provider = w.next()?.to_string();
        let mut n = || w.next()?.parse::<i64>().ok();
        let (spent_micros, cap_micros, allow, resumes_at) = (n()?, n()?, n()?, n()?);
        let valid = spent_micros >= 0 && cap_micros > 0 && allow >= 0 && n().is_none();
        valid.then(|| KeyPause { provider, spent_micros, cap_micros, allow_micros: (allow > 0).then_some(allow), resumes_at })
    }
}

/// What a computer keeps of its keys' pauses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyPauses {
    pauses: Vec<KeyPause>,
}

impl KeyPauses {
    /// What `text` keeps; a line that does not read is none (best effort,
    /// as the guard is).
    pub fn read(text: &str) -> KeyPauses {
        let mut kept = KeyPauses::default();
        for p in text.lines().filter_map(KeyPause::of_line) {
            kept.pauses.retain(|q| q.provider != p.provider);
            kept.pauses.push(p);
        }
        kept
    }

    pub fn text(&self) -> String {
        self.pauses.iter().map(KeyPause::line).collect::<Vec<_>>().join("\n")
    }

    /// The pauses not yet ended by their deadline, at `now_ms`.
    pub fn live(&self, now_ms: i64) -> impl Iterator<Item = &KeyPause> {
        self.pauses.iter().filter(move |p| p.resumes_at > now_ms)
    }

    /// A refusal heard: it replaces the key's notice. `None` when that is
    /// no news of one told (nothing written); else whether its owner's
    /// pages are told (a pause starting, or its cap changed).
    pub fn paused(&mut self, p: KeyPause, now_ms: i64) -> Option<bool> {
        let tell = match self.live(now_ms).find(|q| q.provider == p.provider) {
            Some(h) if h.cap_micros == p.cap_micros && p.resumes_at < h.resumes_at + NEWS_MS => return None,
            Some(h) => h.cap_micros != p.cap_micros,
            None => true,
        };
        self.pauses.retain(|q| q.provider != p.provider && q.resumes_at > now_ms);
        self.pauses.push(p);
        Some(tell)
    }

    /// Its owner's cap change, applied by the ledger: the key's notice
    /// ends. `None` when none was kept (nothing written); else whether one
    /// was told (its owner's pages told).
    pub fn lifted(&mut self, provider: &str, now_ms: i64) -> Option<bool> {
        let held = self.pauses.iter().any(|p| p.provider == provider);
        let told = self.live(now_ms).any(|p| p.provider == provider);
        self.pauses.retain(|p| p.provider != provider && p.resumes_at > now_ms);
        held.then_some(told)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: i64 = 1_790_812_800_000;
    const MIN: i64 = 60_000;
    const USD: i64 = 1_000_000;

    fn pause(cap: i64, resumes_at: i64) -> KeyPause {
        KeyPause { provider: "elevenlabs".into(), spent_micros: cap + 30_000, cap_micros: cap, allow_micros: Some(4 * cap), resumes_at }
    }

    fn told(k: &KeyPauses, now: i64) -> Vec<(&str, i64)> {
        k.live(now).map(|p| (p.provider.as_str(), p.cap_micros)).collect()
    }

    /// Goal (Alex on #1117, review 5819483119): his reordered asks keep
    /// the pause. A asks at T and is delayed; B is allowed at T + 20; a
    /// charge reaches the cap at T + 30; A is refused at T + 40; B's call
    /// completes at T + 50. B's completion is no event of the computer's
    /// pauses (ea7992b11 ended the pause on it: its test failed there).
    /// Method: A's refusal told, its delivery late too.
    #[test]
    fn a_calls_success_ends_no_pause() {
        let mut k = KeyPauses::default();
        assert_eq!(k.paused(pause(5 * USD, T + 60 * MIN), T + 40), Some(true));
        assert_eq!(told(&k, T + 50), [("elevenlabs", 5 * USD)], "B completes: still told, its button there");
        let mut late = KeyPauses::default();
        assert_eq!(late.paused(pause(5 * USD, T + 60 * MIN), T + 5 * MIN), Some(true), "its report late: told as it comes");
    }

    /// Goal (Alex's review 5819600199): a cap change whose notification to
    /// the computer is lost leaves its refusals at the new cap told
    /// (9828f58d3 kept the old cap and dropped them: its test failed
    /// there). Method: a pause at $5; the $80 change commits in the ledger,
    /// its notification lost; a refusal at $80.
    #[test]
    fn a_lost_cap_notification_still_tells_pauses_at_the_new_cap() {
        let mut k = KeyPauses::default();
        k.paused(pause(5 * USD, T + 60 * MIN), T);
        assert_eq!(k.paused(pause(80 * USD, T + 90 * MIN), T + 30 * MIN), Some(true), "the ledger's $80 refusal replaces the notice, told");
        assert_eq!(told(&k, T + 30 * MIN), [("elevenlabs", 80 * USD)]);
    }

    /// Goal (the same review): two cap changes delivered in reverse leave
    /// the ledger's real refusals told (9828f58d3 dropped them). Method:
    /// the $80 change's notification, then the $20 one's; a refusal at $80.
    #[test]
    fn cap_changes_in_any_order_leave_refusals_told() {
        let mut k = KeyPauses::default();
        k.paused(pause(5 * USD, T + 60 * MIN), T);
        assert_eq!(k.lifted("elevenlabs", T + 2), Some(true), "the $80 change: the notice ends");
        assert_eq!(k.lifted("elevenlabs", T + 3), None, "the $20 change, late: nothing held, nothing written");
        assert_eq!(k.paused(pause(80 * USD, T + 90 * MIN), T + 30 * MIN), Some(true), "a refusal at $80: told");
    }

    /// Goal: a refusal heard before a lift, arriving after it, is told
    /// again (the latest replaces), with its old offer: pressing it raises
    /// to at least that, a no-op on the higher cap (`SetKeyCap::at_least`,
    /// ledger::guard's test), and the applied change ends the notice.
    /// Method: a lift, the stale $5 refusal, then its press's change.
    #[test]
    fn a_stale_refusal_after_a_lift_is_told_and_its_press_ends_it() {
        let mut k = KeyPauses::default();
        k.paused(pause(5 * USD, T + 60 * MIN), T);
        assert_eq!(k.lifted("elevenlabs", T + MIN), Some(true));
        assert_eq!(k.paused(pause(5 * USD, T + 60 * MIN), T + MIN + 10), Some(true), "stale, but the latest: told, offering $20");
        assert_eq!(k.live(T + MIN + 10).next().and_then(|p| p.allow_micros), Some(20 * USD));
        assert_eq!(k.lifted("elevenlabs", T + 2 * MIN), Some(true), "its press, applied (at least $20: the cap unchanged), ends it");
        assert!(told(&k, T + 2 * MIN).is_empty());
    }

    /// Goal: a refusal of no news writes nothing, and a pause ends at its
    /// deadline.
    #[test]
    fn no_news_writes_nothing_and_a_deadline_ends_it() {
        let mut k = KeyPauses::default();
        k.paused(pause(5 * USD, T + 60 * MIN), T);
        assert_eq!(k.paused(pause(5 * USD, T + 60 * MIN + 30_000), T + MIN), None, "no news: nothing written");
        assert_eq!(k.paused(pause(5 * USD, T + 62 * MIN), T + 2 * MIN), Some(false), "a later deadline: kept, not told anew");
        let xai = KeyPause { provider: "xai".into(), allow_micros: None, ..pause(5 * USD, T + 10 * MIN) };
        assert_eq!(k.paused(xai, T), Some(true));
        assert_eq!(told(&k, T + 10 * MIN - 1).len(), 2);
        assert_eq!(told(&k, T + 10 * MIN), [("elevenlabs", 5 * USD)], "xai's ended by its deadline");
        assert_eq!(k.lifted("xai", T + 11 * MIN), Some(false), "past its deadline: let go, nothing told");
    }

    /// Goal: what is kept reads back as written, a line that does not read
    /// is none, and it stays bounded.
    #[test]
    fn the_text_reads_back_and_stays_bounded() {
        let mut k = KeyPauses::default();
        k.paused(pause(5 * USD, T + 60 * MIN), T);
        let xai = KeyPause { provider: "xai".into(), allow_micros: None, ..pause(5 * USD, T + 10 * MIN) };
        k.paused(xai.clone(), T);
        assert_eq!(KeyPauses::read(&k.text()), k, "reads back as written: {}", k.text());
        assert_eq!(KeyPause::of_line(&xai.line()), Some(xai.clone()));
        assert_eq!(xai.notice(), ComputerNotice::KeyPaused { provider: "xai".into(), spent_micros: 5_030_000, cap_micros: 5 * USD, allow_micros: None, resumes_at: T + 10 * MIN });
        for bad in ["", "p elevenlabs 1 2", "p elevenlabs 1 0 0 5", "p elevenlabs -1 2 0 5", "p elevenlabs 1 2 0 5 6", "q elevenlabs 1 2 0 5", "c elevenlabs 5"] {
            assert_eq!(KeyPauses::read(bad), KeyPauses::default(), "{bad:?}");
        }
        let mut many = KeyPauses::default();
        for n in 0..500 {
            many.paused(pause(5 * USD + n, T + 60 * MIN), T + n * 1_000);
        }
        assert_eq!(many.pauses.len(), 1, "a line a key");
    }
}
