//! finite-mono's mover, faked (docs/migrations.md): a client of the
//! platform, not a service it calls. It signs as the mover's key (NIP-98),
//! pushes rosters, and works its mailbox as the real one does, command by
//! command, each step reported in order. A stage it uploads through the
//! platform, in parts, then completes: it holds no bucket credential. Its
//! levers script what a command comes to: done, failed, stalled after a
//! part, or nothing at all (the mover down); and the zip and its parts'
//! size.

use std::collections::BTreeMap;
use std::sync::Mutex;

use anyhow::{Context, Result};
use fragment_nip98::Keys;
use fragment_proto::migrations::{Command, CommandKind, LeftBehind, Mailbox, Manifest, Phase, Report, Roster, STAGE_FORMAT};
use serde_json::Value;
use sha2::Digest;

/// What the next commands come to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    /// Each done: stopped, progress, staged; retired; restarted.
    Done,
    /// A stage stops the old agent, then fails.
    Fails,
    /// A stage stops the old agent and uploads its first part, then nothing more.
    Stalls,
    /// The mover is down: it reads nothing and reports nothing.
    Down,
}

/// What one poll did: each command, and the answers to its reports.
pub type Worked = Vec<(Command, Vec<Answered>)>;

/// One call's answer: its status and JSON.
#[derive(Debug, Clone)]
pub struct Answered {
    pub status: u16,
    pub body: Value,
}

pub struct Mover {
    keys: Keys,
    /// The platform's origin, as the mover calls it.
    base: String,
    http: reqwest::blocking::Client,
    script: Mutex<Script>,
    /// The zip it stages, and the size of each part it sends.
    zip: Mutex<(Vec<u8>, usize)>,
    /// The last `seq` reported on each command.
    seqs: Mutex<BTreeMap<String, u64>>,
}

impl Mover {
    pub fn new(base: &str, keys: Keys) -> Mover {
        let zip = fixture_zip(64 * 1024);
        let part = zip.len();
        Mover { keys, base: base.trim_end_matches('/').to_string(), http: reqwest::blocking::Client::new(), script: Mutex::new(Script::Done), zip: Mutex::new((zip, part)), seqs: Mutex::default() }
    }

    /// The key's npub: what the deployment's `migrations.mover` names.
    pub fn npub(&self) -> String {
        fragment_core::npub::encode(self.keys.pubkey_hex())
    }

    pub fn script(&self, s: Script) {
        *self.script.lock().expect("the mover's script") = s;
    }

    /// The zip the next stages upload, in parts of `part` bytes.
    pub fn zip(&self, zip: Vec<u8>, part: usize) {
        assert!(part > 0, "a part holds bytes");
        *self.zip.lock().expect("the mover's zip") = (zip, part);
    }

    /// The zip the next stages upload.
    pub fn zip_bytes(&self) -> Vec<u8> {
        self.zip.lock().expect("the mover's zip").0.clone()
    }

    /// One signed call, as the mover makes it.
    pub fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Answered> {
        let bytes = body.map(|b| b.to_string().into_bytes());
        self.send(method, path, bytes.as_deref(), "application/json")
    }

    /// One signed call with `body` of `kind`, if any.
    fn send(&self, method: &str, path: &str, body: Option<&[u8]>, kind: &str) -> Result<Answered> {
        let url = format!("{}{path}", self.base);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64;
        let auth = self.keys.header(method, &url, body.unwrap_or_default(), now);
        let mut req = self.http.request(method.parse()?, &url).header("authorization", auth);
        if let Some(b) = body {
            req = req.header("content-type", kind).body(b.to_vec());
        }
        let resp = req.send().with_context(|| format!("{method} {url}"))?;
        let status = resp.status().as_u16();
        let text = resp.text()?;
        Ok(Answered { status, body: serde_json::from_str(&text).unwrap_or(Value::String(text)) })
    }

    pub fn push(&self, roster: &Roster) -> Result<Answered> {
        self.call("PUT", "/api/migrations/mover/roster", Some(&serde_json::to_value(roster)?))
    }

    pub fn mailbox(&self) -> Result<Answered> {
        self.call("GET", "/api/migrations/mover/mailbox", None)
    }

    pub fn commands(&self) -> Result<Vec<Command>> {
        let a = self.mailbox()?;
        anyhow::ensure!(a.status == 200, "the mailbox: {} {}", a.status, a.body);
        Ok(serde_json::from_value::<Mailbox>(a.body)?.commands)
    }

    /// Reports `event` on `command` (its command and seq filled in), with the
    /// next `seq` this mover keeps for it.
    pub fn report(&self, command: &Command, event: Report) -> Result<Answered> {
        let seq = {
            let mut seqs = self.seqs.lock().expect("the mover's seqs");
            let s = seqs.entry(command.id.clone()).or_default();
            *s += 1;
            *s
        };
        self.report_as(&command.old, &command.id, seq, event)
    }

    /// Reports with a `seq` and command of the caller's: a replay, or a
    /// stale attempt's.
    pub fn report_as(&self, old: &str, command: &str, seq: u64, event: Report) -> Result<Answered> {
        let r = Report { command: command.into(), seq, ..event };
        self.call("POST", &format!("/api/migrations/mover/agents/{old}/report"), Some(&serde_json::to_value(&r)?))
    }

    /// Part `n` of `stage`'s zip, its hash in its signed URL.
    pub fn put_part(&self, stage: &str, n: u32, bytes: &[u8]) -> Result<Answered> {
        let sha = hex::encode(sha2::Sha256::digest(bytes));
        self.send("PUT", &format!("/api/migrations/mover/stages/{stage}/parts/{n}?sha256={sha}"), Some(bytes), "application/octet-stream")
    }

    /// Part `n` of `stage`, its URL naming `sha256` whatever its bytes are.
    pub fn put_part_claiming(&self, stage: &str, n: u32, bytes: &[u8], sha256: &str) -> Result<Answered> {
        self.send("PUT", &format!("/api/migrations/mover/stages/{stage}/parts/{n}?sha256={sha256}"), Some(bytes), "application/octet-stream")
    }

    /// The parts of `stage` the platform has taken.
    pub fn parts(&self, stage: &str) -> Result<Answered> {
        self.call("GET", &format!("/api/migrations/mover/stages/{stage}/parts"), None)
    }

    pub fn complete(&self, stage: &str, manifest: &Manifest) -> Result<Answered> {
        self.call("POST", &format!("/api/migrations/mover/stages/{stage}/complete"), Some(&serde_json::json!({ "manifest": manifest })))
    }

    /// The manifest of the zip it uploads, for `c`.
    pub fn manifest(&self, c: &Command) -> Result<Manifest> {
        let phase: Phase = c.phase.filter(|_| c.kind == CommandKind::StopAndStage).with_context(|| format!("{} is no stage command", c.id))?;
        let zip = self.zip_bytes();
        Ok(Manifest {
            format: STAGE_FORMAT.into(),
            old: c.old.clone(),
            phase,
            attempt: c.attempt,
            layout: "finite-computer".into(),
            zip_bytes: zip.len() as u64,
            zip_sha256: hex::encode(sha2::Sha256::digest(&zip)),
            bytes: 4096,
            files: 3,
            left: vec![LeftBehind { path: "agent/hermes-home/.env".into(), bytes: 120, why: "secret".into() }],
            left_more: None,
        })
    }

    /// `c`'s stage uploaded, the parts the platform holds skipped (a
    /// resume), then completed: its manifest's SHA-256.
    pub fn stage(&self, c: &Command) -> Result<String> {
        let stage = c.stage.as_deref().filter(|_| c.kind == CommandKind::StopAndStage).with_context(|| format!("{} is no stage command", c.id))?;
        let (zip, part) = self.zip.lock().expect("the mover's zip").clone();
        let held = self.parts(stage)?;
        anyhow::ensure!(held.status == 200, "{stage}'s parts: {} {}", held.status, held.body);
        for (i, chunk) in zip.chunks(part).enumerate() {
            let n = i as u32 + 1;
            if held.body["parts"].as_array().is_some_and(|p| p.iter().any(|p| p["n"] == n && p["bytes"] == chunk.len())) {
                continue;
            }
            let a = self.put_part(stage, n, chunk)?;
            anyhow::ensure!(a.status == 200, "{stage} part {n}: {} {}", a.status, a.body);
        }
        let done = self.complete(stage, &self.manifest(c)?)?;
        anyhow::ensure!(done.status == 200, "{stage}'s complete: {} {}", done.status, done.body);
        done.body["manifest_sha256"].as_str().map(str::to_string).context("a complete answers its manifest's sha256")
    }

    /// One poll: every command in the mailbox worked as the script says,
    /// each step's answer kept.
    pub fn run_once(&self) -> Result<Worked> {
        let script = *self.script.lock().expect("the mover's script");
        if script == Script::Down {
            return Ok(vec![]);
        }
        let mut done = vec![];
        for c in self.commands()? {
            let mut answers = vec![];
            match c.kind {
                CommandKind::StopAndStage => {
                    answers.push(self.report(&c, event("stopped"))?);
                    answers.push(self.report(&c, progress(1, 1))?);
                    match script {
                        Script::Done => {
                            let sha = self.stage(&c)?;
                            answers.push(self.report(&c, staged(&sha))?);
                        }
                        Script::Fails => answers.push(self.report(&c, failed("stream", "the fake mover's lever"))?),
                        Script::Stalls => {
                            // one part, then nothing: an upload cut off
                            let stage = c.stage.clone().context("a stage command names its stage")?;
                            let (zip, part) = self.zip.lock().expect("the mover's zip").clone();
                            answers.push(self.put_part(&stage, 1, &zip[..part.min(zip.len())])?);
                        }
                        Script::Down => {}
                    }
                }
                CommandKind::Retire => answers.push(self.report(&c, retired(&format!("fake-snapshot-{}", c.id)))?),
                CommandKind::Restart => answers.push(self.report(&c, event("restarted"))?),
            }
            done.push((c, answers));
        }
        Ok(done)
    }
}

/// A zip's stand-in, `len` bytes the platform never reads inside (the
/// importer does), each byte known for a range's check.
pub fn fixture_zip(len: usize) -> Vec<u8> {
    (0..len as u64).map(|i| (i.wrapping_mul(31) % 251) as u8).collect()
}

/// A report of `kind` alone: what `Mover::report` fills its command and seq into.
pub fn event(kind: &str) -> Report {
    Report::of("", 0, kind)
}

pub fn progress(bytes: u64, files: u64) -> Report {
    Report { bytes: Some(bytes), files: Some(files), ..event("progress") }
}

/// Staged, its manifest's SHA-256 as complete answered it (4096 bytes in 3 files).
pub fn staged(manifest_sha256: &str) -> Report {
    Report { manifest_sha256: Some(manifest_sha256.into()), bytes: Some(4096), files: Some(3), ..event("staged") }
}

pub fn failed(step: &str, why: &str) -> Report {
    Report { step: Some(step.into()), why: Some(why.into()), ..event("failed") }
}

pub fn retired(receipt: &str) -> Report {
    Report { receipt: Some(receipt.into()), ..event("retired") }
}
