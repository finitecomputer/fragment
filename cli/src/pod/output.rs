// What `fragment pod` prints: every check named with PASS, FAIL or
// SKIPPED and one line of evidence, then what the pod attested, marked
// unverified unless the quote check passed; and the same as `--json`'s data.

use std::io::{self, Write};
use std::path::Path;

use fragment_core::npub;
use fragment_pod::checks::release::Matched;
use fragment_pod::owner::attest::Fetched;
use fragment_pod::owner::checks::{Check, Outcome};
use fragment_pod::owner::flow::Step;
use fragment_pod::unlock::binding::State;
use fragment_pod::verify::policy::Status;
use serde_json::{json, Value};

fn word(s: Status) -> &'static str {
    match s {
        Status::Pass => "PASS",
        Status::Fail => "FAIL",
        Status::Skipped => "SKIPPED",
    }
}

pub fn print_checks(out: &mut dyn Write, checks: &[Check]) -> io::Result<()> {
    for c in checks {
        writeln!(out, "{:<16} {:<7}  {}", c.name, word(c.status), c.detail)?;
    }
    Ok(())
}

/// While locked the pod can only repeat the hint beside its volume.
fn owner_source(state: State) -> &'static str {
    match state {
        State::Blank => "no volume",
        State::Locked => "the hint beside the volume, unauthenticated until unlock",
        State::Open => "recorded inside the volume",
    }
}

fn owner_text(owner: Option<[u8; 32]>, me: Option<[u8; 32]>) -> String {
    match owner {
        None => "none".into(),
        Some(o) => {
            let mine = if Some(o) == me { ", this key" } else { "" };
            format!("{} ({}{mine})", npub::encode(&hex::encode(o)), hex::encode(o))
        }
    }
}

fn release(m: &Matched) -> Value {
    json!({ "seq": m.seq, "name": m.name, "notCompared": m.not_compared })
}

/// `--json`'s data for one attestation and its checks.
pub fn attested(f: &Fetched, o: &Outcome, extra: &[Check], me: Option<[u8; 32]>) -> Value {
    let all: Vec<&Check> = o.checks.iter().chain(extra).collect();
    json!({
        "passed": !all.iter().any(|c| c.status == Status::Fail),
        "attested": trusted(o),
        "checks": all,
        "state": f.status.state.as_str(),
        "owner": f.status.owner.map(|o| npub::encode(&hex::encode(o))),
        "ownerHex": f.status.owner.map(hex::encode),
        "ownerIsThisKey": f.status.owner.is_some() && f.status.owner == me,
        "ownerSource": owner_source(f.status.state),
        "tlsRoot": f.status.tls_root.map(hex::encode),
        "bootKey": hex::encode(f.status.boot_pubkey),
        "release": o.release.as_ref().map(release),
    })
}

fn trusted(o: &Outcome) -> bool {
    o.checks.first().is_some_and(|c| c.status == Status::Pass)
}

pub fn print_attested(out: &mut dyn Write, f: &Fetched, o: &Outcome, extra: &[Check], me: Option<[u8; 32]>) -> io::Result<()> {
    print_checks(out, &o.checks)?;
    print_checks(out, extra)?;
    let mark = if trusted(o) { "" } else { " (unverified)" };
    let hex_or_none = |v: Option<[u8; 32]>| v.map(hex::encode).unwrap_or_else(|| "none".into());
    writeln!(out)?;
    writeln!(out, "state            {}{mark}", f.status.state.as_str())?;
    writeln!(out, "owner            {}; {}{mark}", owner_text(f.status.owner, me), owner_source(f.status.state))?;
    writeln!(out, "tls root         {}{mark}", hex_or_none(f.status.tls_root))?;
    writeln!(out, "boot key         {}{mark}", hex::encode(f.status.boot_pubkey))
}

/// The steps of a key request: printed as they happen, or kept for `--json`.
pub struct Steps {
    json: bool,
    checked: Vec<Check>,
    confirmed: Vec<Check>,
    release: Option<Value>,
}

impl Steps {
    pub fn new(json: bool) -> Steps {
        Steps { json, checked: Vec::new(), confirmed: Vec::new(), release: None }
    }

    pub fn on(&mut self, out: &mut dyn Write, s: Step<'_>) -> io::Result<()> {
        match s {
            Step::Checked(c) => {
                self.checked = c.to_vec();
                if !self.json {
                    print_checks(out, c)?;
                }
            }
            Step::Sent if !self.json => writeln!(out, "\nthe pod says open; confirming with a fresh attestation")?,
            Step::Sent => {}
            Step::Confirming(c) => {
                self.confirmed = c.to_vec();
                if !self.json {
                    print_checks(out, c)?;
                }
            }
            Step::Confirmed { owner } if !self.json => writeln!(out, "\nconfirmed: attested open on the same boot, owned by {}", owner_text(Some(owner), Some(owner)))?,
            Step::Confirmed { .. } => {}
            Step::Recorded(m) => {
                self.release = Some(release(m));
                if !self.json {
                    writeln!(out, "recorded release {} (seq {}) before sending the key: an older release is refused from now on", m.name, m.seq)?;
                }
            }
        }
        Ok(())
    }

    pub fn into_json(self) -> Value {
        json!({ "state": "open", "checks": self.checked, "confirmed": self.confirmed, "release": self.release })
    }
}

pub fn print_recovery(out: &mut dyn Write, code: &str, path: &Path) -> io::Result<()> {
    writeln!(out, "\nrecovery code, shown this once: write it down and keep it offline")?;
    writeln!(out, "  {code}")?;
    writeln!(out, "it is the disk key, also kept in {} (0600); without it the pod's data cannot be opened", path.display())?;
    writeln!(out, "to restore it on another machine, write it as that file's one line")
}
