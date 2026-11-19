//! An app as a member of another fragment (docs/api.md, "An app as a
//! member"; decision 72): `fragment_core::app_member` holds the rule, and
//! this its two ends. Its owner adds it to a fragment they own, and while it
//! is in anywhere, its audience grows only by every such fragment's yes.
//!
//! - **The target asks the app** (`add_app`): `PUT members/<the app's
//!   name>`, its owner's, or their agent's for them. It records the ask
//!   (`app_asks`, at a new version of its changes), and the app answers in
//!   one turn (`app_asked`): it is their own and members only, and it
//!   records that it is joining, at the ask's version (an invite waiting
//!   asks at its yes, below). The target then, in its own turn, takes the answer only while
//!   its ask is still the newest (no later ask, removal or app's end since),
//!   holds its audience to the rule beside every yes it gave before, keeps
//!   it as the app's approved list (`app_audience`), and writes the app's
//!   row. An answer not taken leaves the app told what it holds there now,
//!   once no ask is left to tell it.
//! - **A narrowing pauses** (`apps_held`, at every change to who is in):
//!   never refused, a removal or a lowered role that leaves an app's list
//!   short pauses it in the same turn (`member_apps.paused`), and a paused
//!   app holds no role here (fragment.rs `standing`) until the list is
//!   held again: the person back, the app taken out, or the person out of
//!   the app: any change to who is in the app asks each fragment it is in
//!   to look at its audience again (`app_looked`), and what that fragment
//!   approved before the look and the app no longer shows comes off it.
//! - **Its audience grows by every yes** (`ask_growth`): anyone whose part
//!   in it would grow (added, invited and asked again at their yes,
//!   raised) is asked of each fragment it is in, which records them on its
//!   list as it says yes (`app_grows`). The app notes the growth first
//!   (`app_growth`), so a look meanwhile carries it, and writes it only once
//!   all said yes; a no tells those that said yes to look again.
//! - **While it is a member anywhere** the app stays members only
//!   (`stays_members_only`), beside reach's refusals (reach.rs).
//! - **The two are told as a person's list is** (members.rs
//!   `tell_index`), each way by its own key: `app:<name>`, a target's word
//!   to an app (its role there, or out: removed, refused, the target
//!   deleted, an ask it recorded included), and `in:<name>`, an app's to a
//!   fragment it is in (gone: its life ended). Each is versioned by the
//!   sender's life and its changes, so a late one never undoes a newer one,
//!   and one for a life that ended changes nothing.
//!
//! Its row in the target names no owner (members.rs `standing` reads one as
//! an agent's): its owner reads nothing through it.

use fragment_core::app_member::{self, Audience};
use fragment_proto::{ErrorCode, FragmentKind, Member, Role};
use futures_util::future::join_all;
use serde_json::json;
use worker::SqlStorageValue;

use crate::error::{CellError, CellResult};
use crate::fragment::calls::{AppAnswer, AppAsked, AppAudience, AppAudienceOf, AppGrows, AppTold};
use crate::fragment::{FragmentCell, MetaKey};
use crate::js;
use crate::members::Actor;
use crate::share::label;

/// The index outbox's key for a word to an app member, and for a word from
/// an app to a fragment it is in: each way its own, so neither is read as
/// the other (members.rs `tell_index`).
pub(crate) const TO_APP: &str = "app:";

/// How long an app's growth note lasts: a growth asks each fragment it is
/// in once, in one request, far sooner; a note older is one a request left
/// as it ended (a restart), and looks no longer carry it.
const GROWTH_NOTED_MS: i64 = 10 * 60 * 1000;
pub(crate) const FROM_APP: &str = "in:";

impl FragmentCell {
    /// Its title, else its label: how an app's lines name it.
    fn app_title(&self) -> CellResult<String> {
        Ok(self.title()?.unwrap_or_else(|| label(&self.name().unwrap_or_default()).to_string()))
    }

    /// The fragment this app is a member of, the first by name, if any.
    pub(crate) fn member_of_any(&self) -> CellResult<Option<String>> {
        let rows = self.rows("SELECT name FROM member_of WHERE role IS NOT NULL ORDER BY name LIMIT 1", vec![])?;
        Ok(rows.first().and_then(|r| r["name"].as_str()).map(str::to_string))
    }

    /// Its link or public, refused while it is a member anywhere.
    pub(crate) fn stays_members_only(&self) -> CellResult<()> {
        match self.member_of_any()? {
            Some(target) => Err(CellError::new(ErrorCode::Forbidden, format!("{} is in {}, so it stays members only.", self.app_title()?, label(&target)))),
            None => Ok(()),
        }
    }

    /// The fragments this app is in, by name.
    pub(crate) fn targets(&self) -> CellResult<Vec<String>> {
        let rows = self.rows("SELECT name FROM member_of WHERE role IS NOT NULL ORDER BY name", vec![])?;
        Ok(rows.iter().filter_map(|r| r["name"].as_str().map(str::to_string)).collect())
    }

    /// `person` would hold `role` in this app: asked of every fragment it is
    /// in (none: nothing to ask), the growth noted first, so a look at its
    /// audience meanwhile carries it. All yes: the note, to drop once the
    /// growth is written (`grown`); else the first no, the note dropped and
    /// those that said yes told to look again. A fragment gone is no one to
    /// ask.
    pub(crate) async fn ask_growth(&self, person: &str, role: Role) -> CellResult<Option<i64>> {
        let targets = self.targets()?;
        if targets.is_empty() || role < Role::Viewer {
            return Ok(None);
        }
        let power = if role >= Role::Editor { Role::Editor } else { Role::Viewer };
        let noted = self.rows(
            "INSERT INTO app_growth (person, power, at) VALUES (?, ?, ?) RETURNING id",
            vec![person.into(), power.as_str().into(), SqlStorageValue::Integer(js::now_ms())],
        )?;
        let id = noted.first().and_then(|r| r["id"].as_i64()).ok_or_else(|| CellError::host("a growth noted has no id"))?;
        // the alarm covers the note, should this request end unanswered
        self.schedule().await?;
        let asked = AppGrows { app: self.must(MetaKey::Npub)?, name: self.name()?, title: self.app_title()?, person: person.to_string(), role };
        // bounded: the fragments an app is in hold it as a member
        let answers = join_all(targets.iter().map(|t| crate::fragment::call(&self.env, t, &asked))).await;
        let refused = answers.iter().find_map(|a| a.as_ref().err().filter(|e| e.code != ErrorCode::NotFound));
        // all yes, and its note still kept: the caller writes the growth in
        // this same turn. A note the alarm took as run out meanwhile had
        // every fragment look again, so their yes may be gone: ask again
        let kept = !self.rows("SELECT 1 AS x FROM app_growth WHERE id = ?", vec![SqlStorageValue::Integer(id)])?.is_empty();
        let refused = match (refused, kept) {
            (None, true) => return Ok(Some(id)),
            (Some(refused), _) => CellError::new(refused.code, refused.message.clone()),
            (None, false) => CellError::new(ErrorCode::Conflict, "asking the fragments it is in took too long: try again"),
        };
        // each looks again, a yes given after the alarm's look too
        self.given_up(Some(id)).await?;
        Err(refused)
    }

    /// When the oldest growth note runs out, if one is noted (for the alarm).
    pub(crate) fn growth_due_at(&self) -> CellResult<Option<i64>> {
        let rows = self.rows("SELECT MIN(at) AS at FROM app_growth", vec![])?;
        Ok(rows.first().and_then(|r| r["at"].as_i64()).map(|at| at + GROWTH_NOTED_MS))
    }

    /// The alarm's part: notes a request left as it ended (a restart) go,
    /// and every fragment this app is in looks again, so a yes they kept
    /// for it comes off.
    pub(crate) async fn growth_spent(&self) -> CellResult<()> {
        let gone = self.rows("DELETE FROM app_growth WHERE at < ? RETURNING id", vec![SqlStorageValue::Integer(js::now_ms() - GROWTH_NOTED_MS)])?;
        if !gone.is_empty() {
            self.apps_look()?;
            self.flush_index().await;
        }
        Ok(())
    }

    /// A growth asked and written: its note goes.
    pub(crate) fn grown(&self, noted: Option<i64>) -> CellResult<()> {
        match noted {
            Some(id) => self.exec("DELETE FROM app_growth WHERE id = ?", vec![SqlStorageValue::Integer(id)]),
            None => Ok(()),
        }
    }

    /// A growth asked and not written (a no, a refusal after the yes, an
    /// invite's ask, which its yes asks again): its note goes, and every
    /// fragment it is in looks again, so no yes stays for someone it never
    /// took.
    pub(crate) async fn given_up(&self, noted: Option<i64>) -> CellResult<()> {
        if noted.is_none() {
            return Ok(());
        }
        self.grown(noted)?;
        self.apps_look()?;
        self.flush_index().await;
        Ok(())
    }

    /// An app member's growth asked here (`AppGrows`): yes records the
    /// person on the list this fragment approved for it, before the app
    /// writes them. One not in yet (its join's answer still on its way) is
    /// held to the strictest role, an editor's.
    pub(crate) async fn app_grows(&self, g: AppGrows) -> CellResult<()> {
        self.created()?;
        let asking = !self.rows("SELECT 1 AS x FROM app_asks WHERE name = ?", vec![g.name.as_str().into()])?.is_empty();
        let app_role = match self.member_role(&g.app)? {
            Some(role) => role,
            None if asking => Role::Editor,
            None => return Err(CellError::new(ErrorCode::NotFound, "that app is no member here")),
        };
        let mut approved = self.app_audience(&g.app)?;
        let holds = self.member_role(&g.person)?;
        if let Err(refused) = app_member::grow(&mut approved, app_role, &g.person, g.role, holds) {
            let person = self.emails(vec![g.person.clone()]).await.pop().map_or_else(|| fragment_core::npub::display(&g.person), |(_, email)| email);
            return Err(CellError::new(ErrorCode::Forbidden, refused.line(&g.title, &self.app_title()?, &person)));
        }
        let power = app_member::power_of(&approved, &g.person).unwrap_or(Role::Viewer);
        self.exec(
            "INSERT INTO app_audience (app, person, power, at) VALUES (?, ?, ?, ?) ON CONFLICT (app, person) DO UPDATE SET power = excluded.power, at = excluded.at",
            vec![g.app.as_str().into(), g.person.as_str().into(), power.as_str().into(), SqlStorageValue::Integer(js::now_ms())],
        )
    }

    /// `PUT members/<name>`, `name` an app of its owner's, at `role`: the
    /// app asked, then held to the rule in this fragment's own turn.
    /// `key`: the app life asked for, when one already is (a role by its
    /// key): another life under the name is no yes.
    pub(crate) async fn add_app(&self, actor: &Actor, name: &str, role: Role, key: Option<&str>) -> CellResult<Member> {
        let me = self.name()?;
        let title = self.app_title()?;
        if name == me {
            return Err(CellError::invalid("a fragment is no member of itself"));
        }
        if !matches!(role, Role::Viewer | Role::Editor) {
            return Err(CellError::invalid(app_member::Refusal::Role(role).line(label(name), &title, "")));
        }
        self.connector_role(role)?;
        let owner = self.must(MetaKey::Owner)?;
        let incarnation = self.incarnation()?;
        let version = self.index_version_next()?;
        self.exec(
            "INSERT INTO app_asks (name, version) VALUES (?, ?) ON CONFLICT (name) DO UPDATE SET version = excluded.version",
            vec![name.into(), SqlStorageValue::Integer(version)],
        )?;
        let asked = AppAsked { target: me, title: title.clone(), incarnation, version, owner: owner.clone(), role, key: key.map(str::to_string) };
        let taken = match crate::fragment::call(&self.env, name, &asked).await {
            Ok(answer) => {
                self.test_hold("app-answer").await;
                // each person by their email, as the owner sees them, should a line name one
                let names = self.emails(answer.audience.iter().map(|(p, _)| p.clone()).collect()).await;
                self.app_answered(actor, name, role, (incarnation, version, &owner, &title), &answer, &names)
            }
            Err(e) => Err(e),
        };
        if taken.is_err() && self.incarnation().ok() == Some(incarnation) {
            // its ask settled, the app told what it holds here now once no
            // later ask is left to tell it
            self.exec("DELETE FROM app_asks WHERE name = ? AND version = ?", vec![name.into(), SqlStorageValue::Integer(version)])?;
            if self.rows("SELECT 1 AS x FROM app_asks WHERE name = ?", vec![name.into()])?.is_empty() {
                let now = match self.app_named(name)? {
                    Some(principal) => self.member_role(&principal)?,
                    None => None,
                };
                self.app_change(name, now)?;
            }
        }
        if taken.is_ok() {
            self.test_hold("app-committed").await;
        }
        self.flush_index().await;
        taken
    }

    /// The app's answer to this fragment's ask, taken in its own turn, or
    /// why not: only while the ask is the newest, the life that asked and
    /// its owner's; held to the rule; then its row, the answer's life the
    /// one row of its name. Answers the row as written.
    fn app_answered(&self, actor: &Actor, name: &str, role: Role, (incarnation, version, owner, title): (i64, i64, &str, &str), answer: &AppAnswer, names: &[(String, String)]) -> CellResult<Member> {
        let newest = self.rows("SELECT 1 AS x FROM app_asks WHERE name = ? AND version = ?", vec![name.into(), SqlStorageValue::Integer(version)])?;
        // an answer from a life whose end was heard meanwhile is no yes
        let ended = self.rows("SELECT 1 AS x FROM app_ended WHERE name = ? AND incarnation >= ?", vec![name.into(), SqlStorageValue::Integer(answer.incarnation)])?;
        if self.incarnation().ok() != Some(incarnation) || self.must(MetaKey::Owner)? != owner || newest.is_empty() || !ended.is_empty() {
            return Err(CellError::new(ErrorCode::Conflict, "it changed while its app was asked: ask again"));
        }
        // its list so far (none at a join) and its answer, held here; bounded:
        // an app's audience is its members and their owners
        let approved = self.app_audience(&answer.npub)?;
        // each one's role here, read in this turn (one that will not read holds none: the safe side)
        let list = match app_member::join(role, &approved, answer.audience.clone(), |p| self.member_role(p).ok().flatten()) {
            Ok(list) => list,
            Err(refused) => {
                let person = match &refused {
                    app_member::Refusal::Audience(short) => names.iter().find(|(id, _)| *id == short.person).map_or_else(|| fragment_core::npub::display(&short.person), |(_, email)| email.clone()),
                    _ => String::new(),
                };
                return Err(CellError::new(ErrorCode::Forbidden, refused.line(&answer.title, title, &person)));
            }
        };
        let current = self.member_role(&answer.npub)?;
        // an older life of the name, whose end was not heard yet, gives up
        // its place to this one
        let older = self.rows("SELECT principal FROM member_apps WHERE name = ? AND principal != ?", vec![name.into(), answer.npub.as_str().into()])?;
        self.room_for(u64::from(current.is_none() && older.is_empty()))?;
        self.exec("DELETE FROM app_asks WHERE name = ?", vec![name.into()])?;
        for old in older.iter().filter_map(|o| o["principal"].as_str()) {
            self.app_out(old)?;
        }
        self.exec(
            "INSERT INTO members (principal, role, added_by, added_at, kind) VALUES (?, ?, ?, ?, 'app')
             ON CONFLICT (principal) DO UPDATE SET role = excluded.role",
            vec![answer.npub.as_str().into(), role.as_str().into(), actor.by.as_str().into(), SqlStorageValue::Integer(js::now_ms())],
        )?;
        self.exec(
            "INSERT INTO member_apps (principal, name, owner, incarnation) VALUES (?, ?, ?, ?) ON CONFLICT (principal) DO NOTHING",
            vec![answer.npub.as_str().into(), name.into(), owner.into(), SqlStorageValue::Integer(answer.incarnation)],
        )?;
        // bounded as its audience is
        for (person, power) in &list {
            self.exec(
                "INSERT INTO app_audience (app, person, power, at) VALUES (?, ?, ?, ?) ON CONFLICT (app, person) DO UPDATE SET power = excluded.power, at = excluded.at",
                vec![answer.npub.as_str().into(), person.as_str().into(), power.as_str().into(), SqlStorageValue::Integer(js::now_ms())],
            )?;
        }
        self.app_change(name, Some(role))?;
        self.sharing_changed()?;
        let data = json!({ "principal": answer.npub, "role": role, "kind": "app", "app": name, "owner": owner });
        self.event("member.set", &format!("the app {name} is now {}{}", role.as_str(), actor.said()), actor.noted(data));
        if current.is_some_and(|was| was != role) {
            self.reopen_sockets(&format!("p:{}", answer.npub), "your role changed");
        }
        let row = self.member_list()?.members.into_iter().find(|m| m.principal == answer.npub);
        row.ok_or_else(|| CellError::host("an app member just written is not there"))
    }

    /// The app member `principal`, by its name, if it is one.
    pub(crate) fn app_named(&self, name: &str) -> CellResult<Option<String>> {
        let rows = self.rows("SELECT principal FROM member_apps WHERE name = ? ORDER BY principal LIMIT 1", vec![name.into()])?;
        Ok(rows.first().and_then(|r| r["principal"].as_str()).map(str::to_string))
    }

    /// The app member `principal`'s name, if it is one.
    pub(crate) fn app_of(&self, principal: &str) -> CellResult<Option<String>> {
        let rows = self.rows("SELECT name FROM member_apps WHERE principal = ?", vec![principal.into()])?;
        Ok(rows.first().and_then(|r| r["name"].as_str()).map(str::to_string))
    }

    /// The app member `principal` (named `name`) out, by `actor`: the app is
    /// told, and stays its owner's no longer than it is anywhere else.
    pub(crate) fn remove_app(&self, actor: &Actor, principal: &str, name: &str) -> CellResult<()> {
        self.app_out(principal)?;
        // an ask on its way is taken no more
        self.exec("DELETE FROM app_asks WHERE name = ?", vec![name.into()])?;
        self.app_change(name, None)?;
        self.event("member.removed", &format!("the app {name} was removed{}", actor.said()), actor.noted(json!({ "principal": principal, "app": name })));
        Ok(())
    }

    /// The app member `principal`'s rows gone, with what it held here.
    fn app_out(&self, principal: &str) -> CellResult<()> {
        self.exec("DELETE FROM members WHERE principal = ?", vec![principal.into()])?;
        self.exec("DELETE FROM member_apps WHERE principal = ?", vec![principal.into()])?;
        self.exec("DELETE FROM app_audience WHERE app = ?", vec![principal.into()])?;
        self.drop_subscriptions(principal)?;
        self.close_sockets(&format!("p:{principal}"), "membership revoked");
        self.sharing_changed()
    }

    /// Asked in by a target (`AppAsked`): this app's answer, its joining
    /// recorded in the same turn, or why not.
    pub(crate) fn app_asked(&self, a: AppAsked) -> CellResult<AppAnswer> {
        self.created()?;
        // asked for another life of this name (a stale sheet's): it records nothing
        let npub = self.must(MetaKey::Npub)?;
        if a.key.as_ref().is_some_and(|k| *k != npub) {
            return Err(CellError::new(ErrorCode::Conflict, "that app is no longer the one asked for: open its sheet again"));
        }
        let (name, owner) = (self.name()?, self.must(MetaKey::Owner)?);
        let refused = |why: String| CellError::new(ErrorCode::Forbidden, why);
        // within one person (decision 63): no one adds another's app
        if a.owner != owner {
            return Err(refused(format!("{} is not an app of yours: only its owner adds it.", label(&name))));
        }
        if a.target == name {
            return Err(CellError::invalid("a fragment is no member of itself"));
        }
        let title = self.app_title()?;
        if self.kind()? == FragmentKind::Agent {
            return Err(refused(format!("{title} is an agent: it joins as itself, with its person.")));
        }
        app_member::members_only(self.visibility()?).map_err(|r| refused(r.line(&title, &a.title, "")))?;
        let audience = self.app_audience_of()?.audience;
        // its joining, as it answers: from now on it stays members only and
        // its owner's alone; an ask older than what the target told it
        // since is no yes
        let written = self.rows(
            "INSERT INTO member_of (name, role, incarnation, version) VALUES (?, ?, ?, ?)
             ON CONFLICT (name) DO UPDATE SET role = excluded.role, incarnation = excluded.incarnation, version = excluded.version
               WHERE excluded.incarnation > member_of.incarnation OR (excluded.incarnation = member_of.incarnation AND excluded.version > member_of.version)
             RETURNING name",
            vec![a.target.as_str().into(), a.role.as_str().into(), SqlStorageValue::Integer(a.incarnation), SqlStorageValue::Integer(a.version)],
        )?;
        if written.is_empty() {
            return Err(CellError::new(ErrorCode::Conflict, "an older ask: it was asked again since"));
        }
        self.event("app.joining", &format!("asked into {} as {}, by its owner", a.target, a.role.as_str()), json!({ "target": a.target, "role": a.role }));
        Ok(AppAnswer { npub, incarnation: self.incarnation()?, title, audience })
    }

    /// A change between an app and a target (`AppTold`). To the app: the
    /// target's word on its role there, taken when newer than the row it
    /// keeps (only a row it answered for). To the target: an app member's
    /// life ended, and with it any ask on its way; or its ask to look at its
    /// audience again.
    pub(crate) async fn app_told(&self, c: AppTold) -> CellResult<()> {
        self.created()?;
        // a role held by another life of this name is none of this one's
        if !c.from_app && c.role.is_some() && c.key != Some(self.must(MetaKey::Npub)?) {
            return Ok(());
        }
        if !c.from_app {
            let role = c.role.map_or(SqlStorageValue::Null, |r| r.as_str().into());
            let (inc, ver) = (SqlStorageValue::Integer(c.incarnation), SqlStorageValue::Integer(c.version));
            // a role changes only a row it answered for; an end (none) is kept
            // even with none, so an ask of that life arriving late is no yes
            let took = match c.role {
                Some(_) => self.rows(
                    "UPDATE member_of SET role = ?, incarnation = ?, version = ?
                     WHERE name = ? AND (? > incarnation OR (? = incarnation AND ? > version)) RETURNING name",
                    vec![role, inc.clone(), ver.clone(), c.fragment.as_str().into(), inc.clone(), inc, ver],
                )?,
                None => self.rows(
                    "INSERT INTO member_of (name, role, incarnation, version) VALUES (?, NULL, ?, ?)
                     ON CONFLICT (name) DO UPDATE SET role = NULL, incarnation = excluded.incarnation, version = excluded.version
                       WHERE excluded.incarnation > member_of.incarnation OR (excluded.incarnation = member_of.incarnation AND excluded.version > member_of.version)
                     RETURNING name",
                    vec![c.fragment.as_str().into(), inc, ver],
                )?,
            };
            match (took.is_empty(), c.role) {
                (true, _) => {}
                (false, None) => self.event("app.out", &format!("no longer in {}", c.fragment), json!({ "target": c.fragment })),
                // its role there taken (a join or a re-ask written): the
                // target is asked to look at its audience again, by this
                // app's outbox, so a change here while it was asked (its
                // word found no row then) is taken there too, a restart
                // between or not
                (false, Some(_)) => self.app_look(&c.fragment)?,
            }
            return Ok(());
        }
        let apps = self.rows(
            "SELECT principal FROM member_apps WHERE name = ? AND incarnation = ?",
            vec![c.fragment.as_str().into(), SqlStorageValue::Integer(c.incarnation)],
        )?;
        // bounded: one life of one app
        let apps: Vec<&str> = apps.iter().filter_map(|a| a["principal"].as_str()).collect();
        match c.role {
            None => {
                // its late answers are no yes; a newer life's ask stands
                self.exec(
                    "INSERT INTO app_ended (name, incarnation) VALUES (?, ?) ON CONFLICT (name) DO UPDATE SET incarnation = MAX(incarnation, excluded.incarnation)",
                    vec![c.fragment.as_str().into(), SqlStorageValue::Integer(c.incarnation)],
                )?;
                for app in &apps {
                    self.app_out(app)?;
                    self.event("member.removed", &format!("the app {} was deleted", c.fragment), json!({ "principal": app, "app": c.fragment }));
                }
                // a newer life of the name, told it held the old one's place
                // here, hears it holds none, unless it is in or asked
                let held = self.rows("SELECT 1 AS x FROM member_apps WHERE name = ? UNION ALL SELECT 1 FROM app_asks WHERE name = ?", vec![c.fragment.as_str().into(), c.fragment.as_str().into()])?;
                if !apps.is_empty() && held.is_empty() {
                    self.app_change(&c.fragment, None)?;
                }
            }
            Some(_) => {
                for app in &apps {
                    self.app_looked(app, &c.fragment).await?;
                }
            }
        }
        Ok(())
    }

    /// The app member `principal` (named `name`) looked at again: what this
    /// fragment approved before the look began and the app no longer shows
    /// comes off its list, and each app is held to its list again.
    async fn app_looked(&self, principal: &str, name: &str) -> CellResult<()> {
        let began = js::now_ms();
        let answer = crate::fragment::call(&self.env, name, &AppAudienceOf {}).await?;
        // another life under the name (this one's end not heard yet) tells nothing of this one
        if answer.npub != principal {
            return Ok(());
        }
        // bounded as its audience is
        for (person, power) in self.app_audience(principal)? {
            let left = app_member::power_of(&answer.audience, &person).map(|now| now.min(power));
            if left == Some(power) {
                continue;
            }
            let key = vec![principal.into(), person.as_str().into(), SqlStorageValue::Integer(began)];
            match left {
                Some(left) => self.exec("UPDATE app_audience SET power = ? WHERE app = ? AND person = ? AND at < ?", [vec![left.as_str().into()], key].concat())?,
                None => self.exec("DELETE FROM app_audience WHERE app = ? AND person = ? AND at < ?", key)?,
            }
        }
        self.apps_held()
    }

    /// Its audience as a fragment it is in holds it (`AppAudienceOf`): its
    /// people, and each growth noted and not yet written, with its own key.
    pub(crate) fn app_audience_of(&self) -> CellResult<AppAudience> {
        self.created()?;
        let mut audience = app_member::audience(&self.must(MetaKey::Owner)?, &self.member_list()?.members);
        // a note a request left when it ended unanswered (a restart) goes,
        // and the alarm then asks every fragment to look again (`growth_spent`)
        let fresh = js::now_ms() - GROWTH_NOTED_MS;
        for g in self.rows("SELECT person, power FROM app_growth WHERE at >= ?", vec![SqlStorageValue::Integer(fresh)])? {
            let (Some(person), Some(power)) = (g["person"].as_str(), g["power"].as_str().and_then(Role::parse)) else { continue };
            app_member::count(&mut audience, person, power);
        }
        Ok(AppAudience { npub: self.must(MetaKey::Npub)?, audience })
    }

    /// Each fragment this app is in asked to look at its audience again, at
    /// a change to who is in it (members.rs `sharing_changed`), by the
    /// index outbox (`FROM_APP`, as `viewer`).
    pub(crate) fn apps_look(&self) -> CellResult<()> {
        let targets = self.rows("SELECT name FROM member_of WHERE role IS NOT NULL ORDER BY name", vec![])?;
        // bounded: the fragments an app is in hold it as a member
        for target in targets.iter().filter_map(|t| t["name"].as_str()) {
            self.app_look(target)?;
        }
        Ok(())
    }

    /// `target` asked to look at this app's audience again, by the index
    /// outbox (`FROM_APP`, as `viewer`).
    fn app_look(&self, target: &str) -> CellResult<()> {
        self.index_rows("SELECT ? AS principal, ? AS role", vec![format!("{FROM_APP}{target}").into(), Role::Viewer.as_str().into()])
    }

    /// Whom `target` joining at `role` grows this app's audience by: the
    /// person it counts as (an agent, its person), when they would do more
    /// to it than they do now (`ask_growth`).
    pub(crate) fn grows(&self, target: &fragment_proto::Identity, role: Role) -> CellResult<Option<String>> {
        let owner = self.must(MetaKey::Owner)?;
        let audience = app_member::audience(&owner, &self.member_list()?.members);
        let person = match (target.kind, &target.owner) {
            (fragment_proto::IdentityKind::Agent, Some(person)) => person.as_str(),
            _ => target.id.as_str(),
        };
        let power = if role >= Role::Editor { Role::Editor } else { Role::Viewer };
        Ok(app_member::power_of(&audience, person).is_none_or(|now| now < power).then(|| person.to_string()))
    }

    /// The app `name` told its role here (none: out), at a new version, by
    /// the index outbox (members.rs `tell_index`), as a person's list is.
    fn app_change(&self, name: &str, role: Option<Role>) -> CellResult<()> {
        let role = role.map_or(SqlStorageValue::Null, |r| r.as_str().into());
        self.index_rows("SELECT ? AS principal, ? AS role", vec![format!("{TO_APP}{name}").into(), role])
    }

    /// The audience this fragment approved for the app member `principal`
    /// (none before it joins).
    fn app_audience(&self, principal: &str) -> CellResult<Audience> {
        let rows = self.rows("SELECT person, power FROM app_audience WHERE app = ?", vec![principal.into()])?;
        // one row a person: the list as kept
        Ok(rows.iter().filter_map(|r| Some((r["person"].as_str()?.to_string(), Role::parse(r["power"].as_str()?)?))).collect())
    }

    /// Each app member held to the list it approved, at a change to who
    /// is in (members.rs `sharing_changed`), in that change's turn: one
    /// left short is paused, naming whom it waits on, and one held again
    /// resumes. Its sockets open again at what it now holds.
    pub(crate) fn apps_held(&self) -> CellResult<()> {
        let apps = self.rows(
            "SELECT a.principal, a.name, a.paused, m.role FROM member_apps a JOIN members m USING (principal) ORDER BY a.principal",
            vec![],
        )?;
        // bounded: its app members are members, at most MEMBERS_MAX
        for app in &apps {
            let (Some(principal), Some(name), Some(role)) = (app["principal"].as_str(), app["name"].as_str(), app["role"].as_str().and_then(Role::parse)) else { continue };
            let short = app_member::short(&self.app_audience(principal)?, role, |p| self.member_role(p).ok().flatten());
            let paused = short.as_ref().map(|s| s.person.as_str());
            if paused == app["paused"].as_str() {
                continue;
            }
            self.exec("UPDATE member_apps SET paused = ? WHERE principal = ?", vec![paused.map_or(SqlStorageValue::Null, Into::into), principal.into()])?;
            match &short {
                Some(s) => self.event("app.paused", &s.paused(name, &fragment_core::npub::display(&s.person)), json!({ "principal": principal, "app": name, "person": s.person })),
                None => self.event("app.resumed", &format!("the app {name} reads here again"), json!({ "principal": principal, "app": name })),
            }
            self.reopen_sockets(&format!("p:{principal}"), "its standing here changed");
        }
        Ok(())
    }

    /// Each of `ids` by their email, as this fragment's owner sees them
    /// (none read: none named).
    async fn emails(&self, ids: Vec<String>) -> Vec<(String, String)> {
        let named = crate::share::profiles(&self.env, ids).await;
        named.into_iter().filter_map(|(id, p)| Some((id, p.email?))).collect()
    }

    /// Its life: its making's time.
    fn incarnation(&self) -> CellResult<i64> {
        self.must(MetaKey::CreatedAt)?.parse().map_err(|_| CellError::host("meta.created_at is no number"))
    }
}
