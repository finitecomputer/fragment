//! The full-content audit: the folder against the repo, every file fetched.

use super::journal::load_state;
use super::listing::list_main;
use super::pass::full_name;
use super::scan::{scan_local, Reader, Scan};
use super::{Report, SyncError};
use crate::api::Client;
use crate::codestorage::{CodeStorage, Wants};
use std::path::Path;

/// Full-content audit: local truth vs the repo listing + fetched bytes
/// (no shortcuts — every remote file is fetched and hashed).
pub fn verify(client: &Client, name: &str, dir: &Path) -> Result<Report, SyncError> {
    let name = &full_name(client, name)?;
    let state = load_state(dir, name).map_err(|e| SyncError::Io(e.to_string()))?;
    let Scan { files: local, stats, refused, .. } = scan_local(dir, Some(&state), Reader::Audit).map_err(|e| SyncError::Io(e.to_string()))?;
    let storage = CodeStorage::connect(client, name, Wants::Read)?;
    let mut drift = Report { scan: stats, mode: "verify".into(), ..Default::default() };
    let listing = list_main(&storage)?;
    drift.refused = refused.iter().chain(&listing.refused).map(|not| not.to_string()).collect();
    for p in listing.files.keys() {
        match local.get(p) {
            None => drift.conflicts.push(format!("{p}: in the repo, missing locally")),
            Some(lf) => {
                let bytes = storage.read_file(p.as_str(), listing.rev())?;
                if crate::blobs::content_sha(&bytes) != lf.sha256 {
                    drift.conflicts.push(format!("{p}: content differs from the repo"));
                }
            }
        }
    }
    for p in local.keys() {
        if !listing.files.contains_key(p) {
            drift.conflicts.push(format!("{p}: not in the repo"));
        }
    }
    Ok(drift)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::tests::{client_for, opts, tmpdir};
    use crate::sync::{sync_once, Mode};
    use std::fs;

    #[test]
    fn verify_catches_drift() {
        let mock = crate::mockcs::start();
        mock.seed_repo("t--k3x9", &[("a.txt", b"same"), ("b.txt", b"will-drift")]);
        let c = client_for(&mock);
        let dir = tmpdir("verify");
        sync_once(&c, "t--k3x9", &dir, &opts(Mode::Pull)).unwrap();
        let clean = verify(&c, "t--k3x9", &dir).unwrap();
        assert!(clean.conflicts.is_empty(), "{:?}", clean.conflicts);
        assert_eq!(clean.exit_code(), 0, "a folder in sync exits 0");
        // same size, different content — the exact lie the audit exists for
        fs::write(dir.join("b.txt"), b"went-drft").unwrap();
        let report = verify(&c, "t--k3x9", &dir).unwrap();
        assert!(report.conflicts.iter().any(|c| c.starts_with("b.txt")), "{:?}", report.conflicts);
        assert!(report.conflicts.iter().all(|c| !c.starts_with("a.txt")));
        assert_eq!(report.exit_code(), 3, "drift exits as a conflict does");
        // the audit reads a folder sync refuses (Codex on #1445): a browser
        // profile made in it is drift to report, not a reason to stop
        fs::create_dir_all(dir.join("chrome/Default")).unwrap();
        fs::write(dir.join("chrome/Default/Cookies"), b"x").unwrap();
        let report = verify(&c, "t--k3x9", &dir).unwrap();
        assert!(report.conflicts.iter().any(|c| c == "chrome/Default/Cookies: not in the repo"), "{:?}", report.conflicts);
        fs::remove_dir_all(&dir).ok();
    }
}
