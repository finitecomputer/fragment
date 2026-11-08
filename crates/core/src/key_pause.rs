//! A computer's paused operator keys (docs/ledger.md, "Shared-key guard";
//! FIN-448): what the swap heard its owner's ledger refuse, told its owner
//! as a notice (`ComputerNotice::KeyPaused`). A pause ends only at its
//! deadline (`resumes_at`, the ledger's own reckoning of when the hour lets
//! the key go), when its owner sets the key's cap, or when a later refusal
//! replaces it. A call's success ends nothing: no ordering of answers
//! across isolates is trusted (Alex and Codex on #1117).
//!
//! Its owner's cap is kept as they last set it, and a pause at any other
//! cap is a refusal heard before that change (delivered late, or still
//! cached where it was heard): refused, with no clock compared.
//!
//! Kept by the Computer DO as text (`text`, `read`), a line each: `p
//! <provider> <spent> <cap> <allow> <resumes at>` a pause (its `allow` 0
//! when there is none), `c <provider> <cap>` the cap its owner last set.
//! The cell's Wasm carries no decoder of its own for it. Bounded: a line of
//! each kind a provider at most.

use fragment_proto::computer::ComputerNotice;

/// A pause told again says nothing new unless its cap changed or it ends
/// at least this much later: each refused call reports it.
pub const NEWS_MS: i64 = 60_000;

/// One key paused, as the swap heard it.
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
    /// `(provider, cap)`: the key's cap as its owner last set it.
    caps: Vec<(String, i64)>,
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
            } else if let Some((provider, cap)) = line.strip_prefix("c ").and_then(|l| l.split_once(' ')) {
                if let Ok(cap) = cap.parse::<i64>() {
                    kept.caps.retain(|(q, _)| q != provider);
                    kept.caps.push((provider.to_string(), cap));
                }
            }
        }
        kept
    }

    pub fn text(&self) -> String {
        let pauses = self.pauses.iter().map(KeyPause::line);
        let caps = self.caps.iter().map(|(provider, cap)| format!("c {provider} {cap}"));
        pauses.chain(caps).collect::<Vec<_>>().join("\n")
    }

    /// The pauses not yet ended by their deadline, at `now_ms`.
    pub fn live(&self, now_ms: i64) -> impl Iterator<Item = &KeyPause> {
        self.pauses.iter().filter(move |p| p.resumes_at > now_ms)
    }

    /// A refusal heard: `None` when it changes nothing kept (a pause at a
    /// cap its owner has since changed, or no news of one told); else
    /// whether its owner's pages are told (a pause starting, or its cap
    /// changed).
    pub fn paused(&mut self, p: KeyPause, now_ms: i64) -> Option<bool> {
        if self.caps.iter().any(|(q, cap)| *q == p.provider && *cap != p.cap_micros) {
            return None;
        }
        let tell = match self.live(now_ms).find(|q| q.provider == p.provider) {
            Some(h) if h.cap_micros == p.cap_micros && p.resumes_at < h.resumes_at + NEWS_MS => return None,
            Some(h) => h.cap_micros != p.cap_micros,
            None => true,
        };
        self.pauses.retain(|q| q.provider != p.provider && q.resumes_at > now_ms);
        self.pauses.push(p);
        Some(tell)
    }

    /// Its owner set `provider`'s cap to `cap_micros` (the default's
    /// amount, set back to it): its pause ends, and one at another cap,
    /// heard before, is refused from now. Whether a pause told ended (its
    /// owner's pages told).
    pub fn capped(&mut self, provider: &str, cap_micros: i64, now_ms: i64) -> bool {
        let told = self.live(now_ms).any(|p| p.provider == provider);
        self.pauses.retain(|p| p.provider != provider && p.resumes_at > now_ms);
        self.caps.retain(|(q, _)| q != provider);
        self.caps.push((provider.to_string(), cap_micros));
        told
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

    fn told(k: &KeyPauses, now: i64) -> Vec<&str> {
        k.live(now).map(|p| p.provider.as_str()).collect()
    }

    /// Goal (Alex on #1117, review 5819483119): his reordered asks keep
    /// the pause. A asks at T and is delayed; B asks at T + 10 ms and is
    /// allowed at T + 20; a charge reaches the cap at T + 30; A is refused
    /// at T + 40; B's call completes at T + 50. Method: the computer hears
    /// A's refusal, in either order with B's completion (a delayed
    /// notification); B's completion is no event of its (ea7992b11 ended
    /// the pause on it, its test failing there).
    #[test]
    fn a_calls_success_ends_no_pause() {
        let mut k = KeyPauses::default();
        assert_eq!(k.paused(pause(5 * USD, T + 60 * MIN), T + 40), Some(true), "A's refusal: a pause starting, told");
        // B's call completes at T + 50: the computer is told of its use,
        // and nothing of its pauses changes
        assert_eq!(told(&k, T + 50), ["elevenlabs"], "the ledger still refuses it: told still, its button there");
        // the same, A's notification delivered late, after B's completion
        let mut late = KeyPauses::default();
        assert_eq!(late.paused(pause(5 * USD, T + 60 * MIN), T + 5 * MIN), Some(true));
        assert_eq!(told(&late, T + 5 * MIN), ["elevenlabs"]);
    }

    /// Goal: the pause ends as its owner sets the cap, and a refusal heard
    /// before (at the cap it was), delivered late or still cached where it
    /// was heard, is refused; one at the new cap is told. Method: a lift
    /// to $20, then pauses at $5 and at $20; then back to the default.
    #[test]
    fn its_owner_setting_the_cap_ends_it_and_refuses_late_ones() {
        let mut k = KeyPauses::default();
        k.paused(pause(5 * USD, T + 60 * MIN), T);
        assert!(k.capped("elevenlabs", 20 * USD, T + MIN), "the pause told ends");
        assert!(told(&k, T + MIN).is_empty());
        assert_eq!(k.paused(pause(5 * USD, T + 60 * MIN), T + 2 * MIN), None, "heard at the old cap: refused, however late");
        assert_eq!(k.paused(pause(20 * USD, T + 70 * MIN), T + 30 * MIN), Some(true), "at its new cap: a new pause, told");
        // back to the default: its pause at $20 ends, a late one at $20 is refused
        assert!(k.capped("elevenlabs", 5 * USD, T + 31 * MIN));
        assert_eq!(k.paused(pause(20 * USD, T + 70 * MIN), T + 32 * MIN), None);
        assert_eq!(k.paused(pause(5 * USD, T + 90 * MIN), T + 32 * MIN), Some(true));
        assert!(!KeyPauses::default().capped("elevenlabs", 5 * USD, T), "no pause: nothing told");
        assert!(KeyPauses::default().paused(pause(5 * USD, T + MIN), T).is_some(), "no cap set: any refusal is told");
    }

    /// Goal: a later refusal replaces a pause (a new deadline, a new cap),
    /// one of no news writes nothing, and a pause ends at its deadline.
    /// Method: refusals of one key a minute apart; a second key's pause
    /// past its deadline.
    #[test]
    fn a_later_refusal_replaces_it_and_its_deadline_ends_it() {
        let mut k = KeyPauses::default();
        k.paused(pause(5 * USD, T + 60 * MIN), T);
        assert_eq!(k.paused(pause(5 * USD, T + 60 * MIN + 30_000), T + MIN), None, "no news: nothing written");
        assert_eq!(k.paused(pause(5 * USD, T + 62 * MIN), T + 2 * MIN), Some(false), "a later deadline: kept, not told anew");
        assert_eq!(k.live(T + 2 * MIN).next().map(|p| p.resumes_at), Some(T + 62 * MIN));
        let xai = KeyPause { provider: "xai".into(), allow_micros: None, ..pause(5 * USD, T + 10 * MIN) };
        assert_eq!(k.paused(xai, T), Some(true));
        assert_eq!(told(&k, T + 10 * MIN - 1), ["elevenlabs", "xai"]);
        assert_eq!(told(&k, T + 10 * MIN), ["elevenlabs"], "xai's ended by its deadline");
        assert_eq!(told(&k, T + 62 * MIN), Vec::<&str>::new());
    }

    /// Goal: what is kept reads back as written, a line that does not read
    /// is none, and it stays bounded. Method: the text of two keys' pauses
    /// and a cap; malformed lines; many refusals of one key.
    #[test]
    fn the_text_reads_back_and_stays_bounded() {
        let mut k = KeyPauses::default();
        k.paused(pause(5 * USD, T + 60 * MIN), T);
        k.paused(KeyPause { provider: "xai".into(), allow_micros: None, ..pause(5 * USD, T + 10 * MIN) }, T);
        k.capped("fal", 20 * USD, T);
        assert_eq!(KeyPauses::read(&k.text()), k, "reads back as written: {}", k.text());
        let xai = KeyPause { provider: "xai".into(), allow_micros: None, ..pause(5 * USD, T + 10 * MIN) };
        assert_eq!(KeyPause::of_line(&xai.line()), Some(xai.clone()));
        assert_eq!(xai.notice(), ComputerNotice::KeyPaused { provider: "xai".into(), spent_micros: 5_030_000, cap_micros: 5 * USD, allow_micros: None, resumes_at: T + 10 * MIN });
        for bad in ["", "p elevenlabs 1 2", "p elevenlabs 1 0 0 5", "p elevenlabs -1 2 0 5", "p elevenlabs 1 2 0 5 6", "q elevenlabs 1 2 0 5", "c elevenlabs x"] {
            assert_eq!(KeyPauses::read(bad), KeyPauses::default(), "{bad:?}");
        }
        let mut many = KeyPauses::default();
        for n in 0..500 {
            many.paused(pause(5 * USD, T + 60 * MIN + n * NEWS_MS), T + n * 1_000);
            many.capped("fal", 5 * USD + n, T + n * 1_000);
        }
        assert_eq!((many.pauses.len(), many.caps.len()), (1, 1), "a line of each kind a key");
    }
}
