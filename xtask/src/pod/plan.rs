//! A plan: the commands a `cargo xtask pod` command runs on this machine,
//! in order, each with what it is for. `--dry-run` prints it (`render`)
//! and runs nothing; a run hands each command to a `Runner`, the real one
//! a process, a test's a stub that records what it was asked and answers
//! as the vendor's tool would. Every command a plan holds is one this
//! machine starts: a build host's are `sh -c` here or `gcloud compute ssh`
//! to it (host.rs), so the printed plan is exactly what runs.

use std::fmt::Write as _;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{bail, Context, Result};

/// A command this machine runs, by its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cmd {
    pub argv: Vec<String>,
    pub stdin: Stdin,
    pub stdout: Stdout,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stdin {
    Null,
    /// A local file's bytes.
    File(PathBuf),
    /// These bytes, written by the plan (a small file it makes on a host).
    Bytes(Vec<u8>),
    /// Another command's output, run here and piped in (`git archive`).
    Pipe(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stdout {
    /// To this terminal.
    Inherit,
    /// Into a local file, made or replaced.
    File(PathBuf),
    /// Thrown away.
    Discard,
}

impl Cmd {
    pub fn new<S: Into<String>>(argv: impl IntoIterator<Item = S>) -> Self {
        Self { argv: argv.into_iter().map(Into::into).collect(), stdin: Stdin::Null, stdout: Stdout::Inherit }
    }

    pub fn stdin(self, stdin: Stdin) -> Self {
        Self { stdin, ..self }
    }

    pub fn stdout(self, stdout: Stdout) -> Self {
        Self { stdout, ..self }
    }

    /// As a shell would read it: each argument quoted where it must be,
    /// with its input and output.
    pub fn shown(&self) -> String {
        let line = words(&self.argv);
        let line = match &self.stdin {
            Stdin::Null => line,
            Stdin::File(p) => format!("{line} < {}", quote(&p.to_string_lossy())),
            Stdin::Bytes(b) => format!("{line} <<< {}", quote(String::from_utf8_lossy(b).trim_end_matches('\n'))),
            Stdin::Pipe(argv) => format!("{} | {line}", words(argv)),
        };
        match &self.stdout {
            Stdout::Inherit => line,
            Stdout::File(p) => format!("{line} > {}", quote(&p.to_string_lossy())),
            Stdout::Discard => format!("{line} > /dev/null"),
        }
    }
}

/// Arguments joined as a shell reads them.
pub fn words(argv: &[String]) -> String {
    argv.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ")
}

/// One word for a POSIX shell: as it is when it holds nothing the shell
/// reads specially, else in single quotes.
pub fn quote(word: &str) -> String {
    // `~` expands only at a word's start
    let plain = |c: char| c.is_ascii_alphanumeric() || "_@%+=:,./-~".contains(c);
    if !word.is_empty() && !word.starts_with('~') && word.chars().all(plain) {
        return word.to_string();
    }
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// Whether a step runs, decided by asking first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guard {
    pub probe: Probe,
    /// The step runs when the probe finds its thing (true) or does not
    /// (false).
    pub run_if_found: bool,
    pub otherwise: Otherwise,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// Found when the command succeeds; not found when it fails.
    Succeeds(Cmd),
    /// Found when the command's output lists this name (as a whole word:
    /// lines, `;` and `,` separate them). The command failing is an error.
    Lists(Cmd, String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Otherwise {
    /// Leave this step out and go on.
    Skip(String),
    /// The plan has nothing more to do: stop, successfully.
    Done(String),
    /// Refuse: stop with this error, having run nothing more.
    Refuse(String),
}

/// What a step's output must say for it to have worked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    /// Its first word is this (a hash before its file name, an id).
    FirstWord(String),
    /// It has exactly this many lines.
    Lines(usize),
}

impl Check {
    fn holds(&self, out: &str) -> bool {
        match self {
            Self::FirstWord(w) => out.split_whitespace().next() == Some(w.as_str()),
            Self::Lines(n) => out.lines().filter(|l| !l.trim().is_empty()).count() == *n,
        }
    }

    fn shown(&self) -> String {
        match self {
            Self::FirstWord(w) => format!("its output must begin {w}"),
            Self::Lines(n) => format!("its output must be {n} line{}", if *n == 1 { "" } else { "s" }),
        }
    }
}

/// Run again until it works, up to `tries` times, `pause` apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retry {
    pub tries: u32,
    pub pause: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub what: String,
    pub cmd: Cmd,
    pub guard: Option<Guard>,
    pub check: Option<Check>,
    pub retry: Option<Retry>,
    /// A failure is expected and fine (its effect is checked after).
    pub may_fail: bool,
    /// Run when it fails, to show why (a log's tail).
    pub on_failure: Option<Cmd>,
}

impl Step {
    pub fn new(what: impl Into<String>, cmd: Cmd) -> Self {
        Self { what: what.into(), cmd, guard: None, check: None, retry: None, may_fail: false, on_failure: None }
    }

    /// A step that runs nothing: its guard alone decides whether the plan
    /// goes on (an image that already exists ends a publish).
    pub fn gate(what: impl Into<String>, probe: Probe, run_if_found: bool, otherwise: Otherwise) -> Self {
        Self::new(what, Cmd::new(Vec::<String>::new())).guard(Guard { probe, run_if_found, otherwise })
    }

    pub fn is_gate(&self) -> bool {
        self.cmd.argv.is_empty()
    }

    pub fn guard(self, guard: Guard) -> Self {
        Self { guard: Some(guard), ..self }
    }

    pub fn check(self, check: Check) -> Self {
        Self { check: Some(check), ..self }
    }

    pub fn retry(self, tries: u32, pause: Duration) -> Self {
        Self { retry: Some(Retry { tries, pause }), ..self }
    }

    pub fn may_fail(self) -> Self {
        Self { may_fail: true, ..self }
    }

    pub fn on_failure(self, cmd: Cmd) -> Self {
        Self { on_failure: Some(cmd), ..self }
    }

    /// Runs only when the probe does not find its thing.
    pub fn unless(self, probe: Probe, otherwise: Otherwise) -> Self {
        self.guard(Guard { probe, run_if_found: false, otherwise })
    }

    /// Runs only when the probe finds its thing.
    pub fn only_if(self, probe: Probe, otherwise: Otherwise) -> Self {
        self.guard(Guard { probe, run_if_found: true, otherwise })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub title: String,
    /// Said before the first step, in a run and a dry run alike (what a
    /// delete deletes, what it keeps).
    pub notes: Vec<String>,
    pub steps: Vec<Step>,
}

/// Most steps a plan may hold: every plan here is a few dozen.
pub const MAX_STEPS: usize = 200;

impl Plan {
    pub fn new(title: impl Into<String>) -> Self {
        Self { title: title.into(), notes: Vec::new(), steps: Vec::new() }
    }

    pub fn note(&mut self, note: impl Into<String>) {
        self.notes.push(note.into());
    }

    pub fn push(&mut self, step: Step) {
        self.steps.push(step);
    }

    /// The plan as `--dry-run` prints it: every command, in order.
    pub fn render(&self) -> String {
        let mut out = format!("{}\n", self.title);
        for n in &self.notes {
            let _ = writeln!(out, "  {n}");
        }
        for (i, s) in self.steps.iter().enumerate() {
            let _ = writeln!(out, "{}. {}", i + 1, s.what);
            if let Some(g) = &s.guard {
                let (probe, found) = match &g.probe {
                    Probe::Succeeds(c) => (c.shown(), "it succeeds".to_string()),
                    Probe::Lists(c, name) => (c.shown(), format!("it lists {name}")),
                };
                let when = if g.run_if_found { "unless" } else { "if" };
                let then = match &g.otherwise {
                    Otherwise::Skip(why) => format!("skip this step ({why})"),
                    Otherwise::Done(why) => format!("stop here, done ({why})"),
                    Otherwise::Refuse(why) => format!("refuse ({why})"),
                };
                let _ = writeln!(out, "   first: {probe}\n   {when} {found}: {then}");
            }
            if !s.is_gate() {
                let _ = writeln!(out, "   $ {}", s.cmd.shown());
            }
            if let Some(c) = &s.check {
                let _ = writeln!(out, "   {}", c.shown());
            }
            if let Some(r) = &s.retry {
                let _ = writeln!(out, "   again until it succeeds: up to {} times, {:?} apart", r.tries, r.pause);
            }
            if s.may_fail {
                let _ = writeln!(out, "   a failure here is expected; what it made is checked next");
            }
        }
        out
    }

    /// Runs each step in order with `runner`, stopping at the first that
    /// fails, a guard that refuses, or one that says the plan is done.
    pub fn run(&self, runner: &mut dyn Runner) -> Result<Ran> {
        anyhow::ensure!(self.steps.len() <= MAX_STEPS, "a plan of {} steps is over {MAX_STEPS}", self.steps.len());
        eprintln!("{}", self.title);
        for n in &self.notes {
            eprintln!("  {n}");
        }
        for (i, s) in self.steps.iter().enumerate() {
            eprintln!("{}. {}", i + 1, s.what);
            if let Some(g) = &s.guard {
                let found = match &g.probe {
                    Probe::Succeeds(c) => runner.run(c, true)?.ok,
                    Probe::Lists(c, name) => {
                        let out = runner.run(c, true)?;
                        anyhow::ensure!(out.ok, "{} failed", c.shown());
                        lists(&out.stdout, name)
                    }
                };
                if found != g.run_if_found {
                    match &g.otherwise {
                        Otherwise::Skip(why) => {
                            eprintln!("   skipped: {why}");
                            continue;
                        }
                        Otherwise::Done(why) => {
                            eprintln!("   done: {why}");
                            return Ok(Ran::Done { at: i + 1 });
                        }
                        Otherwise::Refuse(why) => bail!("step {}: {why}", i + 1),
                    }
                }
            }
            if !s.is_gate() {
                run_step(s, runner).with_context(|| format!("step {}: {}", i + 1, s.what))?;
            }
        }
        Ok(Ran::All)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ran {
    All,
    /// A guard ended the plan, successfully, before this step.
    Done { at: usize },
}

fn run_step(s: &Step, runner: &mut dyn Runner) -> Result<()> {
    let tries = s.retry.map_or(1, |r| r.tries.max(1));
    for attempt in 1..=tries {
        let out = runner.run(&s.cmd, s.check.is_some())?;
        let checked = s.check.as_ref().is_none_or(|c| c.holds(&out.stdout));
        if out.ok && checked {
            return Ok(());
        }
        if out.ok && !checked && s.retry.is_none() {
            let check = s.check.as_ref().expect("a check").shown();
            bail!("{} said {:?}: {check}", s.cmd.shown(), out.stdout.trim());
        }
        if attempt < tries {
            runner.pause(s.retry.expect("tries > 1 only with a retry").pause);
        }
    }
    if s.may_fail {
        eprintln!("   failed, as it may");
        return Ok(());
    }
    if let Some(why) = &s.on_failure {
        let _ = runner.run(why, false);
    }
    bail!("{} failed", s.cmd.shown())
}

/// Whether a list command's output names `name` as a whole entry.
pub fn lists(out: &str, name: &str) -> bool {
    out.split(['\n', ';', ',']).map(str::trim).any(|w| w == name)
}

/// What a command did: whether it succeeded, and what it said when its
/// output was asked for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Out {
    pub ok: bool,
    pub stdout: String,
}

/// Runs a plan's commands: a process here, a stub in tests.
pub trait Runner {
    /// Runs `cmd`; with `capture`, its output comes back rather than going
    /// where `cmd` says.
    fn run(&mut self, cmd: &Cmd, capture: bool) -> Result<Out>;
    fn pause(&mut self, d: Duration);
}

/// The real runner: each command a process on this machine.
pub struct Processes;

/// Most output a captured command may give: lists and hashes are small.
const MAX_CAPTURE: u64 = 1 << 20;

impl Runner for Processes {
    fn run(&mut self, cmd: &Cmd, capture: bool) -> Result<Out> {
        let (program, args) = cmd.argv.split_first().context("an empty command")?;
        let mut c = Command::new(program);
        c.args(args);
        let mut feeder = None;
        match &cmd.stdin {
            Stdin::Null => {
                c.stdin(Stdio::null());
            }
            Stdin::File(p) => {
                c.stdin(std::fs::File::open(p).with_context(|| format!("open {}", p.display()))?);
            }
            Stdin::Bytes(_) => {
                c.stdin(Stdio::piped());
            }
            Stdin::Pipe(argv) => {
                let (p, a) = argv.split_first().context("an empty pipe")?;
                let mut producer = Command::new(p).args(a).stdin(Stdio::null()).stdout(Stdio::piped()).spawn().with_context(|| format!("start {}", words(argv)))?;
                c.stdin(producer.stdout.take().expect("piped"));
                feeder = Some(producer);
            }
        }
        if capture {
            c.stdout(Stdio::piped());
        } else {
            match &cmd.stdout {
                Stdout::Inherit => {
                    c.stdout(Stdio::inherit());
                }
                Stdout::File(p) => {
                    c.stdout(std::fs::File::create(p).with_context(|| format!("create {}", p.display()))?);
                }
                Stdout::Discard => {
                    c.stdout(Stdio::null());
                }
            }
        }
        let mut child = c.spawn().with_context(|| format!("start {}", words(&cmd.argv)))?;
        if let Stdin::Bytes(b) = &cmd.stdin {
            child.stdin.take().expect("piped").write_all(b).context("write its input")?;
        }
        let mut stdout = String::new();
        if capture {
            child.stdout.take().expect("piped").take(MAX_CAPTURE).read_to_string(&mut stdout).context("read its output")?;
        }
        let status = child.wait()?;
        let fed = match feeder {
            Some(mut f) => f.wait()?.success(),
            None => true,
        };
        Ok(Out { ok: status.success() && fed, stdout })
    }

    fn pause(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
}

#[cfg(test)]
pub mod stub {
    //! A stand-in for the vendors' tools: it records each command it is
    //! asked to run, as a shell would read it, and answers from a script
    //! of (a part of the command, what it says, whether it succeeds).

    use super::*;

    #[derive(Default)]
    pub struct Stub {
        pub ran: Vec<String>,
        answers: Vec<(String, Out)>,
        pub paused: u32,
    }

    impl Stub {
        /// A command whose shown form contains `part` says `stdout` and
        /// succeeds (`ok`) or fails. Later answers win.
        pub fn answer(mut self, part: &str, stdout: &str, ok: bool) -> Self {
            self.answers.push((part.to_string(), Out { ok, stdout: stdout.to_string() }));
            self
        }
    }

    impl Runner for Stub {
        fn run(&mut self, cmd: &Cmd, _capture: bool) -> Result<Out> {
            let shown = cmd.shown();
            self.ran.push(shown.clone());
            let answer = self.answers.iter().rev().find(|(part, _)| shown.contains(part.as_str()));
            Ok(answer.map_or(Out { ok: true, stdout: String::new() }, |(_, o)| o.clone()))
        }

        fn pause(&mut self, _d: Duration) {
            self.paused += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::stub::Stub;
    use super::*;

    #[test]
    fn quotes_what_a_shell_would_read() {
        assert_eq!(quote("--zone=us-central1-f"), "--zone=us-central1-f");
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote(""), "''");
        assert_eq!(quote("$HOME"), "'$HOME'");
        assert_eq!(quote("containerd.io=2.4.2-3~ubuntu.24.04~noble"), "containerd.io=2.4.2-3~ubuntu.24.04~noble");
        assert_eq!(quote("~/x"), "'~/x'");
        let c = Cmd::new(["tar", "-x"]).stdin(Stdin::Pipe(vec!["git".into(), "archive".into(), "HEAD".into()]));
        assert_eq!(c.shown(), "git archive HEAD | tar -x");
    }

    #[test]
    fn lists_matches_whole_entries() {
        assert!(lists("pod-1\npod-2\n", "pod-2"));
        assert!(lists("boot;pod-data", "pod-data"));
        assert!(!lists("pod-10\n", "pod-1"));
        assert!(!lists("", "pod-1"));
    }

    fn plan() -> Plan {
        let mut p = Plan::new("test");
        p.push(Step::new("make it", Cmd::new(["make", "x"])).unless(Probe::Lists(Cmd::new(["list"]), "x".into()), Otherwise::Skip("there".into())));
        p.push(Step::new("hash it", Cmd::new(["sha256sum", "x"])).check(Check::FirstWord("ab".into())));
        p
    }

    /// Valid: each step runs once, the guard's probe first. Replay: the
    /// thing is there now, so the make is skipped and the rest runs.
    #[test]
    fn runs_in_order_and_a_replay_skips_what_is_done() {
        let mut s = Stub::default().answer("sha256sum", "ab  x\n", true);
        assert_eq!(plan().run(&mut s).unwrap(), Ran::All);
        assert_eq!(s.ran, ["list", "make x", "sha256sum x"]);
        let mut s = Stub::default().answer("list", "x\n", true).answer("sha256sum", "ab  x", true);
        plan().run(&mut s).unwrap();
        assert_eq!(s.ran, ["list", "sha256sum x"]);
    }

    /// Invalid: a check that does not hold, a failing step and a failing
    /// list each stop the plan there.
    #[test]
    fn stops_at_a_failed_check_or_command() {
        let mut s = Stub::default().answer("sha256sum", "cd  x", true);
        assert!(format!("{:#}", plan().run(&mut s).unwrap_err()).contains("must begin ab"));
        let mut s = Stub::default().answer("make", "", false);
        assert!(plan().run(&mut s).is_err());
        assert_eq!(s.ran, ["list", "make x"]);
        let mut s = Stub::default().answer("list", "", false);
        assert!(plan().run(&mut s).is_err());
        assert_eq!(s.ran, ["list"]);
    }

    #[test]
    fn refuses_or_finishes_early_by_its_guard() {
        let mut p = Plan::new("t");
        p.push(Step::new("a", Cmd::new(["a"])).unless(Probe::Succeeds(Cmd::new(["has"])), Otherwise::Refuse("it exists".into())));
        p.push(Step::new("b", Cmd::new(["b"])));
        let mut s = Stub::default();
        let e = p.run(&mut s).unwrap_err();
        assert!(e.to_string().contains("it exists"), "{e}");
        assert_eq!(s.ran, ["has"]);
        p.steps[0].guard.as_mut().unwrap().otherwise = Otherwise::Done("nothing to do".into());
        let mut s = Stub::default();
        assert_eq!(p.run(&mut s).unwrap(), Ran::Done { at: 1 });
        assert_eq!(s.ran, ["has"]);
    }

    #[test]
    fn retries_then_shows_why_it_failed() {
        let mut p = Plan::new("t");
        p.push(Step::new("ready", Cmd::new(["grep", "ready", "log"])).retry(3, Duration::from_secs(5)).on_failure(Cmd::new(["tail", "log"])));
        let mut s = Stub::default().answer("grep", "", false);
        assert!(p.run(&mut s).is_err());
        assert_eq!(s.ran, ["grep ready log", "grep ready log", "grep ready log", "tail log"]);
        assert_eq!(s.paused, 2);
        p.steps[0].may_fail = true;
        let mut s = Stub::default().answer("grep", "", false);
        assert!(p.run(&mut s).is_ok());
    }

    /// The real runner: a command's output captured, a pipe between two,
    /// bytes in, and a failure seen.
    #[test]
    fn the_process_runner_pipes_and_captures() {
        let mut r = Processes;
        let out = r.run(&Cmd::new(["tr", "a-z", "A-Z"]).stdin(Stdin::Pipe(vec!["printf".into(), "abc".into()])), true).unwrap();
        assert_eq!(out, Out { ok: true, stdout: "ABC".into() });
        let out = r.run(&Cmd::new(["cat"]).stdin(Stdin::Bytes(b"xyz".to_vec())), true).unwrap();
        assert_eq!(out.stdout, "xyz");
        assert!(!r.run(&Cmd::new(["false"]), true).unwrap().ok);
    }
}
