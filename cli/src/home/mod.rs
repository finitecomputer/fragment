//! `fragment home`: an agent's home brought here from wherever it ran
//! (FIN-463 §6). Generic and permanent: a source (`source.rs`: a folder,
//! or a zip of one) read through a layout (`layout.rs`: where that
//! platform keeps what an import reads). The judgment (what of a SOUL or a
//! memory still holds here, which skill to keep) is the managed skill
//! `import-agent-home`'s; these commands do what needs none.
//!
//! **An import is data.** Nothing here runs or follows anything a home
//! holds. A file whose path says it may hold a secret is left out of the
//! source unread, and named (`open`): the import never reads a secret.

mod files;
mod layout;
mod plan;
mod routines;
mod rules;
mod skills;
pub mod source;
mod survey;

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Subcommand;
use serde::Serialize;

use crate::api::{Code, CodedError};
pub use layout::Layout;
use source::Source;

#[derive(Subcommand)]
pub enum HomeCmd {
    /// What an import of an agent's home would bring, changing nothing:
    /// its chats, SOUL, memories, settings, skills (with where each came
    /// from), scheduled jobs as routines in UTC, and its files: what the
    /// cap brings and what stays behind, and why. SOURCE is the home's
    /// folder, or a zip of it
    Survey {
        source: PathBuf,
        /// Where the home keeps things: `hermes` (a bare Hermes home), or
        /// a layout file (a platform's, carried by its import skill)
        #[arg(long, default_value = "hermes")]
        layout: String,
    },
    /// Bring an agent's own files into a fragment of files you edit (make
    /// one with `fragment create <label> --visibility members`): each under
    /// `files/`, large ones as blobs, the cap's pick (photos, video, audio
    /// and documents first, the newest first, up to 10 GiB), and
    /// `left-behind.md` listing what stayed and why. Running it again goes
    /// on where it stopped, and sends nothing twice
    Files {
        source: PathBuf,
        /// The fragment to bring them into
        #[arg(long)]
        into: String,
        /// Only say what it would bring and leave; send nothing
        #[arg(long)]
        plan: bool,
        #[arg(long, default_value = "hermes")]
        layout: String,
    },
    /// Copy files and folders out of an agent's home into a folder of
    /// yours, as they are (a SOUL, a skill's folder, a site's files), to
    /// read and adapt them. PATHS are relative to the home's root
    Extract {
        source: PathBuf,
        #[arg(required = true)]
        paths: Vec<String>,
        /// The folder to copy into (made if missing): each path lands at
        /// its own place under it
        #[arg(long)]
        to: PathBuf,
        #[arg(long, default_value = "hermes")]
        layout: String,
    },
}

fn refused(msg: String) -> anyhow::Error {
    anyhow::Error::new(CodedError { code: Code::InvalidRequest, msg })
}

fn usage(msg: String) -> anyhow::Error {
    anyhow::Error::new(CodedError { code: Code::InvalidUsage, msg })
}

/// A home, read through `layout`, less every file that may hold a secret
/// (by its name, or the layout's globs): those are left out unread, and
/// named (`Source::skipped`). A person's own zip may hold one by mistake
/// (an `.npmrc`, a test key), and skipping it keeps it out of everything
/// an import makes as well as refusing would, while the rest moves.
pub fn open(path: &Path, layout: &Layout) -> Result<Source> {
    let mut src = Source::open(path)?;
    let globs = layout.secret_globs().map_err(|e| usage(format!("the layout {}: {e}", layout.name)))?;
    src.skip(|path| rules::secret_name(path.rsplit('/').next().unwrap_or(path)) || globs.is_match(path));
    Ok(src)
}

#[derive(Debug, Serialize)]
pub struct Extracted {
    pub to: String,
    pub files: usize,
    pub bytes: u64,
}

/// Each named file, and each file under each named folder, copied under
/// `to`. What it writes never lands in what it reads: `to` and the home
/// (its folder, or its zip) do not overlap, either way. Nothing is
/// written through a link: each folder on a file's way under `to` is a
/// folder of its own, and the file itself no link.
fn extract(src: &Source, paths: &[String], to: &Path) -> Result<Extracted> {
    let mut picked = Vec::new();
    for p in paths {
        let p = p.trim_matches('/');
        let found: Vec<_> = match src.get(p) {
            Some(e) => vec![e],
            None => src.under(p).collect(),
        };
        if found.is_empty() {
            bail!(usage(format!("{p:?} is no file or folder of this home")));
        }
        picked.extend(found);
    }
    std::fs::create_dir_all(to).with_context(|| format!("making {}", to.display()))?;
    let to = to.canonicalize().with_context(|| format!("reading {}", to.display()))?;
    let origin = src.origin().canonicalize().with_context(|| format!("reading {}", src.origin().display()))?;
    if to.starts_with(&origin) || origin.starts_with(&to) {
        bail!(usage(format!("--to {} and the home {} overlap: copy into a folder apart from it", to.display(), origin.display())));
    }
    let (mut files, mut bytes) = (0, 0);
    for e in picked {
        let target = target_under(&to, &e.path)?;
        // written beside it, then renamed over it: a file there already (a
        // link to one of the home's own, even) is replaced, never written into
        let beside = target.with_file_name(format!(".{}.fragment-{:016x}", target.file_name().and_then(|n| n.to_str()).unwrap_or("file"), rand::random::<u64>()));
        let copied = src.with_reader(&e.path, |r| {
            let mut out = std::fs::File::options().write(true).create_new(true).open(&beside).with_context(|| format!("writing {}", beside.display()))?;
            std::io::copy(r, &mut out).with_context(|| format!("copying {}", e.path))
        });
        let copied = copied.and_then(|n| std::fs::rename(&beside, &target).map(|_| n).with_context(|| format!("writing {}", target.display())));
        if copied.is_err() {
            let _ = std::fs::remove_file(&beside);
        }
        files += 1;
        bytes += copied?;
    }
    Ok(Extracted { to: to.display().to_string(), files, bytes })
}

/// Where a source's file (a plain path, source.rs) goes under `to`: each
/// folder on its way made, or a folder already (never a link to one), and
/// the place itself no link and no folder.
fn target_under(to: &Path, rel: &str) -> Result<PathBuf> {
    assert!(source::plain(rel), "a source lists plain paths: {rel:?}");
    let not_through = |at: &Path| refused(format!("{} is a link or not a folder: nothing is copied through it", at.display()));
    let parts: Vec<&str> = rel.split('/').collect();
    let mut at = to.to_path_buf();
    for dir in &parts[..parts.len() - 1] {
        at.push(dir);
        match std::fs::symlink_metadata(&at) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => bail!(not_through(&at)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir(&at).with_context(|| format!("making {}", at.display()))?,
            Err(e) => return Err(anyhow::Error::new(e).context(format!("reading {}", at.display()))),
        }
    }
    at.push(parts[parts.len() - 1]);
    match std::fs::symlink_metadata(&at) {
        Ok(m) if !m.is_file() => bail!(not_through(&at)),
        _ => Ok(at),
    }
}

/// What a `fragment home` command answered, for main to print.
pub enum Done {
    Survey(Box<survey::Survey>),
    Extracted(Extracted),
    Planned(Box<plan::Counted>),
    Brought(files::Brought),
}

impl Serialize for Done {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Done::Survey(v) => v.serialize(s),
            Done::Extracted(v) => v.serialize(s),
            Done::Planned(v) => v.serialize(s),
            Done::Brought(v) => v.serialize(s),
        }
    }
}

impl HomeCmd {
    /// Whether it calls the platform (and so needs a key to sign with).
    pub fn calls(&self) -> bool {
        matches!(self, HomeCmd::Files { plan: false, .. })
    }
}

/// `c`: the platform's client, for a command that `calls` it.
pub fn run(sub: HomeCmd, c: Option<&crate::api::Client>) -> Result<Done> {
    match sub {
        HomeCmd::Files { source, into, plan, layout } => {
            let layout = Layout::named(&layout)?;
            let src = open(&source, &layout)?;
            match c {
                Some(c) if !plan => Ok(Done::Brought(files::bring(c, &src, &layout, &into)?)),
                _ => Ok(Done::Planned(Box::new(files::planned(&src, &layout)?))),
            }
        }
        HomeCmd::Survey { source, layout } => {
            let layout = Layout::named(&layout)?;
            let src = open(&source, &layout)?;
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
            Ok(Done::Survey(Box::new(survey::survey(&src, &layout, now)?)))
        }
        HomeCmd::Extract { source, paths, to, layout } => {
            let layout = Layout::named(&layout)?;
            let src = open(&source, &layout)?;
            Ok(Done::Extracted(extract(&src, &paths, &to)?))
        }
    }
}

pub fn print(d: &Done) {
    match d {
        Done::Survey(s) => survey::print(s),
        Done::Extracted(e) => println!("{} files ({}) copied into {}", e.files, survey::size(e.bytes), e.to),
        Done::Planned(c) => files::print_planned(c),
        Done::Brought(b) => files::print(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use source::tests::Temp;

    /// The old agent's home every test reads: synthetic, laid out as
    /// finite.computer kept one (`crates/fakes/fixtures/old-home`).
    pub fn fixture() -> PathBuf {
        fragment_fakes::old_home::root()
    }

    /// The fixture's own layout, as a platform's skill carries one.
    fn fixture_layout() -> Layout {
        Layout::named(fragment_fakes::old_home::layout().to_str().unwrap()).unwrap()
    }

    /// Goal: a file that may hold a secret (by its name, or the layout's
    /// globs) is left out of the home: listed, never read, never copied
    /// out; the rest reads. Before, the whole home was refused.
    #[test]
    fn a_secret_is_skipped_never_read_and_named() {
        let t = Temp::new();
        let home = t.0.join("home");
        std::fs::create_dir_all(home.join("agent/keys")).unwrap();
        std::fs::write(home.join("SOUL.md"), "me").unwrap();
        std::fs::write(home.join("agent/keys/signing.txt"), "k").unwrap();
        std::fs::write(home.join("agent/notes.md"), "n").unwrap();
        std::fs::write(home.join(".env"), "OPENROUTER_API_KEY=x").unwrap();
        let layout = Layout { secret: vec!["agent/keys/**".into()], ..Layout::hermes() };
        let src = open(&home, &layout).unwrap();
        assert_eq!(src.skipped.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), [".env", "agent/keys/signing.txt"]);
        assert_eq!(src.entries().iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), ["SOUL.md", "agent/notes.md"]);
        assert!(src.read(".env").is_err() && src.read("agent/keys/signing.txt").is_err(), "never read");
        let out = t.0.join("out");
        assert_eq!(extract(&src, &["agent".into()], &out).unwrap().files, 1);
        assert!(!out.join("agent/keys").exists(), "never copied out");
    }

    /// Goal: a zip a person made holding one file shaped like a secret
    /// (an `.npmrc` in a project) surveys whole, less that file: its plan
    /// brings the rest, and names the file left behind, and why.
    #[test]
    fn a_zip_with_one_secret_surveys_the_rest() {
        let layout = fixture_layout();
        let dir = open(&fixture(), &layout).unwrap();
        let t = Temp::new();
        let zip = t.0.join("home.zip");
        let mut files: Vec<(String, Vec<u8>)> = dir.entries().iter().map(|e| (e.path.clone(), dir.read(&e.path).unwrap())).collect();
        files.push(("workspace/projects/app/.npmrc".into(), b"//registry.npmjs.org/:_authToken=x".to_vec()));
        source::tests::zip_of(&zip, &files.iter().map(|(p, b)| (p.as_str(), b.as_slice())).collect::<Vec<_>>());
        let s = survey::survey(&open(&zip, &layout).unwrap(), &layout, 1_794_225_600).unwrap();
        assert_eq!(s.source.skipped, ["workspace/projects/app/.npmrc"]);
        assert_eq!(s.files.brought.files, 7, "the fixture's own");
        let secret = &s.files.left_by_why[&plan::Why::Secret];
        assert_eq!(secret.largest.iter().map(|l| l.path.as_str()).collect::<Vec<_>>(), ["projects/app/.npmrc"]);
    }

    /// The fixture's survey, from its folder and from a zip of it: the
    /// same plan.
    #[test]
    fn the_fixture_surveyed() {
        let layout = fixture_layout();
        let dir = open(&fixture(), &layout).unwrap();
        let t = Temp::new();
        let zip = t.0.join("stage.zip");
        let files: Vec<(String, Vec<u8>)> = dir.entries().iter().map(|e| (e.path.clone(), dir.read(&e.path).unwrap())).collect();
        source::tests::zip_of(&zip, &files.iter().map(|(p, b)| (p.as_str(), b.as_slice())).collect::<Vec<_>>());
        let zipped = open(&zip, &layout).unwrap();
        let now = 1_794_225_600;
        let (a, b) = (survey::survey(&dir, &layout, now).unwrap(), survey::survey(&zipped, &layout, now).unwrap());
        let (mut ja, mut jb) = (serde_json::to_value(&a).unwrap(), serde_json::to_value(&b).unwrap());
        ja["source"]["kind"] = "-".into();
        jb["source"]["kind"] = "-".into();
        // a zip's times are its own (the fixture's files carry no times of
        // their own in git): the picks are the same set
        for j in [&mut ja, &mut jb] {
            j["files"]["left_by_why"] = serde_json::Value::Null;
        }
        assert_eq!(ja, jb, "a folder and its zip survey the same");
        assert_eq!(a.layout, "old-home");
        assert!(a.sessions.is_none(), "the fixture's state.db is made by the tests that read it");
        assert_eq!(a.soul.as_ref().map(|s| s.path.as_str()), Some("agent/hermes-home/SOUL.md"));
        assert_eq!(a.memories.iter().map(|m| (m.path.as_str(), m.entries)).collect::<Vec<_>>(), [("agent/hermes-home/memories/MEMORY.md", 3), ("agent/hermes-home/memories/USER.md", 2)]);
        let c = a.config.as_ref().unwrap();
        assert_eq!((c.timezone.as_deref(), c.model.as_deref()), (Some("America/New_York"), Some("old/model-large")));
        let skills: Vec<(&str, skills::Provenance)> = a.skills.listed.iter().map(|k| (k.name.as_str(), k.provenance)).collect();
        assert_eq!(skills, [
            ("helper", skills::Provenance::AgentCreated),
            ("recipe-tables", skills::Provenance::Custom),
            ("apps-old", skills::Provenance::Managed),
            ("brains-old", skills::Provenance::ModifiedManaged),
        ]);
        let recipe = &a.skills.listed[1];
        assert_eq!(recipe.attention.iter().map(|l| l.words.join(",")).collect::<Vec<_>>(), ["fsite"], "its one line naming the old platform");
        assert_eq!(a.skills.listed[3].changed, ["our-rules.md"]);
        let r = a.routines.as_ref().unwrap();
        let jobs: Vec<(&str, routines::Suggestion, Option<&str>)> = r.jobs.iter().map(|j| (j.name.as_deref().unwrap_or(""), j.suggestion, j.schedule_utc.as_deref())).collect();
        assert_eq!(jobs, [
            ("Morning brief", routines::Suggestion::Routine, Some("30 12 * * mon-fri")),
            ("Watch the feed", routines::Suggestion::TooFrequent, None),
            ("Old reminder", routines::Suggestion::Paused, None),
        ]);
        assert_eq!(a.parts["brains"].iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["brain-recipes"]);
        assert_eq!(a.parts["sites"].iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["garden"]);
        let f = &a.files;
        assert_eq!(f.root.as_deref(), Some("workspace"));
        let left: Vec<(plan::Why, usize)> = f.left_by_why.iter().map(|(w, r)| (*w, r.files)).collect();
        assert_eq!(left, [(plan::Why::Dependencies, 1), (plan::Why::BuildOutput, 1), (plan::Why::Never, 1)]);
        assert_eq!(f.brought.files, 7);
        assert!(f.brought.by_kind.contains_key(&rules::Kind::Photo) && f.brought.by_kind.contains_key(&rules::Kind::Document));
    }

    #[test]
    fn extract_copies_what_is_named() {
        let layout = fixture_layout();
        let src = open(&fixture(), &layout).unwrap();
        let t = Temp::new();
        let done = extract(&src, &["agent/hermes-home/SOUL.md".into(), "agent/hermes-home/skills/recipes/".into()], &t.0).unwrap();
        assert!(t.0.join("agent/hermes-home/SOUL.md").is_file());
        assert!(t.0.join("agent/hermes-home/skills/recipes/recipe-tables/SKILL.md").is_file());
        assert!(done.files >= 3);
        let e = extract(&src, &["nowhere".into()], &t.0).err().unwrap();
        assert!(format!("{e:#}").contains("is no file or folder of this home"));
    }

    /// Invalid: copying into the home itself, or into a folder holding its
    /// zip, or through a link left in the folder copied into. Nothing of
    /// the home or outside the folder changes.
    #[test]
    fn extract_writes_only_into_a_folder_of_its_own() {
        let t = Temp::new();
        let home = t.0.join("home");
        std::fs::create_dir_all(home.join("sub")).unwrap();
        std::fs::write(home.join("SOUL.md"), "me").unwrap();
        std::fs::write(home.join("sub/note.md"), "note").unwrap();
        let src = open(&home, &Layout::hermes()).unwrap();
        for to in [home.clone(), home.join("sub"), t.0.clone()] {
            let e = extract(&src, &["SOUL.md".into()], &to).err().unwrap_or_else(|| panic!("{} refused", to.display()));
            assert!(format!("{e:#}").contains("overlap"), "{e:#}");
        }
        assert_eq!(std::fs::read_to_string(home.join("SOUL.md")).unwrap(), "me", "the home is untouched");
        let zip = t.0.join("zips/home.zip");
        std::fs::create_dir_all(zip.parent().unwrap()).unwrap();
        source::tests::zip_of(&zip, &[("home.zip", b"a member named as its zip")]);
        let zipped = open(&zip, &Layout::hermes()).unwrap();
        assert!(format!("{:#}", extract(&zipped, &["home.zip".into()], zip.parent().unwrap()).err().unwrap()).contains("overlap"));
        // a link left in the folder copied into: to a file outside, and to a folder outside
        let dest = t.0.join("dest");
        let outside = t.0.join("outside");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("victim"), "keep").unwrap();
        std::os::unix::fs::symlink(outside.join("victim"), dest.join("SOUL.md")).unwrap();
        std::os::unix::fs::symlink(&outside, dest.join("sub")).unwrap();
        for p in ["SOUL.md", "sub/note.md"] {
            let e = extract(&src, &[p.into()], &dest).err().unwrap_or_else(|| panic!("{p} refused"));
            assert!(format!("{e:#}").contains("is a link or not a folder"), "{p}: {e:#}");
        }
        assert_eq!(std::fs::read_to_string(outside.join("victim")).unwrap(), "keep");
        assert!(!outside.join("note.md").exists(), "nothing written through the folder's link");
        // a hard link to the home's own file, left in the folder copied into:
        // replaced, and the home's file kept
        let linked = t.0.join("linked");
        std::fs::create_dir_all(&linked).unwrap();
        std::fs::hard_link(home.join("SOUL.md"), linked.join("SOUL.md")).unwrap();
        assert_eq!(extract(&src, &["SOUL.md".into()], &linked).unwrap().bytes, 2);
        assert_eq!(std::fs::read_to_string(home.join("SOUL.md")).unwrap(), "me", "the home's file, linked, is kept");
        assert_eq!(std::fs::read_to_string(linked.join("SOUL.md")).unwrap(), "me");
        // into a folder of its own, again: the same files, written over
        let fresh = t.0.join("fresh");
        for _ in 0..2 {
            assert_eq!(extract(&src, &["SOUL.md".into(), "sub".into()], &fresh).unwrap().files, 2);
        }
        assert_eq!(std::fs::read_to_string(fresh.join("sub/note.md")).unwrap(), "note");
        assert_eq!(std::fs::read_dir(fresh.join("sub")).unwrap().count(), 1, "nothing left beside it");
    }
}
