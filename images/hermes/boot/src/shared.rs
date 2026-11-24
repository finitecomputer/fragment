//! The team folder (docs/computers.md, "The team folder in our Hermes
//! image"): since a computer per agent, no two of a person's agents share a
//! disk, so each keeps `~/shared` in its work in step with its owner's team
//! folder (their fragment of kind `shared`), through the CLI's own `fragment
//! sync --watch`, one process an agent, as that agent, for its owner. The
//! sync does the rest by its own rules (a change sent within moments, a
//! teammate's arriving through the fragment's change feed, a conflict kept
//! as a copy, a mass delete refused). Hermes sees a folder: no habit to
//! learn (Paul, 2026-11-19).
//!
//! A hold stops every sync (SIGSTOP) before the boot answers it, and lets
//! them go on after (SIGCONT): nothing under `/data` changes while a save
//! is taken (docs/computers.md, "What changes under the hold").

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Child;
use std::time::{Duration, Instant};

use fragment_bridge::api::FragmentEntry;
use fragment_bridge::ev;

/// The folder in an agent's work its team folder is kept in.
pub const DIR: &str = "shared";
/// The platform as each sync reaches it (its `FRAGMENT_API`): the host its
/// journal binds the folder to.
pub const SYNC_API: &str = "http://api.fragment.internal";
/// A sync that ended (its fragment unread, a mass delete it refused) starts
/// again at most this often: never a loop of starts.
pub const RESTART_MS: u64 = 60_000;

/// The owner's team folder among the fragments an agent reaches acting for
/// them: kind `shared`, theirs (one someone else shared is not), one
/// labelled `shared` first (the shell makes it so), else the first by name,
/// as their skills fragment is found (skills.rs, `pick`).
pub fn pick(fragments: &[FragmentEntry]) -> Option<String> {
    let mut theirs: Vec<&str> = fragments.iter().filter(|f| f.kind == "shared" && f.owned).map(|f| f.name.as_str()).collect();
    theirs.sort_unstable();
    let labelled = theirs.iter().find(|n| n.rsplit_once("--").is_some_and(|(label, _)| label == DIR));
    labelled.or(theirs.first()).map(|n| n.to_string())
}

/// A Hermes agent's `~/shared`: in its home, its terminal's and its file
/// tools' `~` (`hermes::home_dir`).
pub fn dir_of(agent_fragment: &str) -> PathBuf {
    crate::hermes::home_dir(agent_fragment).join(DIR)
}

/// The most folders kept beside `~/shared` under one name (`prepare`).
const KEPT_MAX: u32 = 64;
/// The most of a sync journal the boot reads for the fragment it names:
/// larger, it is read as none (the CLI reads it on its own).
const JOURNAL_READ_MAX: u64 = 64 << 20;

/// Readies `dir` for a sync of `fragment`, before its process starts.
/// `previous`, the fragment this boot synced it for until now (a team
/// folder replaced while its sync ran); `repo`, `fragment`'s code repo as
/// its status says it now (its life's; `""` before its first file; none
/// when unread). A folder of another team folder's is kept beside it,
/// unsent edits, journal and all, as `shared-<that fragment>` (`-2`, `-3`…
/// when one has that name), and the chosen one starts in a clean folder:
/// - its journal (the CLI's `.fragment/state.json`) names another fragment;
/// - or names this one, but binds it to another host than the syncs reach
///   (`SYNC_API`), or to a repo other than its life's: an earlier life of
///   the name (deleted and made again), whose journal the CLI refuses
///   (`cli/src/sync/pass.rs`, the world binding);
/// - or it has none yet and `previous` is another.
///
/// The CLI refuses a folder bound elsewhere (file handoff would stop), and
/// one it never bound it would send to the new one. A journal naming it by
/// its bare label (`shared` for `shared--z9z9`) is its own, as the CLI
/// reads it; one the CLI would rebuild (another schema, or unreadable)
/// binds nothing.
pub fn prepare(dir: &std::path::Path, fragment: &str, previous: Option<&str>, repo: Option<&str>) -> std::io::Result<()> {
    let other = |b: &str| b != fragment && fragment.rsplit_once("--").is_none_or(|(label, _)| label != b);
    let elsewhere = |j: &Journal| {
        let host = j.host.as_deref().is_some_and(|h| h.trim_end_matches('/') != SYNC_API);
        let life = j.repo.as_deref().zip(repo).is_some_and(|(was, now)| was != now);
        other(&j.name) || host || life
    };
    let bound = match bound_to(dir) {
        Some(j) => elsewhere(&j).then_some(j.name),
        // none written yet (its first pass never ended), but what it held
        // was another's: kept too, never sent to the new one
        None => previous.filter(|p| other(p) && std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_some())).map(str::to_string),
    };
    if let Some(bound) = bound {
        let parent = dir.parent().ok_or_else(|| std::io::Error::other("~/shared has a parent"))?;
        let base = format!("{DIR}-{}", bound.replace('/', "_"));
        // bounded: a name of its own among at most KEPT_MAX
        let to = (1..=KEPT_MAX)
            .map(|n| parent.join(if n == 1 { base.clone() } else { format!("{base}-{n}") }))
            .find(|p| !p.exists())
            .ok_or_else(|| std::io::Error::other(format!("{KEPT_MAX} folders already kept as {base}")))?;
        std::fs::rename(dir, &to)?;
        ev!("shared.kept", { "from": bound, "to": to.display().to_string(), "now": fragment });
    }
    std::fs::create_dir_all(dir)
}

/// What a sync journal binds its folder to, as the CLI reads it: its
/// fragment, and (from its first pass) the host and repo it synced.
struct Journal {
    name: String,
    host: Option<String>,
    repo: Option<String>,
}

/// What `dir`'s sync journal binds it to, read as the CLI reads it
/// (`cli/src/sync/journal.rs` `read_state`: schema 3, its `name`, `host`,
/// `repo` and `files`; any other it rebuilds, binding nothing). Read
/// without waiting (the agent may have left anything at that path: a FIFO
/// is no journal) and at most `JOURNAL_READ_MAX` of it.
fn bound_to(dir: &std::path::Path) -> Option<Journal> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW).open(dir.join(".fragment").join("state.json")).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(JOURNAL_READ_MAX + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > JOURNAL_READ_MAX {
        return None;
    }
    let journal: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let current = journal["schemaVersion"] == 3 && journal["files"].is_object();
    let name = journal["name"].as_str().filter(|n| current && !n.is_empty())?.to_string();
    let text = |k: &str| journal[k].as_str().map(str::to_string);
    Some(Journal { name, host: text("host"), repo: text("repo") })
}

/// Starts an agent's sync of a team folder (agent, fragment), given the
/// one this boot synced it for until now, if another, and the folder's
/// repo as its status says it (`prepare`).
pub type Spawn<'a> = dyn Fn(&str, &str, Option<&str>, Option<&str>) -> std::io::Result<Child> + 'a;

/// One agent's sync: of which fragment, its process, and when it started.
struct Running {
    fragment: String,
    child: Option<Child>,
    started: Instant,
}

/// The syncs this computer runs, one an agent, and whether a hold stopped
/// them.
#[derive(Default)]
pub struct Syncs {
    running: BTreeMap<String, Running>,
    held: bool,
}

impl Syncs {
    /// Runs `want` (agent → team folder) and nothing else: a sync of an agent
    /// gone, or of another fragment, ends; one missing starts (`spawn`), as
    /// does one that ended, at most once a `RESTART_MS`. Started while held,
    /// it is stopped at once, as the others are.
    pub fn keep(&mut self, want: &BTreeMap<String, String>, repo: Option<&str>, spawn: &Spawn) {
        // the team folder each agent's sync had, where another is wanted
        // now (`prepare` keeps what it held)
        let switched: BTreeMap<String, String> = self.running.iter().filter(|(a, r)| want.get(*a).is_some_and(|f| *f != r.fragment)).map(|(a, r)| (a.clone(), r.fragment.clone())).collect();
        let gone: Vec<String> = self.running.iter().filter(|(a, r)| want.get(*a) != Some(&r.fragment)).map(|(a, _)| a.clone()).collect();
        for agent in gone {
            if let Some(mut r) = self.running.remove(&agent) {
                end(&agent, r.child.as_mut());
            }
        }
        for (agent, fragment) in want {
            let ended = match self.running.get_mut(agent) {
                Some(r) => r.child.as_mut().is_none_or(|c| !matches!(c.try_wait(), Ok(None))) && r.started.elapsed() >= Duration::from_millis(RESTART_MS),
                None => true,
            };
            if !ended {
                continue;
            }
            let child = match spawn(agent, fragment, switched.get(agent).map(String::as_str), repo) {
                Ok(c) => {
                    ev!("shared.started", { "agent": agent, "fragment": fragment, "pid": c.id() });
                    Some(c)
                }
                Err(e) => {
                    ev!("shared.failed", { "agent": agent, "fragment": fragment, "error": e.to_string() });
                    None
                }
            };
            if let (true, Some(c)) = (self.held, &child) {
                signal(c.id(), libc::SIGSTOP);
            }
            self.running.insert(agent.clone(), Running { fragment: fragment.clone(), child, started: Instant::now() });
        }
        assert!(self.running.len() <= want.len(), "one sync an agent, and none of an agent gone");
    }

    /// Whether `keep(want)` would start a sync: one missing, of another
    /// folder, or ended a `RESTART_MS` ago. The boot reads the folder's
    /// repo then alone (`prepare`), not every round.
    pub fn due(&mut self, want: &BTreeMap<String, String>) -> bool {
        want.iter().any(|(agent, fragment)| match self.running.get_mut(agent) {
            Some(r) if r.fragment == *fragment => r.child.as_mut().is_none_or(|c| !matches!(c.try_wait(), Ok(None))) && r.started.elapsed() >= Duration::from_millis(RESTART_MS),
            _ => true,
        })
    }

    /// Whether a hold stopped them.
    pub fn held(&self) -> bool {
        self.held
    }

    /// Stops every sync for a hold (`true`), or lets them go on after it.
    pub fn hold(&mut self, held: bool) {
        if self.held == held {
            return;
        }
        self.held = held;
        let sig = if held { libc::SIGSTOP } else { libc::SIGCONT };
        for r in self.running.values() {
            if let Some(c) = &r.child {
                signal(c.id(), sig);
            }
        }
        ev!("shared.held", { "held": held, "syncs": self.running.len() });
    }
}

fn signal(pid: u32, sig: libc::c_int) {
    let Ok(pid) = libc::pid_t::try_from(pid) else { return };
    // SAFETY: kill(2) on a child of this process, by its pid
    unsafe {
        libc::kill(pid, sig);
    }
}

/// Ends an agent's sync: its folder stays as the sync left it.
fn end(agent: &str, child: Option<&mut Child>) {
    if let Some(c) = child {
        // a stopped process takes no SIGKILL's notice until it runs again
        signal(c.id(), libc::SIGCONT);
        let _ = c.kill();
        let _ = c.wait();
    }
    ev!("shared.ended", { "agent": agent });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, kind: &str, owned: bool) -> FragmentEntry {
        FragmentEntry { name: name.into(), role: "editor".into(), kind: kind.into(), owned, title: String::new(), identity: None, agent_role: None }
    }

    /// Goal: the owner's own team folder is found as their skills are: theirs,
    /// of kind `shared`, labelled `shared` first. Invalid: one shared with
    /// them, another kind, none at all.
    #[test]
    fn the_owners_team_folder_is_found() {
        let list = [entry("notes--k3x9", "app", true), entry("work--p2m4", "shared", true), entry("shared--z9z9", "shared", true), entry("shared--a1a1", "shared", false)];
        assert_eq!(pick(&list).as_deref(), Some("shared--z9z9"), "labelled shared first, theirs");
        assert_eq!(pick(&list[..2]).as_deref(), Some("work--p2m4"), "else the first by name");
        assert_eq!(pick(&[entry("shared--a1a1", "shared", false), entry("shared--b2b2", "skills", true)]), None);
        assert_eq!(dir_of("juniper--k3x9"), crate::hermes::home_dir("juniper--k3x9").join("shared"), "where its `~` is");
        assert!(dir_of("juniper--k3x9").starts_with(crate::hermes::work_dir("juniper--k3x9")), "in its work, saved with it");
    }

    /// The CLI's first pass as it binds `dir` to `fragment`, whose life's
    /// repo is `repo`: refused when its journal names another fragment (its
    /// bare label is the same one's: cli/src/sync/journal.rs `read_state`),
    /// or another host or repo (cli/src/sync/pass.rs, the world binding);
    /// else its journal written, naming it, the syncs' host and its repo.
    fn bind(dir: &std::path::Path, fragment: &str, repo: &str) -> std::io::Result<()> {
        let journal = dir.join(".fragment").join("state.json");
        if let Ok(v) = std::fs::read(&journal).map(|b| serde_json::from_slice::<serde_json::Value>(&b).unwrap_or_default()) {
            let named = v["name"].as_str().unwrap_or_default().to_string();
            let bare = fragment.rsplit_once("--").is_some_and(|(label, _)| label == named);
            if named != fragment && !bare {
                return Err(std::io::Error::other(format!("directory is synced to fragment '{named}', not '{fragment}'")));
            }
            if let (Some(h), Some(r)) = (v["host"].as_str(), v["repo"].as_str()) {
                if h != SYNC_API || r != repo {
                    return Err(std::io::Error::other(format!("this folder is bound to {h} repo {r}, but fragment '{fragment}' is {SYNC_API} repo {repo}")));
                }
            }
        }
        std::fs::create_dir_all(dir.join(".fragment"))?;
        std::fs::write(&journal, serde_json::json!({ "schemaVersion": 3, "name": fragment, "host": SYNC_API, "repo": repo, "files": {} }).to_string())
    }

    /// A fragment's repo, as the tests name each life of one.
    fn repo(fragment: &str, life: u32) -> String {
        format!("e2e--{fragment}--life{life}")
    }

    /// A folder of its own for a test, under the temporary folder.
    fn scratch(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a scratch folder");
        root
    }

    /// Goal (Alex on #1537): the team folder replaced while its sync runs,
    /// the new one's sync starts in a clean `~/shared`, and the old folder,
    /// its unsent edits and its journal, is kept beside it. Invalid: the
    /// same folder (or its journal naming it by its bare label) moves
    /// nothing. Method: `sleep` stands in for the CLI, `bind` for its
    /// first pass.
    #[test]
    fn a_replaced_team_folder_gets_a_clean_folder_and_the_old_is_kept() {
        let root = scratch("shared-switch");
        let dir = root.join(DIR);
        let spawn = |_agent: &str, fragment: &str, previous: Option<&str>, now: Option<&str>| {
            prepare(&dir, fragment, previous, now)?;
            bind(&dir, fragment, now.unwrap_or_default())?;
            std::process::Command::new("sleep").arg("30").spawn()
        };
        let mut syncs = Syncs::default();
        let mut want = BTreeMap::from([("juniper--k3x9".to_string(), "shared--z9z9".to_string())]);
        syncs.keep(&want, None, &spawn);
        assert!(syncs.running["juniper--k3x9"].child.is_some(), "the first started");
        std::fs::write(dir.join("handover.txt"), "unsent").unwrap();
        // the same folder again: nothing moves
        prepare(&dir, "shared--z9z9", None, None).unwrap();
        assert!(dir.join("handover.txt").exists());
        // replaced
        want.insert("juniper--k3x9".into(), "shared--w4w4".into());
        syncs.keep(&want, None, &spawn);
        assert!(syncs.running["juniper--k3x9"].child.is_some(), "the new one's sync started");
        assert!(!dir.join("handover.txt").exists(), "~/shared is the new folder's, clean");
        let kept = root.join("shared-shared--z9z9");
        assert_eq!(std::fs::read_to_string(kept.join("handover.txt")).ok().as_deref(), Some("unsent"), "the old folder kept, its edits in it");
        assert!(std::fs::read_to_string(kept.join(".fragment/state.json")).unwrap().contains("shared--z9z9"), "and its journal");
        syncs.keep(&BTreeMap::new(), None, &spawn);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Goal (Alex on #1537): a boot that finds `~/shared` bound to another
    /// team folder (its journal on disk) starts the chosen one's sync in a
    /// clean folder, the old kept beside it under a name of its own, even
    /// when one from before has that name. Invalid: a journal naming it by
    /// its bare label is the same folder's, and stays.
    #[test]
    fn a_boot_with_the_old_journal_on_disk_starts_clean() {
        let root = scratch("shared-boot");
        let dir = root.join(DIR);
        bind(&dir, "shared--z9z9", "").unwrap();
        std::fs::write(dir.join("draft.md"), "unsent").unwrap();
        std::fs::create_dir_all(root.join("shared-shared--z9z9")).unwrap();
        let spawn = |_agent: &str, fragment: &str, previous: Option<&str>, now: Option<&str>| {
            prepare(&dir, fragment, previous, now)?;
            bind(&dir, fragment, now.unwrap_or_default())?;
            std::process::Command::new("sleep").arg("30").spawn()
        };
        let mut syncs = Syncs::default();
        syncs.keep(&BTreeMap::from([("juniper--k3x9".to_string(), "team--w4w4".to_string())]), None, &spawn);
        assert!(syncs.running["juniper--k3x9"].child.is_some(), "the chosen folder's sync started");
        assert!(!dir.join("draft.md").exists(), "in a clean ~/shared");
        assert_eq!(std::fs::read_to_string(root.join("shared-shared--z9z9-2").join("draft.md")).ok().as_deref(), Some("unsent"), "the old kept, beside one of that name");
        syncs.keep(&BTreeMap::new(), None, &spawn);
        // a journal naming the folder by its bare label is the same one's
        let bare = root.join("bare");
        bind(&bare, "shared", "").unwrap();
        std::fs::write(bare.join("draft.md"), "kept").unwrap();
        prepare(&bare, "shared--z9z9", None, None).unwrap();
        assert!(bare.join("draft.md").exists(), "its own folder, by its label");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Goal (Codex on #1537): a team folder replaced before its sync's
    /// first pass wrote its journal keeps what `~/shared` held too (the
    /// fragment this boot synced it for): never sent to the new one. Method:
    /// the first sync never binds; then the folder is replaced.
    #[test]
    fn a_folder_replaced_before_its_first_journal_is_kept_too() {
        let root = scratch("shared-early");
        let dir = root.join(DIR);
        let spawn = |_agent: &str, fragment: &str, previous: Option<&str>, now: Option<&str>| {
            prepare(&dir, fragment, previous, now)?;
            if fragment == "team--w4w4" {
                bind(&dir, fragment, now.unwrap_or_default())?;
            }
            std::process::Command::new("sleep").arg("30").spawn()
        };
        let mut syncs = Syncs::default();
        let mut want = BTreeMap::from([("juniper--k3x9".to_string(), "shared--z9z9".to_string())]);
        syncs.keep(&want, None, &spawn);
        std::fs::write(dir.join("notes.md"), "unsent").unwrap();
        want.insert("juniper--k3x9".into(), "team--w4w4".into());
        syncs.keep(&want, None, &spawn);
        assert!(syncs.running["juniper--k3x9"].child.is_some(), "the new one's sync started");
        assert!(!dir.join("notes.md").exists(), "nothing of the old one's for the new one to send");
        assert_eq!(std::fs::read_to_string(root.join("shared-shared--z9z9").join("notes.md")).ok().as_deref(), Some("unsent"), "kept beside it");
        syncs.keep(&BTreeMap::new(), None, &spawn);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Goal (Codex on #1537): the journal is read as the CLI reads it, and
    /// never waited on: a FIFO at its path is none (the boot goes on), and
    /// one of another schema, which the CLI rebuilds in place, binds
    /// nothing, so its folder stays.
    #[test]
    fn a_journal_the_cli_would_not_read_binds_nothing() {
        let root = scratch("shared-odd");
        let dir = root.join(DIR);
        std::fs::create_dir_all(dir.join(".fragment")).unwrap();
        let journal = dir.join(".fragment").join("state.json");
        let path = std::ffi::CString::new(journal.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: mkfifo(3) on a path of the test's own folder
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0, "a FIFO where the journal goes");
        std::fs::write(dir.join("draft.md"), "kept").unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let at = dir.clone();
        std::thread::spawn(move || tx.send(prepare(&at, "team--w4w4", None, None).is_ok()).unwrap());
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(true), "prepared at once, the FIFO no journal");
        assert!(dir.join("draft.md").exists(), "and nothing moved");
        std::fs::remove_file(&journal).unwrap();
        std::fs::write(&journal, serde_json::json!({ "schemaVersion": 2, "name": "shared--z9z9", "files": {} }).to_string()).unwrap();
        prepare(&dir, "team--w4w4", None, None).unwrap();
        assert!(dir.join("draft.md").exists(), "another schema: the CLI rebuilds it in place");
        assert!(bound_to(&dir).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Goal (Alex on #1537): the team folder deleted and made again under
    /// its full name while its sync runs (a new life: another repo, which
    /// the CLI's world binding refuses, so that sync ends): the sync started
    /// again finds `~/shared` bound to the earlier life, keeps it beside it,
    /// unsent edits and all, and starts in a clean folder. Invalid: the same
    /// life again moves nothing. Method: `bind` stands in for the CLI's
    /// first pass, with the host and repo it checks.
    #[test]
    fn a_team_folder_made_again_under_its_name_gets_a_clean_folder() {
        let root = scratch("shared-life");
        let dir = root.join(DIR);
        let spawn = |_agent: &str, fragment: &str, previous: Option<&str>, now: Option<&str>| {
            prepare(&dir, fragment, previous, now)?;
            bind(&dir, fragment, now.unwrap_or_default())?;
            std::process::Command::new("sleep").arg("30").spawn()
        };
        let mut syncs = Syncs::default();
        let want = BTreeMap::from([("juniper--k3x9".to_string(), "shared--z9z9".to_string())]);
        let (first, second) = (repo("shared--z9z9", 1), repo("shared--z9z9", 2));
        assert!(syncs.due(&want), "none running: due");
        syncs.keep(&want, Some(&first), &spawn);
        assert!(!syncs.due(&want), "running: not due");
        std::fs::write(dir.join("handover.txt"), "unsent").unwrap();
        // made again: its sync refused by the CLI, and ended
        let r = syncs.running.get_mut("juniper--k3x9").expect("running");
        if let Some(c) = r.child.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
        r.started -= Duration::from_millis(RESTART_MS);
        assert!(syncs.due(&want), "ended a RESTART_MS ago: due");
        syncs.keep(&want, Some(&second), &spawn);
        assert!(syncs.running["juniper--k3x9"].child.is_some(), "the new life's sync started");
        assert!(!dir.join("handover.txt").exists(), "in a clean ~/shared");
        let kept = root.join("shared-shared--z9z9");
        assert_eq!(std::fs::read_to_string(kept.join("handover.txt")).ok().as_deref(), Some("unsent"), "the earlier life's folder kept, its edits in it");
        assert!(std::fs::read_to_string(kept.join(".fragment/state.json")).unwrap().contains(&first), "and its journal, bound to it");
        // the same life again: nothing moves
        std::fs::write(dir.join("notes.md"), "mine").unwrap();
        prepare(&dir, "shared--z9z9", None, Some(&second)).unwrap();
        assert!(dir.join("notes.md").exists());
        syncs.keep(&BTreeMap::new(), None, &spawn);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Goal (Alex on #1537): a boot that finds `~/shared` bound to an
    /// earlier life of its folder's name (its journal's repo another than
    /// the life's, or none yet: its first file makes it), or to another
    /// host than the syncs reach, keeps it beside it and starts clean.
    /// Invalid: the folder's repo unread compares nothing, and moves
    /// nothing.
    #[test]
    fn a_boot_with_an_earlier_lifes_journal_starts_clean() {
        let root = scratch("shared-life-boot");
        let dir = root.join(DIR);
        let spawn = |_agent: &str, fragment: &str, previous: Option<&str>, now: Option<&str>| {
            prepare(&dir, fragment, previous, now)?;
            bind(&dir, fragment, now.unwrap_or_default())?;
            std::process::Command::new("sleep").arg("30").spawn()
        };
        let want = BTreeMap::from([("juniper--k3x9".to_string(), "shared--z9z9".to_string())]);
        // unread: nothing compared, nothing moved (the CLI says why it refuses)
        bind(&dir, "shared--z9z9", &repo("shared--z9z9", 1)).unwrap();
        std::fs::write(dir.join("draft.md"), "unsent").unwrap();
        prepare(&dir, "shared--z9z9", None, None).unwrap();
        assert!(dir.join("draft.md").exists(), "its repo unread: kept where it is");
        // the life made again has no repo until its first file
        let mut syncs = Syncs::default();
        syncs.keep(&want, Some(""), &spawn);
        assert!(syncs.running["juniper--k3x9"].child.is_some(), "the new life's sync started");
        assert!(!dir.join("draft.md").exists(), "in a clean ~/shared");
        assert_eq!(std::fs::read_to_string(root.join("shared-shared--z9z9").join("draft.md")).ok().as_deref(), Some("unsent"), "the earlier life's kept");
        syncs.keep(&BTreeMap::new(), None, &spawn);
        // a journal of another host's
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::create_dir_all(dir.join(".fragment")).unwrap();
        let life = repo("shared--z9z9", 3);
        std::fs::write(dir.join(".fragment/state.json"), serde_json::json!({ "schemaVersion": 3, "name": "shared--z9z9", "host": "https://elsewhere.test", "repo": life, "files": {} }).to_string()).unwrap();
        std::fs::write(dir.join("other.md"), "unsent").unwrap();
        prepare(&dir, "shared--z9z9", None, Some(&life)).unwrap();
        assert!(!dir.join("other.md").exists() && root.join("shared-shared--z9z9-2").join("other.md").exists(), "another host's: kept beside it, under a name of its own");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Goal: one sync an agent, of the folder wanted; an agent gone, or a
    /// folder changed, ends its sync; one that ended starts again no sooner
    /// than `RESTART_MS`; a hold stops each, one started while held too.
    /// Method: `sleep` stands in for the CLI.
    #[test]
    fn one_sync_an_agent_and_a_hold_stops_them() {
        let spawned = std::cell::RefCell::new(vec![]);
        let spawn = |agent: &str, fragment: &str, _previous: Option<&str>, _repo: Option<&str>| {
            spawned.borrow_mut().push((agent.to_string(), fragment.to_string()));
            std::process::Command::new("sleep").arg("30").spawn()
        };
        let mut syncs = Syncs::default();
        let mut want = BTreeMap::from([("juniper--k3x9".to_string(), "shared--z9z9".to_string()), ("maple--p2m4".to_string(), "shared--z9z9".to_string())]);
        syncs.keep(&want, None, &spawn);
        syncs.keep(&want, None, &spawn);
        assert_eq!(spawned.borrow().len(), 2, "one each, kept");
        let state = |pid: u32| std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default().split_whitespace().nth(2).unwrap_or("").to_string();
        let pids: Vec<u32> = syncs.running.values().filter_map(|r| r.child.as_ref().map(Child::id)).collect();
        syncs.hold(true);
        std::thread::sleep(Duration::from_millis(100));
        assert!(pids.iter().all(|p| state(*p) == "T"), "stopped under the hold");
        // an agent made meanwhile starts stopped
        want.insert("rowan--q3q3".into(), "shared--z9z9".into());
        syncs.keep(&want, None, &spawn);
        let rowan = syncs.running["rowan--q3q3"].child.as_ref().map(Child::id).expect("started");
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(state(rowan), "T", "started under a hold, stopped at once");
        syncs.hold(false);
        std::thread::sleep(Duration::from_millis(100));
        assert!(pids.iter().chain([&rowan]).all(|p| state(*p) == "S"), "going on after it");
        // maple leaves; juniper's folder changes
        want.remove("maple--p2m4");
        want.insert("juniper--k3x9".into(), "team--w4w4".into());
        syncs.keep(&want, None, &spawn);
        assert_eq!(syncs.running.len(), 2);
        assert_eq!(syncs.running["juniper--k3x9"].fragment, "team--w4w4");
        assert_eq!(spawned.borrow().last(), Some(&("juniper--k3x9".to_string(), "team--w4w4".to_string())));
        // one that ended waits RESTART_MS before it starts again
        let before = spawned.borrow().len();
        if let Some(c) = syncs.running.get_mut("rowan--q3q3").and_then(|r| r.child.as_mut()) {
            let _ = c.kill();
            let _ = c.wait();
        }
        syncs.keep(&want, None, &spawn);
        assert_eq!(spawned.borrow().len(), before, "not again at once");
        syncs.running.get_mut("rowan--q3q3").expect("kept").started -= Duration::from_millis(RESTART_MS);
        syncs.keep(&want, None, &spawn);
        assert_eq!(spawned.borrow().len(), before + 1, "again after RESTART_MS");
        syncs.keep(&BTreeMap::new(), None, &spawn);
        assert!(syncs.running.is_empty());
    }
}
