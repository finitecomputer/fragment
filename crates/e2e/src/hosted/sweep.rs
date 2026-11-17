//! The hosted lane's sweep: what a run made on a preview, removed.
//!
//! A preview is shared: several sessions run the hosted lane on it at
//! once, so a sweep removes one run's fragments unless told otherwise. A
//! hosted run labels its fragments `e2e-<run>-<base>` (`label`): its id
//! (`RUN_HEX` hex digits: the clock as it starts, then a random nonce)
//! right after the prefix, so a label a lane extends (`<agent label>-chat`)
//! still names its run. A sweep signs e2e people in again (a session that
//! leaves the paid calls they were lent as they are: a run beside it may be
//! spending them) and acts as them: it has no power of its own.
//!
//! **Whom it signs in** (`Who`). A run notes the emails its people signed
//! in by as it ends (`note_run_people`), so its sweep signs in those alone,
//! a handful, rather than every e2e person on the preview (hundreds, each
//! sign-in a registry call and two to their ledger: FIN-362). Nothing
//! smaller is open to it: a preview's levers list no fragments by label
//! (the registry's lever is no route there), so a fragment is found as its
//! owner, signed in. A run not noted here (another checkout's) and
//! `--sweep-all` walk every e2e person (the levers' list).
//!
//! **A failure is skipped and said.** A person whose sign-in, list or
//! computers did not answer, a fragment that did not go, a computer that
//! did not sleep: each is noted (`Swept::left`) and the sweep goes on;
//! `left` decides its exit, after it has said what it did.
//!
//! - **`Scope::Run`** (`--sweep <run>`; `--sweep`, the last run that
//!   finished here; a rehearsal's end) deletes that run's fragments,
//!   whatever their age, and no other. It puts a person's computer to sleep
//!   when it deleted one of theirs and spared none of the e2e's: e2e people
//!   are mostly made fresh by a run, but a few sign in by a fixed email in
//!   every run, and another run may be using their computer.
//! - **`Scope::All`** (`--sweep-all`, for when nothing else runs there)
//!   deletes every e2e fragment but those younger than `SPARED_FOR_MS` (a
//!   run started meanwhile may be using them) and those whose age does not
//!   answer. It puts a person's computer to sleep unless it spared one of
//!   their fragments.
//!
//! Either says what it kept, and why (`Swept::report`).

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use fragment_core::levers::E2E_LABEL_PREFIX;
use fragment_devstack as devstack;
use serde_json::json;

use crate::api::{Api, Call, Preview, Reply};

/// A run's id starts with this many hex digits of its clock, so ids sort
/// by when their runs started.
const CLOCK_HEX: usize = 6;
/// Then this many of a random nonce (a `u16`), so two runs started in the
/// same second, in one checkout or on one preview, have ids of their own.
const NONCE_HEX: usize = 4;
/// A run's id is this many lowercase hex digits.
pub const RUN_HEX: usize = CLOCK_HEX + NONCE_HEX;
/// The values a run's clock cycles through (16^6 seconds: about 194 days,
/// so a run's labels are new beside any earlier run's still on the preview).
const CLOCKS: i64 = 1 << (4 * CLOCK_HEX);
/// `--sweep-all` spares a fragment younger than this: a run started after
/// whoever chose to sweep everything may be using it. A whole hosted run
/// takes well under an hour (agent-smoke, the longest section, half of one).
pub const SPARED_FOR_MS: i64 = 60 * 60 * 1000;
/// The pages of e2e people one sweep walks at most (a page is 100).
const PAGES_MAX: usize = 100;
/// The spared fragments a report names one by one; the rest it counts.
const NAMED_MAX: usize = 20;

const _: () = assert!(CLOCK_HEX * 4 < 63, "a run's clock fits an i64");
const _: () = assert!(NONCE_HEX == 2 * std::mem::size_of::<u16>(), "a nonce is a u16's hex");
const _: () = assert!(SPARED_FOR_MS > 0);

/// The id of a run that starts at `now_s`, drawing `nonce`.
pub fn run_id(now_s: i64, nonce: u16) -> String {
    let id = format!("{:0clock$x}{nonce:0NONCE_HEX$x}", now_s.rem_euclid(CLOCKS), clock = CLOCK_HEX);
    assert!(is_run(&id), "a run's id is {RUN_HEX} hex digits: {id:?}");
    id
}

/// Whether `text` is a run's id: `RUN_HEX` lowercase hex digits.
pub fn is_run(text: &str) -> bool {
    text.len() == RUN_HEX && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The label a hosted run gives `base`: `e2e-<run>-<base>`.
pub fn label(run: &str, base: &str) -> String {
    assert!(is_run(run), "a run's id is {RUN_HEX} hex digits: {run:?}");
    assert!(!base.is_empty(), "a label names what it is after its run");
    format!("{E2E_LABEL_PREFIX}{run}-{base}")
}

/// The run a label names (`e2e-<run>-…`), if it names one. A label of an
/// older layout (`e2e-<base>-<run>`), or one made by hand, names none.
pub fn run_of(label: &str) -> Option<&str> {
    let rest = label.strip_prefix(E2E_LABEL_PREFIX)?;
    let (run, base) = rest.split_once('-')?;
    if is_run(run) && !base.is_empty() {
        Some(run)
    } else {
        None
    }
}

/// What a sweep removes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// One run's fragments, by its id.
    Run(String),
    /// Every e2e fragment at least this old (ms): `--sweep-all` spares
    /// `SPARED_FOR_MS`.
    All { spared_for_ms: i64 },
}

/// Why a sweep kept a fragment.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kept {
    /// Not the e2e's: its label does not start `e2e-`.
    NotE2e,
    /// Another run's, by its label (`None`: the label names no run).
    OtherRun(Option<String>),
    /// Younger than the sweep spares: its age, in ms.
    Young(i64),
    /// Its age did not answer, so the sweep cannot tell it is old enough.
    AgeUnknown,
}

/// What a sweep does with one of an e2e person's fragments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    Delete,
    Keep(Kept),
}

/// The choice for the fragment labelled `label`. `created_ms` asks when it
/// was made, and only `Scope::All` asks (it costs a call).
pub fn choose(scope: &Scope, label: &str, created_ms: impl FnOnce() -> Option<i64>, now_ms: i64) -> Choice {
    if !label.starts_with(E2E_LABEL_PREFIX) {
        return Choice::Keep(Kept::NotE2e);
    }
    match scope {
        Scope::Run(run) => match run_of(label) {
            Some(of) if of == run => Choice::Delete,
            of => Choice::Keep(Kept::OtherRun(of.map(str::to_string))),
        },
        Scope::All { spared_for_ms } => match created_ms() {
            None => Choice::Keep(Kept::AgeUnknown),
            // a fragment the clocks put in the future is young: kept
            Some(at) if now_ms - at >= *spared_for_ms => Choice::Delete,
            Some(at) => Choice::Keep(Kept::Young(now_ms - at)),
        },
    }
}

/// What a sweep does with a person's computers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Computers {
    /// Not the sweep's to touch: the person is not the run's.
    Untouched,
    /// Left awake: the sweep spared one of the person's fragments, so a
    /// run may be using them.
    LeftAwake,
    Sleep,
}

/// The fate of a person's computers, once the sweep chose to delete
/// `chosen` of their fragments and spared `spared` of the e2e's.
pub fn computers(scope: &Scope, chosen: usize, spared: usize) -> Computers {
    let ours = match scope {
        Scope::Run(_) => chosen > 0,
        // every e2e person is the whole sweep's
        Scope::All { .. } => true,
    };
    match (ours, spared) {
        (false, _) => Computers::Untouched,
        (true, 0) => Computers::Sleep,
        (true, _) => Computers::LeftAwake,
    }
}

/// Whom a sweep signs in.
#[derive(Debug, Clone, Copy)]
pub enum Who<'a> {
    /// Every e2e person, as the levers list them.
    Everyone,
    /// These, by the emails they signed in by (a run's own, as it noted
    /// them).
    These(&'a [String]),
}

/// What a sweep did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Swept {
    pub people: usize,
    /// What it did not do, each said: a sweep again finishes it.
    pub left: Vec<String>,
    pub deleted: usize,
    pub slept: usize,
    /// The fragments it kept that are not the e2e's.
    pub not_e2e: usize,
    /// The e2e fragments it kept, by name, and why.
    pub spared: Vec<(String, Kept)>,
    /// The awake computers it left so: their people own a fragment it spared.
    pub awake: usize,
}

impl Swept {
    /// The fragments spared for `why`.
    pub fn spared_for(&self, why: impl Fn(&Kept) -> bool) -> Vec<&str> {
        self.spared.iter().filter(|(_, k)| why(k)).map(|(n, _)| n.as_str()).collect()
    }

    /// What it kept, and why: other runs' fragments counted by run, the
    /// young ones named with their age, and the computers it left awake.
    pub fn report(&self) -> String {
        let mut by_run: BTreeMap<&str, usize> = BTreeMap::new();
        let (mut young, mut unknown) = (vec![], 0usize);
        for (name, why) in &self.spared {
            match why {
                Kept::OtherRun(run) => *by_run.entry(run.as_deref().unwrap_or("no run id")).or_default() += 1,
                Kept::Young(age_ms) => young.push(format!("{name} ({} min old)", (*age_ms).max(0) / 60_000)),
                Kept::AgeUnknown => unknown += 1,
                Kept::NotE2e => unreachable!("a fragment not the e2e's is counted, not spared"),
            }
        }
        let mut out = format!("kept {}", count(self.spared.len(), "e2e- fragment"));
        if !by_run.is_empty() {
            let runs: Vec<String> = by_run.iter().map(|(run, n)| format!("{run} ({n})")).collect();
            out += &format!("\n  {} of other runs: {}", by_run.values().sum::<usize>(), runs.join(", "));
        }
        if !young.is_empty() {
            let more = young.len().saturating_sub(NAMED_MAX);
            young.truncate(NAMED_MAX);
            out += &format!("\n  {} younger than {} min (a run may be using them): {}", young.len() + more, SPARED_FOR_MS / 60_000, young.join(", "));
            if more > 0 {
                out += &format!(", and {more} more");
            }
        }
        if unknown > 0 {
            out += &format!("\n  {unknown} whose age did not answer");
        }
        out += &format!("\nkept {} not the e2e's; left {} awake (their people own a fragment kept)", count(self.not_e2e, "fragment"), count(self.awake, "computer"));
        if !self.left.is_empty() {
            out += &format!("\nleft {} undone (sweep again):\n  {}", count(self.left.len(), "thing"), self.left.join("\n  "));
        }
        out
    }
}

/// `n` of `noun`, as a person says it.
fn count(n: usize, noun: &str) -> String {
    match n {
        1 => format!("1 {noun}"),
        _ => format!("{n} {noun}s"),
    }
}

/// A call as the shell makes it, with a platform session (the sweep's
/// people have no key).
fn shell(api: &Api, session: &str, method: &str, path: &str) -> Result<Reply> {
    api.call(Call {
        method,
        url: format!("{}{path}", api.base),
        cookie: Some(format!("fragment_session={session}")),
        extra: vec![("x-fragment-shell", "1".into()), ("sec-fetch-site", "same-origin".into()), ("origin", api.base.clone())],
        ..Call::default()
    })
}

/// When `name` was made, in ms: its earliest member's `addedAt` (its owner
/// joined as it was made). `None` when that does not answer.
fn created_ms(api: &Api, session: &str, name: &str) -> Option<i64> {
    let r = shell(api, session, "GET", &format!("/api/f/{name}/members")).ok()?;
    if r.status != 200 {
        return None;
    }
    r.body["members"].as_array()?.iter().filter_map(|m| m["addedAt"].as_i64()).min()
}

/// The sweep of `scope`, on the deployment `api` reaches (a preview, or a
/// rehearsal's node), as of `now_ms`, signing in `who`. A failure is in
/// `Swept::left`, the rest done; only a list of people that did not answer
/// ends it early (there is no one further to reach).
pub fn sweep_on(api: &Api, scope: &Scope, who: Who<'_>, now_ms: i64) -> Swept {
    assert!(api.signs_in_by_levers(), "a sweep signs the e2e people in through the levers");
    match scope {
        Scope::Run(run) => assert!(is_run(run), "a run's sweep names a run: {run:?}"),
        Scope::All { spared_for_ms } => assert!(*spared_for_ms >= 0, "a sweep spares fragments younger than an age"),
    }
    let mut swept = Swept::default();
    match who {
        Who::These(emails) => {
            for email in emails {
                sweep_person(api, scope, email, now_ms, &mut swept);
            }
        }
        Who::Everyone => {
            let mut after: Option<String> = None;
            for page in 0..=PAGES_MAX {
                if page == PAGES_MAX {
                    swept.left.push(format!("more than {PAGES_MAX} pages of e2e people: the rest"));
                    break;
                }
                let r = match api.unsigned("POST", "/api/test/people", Some(&json!({ "after": after }))) {
                    Ok(r) if r.status == 200 => r,
                    Ok(r) => {
                        swept.left.push(format!("the e2e people after {after:?}: {r}"));
                        break;
                    }
                    Err(e) => {
                        swept.left.push(format!("the e2e people after {after:?}: {e:#}"));
                        break;
                    }
                };
                for person in r.body["people"].as_array().into_iter().flatten() {
                    match person["email"].as_str() {
                        Some(email) => sweep_person(api, scope, email, now_ms, &mut swept),
                        None => swept.left.push(format!("an e2e person with no email: {person}")),
                    }
                }
                after = r.body["next"].as_str().map(str::to_string);
                if after.is_none() {
                    break;
                }
            }
        }
    }
    if let Scope::Run(run) = scope {
        assert!(swept.spared.iter().all(|(n, _)| run_of(fragment_proto::split_fragment_name(n).map_or("", |(label, _)| label)) != Some(run.as_str())), "a run's sweep spares none of its run's");
    }
    swept
}

/// One e2e person's part of a sweep: signed in again, their fragments
/// chosen (`choose`) and deleted, their computers put to sleep or left
/// (`computers`). What does not answer is noted and passed.
fn sweep_person(api: &Api, scope: &Scope, email: &str, now_ms: i64, swept: &mut Swept) {
    swept.people += 1;
    let session = match api.e2e_session(email) {
        Ok((session, _)) => session,
        Err(e) => return swept.left.push(format!("signing {email} in: {e:#}")),
    };
    let listed = match shell(api, &session, "GET", "/api/fragments") {
        Ok(r) if r.status == 200 => r,
        Ok(r) => return swept.left.push(format!("listing {email}'s fragments: {r}")),
        Err(e) => return swept.left.push(format!("listing {email}'s fragments: {e:#}")),
    };
    // a delete answers within one round of its members' lists (the cell's
    // ended.rs), well inside the client's 60 s
    let (mut chosen, mut spared) = (0usize, 0usize);
    for f in listed.body["fragments"].as_array().into_iter().flatten().filter(|f| f["role"] == "owner") {
        let name = f["name"].as_str().unwrap_or("");
        let label = fragment_proto::split_fragment_name(name).map_or(name, |(label, _)| label);
        match choose(scope, label, || created_ms(api, &session, name), now_ms) {
            Choice::Keep(Kept::NotE2e) => swept.not_e2e += 1,
            Choice::Keep(why) => {
                spared += 1;
                swept.spared.push((name.to_string(), why));
            }
            Choice::Delete => {
                chosen += 1;
                match shell(api, &session, "DELETE", &format!("/api/f/{name}")) {
                    Ok(r) if r.status == 200 || r.status == 404 => swept.deleted += 1,
                    Ok(r) => swept.left.push(format!("deleting {name}: {r}")),
                    Err(e) => swept.left.push(format!("deleting {name}: {e:#}")),
                }
            }
        }
    }
    let fate = computers(scope, chosen, spared);
    if fate == Computers::Untouched {
        return;
    }
    let listed = match shell(api, &session, "GET", "/api/computers") {
        Ok(r) if r.status == 200 => r,
        Ok(r) => return swept.left.push(format!("listing {email}'s computers: {r}")),
        Err(e) => return swept.left.push(format!("listing {email}'s computers: {e:#}")),
    };
    for c in listed.body["computers"].as_array().into_iter().flatten().filter(|c| c["phase"] != "asleep") {
        if fate == Computers::LeftAwake {
            swept.awake += 1;
            continue;
        }
        let id = c["computer"].as_str().unwrap_or("");
        match shell(api, &session, "POST", &format!("/api/computers/{id}/sleep")) {
            Ok(r) if r.status == 200 => swept.slept += 1,
            Ok(r) => swept.left.push(format!("putting {id} to sleep: {r}")),
            Err(e) => swept.left.push(format!("putting {id} to sleep: {e:#}")),
        }
    }
}

/// Where a hosted run that finished here notes its id, for `--sweep`
/// without one: one file a preview, in this checkout's target directory
/// (another checkout's runs are its own).
pub fn last_run_file(preview: &Preview) -> PathBuf {
    devstack::repo_root().join("target/e2e").join(format!("hosted-{}.{}.last-run", preview.branch, preview.zone))
}

/// Notes `run` as the last run that finished here on `preview`.
pub fn note_last_run(preview: &Preview, run: &str) -> Result<()> {
    assert!(is_run(run), "a run's id is {RUN_HEX} hex digits: {run:?}");
    let file = last_run_file(preview);
    std::fs::create_dir_all(file.parent().context("the file is in a directory")?)?;
    std::fs::write(&file, format!("{run}\n")).with_context(|| format!("write {}", file.display()))
}

/// Where a hosted run that finished here notes the emails its people
/// signed in by, for its sweep (`Who::These`): one file a run.
pub fn run_people_file(preview: &Preview, run: &str) -> PathBuf {
    devstack::repo_root().join("target/e2e").join(format!("hosted-{}.{}.{run}.people", preview.branch, preview.zone))
}

/// Notes `emails` as the people of `run` on `preview`, one a line.
pub fn note_run_people(preview: &Preview, run: &str, emails: &[String]) -> Result<()> {
    assert!(is_run(run), "a run's id is {RUN_HEX} hex digits: {run:?}");
    assert!(emails.iter().all(|e| !e.is_empty() && !e.contains('\n')), "an email is one line");
    let file = run_people_file(preview, run);
    std::fs::create_dir_all(file.parent().context("the file is in a directory")?)?;
    std::fs::write(&file, emails.iter().map(|e| format!("{e}\n")).collect::<String>()).with_context(|| format!("write {}", file.display()))
}

/// The people `run` noted here on `preview`, if it ran here.
pub fn run_people(preview: &Preview, run: &str) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(run_people_file(preview, run)).ok()?;
    Some(text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect())
}

/// The last run that finished here on `preview`, as `note_last_run` noted it.
pub fn last_run(preview: &Preview) -> Result<String> {
    let file = last_run_file(preview);
    let text = std::fs::read_to_string(&file).with_context(|| {
        format!(
            "no hosted run of {} finished in this checkout ({}): name one (`--sweep <run>`, the {RUN_HEX} hex digits after `e2e-` in its labels), or `--sweep-all` when nothing else runs there",
            preview.platform(),
            file.display()
        )
    })?;
    let run = text.trim();
    anyhow::ensure!(is_run(run), "{} holds {run:?}, not a run's id: name one (`--sweep <run>`)", file.display());
    Ok(run.to_string())
}
