//! The live socket's bounds (cell/src/live.rs), as pure logic: the
//! queries one principal may run over its sockets, how often a socket
//! may change its presence, and which change is its person beginning to
//! type.

use std::collections::BTreeSet;

use fragment_proto::{limits, Role};
use serde_json::Value;

/// Why a socket's query was not run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryRefused {
    /// A query with this id is running on the socket: one at a time each.
    InFlight,
    /// The principal ran `LIVE_QUERIES_MAX` queries since the fragment
    /// last changed, or has that many running.
    Spent,
}

/// One principal's queries over its live sockets. A page re-runs its live
/// views after changes, so a principal may run `limits::LIVE_QUERIES_MAX`
/// queries between two changes to the fragment (a page with that many
/// views never runs out; one asking again and again between changes
/// does), no more than that many at once, and one at a time for each id
/// (the caller names a run by its socket and the page's id, so two tabs'
/// ids never collide). Its sockets share it: a socket opened again gets
/// no fresh budget. Queries over sockets are outside the public call
/// budget (opening a socket at the public role is one call of it): this
/// is their bound.
#[derive(Debug, Default)]
pub struct QueryBudget {
    /// The fragment's change count when the budget was last refilled.
    change: u64,
    used: u32,
    running: BTreeSet<String>,
}

impl QueryBudget {
    /// Admits a run of query `id` when the fragment has had `change` changes.
    pub fn admit(&mut self, id: &str, change: u64) -> Result<(), QueryRefused> {
        assert!(change >= self.change, "a fragment's change count only grows");
        if change > self.change {
            self.change = change;
            self.used = 0;
        }
        if self.running.contains(id) {
            return Err(QueryRefused::InFlight);
        }
        if self.used >= limits::LIVE_QUERIES_MAX || self.running.len() >= limits::LIVE_QUERIES_MAX as usize {
            return Err(QueryRefused::Spent);
        }
        self.used += 1;
        self.running.insert(id.to_string());
        assert!(self.running.len() <= limits::LIVE_QUERIES_MAX as usize, "a principal runs at most LIVE_QUERIES_MAX queries at once");
        Ok(())
    }

    /// A run of query `id` finished; answers whether it was running.
    pub fn done(&mut self, id: &str) -> bool {
        self.running.remove(id)
    }

    /// Whether no query is running on the socket.
    pub fn idle(&self) -> bool {
        self.running.is_empty()
    }
}

/// The steady gap between one socket's presence changes.
pub const PRESENCE_EVERY_MS: i64 = 1000 / limits::PRESENCE_PER_S;
const _: () = assert!(1000 % limits::PRESENCE_PER_S == 0, "a whole number of milliseconds between presence changes");
const _: () = assert!(limits::PRESENCE_BURST >= 1, "a socket may always make one change");

/// Whether a socket's presence change now is let through: at most
/// `PRESENCE_PER_S` a second, after a burst of `PRESENCE_BURST` (frames
/// bunch up on their way, and a page's library already keeps under the
/// pace). A generic cell rate algorithm: `due_ms` is when the socket's
/// next change is due at the steady pace (0 before its first). Answers
/// the next `due_ms` when the change goes through, `None` when it is
/// dropped (and then `due_ms` stands).
pub fn presence_admit(due_ms: i64, now_ms: i64) -> Option<i64> {
    let due = due_ms.max(now_ms);
    if due - now_ms > (limits::PRESENCE_BURST - 1) * PRESENCE_EVERY_MS {
        return None;
    }
    let next = due + PRESENCE_EVERY_MS;
    assert!(next > now_ms, "the next change is due after this one");
    Some(next)
}

/// Whether a socket's presence change says its person began typing (a
/// use of their own computer, when its agents follow the fragment:
/// `computer::prewoken`, decision 39): `typing` is `true` now and was not
/// on this socket before, on a page whose code is the platform's own
/// (`platform`, asked only then: a blessed template's release, decision
/// 40). The chat's composer turns it on at the first keystroke after a
/// pause and off after a send or `TYPING_MS` of quiet; nothing else of a
/// presence is read. A page sends its presence again as its socket
/// reconnects, and as its tab is shown or hidden: none of those is typing
/// begun. An app's page, whose code is its author's, may say it types with
/// no one there: its presence wakes nothing.
pub fn typing_began(platform: impl FnOnce() -> bool, before: Option<&Value>, now: &Value) -> bool {
    let typing = |v: &Value| v.get("typing") == Some(&Value::Bool(true));
    typing(now) && !before.is_some_and(typing) && platform()
}

/// Whether a page's typing is its person's own, as far as the platform
/// can tell (decision 39). A page sets its presence itself, and any page on
/// a fragment's origin can: an app's code, or a page someone added to a
/// blessed fragment's site (served beside the release's, serve.rs), may
/// say it types with no one there. So it counts only on a fragment of the
/// platform's code (`blessed`, a blessed template's release, decision 40,
/// asked only for its owner) and only from its owner (`role`): every page
/// on their fragment's origin is theirs or one they let its editors add,
/// and the computer it wakes is their own (`computer::prewoken`).
pub fn typing_trusted(role: Role, blessed: impl FnOnce() -> bool) -> bool {
    role == Role::Owner && blessed()
}

/// How often a fragment's owner's computer is pre-woken at most (decision
/// 39): typing in bursts is one sign.
pub const PREWAKE_EVERY_MS: i64 = 30_000;

/// When a fragment's owner's computer is next pre-woken: at most every
/// `PREWAKE_EVERY_MS`. Only its owner's typing pre-wakes (`typing_trusted`),
/// so one pace a fragment holds back no one else's. A fragment's memory
/// only: one that slept forgets, and its next typing begun wakes once more.
#[derive(Debug, Default)]
pub struct PrewakeDue(i64);

impl PrewakeDue {
    /// Whether the pre-wake is due at `now_ms`: when it is, it is next due
    /// `PREWAKE_EVERY_MS` on.
    pub fn take(&mut self, now_ms: i64) -> bool {
        if now_ms < self.0 {
            return false;
        }
        self.0 = now_ms + PREWAKE_EVERY_MS;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX: u32 = limits::LIVE_QUERIES_MAX;

    #[test]
    fn a_principal_runs_its_budget_between_changes() {
        let mut b = QueryBudget::default();
        for i in 0..MAX {
            assert_eq!(b.admit(&format!("q{i}"), 0), Ok(()));
            assert!(b.done(&format!("q{i}")));
        }
        assert_eq!(b.admit("q0", 0), Err(QueryRefused::Spent), "the budget is spent until the fragment changes");
        assert_eq!(b.admit("q0", 1), Ok(()), "a change refills it");
        assert!(b.done("q0"));
        for _ in 1..MAX {
            assert_eq!(b.admit("q0", 1), Ok(()));
            assert!(b.done("q0"));
        }
        assert_eq!(b.admit("q0", 1), Err(QueryRefused::Spent));
        assert_eq!(b.admit("q0", 5), Ok(()), "however many changes came, one refill");
    }

    #[test]
    fn one_run_at_a_time_for_each_id_and_a_bound_on_all() {
        let mut b = QueryBudget::default();
        assert_eq!(b.admit("list", 0), Ok(()));
        assert_eq!(b.admit("list", 0), Err(QueryRefused::InFlight));
        assert_eq!(b.admit("list", 3), Err(QueryRefused::InFlight), "a change does not let a second run of the same id start");
        assert!(b.done("list"));
        assert!(!b.done("list"), "a run finishes once");
        assert!(b.idle());
        // runs that never finish, across changes: at most MAX at once
        let mut b = QueryBudget::default();
        for i in 0..MAX {
            assert_eq!(b.admit(&format!("q{i}"), u64::from(i)), Ok(()));
        }
        assert_eq!(b.admit("one-more", u64::from(MAX)), Err(QueryRefused::Spent), "a fresh budget, but MAX running");
        assert!(b.done("q3"));
        assert_eq!(b.admit("one-more", u64::from(MAX)), Ok(()));
        assert!(!b.idle());
    }

    #[test]
    #[should_panic(expected = "only grows")]
    fn a_change_count_never_goes_back() {
        let mut b = QueryBudget::default();
        b.admit("q", 2).unwrap();
        let _ = b.admit("r", 1);
    }

    /// Changes at `at` (ms), from a socket that has made none: which go through.
    fn admitted(at: &[i64]) -> Vec<bool> {
        let mut due = 0;
        at.iter()
            .map(|now| match presence_admit(due, *now) {
                Some(next) => {
                    due = next;
                    true
                }
                None => false,
            })
            .collect()
    }

    #[test]
    fn a_burst_goes_through_then_the_steady_pace() {
        let t = 1_790_000_000_000;
        let burst = vec![t; limits::PRESENCE_BURST as usize + 1];
        let mut want = vec![true; limits::PRESENCE_BURST as usize];
        want.push(false);
        assert_eq!(admitted(&burst), want, "a burst of PRESENCE_BURST, and the next one dropped");
        let mut then = burst.clone();
        then.push(t + PRESENCE_EVERY_MS - 1);
        then.push(t + PRESENCE_EVERY_MS);
        let got = admitted(&then);
        assert_eq!(&got[burst.len()..], &[false, true], "one more once a gap has passed, and not before");
    }

    #[test]
    fn a_page_at_the_pace_is_never_dropped() {
        let t = 1_790_000_000_000;
        let steady: Vec<i64> = (0..1000).map(|i| t + i * PRESENCE_EVERY_MS).collect();
        assert!(admitted(&steady).iter().all(|a| *a), "a change every 100 ms, a thousand times");
        // the library sends at most every 150 ms; frames may arrive bunched
        let bunched: Vec<i64> = (0..1000).map(|i| t + i * 150 - if i % 7 == 0 { 140 } else { 0 }).collect();
        assert!(admitted(&bunched).iter().all(|a| *a), "a change every 150 ms, some arriving early");
    }

    #[test]
    fn a_flood_gets_the_pace_and_no_more() {
        let t = 1_790_000_000_000;
        // a change every millisecond for ten seconds
        let flood: Vec<i64> = (0..10_000).map(|i| t + i).collect();
        let through = admitted(&flood).iter().filter(|a| **a).count() as i64;
        assert_eq!(through, limits::PRESENCE_BURST + 10 * limits::PRESENCE_PER_S - 1, "a burst, then ten a second");
    }

    /// Goal (Paul, 2026-11-18): only typing begun is a use; a page that
    /// loads, reconnects, or is shown again sends its presence too, and is
    /// none. Valid (begun, on a fresh socket too), invalid (other values,
    /// shapes), replay (typing that goes on, or the same presence again).
    #[test]
    fn only_typing_turning_on_is_typing_begun() {
        let page = serde_json::json!({ "typing": false, "looking": true });
        let hidden = serde_json::json!({ "typing": false, "looking": false });
        let typing = serde_json::json!({ "typing": true, "looking": true });
        assert!(!typing_began(|| true, None, &page), "a page loaded, or its socket reconnected, says it is here");
        assert!(!typing_began(|| true, Some(&page), &hidden) && !typing_began(|| true, Some(&hidden), &page), "the tab hidden, then shown again");
        assert!(typing_began(|| true, Some(&page), &typing), "the first keystroke after a pause");
        assert!(typing_began(|| true, None, &typing), "a socket reconnected as its person types");
        assert!(!typing_began(|| true, Some(&typing), &serde_json::json!({ "typing": true, "looking": false })), "typing goes on");
        assert!(!typing_began(|| true, Some(&typing), &typing), "the same presence again");
        assert!(typing_began(|| true, Some(&page), &typing) && !typing_began(|| true, Some(&typing), &page), "a pause, then typing again begins again");
        for other in [serde_json::json!({ "typing": "yes" }), serde_json::json!({ "typing": 1 }), serde_json::json!(null), serde_json::json!([true]), serde_json::json!("typing"), serde_json::json!({ "x": 1, "y": 2 })] {
            assert!(!typing_began(|| true, None, &other), "{other}");
        }
    }

    /// Goal (review of #1351): an app's page, whose code is its author's,
    /// may set `typing` with no one there; only a page of the platform's
    /// code is a person typing. And the platform's code is asked only
    /// when typing began, not at every presence.
    #[test]
    fn an_apps_own_page_typing_is_no_sign() {
        let page = serde_json::json!({ "typing": false, "looking": true });
        let typing = serde_json::json!({ "typing": true, "looking": true });
        assert!(!typing_began(|| false, Some(&page), &typing), "an app's page says it types");
        assert!(typing_began(|| true, Some(&page), &typing), "the platform's chat");
        assert!(!typing_began(|| panic!("asked for a presence that began nothing"), Some(&page), &page));
    }

    /// Goal: a fragment's owner's typing in bursts pre-wakes their computer
    /// once a pace. Valid (the first), replay (within the pace, nothing),
    /// the bound (once it passed, again).
    #[test]
    fn its_owners_computer_is_pre_woken_once_a_pace() {
        let mut due = PrewakeDue::default();
        assert!(due.take(1_000));
        assert!(!due.take(1_000 + PREWAKE_EVERY_MS - 1), "within its pace");
        assert!(due.take(1_000 + PREWAKE_EVERY_MS), "once its pace passed");
    }

    /// Goal (review of #1351): a page may set `typing` on its own, so only
    /// the fragment's owner's typing on a fragment of the platform's code
    /// counts. Invalid: anyone else on a blessed chat (a visitor opening a
    /// page someone added to its site, a member, a reader), and its owner
    /// on a fragment of its own code; and the platform's code is asked
    /// only for its owner.
    #[test]
    fn only_the_owners_typing_on_the_platforms_code_is_trusted() {
        assert!(typing_trusted(Role::Owner, || true), "its owner, in their chat");
        for role in [Role::Public, Role::Viewer, Role::Editor] {
            assert!(!typing_trusted(role, || true), "{role:?}, on someone else's blessed chat");
            assert!(!typing_trusted(role, || panic!("asked for one who is not its owner")));
        }
        assert!(!typing_trusted(Role::Owner, || false), "its owner, on a fragment of its own code");
    }
}
