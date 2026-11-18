//! An app as a member of another fragment (docs/api.md, "An app as a
//! member"; decision 72): `fragment_core::app_member` holds the rule, and
//! this its two ends. For now an app joins only while it is its owner's
//! alone, added by them to a fragment they own, so its audience is its
//! owner, who holds the target.
//!
//! - **The target asks the app** (`add_app`): `PUT members/<the app's
//!   name>`, its owner's, or their agent's for them. The app answers in one
//!   turn (`app_asked`): it is their own, members only, with no one else
//!   in it and no invite waiting, and it records that it is joining, at
//!   the ask's version. The target then holds the answer to the rule in its
//!   own turn, and writes the app's row or tells the app it is out.
//! - **While it is a member anywhere** the app stays members only and
//!   invites no one (`stays_members_only`, `stays_alone`), beside reach's
//!   refusals (reach.rs).
//! - **The two are told as a person's list is** (members.rs
//!   `tell_index`): a target's change goes to the app by its name (its role
//!   there, or out: removed, refused, or the target deleted), and an app
//!   deleted tells each target it is gone (ended.rs). Each is versioned by
//!   the sender's life and its changes, so a late one never undoes a newer
//!   one, and one for a life that ended changes nothing.
//!
//! Its row in the target names no owner (members.rs `standing` reads one as
//! an agent's): its owner reads nothing through it.

use std::collections::BTreeMap;

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

impl FragmentCell {
    /// Its title, else its label: how an app's lines name it.
    fn app_title(&self) -> CellResult<String> {
        Ok(self.title()?.unwrap_or_else(|| label(&self.name().unwrap_or_default()).to_string()))
    }

    /// The fragment this app is a member of, the first by name, if any.
    fn member_of_any(&self) -> CellResult<Option<String>> {
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
        let asked = AppAsked { target: me, title: title.clone(), incarnation, version: self.index_version_next()?, owner: owner.clone(), role };
        let answer = crate::fragment::call(&self.env, name, &asked).await?;
        // this fragment's own turn: still the life that asked, and its owner's
        if self.incarnation().ok() != Some(incarnation) || self.must(MetaKey::Owner)? != owner {
            return Err(CellError::new(ErrorCode::Conflict, "it changed while its app was asked: ask again"));
        }
        // bounded: an app's audience is its members and their owners
        let mut held = BTreeMap::new();
        for (person, _) in &answer.audience {
            held.insert(person.clone(), self.member_role(person)?);
        }
        // its owner's alone: no yes here waits on the app (that list comes
        // with the audience's growing)
        let audience: Audience = answer.audience.iter().cloned().collect();
        if let Err(refused) = app_member::join(role, &Audience::new(), audience, |p| held.get(p).copied().flatten()) {
            // the app recorded its joining: it is told it is out, at a newer version
            self.app_change(name, None)?;
            self.flush_index().await;
            let person = match &refused {
                app_member::Refusal::Audience(short) => fragment_core::npub::display(&short.person),
                _ => String::new(),
            };
            return Err(CellError::new(ErrorCode::Forbidden, refused.line(&answer.title, &title, &person)));
        }
        let current = self.member_role(&answer.npub)?;
        self.room_for(u64::from(current.is_none()))?;
        self.exec(
            "INSERT INTO members (principal, role, added_by, added_at, kind) VALUES (?, ?, ?, ?, 'app')
             ON CONFLICT (principal) DO UPDATE SET role = excluded.role",
            vec![answer.npub.as_str().into(), role.as_str().into(), actor.by.as_str().into(), SqlStorageValue::Integer(js::now_ms())],
        )?;
        self.exec(
            "INSERT INTO member_apps (principal, name, owner, incarnation) VALUES (?, ?, ?, ?) ON CONFLICT (principal) DO NOTHING",
            vec![answer.npub.as_str().into(), name.into(), owner.as_str().into(), SqlStorageValue::Integer(answer.incarnation)],
        )?;
        self.app_change(name, Some(role))?;
        self.sharing_changed()?;
        let data = json!({ "principal": answer.npub, "role": role, "kind": "app", "app": name, "owner": owner });
        self.event("member.set", &format!("the app {name} is now {}{}", role.as_str(), actor.said()), actor.noted(data));
        if current.is_some_and(|was| was != role) {
            self.reopen_sockets(&format!("p:{}", answer.npub), "your role changed");
        }
        self.flush_index().await;
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
        if self.app_named(&a.target)?.is_some() {
            return Err(refused(format!("{} is in {title} already.", a.title)));
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
        Ok(AppAnswer { npub: self.must(MetaKey::Npub)?, incarnation: self.incarnation()?, title, audience: audience.into_iter().collect() })
    }

    /// A change between an app and a target (`AppTold`). As the app: the
    /// target's word on its role there, taken when newer than the row it
    /// keeps (only a row it answered for). As the target: an app member's
    /// life ended.
    pub(crate) fn app_told(&self, AppTold(c): AppTold) -> CellResult<()> {
        self.created()?;
        if !self.rows("SELECT 1 AS x FROM member_of WHERE name = ?", vec![c.fragment.as_str().into()])?.is_empty() {
            let role = c.role.map_or(SqlStorageValue::Null, |r| r.as_str().into());
            let (inc, ver) = (SqlStorageValue::Integer(c.incarnation), SqlStorageValue::Integer(c.version));
            let took = self.rows(
                "UPDATE member_of SET role = ?, incarnation = ?, version = ?
                 WHERE name = ? AND (? > incarnation OR (? = incarnation AND ? > version)) RETURNING name",
                vec![role, inc.clone(), ver.clone(), c.fragment.as_str().into(), inc.clone(), inc, ver],
            )?;
            if !took.is_empty() && c.role.is_none() {
                self.event("app.out", &format!("no longer in {}", c.fragment), json!({ "target": c.fragment }));
            }
            return Ok(());
        }
        if c.role.is_none() {
            let gone = self.rows(
                "SELECT principal FROM member_apps WHERE name = ? AND incarnation = ?",
                vec![c.fragment.as_str().into(), SqlStorageValue::Integer(c.incarnation)],
            )?;
            // bounded: one life of one app
            for g in gone.iter().filter_map(|g| g["principal"].as_str()) {
                self.app_out(g)?;
                self.event("member.removed", &format!("the app {} was deleted", c.fragment), json!({ "principal": g, "app": c.fragment }));
            }
        }
        Ok(())
    }

    /// The app `name` told its role here (none: out), at a new version, by
    /// the index outbox (members.rs `tell_index`), as a person's list is.
    fn app_change(&self, name: &str, role: Option<Role>) -> CellResult<()> {
        let role = role.map_or(SqlStorageValue::Null, |r| r.as_str().into());
        self.index_rows("SELECT ? AS principal, ? AS role", vec![name.into(), role])
    }

    /// Its life: its making's time.
    fn incarnation(&self) -> CellResult<i64> {
        self.must(MetaKey::CreatedAt)?.parse().map_err(|_| CellError::host("meta.created_at is no number"))
    }
}
