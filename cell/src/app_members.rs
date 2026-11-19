//! An app as a member of another fragment (docs/api.md, "An app as a
//! member"; decision 72): `fragment_core::app_member` holds the rule, and
//! this its two ends. For now an app joins only while it is its owner's
//! alone, added by them to a fragment they own, so its audience is its
//! owner, who holds the target.
//!
//! - **The target asks the app** (`add_app`): `PUT members/<the app's
//!   name>`, its owner's, or their agent's for them. It records the ask
//!   (`app_asks`, at a new version of its changes), and the app answers in
//!   one turn (`app_asked`): it is their own, members only, with no one
//!   else in it and no invite waiting, and it records that it is joining,
//!   at the ask's version. The target then, in its own turn, takes the
//!   answer only while its ask is still the newest (no later ask, removal
//!   or app's end since), holds it to the rule, and writes the app's row.
//!   An answer not taken leaves the app told what it holds there now, once
//!   no ask is left to tell it.
//! - **While it is a member anywhere** the app stays members only and
//!   invites no one (`stays_members_only`, `stays_alone`), beside reach's
//!   refusals (reach.rs).
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
use serde_json::json;
use worker::SqlStorageValue;

use crate::error::{CellError, CellResult};
use crate::fragment::calls::{AppAnswer, AppAsked, AppTold};
use crate::fragment::{FragmentCell, MetaKey};
use crate::js;
use crate::members::Actor;
use crate::share::label;

/// The index outbox's key for a word to an app member, and for a word from
/// an app to a fragment it is in: each way its own, so neither is read as
/// the other (members.rs `tell_index`).
pub(crate) const TO_APP: &str = "app:";
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

    /// Anyone else let in (an invite), refused while it is a member
    /// anywhere: for now an app in another is its owner's alone.
    pub(crate) fn stays_alone(&self) -> CellResult<()> {
        match self.member_of_any()? {
            Some(target) => Err(CellError::new(ErrorCode::Forbidden, format!("{} is in {}, so for now it's yours alone.", self.app_title()?, label(&target)))),
            None => Ok(()),
        }
    }

    /// `PUT members/<name>`, `name` an app of its owner's, at `role`: the
    /// app asked, then held to the rule in this fragment's own turn.
    pub(crate) async fn add_app(&self, actor: &Actor, name: &str, role: Role) -> CellResult<Member> {
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
        let asked = AppAsked { target: me, title: title.clone(), incarnation, version, owner: owner.clone(), role };
        let taken = match crate::fragment::call(&self.env, name, &asked).await {
            Ok(answer) => self.app_answered(actor, name, role, (incarnation, version, &owner, &title), &answer),
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
        self.flush_index().await;
        taken
    }

    /// The app's answer to this fragment's ask, taken in its own turn, or
    /// why not: only while the ask is the newest, the life that asked and
    /// its owner's; held to the rule; then its row, the answer's life the
    /// one row of its name. Answers the row as written.
    fn app_answered(&self, actor: &Actor, name: &str, role: Role, (incarnation, version, owner, title): (i64, i64, &str, &str), answer: &AppAnswer) -> CellResult<Member> {
        let newest = self.rows("SELECT 1 AS x FROM app_asks WHERE name = ? AND version = ?", vec![name.into(), SqlStorageValue::Integer(version)])?;
        // an answer from a life whose end was heard meanwhile is no yes
        let ended = self.rows("SELECT 1 AS x FROM app_ended WHERE name = ? AND incarnation >= ?", vec![name.into(), SqlStorageValue::Integer(answer.incarnation)])?;
        if self.incarnation().ok() != Some(incarnation) || self.must(MetaKey::Owner)? != owner || newest.is_empty() || !ended.is_empty() {
            return Err(CellError::new(ErrorCode::Conflict, "it changed while its app was asked: ask again"));
        }
        // its owner's alone: no yes here waits on the app (that list comes
        // with the audience's growing); bounded, its members and their owners
        if let Err(refused) = app_member::join(role, &Audience::new(), answer.audience.clone(), |p| self.member_role(p).ok().flatten()) {
            let person = match &refused {
                app_member::Refusal::Audience(short) => fragment_core::npub::display(&short.person),
                _ => String::new(),
            };
            return Err(CellError::new(ErrorCode::Forbidden, refused.line(&answer.title, title, &person)));
        }
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
        self.drop_subscriptions(principal)?;
        self.close_sockets(&format!("p:{principal}"), "membership revoked");
        self.sharing_changed()
    }

    /// Asked in by a target (`AppAsked`): this app's answer, its joining
    /// recorded in the same turn, or why not.
    pub(crate) fn app_asked(&self, a: AppAsked) -> CellResult<AppAnswer> {
        self.created()?;
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
        let audience = app_member::audience(&owner, &self.member_list()?.members);
        let invited = self.count_of("SELECT COUNT(*) AS n FROM invites WHERE expires_at > ?", vec![SqlStorageValue::Integer(js::now_ms())])?;
        if app_member::others(&owner, &audience).is_some() || invited > 0 {
            return Err(refused(format!("{title} is shared. For now an app joins only while it's yours alone.")));
        }
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
        Ok(AppAnswer { npub: self.must(MetaKey::Npub)?, incarnation: self.incarnation()?, title, audience })
    }

    /// A change between an app and a target (`AppTold`). To the app: the
    /// target's word on its role there, taken when newer than the row it
    /// keeps (only a row it answered for). To the target: an app member's
    /// life ended, and with it any ask on its way.
    pub(crate) fn app_told(&self, c: AppTold) -> CellResult<()> {
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
            if !took.is_empty() && c.role.is_none() {
                self.event("app.out", &format!("no longer in {}", c.fragment), json!({ "target": c.fragment }));
            }
            return Ok(());
        }
        if c.role.is_none() {
            // its late answers are no yes; a newer life's ask stands
            self.exec(
                "INSERT INTO app_ended (name, incarnation) VALUES (?, ?) ON CONFLICT (name) DO UPDATE SET incarnation = MAX(incarnation, excluded.incarnation)",
                vec![c.fragment.as_str().into(), SqlStorageValue::Integer(c.incarnation)],
            )?;
            let gone = self.rows(
                "SELECT principal FROM member_apps WHERE name = ? AND incarnation = ?",
                vec![c.fragment.as_str().into(), SqlStorageValue::Integer(c.incarnation)],
            )?;
            // bounded: one life of one app
            for g in gone.iter().filter_map(|g| g["principal"].as_str()) {
                self.app_out(g)?;
                self.event("member.removed", &format!("the app {} was deleted", c.fragment), json!({ "principal": g, "app": c.fragment }));
            }
            // a newer life of the name, told it held the old one's place
            // here, hears it holds none, unless it is in or asked
            let held = self.rows("SELECT 1 AS x FROM member_apps WHERE name = ? UNION ALL SELECT 1 FROM app_asks WHERE name = ?", vec![c.fragment.as_str().into(), c.fragment.as_str().into()])?;
            if !gone.is_empty() && held.is_empty() {
                self.app_change(&c.fragment, None)?;
            }
        }
        Ok(())
    }

    /// The app `name` told its role here (none: out), at a new version, by
    /// the index outbox (members.rs `tell_index`), as a person's list is.
    fn app_change(&self, name: &str, role: Option<Role>) -> CellResult<()> {
        let role = role.map_or(SqlStorageValue::Null, |r| r.as_str().into());
        self.index_rows("SELECT ? AS principal, ? AS role", vec![format!("{TO_APP}{name}").into(), role])
    }

    /// Its life: its making's time.
    fn incarnation(&self) -> CellResult<i64> {
        self.must(MetaKey::CreatedAt)?.parse().map_err(|_| CellError::host("meta.created_at is no number"))
    }
}
