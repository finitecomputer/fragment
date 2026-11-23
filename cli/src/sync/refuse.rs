//! The folders sync refuses (FIN-401): a system folder, the home folder, a browser profile.

use super::scan::walk;
use crate::api::{Code, CodedError};
use std::path::{Path, PathBuf};

/// Why `fragment sync` and `fragment deploy` refuse a folder (FIN-401):
/// one whose whole tree is never what a person means to publish. An
/// agent's terminal starts each command in its work folder, so a guide's
/// `cd` does not carry over and `--dir .` once sent that folder's browser
/// profile, cookies included, into a brain.
#[derive(Debug, PartialEq, Eq)]
pub enum DirRefused {
    /// `/` or `/tmp`.
    System(PathBuf),
    /// The home folder, or a folder it is inside of (on a computer the
    /// agent's work folder, `/data/work` and `/data`).
    Home { dir: PathBuf, home: PathBuf },
    /// A browser profile in the folder or one of its subfolders.
    BrowserProfile { dir: PathBuf, found: PathBuf },
}

impl std::fmt::Display for DirRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let what = match self {
            DirRefused::System(dir) => format!("{} is a system folder", dir.display()),
            DirRefused::Home { dir, home } => format!("{} is your home folder ({}) or holds it", dir.display(), home.display()),
            DirRefused::BrowserProfile { dir, found } => format!("{} holds a browser profile, with its cookies and sign-ins ({})", dir.display(), found.display()),
        };
        write!(f, "refusing to sync: {what}. Point --dir at the folder you mean, by its full path")
    }
}
impl std::error::Error for DirRefused {}
impl From<DirRefused> for CodedError {
    /// A folder to point elsewhere: a usage mistake, exit 2.
    fn from(e: DirRefused) -> Self {
        CodedError { code: Code::InvalidUsage, msg: e.to_string() }
    }
}

/// Refuses `dir` (`DirRefused`) when it is `/`, `/tmp`, `home` or a folder
/// above `home` (`refuse_place`), or holds a browser profile at its top
/// level or in a subfolder (`browser_profile`). It reads those levels
/// only, and only what sync would send (`walk`). This is a command's check
/// before it takes the folder; each pass decides from its own scan
/// (`scan_local`), so what it sends is what it checked.
pub fn refuse_dir(dir: &Path, home: Option<&Path>) -> Result<(), DirRefused> {
    refuse_place(dir, home)?;
    let Ok(dir) = dir.canonicalize() else { return Ok(()) };
    let profile = |e: &ignore::DirEntry| browser_profile(e.path(), e.depth(), e.file_type().is_some_and(|t| t.is_file()));
    match walk(&dir).max_depth(Some(3)).build().flatten().find(profile) {
        Some(found) => Err(DirRefused::BrowserProfile { dir, found: found.into_path() }),
        None => Ok(()),
    }
}

/// Refuses `dir` when it is `/`, `/tmp`, `home` or a folder above `home`,
/// by its path alone: nothing in it is read. `home` is taken both as given
/// and with its links resolved: a Hermes terminal's `HOME` is a link into
/// the agent's work folder. A folder not there yet holds nothing.
pub(super) fn refuse_place(dir: &Path, home: Option<&Path>) -> Result<(), DirRefused> {
    let Ok(dir) = dir.canonicalize() else { return Ok(()) };
    let system = [Path::new("/"), Path::new("/tmp")];
    if system.iter().any(|s| *s == dir || s.canonicalize().is_ok_and(|s| s == dir)) {
        return Err(DirRefused::System(dir));
    }
    for h in home.into_iter().flat_map(|h| [Some(h.to_path_buf()), h.canonicalize().ok()]).flatten() {
        if h.starts_with(&dir) {
            return Err(DirRefused::Home { dir, home: h });
        }
    }
    Ok(())
}

/// Whether `path`, `depth` levels into a folder sync walks (`walk`), marks
/// a browser profile in it or a subfolder: a `Default/Cookies`, a `Local
/// State` or an `org.chromium.Chromium.*`. A profile's folder is at depth
/// 0 or 1, so its `Default/Cookies` is at 2 or 3. `path` is under the
/// folder as it resolves (`canonicalize`): `.` from inside a profile's
/// `Default` names no `Default`, and a link is the folder it leads to.
pub(super) fn browser_profile(path: &Path, depth: usize, file: bool) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
    let in_default = path.parent().and_then(Path::file_name).is_some_and(|p| p == "Default");
    let cookies = file && depth <= 3 && name == "Cookies" && in_default;
    let state = file && depth <= 2 && name == "Local State";
    let chromium = depth <= 2 && name.starts_with("org.chromium.Chromium.");
    cookies || state || chromium
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::tests::{git_folder, tmpdir};
    use crate::sync::{read_local, SyncError};
    use std::fs;

    /// Whether a pass's read of `dir` refuses it for a browser profile: its
    /// scan decides from its own walk what `refuse_dir` decides from its.
    fn pass_refuses(dir: &Path) -> bool {
        matches!(read_local(dir, "t--k3x9"), Err(SyncError::Refused(DirRefused::BrowserProfile { .. })))
    }

    /// The incident's folders: an agent's work folder holding its home and
    /// its browser profile, under `/data/work`.
    fn computer(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root = tmpdir(tag).canonicalize().unwrap();
        let work = root.join("data/work/juniper--k3x9");
        let home = work.join("home");
        fs::create_dir_all(home.join("brains/health/wikis")).unwrap();
        fs::write(home.join("brains/health/wikis/sleep.md"), b"# Sleep").unwrap();
        (root, work, home)
    }

    #[test]
    fn refuse_dir_refuses_root_and_tmp() {
        let (_, _, home) = computer("refuse-sys");
        assert!(matches!(refuse_dir(Path::new("/"), Some(&home)), Err(DirRefused::System(_))));
        assert!(matches!(refuse_dir(Path::new("/tmp"), Some(&home)), Err(DirRefused::System(_))));
        assert!(matches!(refuse_dir(Path::new("/"), None), Err(DirRefused::System(_))));
    }

    #[test]
    fn refuse_dir_refuses_home_and_every_folder_above_it() {
        let (root, work, home) = computer("refuse-home");
        for dir in [home.clone(), work.clone(), root.join("data/work"), root.join("data"), root.clone()] {
            let refused = refuse_dir(&dir, Some(&home));
            assert!(matches!(refused, Err(DirRefused::Home { .. })), "{}: {refused:?}", dir.display());
        }
        // `.` from inside it is the same folder
        let rel = home.join("brains/..").join("..");
        assert!(matches!(refuse_dir(&rel, Some(&home)), Err(DirRefused::Home { .. })));
        let shown = refuse_dir(&work, Some(&home)).unwrap_err().to_string();
        assert!(shown.contains("home folder") && shown.ends_with("Point --dir at the folder you mean, by its full path"), "{shown}");
    }

    #[test]
    fn refuse_dir_resolves_a_linked_home() {
        // Hermes: HOME is the profile's `home`, a link to the work folder's
        let (root, work, home) = computer("refuse-link");
        let profile = root.join("data/hermes/profiles/juniper--k3x9");
        fs::create_dir_all(&profile).unwrap();
        std::os::unix::fs::symlink(&home, profile.join("home")).unwrap();
        let linked = profile.join("home");
        assert!(matches!(refuse_dir(&work, Some(&linked)), Err(DirRefused::Home { .. })));
        assert!(matches!(refuse_dir(&profile, Some(&linked)), Err(DirRefused::Home { .. })));
        assert_eq!(refuse_dir(&linked.join("brains/health"), Some(&linked)), Ok(()));
    }

    #[test]
    fn refuse_dir_refuses_a_browser_profile_two_levels_down() {
        let base = tmpdir("refuse-browser").canonicalize().unwrap();
        let elsewhere = base.join("home-elsewhere");
        let cases: [(&str, &[&str]); 5] = [
            ("cookies-below", &["browser-profile/Default/Cookies"]),
            ("cookies-here", &["Default/Cookies"]),
            ("state-here", &["Local State"]),
            ("state-below", &["chrome-profile/Local State"]),
            ("chromium-temp", &["org.chromium.Chromium.a1b2c3/SingletonLock"]),
        ];
        for (tag, files) in cases {
            let dir = base.join(tag);
            for f in files {
                fs::create_dir_all(dir.join(f).parent().unwrap()).unwrap();
                fs::write(dir.join(f), b"x").unwrap();
            }
            let refused = refuse_dir(&dir, Some(&elsewhere));
            assert!(matches!(refused, Err(DirRefused::BrowserProfile { .. })), "{tag}: {refused:?}");
            assert!(pass_refuses(&dir), "{tag}: a pass refuses it too");
        }
        let below = base.join("temp-below");
        fs::create_dir_all(below.join("work/org.chromium.Chromium.zz")).unwrap();
        assert!(matches!(refuse_dir(&below, Some(&elsewhere)), Err(DirRefused::BrowserProfile { .. })));
        assert!(pass_refuses(&below), "an empty profile folder, in a pass too");
        // a folder is judged as it resolves (Codex on #1445): `.` from inside
        // a profile's `Default` or its temp folder (here `sub/..`) names neither
        for inside in ["cookies-here/Default", "chromium-temp/org.chromium.Chromium.a1b2c3"] {
            fs::create_dir_all(base.join(inside).join("sub")).unwrap();
            let dotted = base.join(inside).join("sub/..");
            assert!(matches!(refuse_dir(&dotted, Some(&elsewhere)), Err(DirRefused::BrowserProfile { .. })), "{inside}");
            assert!(pass_refuses(&dotted), "{inside}: a pass refuses it too");
        }
        let shown = refuse_dir(&base.join("cookies-below"), Some(&elsewhere)).unwrap_err().to_string();
        assert!(shown.contains("browser profile") && shown.contains("browser-profile"), "{shown}");
    }

    #[test]
    fn refuse_dir_passes_a_brain_a_site_and_a_new_folder() {
        let (root, _, home) = computer("refuse-pass");
        assert_eq!(refuse_dir(&home.join("brains/health"), Some(&home)), Ok(()));
        let site = root.join("apps/garden");
        fs::create_dir_all(site.join("site/assets")).unwrap();
        fs::write(site.join("fragment.json"), b"{}").unwrap();
        fs::write(site.join("site/index.html"), b"hi").unwrap();
        // a profile three levels down, or in a folder sync never sends
        fs::create_dir_all(site.join("site/assets/deep/Default")).unwrap();
        fs::write(site.join("site/assets/deep/Default/Cookies"), b"x").unwrap();
        fs::create_dir_all(site.join(".cache/Default")).unwrap();
        fs::write(site.join(".cache/Default/Cookies"), b"x").unwrap();
        assert_eq!(refuse_dir(&site, Some(&home)), Ok(()));
        assert_eq!(refuse_dir(&site, None), Ok(()));
        assert!(read_local(&site, "t--k3x9").is_ok(), "a pass passes it too");
        // a link named like a profile's temp folder is the folder it leads to
        let link = root.join("org.chromium.Chromium.link");
        std::os::unix::fs::symlink(&site, &link).unwrap();
        assert_eq!(refuse_dir(&link, Some(&home)), Ok(()));
        assert!(read_local(&link, "t--k3x9").is_ok(), "a pass passes it too");
        assert_eq!(refuse_dir(&root.join("not-yet"), Some(&home)), Ok(()));
        // a profile the folder's .gitignore keeps out, as the scan does
        let repo = git_folder("refuse-ignored", "browser-profile/");
        fs::write(repo.join("index.html"), b"hi").unwrap();
        fs::create_dir_all(repo.join("browser-profile/Default")).unwrap();
        fs::write(repo.join("browser-profile/Default/Cookies"), b"x").unwrap();
        fs::write(repo.join("browser-profile/Local State"), b"x").unwrap();
        assert_eq!(refuse_dir(&repo, Some(&home)), Ok(()));
        assert!(read_local(&repo, "t--k3x9").is_ok());
        // or keeps only the profile's files out
        let repo = git_folder("refuse-ignored-files", "browser-profile/Default/*\nbrowser-profile/Local State");
        fs::create_dir_all(repo.join("browser-profile/Default")).unwrap();
        fs::write(repo.join("browser-profile/Default/Cookies"), b"x").unwrap();
        fs::write(repo.join("browser-profile/Local State"), b"x").unwrap();
        fs::write(repo.join("browser-profile/notes.md"), b"x").unwrap();
        assert_eq!(refuse_dir(&repo, Some(&home)), Ok(()));
        assert!(read_local(&repo, "t--k3x9").is_ok());
    }

    /// Goal (Codex on #1445): a pass ends its walk at a browser profile, so
    /// nothing in it is read: a file there no one may read still gives the
    /// refusal, which stops a watch, not an error it retries forever.
    /// Method: a profile's temp folder holding a file with no permissions.
    #[cfg(unix)]
    #[test]
    fn a_pass_reads_nothing_past_a_profile() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmpdir("refuse-unread");
        fs::write(dir.join("note.md"), b"n").unwrap();
        let locked = dir.join("org.chromium.Chromium.zz/locked");
        fs::create_dir_all(locked.parent().unwrap()).unwrap();
        fs::write(&locked, b"x").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        assert!(fs::read(&locked).is_err(), "the file can't be read (a test not run as root)");
        assert!(pass_refuses(&dir), "{:?}", read_local(&dir, "t--k3x9").err());
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o600)).unwrap();
        fs::remove_dir_all(&dir).ok();
    }

    /// Goal (FIN-401): `fragment sync` decides its refusal from the files
    /// it sends, so a browser profile that comes into the folder while a
    /// pass reads it (`PROFILE_CHANGES`) is refused, and nothing is sent.
    /// Method: a folder whose profile `.gitignore` keeps out syncs once;
    /// the next pass is held at its journal while the folder changes
    /// (`while_a_pass_reads`). A guard that walked the folder before the
    /// scan sent the profile every time.
    #[cfg(unix)]
    #[test]
    fn a_sync_refuses_a_profile_that_comes_while_it_reads() {
        use crate::sync::tests::{client_for, opts, while_a_pass_reads, PROFILE_CHANGES};
        use crate::sync::{sync_once, Mode, SyncError};
        for (tag, change) in PROFILE_CHANGES {
            let mock = crate::mockcs::start();
            mock.seed_repo("t--k3x9", &[]);
            let dir = git_folder(&format!("comes-{tag}"), "browser/");
            fs::create_dir_all(dir.join("browser/Default")).unwrap();
            fs::write(dir.join("browser/Default/Cookies"), b"x").unwrap();
            fs::write(dir.join("note.md"), b"n").unwrap();
            let c = client_for(&mock);
            assert_eq!(sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)).unwrap().pushed, ["note.md"], "{tag}: an ignored profile is not sent");
            let packs = mock.commit_pack_count();
            let held = while_a_pass_reads(&dir, change, || sync_once(&c, "t--k3x9", &dir, &opts(Mode::Mirror)));
            assert!(matches!(held, Err(SyncError::Refused(DirRefused::BrowserProfile { .. }))), "{tag}: {held:?}");
            assert_eq!(mock.commit_pack_count(), packs, "{tag}: a refused pass sends nothing");
            assert_eq!(mock.file_at("t--k3x9", "main", "chrome/Default/Cookies"), None, "{tag}");
            assert_eq!(mock.file_at("t--k3x9", "main", "browser/Default/Cookies"), None, "{tag}");
            fs::remove_dir_all(&dir).ok();
        }
    }
}
