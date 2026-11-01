//! The gate's cache (docs/ledger.md, "The gate"): what a person's ledger
//! last answered a question a spend asks before it starts (`may_spend`, or
//! `fragment_open` of a fragment they own), kept by each isolate, so a paid
//! call or a write waits on the ledger at most once a `STANDING_CACHE_MS`,
//! never on every call (decision 62).
//!
//! A cache, so its contract. Source: the person's ledger (its
//! `/may-spend` and `/fragment-open` routes). Invalidation: time alone, and
//! a test fleet's `forget-standing` lever, which forgets every answer.
//! Stale reads: for up to `STANDING_CACHE_MS` after a person's standing
//! changes, an isolate answers as it last heard: a person out of credit
//! spends on for up to a minute more (what it spends is charged: meters
//! always record), and one who topped up waits up to a minute for their
//! agents (Paul, 2026-10-21: "'overspend by whatever is in flight during
//! the cache window' is a small price to pay"). A refusal is kept as an
//! answer is. A ledger that does not answer is no answer: the spend goes
//! on, and nothing is kept, so the next one asks again.

use std::future::Future;

use super::Refused;

/// How long an isolate trusts what a ledger answered its gate.
pub const STANDING_CACHE_MS: i64 = 60_000;
/// Answers an isolate keeps at once: one per person and question it asked
/// within the window. Past it, the stale go first, then the oldest.
pub const GATE_ENTRIES_MAX: usize = 1_024;

const _: () = assert!(STANDING_CACHE_MS <= 60_000, "a spend learns its payer's standing within a minute");

/// One question's answer, and when it was heard (the isolate's clock).
#[derive(Debug)]
struct Seen {
    key: String,
    at_ms: i64,
    answer: Result<(), Refused>,
}

/// What each question was last answered. A list searched whole (at most
/// `GATE_ENTRIES_MAX`, a few microseconds), not a map: the cell's Wasm is
/// held to a budget, and a map's code costs more of it.
#[derive(Debug, Default)]
pub struct Gate {
    seen: Vec<Seen>,
}

fn fresh_at(at_ms: i64, now_ms: i64) -> bool {
    (0..STANDING_CACHE_MS).contains(&(now_ms - at_ms))
}

impl Gate {
    pub const fn new() -> Gate {
        Gate { seen: Vec::new() }
    }

    /// The answer to `key` heard less than `STANDING_CACHE_MS` before
    /// `now_ms`. A clock that runs backwards makes it stale, never fresher.
    pub fn fresh(&self, key: &str, now_ms: i64) -> Option<Result<(), Refused>> {
        let seen = self.seen.iter().find(|s| s.key == key)?;
        fresh_at(seen.at_ms, now_ms).then_some(seen.answer)
    }

    /// What the ledger said to `key`, asked at `now_ms` (`None`: it did not
    /// answer, or not as a ledger does), as the spend reads it: a refusal
    /// stops it, anything else lets it start. An answer is kept; no answer
    /// is not, so the next spend asks again.
    pub fn heard(&mut self, key: &str, answer: Option<Result<(), Refused>>, now_ms: i64) -> Result<(), Refused> {
        let Some(answer) = answer else { return Ok(()) };
        if let Some(seen) = self.seen.iter_mut().find(|s| s.key == key) {
            (seen.at_ms, seen.answer) = (now_ms, answer);
            return answer;
        }
        if self.seen.len() >= GATE_ENTRIES_MAX {
            self.seen.retain(|s| fresh_at(s.at_ms, now_ms));
        }
        if self.seen.len() >= GATE_ENTRIES_MAX {
            let oldest = self.seen.iter().enumerate().min_by_key(|(_, s)| s.at_ms).map(|(i, _)| i).expect("a full gate holds an answer");
            self.seen.swap_remove(oldest);
        }
        self.seen.push(Seen { key: key.to_string(), at_ms: now_ms, answer });
        assert!(self.seen.len() <= GATE_ENTRIES_MAX, "the gate keeps at most GATE_ENTRIES_MAX answers");
        answer
    }

    /// Forgets every answer (a test fleet's `forget-standing`).
    pub fn forget(&mut self) {
        self.seen.clear();
    }

    pub fn len(&self) -> usize {
        self.seen.len()
    }

    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }
}

/// A spend's two questions, asked side by side (cell models.rs `gate`):
/// its payer's `may_spend`, and, when another person owns its fragment,
/// that owner's `fragment_open`. Neither answer depends on the other, so
/// neither waits for the other (cold, each wakes a Ledger: FIN-320). Both
/// must allow; both refusing, the payer's refusal is the one said.
pub async fn asked_together<E>(payer: impl Future<Output = Result<(), E>>, owner: Option<impl Future<Output = Result<(), E>>>) -> Result<(), E> {
    let owner = async {
        match owner {
            Some(asked) => asked.await,
            None => Ok(()),
        }
    };
    let (payer, owner) = futures_util::future::join(payer, owner).await;
    payer.and(owner)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::future::{poll_fn, ready};
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    use futures_util::FutureExt;

    use super::*;
    use fragment_proto::ledger::Why;

    const STOPPED: Refused = Refused::AgentsStopped { why: Why::NoCredit };

    /// The window: an answer heard at t is the answer until t + 60 s, asked
    /// again from then on; a refusal is kept as an allowance is, and an
    /// answer heard again replaces the one before. Method: a gate driven by
    /// a clock the test holds.
    #[test]
    fn an_answer_is_trusted_for_at_most_a_minute() {
        let mut gate = Gate::new();
        assert_eq!(gate.fresh("p may", 1_000), None, "nothing heard, nothing kept");
        assert_eq!(gate.heard("p may", Some(Ok(())), 1_000), Ok(()));
        assert_eq!(gate.fresh("p may", 1_000), Some(Ok(())));
        assert_eq!(gate.fresh("p may", 1_000 + STANDING_CACHE_MS - 1), Some(Ok(())), "fresh until the minute is out");
        assert_eq!(gate.fresh("p may", 1_000 + STANDING_CACHE_MS), None, "asked again once it is");
        assert_eq!(gate.fresh("q may", 1_000), None, "each question is its own");

        // the person ran out: a refusal heard is the answer for its minute
        assert_eq!(gate.heard("p may", Some(Err(STOPPED)), 1_000 + STANDING_CACHE_MS), Err(STOPPED));
        assert_eq!(gate.fresh("p may", 1_000 + 2 * STANDING_CACHE_MS - 1), Some(Err(STOPPED)));
        assert_eq!(gate.fresh("p may", 1_000 + 2 * STANDING_CACHE_MS), None);
    }

    /// Fail open: a ledger that does not answer lets the spend start and is
    /// kept as nothing, so the next spend asks again, and what was heard
    /// before is not replaced by the silence.
    #[test]
    fn no_answer_lets_the_spend_start_and_is_not_kept() {
        let mut gate = Gate::new();
        assert_eq!(gate.heard("p may", None, 1_000), Ok(()));
        assert_eq!(gate.fresh("p may", 1_000), None, "no answer is no answer: asked again");
        assert!(gate.is_empty());
        assert_eq!(gate.heard("p may", Some(Err(STOPPED)), 1_000), Err(STOPPED));
        assert_eq!(gate.heard("p may", None, 2_000), Ok(()), "a silence lets this one start");
        assert_eq!(gate.fresh("p may", 2_000), Some(Err(STOPPED)), "and leaves the refusal heard before as it was");
    }

    /// A clock that steps back (an isolate's clock is the request's) makes
    /// an answer stale: asked again, never trusted for longer.
    #[test]
    fn a_clock_that_runs_backwards_asks_again() {
        let mut gate = Gate::new();
        assert_eq!(gate.heard("p may", Some(Ok(())), 50_000), Ok(()));
        assert_eq!(gate.fresh("p may", 49_999), None);
        assert_eq!(gate.fresh("p may", 50_000), Some(Ok(())));
    }

    /// Side by side, with no clock: each ask in turn stays pending until
    /// the other has been asked (polled), so asked one after the other in
    /// either order they never finish; asked together they finish by the
    /// second poll. Method: the joined future polled by hand, with a waker
    /// that does nothing.
    #[test]
    fn the_two_asks_never_wait_on_each_other() {
        // an ask that, if it `waits`, answers only once the other was asked
        fn ask<'a>(mine: &'a Cell<bool>, theirs: &'a Cell<bool>, waits: bool) -> impl Future<Output = Result<(), Refused>> + 'a {
            poll_fn(move |_| {
                mine.set(true);
                if waits && !theirs.get() {
                    Poll::Pending
                } else {
                    Poll::Ready(Ok(()))
                }
            })
        }
        for payer_waits in [true, false] {
            let (payer_asked, owner_asked) = (Cell::new(false), Cell::new(false));
            let payer = ask(&payer_asked, &owner_asked, payer_waits);
            let owner = ask(&owner_asked, &payer_asked, !payer_waits);
            let mut both = pin!(asked_together(payer, Some(owner)));
            let mut cx = Context::from_waker(Waker::noop());
            // at most two polls, none after it answers
            let answered = (0..2).find_map(|_| match both.as_mut().poll(&mut cx) {
                Poll::Ready(answer) => Some(answer),
                Poll::Pending => None,
            });
            let waiting = if payer_waits { "payer's" } else { "owner's" };
            assert_eq!(answered, Some(Ok(())), "the {waiting} ask answers once the other is asked: asked alone first, it never would");
        }
    }

    /// The decision, and whose refusal is said: both must allow; either
    /// refusing refuses; both refusing, the payer's is said; with no owner
    /// to ask, the payer's answer is the whole answer.
    #[test]
    fn both_must_allow_and_the_payers_refusal_comes_first() {
        const CAPPED: Refused = Refused::CapReached { cap: 0, spent: 0 };
        let answer = |payer: Result<(), Refused>, owner: Option<Result<(), Refused>>| asked_together(ready(payer), owner.map(ready)).now_or_never().expect("ready asks answer at once");
        assert_eq!(answer(Ok(()), Some(Ok(()))), Ok(()));
        assert_eq!(answer(Err(STOPPED), Some(Ok(()))), Err(STOPPED));
        assert_eq!(answer(Ok(()), Some(Err(CAPPED))), Err(CAPPED));
        assert_eq!(answer(Err(STOPPED), Some(Err(CAPPED))), Err(STOPPED), "the payer's refusal is said");
        assert_eq!(answer(Ok(()), None), Ok(()));
        assert_eq!(answer(Err(STOPPED), None), Err(STOPPED));
    }

    /// Bounded: at the bound the stale answers go first, then the oldest;
    /// an answer heard again for a question kept takes no room; and
    /// `forget` empties it.
    #[test]
    fn the_gate_keeps_at_most_its_bound() {
        let mut gate = Gate::new();
        for i in 0..GATE_ENTRIES_MAX {
            assert_eq!(gate.heard(&format!("p{i}"), Some(Ok(())), i as i64), Ok(()));
        }
        assert_eq!(gate.len(), GATE_ENTRIES_MAX);
        assert_eq!(gate.heard("p0", Some(Err(STOPPED)), 2_000), Err(STOPPED));
        assert_eq!(gate.len(), GATE_ENTRIES_MAX, "a question kept again takes no room");
        // all fresh: the oldest (p1, now that p0 was heard again) goes
        assert_eq!(gate.heard("new", Some(Ok(())), 2_000), Ok(()));
        assert_eq!((gate.len(), gate.fresh("p1", 2_000), gate.fresh("p0", 2_000)), (GATE_ENTRIES_MAX, None, Some(Err(STOPPED))));
        // a minute on, every answer before 2_000 is stale: they all go
        let later = 2 + STANDING_CACHE_MS + GATE_ENTRIES_MAX as i64;
        assert_eq!(gate.heard("newer", Some(Ok(())), later), Ok(()));
        assert_eq!((gate.len(), gate.fresh("newer", later)), (3, Some(Ok(()))), "the stale went, the two heard at 2_000 and the new one stayed");
        gate.forget();
        assert!(gate.is_empty());
        assert_eq!(gate.fresh("newer", later), None);
    }
}
