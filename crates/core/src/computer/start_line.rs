//! One log line per start of a computer (docs/computers.md, "The start
//! line"): the path it took, why a whole restore when it is one, what it
//! restored, and how long each step took from the wake that asked for it.
//! The Computer DO keeps the start under way in memory (`Starts`), tells it
//! each step as it happens, and writes its line once, as it ends: failed,
//! replaced, or ready (once its guest takes a turn, `TURN_WAIT_MS` pass, or
//! its life ends first). A week of these says how production wakes go.
//!
//! In memory only: nothing is stored, and no step waits on it. A start
//! whose isolate goes before its line is written (a deploy, a crash of the
//! platform) has none, and an ask an earlier isolate heard is not known.

use std::fmt::Write;

use fragment_proto::computer::RestoreSource;

use super::{Event, Lifecycle, Phase, Socket, Wake};

/// After a start comes up, how long its line waits for its guest's first
/// turn (a keepalive opened): a turn its bridge takes up after that is not
/// its wake's.
pub const TURN_WAIT_MS: i64 = 60_000;
/// A computer's id, as a line names it, at most this long (`computer:` and
/// 24 hex digits are 33).
pub const LINE_ID_MAX_BYTES: usize = 64;
/// A line, whole, at most this long.
pub const LINE_MAX_BYTES: usize = 512;

/// Why a start restores its save whole, rather than from a snapshot
/// (`Saves::full_why`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Full {
    /// This deployment takes no snapshots (or names no image to take them of).
    Off,
    /// Its owner restarted it: a restart's start is fresh from the image.
    Restart,
    /// A newer save would not restore: it falls back to an older one.
    Unusable,
    /// Its snapshot is of another image (a deploy, or a pin).
    Image,
    /// Its start from the snapshot, in this wake, failed or came up dead.
    DeadSnapshot,
    /// Its last life ended by an exit: a crash, or a host's stop of it
    /// while its work was unsaved.
    Crash,
    /// Its last sleep could not save.
    Unsaved,
    /// Its last life ended saved, and no snapshot of that save is kept: a
    /// host stopped it while it was idle, or that sleep's snapshot failed.
    NoSnapshot,
    /// None of these (a computer whose last end the DO never kept).
    Other,
}

impl Full {
    fn code(self) -> &'static str {
        match self {
            Full::Off => "off",
            Full::Restart => "restart",
            Full::Unusable => "unusable",
            Full::Image => "image",
            Full::DeadSnapshot => "dead_snapshot",
            Full::Crash => "crash",
            Full::Unsaved => "unsaved",
            Full::NoSnapshot => "no_snapshot",
            Full::Other => "other",
        }
    }
}

/// What asked for the wake a start is of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Woke {
    Wake(Wake),
    /// Its owner's restart.
    Restart,
}

impl Woke {
    fn code(self) -> &'static str {
        match self {
            Woke::Wake(Wake::Record) => "record",
            Woke::Wake(Wake::Presence) => "presence",
            Woke::Wake(Wake::Tab) => "tab",
            Woke::Wake(Wake::Owner) => "owner",
            Woke::Wake(Wake::Joined) => "joined",
            Woke::Restart => "restart",
        }
    }
}

/// Why a start did not come up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failed {
    /// Its own steps failed (the platform, the container, the runtime).
    Start,
    /// Its snapshot would not start.
    Snapshot,
    /// Its guest never read its view within its wait: a start from a
    /// snapshot that came up dead (#362), or the `alive-check` lever's.
    Dead,
    /// Its save would not restore.
    Unusable,
    /// Its container stopped before it came up.
    Exit,
    /// It did not come up within `START_DEADLINE_MS`.
    Deadline,
    /// The container before it still ran past its destroy: it started
    /// nothing, and is made again (`Event::StartBlocked`).
    Blocked,
    Other,
}

impl Failed {
    fn code(self) -> &'static str {
        match self {
            Failed::Start => "start",
            Failed::Snapshot => "snapshot",
            Failed::Dead => "dead",
            Failed::Unusable => "unusable",
            Failed::Exit => "exit",
            Failed::Deadline => "deadline",
            Failed::Blocked => "blocked",
            Failed::Other => "other",
        }
    }

    /// Why, as the lifecycle heard it, when the start said nothing first.
    fn by(event: &Event) -> Failed {
        match event {
            Event::StartFailed { .. } => Failed::Start,
            Event::SnapshotFailed { .. } => Failed::Snapshot,
            Event::RestoreFailed { .. } => Failed::Unusable,
            Event::Exited { .. } => Failed::Exit,
            Event::Alarm => Failed::Deadline,
            Event::StartBlocked { .. } => Failed::Blocked,
            _ => Failed::Other,
        }
    }
}

/// What a start restores, as its plan chose it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts {
    pub path: RestoreSource,
    /// Why a whole restore (`path` the save's alone).
    pub why: Option<Full>,
    /// The stored bytes of the save it restores, or that its snapshot
    /// caches (each record's `size`: archives, compressed).
    pub bytes: Option<u64>,
    /// Whether it runs another image than its last life ran (none: no
    /// life of it ran yet). Whether the host pulled it is not known here.
    pub new_image: Option<bool>,
}

/// How a start's line ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    Ready,
    Failed,
    /// A later start began before this one's end was heard.
    Replaced,
}

/// The start under way, as its line will say it. Times are ms.
#[derive(Debug, Clone)]
struct Line {
    computer: String,
    generation: u64,
    woke: Option<Woke>,
    /// When the wake it is of was asked (its own beginning, when none
    /// was): what each step's time counts from.
    from_ms: i64,
    began_ms: i64,
    facts: Option<Facts>,
    up_ms: Option<i64>,
    data_ms: Option<i64>,
    ready_ms: Option<i64>,
    heard_ms: Option<i64>,
    turn_ms: Option<i64>,
    failing: Option<Failed>,
}

impl Line {
    /// Its line, plain JSON, at most `LINE_MAX_BYTES`.
    fn render(&self, end: End) -> String {
        let mut s = String::with_capacity(LINE_MAX_BYTES);
        let end_code = match end {
            End::Ready => "ready",
            End::Failed => "failed",
            End::Replaced => "replaced",
        };
        let _ = write!(s, r#"{{"start":{{"generation":{},"end":"{end_code}""#, self.generation);
        if let (End::Failed, Some(f)) = (end, self.failing) {
            let _ = write!(s, r#","failed":"{}""#, f.code());
        }
        let _ = write!(s, r#","woke":"{}""#, self.woke.map_or("itself", Woke::code));
        if let Some(f) = self.facts {
            let path = match f.path {
                RestoreSource::Snapshot => "snapshot",
                RestoreSource::Backup => "restore",
                RestoreSource::Nothing => "fresh",
            };
            let _ = write!(s, r#","path":"{path}""#);
            if let Some(why) = f.why {
                let _ = write!(s, r#","why":"{}""#, why.code());
            }
            if let Some(bytes) = f.bytes {
                let _ = write!(s, r#","bytes":{bytes}"#);
            }
            if let Some(new) = f.new_image {
                let _ = write!(s, r#","newImage":{new}"#);
            }
        }
        let steps = [("beganMs", Some(self.began_ms)), ("upMs", self.up_ms), ("dataMs", self.data_ms), ("readyMs", self.ready_ms), ("heardMs", self.heard_ms), ("turnMs", self.turn_ms)];
        for (key, at) in steps {
            if let Some(at) = at {
                let _ = write!(s, r#","{key}":{}"#, at.saturating_sub(self.from_ms).max(0));
            }
        }
        let _ = write!(s, r#"}},"computer":"{}"}}"#, self.computer);
        assert!(s.len() <= LINE_MAX_BYTES, "a start's line is bounded: {s}");
        s
    }
}

/// The id a line names: what an id is made of (`computer:` and hex), at
/// most `LINE_ID_MAX_BYTES`, so a line needs no escaping and stays bounded.
fn id_of(computer: &str) -> String {
    computer.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '-' | '_')).take(LINE_ID_MAX_BYTES).collect()
}

/// The Computer DO's start under way, and the wake asked that it answers.
#[derive(Debug, Default)]
pub struct Starts {
    line: Option<Line>,
    /// The wake asked that no start has answered yet (its first ask, and
    /// what asked).
    asked: Option<(i64, Woke)>,
}

impl Starts {
    /// A wake was asked at `now_ms`, its computer `phase` (read as it was
    /// let start): the clock of the start it brings. Nothing while a start
    /// is under way or up (it answers no ask of its own). While a start is
    /// owed or due (a sleep's end, a retry), the first ask stands; with
    /// none, an older ask is no longer waited on. An owner's restart is its
    /// own wake, whatever came before.
    pub fn asked(&mut self, woke: Woke, phase: &Phase, now_ms: i64) {
        let anew = woke == Woke::Restart || matches!(phase, Phase::Asleep { retry: None } | Phase::Failed { .. });
        let keeps = match phase {
            Phase::Starting { .. } => true,
            Phase::Awake { .. } => woke != Woke::Restart,
            _ => !anew && self.asked.is_some(),
        };
        if !keeps {
            self.asked = Some((now_ms, woke));
        }
    }

    /// The lifecycle is about to apply `event` to `life` at `now_ms`: an
    /// owner's restart it takes (`Lifecycle::restarts`) asks the start it
    /// brings, from then. One of an earlier start, made already, asks
    /// nothing, so it never stands in for another's ask.
    pub fn applying(&mut self, event: &Event, life: &Lifecycle, now_ms: i64) {
        if let Event::Restart { generation } = event {
            if life.restarts(*generation) {
                self.asked(Woke::Restart, &life.phase, now_ms);
            }
        }
    }

    /// Start `generation` of `computer` begins at `now_ms`: what the line
    /// of a start before it says, if one was under way still (replaced).
    pub fn begin(&mut self, computer: &str, generation: u64, now_ms: i64) -> Option<String> {
        let before = self.line.take().map(|l| l.render(if l.ready_ms.is_some() { End::Ready } else { End::Replaced }));
        let (from_ms, woke) = match self.asked {
            Some((at, woke)) if at <= now_ms => (at, Some(woke)),
            _ => (now_ms, None),
        };
        self.line = Some(Line { computer: id_of(computer), generation, woke, from_ms, began_ms: now_ms, facts: None, up_ms: None, data_ms: None, ready_ms: None, heard_ms: None, turn_ms: None, failing: None });
        before
    }

    fn of(&mut self, generation: u64) -> Option<&mut Line> {
        self.line.as_mut().filter(|l| l.generation == generation)
    }

    /// What start `generation` restores, as its plan chose.
    pub fn planned(&mut self, generation: u64, facts: Facts) {
        if let Some(l) = self.of(generation) {
            l.facts = Some(facts);
        }
    }

    /// Start `generation`'s container took its first exec.
    pub fn up(&mut self, generation: u64, now_ms: i64) {
        if let Some(l) = self.of(generation) {
            l.up_ms.get_or_insert(now_ms);
        }
    }

    /// Start `generation`'s `/data` is back: its save restored and checked,
    /// its gate open.
    pub fn data(&mut self, generation: u64, now_ms: i64) {
        if let Some(l) = self.of(generation) {
            l.data_ms.get_or_insert(now_ms);
        }
    }

    /// Start `generation` failed so: its line says it once the lifecycle
    /// has its report (`after`).
    pub fn failing(&mut self, generation: u64, why: Failed) {
        if let Some(l) = self.of(generation) {
            l.failing.get_or_insert(why);
        }
    }

    /// Its guest read its view at `now_ms`: the first read since the start
    /// under way began (its boot's, or its bridge's). Answers a line due.
    pub fn heard(&mut self, now_ms: i64) -> Option<String> {
        if let Some(l) = self.line.as_mut().filter(|l| now_ms >= l.began_ms) {
            l.heard_ms.get_or_insert(now_ms);
        }
        self.due(now_ms)
    }

    /// The lifecycle applied `event` at `now_ms`, and is `life` now: the
    /// start under way came up (it is awake on it), its guest opened a
    /// keepalive (a turn), or the lifecycle runs it no more (it ended).
    /// Answers its line, when that ends it.
    pub fn after(&mut self, event: &Event, life: &Lifecycle, now_ms: i64) -> Option<String> {
        // a wake that leaves it awake was answered by the container it has
        // (it cancelled an idle sleep): no start comes for its ask
        if matches!(event, Event::Wake { .. }) && matches!(life.phase, Phase::Awake { .. }) {
            self.asked = None;
        }
        let l = self.line.as_mut()?;
        if life.running() != Some(l.generation) {
            // ended: as it came up, or as it failed
            if l.ready_ms.is_none() {
                l.failing.get_or_insert(Failed::by(event));
            }
            return self.write();
        }
        if l.ready_ms.is_none() && matches!(life.phase, Phase::Awake { generation, .. } if generation == l.generation) {
            l.ready_ms = Some(now_ms);
            // the wake it answered is answered
            self.asked = None;
        }
        // a turn once the wait passed is not its wake's, though no event
        // came between to write its line
        if matches!(event, Event::Opened { socket: Socket::Keepalive }) && l.ready_ms.is_none_or(|ready| now_ms - ready < TURN_WAIT_MS) {
            l.turn_ms.get_or_insert(now_ms);
        }
        self.due(now_ms)
    }

    /// Its line, once its start came up and its guest took a turn or
    /// `TURN_WAIT_MS` passed (written at the first event or view read after).
    fn due(&mut self, now_ms: i64) -> Option<String> {
        let l = self.line.as_ref()?;
        match l.ready_ms {
            Some(ready) if l.turn_ms.is_some() || now_ms - ready >= TURN_WAIT_MS => self.write(),
            _ => None,
        }
    }

    fn write(&mut self) -> Option<String> {
        let l = self.line.take()?;
        Some(l.render(if l.ready_ms.is_some() { End::Ready } else { End::Failed }))
    }
}

#[cfg(test)]
mod tests {
    use fragment_proto::computer::{LifeEnd, RestoreSource};
    use serde_json::Value;

    use super::*;
    use crate::computer::testing::{rec, save_asked, Computer, IMAGE};
    use crate::computer::*;

    /// The lifecycle and its saves driven as the Computer DO drives them,
    /// with its starts' lines: each event's `after`, each start's steps as
    /// `start` makes them, the lines written.
    struct Driven {
        c: Computer,
        starts: Starts,
        lines: Vec<String>,
        /// The image its starts run: `None`, snapshots off.
        image: Option<&'static str>,
    }

    /// How a start the test makes ends.
    enum Outcome {
        Ready,
        /// Its steps failed so, reported as the cell's `start` reports it.
        Fails(Failed),
    }

    impl Driven {
        fn new() -> Driven {
            Driven { c: Computer::new(), starts: Starts::default(), lines: vec![], image: Some(IMAGE) }
        }

        fn apply(&mut self, e: Event) -> Step {
            // as the cell's `drive_events`: asked as it applies, at its time
            self.starts.applying(&e, &self.c.life, self.c.now + 1_000);
            let s = self.c.apply(e.clone());
            self.lines.extend(self.starts.after(&e, &self.c.life, self.c.now));
            s
        }

        /// A wake asked as the cell's `wake` asks it.
        fn wake(&mut self, why: Wake) -> Step {
            self.starts.asked(Woke::Wake(why), &self.c.life.phase, self.c.now);
            self.apply(Event::Wake { why })
        }

        /// The start `s` owes, made by the alarm it asks for at once.
        fn owed(&mut self, s: &Step) -> Step {
            assert_eq!(s.alarm_ms, Some(self.c.now), "an alarm at once: {s:?}");
            self.apply(Event::Alarm)
        }

        /// The start `s` asks for, made as the cell's `start` makes it: its
        /// generation, and the step its end's report answered.
        fn start(&mut self, s: &Step, outcome: Outcome) -> (u64, Step) {
            let Some(Action::Start { generation: g }) = s.actions.iter().find(|a| matches!(a, Action::Start { .. })).cloned() else { panic!("a start: {s:?}") };
            self.c.now += 100;
            self.lines.extend(self.starts.begin("computer:00ff00ff00ff00ff00ff00ff", g, self.c.now));
            let image = self.image;
            let plan = self.c.saves.plan(image);
            let why = self.c.saves.full_why(image, self.c.life.fell_back());
            let bytes = self.c.saves.current().filter(|_| plan != Plan::Nothing).map(|s| s.records.iter().filter_map(|r| r["size"].as_u64()).sum());
            let new_image = self.c.saves.last_image().map(|ran| Some(ran) != image);
            self.c.saves.starting(g, plan.source());
            self.starts.planned(g, Facts { path: plan.source(), why, bytes, new_image });
            if plan != Plan::Nothing {
                self.c.now += 800;
                self.starts.up(g, self.c.now);
            }
            if plan == Plan::Backup {
                self.c.now += 25_000;
                self.starts.data(g, self.c.now);
            }
            let end = match outcome {
                Outcome::Ready => {
                    self.c.now += 200;
                    // its guest reads its view as its entrypoint starts
                    self.lines.extend(self.starts.heard(self.c.now));
                    let save = self.c.saves.current().map(|c| c.id.clone()).filter(|_| plan != Plan::Nothing);
                    assert!(self.c.saves.came_up(g, plan.source(), save.as_deref(), image, self.c.now).is_some());
                    self.apply(Event::Ready { generation: g })
                }
                Outcome::Fails(why) => {
                    self.starts.failing(g, why);
                    let event = match why {
                        Failed::Snapshot | Failed::Dead => {
                            self.c.saves.snapshot_failed();
                            Event::SnapshotFailed { generation: g, why: "it would not start".into() }
                        }
                        Failed::Unusable => Event::RestoreFailed { generation: g, why: "its archive is gone".into(), older: false },
                        _ => Event::StartFailed { generation: g, why: "the container failed".into() },
                    };
                    self.apply(event)
                }
            };
            (g, end)
        }

        /// Its guest takes a turn: a keepalive opened, then closed.
        fn turn(&mut self) {
            self.apply(Event::Opened { socket: Socket::Keepalive });
            self.apply(Event::Closed { socket: Socket::Keepalive });
        }

        /// The save its turn's end asks for once it settles, kept as `id`.
        fn saved_awake(&mut self, id: &str) {
            let g = self.c.life.generation();
            self.c.now += SAVE_SETTLE_MS;
            assert_eq!(self.apply(Event::Alarm).actions.last(), Some(&Action::Hold { generation: g }));
            let (_, held, seq) = save_asked(&self.apply(Event::Held { generation: g, held: true }));
            self.c.saves.saved(g, sized(id), self.c.now, self.c.now, held);
            self.apply(Event::Saved { generation: g, seq });
        }

        /// Its owner's sleep, saved as `save`, its stop taking `snapshot`
        /// (none: that snapshot failed), as the cell's `stop` does.
        fn sleep(&mut self, save: &str, snapshot: Option<&str>) {
            let g = self.c.life.generation();
            self.apply(Event::Sleep);
            let (_, held, seq) = save_asked(&self.apply(Event::Held { generation: g, held: true }));
            self.c.saves.saved(g, sized(save), self.c.now, self.c.now, held);
            self.apply(Event::Saved { generation: g, seq });
            if let Some(id) = snapshot {
                let ran = self.c.saves.image_of(g).expect("the image it ran").to_string();
                assert!(self.c.saves.snapshotted(save, id, &ran));
            }
            self.apply(Event::Asleep { generation: g });
        }

        /// Start `g`'s line, parsed: exactly one was written.
        fn line(&self, g: u64) -> Value {
            let of: Vec<Value> = self.lines.iter().map(|l| serde_json::from_slice::<Value>(l.as_bytes()).expect("a line is JSON")).filter(|v| v["start"]["generation"] == g).collect();
            assert_eq!(of.len(), 1, "start {g}'s line, once: {:?}", self.lines);
            of[0]["start"].clone()
        }
    }

    /// A save's one record, its archive 4 MB.
    fn sized(id: &str) -> Vec<Value> {
        let mut records = rec(id);
        records[0]["size"] = Value::from(4_000_000u64);
        records
    }

    fn ms(l: &Value, key: &str) -> i64 {
        l[key].as_i64().unwrap_or_else(|| panic!("{key} in {l}"))
    }

    /// Goal (P3, FIN-474's plan): every start of a computer writes one line,
    /// once, saying its path: a fresh start, a snapshot wake, a whole
    /// restore and why, a start that failed, and the fallback a wake makes
    /// past a dead snapshot; ready once its guest takes a turn, or the wait
    /// for one passed. Valid (each path), invalid (a failed start, a dead
    /// snapshot), the bound (no turn: written once `TURN_WAIT_MS` passed,
    /// not before), replay (a turn after its line was written writes
    /// nothing more). Method: the lifecycle and its saves driven as the DO
    /// drives them (`Driven`), the lines it wrote parsed.
    #[test]
    fn every_start_writes_one_line_with_its_path() {
        let mut d = Driven::new();
        // a fresh start: no save yet; its line waits for its guest's turn
        let s = d.wake(Wake::Joined);
        let (g1, _) = d.start(&s, Outcome::Ready);
        assert!(d.lines.is_empty(), "ready, waiting for its guest's first turn: {:?}", d.lines);
        d.turn();
        let l = d.line(g1);
        assert_eq!((&l["end"], &l["path"], &l["woke"], l.get("why"), l.get("bytes"), l.get("newImage")), (&"ready".into(), &"fresh".into(), &"joined".into(), None, None, None), "{l}");
        assert!(ms(&l, "beganMs") < ms(&l, "readyMs") && ms(&l, "heardMs") < ms(&l, "readyMs") && ms(&l, "readyMs") < ms(&l, "turnMs"), "{l}");
        assert!(l.get("upMs").is_none() && l.get("dataMs").is_none() && l.get("failed").is_none(), "a fresh start waits for neither: {l}");
        // a snapshot wake, after a sleep that saved and snapshotted
        d.sleep("b1", Some("s1"));
        let s = d.wake(Wake::Record);
        let (g2, _) = d.start(&s, Outcome::Ready);
        d.turn();
        let l = d.line(g2);
        assert_eq!((&l["path"], l.get("why"), &l["bytes"], &l["newImage"], &l["woke"]), (&"snapshot".into(), None, &4_000_000.into(), &false.into(), &"record".into()), "{l}");
        assert!(l.get("dataMs").is_none() && ms(&l, "upMs") < ms(&l, "readyMs"), "/data came with the snapshot: {l}");
        // its turn saved awake, then a crash while a turn runs: a whole
        // restore the computer made itself (no wake asked); no turn comes,
        // so its line is written once the wait passed, and not before
        d.saved_awake("b1a");
        d.apply(Event::Opened { socket: Socket::Keepalive });
        let s = d.apply(Event::Exited { generation: g2 });
        let s = d.owed(&s);
        let (g3, _) = d.start(&s, Outcome::Ready);
        d.c.now += TURN_WAIT_MS - 2_000;
        d.apply(Event::Alarm);
        assert!(!d.lines.iter().any(|l| l.contains(&format!(r#""generation":{g3},"#))), "within its wait, no line yet: {:?}", d.lines);
        d.c.now += 2_000;
        d.apply(Event::Alarm);
        let l = d.line(g3);
        assert_eq!((&l["end"], &l["path"], &l["why"], &l["woke"], l.get("turnMs")), (&"ready".into(), &"restore".into(), &"crash".into(), &"itself".into(), None), "{l}");
        assert!(ms(&l, "beganMs") == 0 && ms(&l, "upMs") < ms(&l, "dataMs") && ms(&l, "dataMs") < ms(&l, "readyMs"), "{l}");
        // replay: its turn after the wait writes nothing more
        d.turn();
        let _ = d.line(g3);
        // a quiet computer: no event at all until a turn past the wait (Codex
        // on #1344): written then, without that turn
        let mut quiet = Driven::new();
        let s = quiet.wake(Wake::Presence);
        let (q1, _) = quiet.start(&s, Outcome::Ready);
        quiet.c.now += TURN_WAIT_MS + 14_000;
        quiet.apply(Event::Opened { socket: Socket::Keepalive });
        let l = quiet.line(q1);
        assert_eq!((&l["end"], l.get("turnMs")), (&"ready".into(), None), "{l}");
        // saved awake, its host stops it idle (a sleep, the lifecycle's,
        // no snapshot taken); then a start that fails, and its retry: a
        // line each, the retry's times from the wake that asked for both
        d.saved_awake("b2");
        let s = d.apply(Event::Exited { generation: g3 });
        assert!(!s.actions.iter().any(|a| matches!(a, Action::Start { .. })) && s.ended.is_some_and(|e| e.by == LifeEnd::Sleep && e.saved), "a host's stop of it idle and saved: {s:?}");
        let s = d.wake(Wake::Owner);
        let (g4, s) = d.start(&s, Outcome::Fails(Failed::Start));
        let l = d.line(g4);
        assert_eq!((&l["end"], &l["failed"], &l["path"], &l["why"], l.get("readyMs")), (&"failed".into(), &"start".into(), &"restore".into(), &"no_snapshot".into(), None), "{l}");
        let retry = s.alarm_ms.expect("its retry's alarm");
        d.c.now = retry;
        let s = d.apply(Event::Alarm);
        let (g5, _) = d.start(&s, Outcome::Ready);
        d.turn();
        let l = d.line(g5);
        assert_eq!((&l["end"], &l["woke"]), (&"ready".into(), &"owner".into()), "{l}");
        assert!(ms(&l, "beganMs") >= RETRY_PAUSE_MS, "counted from the ask the failed start answered first: {l}");
        // a snapshot that comes up dead: its line, then its fallback's, a
        // whole restore because of it, from the same ask
        d.sleep("b3", Some("s3"));
        let s = d.wake(Wake::Presence);
        let (g6, s) = d.start(&s, Outcome::Fails(Failed::Dead));
        let l = d.line(g6);
        assert_eq!((&l["end"], &l["failed"], &l["path"]), (&"failed".into(), &"dead".into(), &"snapshot".into()), "{l}");
        let s = d.owed(&s);
        let (g7, _) = d.start(&s, Outcome::Ready);
        d.turn();
        let l = d.line(g7);
        assert_eq!((&l["path"], &l["why"], &l["woke"]), (&"restore".into(), &"dead_snapshot".into(), &"presence".into()), "{l}");
        // once each, all seven
        assert_eq!(d.lines.len(), 7, "{:?}", d.lines);
        for g in 1..=7 {
            let _ = d.line(g);
        }
    }

    /// Goal: a whole restore says why, the first the DO's facts show, and
    /// agrees with `plan` (no why for a snapshot or a fresh start). Valid
    /// (each why), invalid (a plan that is no whole restore). Method: one
    /// computer's saves, each way a snapshot is not used.
    #[test]
    fn a_whole_restore_says_why() {
        const NEXT: &str = "registry/stub@sha256:2";
        let mut c = Computer::new();
        assert_eq!(c.saves.full_why(Some(IMAGE), false), None, "fresh: nothing to restore");
        c.wake();
        c.sleep(Some("b1"));
        assert!(c.saves.snapshotted("b1", "s1", IMAGE));
        assert_eq!(c.saves.full_why(Some(IMAGE), false), None, "the snapshot serves");
        assert_eq!(c.saves.full_why(None, false), Some(Full::Off));
        assert_eq!(c.saves.full_why(Some(NEXT), false), Some(Full::Image), "another image's start");
        let mut restarted = c.saves.clone();
        restarted.restart_fresh();
        assert_eq!(restarted.full_why(Some(IMAGE), false), Some(Full::Restart));
        let mut fell = c.saves.clone();
        fell.snapshot_failed();
        assert_eq!((fell.full_why(Some(IMAGE), true), fell.full_why(Some(IMAGE), false)), (Some(Full::DeadSnapshot), Some(Full::NoSnapshot)));
        // a newer save that would not restore: an older one, whole
        c.wake();
        c.turn_saved("b2");
        assert!(c.saves.unusable("b2"));
        assert_eq!(c.saves.full_why(Some(IMAGE), false), Some(Full::Unusable));
        // a sleep that could not save
        let mut c = Computer::new();
        c.wake();
        c.sleep(Some("b1"));
        c.wake();
        c.sleep(None);
        assert_eq!(c.saves.full_why(Some(IMAGE), false), Some(Full::Unsaved));
        // a crash after an awake save, a turn running (idle, its host's
        // stop of it is a sleep: no snapshot)
        c.wake();
        c.turn_saved("b2");
        let mut idle = c.saves.clone();
        idle.ended(Ended { generation: c.life.generation(), by: LifeEnd::Sleep, saved: true, at_ms: c.now, restart: false, quiet: true });
        assert_eq!(idle.full_why(Some(IMAGE), false), Some(Full::NoSnapshot));
        c.apply(Event::Opened { socket: Socket::Keepalive });
        c.crash();
        assert_eq!(c.saves.full_why(Some(IMAGE), false), Some(Full::Crash));
    }

    /// Goal: a line is plain JSON with a stable prefix and field names,
    /// bounded, naming the computer by its id alone, whatever that id holds.
    /// Valid (every field), invalid (an id with quotes and too long), the
    /// bound (`LINE_MAX_BYTES`). Method: a start with every step, rendered.
    #[test]
    fn a_line_is_plain_bounded_json() {
        let mut starts = Starts::default();
        let id = format!("computer:{}\"}}}},\"x\":\"{}", "f".repeat(24), "9".repeat(200));
        starts.asked(Woke::Wake(Wake::Record), &Phase::ASLEEP, 1_000);
        assert_eq!(starts.begin(&id, u64::MAX, 1_100), None);
        starts.planned(u64::MAX, Facts { path: RestoreSource::Backup, why: Some(Full::DeadSnapshot), bytes: Some(u64::MAX), new_image: Some(true) });
        starts.up(u64::MAX, 2_000);
        starts.data(u64::MAX, i64::MAX);
        starts.failing(u64::MAX, Failed::Deadline);
        let mut life = Lifecycle::new();
        life.apply(Event::Wake { why: Wake::Record }, 900);
        let line = starts.after(&Event::Alarm, &life, i64::MAX).expect("its lifecycle runs another start: it ended");
        assert!(line.len() <= LINE_MAX_BYTES && line.starts_with(r#"{"start":{"generation":"#), "{line}");
        let v: Value = serde_json::from_slice(line.as_bytes()).expect("JSON");
        let keys: Vec<&str> = v["start"].as_object().expect("its start").keys().map(String::as_str).collect();
        let mut want = vec!["generation", "end", "failed", "woke", "path", "why", "bytes", "newImage", "beganMs", "upMs", "dataMs"];
        want.sort_unstable();
        assert_eq!(keys, want, "{line}");
        assert_eq!((&v["start"]["end"], &v["start"]["failed"], &v["start"]["beganMs"], &v["start"]["upMs"]), (&"failed".into(), &"deadline".into(), &100.into(), &1_000.into()), "{line}");
        let named = v["computer"].as_str().expect("its id");
        assert!(named.len() == LINE_ID_MAX_BYTES && named.starts_with("computer:ffff") && v.as_object().map(|o| o.len()) == Some(2), "{line}");
    }

    /// Goal: a start counts from the wake that asked for it: one asked
    /// during a sleep counts the rest of that sleep; one asked while it is
    /// up asks nothing; an older ask no start answered (it won't wake) is
    /// not counted, and a restart is its own; a start begun while another's
    /// line is under way ends that one as replaced. Method: `Starts` alone.
    /// Goal (#1380): a start held up by the container before it says so,
    /// `blocked`, whether its own step said it or only the lifecycle's
    /// report did, and the start made again after it counts from the same
    /// wake, so the line that comes up shows the whole wait. Valid (held
    /// up, then up), replay (its report heard twice writes one line).
    /// Method: `Starts` beside the lifecycle, as the DO drives them.
    #[test]
    fn a_held_up_start_says_blocked_and_the_next_counts_from_its_wake() {
        let mut starts = Starts::default();
        let mut life = Lifecycle::new();
        starts.asked(Woke::Wake(Wake::Owner), &life.phase, 10_000);
        life.apply(Event::Wake { why: Wake::Owner }, 10_000);
        assert!(starts.begin("computer:1", 1, 10_000).is_none());
        // its own step names it (the DO's `start`), and its report ends it
        starts.failing(1, Failed::Blocked);
        let blocked = Event::StartBlocked { generation: 1 };
        life.apply(blocked.clone(), 25_000);
        let line = starts.after(&blocked, &life, 25_000).expect("its line, as it ends");
        assert!(line.contains(r#""generation":1,"end":"failed","failed":"blocked","woke":"owner","beganMs":0}"#), "{line}");
        assert!(starts.after(&blocked, &life, 25_001).is_none(), "one line");
        // the lifecycle's report alone names it too
        let mut alone = Starts::default();
        let mut l = Lifecycle::new();
        alone.asked(Woke::Wake(Wake::Record), &l.phase, 0);
        l.apply(Event::Wake { why: Wake::Record }, 0);
        alone.begin("computer:1", 1, 0);
        l.apply(Event::StartBlocked { generation: 1 }, 15_000);
        assert!(alone.after(&Event::StartBlocked { generation: 1 }, &l, 15_000).is_some_and(|line| line.contains(r#""failed":"blocked""#)));
        // made again: its line counts from the owner's wake, 15 s before
        life.apply(Event::Alarm, 25_000);
        assert!(starts.begin("computer:1", 2, 25_000).is_none());
        life.apply(Event::Ready { generation: 2 }, 28_000);
        assert!(starts.after(&Event::Ready { generation: 2 }, &life, 28_000).is_none(), "up, its line waits for a turn");
        assert_eq!(starts.line.as_ref().map(|l| (l.from_ms, l.began_ms)), Some((10_000, 25_000)), "from the wake, not its own beginning");
    }

    #[test]
    fn a_start_counts_from_the_wake_that_asked() {
        let sleeping = Phase::Sleeping { generation: 1, since_ms: 0, owner: false, restarting: false, owes_start: false };
        let mut starts = Starts::default();
        starts.asked(Woke::Wake(Wake::Record), &sleeping, 10_000);
        starts.asked(Woke::Wake(Wake::Presence), &sleeping, 20_000);
        starts.begin("computer:1", 2, 70_000);
        let mut life = Lifecycle::new();
        life.apply(Event::Wake { why: Wake::Record }, 69_000);
        life.apply(Event::Wake { why: Wake::Record }, 69_000);
        let replaced = starts.begin("computer:1", 3, 75_000).expect("start 2's line, replaced");
        assert!(replaced.contains(r#""generation":2,"end":"replaced","woke":"record","beganMs":60000}"#), "the first ask, the rest of the sleep counted: {replaced}");
        // up: asks nothing; the start under way keeps its clock
        starts.asked(Woke::Wake(Wake::Owner), &Phase::Awake { generation: 3, since_ms: 76_000 }, 80_000);
        assert_eq!(starts.asked, Some((10_000, Woke::Wake(Wake::Record))));
        // won't wake, then a new ask: the old one no longer counts
        starts.asked(Woke::Wake(Wake::Owner), &Phase::Failed { why: "x".into(), since_ms: 0 }, 90_000);
        assert_eq!(starts.asked, Some((90_000, Woke::Wake(Wake::Owner))));
        starts.asked(Woke::Restart, &Phase::Asleep { retry: Some(Retry { at_ms: 95_000, owed: true }) }, 91_000);
        assert_eq!(starts.asked, Some((91_000, Woke::Restart)), "a restart is its own wake");
        starts.asked(Woke::Wake(Wake::Record), &Phase::Asleep { retry: Some(Retry { at_ms: 95_000, owed: true }) }, 92_000);
        assert_eq!(starts.asked, Some((91_000, Woke::Restart)), "a start owed: the first ask stands");
    }

    /// Goal (#1357 with #1344): a wake during an idle sleep's hold cancels
    /// the sleep and starts nothing, so the container it has answers its
    /// ask: the start after (its container stopping under a turn an hour
    /// on) is no wake's, timed from its own beginning. Method: the
    /// lifecycle driven as the DO drives it, the wake asked as the cell
    /// asks it, the lines parsed.
    #[test]
    fn a_wake_that_cancels_a_sleep_is_answered_by_it() {
        let mut d = Driven::new();
        let s = d.wake(Wake::Owner);
        let (g1, _) = d.start(&s, Outcome::Ready);
        d.turn();
        let _ = d.line(g1);
        // its hold runs out: its idle sleep, at its hold
        d.c.now = d.c.life.held_until_ms() - 1_000;
        let s = d.apply(Event::Alarm);
        assert_eq!(s.actions.last(), Some(&Action::Sleep { generation: g1 }), "{s:?}");
        // a message's wake cancels it: awake on its container, no start
        let s = d.wake(Wake::Record);
        assert!(!s.actions.iter().any(|a| matches!(a, Action::Start { .. })) && matches!(d.c.life.phase, Phase::Awake { .. }), "{s:?}");
        // an hour on, its container stops under a turn: a start of its own
        d.c.now += 3_600_000;
        d.apply(Event::Opened { socket: Socket::Keepalive });
        let s = d.apply(Event::Exited { generation: g1 });
        let s = d.owed(&s);
        let (g2, _) = d.start(&s, Outcome::Ready);
        d.turn();
        let l = d.line(g2);
        assert_eq!((&l["woke"], ms(&l, "beganMs")), (&"itself".into(), 0), "{l}");
        // an owner's restart, a wake landing before its event (its drive
        // waiting on the wake's takeover: #1357's review): it asks its start
        // as it applies, so that start is the restart's
        d.wake(Wake::Owner);
        d.apply(Event::Restart { generation: 0 });
        let g = d.c.life.generation();
        let (_, _, seq) = save_asked(&d.apply(Event::Held { generation: g, held: true }));
        d.apply(Event::Saved { generation: g, seq });
        let s = d.apply(Event::Asleep { generation: g });
        let s = d.owed(&s);
        let (g3, _) = d.start(&s, Outcome::Ready);
        d.turn();
        let l = d.line(g3);
        assert!(l["woke"] == "restart" && ms(&l, "beganMs") > 0, "{l}");
        // a duplicate of that restart, applied once its start came up (its
        // ledger's answer slow: #1357's review): it names an earlier start,
        // made already, so it asks nothing; a wake, then its container
        // stopping under a turn: that start is no restart's
        d.apply(Event::Restart { generation: g });
        assert!(matches!(d.c.life.phase, Phase::Awake { .. }) && d.starts.asked.is_none(), "{:?}", d.starts.asked);
        d.wake(Wake::Owner);
        d.apply(Event::Opened { socket: Socket::Keepalive });
        let s = d.apply(Event::Exited { generation: g3 });
        let s = d.owed(&s);
        let (g4, _) = d.start(&s, Outcome::Ready);
        d.turn();
        let l = d.line(g4);
        assert_eq!((&l["woke"], ms(&l, "beganMs")), (&"itself".into(), 0), "{l}");
        // a stale duplicate applied just before a valid restart (each
        // request's drive in turn: #1357's review): the stale one asks
        // nothing, so the valid one's start is the restart's
        d.apply(Event::Restart { generation: g3 });
        d.apply(Event::Restart { generation: 0 });
        let g = d.c.life.generation();
        let (_, _, seq) = save_asked(&d.apply(Event::Held { generation: g, held: true }));
        d.apply(Event::Saved { generation: g, seq });
        let s = d.apply(Event::Asleep { generation: g });
        let s = d.owed(&s);
        let (g5, _) = d.start(&s, Outcome::Ready);
        d.turn();
        let l = d.line(g5);
        assert!(l["woke"] == "restart" && ms(&l, "beganMs") > 0, "{l}");
    }
}
