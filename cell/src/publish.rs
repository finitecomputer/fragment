//! A fragment's files from the platform (docs/api.md, Control API): a
//! template as a new fragment's first commit and first deploy,
//! `POST /api/files` (one commit to `main`, as a CLI sync makes), and
//! `POST /api/deploy` (`live` to `main`'s tip, as `fragment deploy` does).
//! The shell uses the same two routes (an agent's settings, a rename).

use std::collections::BTreeMap;

use fragment_core::seed::{self, Effect, Event, Stage, Step};
use fragment_proto::{ErrorCode, RepoPath, Role};
use fragment_templates::{blessed, Template, CATALOG};
use serde_json::{json, Value};
use worker::*;

use crate::error::{CellError, CellResult};
use crate::files::{content_of, FileWrite, Wrote};
use crate::fragment::{json_response, Caller, FragmentCell, MetaKey};
use crate::js;

/// `live` moving under a deploy this many times is an error.
const DEPLOY_ATTEMPTS: usize = 5;

/// The blessed templates whose fragment is answered before its repo is
/// made (`seed::Making::SeedsLater`): the create answers once its claim and
/// its facts are written, the fragment runs its template from the release
/// meanwhile (`install_pending`), and its repo and first commit come from
/// its alarm, armed at once by the create, else at a later pass. Only the
/// chat for now (Paul, 2026-11-04: what a person waits on): each other
/// template has readers of its repo of its own (a computer syncing its
/// agent, a brain's sync), to be measured and tested before it joins.
const SEEDS_LATER: [&str; 1] = ["chat"];

/// Whether a create of `template` answers before its repo is made
/// (`SEEDS_LATER`): a blessed template's, whose content the release serves.
pub(crate) fn seeds_later(template: &str) -> bool {
    let later = SEEDS_LATER.contains(&template);
    assert!(!later || blessed::template(template).is_some(), "only a blessed template runs from the release before its repo: {template}");
    later
}

/// The name of the repo of `fragment`, made by `owner`: derived here
/// alone, from both, so a name another identity makes later never finds
/// it.
pub(crate) fn repo_name(cfg: &crate::config::Config, fragment: &str, owner: &str) -> CellResult<String> {
    fragment_core::codestorage::repo_name(&cfg.codestorage().repo_prefix, fragment, owner)
        .ok_or_else(|| CellError::invalid("a fragment's name is <label>--<suffix>, and its owner an identity"))
}

/// A blessed template's `fragment.json` as its seed commits it: the
/// template named, and its title. What runs before the commit lands
/// (`install_pending`) is read from these same bytes.
pub(crate) fn blessed_manifest_bytes(which: &str, title: Option<&str>) -> Vec<u8> {
    let mut manifest = json!({ "template": which });
    if let Some(title) = title {
        manifest["meta"] = json!({ "title": title });
    }
    serde_json::to_vec_pretty(&manifest).expect("a manifest serializes")
}

/// A template a fragment can start from (`fragment_templates::CATALOG`):
/// its files are copied into the fragment's repo.
pub(crate) fn template(name: &str) -> Option<&'static Template> {
    CATALOG.into_iter().find(|t| t.name == name)
}

/// The template's fragment.json with the fragment's own name in it, and
/// the title its maker gave it, if any, over the template's.
fn stamp(bytes: &[u8], name: &str, title: Option<&str>) -> Vec<u8> {
    match serde_json::from_slice::<Value>(bytes) {
        Ok(Value::Object(mut o)) => {
            o.insert("name".into(), name.into());
            if let Some(title) = title {
                match o.get_mut("meta") {
                    Some(Value::Object(meta)) => {
                        meta.insert("title".into(), title.into());
                    }
                    _ => {
                        o.insert("meta".into(), json!({ "title": title }));
                    }
                }
            }
            serde_json::to_vec_pretty(&Value::Object(o)).expect("a manifest serializes")
        }
        _ => bytes.to_vec(),
    }
}

/// A template's files as the fragment `name` holds them, read from the
/// release: its fragment.json stamped with that name and title.
async fn stamped(env: &Env, t: &'static Template, name: &str, title: Option<&str>) -> CellResult<Vec<FileWrite>> {
    let files: Vec<&'static fragment_templates::File> = t.files.iter().collect();
    let read = crate::assets::read_all(env, &files).await?;
    let mut writes = Vec::with_capacity(files.len());
    for (f, bytes) in files.iter().zip(read) {
        let path = RepoPath::parse(f.path).map_err(|not| CellError::host(format!("the {} template: {not}", t.name)))?;
        writes.push(FileWrite { path, bytes: Some(if f.path == "fragment.json" { stamp(&bytes, name, title) } else { bytes }) });
    }
    Ok(writes)
}

impl FragmentCell {
    /// The rows the stage of its making is read from (`seed::Rows`): its
    /// template's commit is the one keyed by its life, asked only while a
    /// template is to land.
    fn seed_rows(&self) -> CellResult<seed::Rows> {
        let keys = [MetaKey::ClaimedAt, MetaKey::CreatedAt, MetaKey::TemplatePending, MetaKey::Repo, MetaKey::PinLive, MetaKey::LiveReadAt, MetaKey::Blessed];
        let [claimed_at, created_at, pending, repo, live, live_read_at, blessed] = self.metas(keys)?;
        let created_at = created_at.map(|at| at.parse::<i64>().map_err(|_| CellError::host(format!("its created_at is not a time: {at:?}")))).transpose()?;
        let committed = match (created_at, &pending) {
            (Some(life), Some(_)) => !self.rows("SELECT 1 FROM file_commits WHERE key = ?", vec![format!("template:{life}").into()])?.is_empty(),
            _ => false,
        };
        // a claim that is not a time holds nothing, as a lapsed one
        let claimed_at = claimed_at.and_then(|at| at.parse().ok());
        Ok(seed::Rows { now_ms: js::now_ms(), claimed_at, created_at, template_pending: pending.is_some(), repo: repo.is_some(), committed, live, live_read_at, blessed: blessed.is_some() })
    }

    /// How far its making has got (`fragment_core::seed`), read from its
    /// rows: the one answer to whether its template is still to land.
    pub(crate) fn stage(&self) -> CellResult<Stage> {
        seed::stage(&self.seed_rows()?).map_err(contradiction)
    }

    /// [`Self::stage`] for a seed that began in `life` (`Stage::Ended` once
    /// that life is not the fragment's).
    fn stage_in(&self, life: i64) -> CellResult<Stage> {
        seed::stage_in(&self.seed_rows()?, life).map_err(contradiction)
    }

    /// Lands the template a fragment was made from (`template_pending`), a
    /// step of `fragment_core::seed` at a time from the stage its rows
    /// say: its repo, its commit (keyed by its life, so a retry commits
    /// nothing twice), its deploy and install, and its landing. A pass
    /// after a failure or a restart takes the step that is next.
    ///
    /// One at a time: the create seeds, and the alarm it arms meanwhile
    /// (its index flush) seeds too. On code.storage, a round trip away, the
    /// two met: the second commit, changing nothing, was refused (412), and
    /// when that was the create's, it answered before its template was
    /// live (a skills fragment listing none of the release). Under the
    /// lock the second finds the template landed, and does nothing.
    ///
    /// Landed is installed: the agents that joined meanwhile are told then
    /// (`Stage::joins_wait`), by whichever seed lands it, the create's or a
    /// later one.
    pub(crate) async fn seed(&self) -> CellResult<()> {
        for effect in self.seed_locked().await? {
            assert_eq!(effect, Effect::TellJoined, "only its agents' notice follows a seed's lock");
            self.flush_joined().await;
        }
        Ok(())
    }

    /// [`Self::seed`] under its lock: each step carried out, its outcome
    /// stepped, and the rows read back, which must say the step the
    /// machine said (a life that ended meanwhile goes no further). Answers
    /// what is left for after the lock.
    async fn seed_locked(&self) -> CellResult<Vec<Effect>> {
        let _seeding = self.seeding.lock().await;
        let Some(life) = self.seed_rows()?.created_at else { return Ok(vec![]) };
        let mut stage = self.stage_in(life)?;
        if !stage.waits_template() {
            return Ok(vec![]);
        }
        let (_, mut effects) = seed::step(&stage, &Event::Alarm).map_err(refused)?;
        let (name, owner) = (self.name()?, self.must(MetaKey::Owner)?);
        let mut after = vec![];
        let mut steps = 0;
        while !effects.is_empty() {
            let effect = effects.remove(0);
            if !effect.is_seed_step() {
                after.push(effect);
                continue;
            }
            steps += 1;
            assert!(steps <= seed::SEED_STEPS_MAX, "a seed takes each of its steps once: {effect:?} is step {steps}");
            let events = self.seed_step(effect, life, &name, &owner).await?;
            let mut want = stage.clone();
            for event in &events {
                let (next, more) = seed::step(&want, event).map_err(refused)?;
                want = next;
                effects.extend(more);
            }
            // the rows say where it is: its life may have ended under an
            // await (its next life's pending template is that life's own
            // seed's), and the machine's step must be theirs
            let read = self.stage_in(life)?;
            if read == Stage::Ended {
                return Err(CellError::new(ErrorCode::NotFound, format!("{name} ended during its template's {effect:?}")));
            }
            // deployed is not installed: an install its deploy left to the
            // poll keeps it pending, and the next seed's deploy, from the
            // same commit, installs it (its agents' joins wait meanwhile)
            if let Stage::Pending { step: Step::Deployed { live }, .. } = &want {
                return Err(CellError::host(format!("{name}'s template is deployed (live {live}) but not installed yet")));
            }
            if !read.same_step(&want) {
                return Err(CellError::host(format!("{name}'s template: after {effect:?} its rows say {read:?}, not {want:?}")));
            }
            stage = read;
        }
        assert_eq!(stage, Stage::Landed, "a seed whose every step passed lands its template");
        Ok(after)
    }

    /// Carries out one seed step in `life` (`seed::Effect`): what happened.
    async fn seed_step(&self, effect: Effect, life: i64, name: &str, owner: &str) -> CellResult<Vec<Event>> {
        match effect {
            // a template that seeds after the create's answer (`seeds_later`)
            // makes its repo here, once: a later try finds it by its name
            Effect::EnsureRepo => {
                let repo = self.cs()?.ensure_repo(&repo_name(self.cfg, name, owner)?).await?;
                // its life ended while code.storage made the repo: the repo
                // stays, as a deleted life's does (the same owner making the
                // name again finds it), and this life's seed goes no further
                self.same_life(life, "while its repo was made")?;
                self.set_meta(MetaKey::Repo, &repo)?;
                self.event("template.repo", &format!("{name}'s repo is made ({repo})"), json!({ "repo": repo }));
                Ok(vec![Event::RepoMade])
            }
            Effect::Commit => {
                let which = self.must(MetaKey::TemplatePending)?;
                let title = self.meta(MetaKey::TemplateTitle)?;
                let files = match (template(&which), blessed::template(&which)) {
                    (Some(t), _) => stamped(&self.env, t, name, title.as_deref()).await?,
                    // a blessed template is named, not copied: the release serves it
                    (None, Some(_)) => {
                        let bytes = blessed_manifest_bytes(&which, title.as_deref());
                        vec![FileWrite { path: RepoPath::parse("fragment.json").expect("fragment.json is a repo path"), bytes: Some(bytes) }]
                    }
                    (None, None) => return Err(CellError::host(format!("no template {which}"))),
                };
                self.same_life(life, "while its template's files were read")?;
                let repo = self.must(MetaKey::Repo)?;
                let key = format!("template:{life}");
                if let Wrote::Conflict(why) = self.commit_in(&repo, &key, &files, &BTreeMap::new(), &format!("start from the {which} template"), owner, 0).await? {
                    return Err(CellError::host(why));
                }
                Ok(vec![Event::Committed])
            }
            Effect::Deploy => {
                let repo = self.must(MetaKey::Repo)?;
                let live = self.go_live_in(&repo, owner, &format!("deploy {name}")).await?;
                // what follows live installed from it with the deploy, or
                // was left to the poll
                let installed = self.meta(MetaKey::LiveReadAt)?.as_deref() == Some(live.as_str());
                let mut events = vec![Event::Deployed { live: live.clone() }];
                if installed {
                    events.push(Event::Installed { live });
                }
                Ok(events)
            }
            Effect::Land => {
                let which = self.must(MetaKey::TemplatePending)?;
                self.event("template", &format!("{name} starts from the {which} template"), json!({ "template": which }));
                self.del_meta(MetaKey::TemplateTitle)?;
                self.del_meta(MetaKey::TemplatePending)?;
                Ok(vec![Event::Landed])
            }
            Effect::InstallFromRelease | Effect::FlushIndex | Effect::TellJoined | Effect::Arm { .. } => unreachable!("{effect:?} is not a seed step"),
        }
    }

    /// The life a seed began in, `life` (its `CreatedAt`), is still the
    /// fragment's, or the seed goes no further (404): a delete, and the
    /// name made again, between its awaits leaves the next life's pending
    /// template, title and repo to that life's own seed. Asked after an
    /// await and before what the step writes or commits next.
    fn same_life(&self, life: i64, when: &str) -> CellResult<()> {
        if self.meta(MetaKey::CreatedAt)?.and_then(|at| at.parse::<i64>().ok()) == Some(life) {
            return Ok(());
        }
        Err(CellError::new(ErrorCode::NotFound, format!("{} ended {when}", self.name().unwrap_or_default())))
    }

    /// Moves `live` to `main`'s tip: the first deploy makes the branch,
    /// later ones fast-forward it (after a rollback, restore commits and a
    /// merge: `Cs::promote_live`), each guarded against a `live` that moved
    /// meanwhile. Answers the new tip.
    pub(crate) async fn go_live(&self, principal: &str, message: &str) -> CellResult<String> {
        // (a deploy before its template's first commit lands that first: `repo_ready`)
        let repo = self.repo_ready().await?;
        self.go_live_in(&repo, principal, message).await
    }

    /// [`Self::go_live`] in `repo`, made: the template's seed's own.
    async fn go_live_in(&self, repo: &str, principal: &str, message: &str) -> CellResult<String> {
        let repo = repo.to_string();
        let cs = self.cs()?;
        let main = cs.branch_head(&repo, "main").await?.ok_or_else(|| CellError::invalid("nothing to deploy: main has no commits"))?;
        let (author, email) = crate::files::author(principal);
        for _ in 0..DEPLOY_ATTEMPTS {
            let moved = match cs.branch_head(&repo, "live").await? {
                None => cs.create_branch(&repo, &main, "live").await?,
                Some(tip) if tip == main => Some(tip),
                Some(_) => {
                    // Held across the steps: a refresh or a poll between them
                    // waits, then pins the second, so the files live
                    // holds between them are never served.
                    let _held = self.plane.lock().await;
                    cs.promote_live(&repo, message, (&author, &email)).await?
                }
            };
            if let Some(tip) = moved {
                // the pin follows now; failing that, the poll backstop
                if let Err(e) = self.interpret(&["live"]).await {
                    self.event("deploy.refresh-failed", &e.message, json!({ "live": tip }));
                    self.may_lag().await?;
                }
                return Ok(tip);
            }
        }
        Err(CellError::new(ErrorCode::UpstreamFailed, format!("live kept moving under {DEPLOY_ATTEMPTS} deploys; try again")))
    }

    /// `POST /api/files {files: [{path, text | base64 | delete: true}],
    /// message?, key?}`: one commit to `main`, for editors (at most 16
    /// files and 1 MiB). A `key` makes a retry commit nothing twice.
    pub(crate) async fn write_files_api(&self, caller: &Caller, body: Value) -> CellResult<Response> {
        self.require(caller, false, Role::Editor)?;
        self.writable().await?;
        let who = self.caller_id(caller)?.to_string();
        let files = body["files"].as_array().filter(|f| !f.is_empty()).ok_or_else(|| CellError::invalid("files: a list of {path, text | base64 | delete}"))?;
        let mut writes = Vec::with_capacity(files.len());
        for f in files {
            let given = f["path"].as_str().ok_or_else(|| CellError::invalid("each file has a path"))?;
            let path = RepoPath::parse(given).map_err(|not| CellError::invalid(not.to_string()))?;
            let bytes = match f["delete"].as_bool() {
                Some(true) => None,
                _ => Some(content_of(f).map_err(|e| CellError::invalid(format!("{path}: {e}")))?),
            };
            writes.push(FileWrite { path, bytes });
        }
        if writes.len() > fragment_proto::limits::FILE_WRITES_MAX {
            return Err(CellError::invalid(format!("at most {} files per write", fragment_proto::limits::FILE_WRITES_MAX)));
        }
        let total: usize = writes.iter().filter_map(|w| w.bytes.as_ref().map(Vec::len)).sum();
        if total > fragment_proto::limits::API_FILES_WRITE_MAX_BYTES {
            return Err(CellError::too_large("the files written", total, fragment_proto::limits::API_FILES_WRITE_MAX_BYTES));
        }
        // an agent's picture is refused here, before it is committed, as its
        // install refuses one (docs/api.md, "An agent's name and picture")
        if let Some(bytes) = writes.iter().find(|w| w.path == fragment_core::face::AGENT_PICTURE).and_then(|w| w.bytes.as_deref()) {
            if self.kind()? == fragment_proto::FragmentKind::Agent {
                fragment_core::face::check_picture(bytes).map_err(|r| crate::picture_refused(r, fragment_core::face::AGENT_PICTURE))?;
            }
        }
        let message = body["message"].as_str().map_or_else(|| format!("write {} file(s)", writes.len()), str::to_string);
        let key = format!("api:{who}:{}", body["key"].as_str().map_or_else(js::random_hex::<16>, str::to_string));
        match self.commit(&key, &writes, &BTreeMap::new(), &message, &who, 0).await? {
            Wrote::Commit(sha) => json_response(&json!({ "commit": sha })),
            // nothing was expected, so nothing can conflict
            Wrote::Conflict(why) => Err(CellError::host(why)),
        }
    }

    /// `POST /api/deploy {note?}`: `live` to `main`'s tip, for editors.
    pub(crate) async fn deploy_api(&self, caller: &Caller, body: Value) -> CellResult<Response> {
        self.require(caller, false, Role::Editor)?;
        self.writable().await?;

        let who = self.caller_id(caller)?.to_string();
        let name = self.name()?;
        let message = format!("deploy {name}{}", body["note"].as_str().map(|n| format!(": {n}")).unwrap_or_default());
        let live = self.go_live(&who, &message).await?;
        json_response(&json!({ "live": live, "canonical": self.cfg.canonical(&caller.url, &name) }))
    }
}

/// Rows no making leaves: the fragment fails closed, writing nothing.
fn contradiction(c: seed::Contradiction) -> CellError {
    CellError::host(format!("its making's rows contradict each other: {c}"))
}

/// The machine refused a step the cell took: the cell's mistake.
pub(crate) fn refused(r: seed::Refused) -> CellError {
    CellError::host(format!("its making's machine refused a step: {r:?}"))
}
