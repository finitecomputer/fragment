//! The computer's Python environment (docs/computers.md, "Python in our
//! Hermes image"): one virtual environment its agents' `pip`, `python` and
//! `uv pip` use, made by the boot and kept with their work.

use std::path::{Path, PathBuf};

/// Where it is: under the work, as what an agent installs is its tools'
/// work, saved with the rest of it. One for the computer: its agents run as
/// one user (decision 44). Its login shells' `VIRTUAL_ENV` (the image's
/// `/etc/profile.d/fragment-python.sh`) names it.
pub const DIR: &str = "/data/work/python";

/// The image's Python, by the name upstream links it at: the environment
/// runs whichever Python the image holds there, so an image whose Python is
/// a patch release newer keeps it, packages and all.
pub const PYTHON: &str = "/usr/local/bin/python3";

/// Hermes' own uv, linked by the image (its Dockerfile).
pub const UV: &str = "/usr/local/bin/uv";

/// The image's own Python packages (the document skills' libraries,
/// Playwright), installed at its build apart from Hermes' store, whose
/// Python it records a digest of. The environment reads them after its own
/// (`image_pth`), so an agent's install of one wins.
pub const IMAGE_PACKAGES: &str = "/opt/fragment/python";

/// The `.pth` that names `IMAGE_PACKAGES` in an environment of Python
/// `minor`, written at each start.
pub fn image_pth(dir: &Path, minor: &str) -> PathBuf {
    dir.join(format!("lib/python{minor}/site-packages/fragment-image.pth"))
}

/// What a start does with the environment.
#[derive(Debug, PartialEq)]
pub enum Plan {
    /// Whole, and of the image's Python: nothing.
    Keep,
    /// None yet (or an empty directory).
    Make,
    /// Made, but without its `pip` (a start cut while it was made).
    AddPip,
    /// Of another Python (`made_for`, `None` when unreadable): set aside
    /// whole, and a new one made. Its packages were built for that Python.
    SetAside(Option<String>),
}

/// Binds an environment of the image's Python (`image`, `major.minor`) to
/// it by the name every image links it at: `bin/python` a link to
/// `PYTHON`, and `pyvenv.cfg`'s `home` that link's directory. Whatever made
/// it (uv keeps the name it is given; another may write the store's own
/// path, which an image update of the same minor release removes), a start
/// keeps it on that name before it plans. One of another Python, or none,
/// is left as it is. True when it changed.
pub fn rebind(dir: &Path, image: &str) -> std::io::Result<bool> {
    let cfg_path = dir.join("pyvenv.cfg");
    let Ok(cfg) = std::fs::read_to_string(&cfg_path) else { return Ok(false) };
    if made_for(&cfg).as_deref() != Some(image) {
        return Ok(false);
    }
    let home = Path::new(PYTHON).parent().expect("PYTHON names a file in a directory").display().to_string();
    let mut changed = false;
    let mut lines: Vec<String> = Vec::new();
    for line in cfg.lines() {
        match line.split_once('=') {
            Some((k, v)) if k.trim() == "home" && v.trim() != home => {
                changed = true;
                lines.push(format!("home = {home}"));
            }
            _ => lines.push(line.to_string()),
        }
    }
    if changed {
        let next = dir.join("pyvenv.cfg.rebind");
        std::fs::write(&next, lines.join("\n") + "\n")?;
        std::fs::rename(&next, &cfg_path)?;
    }
    let link = dir.join("bin/python");
    if std::fs::read_link(&link).ok().as_deref() != Some(Path::new(PYTHON)) {
        match std::fs::remove_file(&link) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
        std::fs::create_dir_all(dir.join("bin"))?;
        std::os::unix::fs::symlink(PYTHON, &link)?;
        changed = true;
    }
    Ok(changed)
}

/// What a start does with `dir`, the image's Python being `image`
/// (`major.minor`), after `rebind`.
pub fn plan(dir: &Path, image: &str) -> Plan {
    match std::fs::symlink_metadata(dir) {
        Err(_) => return Plan::Make,
        Ok(m) if m.is_dir() && std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_none()) => return Plan::Make,
        Ok(_) => {}
    }
    let made = std::fs::read_to_string(dir.join("pyvenv.cfg")).ok().and_then(|cfg| made_for(&cfg));
    match made {
        Some(made) if made == image && std::fs::symlink_metadata(dir.join("bin/python")).is_ok() => {
            if dir.join("bin/pip").exists() {
                Plan::Keep
            } else {
                Plan::AddPip
            }
        }
        other => Plan::SetAside(other),
    }
}

/// The Python an environment was made for (its `pyvenv.cfg`'s
/// `version_info`), as `major.minor`.
pub fn made_for(cfg: &str) -> Option<String> {
    cfg.lines().find_map(|l| l.split_once('=').filter(|(k, _)| k.trim() == "version_info").map(|(_, v)| v.trim().to_string())).and_then(|v| minor(&v))
}

/// `3.14.7` as `3.14`.
pub fn minor(version: &str) -> Option<String> {
    let mut parts = version.trim().split('.');
    let (major, minor) = (parts.next()?, parts.next()?);
    (major.parse::<u32>().is_ok() && minor.parse::<u32>().is_ok()).then(|| format!("{major}.{minor}"))
}

/// The name an environment of another Python is set aside as, beside it:
/// `python.3.13`, or `python.unknown` (the first of those, `-1`… after, not
/// taken).
pub fn aside(dir: &Path, made: Option<&str>) -> Option<PathBuf> {
    let base = format!("{}.{}", dir.display(), made.unwrap_or("unknown"));
    (0..100).map(|n| PathBuf::from(if n == 0 { base.clone() } else { format!("{base}-{n}") })).find(|p| std::fs::symlink_metadata(p).is_err())
}

/// The packages an environment held (`name==version`, one a line, pip's
/// own left out), from its `.dist-info` directories: its Python is gone, so
/// none is asked. What `pip install -r` takes, to install them again.
pub fn requirements(env: &Path) -> String {
    let mut found: Vec<String> = Vec::new();
    for lib in std::fs::read_dir(env.join("lib")).into_iter().flatten().flatten() {
        for entry in std::fs::read_dir(lib.path().join("site-packages")).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(dist) = name.strip_suffix(".dist-info") else { continue };
            // `{name}-{version}`: a distribution's name has no `-` there
            let Some((name, version)) = dist.split_once('-') else { continue };
            if !name.eq_ignore_ascii_case("pip") {
                found.push(format!("{name}=={version}"));
            }
        }
    }
    found.sort();
    found.into_iter().map(|l| l + "\n").collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hermes-boot-python-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// An environment as uv makes one: its `pyvenv.cfg`, its `python`, and
    /// (`pip`) pip's script.
    fn env_of(dir: &Path, version: &str, pip: bool) {
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("pyvenv.cfg"), format!("home = /usr/local/bin\nimplementation = CPython\nuv = 0.12.3\nversion_info = {version}\ninclude-system-site-packages = false\n")).unwrap();
        let _ = std::fs::remove_file(dir.join("bin/python"));
        std::os::unix::fs::symlink(PYTHON, dir.join("bin/python")).unwrap();
        if pip {
            std::fs::write(dir.join("bin/pip"), "").unwrap();
        }
    }

    /// Goal: a start keeps an environment of the image's Python, a patch
    /// release apart included (valid); makes one where there is none, or an
    /// empty directory (a first start); adds pip to one a cut start left
    /// without it (restart); and sets aside one of another Python, or one
    /// it cannot read (invalid).
    #[test]
    fn a_start_keeps_makes_mends_or_sets_aside() {
        let root = tmp("plan");
        let dir = root.join("python");
        assert_eq!(plan(&dir, "3.14"), Plan::Make, "none yet");
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(plan(&dir, "3.14"), Plan::Make, "an empty directory");
        env_of(&dir, "3.14.7", true);
        assert_eq!(plan(&dir, "3.14"), Plan::Keep);
        env_of(&dir, "3.14.2", true);
        assert_eq!(plan(&dir, "3.14"), Plan::Keep, "a patch release apart");
        std::fs::remove_file(dir.join("bin/pip")).unwrap();
        assert_eq!(plan(&dir, "3.14"), Plan::AddPip, "a start cut before pip");
        env_of(&dir, "3.13.5", true);
        assert_eq!(plan(&dir, "3.14"), Plan::SetAside(Some("3.13".into())));
        std::fs::write(dir.join("pyvenv.cfg"), "nonsense").unwrap();
        assert_eq!(plan(&dir, "3.14"), Plan::SetAside(None), "unreadable");
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::write(&dir, "a file").unwrap();
        assert_eq!(plan(&dir, "3.14"), Plan::SetAside(None), "not a directory");
        std::fs::remove_dir_all(&root).ok();
    }

    /// Goal (Codex on #989): a kept environment runs the image's Python by
    /// its stable name, whatever made it. Valid: one uv made (the name kept)
    /// is left alone. Invalid: one bound to the store's own path (another
    /// uv's way), or with no `bin/python`, is rebound, and the rest of its
    /// `pyvenv.cfg` kept. Replay: a second rebind changes nothing. One of
    /// another Python, or unreadable, is left for `plan` to set aside.
    #[test]
    fn a_kept_environment_is_bound_to_the_stable_name() {
        let root = tmp("rebind");
        let dir = root.join("python");
        env_of(&dir, "3.14.7", true);
        assert!(!rebind(&dir, "3.14").unwrap(), "uv's own: nothing to do");

        let store = "/opt/hermes/tools/python-3.14.7+20260901-linux-x64/bin";
        std::fs::write(dir.join("pyvenv.cfg"), format!("home = {store}\nimplementation = CPython\nversion_info = 3.14.7\ninclude-system-site-packages = false\n")).unwrap();
        std::fs::remove_file(dir.join("bin/python")).unwrap();
        std::os::unix::fs::symlink(format!("{store}/python3.14"), dir.join("bin/python")).unwrap();
        assert!(rebind(&dir, "3.14").unwrap());
        assert_eq!(std::fs::read_link(dir.join("bin/python")).unwrap(), PathBuf::from(PYTHON));
        assert_eq!(std::fs::read_to_string(dir.join("pyvenv.cfg")).unwrap(), "home = /usr/local/bin\nimplementation = CPython\nversion_info = 3.14.7\ninclude-system-site-packages = false\n");
        assert!(!rebind(&dir, "3.14").unwrap(), "replay: bound already");
        assert_eq!(plan(&dir, "3.14"), Plan::Keep);

        std::fs::remove_file(dir.join("bin/python")).unwrap();
        assert!(rebind(&dir, "3.14").unwrap(), "no bin/python: linked");
        assert_eq!(std::fs::read_link(dir.join("bin/python")).unwrap(), PathBuf::from(PYTHON));

        let other = format!("home = {store}\nversion_info = 3.13.5\n");
        std::fs::write(dir.join("pyvenv.cfg"), &other).unwrap();
        assert!(!rebind(&dir, "3.14").unwrap());
        assert_eq!(std::fs::read_to_string(dir.join("pyvenv.cfg")).unwrap(), other, "another Python's is left for plan");
        std::fs::remove_file(dir.join("pyvenv.cfg")).unwrap();
        assert!(!rebind(&dir, "3.14").unwrap(), "unreadable: left for plan");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn its_python_is_read_as_major_minor() {
        assert_eq!(made_for("home = /usr/local/bin\nversion_info = 3.14.7\n").as_deref(), Some("3.14"));
        assert_eq!(made_for("version_info=3.13.0rc1").as_deref(), Some("3.13"));
        for bad in ["", "version = 3.14.7", "version_info = three", "version_info = 3"] {
            assert_eq!(made_for(bad), None, "{bad:?}");
        }
        assert_eq!(minor("3.14.7\n").as_deref(), Some("3.14"));
        assert_eq!(image_pth(Path::new(DIR), "3.14"), PathBuf::from("/data/work/python/lib/python3.14/site-packages/fragment-image.pth"));
    }

    /// Goal: one set aside is named for its Python, never over another set
    /// aside before it, and lists what it held for `pip install -r`.
    /// Replay: a second set-aside of the same Python takes the next name.
    #[test]
    fn one_set_aside_keeps_its_name_and_its_list() {
        let root = tmp("aside");
        let dir = root.join("python");
        assert_eq!(aside(&dir, Some("3.13")), Some(root.join("python.3.13")));
        std::fs::create_dir_all(root.join("python.3.13")).unwrap();
        assert_eq!(aside(&dir, Some("3.13")), Some(root.join("python.3.13-1")), "never over the one before");
        assert_eq!(aside(&dir, None), Some(root.join("python.unknown")));

        let site = dir.join("lib/python3.13/site-packages");
        for d in ["requests-2.32.5.dist-info", "pip-26.2.1.dist-info", "playwright_stealth-2.0.0.dist-info", "requests", "__pycache__"] {
            std::fs::create_dir_all(site.join(d)).unwrap();
        }
        assert_eq!(requirements(&dir), "playwright_stealth==2.0.0\nrequests==2.32.5\n");
        assert_eq!(requirements(&root.join("none")), "", "nothing held, nothing listed");
        std::fs::remove_dir_all(&root).ok();
    }
}
