// `fragment pod keyd status|deposit|approve <pod> --keyd <host[:port]>`:
// a key service (pod-keyd) that gives a pod its disk key after a restart
// on the owner's terms, so the pod need not wait for this machine
// (docs/pod.md, "A key service"). A thin command line over fragment_pod's
// `owner::keyd`, with this CLI's key as the owner, as the other pod
// commands. The key service is held to `keyd-policy.json` in the pod's
// folder; nothing is signed for it unless its four checks pass.
//
// Exit codes as the other pod commands': 0 when every check passed and
// the action was done; 1 when a check failed or the key service refused
// (`pod_refused`, naming which), or it did not answer; 2 for a usage
// mistake (no keyd-policy.json, a bad duration).

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use clap::Subcommand;
use fragment_core::npub;
use fragment_pod::checks::release::Matched;
use fragment_pod::keyd::approve::Approval;
use fragment_pod::keyd::view::RecordView;
use fragment_pod::owner::checks::Check;
use fragment_pod::owner::keyd::{self, Window, DEFAULT_PORT, KEYD_POLICY};
use fragment_pod::unlock::release::Release;
use fragment_pod::verify::policy::Status;
use fragment_pod::verify::time::rfc3339;
use serde_json::{json, Value};

use super::output::print_checks;
use super::{coded, open_dir, owner_key, require_key, usage, Address, Ctx, Report};

/// A window's longest: ten years, in seconds.
const DURATION_MAX: u64 = 3650 * 86_400;

#[derive(Subcommand)]
pub enum KeydCmd {
    /// The key service's four checks (quote, release, platform,
    /// google_instance, against keyd-policy.json) and the record it holds
    /// for this key, changing nothing; exit 1 if a check failed
    Status {
        /// The pod: host[:port]
        pod: String,
        /// The key service: host[:port] (port 7100 if none)
        #[arg(long)]
        keyd: String,
    },
    /// Every check, then this pod's disk key deposited with the key
    /// service, sealed to this boot of it: policy.json's instance and its
    /// releases at or above the floor as the approvals, and the floor as
    /// the oldest release it may serve. Replaces the record it holds
    Deposit {
        pod: String,
        #[arg(long)]
        keyd: String,
    },
    /// Every check, then one more release approved for the record the key
    /// service holds, from a release file (one release, as policy.json
    /// lists them)
    Approve {
        pod: String,
        #[arg(long)]
        keyd: String,
        /// The release file
        #[arg(long)]
        release: std::path::PathBuf,
        /// Approved only after this long: <n>s, m, h or d
        #[arg(long)]
        delay: Option<String>,
        /// Approved only until this long from now: <n>s, m, h or d
        #[arg(long)]
        expires: Option<String>,
    },
}

/// `<n>` and a unit, s, m, h or d; at most ten years.
pub fn parse_duration(text: &str) -> Result<u64> {
    let bad = || usage(format!("{text:?} is not a duration: <n>s, <n>m, <n>h or <n>d, at most 3650d"));
    let unit = match text.chars().last() {
        Some('s') => 1,
        Some('m') => 60,
        Some('h') => 3600,
        Some('d') => 86_400,
        _ => return Err(bad()),
    };
    let n: u64 = text[..text.len() - 1].parse().map_err(|_| bad())?;
    n.checked_mul(unit).filter(|s| *s > 0 && *s <= DURATION_MAX).ok_or_else(bad)
}

/// The window `--delay` and `--expires` give, from `now`.
pub fn window(delay: Option<&str>, expires: Option<&str>, now: u64) -> Result<Window> {
    let not_before = delay.map(parse_duration).transpose()?.map(|d| now + d);
    let until = expires.map(parse_duration).transpose()?.map(|d| now + d);
    if let (Some(from), Some(to)) = (not_before, until) {
        if to <= from {
            return Err(usage("--expires must be later than --delay: the approval would never open"));
        }
    }
    Ok(Window { not_before, expires: until })
}

pub fn run(ctx: &Ctx<'_>, cmd: KeydCmd, out: &mut dyn Write) -> Result<Report> {
    match cmd {
        KeydCmd::Status { pod, keyd } => status(ctx, &Address::parse(&pod)?, &key_service(&keyd)?, out),
        KeydCmd::Deposit { pod, keyd } => deposit(ctx, &Address::parse(&pod)?, &key_service(&keyd)?, out),
        KeydCmd::Approve { pod, keyd, release, delay, expires } => {
            let w = window(delay.as_deref(), expires.as_deref(), keyd::now())?;
            approve(ctx, &Address::parse(&pod)?, &key_service(&keyd)?, &release, w, out)
        }
    }
}

fn key_service(text: &str) -> Result<Address> {
    Address::parse_port(text, DEFAULT_PORT)
}

/// The pod's folder, which must hold keyd-policy.json beside policy.json.
fn keyd_dir(ctx: &Ctx<'_>, a: &Address) -> Result<fragment_pod::owner::dir::Dir> {
    let (dir, path) = open_dir(ctx, a)?;
    let policy = path.join(KEYD_POLICY);
    if !policy.is_file() {
        return Err(usage(format!(
            "no key service policy for the pod {}: write {} (the key service's releases and Google instance; cli/GUIDE.md, \"Your pod\")",
            a.host,
            policy.display()
        )));
    }
    Ok(dir)
}

fn me(ctx: &Ctx<'_>) -> Option<[u8; 32]> {
    ctx.key.map(|id| owner_key(id).public())
}

fn status(ctx: &Ctx<'_>, pod: &Address, ks: &Address, out: &mut dyn Write) -> Result<Report> {
    let dir = keyd_dir(ctx, pod)?;
    let r = keyd::status(&dir, me(ctx), ctx.attestor, &ks.host_port())?;
    let passed = !r.any_fail();
    let trusted = r.outcome.checks.first().is_some_and(|c| c.status == Status::Pass);
    let mut data = json!({
        "pod": pod.host_port(),
        "keyd": ks.host_port(),
        "passed": passed,
        "attested": trusted,
        "checks": r.outcome.checks,
        "bootKey": hex::encode(r.fetched.status.boot_pubkey),
        "release": r.outcome.release.as_ref().map(release),
    });
    data["record"] = r.record.as_ref().map(record).unwrap_or(Value::Null);
    if !ctx.json {
        print_checks(out, &r.outcome.checks)?;
        let mark = if trusted { "" } else { " (unverified)" };
        writeln!(out)?;
        writeln!(out, "key service      {}, boot key {}{mark}", ks.host_port(), hex::encode(r.fetched.status.boot_pubkey))?;
        print_record(out, r.record.as_ref())?;
    }
    Ok(Report { data, passed })
}

/// The checks as they come (or kept for `--json`); then the record.
fn signed(ctx: &Ctx<'_>, out: &mut dyn Write, act: impl FnOnce(&mut dyn FnMut(&[Check])) -> Result<RecordView>) -> Result<(Vec<Check>, RecordView)> {
    let mut seen = Vec::new();
    let mut failed_write = None;
    let r = act(&mut |c: &[Check]| {
        seen = c.to_vec();
        if !ctx.json {
            if let Err(e) = print_checks(out, c) {
                failed_write.get_or_insert(e);
            }
        }
    });
    let view = r.map_err(coded)?;
    if let Some(e) = failed_write {
        return Err(e).context("printing the checks");
    }
    Ok((seen, view))
}

fn deposit(ctx: &Ctx<'_>, pod: &Address, ks: &Address, out: &mut dyn Write) -> Result<Report> {
    let id = require_key(ctx)?;
    let dir = keyd_dir(ctx, pod)?;
    let (checks, view) = signed(ctx, out, |on| keyd::deposit(&dir, &owner_key(id), ctx.attestor, &ks.host_port(), on))?;
    if !ctx.json {
        writeln!(out, "\ndeposited this pod's disk key, sealed to this boot of the key service")?;
        print_record(out, Some(&view))?;
        writeln!(out, "the key service keeps it in memory only: after it restarts, deposit again")?;
    }
    Ok(Report { data: json!({ "pod": pod.host_port(), "keyd": ks.host_port(), "checks": checks, "record": record(&view) }), passed: true })
}

fn approve(ctx: &Ctx<'_>, pod: &Address, ks: &Address, file: &Path, w: Window, out: &mut dyn Write) -> Result<Report> {
    let id = require_key(ctx)?;
    let dir = keyd_dir(ctx, pod)?;
    let bytes = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    let rel = Release::from_json(&bytes).map_err(|e| usage(format!("{}: {e}", file.display())))?;
    let (checks, view) = signed(ctx, out, |on| keyd::approve(&dir, &owner_key(id), ctx.attestor, &ks.host_port(), rel, w, on))?;
    if !ctx.json {
        writeln!(out, "\napproved")?;
        print_record(out, Some(&view))?;
    }
    Ok(Report { data: json!({ "pod": pod.host_port(), "keyd": ks.host_port(), "checks": checks, "record": record(&view) }), passed: true })
}

fn release(m: &Matched) -> Value {
    json!({ "seq": m.seq, "name": m.name, "notCompared": m.not_compared })
}

fn approval(a: &Approval) -> Value {
    json!({ "seq": a.release.seq, "name": a.release.name, "notBefore": a.not_before, "expires": a.expires })
}

fn record(v: &RecordView) -> Value {
    json!({
        "id": hex::encode(v.record),
        "owner": npub::encode(&hex::encode(v.owner)),
        "gceInstance": v.gce_instance,
        "allowTcbStatus": v.allow_tcb_status,
        "minSeq": v.min_seq,
        "lastRelease": v.last_release,
        "approvals": v.approvals.iter().map(approval).collect::<Vec<_>>(),
    })
}

fn when(t: Option<u64>) -> String {
    t.and_then(|t| i64::try_from(t).ok()).map(rfc3339).unwrap_or_default()
}

fn print_record(out: &mut dyn Write, v: Option<&RecordView>) -> std::io::Result<()> {
    let Some(v) = v else {
        return writeln!(out, "record           none for this key (none deposited since the key service started)");
    };
    writeln!(out, "record           {} (as the key service reports it, not attested)", hex::encode(v.record))?;
    writeln!(out, "instance         {}", v.gce_instance)?;
    writeln!(out, "min seq          {}: an older release is refused", v.min_seq)?;
    writeln!(out, "last release     {}", v.last_release.map(rfc3339).unwrap_or_else(|| "never".into()))?;
    for a in &v.approvals {
        let from = a.not_before.map(|t| format!(" from {}", when(Some(t)))).unwrap_or_default();
        let until = a.expires.map(|t| format!(" until {}", when(Some(t)))).unwrap_or_default();
        writeln!(out, "approved         {} (seq {}){from}{until}", a.release.name, a.release.seq)?;
    }
    Ok(())
}
