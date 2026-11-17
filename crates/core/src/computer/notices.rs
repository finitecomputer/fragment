//! What a computer's owner is told of it now (`notices`), and where they get
//! help when it won't start (`support_url_ok`).

use fragment_proto::computer::ComputerNotice;

use crate::computer::images::{version_running, Images};
use crate::computer::lifecycle::{Lifecycle, Phase, Rules};
use crate::computer::limits::UNSAVED_NOTICE_FAILURES;
use crate::computer::saves::{loss_cause, Saves};

/// A support link is at most this long.
pub const SUPPORT_URL_MAX_BYTES: usize = 512;

/// Whether `url` may be the deployment's support link (`FRAGMENT_SUPPORT_URL`,
/// the deploy config's `support_url`): where a person whose computer will
/// not start gets help, an `https:` page or a `mailto:` address, of visible
/// ASCII, at most `SUPPORT_URL_MAX_BYTES`. The shell links to it as given.
pub fn support_url_ok(url: &str) -> bool {
    let visible = url.bytes().all(|b| (0x21..=0x7e).contains(&b) && b != b'"' && b != b'<' && b != b'>' && b != b'\\');
    let scheme = url.strip_prefix("https://").is_some_and(|rest| rest.len() > 1) || url.strip_prefix("mailto:").is_some_and(|rest| rest.contains('@'));
    url.len() <= SUPPORT_URL_MAX_BYTES && visible && scheme
}

/// What a computer's owner is told of it now (docs/computers.md, "What its
/// owner is told"), the most pressing first, from what its Computer DO
/// knows alone (no clock: the same state tells the same, so a change of
/// what it tells is a change of state, which the DO tells its owner's open
/// pages of):
///
/// - `wont_wake`: its starts kept failing, and it starts again only when
///   its owner asks;
/// - `unsaved`: this life's saves are failing (a sleep's at once, awake
///   ones once `UNSAVED_NOTICE_FAILURES` failed in a row): since when its
///   work is in no save, the save a stop goes back to, and when the bound
///   stops it if no save works first and nothing uses it;
/// - `went_back`: a start went back to an older save, or will as it next
///   starts (asleep after a crash or an unsaved stop), until its owner says
///   they saw it (`Saves::saw`); the newest loss only, and none for a life
///   on which nothing was done since that save (`Ended::quiet`): it lost
///   nothing;
/// - `update`: it is awake on another version than its next start runs
///   (`Images`), with no turn running (no keepalive: never in the middle of
///   its agents' work), until its owner puts that version off
///   (`Images::later`). Never while it sleeps or starts: its next start
///   takes the new version anyway;
/// - `disk_filling`: a read found its disk used past `DISK_TOLD_USED_PCT`
///   with at most `DISK_TOLD_FREE_MAX` free, and none since found clearly
///   more room (`disk_clear`).
pub fn notices(life: &Lifecycle, saves: &Saves, images: &Images, rules: &Rules) -> Vec<ComputerNotice> {
    let mut out = Vec::with_capacity(5);
    if let Phase::Failed { why, .. } = &life.phase {
        out.push(ComputerNotice::WontWake { why: why.clone() });
    }
    if let (Some(running), Some(u)) = (life.running(), life.unsaved(rules)) {
        if u.failures >= UNSAVED_NOTICE_FAILURES || u.sleep_failed_since_ms.is_some() {
            // its work is in no save since this life's newest save, or, with
            // none of its own, since it came up (its /data was that save)
            let own = saves.all().first().filter(|s| s.generation == running).map(|s| s.at_ms);
            let began = saves.restored().filter(|r| r.generation == running).map(|r| r.at);
            out.push(ComputerNotice::Unsaved {
                since: own.or(began).unwrap_or(u.failing.since_ms),
                save: saves.current().map(|s| s.number),
                failing_since: u.failing.since_ms,
                failures: u.failures,
                why: u.failing.why,
                stops_at: u.stops_at_ms,
            });
        }
    }
    // a loss its next start goes back for is newer than any it went back for
    let pending = saves.pending_loss().filter(|_| life.running().is_none() || matches!(life.phase, Phase::Starting { .. }));
    match (pending, saves.went_back()) {
        (Some(e), _) if e.generation > saves.seen => {
            let back_to = saves.current();
            out.push(ComputerNotice::WentBack {
                life: e.generation,
                cause: loss_cause(e),
                ended_at: (e.at_ms > 0).then_some(e.at_ms),
                pending: true,
                save: back_to.map(|s| s.number),
                saved_at: back_to.map(|s| s.at_ms),
                at: None,
            });
        }
        (Some(_), _) => {}
        (None, Some(w)) if w.life > saves.seen => out.push(ComputerNotice::WentBack {
            life: w.life,
            cause: w.cause,
            ended_at: (w.ended_at_ms > 0).then_some(w.ended_at_ms),
            pending: false,
            save: w.save,
            saved_at: w.restored.saved_at,
            at: Some(w.restored.at),
        }),
        (None, _) => {}
    }
    let awake = matches!(life.phase, Phase::Awake { .. });
    if let (true, Some(running), Some(latest)) = (awake, version_running(life, saves), images.latest()) {
        if running != latest && life.keepalives() == 0 && images.later.as_deref() != Some(latest.as_str()) {
            out.push(ComputerNotice::Update { version: latest });
        }
    }
    if let Some(d) = saves.disk.filter(|_| saves.disk_told) {
        out.push(ComputerNotice::DiskFilling { total: d.total, free: d.free });
    }
    assert!(out.len() <= 5, "one notice of each kind at most");
    out
}

#[cfg(test)]
mod tests {
    use fragment_proto::computer::{ComputerNotice, LifeEnd, LossCause};

    use super::*;
    use crate::computer::testing::{save_asked, Computer};
    use crate::computer::*;

    /// Goal (told afterwards, once): a start that went back in time is told
    /// to its owner, with what it went back to (which save, when it was
    /// taken) and why (a crash, an unsaved stop, a restart's failed save, a
    /// save that would not restore); while it is asleep after the loss, the
    /// same notice says it will. Once they say they saw it, it is told no
    /// more, nor is any loss before it, whatever starts follow; a loss not
    /// told is refused; seeing it twice is the same. It survives a restart
    /// of the Durable Object. Method: one computer's history.
    #[test]
    fn its_owner_is_told_once_what_a_start_went_back_to() {
        let rules = Rules::default();
        let went_back = |c: &Computer| notices(&c.life, &c.saves, &c.images, &rules).into_iter().find(|n| matches!(n, ComputerNotice::WentBack { .. }));
        let mut c = Computer::new();
        c.wake();
        c.sleep(Some("b1"));
        let saved_at = c.saves.current().map(|s| s.at_ms);
        let r = c.wake();
        assert_eq!(went_back(&c), None, "a wake from its sleep's save went back nowhere");
        // a message for its agent, then a crash: it comes back from b1
        c.apply(Event::Wake { why: Wake::Record });
        let g = r.generation;
        let s = c.apply(Event::Exited { generation: g });
        let crashed_at = c.now;
        let s = c.owed(&s);
        let Some(Action::Start { generation }) = s.actions.iter().find(|a| matches!(a, Action::Start { .. })).cloned() else { panic!("{s:?}") };
        let pending = ComputerNotice::WentBack { life: g, cause: LossCause::Crash, ended_at: Some(crashed_at), pending: true, save: Some(1), saved_at, at: None };
        assert_eq!(went_back(&c), Some(pending), "starting again: it goes back to b1");
        let r = c.come_up(generation);
        let told = ComputerNotice::WentBack { life: g, cause: LossCause::Crash, ended_at: Some(crashed_at), pending: false, save: Some(1), saved_at, at: Some(r.at) };
        assert_eq!(went_back(&c), Some(told.clone()));
        // a sleep that saves, and a wake from it: the loss is still told
        c.sleep(Some("b2"));
        c.wake();
        assert_eq!(went_back(&c), Some(told), "told until seen, whatever starts follow");
        assert!(!c.saves.saw(g + 7) && !c.saves.saw(0), "no notice of those was ever told");
        assert!(c.saves.saw(g - 1) && went_back(&c).is_some(), "an older notice seen hides only itself");
        assert!(c.saves.saw(g) && c.saves.saw(g), "seen, and again the same");
        assert_eq!(went_back(&c), None);
        // a turn, then an unsaved stop: pending while asleep, then told as
        // it comes up
        let g = c.life.generation();
        c.apply(Event::Opened { socket: Socket::Keepalive });
        c.apply(Event::Closed { socket: Socket::Keepalive });
        c.sleep(None);
        let pending = went_back(&c).expect("asleep after an unsaved stop");
        assert!(matches!(pending, ComputerNotice::WentBack { life, cause: LossCause::Unsaved, pending: true, save: Some(2), .. } if life == g), "{pending:?}");
        // seen while pending: the start after it tells it no more
        assert!(c.saves.saw(g));
        c.wake();
        assert_eq!(went_back(&c), None, "seen as it was pending");
        // a save that will not restore: the start falls back, told as such
        c.sleep(Some("b3"));
        let s = c.apply(Event::Wake { why: Wake::Owner });
        let Some(Action::Start { generation }) = s.actions.first().cloned() else { panic!("{s:?}") };
        assert!(c.saves.unusable("b3"));
        let s = c.apply(Event::RestoreFailed { generation, why: "corrupt".into(), older: true });
        let s = c.owed(&s);
        let Some(Action::Start { generation }) = s.actions.first().cloned() else { panic!("{s:?}") };
        let r = c.come_up(generation);
        assert!(r.rollback);
        let n = went_back(&c).expect("told");
        assert!(matches!(n, ComputerNotice::WentBack { cause: LossCause::Unusable, save: Some(2), pending: false, .. }), "{n:?}");
        // what it knows survives a restart of its Durable Object
        let back: Saves = serde_json::from_str(&serde_json::to_string(&c.saves).unwrap()).unwrap();
        assert_eq!(back, c.saves);
        assert_eq!(notices(&c.life, &back, &c.images, &rules), notices(&c.life, &c.saves, &c.images, &rules));
        // a record stored before notices reads as none told
        let mut v = serde_json::to_value(&c.saves).unwrap();
        for k in ["wentBack", "seen", "fresh"] {
            v.as_object_mut().unwrap().remove(k);
        }
        let older: Saves = serde_json::from_value(v).unwrap();
        assert!(older.went_back().is_none() && !older.fresh());
    }

    /// Goal (Paul, 2026-10-23: "if the user had no activity between last
    /// good save and stop I don't know if we even need the notice at all"):
    /// a stop that lost nothing tells its owner nothing. Something done on
    /// the lost life after the save it goes back to (a keepalive or a port
    /// tab opened or closed, a record or a join woke it, even as that save
    /// was taken): told, pending and once it came up. Nothing done since
    /// (its turns ended and were saved; a keepalive only stayed counted
    /// open, as the preview's did for 14 hours): nothing told, nor to see,
    /// while its guest still reads a rollback and it is counted. A loss
    /// told before and unseen stays told. Replay: a late report changes
    /// nothing. Restart: an end, or a lifecycle, stored before this was
    /// kept is told as any loss. Method: one computer's lives, `notices`
    /// read after each step.
    #[test]
    fn a_stop_that_lost_nothing_tells_nothing() {
        let rules = Rules::default();
        let went_back = |c: &Computer| notices(&c.life, &c.saves, &c.images, &rules).into_iter().find(|n| matches!(n, ComputerNotice::WentBack { .. }));
        let mut c = Computer::new();
        c.wake();
        c.sleep(Some("b1"));
        // a turn, its end saved, then only a page's presence, its owner's
        // port tab open since before that save: a crash loses nothing (with
        // nothing open, the same exit is a sleep: `starts`' tests)
        let g = c.wake().generation;
        c.apply(Event::Opened { socket: Socket::Tab });
        c.turn_saved("b2");
        c.apply(Event::Wake { why: Wake::Presence });
        let next = c.crash();
        assert_eq!(went_back(&c), None, "it starts again from b2, which holds all it did");
        assert!(!c.saves.saw(g), "nothing told, so nothing to see");
        let r = c.come_up(next);
        assert_eq!((r.save.as_deref(), r.after, r.rollback, c.saves.rollbacks()), (Some("b2"), Some(LifeEnd::Exit), true, 1), "its guest still reads a rollback: {r:?}");
        assert_eq!((went_back(&c), c.saves.went_back()), (None, None));
        let before = (c.life.clone(), c.saves.clone());
        assert!(c.apply(Event::Exited { generation: g }).actions.is_empty() && (c.life.clone(), c.saves.clone()) == before, "the crash's report again changes nothing");
        // the preview's (2026-10-23): a keepalive counted open across its
        // busy saves and nothing else; save b3 works, and the next one's
        // backup fails as its container goes
        let g = c.life.generation();
        c.apply(Event::Opened { socket: Socket::Keepalive });
        c.now += SAVE_EVERY_MS;
        assert_eq!(c.apply(Event::Alarm).actions.last(), Some(&Action::Hold { generation: g }), "busy: saved on its timer");
        c.saved_as("b3");
        c.now += SAVE_EVERY_MS;
        assert_eq!(c.apply(Event::Alarm).actions.last(), Some(&Action::Hold { generation: g }));
        let (_, _, seq) = save_asked(&c.apply(Event::Held { generation: g, held: true }));
        let next = c.crash();
        assert!(c.apply(Event::SaveFailed { generation: g, seq, why: "backup: sandbox-shim returned truncated control data".into() }).actions.is_empty());
        assert_eq!(went_back(&c), None, "nothing was done since b3");
        let r = c.come_up(next);
        assert_eq!((r.save.as_deref(), r.rollback, went_back(&c)), (Some("b3"), true, None));
        // a life with no save of its own, nothing done: back to the save it came up from
        let next = c.crash();
        assert_eq!(went_back(&c), None, "back to b3, which it came up from");
        c.come_up(next);
        assert_eq!(went_back(&c), None);
        // invalid for quiet: a message after its save, told
        let g = c.life.generation();
        c.turn_saved("b4");
        c.apply(Event::Wake { why: Wake::Record });
        let next = c.crash();
        let pending = went_back(&c).expect("a message came after b4");
        assert!(matches!(pending, ComputerNotice::WentBack { life, cause: LossCause::Crash, pending: true, save: Some(4), .. } if life == g), "{pending:?}");
        c.come_up(next);
        let told = went_back(&c).expect("told once it came up");
        assert!(matches!(told, ComputerNotice::WentBack { life, pending: false, save: Some(4), .. } if life == g), "{told:?}");
        // a quiet stop after it (idle and saved: a sleep): the loss told
        // before, unseen, is still told
        c.turn_saved("b5");
        let next = c.crash();
        assert_eq!(went_back(&c), Some(told.clone()), "asleep after a stop that lost nothing");
        c.come_up(next);
        assert_eq!(went_back(&c), Some(told), "and once it came up");
        assert!(c.saves.saw(g) && went_back(&c).is_none() && !c.saves.saw(g + 1), "seen; the quiet one never told");
        // a keepalive that closed as its save was taken: its end may not be in it
        let g = c.life.generation();
        c.apply(Event::Opened { socket: Socket::Keepalive });
        c.now += SAVE_EVERY_MS;
        assert_eq!(c.apply(Event::Alarm).actions.last(), Some(&Action::Hold { generation: g }));
        c.apply(Event::Closed { socket: Socket::Keepalive });
        c.saved_as("b6");
        let mut next = c.crash();
        assert!(matches!(went_back(&c), Some(ComputerNotice::WentBack { life, save: Some(6), .. }) if life == g), "{:?}", went_back(&c));
        // a tab, or a join, on a life with no save of its own: told
        for work in [Event::Opened { socket: Socket::Tab }, Event::Wake { why: Wake::Joined }] {
            assert!(c.saves.saw(c.life.generation() - 1));
            c.come_up(next);
            let g = c.life.generation();
            c.apply(work.clone());
            next = c.crash();
            assert!(matches!(went_back(&c), Some(ComputerNotice::WentBack { life, save: Some(6), pending: true, .. }) if life == g), "{work:?}: {:?}", went_back(&c));
        }
        assert!(c.saves.saw(c.life.generation() - 1));
        // a sleep that slept unsaved, nothing done since its save: nothing
        c.come_up(next);
        c.sleep(None);
        assert_eq!(went_back(&c), None, "slept unsaved, back to b6, which it came up from");
        assert!(c.wake().rollback && went_back(&c).is_none());
        // restart: an end stored before `quiet` was kept is told (a crash:
        // a tab open since before its save)
        c.apply(Event::Opened { socket: Socket::Tab });
        c.turn_saved("b7");
        let next = c.crash();
        assert_eq!(went_back(&c), None);
        let mut v = serde_json::to_value(&c.saves).unwrap();
        v["ended"].as_object_mut().unwrap().remove("quiet");
        let older: Saves = serde_json::from_value(v).unwrap();
        assert!(notices(&c.life, &older, &c.images, &rules).iter().any(|n| matches!(n, ComputerNotice::WentBack { pending: true, .. })), "told as any loss");
        let back: Saves = serde_json::from_str(&serde_json::to_string(&c.saves).unwrap()).unwrap();
        assert_eq!(notices(&c.life, &back, &c.images, &rules), notices(&c.life, &c.saves, &c.images, &rules), "the same after its Durable Object restarts");
        // and a lifecycle stored before its work was kept counts none kept,
        // even after a save of its own
        assert!(c.come_up(next).rollback);
        let g = c.life.generation();
        let mut v = serde_json::to_value(&c.life).unwrap();
        v.as_object_mut().unwrap().retain(|k, _| k != "worked_ms" && k != "kept_ms");
        c.life = serde_json::from_value(v).unwrap();
        c.turn_saved("b8");
        c.crash();
        assert!(matches!(went_back(&c), Some(ComputerNotice::WentBack { life, save: Some(8), .. }) if life == g), "{:?}", went_back(&c));
    }

    /// Valid: an https page or a mailto address. Invalid: anything a link
    /// should not be, or too long.
    #[test]
    fn a_support_link_is_a_page_or_an_address() {
        for ok in ["https://help.example.dev", "https://example.dev/support?from=computer", "mailto:help@example.dev"] {
            assert!(support_url_ok(ok), "{ok}");
        }
        for bad in ["", "http://example.dev", "https://", "javascript:alert(1)", "mailto:nobody", "https://a b", "https://a\"b", "https://a<b", "https://é.dev"] {
            assert!(!support_url_ok(bad), "{bad:?}");
        }
        assert!(support_url_ok(&format!("https://{}", "a".repeat(SUPPORT_URL_MAX_BYTES - 8))));
        assert!(!support_url_ok(&format!("https://{}", "a".repeat(SUPPORT_URL_MAX_BYTES - 7))));
    }

    /// Goal: won't wake is told, with its why, and goes once its owner's
    /// restart starts it.
    #[test]
    fn its_owner_is_told_it_wont_wake_until_a_restart() {
        let rules = Rules::default();
        let mut c = Computer::new();
        let s = c.apply(Event::Wake { why: Wake::Owner });
        let Some(Action::Start { mut generation }) = s.actions.first().cloned() else { panic!("{s:?}") };
        for _ in 0..STARTS_FAILED_MAX {
            let s = c.apply(Event::StartFailed { generation, why: "the image would not pull".into() });
            if let Some(at) = s.alarm_ms {
                c.now = at;
                let s = c.apply(Event::Alarm);
                let Some(Action::Start { generation: next }) = s.actions.first().cloned() else { panic!("{s:?}") };
                generation = next;
            }
        }
        assert_eq!(notices(&c.life, &c.saves, &c.images, &rules), vec![ComputerNotice::WontWake { why: "the image would not pull".into() }]);
        let s = c.apply(Event::Restart { generation });
        assert!(notices(&c.life, &c.saves, &c.images, &rules).is_empty() && c.life.starting(c.now), "starting again, its start owed");
        let s = c.owed(&s);
        assert!(matches!(s.actions.first(), Some(Action::Start { .. })), "{s:?}");
        assert!(notices(&c.life, &c.saves, &c.images, &rules).is_empty(), "starting again");
    }
}
