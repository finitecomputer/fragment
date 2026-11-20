//! `cargo xtask skills-catalog`: the catalog of
//! Hermes' optional skills a person can switch on (the shell's Skills
//! page), written to `templates/skills/catalog/hermes.json`, the skills
//! template's data, so every skills fragment reads it from the release.
//!
//! It is read from the Hermes image images/hermes/Dockerfile pins (`ARG
//! HERMES`), the one the computers' image is built on, and only that one,
//! so the version it is stamped with is its own: its
//! `/opt/hermes/optional-skills` is copied out with Docker
//! (`docker create` and `docker cp`, nothing run in it). Each skill is a
//! `SKILL.md` at any depth there; its id is its directory's path, as the
//! boot finds it (images/hermes/boot/src/skills.rs, `settle_optional`).
//! Only the index is written (id, name, description, license, labels):
//! the tree is about 11 MB, past the template's bounds, and every
//! computer already has it.
//!
//! Run it again when the pin moves: crates/templates' test holds the
//! catalog's `hermes` to the Dockerfile's.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{ensure, Context, Result};
use serde::Serialize;

/// Where the catalog is written, under the repo.
pub const CATALOG: &str = "templates/skills/catalog/hermes.json";
/// Where Hermes' image keeps its optional skills.
const OPTIONAL_DIR: &str = "/opt/hermes/optional-skills";
/// Bounds: skills, how deep one sits, a description.
const SKILLS_MAX: usize = 1_000;
const DEPTH_MAX: usize = 4;
const DESCRIPTION_MAX: usize = 1_024;

/// Words the Skills page shows beside a skill, chosen by hand (they are not
/// in the skills' own files): what it can do that a person should know
/// before switching it on, since our agents ask no one before acting
/// (docs/computers.md, "Approvals in our Hermes image"). Nothing is hidden.
/// Each id must be in the catalog.
const LABELS: [(&str, &[&str]); 13] = [
    ("payments/stripe-link-cli", &["spends"]),
    ("payments/stripe-projects", &["spends"]),
    ("productivity/shop", &["spends"]),
    ("productivity/telephony", &["spends", "contacts"]),
    ("mlops/lambda-labs", &["spends"]),
    ("mlops/modal", &["spends"]),
    ("email/agentmail", &["contacts"]),
    ("security/unbroker", &["contacts"]),
    ("security/godmode", &["care"]),
    ("mlops/obliteratus", &["care"]),
    ("security/web-pentest", &["care"]),
    ("security/sherlock", &["care"]),
    ("research/osint-investigation", &["care"]),
];

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub license: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<&'static str>,
}

pub fn run(args: &[String]) -> Result<()> {
    let root = fragment_devstack::repo_root();
    let (reference, version) = pin(&std::fs::read_to_string(root.join("images/hermes/Dockerfile")).context("read images/hermes/Dockerfile")?)?;
    ensure!(args.is_empty(), "skills-catalog takes no arguments, not {args:?}");
    let image = reference;
    let scratch = root.join("target/skills-catalog");
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch)?;
    copy_out(&image, &scratch)?;
    let skills = read_tree(&scratch.join("optional-skills"))?;
    let text = render(&version, &skills);
    std::fs::create_dir_all(root.join(CATALOG).parent().expect("the catalog has a directory"))?;
    std::fs::write(root.join(CATALOG), &text).with_context(|| format!("write {CATALOG}"))?;
    let _ = std::fs::remove_dir_all(&scratch);
    println!("{CATALOG}: {} optional skills of Hermes {version} (from {image})", skills.len());
    Ok(())
}

/// The Dockerfile's Hermes pin, and the version its tag names
/// (`rc.<N>-vX.Y.Z-desktop`: `vX.Y.Z`).
pub fn pin(dockerfile: &str) -> Result<(String, String)> {
    let reference = dockerfile.lines().find_map(|l| l.strip_prefix("ARG HERMES=")).context("the Dockerfile pins no ARG HERMES")?.trim().to_string();
    let tag = reference.split_once(':').map(|(_, t)| t).and_then(|t| t.split('@').next()).context("the Hermes pin has no tag")?;
    let version = tag.split('-').find(|p| p.starts_with('v') && p[1..].split('.').count() == 3).with_context(|| format!("the Hermes pin's tag names no version: {tag}"))?.to_string();
    Ok((reference, version))
}

/// The image's optional skills, copied to `scratch/optional-skills`.
fn copy_out(image: &str, scratch: &Path) -> Result<()> {
    let out = Command::new("docker").args(["create", "--platform", "linux/amd64", image]).output().context("could not start docker")?;
    ensure!(out.status.success(), "docker create {image} failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    let id = String::from_utf8(out.stdout)?.trim().to_string();
    let copied = crate::run(Command::new("docker").args(["cp", "-q", &format!("{id}:{OPTIONAL_DIR}"), &scratch.display().to_string()]));
    let _ = Command::new("docker").args(["rm", "-f", &id]).output();
    copied
}

/// Every `SKILL.md` under `dir` (bounded), as the catalog lists it, by id.
pub fn read_tree(dir: &Path) -> Result<Vec<Skill>> {
    let mut found: Vec<PathBuf> = vec![];
    let mut pending = vec![(dir.to_path_buf(), 0)];
    while let Some((at, depth)) = pending.pop() {
        for entry in std::fs::read_dir(&at).with_context(|| format!("read {}", at.display()))? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                ensure!(depth < DEPTH_MAX, "a skill deeper than {DEPTH_MAX}: {}", entry.path().display());
                pending.push((entry.path(), depth + 1));
            } else if entry.file_name() == "SKILL.md" && at != dir {
                found.push(at.clone());
                ensure!(found.len() <= SKILLS_MAX, "more than {SKILLS_MAX} optional skills");
            }
        }
    }
    let mut skills = vec![];
    for at in found {
        let id = at.strip_prefix(dir).expect("under the tree").to_string_lossy().into_owned();
        let text = std::fs::read_to_string(at.join("SKILL.md")).with_context(|| format!("read {id}/SKILL.md"))?;
        skills.push(skill(&id, &text).with_context(|| format!("{id}/SKILL.md"))?);
    }
    skills.sort_by(|a, b| a.id.cmp(&b.id));
    let ids: BTreeSet<&str> = skills.iter().map(|s| s.id.as_str()).collect();
    for (id, _) in LABELS {
        ensure!(ids.contains(id), "a label names {id}, which the image has no longer");
    }
    Ok(skills)
}

/// One skill's entry from its `SKILL.md`: its frontmatter's `name` (else
/// its directory's), `description` and `license`, and its labels.
pub fn skill(id: &str, text: &str) -> Result<Skill> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text).replace("\r\n", "\n");
    let front = text.strip_prefix("---\n").and_then(|r| r.split_once("\n---")).map(|(f, _)| f).context("no frontmatter")?;
    let field = |key: &str| -> Result<String> {
        let Some(raw) = front.lines().find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix(':'))) else { return Ok(String::new()) };
        scalar(raw.trim())
    };
    let alias = id.rsplit('/').next().expect("an id has a name");
    let name = Some(field("name")?).filter(|n| !n.is_empty()).unwrap_or_else(|| alias.to_string());
    let description = field("description")?;
    ensure!(!description.is_empty(), "no description");
    ensure!(description.len() <= DESCRIPTION_MAX, "a description past {DESCRIPTION_MAX} bytes");
    let labels = LABELS.iter().find(|(l, _)| *l == id).map(|(_, l)| l.to_vec()).unwrap_or_default();
    Ok(Skill { id: id.to_string(), name, description, license: field("license")?, labels })
}

/// A one-line YAML scalar: double-quoted (as JSON reads it), single-quoted,
/// or plain. A block scalar is refused: none of the skills has one.
fn scalar(raw: &str) -> Result<String> {
    if raw.starts_with('"') {
        return serde_json::from_str(raw).with_context(|| format!("a double-quoted value JSON does not read: {raw}"));
    }
    if let Some(inner) = raw.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')) {
        return Ok(inner.replace("''", "'"));
    }
    ensure!(!raw.starts_with('|') && !raw.starts_with('>'), "a block scalar: {raw}");
    Ok(raw.to_string())
}

/// The catalog's text: its header, then one skill a line, so a pin's move
/// reads as a diff of the skills it changed.
pub fn render(version: &str, skills: &[Skill]) -> String {
    let lines: Vec<String> = skills.iter().map(|s| format!("    {}", serde_json::to_string(s).expect("a skill serializes"))).collect();
    let source = format!("https://github.com/NousResearch/hermes-agent/tree/{version}/optional-skills");
    format!(
        "{{\n  \"runtime\": \"hermes\",\n  \"hermes\": {},\n  \"source\": {},\n  \"skills\": [\n{}\n  ]\n}}\n",
        serde_json::to_string(version).expect("a string serializes"),
        serde_json::to_string(&source).expect("a string serializes"),
        lines.join(",\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pin_names_its_version() {
        let (reference, version) = pin("FROM x\nARG HERMES=nousresearch/hermes-agent:rc.13-v0.23.2-desktop@sha256:87f2\n").unwrap();
        assert_eq!((reference.as_str(), version.as_str()), ("nousresearch/hermes-agent:rc.13-v0.23.2-desktop@sha256:87f2", "v0.23.2"));
        assert!(pin("FROM x\n").is_err());
        let real = std::fs::read_to_string(fragment_devstack::repo_root().join("images/hermes/Dockerfile")).unwrap();
        assert!(pin(&real).is_ok(), "the Dockerfile's own pin reads");
    }

    /// Valid: a skill's name, description (each quoting) and license, and
    /// its labels by id. Invalid: no frontmatter, no description, a block
    /// scalar.
    #[test]
    fn a_skills_entry_is_its_frontmatters() {
        let s = skill("productivity/telephony", "---\nname: telephony\ndescription: \"Calls: \\\"and\\\" texts.\"\nlicense: MIT\nmetadata:\n  hermes:\n    tags: [x]\n---\n# body\ndescription: no\n").unwrap();
        assert_eq!(s, Skill { id: "productivity/telephony".into(), name: "telephony".into(), description: "Calls: \"and\" texts.".into(), license: "MIT".into(), labels: vec!["spends", "contacts"] });
        let s = skill("creative/sketch", "---\ndescription: 'It''s a sketch.'\n---\n").unwrap();
        assert_eq!((s.name.as_str(), s.description.as_str(), s.labels.len()), ("sketch", "It's a sketch.", 0));
        assert!(skill("a/b", "# no frontmatter\n").is_err());
        assert!(skill("a/b", "---\nname: b\n---\n").is_err(), "no description");
        assert!(skill("a/b", "---\ndescription: >\n  folded\n---\n").is_err(), "a block scalar");
    }

    /// A tree's skills at any depth, sorted by id, rendered one a line; a
    /// label naming a skill the tree lacks fails.
    #[test]
    fn a_tree_is_read_at_any_depth_and_rendered_a_skill_a_line() {
        let dir = std::env::temp_dir().join(format!("xtask-skills-catalog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let write = |rel: &str, text: &str| {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        write("DESCRIPTION.md", "# Optional skills\n");
        write("mlops/training/unsloth/SKILL.md", "---\nname: unsloth\ndescription: Fast fine-tuning.\n---\n");
        write("creative/excalidraw/SKILL.md", "---\nname: excalidraw\ndescription: Diagrams.\nlicense: MIT\n---\n");
        write("creative/excalidraw/references/x.md", "not a skill\n");
        assert!(read_tree(&dir).unwrap_err().to_string().contains("a label names"), "labels name skills the tree has");
        for (id, _) in LABELS {
            write(&format!("{id}/SKILL.md"), "---\ndescription: Labelled.\n---\n");
        }
        let skills = read_tree(&dir).unwrap();
        assert_eq!(skills.len(), 2 + LABELS.len());
        assert_eq!(skills.iter().filter(|s| s.id.starts_with("creative/") || s.id.starts_with("mlops/training")).map(|s| s.id.as_str()).collect::<Vec<_>>(), ["creative/excalidraw", "mlops/training/unsloth"]);
        let text = render("v0.23.2", &skills);
        let read: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(read["hermes"], "v0.23.2");
        assert_eq!(read["skills"].as_array().unwrap().len(), skills.len());
        assert!(text.contains("\n    {\"id\":\"creative/excalidraw\",\"name\":\"excalidraw\",\"description\":\"Diagrams.\",\"license\":\"MIT\"},\n"), "{text}");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
