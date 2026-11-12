//! Where an agent's home keeps what an import reads: a layout. The CLI
//! knows one, `hermes` (a bare Hermes home, the whole source). Any other
//! is a file its skill carries, read here as data (`--layout <file>`), so
//! a platform's own paths live with the skill that imports from it and
//! leave with it (FIN-463 §6: the importer is generic, a platform's
//! layout and profile removable).

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{bail, Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

use crate::api::{Code, CodedError};

/// What a layout file names its format.
pub const FORMAT: &str = "fragment-home-layout/1";
/// A layout file, at most.
const FILE_MAX_BYTES: u64 = 64 * 1024;
/// Globs, parts and words one layout names, each at most.
const LIST_MAX: usize = 256;
/// One of them, at most.
const ITEM_MAX_BYTES: usize = 256;

/// A layout, as its file says it. Every path is relative to the source's
/// root, `/`-separated; `.` is the root itself.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    pub format: String,
    pub name: String,
    /// The Hermes home: its `state.db`, `SOUL.md`, `memories/`, `skills/`,
    /// `cron/` and `scripts/`.
    pub hermes_home: String,
    /// The agent's own files, which `fragment home files` brings (none:
    /// the agent kept none apart from its home).
    #[serde(default)]
    pub work: Option<String>,
    /// The settings file the survey reads its few public settings from,
    /// by name only (its time zone, personality and model). Default:
    /// `<hermes_home>/config.yaml`.
    #[serde(default)]
    pub config: Option<String>,
    /// The copies of its platform's managed skills the agent had, to tell
    /// an agent's own skill (or its edit of one of them) from theirs.
    #[serde(default)]
    pub managed_skills: Option<String>,
    /// Folders a profile brings its own way (a platform's brains, its
    /// sites): the survey lists each one's folders, and they are no part
    /// of `work`'s files.
    #[serde(default)]
    pub parts: BTreeMap<String, String>,
    /// Globs of the paths that hold secrets on its platform, beside the
    /// names any platform's do (`rules::secret_name`): a source holding
    /// one is refused.
    #[serde(default)]
    pub secret: Vec<String>,
    /// Words that name the old platform (`fsite`), case aside: each line of
    /// a skill or a job that holds one is listed, for the import to adapt.
    #[serde(default)]
    pub attention: Vec<String>,
}

fn usage(msg: String) -> anyhow::Error {
    anyhow::Error::new(CodedError { code: Code::InvalidUsage, msg })
}

impl Layout {
    /// A bare Hermes home: the whole source is its Hermes home.
    pub fn hermes() -> Layout {
        Layout {
            format: FORMAT.into(),
            name: "hermes".into(),
            hermes_home: ".".into(),
            work: None,
            config: None,
            managed_skills: None,
            parts: BTreeMap::new(),
            secret: Vec::new(),
            attention: Vec::new(),
        }
    }

    /// The layout `--layout` names: `hermes`, or a layout file.
    pub fn named(arg: &str) -> Result<Layout> {
        if arg == "hermes" {
            return Ok(Layout::hermes());
        }
        let path = Path::new(arg);
        if !path.is_file() {
            bail!(usage(format!("--layout is `hermes` or a layout file, and {arg} is neither")));
        }
        let size = std::fs::metadata(path)?.len();
        if size > FILE_MAX_BYTES {
            bail!(usage(format!("{arg} is {size} bytes: a layout file is at most {FILE_MAX_BYTES}")));
        }
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {arg}"))?;
        let layout: Layout = serde_json::from_str(&text).map_err(|e| usage(format!("{arg} is no layout: {e}")))?;
        layout.checked().map_err(|e| usage(format!("{arg}: {e}")))
    }

    fn checked(self) -> Result<Layout, String> {
        if self.format != FORMAT {
            return Err(format!("its format is {:?}, and this CLI reads {FORMAT:?}", self.format));
        }
        let paths = [Some(&self.hermes_home), self.work.as_ref(), self.config.as_ref(), self.managed_skills.as_ref()];
        for p in paths.into_iter().flatten().chain(self.parts.values()) {
            if p.starts_with('/') || p.split('/').any(|c| c == ".." || c.is_empty() && p != ".") || p.len() > ITEM_MAX_BYTES {
                return Err(format!("{p:?} is no path inside the source"));
            }
        }
        if [self.secret.len(), self.parts.len(), self.attention.len()].iter().any(|n| *n > LIST_MAX) {
            return Err(format!("it names more than {LIST_MAX} globs, parts or words"));
        }
        if self.attention.iter().chain(&self.secret).any(|w| w.trim().is_empty() || w.len() > ITEM_MAX_BYTES) {
            return Err(format!("a glob or word is empty, or longer than {ITEM_MAX_BYTES} bytes"));
        }
        self.secret_globs()?;
        Ok(self)
    }

    /// `rel` under `base` (`.` the root), as a source path.
    pub fn join(base: &str, rel: &str) -> String {
        match base.trim_end_matches('/') {
            "." | "" => rel.to_string(),
            b => format!("{b}/{rel}"),
        }
    }

    pub fn in_home(&self, rel: &str) -> String {
        Layout::join(&self.hermes_home, rel)
    }

    pub fn config_path(&self) -> String {
        self.config.clone().unwrap_or_else(|| self.in_home("config.yaml"))
    }

    /// The layout's secret paths, as one matcher.
    pub fn secret_globs(&self) -> Result<GlobSet, String> {
        let mut set = GlobSetBuilder::new();
        for g in &self.secret {
            set.add(Glob::new(g).map_err(|e| format!("{g:?} is no glob: {e}"))?);
        }
        set.build().map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::home::source::tests::Temp;

    #[test]
    fn a_layout_file_is_checked() {
        let t = Temp::new();
        let write = |body: &str| {
            let p = t.0.join("layout.json");
            std::fs::write(&p, body).unwrap();
            p.display().to_string()
        };
        let good = write(r#"{"format":"fragment-home-layout/1","name":"x","hermes_home":"agent/home","work":"work","parts":{"brains":"work/brains"},"secret":["agent/keys/**"],"attention":["oldtool"]}"#);
        let l = Layout::named(&good).unwrap();
        assert_eq!((l.in_home("state.db"), l.config_path()), ("agent/home/state.db".into(), "agent/home/config.yaml".into()));
        assert!(l.secret_globs().unwrap().is_match("agent/keys/a/b.json"));
        assert_eq!(Layout::named("hermes").unwrap().in_home("SOUL.md"), "SOUL.md");
        for (body, said) in [
            (r#"{"format":"other/1","name":"x","hermes_home":"."}"#, "its format is"),
            (r#"{"format":"fragment-home-layout/1","name":"x","hermes_home":"../up"}"#, "is no path inside the source"),
            (r#"{"format":"fragment-home-layout/1","name":"x","hermes_home":"/abs"}"#, "is no path inside the source"),
            (r#"{"format":"fragment-home-layout/1","name":"x","hermes_home":".","secret":["a/[b"]}"#, "is no glob"),
            (r#"{"format":"fragment-home-layout/1","name":"x","hermes_home":".","extra":1}"#, "unknown field"),
        ] {
            let e = Layout::named(&write(body)).expect_err(body);
            assert!(format!("{e:#}").contains(said), "{body}: {e:#}");
        }
        assert!(format!("{:#}", Layout::named("finite-computer").err().unwrap()).contains("is `hermes` or a layout file"));
    }

    /// Goal (FIN-463): the finite-computer layout the import skill ships
    /// never reads what its stage keeps for the computer itself: the old
    /// agent's SimpleX identity (`.finite-stage/simplex/`), which only its
    /// bridge adopts, at its owner's word (docs/migrations.md).
    #[test]
    fn the_finite_computer_layout_never_reads_the_stages_own_files() {
        let shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/../templates/skills/skills/productivity/import-agent-home/profiles/finite-computer.layout.json");
        let l = Layout::named(shipped).unwrap();
        let secret = l.secret_globs().unwrap();
        for p in [".finite-stage/simplex/chat.db", ".finite-stage/simplex/agent.db", ".finite-stage/simplex/owner.json"] {
            assert!(secret.is_match(p), "{p}");
        }
        assert!(!secret.is_match("workspace/notes.md"));
    }
}
