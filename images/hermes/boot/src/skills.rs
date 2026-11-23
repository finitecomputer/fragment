//! The skills every profile finds beyond its own (docs/computers.md):
//!
//! - The managed skills (decision 17): the owner's skills fragment's
//!   `skills/`, the blessed `skills` template's release beneath any file of
//!   the fragment's own at the same path. The boot installs them read-only
//!   at `MANAGED_DIR`. The owner's skills fragment is found as the
//!   computer's first agent acting for its owner (`for`): of the owner's
//!   fragments, one of kind `skills` they own. None
//!   means no managed skills, and what was installed goes: the skills a
//!   person's settings list are their agents' (the shell reads the same
//!   fragment).
//! - Hermes' pinned bundled skills, except its Google OAuth skill (the
//!   managed google-workspace-finite uses the platform's connection).
//!   Read-only copies in PLATFORM_VIEW, withheld when a managed skill takes
//!   the same name or alias: Hermes' external dirs share one rank.
//! - Hermes' optional skills the owner switched on: one marker file each in
//!   their skills fragment (`optional/hermes/<id>.on`, the shell's Skills
//!   page writes it). Read-only copies of the image's own
//!   (`OPTIONAL_DIR`) in PLATFORM_VIEW, beside the bundled ones; withheld
//!   when a managed or bundled skill takes the same name or alias, and
//!   removed once switched off (`settle_optional`).
//! - The platform skill, `fragment`, unless a managed skill takes its name:
//!   the `fragment` CLI's own skill (`fragment skill`) after a page of what
//!   the computer adds (`computer.md`), written at the image's build into
//!   `PLATFORM_DIR` (`hermes-boot build-info`), so it is the binary's in the
//!   image, and shown to the profiles in `PLATFORM_VIEW` (`settle_platform`,
//!   at each start and after each install).
//!
//! Every profile names both in `skills.external_dirs` (hermes.rs). Hermes
//! ranks a profile's own `skills/` (its agent fragment's, synced: sync.rs)
//! above its external dirs, so an agent's own skill wins over a managed one
//! or the platform's. Its external dirs are one rank: two skills there of
//! one name are ambiguous, and Hermes finds neither by it (since v0.21.6;
//! v0.21.5 took the first dir's). So a managed `fragment` wins
//! over the platform skill by the platform skill leaving the view.
//!
//! The plan is a pure function (`pick`, `plan`); `install` carries it out.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use fragment_bridge::api::{Api, ApiError, FileEntry, FragmentEntry};
use fragment_bridge::ev;
use fragment_bridge::runtime::Agent;

/// Where the managed skills are installed: under `/data`, so a wake finds
/// the last install and fetches only what changed since.
pub const MANAGED_DIR: &str = "/data/hermes/managed-skills";
/// What the installed files were, by path under `MANAGED_DIR`: the
/// listing's version of each (`lastCommitSha`).
pub const MANIFEST: &str = "/data/hermes-sync/managed-skills.json";
/// The managed set's files in the skills fragment.
pub const PREFIX: &str = "skills/";
/// Bounds on what one install takes (the platform's own are below these:
/// crates/templates `blessed::DATA_*`): files, each file, and all of them.
pub const FILES_MAX: usize = 1_000;
pub const FILE_MAX_BYTES: usize = 256 * 1024;
pub const TOTAL_MAX_BYTES: usize = 8 * 1024 * 1024;
/// Files fetched at once.
pub const FETCH_AT_ONCE: usize = 8;
/// A path under `MANAGED_DIR` is at most this long, and this deep.
const PATH_MAX: usize = 300;
const DEPTH_MAX: usize = 8;
/// While the owner has no skills fragment, it is looked for this often
/// rather than every `SKILLS_EVERY_MS` (main.rs): a person who lacks one
/// adds it from the shell's settings, and their agents have it within
/// about this.
pub const ABSENT_EVERY_MS: u64 = 60_000;

/// The skill files Hermes indexes into a system prompt under `dirs` (the
/// SKILL.md and DESCRIPTION.md files, links followed, as its skills-index
/// snapshot watches them; it passes over some folders this walks, such as
/// a skill's `scripts/`, which only asks a refresh it did not need), each
/// its path and bytes, for `hermes::prompt_inputs`; and whether a bound cut
/// it short. Each dir gives at most `FILES_MAX` files (an install's own
/// bound, so the managed set is watched whole beside the platform's), each
/// of at most `FILE_MAX_BYTES` (a larger one counts as its size alone),
/// `DEPTH_MAX` deep. A directory that cannot be read counts as empty.
pub fn indexed(dirs: &[&Path]) -> (Vec<(String, Vec<u8>)>, bool) {
    let mut out = Vec::new();
    let mut cut = false;
    for root in dirs {
        let mut found = 0;
        let mut pending: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
        'walk: while let Some((dir, depth)) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            let mut entries: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
            entries.sort();
            for path in entries {
                let Ok(meta) = std::fs::metadata(&path) else { continue };
                if meta.is_dir() {
                    if depth < DEPTH_MAX {
                        pending.push((path, depth + 1));
                    } else {
                        cut = true;
                    }
                } else if path.file_name().is_some_and(|n| n == "SKILL.md" || n == "DESCRIPTION.md") {
                    if found >= FILES_MAX {
                        cut = true;
                        break 'walk;
                    }
                    found += 1;
                    let bytes = if meta.len() <= FILE_MAX_BYTES as u64 { std::fs::read(&path).unwrap_or_default() } else { meta.len().to_string().into_bytes() };
                    out.push((path.display().to_string(), bytes));
                }
            }
        }
    }
    (out, cut)
}

/// Where the platform skill is: in the image, read-only to the agents.
pub const PLATFORM_DIR: &str = "/opt/fragment/skills";
/// Hermes' skills, moved out of its sync source at the image's build.
pub const BUNDLED_DIR: &str = "/opt/fragment/bundled-skills";
/// Where the profiles find it: a copy of it while no managed skill takes
/// its name (`settle_platform`), the boot's, read-only to the agents. In
/// the boot's run directory: made at each start, never saved.
pub const PLATFORM_VIEW: &str = "/var/lib/fragment-run/platform-skills";
/// Its name, which a managed or an agent's own skill of the same name
/// shadows; and its path under `PLATFORM_DIR` (`<category>/<name>/`).
pub const PLATFORM_NAME: &str = "fragment";
pub const PLATFORM_PATH: &str = "platform/fragment/SKILL.md";
/// Hermes' optional skills, in the image as upstream ships them (any
/// depth: `<category>/<name>` or `<category>/<group>/<name>`), not
/// activated by default: the owner switches each on.
pub const OPTIONAL_DIR: &str = "/opt/hermes/optional-skills";
/// The owner's switches in their skills fragment: an `<id>.on` file under
/// this prefix for each optional skill switched on, its id its path under
/// `OPTIONAL_DIR`. One file a switch, so two pages switching two skills
/// never undo each other. What a file holds is not read.
pub const OPTIONAL_PREFIX: &str = "optional/hermes/";
pub const OPTIONAL_SUFFIX: &str = ".on";
/// At most this many are switched on (the rest are refused): each is a
/// line of every turn's skills index.
pub const OPTIONAL_MAX: usize = 256;
/// The last install's switches, under `/data`: a wake shows them before
/// its first install answers.
pub const OPTIONAL_CHOSEN: &str = "/data/hermes-sync/optional-skills.json";
/// Which optional skills the view holds, beside it in the boot's run
/// directory (made at each start, as the view is): what a switch turned
/// off removes.
pub const OPTIONAL_SHOWN: &str = "/var/lib/fragment-run/optional-skills.json";
/// The external dirs every profile names: the managed set, and the
/// platform skill's view.
pub const EXTERNAL_DIRS: [&str; 2] = [MANAGED_DIR, PLATFORM_VIEW];
/// The page the computer adds to the CLI's skill.
const COMPUTER_PAGE: &str = include_str!("computer.md");
/// The platform skill's description, which Hermes lists in every turn's
/// prompt (its skills index): what makes an agent load it.
const PLATFORM_DESCRIPTION: &str = "You are an agent on a Fragment computer: what that is, and the `fragment` CLI in your terminal, which acts as you. Load it before you list, read, make, change, deploy or share your owner's fragments (apps, sites, brains, chats), when asked what you can do here, to ask another of your owner's agents (`fragment ask`), for anything later or recurring (`fragment schedule`), to search past conversations and brains (`fragment search`), and for your owner's connections (Google: GOOGLE_OAUTH_ACCESS_TOKEN) or your desktop.";
/// The platform skill is a page or two, not a manual. 20 KiB since
/// 2026-10-22: the computer's page's "Files your owner sends you" took it
/// past 16 KiB, which it had all but reached. 21 KiB since 2026-11-18:
/// what a sleep ends took it past 20 KiB, which it had reached exactly.
pub const PLATFORM_MAX_BYTES: usize = 21 * 1024;

/// The platform skill: `cli_skill` (what `fragment skill` prints, a skill
/// named `fragment`) with the computer's page before its body, and the
/// computer's description. Refused unless the CLI's skill is one named
/// `fragment`, and the whole is within `PLATFORM_MAX_BYTES`.
pub fn platform_skill(cli_skill: &str) -> Result<String, String> {
    let (front, body) = cli_skill.strip_prefix("---\n").and_then(|rest| rest.split_once("\n---\n")).ok_or("`fragment skill` printed no skill: no frontmatter")?;
    if !front.lines().any(|l| l.trim_end() == format!("name: {PLATFORM_NAME}")) {
        return Err(format!("`fragment skill` printed a skill not named {PLATFORM_NAME}: {front:?}"));
    }
    let description = serde_json::to_string(PLATFORM_DESCRIPTION).expect("a string serializes");
    let skill = format!("---\nname: {PLATFORM_NAME}\ndescription: {description}\n---\n\n{}\n\n{}\n", COMPUTER_PAGE.trim(), body.trim());
    if skill.len() > PLATFORM_MAX_BYTES {
        return Err(format!("the platform skill is {} bytes, past {PLATFORM_MAX_BYTES}", skill.len()));
    }
    Ok(skill)
}

/// Writes the platform skill whole under `dir` (the image's build: root's,
/// readable by all).
pub fn write_platform_skill(dir: &Path, skill: &str) -> std::io::Result<()> {
    write_file(dir, PLATFORM_PATH, skill.as_bytes())
}

/// At image build, every concrete reference mentioned by a bundled or
/// platform skill must exist. Walk bounded trees iteratively, and check
/// their text helpers too: a reference may itself point to another file.
pub fn check_references(dir: &Path) -> std::io::Result<()> {
    let mut pending = vec![(dir.to_path_buf(), None::<PathBuf>, 0)];
    let mut count = 0;
    while let Some((dir, root, depth)) = pending.pop() {
        assert!(depth <= DEPTH_MAX, "the image's skill tree is bounded");
        let root = if dir.join("SKILL.md").is_file() { Some(dir.clone()) } else { root };
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            count += 1;
            assert!(count <= 10_000, "the pinned bundled set is bounded");
            if entry.file_type()?.is_dir() {
                pending.push((entry.path(), root.clone(), depth + 1));
            } else if let Some(root) = &root {
                let path = entry.path();
                let bytes = std::fs::read(&path)?;
                if let Ok(text) = std::str::from_utf8(&bytes) {
                    for reference in crate::skill_references::paths(text) {
                        if !root.join(reference).exists() {
                            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{} mentions missing {reference}", path.display())));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// When the next install of the managed skills is due after one that found
/// the owner's skills fragment (`Some(true)`), found none (`Some(false)`),
/// or did not answer (`None`): sooner while there is none.
pub fn next_install_ms(found: Option<bool>, every_ms: u64) -> u64 {
    match found {
        Some(false) => every_ms.min(ABSENT_EVERY_MS),
        Some(true) | None => every_ms,
    }
}

/// The owner's skills fragment among the fragments the agent reaches acting
/// for them: kind `skills`, owned by them (a skills fragment someone else
/// shared with them is not theirs to install). One labelled `skills` first
/// (the shell makes it so), else the first by name.
pub fn pick(fragments: &[FragmentEntry]) -> Option<String> {
    let mut theirs: Vec<&str> = fragments.iter().filter(|f| f.kind == "skills" && f.owned).map(|f| f.name.as_str()).collect();
    theirs.sort_unstable();
    let labelled = theirs.iter().find(|n| n.rsplit_once("--").is_some_and(|(label, _)| label == "skills"));
    labelled.or(theirs.first()).map(|n| n.to_string())
}

/// Installed files: path under `MANAGED_DIR` → the version installed.
pub type Installed = BTreeMap<String, String>;

/// One install's steps.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// (path under `MANAGED_DIR`, its path in the fragment, its version)
    pub fetch: Vec<(String, String, String)>,
    /// paths under `MANAGED_DIR` to delete
    pub remove: Vec<String>,
    /// what is installed once it is done
    pub wanted: Installed,
    /// files of the listing left out, and why
    pub refused: Vec<(String, &'static str)>,
}

/// Whether `rel` is a path a managed file may have under `MANAGED_DIR`.
pub fn valid_rel(rel: &str) -> bool {
    let parts: Vec<&str> = rel.split('/').collect();
    !rel.is_empty()
        && rel.len() <= PATH_MAX
        && parts.len() <= DEPTH_MAX
        && parts.iter().all(|p| !p.is_empty() && *p != "." && *p != ".." && !p.starts_with('.') && *p != "__pycache__")
        && rel.bytes().all(|b| b.is_ascii_graphic() && b != b'\\')
}

/// The steps from what is installed to the listing's managed set: fetch
/// what is new or changed, remove what the listing no longer has. A file
/// past a bound or at a path that is no safe path is refused, never
/// fetched; the rest of the set still installs.
pub fn plan(listing: Option<&[FileEntry]>, installed: &Installed) -> Plan {
    let mut out = Plan::default();
    let mut total = 0usize;
    for f in listing.unwrap_or_default() {
        let Some(rel) = f.path.strip_prefix(PREFIX) else { continue };
        let refused = if !valid_rel(rel) {
            Some("not a safe path")
        } else if f.size as usize > FILE_MAX_BYTES {
            Some("larger than a managed file may be")
        } else if out.wanted.len() >= FILES_MAX {
            Some("past the managed set's files")
        } else if total + f.size as usize > TOTAL_MAX_BYTES {
            Some("past the managed set's bytes")
        } else {
            None
        };
        if let Some(why) = refused {
            out.refused.push((f.path.clone(), why));
            continue;
        }
        total += f.size as usize;
        if installed.get(rel) != Some(&f.last_commit_sha) || f.last_commit_sha.is_empty() {
            out.fetch.push((rel.to_string(), f.path.clone(), f.last_commit_sha.clone()));
        }
        out.wanted.insert(rel.to_string(), f.last_commit_sha.clone());
    }
    out.remove = installed.keys().filter(|p| !out.wanted.contains_key(*p)).cloned().collect();
    out
}

/// Which installed paths are a skill's `SKILL.md`: what the profiles see.
pub fn skill_names(installed: &Installed) -> BTreeSet<String> {
    installed.keys().filter_map(|p| p.strip_suffix("/SKILL.md")).filter_map(|d| d.rsplit('/').next()).map(str::to_string).collect()
}

/// The names a profile's own skills go by (its `skills/`, its agent
/// fragment's): each `SKILL.md`'s name as Hermes gives it, and its
/// directory's. Wherever under it one is: Hermes finds a skill at any
/// depth, and `sync::scan` syncs one. Bounded as that scan is: by the
/// profile's tree, no link followed, and `FILES_MAX` skills in it.
fn own_names(own: &Path) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut dirs = vec![own.to_path_buf()];
    let mut read = 0;
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.filter_map(Result::ok) {
            // the entry's own kind: a link is neither, and is not followed
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                dirs.push(entry.path());
            } else if kind.is_file() && entry.file_name() == "SKILL.md" && read < FILES_MAX {
                read += 1;
                let dir_name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if let Ok(text) = std::fs::read_to_string(entry.path()) {
                    names.insert(skill_name(&text, &dir_name));
                }
                names.insert(dir_name);
            }
        }
    }
    names
}

/// The managed skills an agent is not shown: each connection's skill whose
/// connection is none of `providers` (the providers of the credentials it
/// may use now). So its prompt lists the skills of what its owner
/// connected, however many connections the platform offers, and a skill
/// appears as its connection does. Their names as Hermes gives them,
/// sorted; one whose name is not plain is left shown.
///
/// Hermes disables by name, whichever skill holds the name, and a
/// profile's own skill wins a name over a managed one. So a name one of
/// the profile's own skills (in `own`) goes by is never withheld: the
/// agent's own copy stays its own, and the managed one it shadows is not
/// listed anyway.
pub fn withheld(dir: &Path, installed: &Installed, providers: &BTreeSet<String>, own: &Path) -> Vec<String> {
    let kept = own_names(own);
    let mut names = BTreeSet::new();
    for rel in installed.keys().filter(|p| p.ends_with("/SKILL.md")) {
        let Ok(text) = std::fs::read_to_string(dir.join(rel)) else { continue };
        let Some(connection) = crate::skill_connection::of(&text) else { continue };
        let name = skill_name(&text, rel.trim_end_matches("/SKILL.md").rsplit('/').next().unwrap_or_default());
        let plain = !name.is_empty() && name.len() <= 64 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if plain && !providers.contains(&connection) && !kept.contains(&name) {
            names.insert(name);
        }
    }
    names.into_iter().collect()
}

/// A profile's hidden connection skills as its `config.yaml` says them
/// (`hermes::profile_config`'s `skills.disabled` line, under `skills:`),
/// or nothing when it hides none.
pub fn hidden_of(config: &str) -> String {
    let mut in_skills = false;
    for line in config.lines() {
        if !line.starts_with(' ') {
            in_skills = line == "skills:";
        } else if in_skills && line.starts_with("  disabled: ") {
            return line.to_string();
        }
    }
    String::new()
}

/// Each profile's hidden connection skills under `profiles` (its
/// `<profile>/config.yaml`), as prompt inputs (`hermes::prompt_inputs`):
/// its path and its line, for each profile that hides any. Bounded by the
/// profiles a computer holds; a config never followed through a link.
pub fn hidden_lists(profiles: &Path) -> Vec<(String, Vec<u8>)> {
    let Ok(entries) = std::fs::read_dir(profiles) else { return Vec::new() };
    let mut found = Vec::new();
    for e in entries.flatten().take(FILES_MAX) {
        let path = e.path().join("config.yaml");
        if !std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file() && m.len() <= FILE_MAX_BYTES as u64) {
            continue;
        }
        let line = hidden_of(&std::fs::read_to_string(&path).unwrap_or_default());
        if !line.is_empty() {
            found.push((format!("{} skills.disabled", path.display()), line.into_bytes()));
        }
    }
    found.sort();
    found
}

/// The name Hermes gives the skill whose `SKILL.md` reads `text`, in a
/// directory named `dir_name`: its frontmatter's `name` (quoted or not),
/// else the directory's.
pub fn skill_name(text: &str, dir_name: &str) -> String {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text).replace("\r\n", "\n");
    let front = text.strip_prefix("---\n").and_then(|r| r.split_once("\n---").map(|(f, _)| f.to_string())).unwrap_or_default();
    let declared = front.lines().find_map(|l| l.strip_prefix("name:")).map(|v| v.trim().trim_matches(|c| c == '"' || c == '\'').trim().to_string()).filter(|v| !v.is_empty());
    declared.unwrap_or_else(|| dir_name.to_string())
}

/// Whether a managed skill takes the platform skill's name: a `SKILL.md`
/// installed under `dir` (as `installed` lists them) that Hermes names
/// `PLATFORM_NAME`.
pub fn shadows_platform(dir: &Path, installed: &Installed) -> bool {
    installed.keys().filter(|rel| *rel == "SKILL.md" || rel.ends_with("/SKILL.md")).any(|rel| {
        let path = dir.join(rel);
        let dir_name = path.parent().and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        // bounded: a managed file is at most FILE_MAX_BYTES
        std::fs::read_to_string(&path).is_ok_and(|text| skill_name(&text, &dir_name) == PLATFORM_NAME)
    })
}

/// Shows the platform skill to the profiles: in `view`, a copy of
/// `platform`'s (written whole when it differs) unless a managed skill
/// installed in `dir` (as `manifest` lists them) takes its name, when it
/// leaves the view. The view is there either way: Hermes skips an external
/// dir it does not find until the profile's config changes. Whether it is
/// shown.
pub fn settle_platform(dir: &Path, manifest: &Path, platform: &Path, view: &Path) -> std::io::Result<bool> {
    std::fs::create_dir_all(view)?;
    if shadows_platform(dir, &load_manifest(manifest)) {
        remove_file(view, PLATFORM_PATH);
        return Ok(false);
    }
    let skill = std::fs::read(platform.join(PLATFORM_PATH))?;
    if std::fs::read(view.join(PLATFORM_PATH)).ok().as_deref() != Some(&skill[..]) {
        write_file(view, PLATFORM_PATH, &skill)?;
    }
    Ok(true)
}

/// Expose the image's bundled skills without copying them into a saved
/// profile. Upstream's pinned tree is category/name/SKILL.md, at most
/// FILES_MAX directories. Managed names and aliases win by withholding the
/// corresponding copy; removal restores it on the next settle. Agent-owned
/// skills win through Hermes' own higher profile tier.
pub fn settle_bundled(dir: &Path, manifest: &Path, bundled: &Path, view: &Path) -> std::io::Result<()> {
    let managed = managed_names(dir, manifest)?;
    for (rel, name, alias) in bundled_skills(bundled)? {
        let target = view.join(&rel);
        if [name, alias, rel.to_string_lossy().into_owned()].iter().any(|n| managed.contains(n)) {
            if target.exists() { std::fs::remove_dir_all(&target)?; }
        } else if !target.join("SKILL.md").is_file() {
            // The source is immutable for this image's life and the
            // view is rebuilt at every boot, so copy once. Copies keep
            // Hermes' resolved-path trust check inside its search root;
            // symlinks outside it warn on every skill_view call.
            copy_bundled_skill(&bundled.join(&rel), &target)?;
        }
    }
    Ok(())
}

/// The names, aliases and paths of the managed skills installed in `dir`
/// (as `manifest` lists them): what an image's skill of the same yields to.
fn managed_names(dir: &Path, manifest: &Path) -> std::io::Result<BTreeSet<String>> {
    let mut managed = BTreeSet::new();
    for rel in load_manifest(manifest).keys().filter(|p| *p == "SKILL.md" || p.ends_with("/SKILL.md")) {
        let parent = Path::new(rel).parent().expect("a skill has a directory");
        let alias = parent.file_name().or_else(|| dir.file_name()).expect("a skill has a name").to_string_lossy();
        let text = std::fs::read_to_string(dir.join(rel))?;
        managed.extend([skill_name(&text, &alias), alias.into_owned(), parent.to_string_lossy().into_owned()]);
    }
    Ok(managed)
}

/// Each skill of the image's bundled tree (`<category>/<name>/SKILL.md`):
/// its path under the tree, its name, and its alias (its directory's).
fn bundled_skills(bundled: &Path) -> std::io::Result<Vec<(PathBuf, String, String)>> {
    let mut out = vec![];
    for category in std::fs::read_dir(bundled)? {
        let category = category?;
        if !category.file_type()?.is_dir() { continue; }
        for skill in std::fs::read_dir(category.path())? {
            let skill = skill?;
            if !skill.file_type()?.is_dir() || !skill.path().join("SKILL.md").is_file() { continue; }
            assert!(out.len() < FILES_MAX, "the pinned bundled tree is bounded");
            let alias = skill.file_name().to_string_lossy().into_owned();
            let name = skill_name(&std::fs::read_to_string(skill.path().join("SKILL.md"))?, &alias);
            out.push((PathBuf::from(category.file_name()).join(&alias), name, alias));
        }
    }
    Ok(out)
}

/// The optional skills a skills fragment's listing switches on: each
/// `<id>.on` under `OPTIONAL_PREFIX` whose id is a safe path, at most
/// `OPTIONAL_MAX`. The rest are refused, and why.
pub fn switched_on(listing: &[FileEntry]) -> (BTreeSet<String>, Vec<(String, &'static str)>) {
    let (mut on, mut refused) = (BTreeSet::new(), vec![]);
    for f in listing {
        let Some(id) = f.path.strip_prefix(OPTIONAL_PREFIX).and_then(|p| p.strip_suffix(OPTIONAL_SUFFIX)) else { continue };
        if !valid_rel(id) {
            refused.push((f.path.clone(), "not a safe path"));
        } else if on.len() >= OPTIONAL_MAX {
            refused.push((f.path.clone(), "past the optional skills switched on"));
        } else {
            on.insert(id.to_string());
        }
    }
    (on, refused)
}

/// What one settle of the optional skills did, for the log.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct OptionalShown {
    /// in the view now
    pub shown: Vec<String>,
    /// switched on, but this image has no such skill (another pin's)
    pub missing: Vec<String>,
    /// switched on, but a managed or bundled skill takes its name
    pub withheld: Vec<String>,
    /// could not be copied, and why
    pub failed: Vec<(String, String)>,
}

/// Shows the optional skills switched on (as `chosen` lists them) to the
/// profiles: read-only copies of the image's (`optional`) in `view`, at
/// their path under it, as bundled ones are. One the image lacks, or whose
/// name or alias a managed skill, a bundled one or the platform skill
/// takes, is left out: Hermes' external dirs share one rank, where two of
/// a name are ambiguous. One switched off since goes (`shown` lists what
/// the view holds). One that fails to copy is said and the rest show.
pub fn settle_optional(dir: &Path, manifest: &Path, bundled: &Path, optional: &Path, chosen: &Path, shown: &Path, view: &Path) -> std::io::Result<OptionalShown> {
    let wanted: BTreeSet<String> = load_set(chosen).into_iter().filter(|id| valid_rel(id)).collect();
    let before: BTreeSet<String> = load_set(shown).into_iter().filter(|id| valid_rel(id)).collect();
    // switched off goes first, whatever fails below
    for id in before.difference(&wanted) {
        remove_dir(view, id);
    }
    let mut taken = managed_names(dir, manifest)?;
    taken.insert(PLATFORM_NAME.to_string());
    if bundled.is_dir() {
        for (rel, name, alias) in bundled_skills(bundled)? {
            taken.extend([name, alias, rel.to_string_lossy().into_owned()]);
        }
    }
    let mut out = OptionalShown::default();
    let mut show = BTreeSet::new();
    for id in wanted {
        // bounded: the image's own file
        let Ok(text) = std::fs::read_to_string(optional.join(&id).join("SKILL.md")) else {
            out.missing.push(id);
            continue;
        };
        let alias = id.rsplit('/').next().expect("an id has a name");
        let name = skill_name(&text, alias);
        if [name.as_str(), alias, id.as_str()].iter().any(|n| taken.contains(*n)) {
            out.withheld.push(id);
        } else {
            show.insert(id);
        }
    }
    for id in before.difference(&show) {
        remove_dir(view, id);
    }
    // listed before any is copied: a copy cut short still goes once
    // switched off
    save_json(shown, &before.union(&show).collect::<BTreeSet<_>>())?;
    for id in show {
        let target = view.join(&id);
        if !target.join("SKILL.md").is_file() {
            if let Err(e) = copy_bundled_skill(&optional.join(&id), &target) {
                remove_dir(view, &id);
                out.failed.push((id, e.to_string()));
                continue;
            }
        }
        out.shown.push(id);
    }
    // one that would not go is tried again at the next settle
    let stuck = before.iter().filter(|id| !out.shown.contains(*id) && view.join(id).exists());
    save_json(shown, &out.shown.iter().chain(stuck).collect::<BTreeSet<_>>())?;
    Ok(out)
}

/// A bounded, iterative copy of one immutable skill into the temporary
/// view. Publish the directory whole, so a failed copy is retried and a
/// profile never loads a skill before its helpers are present.
fn copy_bundled_skill(source: &Path, target: &Path) -> std::io::Result<()> {
    let scratch = target.with_file_name(format!(".{}.fragment-tmp", target.file_name().expect("a skill has a name").to_string_lossy()));
    if scratch.exists() { std::fs::remove_dir_all(&scratch)?; }
    let copied = copy_tree(source, &scratch).and_then(|()| std::fs::rename(&scratch, target));
    if copied.is_err() {
        // no half a skill left where Hermes looks
        let _ = std::fs::remove_dir_all(&scratch);
    }
    copied
}

fn copy_tree(source: &Path, scratch: &Path) -> std::io::Result<()> {
    let mut pending = vec![(source.to_path_buf(), scratch.to_path_buf(), 0)];
    let mut count = 0;
    while let Some((from, to, depth)) = pending.pop() {
        assert!(depth <= DEPTH_MAX, "the pinned skill's tree is bounded");
        std::fs::create_dir_all(&to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            count += 1;
            assert!(count <= FILES_MAX, "the pinned skill's files are bounded");
            let dest = to.join(entry.file_name());
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push((entry.path(), dest, depth + 1));
            } else if kind.is_file() {
                std::fs::copy(entry.path(), dest)?;
            } else {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("an image's skill has a non-file: {}", entry.path().display())));
            }
        }
    }
    Ok(())
}

pub fn load_manifest(path: &Path) -> Installed {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_manifest(path: &Path, m: &Installed) -> std::io::Result<()> {
    save_json(path, m)
}

/// A set of ids saved by `save_json`; none when it is missing or unread.
fn load_set(path: &Path) -> BTreeSet<String> {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Writes `value` as JSON at `path` whole (a temporary file renamed over it).
fn save_json(path: &Path, value: &impl Serialize) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(value).expect("a set of paths serializes"))?;
    std::fs::rename(&tmp, path)
}

/// Writes `bytes` at `dir/rel` whole (a temporary file renamed over it), so
/// Hermes never reads half a skill. Owned by the boot (root), readable by
/// all: the managed set is read-only to the agents.
fn write_file(dir: &Path, rel: &str, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let target = dir.join(rel);
    let parent = target.parent().expect("a managed file has a directory");
    std::fs::create_dir_all(parent)?;
    let tmp = target.with_file_name(format!(".{}.fragment-tmp", target.file_name().and_then(|n| n.to_str()).unwrap_or("file")));
    std::fs::write(&tmp, bytes)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644))?;
    std::fs::rename(&tmp, &target)
}

/// Removes `dir/rel`, then each directory it leaves empty up to `dir`.
fn remove_file(dir: &Path, rel: &str) {
    let target = dir.join(rel);
    let _ = std::fs::remove_file(&target);
    prune(dir, &target);
}

/// Removes the directory `dir/rel` whole, then each directory it leaves
/// empty up to `dir`.
fn remove_dir(dir: &Path, rel: &str) {
    let target = dir.join(rel);
    let _ = std::fs::remove_dir_all(&target);
    prune(dir, &target);
}

/// Removes each directory above `target` that is empty, up to `dir`.
fn prune(dir: &Path, target: &Path) {
    let mut parent = target.parent().map(Path::to_path_buf);
    // bounded by the path's depth
    while let Some(p) = parent {
        if p == dir || std::fs::remove_dir(&p).is_err() {
            break;
        }
        parent = p.parent().map(Path::to_path_buf);
    }
}

/// What one install did, for the log.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Done {
    pub fragment: Option<String>,
    pub fetched: u32,
    pub removed: u32,
    pub refused: u32,
    pub skills: u32,
    /// optional skills switched on
    pub optional: u32,
}

/// Why an install stopped: the platform did not answer as it should, or a
/// file could not be written. Either way what is installed stays whole.
#[derive(Debug)]
pub enum InstallError {
    Api(ApiError),
    Disk(String),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallError::Api(e) => write!(f, "{e}"),
            InstallError::Disk(e) => write!(f, "writing the managed skills: {e}"),
        }
    }
}

impl From<ApiError> for InstallError {
    fn from(e: ApiError) -> InstallError {
        InstallError::Api(e)
    }
}

/// One install of the owner's managed skills into `dir`, read as `reader`
/// acting for `owner`, and of the optional skills they switched on, saved
/// at `chosen` for `settle_optional`. An answer the platform did not give
/// (a transport error, a refusal) changes nothing installed: the last
/// install stays.
pub async fn install(api: &Api, reader: &Agent, owner: &str, dir: &Path, manifest: &Path, chosen: &Path) -> Result<Done, InstallError> {
    // what the manifest says is installed and is still on disk (a file gone
    // from under it is fetched again)
    let mut installed: Installed = load_manifest(manifest).into_iter().filter(|(rel, _)| valid_rel(rel) && dir.join(rel).is_file()).collect();
    let fragments = api.fragments_for(&reader.fragment, owner).await?;
    let fragment = pick(&fragments);
    let listing = match &fragment {
        Some(f) => Some(api.files_for(&reader.fragment, f, owner).await?),
        None => None,
    };
    let steps = plan(listing.as_deref(), &installed);
    let (on, refused_on) = listing.as_deref().map(switched_on).unwrap_or_default();
    let mut done = Done { fragment: fragment.clone(), refused: (steps.refused.len() + refused_on.len()) as u32, optional: on.len() as u32, ..Done::default() };
    for (path, why) in steps.refused.iter().chain(&refused_on) {
        ev!("skills.refused", { "path": path, "why": why });
    }
    std::fs::create_dir_all(dir).map_err(|e| InstallError::Disk(format!("{}: {e}", dir.display())))?;
    if let Some(f) = &fragment {
        for batch in steps.fetch.chunks(FETCH_AT_ONCE) {
            let reads = batch.iter().map(|(_, path, _)| api.file_for(&reader.fragment, f, owner, path, FILE_MAX_BYTES));
            let got = futures_util::future::join_all(reads).await;
            for ((rel, _, version), bytes) in batch.iter().zip(got) {
                let bytes = bytes?;
                write_file(dir, rel, &bytes).map_err(|e| InstallError::Disk(format!("{rel}: {e}")))?;
                installed.insert(rel.clone(), version.clone());
                done.fetched += 1;
            }
            // saved as it goes: a cut install fetches only what it lacks next time
            if let Err(e) = save_manifest(manifest, &installed) {
                ev!("skills.unsaved", { "error": e.to_string() });
            }
        }
    }
    for rel in &steps.remove {
        remove_file(dir, rel);
        installed.remove(rel);
        done.removed += 1;
    }
    assert_eq!(installed, steps.wanted, "an install that answered whole installs exactly the listing's set");
    if let Err(e) = save_manifest(manifest, &installed) {
        ev!("skills.unsaved", { "error": e.to_string() });
    }
    save_json(chosen, &on).map_err(|e| InstallError::Disk(format!("{}: {e}", chosen.display())))?;
    done.skills = skill_names(&installed).len() as u32;
    Ok(done)
}

/// The agent the boot reads the managed set as: the computer's first agent
/// whose owner is the computer's (one computer per person: decision 13).
pub fn reader<'a>(agents: &'a [Agent], owner: &str) -> Option<&'a Agent> {
    agents.iter().find(|a| a.owner == owner)
}

/// The managed directory every profile names (hermes.rs).
pub fn managed_dir() -> PathBuf {
    PathBuf::from(MANAGED_DIR)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A connection's skill is shown to an agent that holds the connection:
    /// each managed skill that names one (`metadata: fragment: connection`)
    /// is withheld from an agent whose credentials have no such provider,
    /// and shown again once they do. A skill of no connection is never
    /// withheld, nor one whose name is not plain, nor one that is not read,
    /// nor a name the profile's own skill goes by: Hermes disables by name,
    /// and the agent's own copy is not ours to hide.
    #[test]
    fn a_connections_skill_is_shown_to_an_agent_that_holds_it() {
        let dir = std::env::temp_dir().join(format!("hermes-boot-withheld-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let skill = |name: &str, connection: Option<&str>| {
            let meta = connection.map(|c| format!("  fragment:\n    connection: {c}\n")).unwrap_or_default();
            format!("---\nname: {name}\ndescription: d\nmetadata:\n  hermes:\n    tags: [A]\n{meta}---\n\n# {name}\n")
        };
        let mut installed = Installed::new();
        for (rel, text) in [
            ("productivity/linear-finite/SKILL.md", skill("linear-finite", Some("linear"))),
            ("productivity/jira-finite/SKILL.md", skill("jira-finite", Some("atlassian"))),
            ("productivity/confluence-finite/SKILL.md", skill("confluence-finite", Some("atlassian"))),
            ("productivity/google-workspace-finite/SKILL.md", skill("google-workspace-finite", Some("google"))),
            ("software-development/apps-finite/SKILL.md", skill("apps-finite", None)),
            ("odd/SKILL.md", skill("an odd, name", Some("linear"))),
        ] {
            write_file(&dir, rel, text.as_bytes()).unwrap();
            installed.insert(rel.to_string(), "v".into());
        }
        installed.insert("gone/SKILL.md".into(), "v".into());
        let own = dir.join("own-skills");
        let holding = |providers: &[&str]| withheld(&dir, &installed, &providers.iter().map(|p| p.to_string()).collect(), &own);
        assert_eq!(holding(&[]), ["confluence-finite", "google-workspace-finite", "jira-finite", "linear-finite"]);
        assert_eq!(holding(&["google", "perplexity"]), ["confluence-finite", "jira-finite", "linear-finite"]);
        assert_eq!(holding(&["atlassian", "linear"]), ["google-workspace-finite"], "both of one connection's skills");
        assert!(holding(&["atlassian", "google", "linear"]).is_empty());
        // the agent's own skill of a managed one's name, by its frontmatter or its folder: left shown
        write_file(&own, "linear-finite/SKILL.md", skill("linear-finite", None).as_bytes()).unwrap();
        write_file(&own, "tracking/jira-finite/SKILL.md", skill("my jira notes", None).as_bytes()).unwrap();
        write_file(&own, "notes/SKILL.md", skill("unrelated", None).as_bytes()).unwrap();
        assert_eq!(holding(&[]), ["confluence-finite", "google-workspace-finite"], "its own linear-finite and jira-finite are not hidden with the managed ones");
        // and however deep its own skill sits, by its folder or its frontmatter (Hermes finds one at any depth)
        write_file(&own, "work/tracking/wiki/confluence-finite/SKILL.md", skill("team wiki", None).as_bytes()).unwrap();
        write_file(&own, "a/b/c/d/mail/SKILL.md", skill("google-workspace-finite", None).as_bytes()).unwrap();
        // a link back up its own tree is not followed
        std::os::unix::fs::symlink(&own, own.join("work/again")).unwrap();
        assert!(holding(&[]).is_empty() && holding(&["atlassian", "google", "linear"]).is_empty(), "none of its own is hidden, connected or not");
        assert_eq!(own_names(&own).into_iter().collect::<Vec<_>>(), ["confluence-finite", "google-workspace-finite", "jira-finite", "linear-finite", "mail", "my jira notes", "notes", "team wiki", "unrelated"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Goal (the prompt refresh's inputs, hermes::prompt_inputs): what the
    /// skills index is built from is read, and nothing else: each SKILL.md
    /// and DESCRIPTION.md (in any order: the digest sorts), a larger one than an install takes
    /// by its size alone, each dir's own budget, a cut said, and a dir that
    /// is not there counting as empty.
    #[test]
    fn the_files_a_skills_index_is_built_from() {
        let root = std::env::temp_dir().join(format!("hermes-boot-indexed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (managed, view) = (root.join("managed"), root.join("view"));
        let put = |path: &Path, bytes: &[u8]| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        };
        put(&managed.join("notes/garden-log/SKILL.md"), b"name: garden-log");
        put(&managed.join("notes/DESCRIPTION.md"), b"notes");
        put(&managed.join("notes/garden-log/scripts/log.py"), b"print(1)");
        put(&managed.join("notes/garden-log/README.md"), b"not indexed");
        put(&managed.join("big/SKILL.md"), &vec![b'x'; FILE_MAX_BYTES + 1]);
        put(&view.join("fragment/SKILL.md"), b"name: fragment");
        let (found, cut) = indexed(&[&managed, &view, &root.join("gone")]);
        let mut paths: Vec<String> = found.iter().map(|(p, _)| p.strip_prefix(&root.display().to_string()).unwrap_or(p).to_string()).collect();
        paths.sort();
        let bytes_of = |end: &str| found.iter().find(|(p, _)| p.ends_with(end)).map(|(_, b)| b.clone()).unwrap_or_default();
        assert_eq!(paths, ["/managed/big/SKILL.md", "/managed/notes/DESCRIPTION.md", "/managed/notes/garden-log/SKILL.md", "/view/fragment/SKILL.md"], "{paths:?}");
        assert_eq!(bytes_of("/big/SKILL.md"), (FILE_MAX_BYTES + 1).to_string().into_bytes(), "a file past an install's bound counts as its size");
        assert_eq!(bytes_of("/garden-log/SKILL.md"), b"name: garden-log");
        assert!(!cut, "nothing cut");
        // each dir its own budget: a full managed set leaves the view watched
        for i in 0..FILES_MAX {
            put(&managed.join(format!("many/s{i:04}/SKILL.md")), b"name: s");
        }
        let (found, cut) = indexed(&[&managed, &view]);
        assert!(cut, "the managed set past its budget is cut, and said");
        assert_eq!(found.iter().filter(|(p, _)| p.starts_with(&managed.display().to_string())).count(), FILES_MAX);
        assert!(found.iter().any(|(p, _)| p.ends_with("/view/fragment/SKILL.md")), "the view keeps its own budget");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A profile's hidden connection skills are a prompt input: its
    /// `skills.disabled` line alone (a model change is not one), each
    /// profile's that hides any, so a connect or a disconnect asks the open
    /// chats to rebuild their prompts.
    #[test]
    fn a_profiles_hidden_skills_are_a_prompt_input() {
        let config = |model: &str, hidden: &[&str]| {
            let line = if hidden.is_empty() { String::new() } else { format!("  disabled: [{}]\n", hidden.iter().map(|h| format!("\"{h}\"")).collect::<Vec<_>>().join(", ")) };
            format!("model:\n  default: {model}\n  disabled: [\"x\"]\nskills:\n  external_dirs: [\"/m\"]\n{line}plugins:\n  disabled:\n    - p\n")
        };
        assert_eq!(hidden_of(&config("a", &["linear-finite"])), "  disabled: [\"linear-finite\"]");
        assert_eq!(hidden_of(&config("a", &[])), "", "none hidden: no line, nor another block's");
        assert_eq!(hidden_of(&config("a", &["linear-finite"])), hidden_of(&config("b", &["linear-finite"])), "a model change is not one");
        let root = std::env::temp_dir().join(format!("hermes-boot-hidden-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let put = |profile: &str, text: &str| {
            std::fs::create_dir_all(root.join(profile)).unwrap();
            std::fs::write(root.join(profile).join("config.yaml"), text).unwrap();
        };
        put("juniper--k3x9", &config("a", &["google-workspace-finite", "linear-finite"]));
        put("fred--k3x9", &config("a", &[]));
        let before = hidden_lists(&root);
        assert_eq!(before.len(), 1, "only a profile that hides any: {before:?}");
        put("juniper--k3x9", &config("b", &["google-workspace-finite", "linear-finite"]));
        assert_eq!(hidden_lists(&root), before, "a model change asks nothing");
        put("juniper--k3x9", &config("b", &["linear-finite"]));
        assert_ne!(hidden_lists(&root), before, "a connect is a change");
        assert!(hidden_lists(&root.join("gone")).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    fn frag(name: &str, kind: &str, owned: bool) -> FragmentEntry {
        FragmentEntry { name: name.into(), role: "editor".into(), kind: kind.into(), owned, title: String::new() }
    }

    fn file(path: &str, size: u64, version: &str) -> FileEntry {
        FileEntry { path: path.into(), size, last_commit_sha: version.into() }
    }

    /// Valid: the owner's skills fragment, one labelled `skills` first.
    /// Invalid: someone else's shared with them, a fragment of another kind.
    #[test]
    fn the_owners_skills_fragment_is_found_by_kind_and_owner() {
        let list = [frag("garden--k3x9", "app", true), frag("skills--p2m4", "skills", false), frag("extra--r7t5", "skills", true), frag("skills--z8w6", "skills", true)];
        assert_eq!(pick(&list).as_deref(), Some("skills--z8w6"));
        assert_eq!(pick(&list[..3]).as_deref(), Some("extra--r7t5"), "another label of theirs");
        assert_eq!(pick(&list[..2]), None, "a skills fragment shared with them is not theirs");
        assert_eq!(pick(&[]), None);
    }

    #[test]
    fn only_safe_paths_are_installed() {
        for ok in ["research/arxiv-finite/SKILL.md", "grill-me/SKILL.md", "a/b/c/d.py"] {
            assert!(valid_rel(ok), "{ok}");
        }
        for bad in ["", "../x", "a/../b", "/etc/passwd", "a//b", "a/.hidden", "a/__pycache__/x.pyc", "a/b c", "a\\b", &"a/".repeat(9), &"x".repeat(301)] {
            assert!(!valid_rel(bad), "{bad}");
        }
    }

    /// Valid: a first install fetches the managed set (and nothing else of
    /// the fragment's); a change fetches what changed and removes what
    /// went. Replay: a settled install does nothing. Invalid: a file past a
    /// bound, or at an unsafe path, is refused and the rest installs.
    #[test]
    fn a_plan_fetches_what_changed_and_removes_what_went() {
        let listing = vec![
            file("fragment.json", 40, "c1"),
            file("skills/research/arxiv-finite/SKILL.md", 100, "release:a"),
            file("skills/research/arxiv-finite/scripts/x.py", 200, "release:b"),
            file("skills/grill-me/SKILL.md", 50, "c2"),
        ];
        let first = plan(Some(&listing), &Installed::new());
        assert_eq!(first.fetch.iter().map(|f| f.0.as_str()).collect::<Vec<_>>(), ["research/arxiv-finite/SKILL.md", "research/arxiv-finite/scripts/x.py", "grill-me/SKILL.md"]);
        assert!(first.remove.is_empty() && first.refused.is_empty());
        assert_eq!(skill_names(&first.wanted), ["arxiv-finite", "grill-me"].map(String::from).into_iter().collect());
        // replay: what is installed is the listing
        let settled = plan(Some(&listing), &first.wanted);
        assert!(settled.fetch.is_empty() && settled.remove.is_empty(), "{settled:?}");
        // a release changed one file and dropped another
        let next = vec![file("skills/research/arxiv-finite/SKILL.md", 120, "release:a2"), file("skills/grill-me/SKILL.md", 50, "c2")];
        let changed = plan(Some(&next), &first.wanted);
        assert_eq!(changed.fetch, vec![("research/arxiv-finite/SKILL.md".to_string(), "skills/research/arxiv-finite/SKILL.md".to_string(), "release:a2".to_string())]);
        assert_eq!(changed.remove, vec!["research/arxiv-finite/scripts/x.py".to_string()]);
        // no skills fragment: everything goes
        let none = plan(None, &first.wanted);
        assert!(none.fetch.is_empty() && none.wanted.is_empty() && none.remove.len() == 3);
        // past a bound, or unsafe: refused, the rest installs
        let bad = vec![file("skills/x/../../etc/passwd", 1, "v"), file("skills/big/SKILL.md", FILE_MAX_BYTES as u64 + 1, "v"), file("skills/ok/SKILL.md", 1, "v")];
        let p = plan(Some(&bad), &Installed::new());
        assert_eq!(p.refused.len(), 2);
        assert_eq!(p.wanted.keys().collect::<Vec<_>>(), ["ok/SKILL.md"]);
        let many: Vec<FileEntry> = (0..FILES_MAX + 5).map(|i| file(&format!("skills/s{i}/SKILL.md"), 1, "v")).collect();
        let p = plan(Some(&many), &Installed::new());
        assert_eq!((p.wanted.len(), p.refused.len()), (FILES_MAX, 5));
    }

    #[test]
    fn the_reader_is_an_agent_of_the_computers_owner() {
        let a = |f: &str, o: &str| Agent { fragment: f.into(), identity: format!("npub1{f}"), name: f.into(), owner: o.into(), credentials: vec![], key: None, runtime: None, claude: Default::default() };
        let agents = [a("x.skyler", "npub1skyler"), a("juniper--k3x9", "npub1paul")];
        assert_eq!(reader(&agents, "npub1paul").map(|r| r.fragment.as_str()), Some("juniper--k3x9"));
        assert!(reader(&agents[..1], "npub1paul").is_none());
    }

    /// Goal: an install against the files route writes the managed set, a
    /// change of it is followed, and what went is removed; a platform that
    /// does not answer changes nothing installed. Method: a fake API with
    /// an owner's skills fragment; the install runs as the agent for its owner.
    #[tokio::test]
    async fn an_install_follows_the_skills_fragment() {
        use std::sync::{Arc, Mutex};
        fragment_bridge::log::set_quiet(true);
        let files: fake::Files = Arc::new(Mutex::new(BTreeMap::new()));
        files.lock().unwrap().insert("skills/research/arxiv-finite/SKILL.md".into(), ("release:a".into(), b"---\nname: arxiv-finite\n---\n".to_vec()));
        files.lock().unwrap().insert("skills/grill-me/SKILL.md".into(), ("release:g".into(), b"---\nname: grill-me\n---\n".to_vec()));
        files.lock().unwrap().insert("fragment.json".into(), ("c1".into(), b"{}".to_vec()));
        files.lock().unwrap().insert("optional/hermes/creative/excalidraw.on".into(), ("c2".into(), b"on\n".to_vec()));
        let down = Arc::new(Mutex::new(false));
        let (addr, _stop) = fake::start(files.clone(), down.clone(), Arc::new(Mutex::new(true))).await;
        let api = Api::new(&format!("http://{addr}")).unwrap();
        let agent = Agent { fragment: "juniper--k3x9".into(), identity: "npub1j".into(), name: "Juniper".into(), owner: "npub1paul".into(), credentials: vec![], key: None, runtime: None, claude: Default::default() };
        let root = std::env::temp_dir().join(format!("hermes-boot-skills-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (dir, manifest) = (root.join("managed"), root.join("sync/managed.json"));

        let d = install(&api, &agent, "npub1paul", &dir, &manifest, &root.join("sync/optional.json")).await.unwrap();
        assert_eq!((d.fragment.as_deref(), d.fetched, d.skills, d.optional), (Some("skills--z8w6"), 2, 2, 1));
        assert!(dir.join("research/arxiv-finite/SKILL.md").exists() && !dir.join("fragment.json").exists());
        let chosen = root.join("sync/optional.json");
        assert_eq!(load_set(&chosen), BTreeSet::from(["creative/excalidraw".to_string()]), "its switches, saved");
        assert!(!dir.join("optional").exists(), "a switch is no managed file");
        let replay = install(&api, &agent, "npub1paul", &dir, &manifest, &root.join("sync/optional.json")).await.unwrap();
        assert_eq!((replay.fetched, replay.removed), (0, 0), "settled");
        // a file gone from under the manifest is fetched again
        std::fs::remove_file(dir.join("grill-me/SKILL.md")).unwrap();
        let repaired = install(&api, &agent, "npub1paul", &dir, &manifest, &root.join("sync/optional.json")).await.unwrap();
        assert_eq!(repaired.fetched, 1);
        assert!(dir.join("grill-me/SKILL.md").exists());

        // the release changes one skill and drops the other
        files.lock().unwrap().insert("skills/research/arxiv-finite/SKILL.md".into(), ("release:a2".into(), b"---\nname: arxiv-finite\n---\nnew\n".to_vec()));
        files.lock().unwrap().remove("skills/grill-me/SKILL.md");
        let d = install(&api, &agent, "npub1paul", &dir, &manifest, &root.join("sync/optional.json")).await.unwrap();
        assert_eq!((d.fetched, d.removed, d.skills), (1, 1, 1));
        assert!(std::fs::read_to_string(dir.join("research/arxiv-finite/SKILL.md")).unwrap().ends_with("new\n"));
        assert!(!dir.join("grill-me").exists(), "its directory goes with its last file");

        // switched off
        files.lock().unwrap().remove("optional/hermes/creative/excalidraw.on");
        files.lock().unwrap().insert("optional/hermes/research/llm-wiki.on".into(), ("c3".into(), b"on\n".to_vec()));
        install(&api, &agent, "npub1paul", &dir, &manifest, &chosen).await.unwrap();
        assert_eq!(load_set(&chosen), BTreeSet::from(["research/llm-wiki".to_string()]));

        // the platform down: nothing installed changes, the switches neither
        *down.lock().unwrap() = true;
        assert!(install(&api, &agent, "npub1paul", &dir, &manifest, &root.join("sync/optional.json")).await.is_err());
        assert!(dir.join("research/arxiv-finite/SKILL.md").exists());
        assert_eq!(load_set(&chosen), BTreeSet::from(["research/llm-wiki".to_string()]));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Hermes' rule for a profile's skills (its `resolve_skill_catalog`,
    /// `agent/skill_utils.py`, since v0.21.6): the profile's own
    /// dir ranks above its external dirs, which are one rank. A skill is
    /// known by its name, its directory's name and its path under its dir;
    /// the best rank holding a name wins it, and two skills of that rank
    /// holding it (not copies of one) are ambiguous: neither is found by it.
    /// Each name found → the file that is its skill.
    fn hermes_finds(own: &Path, external: &[PathBuf]) -> BTreeMap<String, PathBuf> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else { return };
            for e in entries.filter_map(Result::ok) {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.file_name().is_some_and(|n| n == "SKILL.md") {
                    out.push(p);
                }
            }
        }
        // (rank, name, aliases, file)
        let mut skills = vec![];
        for (rank, dir) in std::iter::once((0, own)).chain(external.iter().map(|d| (1, d.as_path()))) {
            let mut files = vec![];
            walk(dir, &mut files);
            for f in files {
                let dir_name = f.parent().unwrap().file_name().unwrap().to_string_lossy().into_owned();
                let name = skill_name(&std::fs::read_to_string(&f).unwrap(), &dir_name);
                let rel = f.parent().unwrap().strip_prefix(dir).unwrap().to_string_lossy().into_owned();
                skills.push((rank, name.clone(), BTreeSet::from([name, dir_name, rel]), f));
            }
        }
        let mut found = BTreeMap::new();
        for (rank, name, _, file) in &skills {
            let holders: Vec<_> = skills.iter().filter(|s| s.2.contains(name)).collect();
            let best = holders.iter().map(|s| s.0).min().unwrap();
            let at_best: Vec<_> = holders.iter().filter(|s| s.0 == best).collect();
            if *rank == best && at_best.len() == 1 {
                found.insert(name.clone(), file.clone());
            }
        }
        found
    }

    #[test]
    fn a_skills_name_is_its_frontmatters_else_its_directorys() {
        assert_eq!(skill_name("---\nname: fragment\ndescription: x\n---\n# body\n", "other"), "fragment");
        assert_eq!(skill_name("---\nname: \"fragment\"\n---\n", "other"), "fragment");
        assert_eq!(skill_name("\u{feff}---\r\nname: 'fragment'\r\n---\r\n", "other"), "fragment");
        assert_eq!(skill_name("---\ndescription: no name\n---\n", "fragment"), "fragment");
        assert_eq!(skill_name("# no frontmatter\nname: fragment\n", "notes"), "notes", "a body's line is no name");
    }

    /// Bundled skills are visible without a skills fragment, managed
    /// names/aliases suppress them, agent-owned skills win, and removing a
    /// managed override restores the image's unchanged skill. Replay and a
    /// fresh view (a restart) produce the same catalog.
    #[test]
    fn bundled_skills_yield_to_managed_aliases_and_return_when_removed() {
        let root = std::env::temp_dir().join(format!("hermes-bundled-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (bundled, managed, manifest, view, own) = (root.join("bundled"), root.join("managed"), root.join("manifest.json"), root.join("view"), root.join("own"));
        write_file(&bundled, "productivity/pdf/SKILL.md", b"---\nname: pdf\n---\nThe image's PDF.\n").unwrap();
        write_file(&bundled, "productivity/pdf/scripts/create.py", b"print('pdf')\n").unwrap();
        let settle = || settle_bundled(&managed, &manifest, &bundled, &view).unwrap();
        settle();
        settle();
        assert_eq!(hermes_finds(&own, &[managed.clone(), view.clone()]).get("pdf"), Some(&view.join("productivity/pdf/SKILL.md")));
        assert!(view.join("productivity/pdf/scripts/create.py").is_file(), "helpers travel with the skill");
        write_file(&managed, "pdf/SKILL.md", b"---\nname: custom-pdf\n---\nThe owner's PDF.\n").unwrap();
        save_manifest(&manifest, &BTreeMap::from([("pdf/SKILL.md".into(), "release:1".into())])).unwrap();
        assert!(!hermes_finds(&own, &[managed.clone(), view.clone()]).contains_key("pdf"), "same-rank alias is ambiguous before settling");
        settle();
        settle();
        assert!(!view.join("productivity/pdf").exists(), "a managed alias with a different declared name also wins");
        assert!(hermes_finds(&own, &[managed.clone(), view.clone()]).contains_key("custom-pdf"));
        remove_file(&managed, "pdf/SKILL.md");
        save_manifest(&manifest, &BTreeMap::new()).unwrap();
        settle();
        assert!(hermes_finds(&own, &[managed.clone(), view.clone()]).contains_key("pdf"));
        write_file(&managed, "SKILL.md", b"---\nname: pdf\n---\nA root-level managed PDF.\n").unwrap();
        save_manifest(&manifest, &BTreeMap::from([("SKILL.md".into(), "release:2".into())])).unwrap();
        settle();
        assert!(!view.join("productivity/pdf").exists(), "Hermes also discovers SKILL.md directly at a search root");
        remove_file(&managed, "SKILL.md");
        save_manifest(&manifest, &BTreeMap::new()).unwrap();
        settle();
        assert!(view.join("productivity/pdf/scripts/create.py").is_file(), "removing the root-level override restores helpers too");
        write_file(&own, "pdf/SKILL.md", b"---\nname: pdf\n---\nThe agent's PDF.\n").unwrap();
        assert_eq!(hermes_finds(&own, &[managed.clone(), view.clone()]).get("pdf"), Some(&own.join("pdf/SKILL.md")));
        std::fs::remove_dir_all(&view).unwrap();
        settle();
        assert!(view.join("productivity/pdf/SKILL.md").is_file(), "a fresh boot rebuilds the view");
        assert!(std::fs::read_to_string(bundled.join("productivity/pdf/SKILL.md")).unwrap().contains("image's PDF"));
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Valid: each `<id>.on` under the prefix is a switch, nested ids too.
    /// Invalid: an unsafe id, a file of another name, and switches past
    /// the bound are refused or ignored; the rest stand.
    #[test]
    fn a_listings_switches_are_its_on_files() {
        let listing = vec![
            file("optional/hermes/creative/excalidraw.on", 3, "c1"),
            file("optional/hermes/mlops/training/unsloth.on", 3, "c2"),
            file("optional/hermes/creative/pixel-art.off", 3, "c3"),
            file("optional/other/creative/sketch.on", 3, "c4"),
            file("optional/hermes/../../etc/passwd.on", 3, "c5"),
            file("skills/research/arxiv-finite/SKILL.md", 100, "release:a"),
        ];
        let (on, refused) = switched_on(&listing);
        assert_eq!(on, BTreeSet::from(["creative/excalidraw".to_string(), "mlops/training/unsloth".to_string()]));
        assert_eq!(refused, vec![("optional/hermes/../../etc/passwd.on".to_string(), "not a safe path")]);
        let many: Vec<FileEntry> = (0..OPTIONAL_MAX + 3).map(|i| file(&format!("optional/hermes/c/s{i}.on"), 3, "v")).collect();
        let (on, refused) = switched_on(&many);
        assert_eq!((on.len(), refused.len()), (OPTIONAL_MAX, 3));
    }

    /// Goal: an optional skill switched on is one Hermes finds, helpers and
    /// all, at any depth; switched off, it goes and its empty directories
    /// with it, never a bundled skill beside it. Invalid: one the image
    /// lacks, or whose name a managed or bundled skill takes (which would
    /// make both ambiguous), is left out and said; one that cannot be
    /// copied is said and the rest show. Replay changes nothing, and a
    /// fresh view (a restart) shows the same.
    #[test]
    fn optional_skills_switched_on_are_shown_and_switched_off_go() {
        let root = std::env::temp_dir().join(format!("hermes-optional-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let at = |d: &str| root.join(d);
        let (bundled, optional, managed, manifest, chosen, shown, view, own) = (at("bundled"), at("optional"), at("managed"), at("manifest.json"), at("chosen.json"), at("shown.json"), at("view"), at("own"));
        write_file(&bundled, "creative/ascii-art/SKILL.md", b"---\nname: ascii-art\n---\n").unwrap();
        write_file(&optional, "creative/excalidraw/SKILL.md", b"---\nname: excalidraw\n---\n").unwrap();
        write_file(&optional, "creative/excalidraw/references/shapes.md", b"shapes\n").unwrap();
        write_file(&optional, "mlops/training/unsloth/SKILL.md", b"---\nname: unsloth\n---\n").unwrap();
        write_file(&optional, "creative/ascii-art/SKILL.md", b"---\nname: ascii-art\n---\nAnother ASCII art.\n").unwrap();
        write_file(&optional, "research/arxiv/SKILL.md", b"---\nname: arxiv-plus\n---\n").unwrap();
        write_file(&managed, "research/arxiv/SKILL.md", b"---\nname: arxiv\n---\n").unwrap();
        save_manifest(&manifest, &BTreeMap::from([("research/arxiv/SKILL.md".into(), "release:1".into())])).unwrap();
        let choose = |ids: &[&str]| save_json(&chosen, &ids.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>()).unwrap();
        let settle = || {
            settle_bundled(&managed, &manifest, &bundled, &view).unwrap();
            settle_optional(&managed, &manifest, &bundled, &optional, &chosen, &shown, &view).unwrap()
        };
        let finds = || hermes_finds(&own, &[managed.clone(), view.clone()]);

        // nothing switched on: none shown
        assert_eq!(settle(), OptionalShown::default());
        assert!(!view.join("creative/excalidraw").exists());

        choose(&["creative/excalidraw", "mlops/training/unsloth", "creative/ascii-art", "research/arxiv", "security/gone"]);
        let o = settle();
        assert_eq!(o.shown, ["creative/excalidraw", "mlops/training/unsloth"]);
        assert_eq!(o.missing, ["security/gone"], "another pin's: left out");
        assert_eq!(o.withheld, ["creative/ascii-art", "research/arxiv"], "a bundled name, a managed alias");
        let found = finds();
        assert_eq!(found.get("excalidraw"), Some(&view.join("creative/excalidraw/SKILL.md")));
        assert_eq!(found.get("unsloth"), Some(&view.join("mlops/training/unsloth/SKILL.md")), "nested ids show too");
        assert_eq!(found.get("ascii-art"), Some(&view.join("creative/ascii-art/SKILL.md")), "the bundled one, unambiguous");
        assert!(std::fs::read_to_string(view.join("creative/ascii-art/SKILL.md")).unwrap().ends_with("---\n"), "the bundled one's text");
        assert_eq!(found.get("arxiv"), Some(&managed.join("research/arxiv/SKILL.md")));
        assert!(view.join("creative/excalidraw/references/shapes.md").is_file(), "helpers travel with it");
        assert_eq!(settle(), o, "replay");

        // switched off: gone, its empty parents too, the bundled skill beside it stays
        choose(&["creative/excalidraw"]);
        let o = settle();
        assert_eq!(o.shown, ["creative/excalidraw"]);
        assert!(!view.join("mlops").exists(), "its empty directories go");
        assert!(view.join("creative/ascii-art/SKILL.md").is_file() && view.join("creative/excalidraw/SKILL.md").is_file());
        assert!(!finds().contains_key("unsloth"));

        // a restart: a fresh view shows the same
        std::fs::remove_dir_all(&view).unwrap();
        std::fs::remove_file(&shown).unwrap();
        assert_eq!(settle().shown, ["creative/excalidraw"]);
        assert!(finds().contains_key("excalidraw"));

        // one that cannot be copied (a non-file in it) is said, the rest show
        write_file(&optional, "devops/odd/SKILL.md", b"---\nname: odd\n---\n").unwrap();
        std::os::unix::fs::symlink("/etc/hostname", optional.join("devops/odd/link")).unwrap();
        choose(&["creative/excalidraw", "devops/odd"]);
        let o = settle();
        assert_eq!((o.shown.as_slice(), o.failed.len()), (&["creative/excalidraw".to_string()][..], 1));
        assert!(!view.join("devops/odd").exists(), "never half a skill");

        // switched off goes even when the settle fails after (a managed
        // skill it cannot read)
        std::fs::remove_file(managed.join("research/arxiv/SKILL.md")).unwrap();
        choose(&[]);
        assert!(settle_optional(&managed, &manifest, &bundled, &optional, &chosen, &shown, &view).is_err());
        assert!(!view.join("creative/excalidraw").exists());
        write_file(&managed, "research/arxiv/SKILL.md", b"---\nname: arxiv\n---\n").unwrap();
        choose(&["creative/excalidraw"]);
        assert_eq!(settle().shown, ["creative/excalidraw"]);

        // all switched off
        choose(&[]);
        assert!(settle().shown.is_empty());
        assert!(!view.join("creative/excalidraw").exists() && view.join("creative/ascii-art/SKILL.md").is_file());
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Hermes' skill guard: a skill whose text holds one of these is
    /// flagged as a prompt injection (its `_INJECTION_PATTERNS`).
    const HERMES_INJECTION_PATTERNS: [&str; 9] = ["ignore previous instructions", "ignore all previous", "you are now", "disregard your", "forget your instructions", "new instructions:", "system prompt:", "<system>", "]]>"];

    /// `cli/SKILL.md` as `fragment skill` prints it at the image's build,
    /// which `PLATFORM_MAX_BYTES` holds: its blanks filled with the CLI's
    /// default host (the build has no config: cli/src/main.rs
    /// `DEFAULT_HOST`) and its stand-in for a fragment's origin
    /// (crates/templates `ANY_FRAGMENT`). The file as committed is about 110
    /// bytes shorter: measured unfilled, this passed while the image's
    /// build refused a skill of 16 451 bytes.
    fn cli_skill() -> String {
        include_str!("../../../../cli/SKILL.md").replace("{{platform}}", "https://finite.chat").replace("{{fragment}}", "https://<name>.<the platform's domain for fragments>")
    }

    /// Valid: the platform skill is named `fragment`, says what the
    /// computer adds before the CLI's own skill, whole, and reads to Hermes
    /// as a skill (one frontmatter, a one-line description it lists, nothing
    /// its guard flags or its preprocessing would run). Invalid: a CLI skill
    /// with no frontmatter, or another name, is refused.
    /// Goal (Paul's prod agent, 2026-11-02: `session_search` found nothing of
    /// a conversation from before its computer, which the Session Archive
    /// held): every agent is told what Hermes' `session_search` sees, and
    /// how to search the rest, in the page and in the description that gets
    /// it loaded.
    #[test]
    fn an_agent_is_told_where_past_conversations_are() {
        for said in ["`session_search` sees only this computer's other chats since it started", "never the one you are in", "chats imported from elsewhere", "fragment search --all \"<words>\"", "`--in <fragment>`"] {
            assert!(COMPUTER_PAGE.contains(said), "computer.md says {said}");
        }
        assert!(PLATFORM_DESCRIPTION.contains("to search past conversations and brains (`fragment search`)"));
    }

    /// Goal (P5 of the plan to sandbox per agent): an agent plans
    /// honestly, knowing a sleep or restart ends the servers and background
    /// commands it started and that nothing starts them again, where what
    /// must outlive its turn lives instead, and (P6) that one still working
    /// holds the computer only up to the bridge's bound, which both pages
    /// name as the code keeps it.
    #[test]
    fn an_agent_is_told_a_sleep_ends_its_processes() {
        let page = COMPUTER_PAGE.split_whitespace().collect::<Vec<_>>().join(" ");
        let hours = fragment_bridge::limits::LEFT_RUNNING_HOLD_MS / 3_600_000;
        assert_eq!(hours * 3_600_000, fragment_bridge::limits::LEFT_RUNNING_HOLD_MS, "a bound in whole hours");
        let held = format!("keeps your computer awake up to {hours} hours after your turn; an idle server does not");
        for said in ["A sleep or a restart ends every process you started (a server, a background command, a loop), and nothing starts it again: never rely on crontab or `hermes cron`", "a schedule on yourself, kept through every restart and sleep", "What should run without you (frequent checks, or for others) is an app", &held] {
            assert!(page.contains(said), "computer.md says {said}");
        }
        let guide = include_str!("../../../../cli/GUIDE.md").split_whitespace().collect::<Vec<_>>().join(" ");
        let held = format!("keeps the computer awake up to {hours} hours after the agent's last turn; an idle server does not");
        assert!(guide.contains(&held), "GUIDE.md says {held}");
    }

    #[test]
    fn the_platform_skill_is_the_clis_after_the_computers_page() {
        let cli = &cli_skill();
        assert!(!cli.contains("{{platform}}") && !cli.contains("{{fragment}}"), "both blanks filled, as the CLI fills them (a secret's `{{NAME}}` is none)");
        let skill = platform_skill(cli).unwrap();
        assert!(skill.starts_with("---\nname: fragment\ndescription: \"You are an agent on a Fragment computer"), "{skill}");
        let (front, body) = skill.strip_prefix("---\n").and_then(|r| r.split_once("\n---\n")).unwrap();
        assert_eq!(front.lines().count(), 2, "a name and a description: {front}");
        let cli_body = cli.strip_prefix("---\n").and_then(|r| r.split_once("\n---\n")).unwrap().1.trim();
        assert!(body.contains(cli_body), "the CLI's skill, whole");
        let page = body.find("# Your computer").unwrap();
        assert!(page < body.find("# fragment").unwrap(), "the computer's page first");
        for said in ["FRAGMENT_AS_AGENT", "skip its Install and Pair", "apps-finite", "brain-finite", "GOOGLE_OAUTH_ACCESS_TOKEN", "google-workspace-finite", "\"Its screen\"", "take over", "human_has_control", "another agent's desktop", "fragment create", "fragment write", "fragment deploy", "fragment call", "fragment list", "fragment schedule add --routine", "`hermes cron`", "every 15 minutes", "a cron trigger and a job", "A routine may end with exactly `[SILENT]` when there is nothing to tell", "fragment quote", "ask once, in one\nsentence"] {
            assert!(body.contains(said), "it says {said:?}");
        }
        // Paul, 2026-10-20: an agent asked to check something hourly found no
        // scheduler and started a loop that died with its container
        assert!(PLATFORM_DESCRIPTION.contains("anything later or recurring (`fragment schedule`)"), "an agent asked for anything on a schedule loads it");
        let lower = skill.to_lowercase();
        assert!(HERMES_INJECTION_PATTERNS.iter().all(|p| !lower.contains(p)), "nothing Hermes' guard flags");
        assert!(!skill.contains("!`") && !skill.contains("${HERMES_"), "nothing Hermes' preprocessing runs or fills");
        assert!(skill.len() <= PLATFORM_MAX_BYTES && PLATFORM_DESCRIPTION.len() <= 1024, "a page, and a description Hermes lists whole");
        assert!(platform_skill(cli_body).is_err(), "no frontmatter");
        assert!(platform_skill(&cli.replacen("name: fragment", "name: other", 1)).is_err(), "another name");
        assert!(platform_skill(&format!("{cli}{}", "x".repeat(PLATFORM_MAX_BYTES))).is_err(), "past its bound");
    }

    /// The real platform skill passes. A bundled skill's missing reference
    /// fails the build check; shipping it fixes the check, including nested
    /// references, and removing it fails again.
    #[test]
    fn platform_references_exist_and_missing_ones_fail() {
        let root = std::env::temp_dir().join(format!("hermes-boot-references-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let platform = root.join("platform");
        let skill = platform_skill(&cli_skill()).unwrap();
        write_platform_skill(&platform, &skill).unwrap();
        check_references(&platform).unwrap();
        write_file(&platform, "test/example/SKILL.md", b"Read `references/example.md`.\n").unwrap();
        assert!(check_references(&platform).unwrap_err().to_string().contains("references/example.md"));
        write_file(&platform, "test/example/references/example.md", b"Read `references/missing.md`.\n").unwrap();
        assert!(check_references(&platform).unwrap_err().to_string().contains("references/missing.md"));
        write_file(&platform, "test/example/references/missing.md", b"The nested reference.\n").unwrap();
        check_references(&platform).unwrap();
        std::fs::remove_file(platform.join("test/example/references/example.md")).unwrap();
        assert!(check_references(&platform).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_skills_fragment_is_looked_for_sooner_while_there_is_none() {
        assert_eq!(next_install_ms(Some(true), 600_000), 600_000);
        assert_eq!(next_install_ms(None, 600_000), 600_000, "an install that failed waits its cadence");
        assert_eq!(next_install_ms(Some(false), 600_000), ABSENT_EVERY_MS);
        assert_eq!(next_install_ms(Some(false), 2_000), 2_000, "a test's cadence is never slowed");
    }

    /// Goal: the platform skill is every profile's unless a managed skill
    /// takes its name. Valid: with no skills fragment it is the `fragment`
    /// Hermes finds; a managed `fragment` shadows it (the platform skill
    /// leaves the view). Invalid: both in the external dirs, Hermes finds
    /// no `fragment` at all. Replay: the managed one gone, it is found
    /// again, as it was written. Method: the managed set installed against
    /// the fake API, the view settled after each install as the boot does,
    /// and Hermes' rule over a profile's own dir and `EXTERNAL_DIRS`.
    #[tokio::test]
    async fn the_platform_skill_is_there_with_no_skills_fragment_and_a_managed_one_shadows_it() {
        use std::sync::{Arc, Mutex};
        fragment_bridge::log::set_quiet(true);
        let files: fake::Files = Arc::new(Mutex::new(BTreeMap::new()));
        let listed = Arc::new(Mutex::new(false));
        let (addr, _stop) = fake::start(files.clone(), Arc::new(Mutex::new(false)), listed.clone()).await;
        let api = Api::new(&format!("http://{addr}")).unwrap();
        let agent = Agent { fragment: "juniper--k3x9".into(), identity: "npub1j".into(), name: "Juniper".into(), owner: "npub1paul".into(), credentials: vec![], key: None, runtime: None, claude: Default::default() };
        let root = std::env::temp_dir().join(format!("hermes-boot-platform-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (own, manifest) = (root.join("profile/skills"), root.join("sync/managed.json"));
        // the profile's dirs as its config names them, under the test's root
        let at = |d: &str| root.join(d.trim_start_matches('/'));
        let external: Vec<PathBuf> = EXTERNAL_DIRS.iter().map(|d| at(d)).collect();
        let (managed, platform, view) = (at(MANAGED_DIR), at(PLATFORM_DIR), at(PLATFORM_VIEW));
        let skill = platform_skill(&cli_skill()).unwrap();
        write_platform_skill(&platform, &skill).unwrap();
        std::fs::create_dir_all(own.join("garden-notes")).unwrap();
        std::fs::write(own.join("garden-notes/SKILL.md"), "---\nname: garden-notes\n---\n").unwrap();
        let settle = || settle_platform(&managed, &manifest, &platform, &view).unwrap();

        // no skills fragment: no managed skills, the platform's `fragment`
        let d = install(&api, &agent, "npub1paul", &managed, &manifest, &root.join("sync/optional.json")).await.unwrap();
        assert_eq!((d.fragment.as_deref(), d.skills), (None, 0));
        assert!(settle(), "shown");
        let found = hermes_finds(&own, &external);
        assert_eq!(found.get("fragment"), Some(&view.join(PLATFORM_PATH)), "{found:?}");
        assert!(found.contains_key("garden-notes"));

        // the skills fragment appears, holding a `fragment` of its own: it wins
        *listed.lock().unwrap() = true;
        files.lock().unwrap().insert("skills/fragment/SKILL.md".into(), ("release:f".into(), b"---\nname: fragment\ndescription: The managed one.\n---\n".to_vec()));
        files.lock().unwrap().insert("skills/grill-me/SKILL.md".into(), ("release:g".into(), b"---\nname: grill-me\n---\n".to_vec()));
        let d = install(&api, &agent, "npub1paul", &managed, &manifest, &root.join("sync/optional.json")).await.unwrap();
        assert_eq!((d.fragment.as_deref(), d.skills), (Some("skills--z8w6"), 2));
        // invalid: both beside each other in one rank, Hermes finds neither
        assert_eq!(hermes_finds(&own, &external).get("fragment"), None, "ambiguous");
        assert!(!settle(), "shadowed");
        assert!(view.is_dir() && !view.join(PLATFORM_PATH).exists(), "the view stays, without it");
        let found = hermes_finds(&own, &external);
        assert_eq!(found.get("fragment"), Some(&managed.join("fragment/SKILL.md")), "the managed one shadows it: {found:?}");
        assert!(found.contains_key("grill-me") && found.contains_key("garden-notes"));
        assert!(!settle(), "replay: still shadowed");

        // the managed one gone: the platform's again, untouched
        files.lock().unwrap().remove("skills/fragment/SKILL.md");
        let d = install(&api, &agent, "npub1paul", &managed, &manifest, &root.join("sync/optional.json")).await.unwrap();
        assert_eq!((d.removed, d.skills), (1, 1));
        assert!(settle());
        assert_eq!(hermes_finds(&own, &external).get("fragment"), Some(&view.join(PLATFORM_PATH)));
        assert_eq!(std::fs::read_to_string(view.join(PLATFORM_PATH)).unwrap(), skill, "the image's, as written");
        assert_eq!(std::fs::read_to_string(platform.join(PLATFORM_PATH)).unwrap(), skill, "no install touches it");
        // an agent's own `fragment` wins over the platform's (its own rank)
        std::fs::create_dir_all(own.join("fragment")).unwrap();
        std::fs::write(own.join("fragment/SKILL.md"), "---\nname: fragment\n---\n").unwrap();
        assert_eq!(hermes_finds(&own, &external).get("fragment"), Some(&own.join("fragment/SKILL.md")));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The fragment list and a skills fragment's files, as the platform
    /// answers an agent acting for its owner (`for`); `listed` says whether
    /// the owner has the skills fragment.
    mod fake {
        use std::collections::BTreeMap;
        use std::net::SocketAddr;
        use std::sync::{Arc, Mutex};

        use fragment_bridge::net;
        use hyper::{Method, StatusCode};
        use serde_json::{json, Value};

        pub type Files = Arc<Mutex<BTreeMap<String, (String, Vec<u8>)>>>;

        pub async fn start(files: Files, down: Arc<Mutex<bool>>, listed: Arc<Mutex<bool>>) -> (SocketAddr, tokio::sync::watch::Sender<bool>) {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let (stop, rx) = tokio::sync::watch::channel(false);
            let handler = move |req: hyper::Request<hyper::body::Incoming>, _: SocketAddr| {
                let (files, down, listed) = (files.clone(), down.clone(), listed.clone());
                async move {
                    if *down.lock().unwrap() {
                        return net::refusal(StatusCode::SERVICE_UNAVAILABLE, "unavailable", "down");
                    }
                    assert_eq!(req.headers().get("x-fragment-agent").and_then(|v| v.to_str().ok()), Some("juniper--k3x9"), "as the agent");
                    let q = req.uri().query().unwrap_or("").to_string();
                    assert!(q.split('&').any(|kv| kv == "for=npub1paul"), "acting for its owner: {q}");
                    let skills = *listed.lock().unwrap();
                    match (req.method().clone(), req.uri().path()) {
                        (Method::GET, "/api/fragments") => {
                            let mut list = vec![json!({ "name": "garden--k3x9", "role": "editor", "kind": "app", "owned": true })];
                            if skills {
                                list.insert(0, json!({ "name": "skills--z8w6", "role": "editor", "kind": "skills", "owned": true }));
                            }
                            net::json_answer(StatusCode::OK, &json!({ "fragments": list }))
                        }
                        (_, path) if !skills && path.starts_with("/api/f/skills--z8w6/") => net::refusal(StatusCode::NOT_FOUND, "not_found", "no such fragment"),
                        (Method::GET, "/api/f/skills--z8w6/files") => {
                            let list: Vec<Value> = files.lock().unwrap().iter().map(|(p, (v, b))| json!({ "path": p, "size": b.len(), "lastCommitSha": v })).collect();
                            net::json_answer(StatusCode::OK, &json!({ "files": list }))
                        }
                        (Method::GET, "/api/f/skills--z8w6/file") => {
                            let path = q.split('&').find_map(|kv| kv.strip_prefix("path=")).unwrap_or("").replace("%2F", "/");
                            match files.lock().unwrap().get(&path) {
                                Some((_, b)) => net::respond(StatusCode::OK, "text/markdown", b.clone()),
                                None => net::refusal(StatusCode::NOT_FOUND, "not_found", "no such file"),
                            }
                        }
                        (_, path) => net::refusal(StatusCode::NOT_FOUND, "not_found", path),
                    }
                }
            };
            tokio::spawn(net::serve(listener, handler, rx));
            (addr, stop)
        }
    }
}
