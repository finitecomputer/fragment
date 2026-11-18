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
/// above `home`, or holds a browser profile at its top level or in a
/// subfolder: a `Default/Cookies`, a `Local State` or an
/// `org.chromium.Chromium.*`. It reads those levels only, and only what
/// sync would send (`walk`). `home` is taken both as given and
/// with its links resolved: a Hermes terminal's `HOME` is a link into the
/// agent's work folder. A folder not there yet holds nothing.
pub fn refuse_dir(dir: &Path, home: Option<&Path>) -> Result<(), DirRefused> {
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
    // a profile's folder is at depth 0 or 1, so its `Default/Cookies` at 2 or 3
    let profile = |e: &ignore::DirEntry| {
        let name = e.file_name().to_string_lossy();
        let file = e.file_type().is_some_and(|t| t.is_file());
        let in_default = e.path().parent().and_then(Path::file_name).is_some_and(|p| p == "Default");
        let chromium = e.depth() <= 2 && name.starts_with("org.chromium.Chromium.");
        (file && name == "Cookies" && in_default) || (file && e.depth() <= 2 && name == "Local State") || chromium
    };
    match walk(&dir).max_depth(Some(3)).build().flatten().find(profile) {
        Some(found) => Err(DirRefused::BrowserProfile { dir, found: found.into_path() }),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::tests::{git_folder, tmpdir};
    use std::fs;

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
        }
        let below = base.join("temp-below");
        fs::create_dir_all(below.join("work/org.chromium.Chromium.zz")).unwrap();
        assert!(matches!(refuse_dir(&below, Some(&elsewhere)), Err(DirRefused::BrowserProfile { .. })));
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
        assert_eq!(refuse_dir(&root.join("not-yet"), Some(&home)), Ok(()));
        // a profile the folder's .gitignore keeps out, as the scan does
        let repo = git_folder("refuse-ignored", "browser-profile/");
        fs::write(repo.join("index.html"), b"hi").unwrap();
        fs::create_dir_all(repo.join("browser-profile/Default")).unwrap();
        fs::write(repo.join("browser-profile/Default/Cookies"), b"x").unwrap();
        fs::write(repo.join("browser-profile/Local State"), b"x").unwrap();
        assert_eq!(refuse_dir(&repo, Some(&home)), Ok(()));
        // or keeps only the profile's files out
        let repo = git_folder("refuse-ignored-files", "browser-profile/Default/*\nbrowser-profile/Local State");
        fs::create_dir_all(repo.join("browser-profile/Default")).unwrap();
        fs::write(repo.join("browser-profile/Default/Cookies"), b"x").unwrap();
        fs::write(repo.join("browser-profile/Local State"), b"x").unwrap();
        fs::write(repo.join("browser-profile/notes.md"), b"x").unwrap();
        assert_eq!(refuse_dir(&repo, Some(&home)), Ok(()));
    }
}
