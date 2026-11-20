//! What the computer's tests share: a time, an image, a save's records, and
//! the lifecycle and its saves driven as the Computer DO drives them
//! (`Computer`).

use fragment_proto::computer::ComputerRestore;
use serde_json::{json, Value};

use super::*;

pub(super) const T: i64 = 1_800_000_000_000;

/// The phase, asleep without its due start (a test that says when that
/// start is due says it by the alarm beside it).
pub(super) fn bare(p: &Phase) -> Phase {
    match p {
        Phase::Asleep { .. } => Phase::ASLEEP,
        p => p.clone(),
    }
}

pub(super) fn started(l: &mut Lifecycle, at: i64) -> u64 {
    let s = l.apply(Event::Wake { why: Wake::Record }, at);
    let Some(Action::Start { generation }) = s.actions.first().cloned() else { panic!("a wake starts it: {s:?}") };
    l.apply(Event::Ready { generation }, at + 3_000);
    generation
}

/// The start `s` (applied `at`) owes: none in its own step, which asks
/// for an alarm at once; that alarm's step, which makes it.
pub(super) fn owed(l: &mut Lifecycle, s: &Step, at: i64) -> Step {
    assert!(!s.actions.iter().any(|a| matches!(a, Action::Start { .. })), "never in the invocation that saw it go: {s:?}");
    assert_eq!(s.alarm_ms, Some(at), "an alarm at once");
    assert!(l.start_due(at));
    l.apply(Event::Alarm, at)
}

pub(super) const IMAGE: &str = "registry/stub@sha256:1";

/// One save's records, as the DO's `DirectoryBackup` answers them.
pub(super) fn rec(id: &str) -> Vec<Value> {
    vec![json!({ "id": id, "dir": "/data" })]
}

/// The save a step asks for: whether it is held, and its number.
pub(super) fn save_asked(s: &Step) -> (u64, bool, u64) {
    match s.actions.iter().find(|a| matches!(a, Action::Save { .. })) {
        Some(Action::Save { generation, held, seq }) => (*generation, *held, *seq),
        _ => panic!("a save is asked for: {s:?}"),
    }
}

/// The lifecycle and its saves, as the Computer DO drives them: each
/// step's end told to the saves, each start planned and recorded, each
/// sleep's hold, save and stop reported back in turn.
pub(super) struct Computer {
    pub(super) life: Lifecycle,
    pub(super) saves: Saves,
    /// The image its next start runs, as the DO last looked: `IMAGE`
    /// until a test deploys another.
    pub(super) images: Images,
    pub(super) now: i64,
}

impl Computer {
    pub(super) fn new() -> Computer {
        let mut images = Images::default();
        assert!(images.looked(Some(IMAGE)), "its first look is news");
        Computer { life: Lifecycle::new(), saves: Saves::default(), images, now: T }
    }

    pub(super) fn apply(&mut self, e: Event) -> Step {
        self.apply_with(e, &Rules::default())
    }

    fn apply_with(&mut self, e: Event, rules: &Rules) -> Step {
        self.now += 1_000;
        let s = self.life.apply_with(e, self.now, rules);
        if let Some(ended) = s.ended {
            self.saves.ended(ended);
        }
        s
    }

    /// The start `s` owes, made by the alarm it asks for (`owed`).
    pub(super) fn owed(&mut self, s: &Step) -> Step {
        assert!(!s.actions.iter().any(|a| matches!(a, Action::Start { .. })), "never in the invocation that saw it go: {s:?}");
        assert_eq!(s.alarm_ms, Some(self.now), "an alarm at once");
        self.apply(Event::Alarm)
    }

    /// A wake that comes up: what it restored.
    pub(super) fn wake(&mut self) -> ComputerRestore {
        let s = self.apply(Event::Wake { why: Wake::Owner });
        let Some(Action::Start { generation }) = s.actions.iter().find(|a| matches!(a, Action::Start { .. })).cloned() else { panic!("{s:?}") };
        self.come_up(generation)
    }

    /// The start `generation` comes up on the image the DO looked at
    /// last (its `plan` looks, and starts what it saw).
    pub(super) fn come_up(&mut self, generation: u64) -> ComputerRestore {
        let image = self.images.next().map(str::to_string);
        let plan = self.saves.plan(image.as_deref());
        let save = self.saves.current().map(|c| c.id.clone()).filter(|_| plan != Plan::Nothing);
        self.saves.starting(generation, plan.source());
        let r = self.saves.came_up(generation, plan.source(), save.as_deref(), image.as_deref(), self.now).expect("the start under way");
        self.apply(Event::Ready { generation });
        r
    }

    /// The owner's sleep, held, saving `/data` as `save`; or failing to,
    /// past its bound (`unsaved_max_ms` 0), so it sleeps unsaved.
    pub(super) fn sleep(&mut self, save: Option<&str>) {
        let g = self.life.generation();
        let s = self.apply(Event::Sleep);
        assert!(s.actions.contains(&Action::Sleep { generation: g }), "{s:?}");
        let (_, held, seq) = save_asked(&self.apply(Event::Held { generation: g, held: true }));
        assert!(held);
        let s = match save {
            Some(id) => {
                self.saves.saved(g, rec(id), 0, self.now, held);
                self.apply(Event::Saved { generation: g, seq })
            }
            None => {
                assert!(self.saves.save_failed(g));
                self.apply_with(Event::SaveFailed { generation: g, seq, why: "no room".into() }, &Rules { unsaved_max_ms: 0 })
            }
        };
        assert!(s.actions.contains(&Action::Stop { generation: g, saved: save.is_some() }), "{s:?}");
        self.apply(Event::Asleep { generation: g });
    }

    /// A turn, and the save its end asks for once it settles, kept as `id`.
    pub(super) fn turn_saved(&mut self, id: &str) {
        let g = self.life.generation();
        self.apply(Event::Opened { socket: Socket::Keepalive });
        self.apply(Event::Closed { socket: Socket::Keepalive });
        self.now += SAVE_SETTLE_MS;
        assert_eq!(self.apply(Event::Alarm).actions.last(), Some(&Action::Hold { generation: g }));
        self.saved_as(id);
    }

    /// The hold under way answered, and its save kept as `id`.
    pub(super) fn saved_as(&mut self, id: &str) {
        let g = self.life.generation();
        let (_, held, seq) = save_asked(&self.apply(Event::Held { generation: g, held: true }));
        self.saves.saved(g, rec(id), 0, self.now, held);
        self.apply(Event::Saved { generation: g, seq });
    }

    /// Its container stops on its own: the start that follows (its
    /// owner's wake, when nothing wanted it).
    pub(super) fn crash(&mut self) -> u64 {
        let mut s = self.apply(Event::Exited { generation: self.life.generation() });
        if matches!(self.life.phase, Phase::Asleep { .. }) {
            s = self.apply(Event::Wake { why: Wake::Owner });
        }
        let Some(Action::Start { generation }) = s.actions.iter().find(|a| matches!(a, Action::Start { .. })).cloned() else { panic!("{s:?}") };
        generation
    }
}
