//! `fragment home`: an agent's home brought here from wherever it ran
//! (FIN-463 §6). Generic and permanent: a source (`source.rs`: a folder,
//! or a zip of one) read through a layout (`layout.rs`: where that
//! platform keeps what an import reads). The judgment (what of a SOUL or a
//! memory still holds here, which skill to keep) is the managed skill
//! `import-agent-home`'s; these commands do what needs none.
//!
//! **An import is data.** Nothing here runs or follows anything a home
//! holds. A source holding a path that may hold a secret is refused
//! whole (`open`): the import never reads a secret, and whatever staged the
//! home should have left it out.

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

/// A home, read through `layout`: refused whole if any path in it is one
/// that may hold a secret (by its name, or the layout's own globs).
pub fn open(path: &Path, layout: &Layout) -> Result<Source> {
    let src = Source::open(path)?;
    let globs = layout.secret_globs().map_err(|e| usage(format!("the layout {}: {e}", layout.name)))?;
    let secrets: Vec<String> = src.entries().iter().filter(|e| rules::secret_name(e.path.rsplit('/').next().unwrap_or(&e.path)) || globs.is_match(&e.path)).map(|e| e.path.clone()).collect();
    if !secrets.is_empty() {
        bail!(refused(format!(
            "{} holds files that may hold passwords or keys ({}): an import never reads them, so it reads none of this. Leave them out of it and try again",
            path.display(),
            source::said(&secrets)
        )));
    }
    Ok(src)
}

#[derive(Debug, Serialize)]
pub struct Extracted {
    pub to: String,
    pub files: usize,
    pub bytes: u64,
}

/// Each named file, and each file under each named folder, copied under `to`.
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
    let (mut files, mut bytes) = (0, 0);
    for e in picked {
        // a source's paths are relative names (source.rs): this stays under `to`
        let target = to.join(&e.path);
        assert!(target.strip_prefix(to).is_ok_and(|r| r.components().all(|c| matches!(c, std::path::Component::Normal(_)))), "a source path is relative: {}", e.path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).with_context(|| format!("making {}", parent.display()))?;
        }
        let copied = src.with_reader(&e.path, |r| {
            let mut out = std::fs::File::create(&target).with_context(|| format!("writing {}", target.display()))?;
            std::io::copy(r, &mut out).with_context(|| format!("copying {}", e.path))
        })?;
        files += 1;
        bytes += copied;
    }
    Ok(Extracted { to: to.display().to_string(), files, bytes })
}

/// What a `fragment home` command answered, for main to print.
pub enum Done {
    Survey(Box<survey::Survey>),
    Extracted(Extracted),
}

impl Serialize for Done {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Done::Survey(v) => v.serialize(s),
            Done::Extracted(v) => v.serialize(s),
        }
    }
}

pub fn run(sub: HomeCmd) -> Result<Done> {
    match sub {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use source::tests::Temp;

    /// The old agent's home every test reads: synthetic, laid out as
    /// finite.computer kept one (`crates/fakes/fixtures/old-home`).
    pub fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/fakes/fixtures/old-home")
    }

    /// The fixture's own layout, as a platform's skill carries one.
    fn fixture_layout() -> Layout {
        Layout::named(fixture().with_extension("layout.json").to_str().unwrap()).unwrap()
    }

    #[test]
    fn a_secret_refuses_the_whole_home() {
        let t = Temp::new();
        std::fs::create_dir_all(t.0.join("home/agent/keys")).unwrap();
        std::fs::write(t.0.join("home/SOUL.md"), "me").unwrap();
        let hermes = Layout::hermes();
        assert!(open(&t.0.join("home"), &hermes).is_ok());
        std::fs::write(t.0.join("home/agent/keys/signing.txt"), "k").unwrap();
        let layout = Layout { secret: vec!["agent/keys/**".into()], ..Layout::hermes() };
        let e = open(&t.0.join("home"), &layout).err().expect("refused by the layout's glob");
        assert!(format!("{e:#}").contains("\"agent/keys/signing.txt\""), "{e:#}");
        std::fs::write(t.0.join("home/.env"), "OPENROUTER_API_KEY=x").unwrap();
        let e = open(&t.0.join("home"), &hermes).err().expect("refused by its name");
        assert!(format!("{e:#}").contains("may hold passwords or keys (\".env\")"), "{e:#}");
        assert_eq!(e.downcast_ref::<CodedError>().map(|c| c.code), Some(Code::InvalidRequest));
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
}
