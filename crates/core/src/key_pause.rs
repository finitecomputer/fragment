//! A computer's paused operator keys (docs/ledger.md, "Shared-key guard";
//! FIN-448): what the swap heard its owner's ledger refuse, told its owner
//! as a notice (`ComputerNotice::KeyPaused`) until the pause has ended, and
//! only on evidence that it has: its deadline passing, its owner changing
//! the key's cap, or a call on the key that a ledger answer heard after the
//! pause let through. A call let through by an answer heard before it (one
//! in flight as the pause began, or another isolate's allowance still
//! cached) proves nothing, and nor does a report that arrives late: each
//! carries the time its answer was heard, and the rules compare those.
//!
//! Kept by the Computer DO as text (`text`, `read`), a line each: `p
//! <provider> <spent> <cap> <allow> <resumes at> <paused at>` a pause (its
//! `allow` 0 when there is none), `l <provider> <at>` its owner's last
//! change of that key's cap. The cell's Wasm carries no decoder of its own
//! for it. Bounded: a line of each kind a provider at most, and a change
//! older than `LIFT_KEPT_MS` is let go (a pause heard before it has ended
//! by its deadline).

use fragment_proto::computer::ComputerNotice;

/// How long an owner's change of a key's cap is kept to refuse pauses
/// heard before it: a pause lasts at most the guard's hour and slot, so
/// one heard before this is past its deadline.
pub const LIFT_KEPT_MS: i64 = 2 * 3_600_000;
/// A pause told again says nothing new unless its cap changed or it ends
/// at least this much later: each refused call reports it.
pub const NEWS_MS: i64 = 60_000;

/// One key paused, as the swap heard it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyPause {
    pub provider: String,
    /// What the key was charged this last hour, and its cap.
    pub spent_micros: i64,
    pub cap_micros: i64,
    /// The cap its owner's one tap allows (`ledger::guard::next_lift`):
    /// none at the most a person may allow.
    pub allow_micros: Option<i64>,
    /// When it resumes as the hour passes, as the ledger said.
    pub resumes_at: i64,
    /// When the refusal was heard from the ledger (the swap's gate).
    pub paused_at: i64,
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
        format!("p {} {} {} {} {} {}", self.provider, self.spent_micros, self.cap_micros, self.allow_micros.unwrap_or(0), self.resumes_at, self.paused_at)
    }

    /// A line `line` wrote; none for any other.
    pub fn of_line(line: &str) -> Option<KeyPause> {
        let mut w = line.strip_prefix("p ")?.split_whitespace();
        let provider = w.next()?.to_string();
        let mut n = || w.next()?.parse::<i64>().ok();
        let (spent_micros, cap_micros, allow, resumes_at, paused_at) = (n()?, n()?, n()?, n()?, n()?);
        let valid = spent_micros >= 0 && cap_micros > 0 && allow >= 0 && n().is_none();
        valid.then(|| KeyPause { provider, spent_micros, cap_micros, allow_micros: (allow > 0).then_some(allow), resumes_at, paused_at })
    }
}

/// What a computer keeps of its keys' pauses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyPauses {
    pauses: Vec<KeyPause>,
    /// `(provider, at)`: its owner's last change of the key's cap.
    lifted: Vec<(String, i64)>,
}

impl KeyPauses {
    /// What `text` keeps; a line that does not read is none (best effort,
    /// as the guard is).
    pub fn read(text: &str) -> KeyPauses {
        let mut kept = KeyPauses::default();
        for line in text.lines() {
            if let Some(p) = KeyPause::of_line(line) {
                kept.pauses.retain(|q| q.provider != p.provider);
                kept.pauses.push(p);
            } else if let Some((provider, at)) = line.strip_prefix("l ").and_then(|l| l.split_once(' ')) {
                if let Ok(at) = at.parse::<i64>() {
                    kept.lifted.retain(|(q, _)| q != provider);
                    kept.lifted.push((provider.to_string(), at));
                }
            }
        }
        kept
    }

    pub fn text(&self) -> String {
        let pauses = self.pauses.iter().map(KeyPause::line);
        let lifted = self.lifted.iter().map(|(provider, at)| format!("l {provider} {at}"));
        pauses.chain(lifted).collect::<Vec<_>>().join("\n")
    }

    /// The pauses not yet ended by their deadline, at `now_ms`.
    pub fn live(&self, now_ms: i64) -> impl Iterator<Item = &KeyPause> {
        self.pauses.iter().filter(move |p| p.resumes_at > now_ms)
    }

    fn lifted_at(&self, provider: &str) -> Option<i64> {
        self.lifted.iter().find(|(q, _)| q == provider).map(|(_, at)| *at)
    }

    /// Drops what no longer counts at `now_ms`.
    fn prune(&mut self, now_ms: i64) {
        self.pauses.retain(|p| p.resumes_at > now_ms);
        self.lifted.retain(|(_, at)| *at > now_ms - LIFT_KEPT_MS);
    }

    /// A refusal heard: `None` when it changes nothing kept (one heard
    /// before its owner last changed the cap, or no news of one told);
    /// else whether its owner's pages are told (a pause starting, or its
    /// cap changed).
    pub fn paused(&mut self, p: KeyPause, now_ms: i64) -> Option<bool> {
        if self.lifted_at(&p.provider).is_some_and(|at| p.paused_at <= at) {
            return None;
        }
        let held = self.live(now_ms).find(|q| q.provider == p.provider).cloned();
        let tell = match &held {
            Some(h) if h.cap_micros == p.cap_micros && p.resumes_at < h.resumes_at + NEWS_MS => return None,
            Some(h) => h.cap_micros != p.cap_micros,
            None => true,
        };
        self.prune(now_ms);
        self.pauses.retain(|q| q.provider != p.provider);
        // a pause told again keeps the later of their times heard: a call
        // let through before it proves nothing of it
        let paused_at = held.map_or(p.paused_at, |h| h.paused_at.max(p.paused_at));
        self.pauses.push(KeyPause { paused_at, ..p });
        Some(tell)
    }

    /// A call on `provider` went through, let through by a ledger answer
    /// heard at `opened` (0: none heard, a ledger that did not answer):
    /// whether that ended its pause (its owner's pages told).
    pub fn used(&mut self, provider: &str, opened: i64, now_ms: i64) -> bool {
        let ended = self.pauses.iter().any(|p| p.provider == provider && p.paused_at < opened);
        if ended {
            let told = self.live(now_ms).any(|p| p.provider == provider);
            self.prune(now_ms);
            self.pauses.retain(|p| p.provider != provider);
            return told;
        }
        false
    }

    /// Its owner changed `provider`'s cap at `at_ms`: its pause ends, and a
    /// pause heard before then, arriving late, is refused. Whether a pause
    /// told ended (its owner's pages told).
    pub fn lifted(&mut self, provider: &str, at_ms: i64) -> bool {
        let told = self.live(at_ms).any(|p| p.provider == provider);
        self.prune(at_ms);
        self.pauses.retain(|p| p.provider != provider);
        self.lifted.retain(|(q, _)| q != provider);
        self.lifted.push((provider.to_string(), at_ms));
        told
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: i64 = 1_790_812_800_000;
    const MIN: i64 = 60_000;

    fn pause(paused_at: i64) -> KeyPause {
        KeyPause { provider: "elevenlabs".into(), spent_micros: 5_030_000, cap_micros: 5_000_000, allow_micros: Some(20_000_000), resumes_at: paused_at + 60 * MIN, paused_at }
    }

    fn told(k: &KeyPauses, now: i64) -> Vec<&str> {
        k.live(now).map(|p| p.provider.as_str()).collect()
    }

    /// Goal (Alex on #1117, P2 1): a call let through before the pause
    /// began, finishing after it, ends nothing; a call let through after
    /// it does. Method: a pause heard at T; a call opened at T - 1 reports
    /// in, then one opened at T + 1.
    #[test]
    fn a_call_let_through_before_the_pause_ends_nothing() {
        let mut k = KeyPauses::default();
        assert_eq!(k.paused(pause(T), T), Some(true), "a pause starting is told");
        assert!(!k.used("elevenlabs", T - 1, T + 2 * MIN), "in flight as it began: proves nothing");
        assert_eq!(told(&k, T + 2 * MIN), ["elevenlabs"]);
        assert!(!k.used("elevenlabs", 0, T + 2 * MIN), "let through by no answer (a ledger that did not answer): nothing");
        assert!(!k.used("xai", T + MIN, T + 2 * MIN), "another key's call: nothing");
        assert!(k.used("elevenlabs", T + 1, T + 2 * MIN), "let through by an answer heard after it: ended, told");
        assert!(told(&k, T + 2 * MIN).is_empty());
    }

    /// Goal: the same, its reports delivered late or out of order. Method:
    /// the call opened before the pause reports before the pause arrives,
    /// and after; a pause told again by a later refusal keeps the later
    /// time heard.
    #[test]
    fn late_reports_end_nothing_they_should_not() {
        // the in-flight call's report arrives first; the pause, late, still holds
        let mut k = KeyPauses::default();
        assert!(!k.used("elevenlabs", T - 1, T + MIN));
        assert_eq!(k.paused(pause(T), T + MIN), Some(true));
        assert_eq!(told(&k, T + MIN), ["elevenlabs"]);
        // told again by a later refusal (another isolate's): a call opened
        // between the two still proves nothing
        assert_eq!(k.paused(pause(T + 10 * MIN), T + 10 * MIN), Some(false), "a minute or more later: kept, not told anew");
        assert!(!k.used("elevenlabs", T + 5 * MIN, T + 11 * MIN), "opened between the refusals, by a stale allowance");
        assert_eq!(told(&k, T + 11 * MIN), ["elevenlabs"]);
        // an older refusal arriving after a newer one keeps the newer's time
        assert_eq!(k.paused(KeyPause { resumes_at: T + 200 * MIN, ..pause(T + 2 * MIN) }, T + 11 * MIN), Some(false));
        assert!(!k.used("elevenlabs", T + 5 * MIN, T + 11 * MIN), "still the later time heard");
    }

    /// Goal: its owner's change of the cap ends the pause, and a pause
    /// heard before it, delivered late (or a refusal still cached where
    /// it was heard), is refused; one heard after it is told. Method: a
    /// lift at T + 5 min, then pauses heard at T + 4 and T + 6 min.
    #[test]
    fn a_lift_ends_the_pause_and_refuses_late_ones() {
        let mut k = KeyPauses::default();
        k.paused(pause(T), T);
        assert!(k.lifted("elevenlabs", T + 5 * MIN), "the pause told ends");
        assert!(told(&k, T + 5 * MIN).is_empty());
        assert_eq!(k.paused(pause(T + 4 * MIN), T + 6 * MIN), None, "heard before the lift: refused");
        assert_eq!(k.paused(pause(T + 5 * MIN), T + 6 * MIN), None, "heard at it: refused");
        assert_eq!(k.paused(pause(T + 6 * MIN), T + 6 * MIN), Some(true), "heard after it: a new pause, told");
        assert!(!KeyPauses::default().lifted("elevenlabs", T), "no pause: nothing told");
    }

    /// Goal: a pause ends at its deadline, a report of no news writes
    /// nothing, and what is kept is bounded and reads back as written.
    /// Method: pauses of two keys, the clock past one's deadline; the text.
    #[test]
    fn deadlines_news_and_the_text() {
        let mut k = KeyPauses::default();
        k.paused(pause(T), T);
        let xai = KeyPause { provider: "xai".into(), allow_micros: None, resumes_at: T + 10 * MIN, ..pause(T) };
        assert_eq!(k.paused(xai.clone(), T), Some(true));
        assert_eq!(k.paused(KeyPause { resumes_at: T + 60 * MIN + 30_000, ..pause(T) }, T + MIN), None, "no news: nothing written");
        assert_eq!(k.paused(KeyPause { cap_micros: 20_000_000, ..pause(T + MIN) }, T + MIN), Some(true), "its cap changed: told");
        assert_eq!(told(&k, T + 10 * MIN - 1), ["xai", "elevenlabs"]);
        assert_eq!(told(&k, T + 10 * MIN), ["elevenlabs"], "xai's ended by its deadline");
        k.lifted("elevenlabs", T + 2 * MIN);
        let text = k.text();
        assert_eq!(KeyPauses::read(&text), k, "reads back as written: {text}");
        assert_eq!(xai.notice(), ComputerNotice::KeyPaused { provider: "xai".into(), spent_micros: 5_030_000, cap_micros: 5_000_000, allow_micros: None, resumes_at: T + 10 * MIN });
        assert_eq!(KeyPause::of_line(&xai.line()), Some(xai));
        for bad in ["", "p elevenlabs 1 2", "p elevenlabs 1 0 0 5 6", "p elevenlabs -1 2 0 5 6", "p elevenlabs 1 2 0 5 6 7", "q elevenlabs 1 2 0 5 6", "l elevenlabs x"] {
            assert_eq!(KeyPauses::read(bad), KeyPauses::default(), "{bad:?}");
        }
        // a change older than LIFT_KEPT_MS is let go as the next one writes
        k.lifted("xai", T + LIFT_KEPT_MS + 3 * MIN);
        assert_eq!(k.lifted.len(), 1, "{:?}", k.lifted);
    }
}
