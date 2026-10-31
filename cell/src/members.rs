//! Membership is live cell state (docs/MODEL.md): members, invites,
//! visibility, and the share link's token live in the supervisor, change in
//! one transaction, and take effect on the next request. Only the owner
//! changes them, or their own agent sharing for them (Paul, 2026-10-04:
//! `access::agent_shares`, decided here from the owner's share whatever
//! the router let through); a member may leave. Every change names who
//! made it (`Actor`): an agent's names the agent and the owner it acted
//! for, in the events, and the agent in members' `added_by` and invites'
//! `created_by`, so its owner sees what it shared.
//!
//! Members are identities (npubs); a request may name one by a key, which
//! the registry resolves to the identity holding it. An agent member's
//! owner is recorded beside it: the owner reads what the agent reads
//! (fragment.rs, `standing`). Kinds and owners never change once
//! registered, so the copy here cannot go stale. A new agent member's
//! computer is told it joined, through an outbox of its own (runs_on.rs):
//! nothing that adds an agent need post `joined` itself.
//!
//! Each identity's list of fragments is an index in its `Principal` cell.
//! The fragment is the authority: a change is written here with an outbox
//! row in the same turn, then delivered (and retried from the alarm). The
//! owner's row also carries the fragment's sharing (`Sharing`: who may
//! open it, its members and guests, and the invites waiting), made when
//! the row is sent: a change to members, invites or visibility sends it
//! again (`sharing_changed`), so the
//! platform's page reads the owner's list alone. Every row carries the
//! fragment's face (its kind and title) and its agents, the first added
//! first: an install that changes the face, or an agent joining or
//! leaving, sends every row again (`reindex`). A person's search cursor
//! (search.rs) follows their row: made with a role, gone without one.
//!
//! An invite waits on an email (decisions 48 and 63): the registry keeps
//! it against the email and mails them, and whoever signs in as it
//! accepts it, which makes them a member here (`claim_invite`), or
//! declines it (`decline_invite`): an invite makes no one a member without
//! their yes (`PUT members` adds an identity at once: the API's, by its
//! key, an agent's above all). A forwarded mail admits no one else: only
//! someone signed in with the verified email answers it. There are no
//! bearer invites: an unguessable link is `link` visibility.

use fragment_core::access;
use fragment_core::ended::Failure;
use fragment_core::npub;
use fragment_proto::{
    limits, CreateInvite, ErrorCode, FragmentKind, Identity, IdentityKind, Invite, InviteList, Member, MemberList, PutSecret, Role, Rotated, SecretInfo,
    SecretList, SetRole, SetVisibility, Shared, Sharing, Visibility,
};
use futures_util::future::join_all;
use serde_json::{json, Value};
use worker::*;

use crate::error::{CellError, CellResult};
use crate::fragment::{json_response, Caller, FragmentCell, MetaKey};
use crate::js;
use crate::registry::calls::{ClaimInvite, InviteEmail};
use crate::registry::invites::ForgetInvite;

/// Index changes one flush sends at most, all at once (and a delete, of
/// its ended life's: ended.rs). Sent one at a time, a fragment of
/// `MEMBERS_MAX` members took 300 s to tell their lists it was deleted on
/// the e2e preview (2026-10-06): about 0.3 s a list, each a Principal made
/// on the spot.
pub(crate) const INDEX_FLUSH_MAX: i64 = 32;

/// Marks the registry's `invites/claim` and `invites/decline`, a person's
/// yes or no to the invite waiting on their email, and the platform's
/// `members/agent` (`agent_made`); the router never sets it.
pub(crate) const CLAIM_HEADER: &str = "x-fragment-invite-claim";

/// `agent`, just made by `person` (registered: lib.rs `identities`; or
/// taken by their computer: computer.rs, once both its agent fragment and
/// the computer hold it, so its computer hears each join), comes in
/// beside them wherever they are a member of
/// someone else's fragment (decision 64: their membership is their
/// agents'), each fragment asked once, `INDEX_FLUSH_MAX` at a time. Best
/// effort: one not told keeps it out until its person's next yes there,
/// said in the log.
pub(crate) async fn agent_made(env: &Env, person: &str, agent: &str) {
    let theirs: Vec<String> = match crate::listed(env, person).await {
        Ok(list) => list.fragments.into_iter().filter(|f| matches!(f.role, Role::Viewer | Role::Editor)).map(|f| f.name).collect(),
        Err(e) => return console_error!("{}", json!({ "agent-made": agent, "list": person, "failed": e.message })),
    };
    let body = json!({ "person": person, "agent": agent });
    for round in theirs.chunks(INDEX_FLUSH_MAX as usize) {
        let told = join_all(round.iter().map(|f| crate::fragment::ask(env, f, "members/agent", &body))).await;
        for (f, told) in round.iter().zip(told) {
            if let Err(e) = told {
                console_error!("{}", json!({ "agent-made": agent, "fragment": f, "failed": e.message, "code": e.code }));
            }
        }
    }
}

/// Why a chat's own agent (`chat_agent`) keeps its place: the chat is its
/// conversation, and its replies and steps post there as an editor.
const CHAT_AGENT_STAYS: &str = "it is this chat's own agent, whose replies and steps make the chat: it is not removed (archive or delete the chat instead)";
const CHAT_AGENT_STAYS_EDITOR: &str = "it is this chat's own agent, whose replies and steps post there as an editor: it stays one (archive or delete the chat instead)";

/// An invite to someone in it already is refused: their role changes with
/// `PUT members` (the share sheet's list, `fragment members role`).
fn already_in(email: &str, role: Role) -> String {
    match role {
        Role::Owner => format!("{email} owns it"),
        _ => format!("{email} is in it already, as {}: a role changes with PUT members/{email} (`fragment members role`), not an invite", if role == Role::Editor { "an editor" } else { "a viewer" }),
    }
}

fn refusal(actor_is_owner: bool, why: &str) -> CellError {
    if actor_is_owner {
        CellError::invalid(why)
    } else {
        CellError::new(ErrorCode::Forbidden, why)
    }
}

const MEMBER_COLUMNS: &str = "principal, role, added_by, added_at, kind, owner";

fn member_json(r: &Value) -> CellResult<Member> {
    let s = |k: &str| r[k].as_str().map(str::to_string).ok_or_else(|| CellError::host(format!("members.{k}")));
    Ok(Member {
        principal: npub::display(&s("principal")?),
        role: Role::parse(&s("role")?).ok_or_else(|| CellError::host("members.role"))?,
        added_by: npub::display(&s("added_by")?),
        added_at: r["added_at"].as_i64().unwrap_or(0),
        kind: r["kind"].as_str().and_then(IdentityKind::parse),
        owner: r["owner"].as_str().map(str::to_string),
    })
}

fn opt(v: Option<&str>) -> SqlStorageValue {
    v.map_or(SqlStorageValue::Null, |s| s.into())
}

/// Who changes a fragment's sharing in one request, and with what role.
struct Actor {
    /// Whether they administer who is in and how it is shared: a person's
    /// fragment's owner, or their agent sharing for them; an org's, one of
    /// its content admins, as themselves (decisions 69 to 71). Never a role.
    admin: bool,
    /// Who made the change: members' `added_by`, invites' `created_by`,
    /// the events' `by`.
    by: String,
    /// The owner an agent shared for: the events' `for`.
    for_owner: Option<String>,
}

impl Actor {
    /// The words a change's summary ends with: an agent's names it and the
    /// owner it acted for; a person's, nothing (the owner made it).
    fn said(&self) -> String {
        match &self.for_owner {
            Some(owner) => format!(", by {} (an agent, for {owner})", self.by),
            None => String::new(),
        }
    }

    /// A change's event data, naming who made it (`by`), and the owner an
    /// agent made it for (`for`).
    fn noted(&self, mut data: Value) -> Value {
        assert!(data.is_object(), "an event's data is an object");
        data["by"] = json!(self.by);
        if let Some(owner) = &self.for_owner {
            data["for"] = json!(owner);
        }
        data
    }
}

fn invite_json(r: &Value) -> Invite {
    Invite {
        email: r["email"].as_str().unwrap_or("").to_string(),
        role: r["role"].as_str().and_then(Role::parse).unwrap_or(Role::Viewer),
        expires_at: r["expires_at"].as_i64().unwrap_or(0),
        created_by: npub::display(r["created_by"].as_str().unwrap_or("")),
    }
}

impl FragmentCell {
    /// Records an index change for `principal` (`None` removes them). Runs
    /// in the caller's turn, beside the membership write it mirrors. Their
    /// search cursor follows it (search.rs).
    pub(crate) fn index_change(&self, principal: &str, role: Option<Role>) -> CellResult<()> {
        let version: i64 = self.meta(MetaKey::IndexVersion)?.and_then(|v| v.parse().ok()).unwrap_or(0) + 1;
        self.set_meta(MetaKey::IndexVersion, &version.to_string())?;
        let stored = role.map_or(SqlStorageValue::Null, |r| r.as_str().into());
        self.exec(
            "INSERT INTO index_outbox (principal, role, version, attempts, next_at) VALUES (?, ?, ?, 0, ?)
             ON CONFLICT (principal) DO UPDATE SET role = excluded.role, version = excluded.version, attempts = 0, next_at = excluded.next_at",
            vec![principal.into(), stored, SqlStorageValue::Integer(version), SqlStorageValue::Integer(js::now_ms())],
        )?;
        self.search_follows(principal, role)
    }

    /// What its members' lists show of it, at each install of live: its
    /// kind and title, an agent's role and picture (and its agents, read as
    /// each row is sent). A change is sent to every member's list (at most
    /// `MEMBERS_MAX`) and its owner's, as an agent's joining or leaving is.
    pub(crate) fn face_is(&self, kind: FragmentKind, title: Option<&str>, role: Option<&str>, picture: Option<&str>) -> CellResult<()> {
        let face = face(kind, title, role, picture);
        if self.meta(MetaKey::Face)?.as_deref() == Some(face.as_str()) {
            return Ok(());
        }
        self.set_meta(MetaKey::Face, &face)?;
        self.reindex()
    }

    /// Every list's row is sent again: its owner's and each member's.
    pub(crate) fn reindex(&self) -> CellResult<()> {
        self.reindex_readers(Role::Public)
    }

    /// The rows of everyone whose role is at least `read` are sent again:
    /// its owner's, and each such member's (a listed record of a channel
    /// they read moved: listed.rs). The others' rows carry nothing of it.
    pub(crate) fn reindex_readers(&self, read: Role) -> CellResult<()> {
        let owner = self.owner()?.name();
        self.index_change(&owner, Some(Role::Owner))?;
        // bounded by MEMBERS_MAX
        for r in self.rows("SELECT principal, role FROM members", vec![])? {
            let (Some(p), Some(role)) = (r["principal"].as_str(), r["role"].as_str().and_then(Role::parse)) else { continue };
            if p != owner && role >= read {
                self.index_change(p, Some(role))?;
            }
        }
        Ok(())
    }

    /// Who is in, or who may open it, changed: the owner's row in their
    /// list is sent again, with the sharing as it is when it goes.
    pub(crate) fn sharing_changed(&self) -> CellResult<()> {
        self.index_change(&self.owner()?.name(), Some(Role::Owner))
    }

    /// Sends one index change to `principal`'s list: whether it took it.
    pub(crate) async fn send_index(&self, principal: &str, body: &Value) -> bool {
        self.tell_index(principal, body).await.is_ok()
    }

    /// Sends one index change to `principal`'s list, and tells apart why
    /// it was not taken (`fragment_core::ended::list_told`): the list's
    /// refusal, or a failure a later try may pass.
    pub(crate) async fn tell_index(&self, principal: &str, body: &Value) -> Result<(), Failure> {
        let sent = async {
            let headers = Headers::new();
            headers.set("content-type", "application/json")?;
            let mut init = RequestInit::new();
            init.with_method(Method::Post).with_headers(headers).with_body(Some(body.to_string().into()));
            let req = Request::new_with_init("https://principal.internal/index", &init)?;
            let mut resp = crate::routed::send(&self.env, "PRINCIPAL", principal, req).await?;
            let status = resp.status_code();
            // what a list says of a change it did not take (its error's message)
            let detail = if status == 200 { String::new() } else { resp.text().await.unwrap_or_default() };
            Ok::<(u16, String), CellError>((status, detail))
        }
        .await;
        match sent {
            Ok((status, detail)) => fragment_core::ended::list_told(Some(status), &detail),
            // a list of another release refused it unread (a deploy going out): a later try's
            Err(e) => fragment_core::ended::list_told(None, &e.message),
        }
    }

    /// Delivers due index changes to the people's `Principal` cells, at
    /// most `INDEX_FLUSH_MAX` of them, all at once, the newest first: a
    /// request that flushes sends its own change and waits one round,
    /// whatever its members; the alarm sends the rest, a batch a pass. A
    /// failure stays in the outbox with a backoff; the alarm retries it.
    pub(crate) async fn flush_index(&self) {
        let (Ok(name), Ok(Some(incarnation)), Ok(owner)) = (self.must(MetaKey::Name), self.meta(MetaKey::CreatedAt), self.owner().map(|o| o.name())) else { return };
        if let Err(e) = self.search_fence() {
            console_error!("{name}: its search was not fenced to the channels searched now ({:?}): {}", e.code, e.message);
        }
        let searched = match self.searched_channels() {
            Ok(searched) => searched,
            Err(e) => return console_error!("{name}: its searched channels did not read ({:?}): {}", e.code, e.message),
        };
        let agents = match self.listed_agents() {
            Ok(agents) => agents,
            Err(e) => return console_error!("{name}: its agents did not read ({:?}): {}", e.code, e.message),
        };
        if let Err(e) = self.listed_fence() {
            console_error!("{name}: its listed records were not fenced to what its channels list now ({:?}): {}", e.code, e.message);
        }
        let listed = match self.listed_records() {
            Ok(listed) => listed,
            Err(e) => return console_error!("{name}: its listed records did not read ({:?}): {}", e.code, e.message),
        };
        let due = self
            .rows(
                "SELECT principal, role, version, attempts FROM index_outbox WHERE next_at <= ? ORDER BY version DESC LIMIT ?",
                vec![SqlStorageValue::Integer(js::now_ms()), SqlStorageValue::Integer(INDEX_FLUSH_MAX)],
            )
            .unwrap_or_default();
        assert!(due.len() as i64 <= INDEX_FLUSH_MAX, "a flush sends a bounded batch");
        // an org's: a member offboarded since is sent no more of it, its
        // row taken from their list instead (Alex's review); not knowing
        // who is, it sends nothing now, and the alarm tries again
        let named: Vec<String> = due.iter().filter(|r| r["role"].is_string()).filter_map(|r| r["principal"].as_str().map(str::to_string)).collect();
        let stale = match self.stale_members(&named).await {
            Ok(stale) => stale,
            Err(e) => {
                console_error!("{name}: who is in its org now was not read ({:?}): {}", e.code, e.message);
                let _ = self.schedule().await;
                return;
            }
        };
        let mut batch = Vec::with_capacity(due.len());
        for row in &due {
            let principal = row["principal"].as_str().unwrap_or("").to_string();
            let version = row["version"].as_i64().unwrap_or(0);
            let role = if stale.contains(&principal) { Value::Null } else { row["role"].clone() };
            let mut body = json!({
                "fragment": name,
                "role": role,
                "incarnation": incarnation.parse::<i64>().unwrap_or(0),
                "version": version,
            });
            // every row a role names: the channels searched, the
            // fragment's face and its agents, and the listed records of the
            // channels its role reads, as they are now
            if let Some(role) = body["role"].as_str().and_then(Role::parse) {
                body["searched"] = json!(searched);
                body["listed"] = json!(fragment_core::listed::for_role(&listed, role));
                if let Ok(Some(face)) = self.meta(MetaKey::Face) {
                    body["face"] = serde_json::from_str(&face).unwrap_or(Value::Null);
                    body["face"]["agents"] = json!(agents);
                }
            }
            // the owner's row: the sharing now, which no later change undoes
            // (a later one sends a newer version, made after it)
            if principal == owner && body["role"].is_string() {
                match self.sharing_counts() {
                    Ok(sharing) => body["sharing"] = json!(sharing),
                    Err(e) => console_error!("{name}: its sharing did not read ({:?}): {}", e.code, e.message),
                }
            }
            batch.push((principal, version, row["attempts"].as_i64().unwrap_or(0), body));
        }
        let delivered = join_all(batch.iter().map(|(principal, _, _, body)| self.send_index(principal, body))).await;
        for ((principal, version, attempts, _), delivered) in batch.into_iter().zip(delivered) {
            if delivered {
                let _ = self.exec(
                    "DELETE FROM index_outbox WHERE principal = ? AND version = ?",
                    vec![principal.as_str().into(), SqlStorageValue::Integer(version)],
                );
            } else {
                let attempts = attempts + 1;
                let _ = self.exec(
                    "UPDATE index_outbox SET attempts = ?, next_at = ? WHERE principal = ? AND version = ?",
                    vec![
                        SqlStorageValue::Integer(attempts),
                        SqlStorageValue::Integer(js::now_ms() + fragment_core::backoff::outbox_retry_ms(attempts)),
                        principal.as_str().into(),
                        SqlStorageValue::Integer(version),
                    ],
                );
            }
        }
        // what the batch left, or did not deliver, is the alarm's; and a
        // cursor made with a role is due at once: the alarm sends it
        // (search.rs), after the row it follows is delivered here
        let left = self.rows("SELECT 1 FROM index_outbox LIMIT 1", vec![]).is_ok_and(|r| !r.is_empty());
        if left || self.search_due_at().ok().flatten().is_some_and(|at| at <= js::now_ms()) {
            if let Err(e) = self.schedule().await {
                console_error!("{name}: the alarm was not armed for its index or search outbox ({:?}): {}", e.code, e.message);
            }
        }
    }

    /// Who shares in this request (Paul, 2026-10-04): a person, with their
    /// own membership; an agent, with its owner's role when
    /// `access::agent_shares` lends it, and refused with why (403) when it
    /// does not, and on a connector always (`agents_share`). An agent's own
    /// membership never shares (it is at most `AGENT_ROLE_MAX`). The router refused what it could tell from who
    /// the agent is; this decides again, so a path it read otherwise
    /// gains nothing.
    fn sharer(&self, caller: &Caller) -> CellResult<Actor> {
        self.name()?;
        let Some(signed) = caller.signed.as_ref() else {
            return Err(CellError::new(ErrorCode::Unauthenticated, "sign the request"));
        };
        let owner = self.owner()?;
        match (signed.kind, owner.org()) {
            // an org's: its content admins, each as themselves, at the place
            // the registry answered for this request; never an agent
            (IdentityKind::Person, Some(org)) => {
                assert!(signed.acting_for.is_none(), "a person acts as themselves (the router refuses `for` from one)");
                let place = signed.org_place(org).filter(|p| p.admin);
                Ok(Actor { admin: place.is_some(), by: signed.id.clone(), for_owner: None })
            }
            (IdentityKind::Agent, Some(_)) => Err(CellError::new(ErrorCode::Forbidden, "an org's fragment is shared by its content admins, each as themselves: no agent shares it")),
            (IdentityKind::Person, None) => {
                assert!(signed.acting_for.is_none(), "a person acts as themselves (the router refuses `for` from one)");
                Ok(Actor { admin: self.member_role(&signed.id)? == Some(Role::Owner), by: signed.id.clone(), for_owner: None })
            }
            (IdentityKind::Agent, None) => {
                self.agents_share()?;
                let owner = signed.owner.as_deref();
                let for_owner = owner.is_some() && signed.acting_for.as_deref() == owner;
                let sharer = access::Sharer { for_owner, held: signed.held };
                let share = match owner {
                    Some(owner) => self.member_role(owner)?,
                    None => None,
                };
                match access::agent_shares(sharer, share) {
                    Ok(role) => {
                        assert_eq!(role, Role::Owner, "an agent shares with its owner's ownership");
                        let owner = owner.expect("an agent that shares acts for its owner").to_string();
                        Ok(Actor { admin: true, by: signed.id.clone(), for_owner: Some(owner) })
                    }
                    Err(refusal) => Err(CellError::new(ErrorCode::Forbidden, refusal.message())),
                }
            }
        }
    }

    /// Who administers it (`Actor::admin`): who they are, or 403.
    fn require_owner(&self, caller: &Caller) -> CellResult<Actor> {
        let actor = self.sharer(caller)?;
        if !actor.admin {
            return Err(CellError::new(ErrorCode::Forbidden, "only the owner does this (an org's fragment: its content admins)"));
        }
        Ok(actor)
    }

    /// Whether `caller` shares it: who administers it. Only they hold its
    /// share link (a member asks them).
    pub(crate) fn shares(&self, caller: &Caller) -> bool {
        self.sharer(caller).is_ok_and(|actor| actor.admin)
    }

    /// What it is, as its live `fragment.json` last said (an app until it
    /// has; `Other` for a kind a newer release stored, read after a
    /// rollback).
    pub(crate) fn kind(&self) -> CellResult<FragmentKind> {
        let face: Option<Value> = self.meta(MetaKey::Face)?.and_then(|f| serde_json::from_str(&f).ok());
        Ok(face.and_then(|f| serde_json::from_value(f["kind"].clone()).ok()).unwrap_or_default())
    }

    /// Whether its content files read from main first (`__file`, `__files`):
    /// an agent's and a session archive's, whose writers write main and
    /// never deploy.
    pub(crate) fn main_first(&self) -> CellResult<bool> {
        Ok(matches!(self.kind()?, FragmentKind::Agent | FragmentKind::Sessions))
    }

    /// A chat's own agent: its lead, the first agent added, which answers
    /// when no one is named (docs/chat-records.md) and posts its turns and
    /// steps to `work`, which takes editors only. None for anything else.
    fn chat_agent(&self) -> CellResult<Option<String>> {
        if self.kind()? != FragmentKind::Chat {
            return Ok(None);
        }
        Ok(self.listed_agents()?.into_iter().next())
    }

    /// The identity `who` (an npub or 64 hex: an identity, or a key of
    /// one; or an email: the person who signs in as it) names.
    async fn named(&self, who: &str) -> CellResult<Identity> {
        if npub::parse(who).is_none() && !fragment_core::mail::valid_address(who) {
            return Err(CellError::invalid(format!("{who:?} is not an email, an npub, or a 64-hex key")));
        }
        crate::ask_registry(&self.env, &crate::registry::calls::Lookup { who: who.to_string() }).await
    }

    /// Names for identities, as this fragment's page and code show them
    /// (`__people`, `job.people`): the registry's profiles (a person's
    /// email only for those `emails_of` names), each agent by its agent
    /// fragment's face (share.rs `name_agents`), and each picture an
    /// address on the platform's origin, which a fragment's page loads from
    /// its own.
    pub(crate) async fn people(&self, ids: Vec<String>, emails_of: Vec<String>) -> CellResult<crate::registry::calls::ProfilesAnswer> {
        let mut answer = crate::ask_registry(&self.env, &crate::registry::calls::Profiles { ids, emails_of }).await?;
        crate::share::name_agents(&self.env, &mut answer.profiles).await;
        let platform = self.cfg.platform();
        // bounded: the registry answers at most 64 profiles
        for p in answer.profiles.values_mut() {
            p.picture = p.picture.take().map(|path| format!("{platform}{path}"));
        }
        Ok(answer)
    }

    /// Which of `ids` are members here: those whose emails a member may
    /// see (docs/cloudflare-v1.md, decision 48). Bounded by the caller's
    /// list (a profile ask is at most `PROFILES_MAX`).
    pub(crate) fn members_among(&self, ids: &[String]) -> CellResult<Vec<String>> {
        assert!(ids.len() <= crate::registry::PROFILES_MAX * 2, "a profile ask is bounded before it reaches here");
        let mut out = vec![];
        for id in ids {
            if self.member_role(id)?.is_some() {
                out.push(id.clone());
            }
        }
        Ok(out)
    }

    pub(crate) fn members(&self, caller: &Caller) -> CellResult<Response> {
        // an org's content admins administer who is in, member or not
        let admin = self.owner()?.org().is_some_and(|org| self.content_admin(caller, org).is_ok());
        if !admin {
            self.require(caller, false, Role::Viewer)?;
        }
        json_response(&self.member_list()?)
    }

    /// Every member, the first added first (a chat's lead is its first
    /// agent): the API's list, and a page's (`__members`, serve.rs).
    pub(crate) fn member_list(&self) -> CellResult<MemberList> {
        let rows = self.rows(&format!("SELECT {MEMBER_COLUMNS} FROM members ORDER BY added_at, principal"), vec![])?;
        Ok(MemberList { members: rows.iter().map(member_json).collect::<CellResult<Vec<_>>>()? })
    }

    /// Its agent members as every list's row names them: the first added
    /// first, at most `LISTED_AGENTS_MAX`.
    fn listed_agents(&self) -> CellResult<Vec<String>> {
        let rows = self.rows(
            "SELECT principal FROM members WHERE kind = 'agent' ORDER BY added_at, principal LIMIT ?",
            vec![SqlStorageValue::Integer(limits::LISTED_AGENTS_MAX as i64)],
        )?;
        Ok(rows.iter().filter_map(|r| r["principal"].as_str().map(str::to_string)).collect())
    }

    /// `adding` new members (a person, and the agents in with them) need
    /// room under `MEMBERS_MAX`; a role change needs none.
    fn room_for(&self, adding: u64) -> CellResult<()> {
        if adding > 0 && self.count("SELECT COUNT(*) AS n FROM members")? + adding > limits::MEMBERS_MAX as u64 {
            return Err(CellError::invalid(format!("a fragment has at most {} members (a person's agents count, in with them)", limits::MEMBERS_MAX)));
        }
        Ok(())
    }

    /// A test hook (`ops::test_fragment`): placeholder members until there are
    /// `fill`, so the e2e reaches the member cap without a thousand sign-ins.
    pub(crate) fn fill_members(&self, fill: u64) -> CellResult<u64> {
        if fill > limits::MEMBERS_MAX as u64 {
            return Err(CellError::invalid(format!("fill is at most {}", limits::MEMBERS_MAX)));
        }
        let have = self.count("SELECT COUNT(*) AS n FROM members")?;
        // Bounded by MEMBERS_MAX, just checked.
        for _ in have..fill {
            self.exec(
                "INSERT INTO members (principal, role, added_by, added_at, kind) VALUES (?, 'viewer', 'test', ?, 'person')",
                vec![format!("id:e2e-filler-{}", js::random_hex::<8>()).into(), SqlStorageValue::Integer(js::now_ms())],
            )?;
        }
        let now = self.count("SELECT COUNT(*) AS n FROM members")?;
        assert!(now >= fill && now <= limits::MEMBERS_MAX as u64, "filled to the count asked, within the cap");
        Ok(now)
    }

    /// `gone`'s memberships taken away, each as a removal takes it: its
    /// row, its listed records, its subscriptions, its computer's place,
    /// its list's row and its sockets (an agent's owner's too, who has no
    /// standing of their own left). Bounded: one person and their agents.
    fn revoke(&self, gone: &[String]) -> CellResult<()> {
        assert!(gone.len() as u64 <= 1 + limits::AGENTS_PER_OWNER_MAX, "one person and their agents");
        let mut agents_left = false;
        for who in gone {
            let removed = self.rows("DELETE FROM members WHERE principal = ? RETURNING owner, kind", vec![who.as_str().into()])?;
            agents_left |= removed.first().is_some_and(|r| r["kind"] == "agent");
            self.listed_left(who)?;
            self.drop_subscriptions(who)?;
            self.agent_removed(who)?;
            self.index_change(who, None)?;
            self.close_sockets(&format!("p:{who}"), "membership revoked");
            // an agent's owner who read through it, and has no standing of their own now
            if let Some(owner) = removed.first().and_then(|r| r["owner"].as_str()) {
                let still = self.member_role(owner)?.is_some() || !self.rows("SELECT principal FROM members WHERE owner = ? LIMIT 1", vec![owner.into()])?.is_empty();
                if !still {
                    self.close_sockets(&format!("p:{owner}"), "their agent's membership was revoked");
                }
            }
        }
        self.sharing_changed()?;
        // every list's row names the agents left
        if agents_left {
            self.reindex()?;
        }
        Ok(())
    }

    /// Of `principals`, the members here whose rows count no more (Alex's
    /// review): in an org's fragment, a row of someone not in its org now
    /// (an agent's row: its person), or added before they came in again
    /// (`fragment_core::governance::counts`: a rehire starts fresh). What a request carries decides its own
    /// standing (`fence`); this decides where none does, for its
    /// deliveries, its members' lists and an admission. One registry read
    /// (`PlacesNow`); a person's fragment, and anyone with no row: none.
    pub(crate) async fn stale_members(&self, principals: &[String]) -> CellResult<std::collections::BTreeSet<String>> {
        let mut stale = std::collections::BTreeSet::new();
        let Some(org) = self.owner()?.org().map(str::to_string) else { return Ok(stale) };
        // each member's row: when it was added, and whose place it counts by
        let mut rows: Vec<(String, String, i64)> = vec![];
        for p in principals {
            if rows.iter().any(|r| &r.0 == p) {
                continue;
            }
            if let Some(r) = self.rows("SELECT kind, owner, added_at FROM members WHERE principal = ?", vec![p.as_str().into()])?.first() {
                let person = match (r["kind"].as_str(), r["owner"].as_str()) {
                    (Some("agent"), Some(owner)) => owner.to_string(),
                    _ => p.clone(),
                };
                rows.push((p.clone(), person, r["added_at"].as_i64().unwrap_or(0)));
            }
        }
        if rows.is_empty() {
            return Ok(stale);
        }
        let mut people: Vec<String> = rows.iter().map(|r| r.1.clone()).collect();
        people.sort();
        people.dedup();
        assert!(people.len() <= crate::registry::content::PLACES_ASKED_MAX, "a batch of members is bounded");
        let now = crate::ask_registry(&self.env, &crate::registry::content::PlacesNow { org, people }).await?;
        for (principal, person, added_at) in rows {
            let place = now.get(&person).map(|since| fragment_core::governance::Place { admin: false, since: *since });
            if !fragment_core::governance::counts(place, added_at) {
                stale.insert(principal);
            }
        }
        Ok(stale)
    }

    /// Of `asks` (a subscriber and the role what they are sent reads at),
    /// whether each may still be sent it (Alex's review): in an org's
    /// fragment, as a request of theirs would be decided now, by their
    /// membership as it counts (none when their row is stale or gone:
    /// `stale_members`) or else by its visibility (a link's holder counts
    /// on a link's: what they subscribed by is not revoked by an
    /// offboarding). A person's fragment: each may, as before, and no
    /// registry read. Bounded by the caller's batch.
    pub(crate) async fn receivers(&self, asks: &[(String, Role)]) -> CellResult<Vec<bool>> {
        if self.owner()?.org().is_none() {
            return Ok(vec![true; asks.len()]);
        }
        let principals: Vec<String> = asks.iter().map(|(p, _)| p.clone()).collect();
        let stale = self.stale_members(&principals).await?;
        let visibility = self.visibility()?;
        let mut may = Vec::with_capacity(asks.len());
        for (principal, read) in asks {
            let member = if stale.contains(principal) { None } else { self.member_role(principal)? };
            let standing = access::Standing { member, owns_member_agent: false, link: visibility == Visibility::Link, signed: npub::is_identity(principal), cap: None, held: None };
            may.push(matches!(access::decide(visibility, standing, access::Purpose::Read, *read), access::Decision::Allow(_)));
        }
        Ok(may)
    }

    /// In an org's fragment, `person`'s rows and their agents' that count no
    /// more (`stale_members`) are revoked grants: taken away, as a removal
    /// takes them, before anything admits them again, so it admits them
    /// afresh, from their place now, and an ordinary member's way back is
    /// still their own yes to an invite (Alex's review). Its event says so.
    pub(crate) async fn revoke_stale(&self, person: &str) -> CellResult<()> {
        if self.owner()?.org().is_none() {
            return Ok(());
        }
        let theirs: Vec<String> = self
            .rows("SELECT principal FROM members WHERE principal = ? OR owner = ?", vec![person.into(), person.into()])?
            .iter()
            .filter_map(|r| r["principal"].as_str().map(str::to_string))
            .collect();
        let stale: Vec<String> = self.stale_members(&theirs).await?.into_iter().collect();
        if stale.is_empty() {
            return Ok(());
        }
        self.revoke(&stale)?;
        self.event("member.fenced", &format!("{} was offboarded from its org since they came in: what they held here is taken away", npub::display(person)), json!({ "principal": npub::display(person), "revoked": stale.len() }));
        Ok(())
    }

    /// `who` (an agent: its person) in `org`'s content as they come into
    /// this, one of its fragments: the registry enrolls one not in it yet,
    /// refused for an invite made before they last left (`invited_at`).
    pub(crate) async fn enrolled(&self, org: &str, who: &Identity, invited_at: Option<i64>) -> CellResult<()> {
        let person = match who.kind {
            IdentityKind::Person => who.id.clone(),
            IdentityKind::Agent => who.owner.clone().ok_or_else(|| CellError::host("an agent has its owner"))?,
        };
        crate::ask_registry(&self.env, &crate::registry::content::Enroll { org: org.to_string(), person, invited_at }).await
    }

    pub(crate) async fn set_member(&self, caller: &Caller, who: &str, body: SetRole) -> CellResult<Response> {
        let actor = self.sharer(caller)?;
        // only who administers it learns whom a key names
        if !actor.admin {
            let why = access::refuse_set_role(false, None, body.role).expect("only who administers it manages members");
            return Err(refusal(false, why));
        }
        let target = self.named(who).await?;
        json_response(&self.put_member(&actor, &target, body, false).await?)
    }

    /// A test hook (`ops::test_fragment`'s `member`): `who` made a member,
    /// or their role changed, as its owner would, with no one's yes, so the
    /// e2e shares with people without mailing each an invite to accept.
    pub(crate) async fn lever_member(&self, who: &str, role: Role) -> CellResult<Member> {
        let target = self.named(who).await?;
        // an org's: in its content first (a test's, named whole)
        if let Some(org) = self.owner()?.org() {
            self.enrolled(org, &target, None).await?;
        }
        let actor = Actor { admin: true, by: self.owner()?.name(), for_owner: None };
        self.put_member(&actor, &target, SetRole { role }, true).await
    }

    /// `target` made a member at `body`'s role, or their role changed, by
    /// `actor`, the owner or their agent sharing for them. A new member
    /// this way is one of its owner's own agents (decision 63: "I shouldn't
    /// be able to add someone to something and then it just shows up for
    /// them"): anyone else joins by their own yes to an invite, but for a
    /// test's `lever`.
    async fn put_member(&self, actor: &Actor, target: &Identity, body: SetRole, lever: bool) -> CellResult<Member> {
        // an org's: rows from before the person's place now are no grant
        // (a returning admin's own add comes in afresh)
        let person = match target.kind {
            IdentityKind::Agent => target.owner.clone().ok_or_else(|| CellError::host("an agent has its owner"))?,
            IdentityKind::Person => target.id.clone(),
        };
        self.revoke_stale(&person).await?;
        let current = self.member_role(&target.id)?;
        if let Some(why) = access::refuse_set_role(actor.admin, current, body.role) {
            return Err(refusal(true, why));
        }
        // added at once: its owner's own agents; an org's, a content admin
        // themselves or their own agents (decision 71: they may give
        // themselves access, which their event shows)
        let owner = self.owner()?;
        let own = match &owner {
            fragment_core::owner::Owner::Person(p) => target.owner.as_deref() == Some(p.as_str()),
            fragment_core::owner::Owner::Org(_) => target.id == actor.by || target.owner.as_deref() == Some(actor.by.as_str()),
        };
        if current.is_none() && !lever && !own {
            return Err(CellError::new(ErrorCode::Forbidden, match target.kind {
                IdentityKind::Person => "a person joins by their own yes: invite them by email instead (the share sheet's Add, `fragment members add <name> <email>`), and they are in once they accept it",
                IdentityKind::Agent => "someone else's agent joins with its owner: invite its owner by email instead (the share sheet's Add, `fragment members add <name> <email>`); once they accept, their agents act there for them",
            }));
        }
        self.connector_role(body.role)?;
        if current.is_some() && body.role < Role::Editor && self.chat_agent()?.as_deref() == Some(target.id.as_str()) {
            return Err(refusal(true, CHAT_AGENT_STAYS_EDITOR));
        }
        // an agent in with its person holds their role (decision 64)
        if target.kind == IdentityKind::Agent && self.entity_of(&target.id)?.is_some() {
            return Err(refusal(true, "it is in with its person, at their role: change theirs, and it follows"));
        }
        self.room_for(u64::from(current.is_none()))?;
        let by = actor.by.as_str();
        let now = js::now_ms();
        self.exec(
            "INSERT INTO members (principal, role, added_by, added_at, kind, owner) VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT (principal) DO UPDATE SET role = excluded.role",
            vec![
                target.id.as_str().into(),
                body.role.as_str().into(),
                by.into(),
                SqlStorageValue::Integer(now),
                target.kind.as_str().into(),
                opt(target.owner.as_deref()),
            ],
        )?;
        let row = self.rows(&format!("SELECT {MEMBER_COLUMNS} FROM members WHERE principal = ?"), vec![target.id.as_str().into()])?;
        let member = member_json(row.first().ok_or_else(|| CellError::host("a member just written is not there"))?)?;
        self.index_change(&target.id, Some(body.role))?;
        self.sharing_changed()?;
        // a new agent member's computer hears it joined (runs_on.rs), and
        // every list's row names it; a role change is no join
        if current.is_none() && target.kind == IdentityKind::Agent {
            self.agent_added(&target.id, target.owner.as_deref(), now)?;
            self.reindex()?;
        }
        // sharing with an agent says so: its owner reads what it reads (FIN-11)
        let summary = match &target.owner {
            Some(owner) => format!("{} (an agent) is now {}; its owner {owner} reads what it reads", target.id, body.role.as_str()),
            None => format!("{} is now {}", target.id, body.role.as_str()),
        };
        let data = json!({ "principal": target.id, "role": body.role, "kind": target.kind, "owner": target.owner });
        self.event("member.set", &format!("{summary}{}", actor.said()), actor.noted(data));
        // A socket's role is fixed when it opens, and it answers queries at
        // that role: a changed role reopens the member's sockets (and their
        // owner's, who reads through an agent), at the new one.
        if current.is_some_and(|was| was != body.role) {
            self.reopen_sockets(&format!("p:{}", target.id), "your role changed");
            if let Some(owner) = &target.owner {
                self.reopen_sockets(&format!("p:{owner}"), "your agent's role changed");
            }
            // a person's agents in with them take their role (decision 64)
            if target.kind == IdentityKind::Person && self.entity_of(&target.id)?.is_some() {
                let agents = self.entity_agents(&target.id)?;
                self.agents_join(&target.id, &agents, body.role)?;
            }
        }
        // the agent's list holds this fragment before its computer is told
        self.flush_index().await;
        self.flush_joined().await;
        Ok(member)
    }

    pub(crate) async fn remove_member(&self, caller: &Caller, who: &str) -> CellResult<Response> {
        self.name()?;
        let me = self.caller_id(caller)?.to_string();
        // a member is removed as it is (`me` is the caller), even one the
        // registry no longer holds; a key names the identity holding it
        let target = match npub::parse(who) {
            _ if who == "me" => me.clone(),
            Some(key) if self.member_role(&npub::identity_of(&key))?.is_some() => npub::identity_of(&key),
            Some(_) => self.named(who).await?.id,
            None if fragment_core::mail::valid_address(who) => self.named(&who.to_ascii_lowercase()).await?.id,
            None => return Err(CellError::invalid(format!("{who:?} is not an email, an npub, or a 64-hex key"))),
        };
        let is_self = me == target;
        // leaving is any member's own; removing anyone else is sharing
        let actor = match is_self {
            // its owner leaving is refused as theirs to ask (400), not as
            // someone who may not (an org's fragment has no owner member)
            true => Actor { admin: self.member_role(&me)? == Some(Role::Owner), by: me.clone(), for_owner: None },
            false => self.sharer(caller)?,
        };
        let current = self.member_role(&target)?;
        if let Some(why) = access::refuse_remove(actor.admin, is_self, current) {
            return Err(match current {
                None => CellError::new(ErrorCode::NotFound, why),
                Some(_) => refusal(actor.admin, why),
            });
        }
        // the whole entity goes (decision 64): a person with the agents in
        // with them, or such an agent with its person and theirs
        let entity = self.entity_of(&target)?;
        let gone: Vec<String> = match &entity {
            Some(person) => std::iter::once(person.clone()).chain(self.entity_agents(person)?).collect(),
            None => vec![target.clone()],
        };
        // a chat's own agent is not removed by anyone else (it may leave:
        // its owner's wipe does so, ended.rs)
        let lead = self.chat_agent()?;
        if !is_self && gone.iter().any(|g| lead.as_deref() == Some(g.as_str())) {
            return Err(refusal(actor.admin, CHAT_AGENT_STAYS));
        }
        self.revoke(&gone)?;
        let principal = npub::display(entity.as_deref().unwrap_or(&target));
        let how = match (is_self, gone.len() > 1) {
            (true, true) => "left, with their agents",
            (true, false) => "left",
            (false, true) => "was removed, with their agents",
            (false, false) => "was removed",
        };
        self.event("member.removed", &format!("{principal} {how}{}", actor.said()), actor.noted(json!({ "principal": principal })));
        self.flush_index().await;
        json_response(&json!({ "ok": true, "removed": npub::display(&target) }))
    }

    /// Drops the invites that ran out: the alarm does at each one's expiry
    /// (`invites_due_at` arms it; fragment.rs `on_alarm`), and every call
    /// that reads them does first. When any went, the owner's list counts
    /// them no more: its row is sent again, the alarm armed to send it (a
    /// call's own flush may send it first). Until then an invite that ran
    /// out was counted on its owner's list, and shown waiting, until some
    /// other change to its sharing.
    pub(crate) async fn drop_spent_invites(&self) -> CellResult<()> {
        let now = js::now_ms();
        let gone = self.rows("DELETE FROM invites WHERE expires_at <= ? RETURNING email", vec![SqlStorageValue::Integer(now)])?;
        assert!(gone.len() <= limits::INVITES_MAX, "a fragment holds at most INVITES_MAX invites");
        assert!(self.invites_due_at()?.is_none_or(|at| at > now), "no invite left has run out");
        if !gone.is_empty() {
            self.sharing_changed()?;
            self.schedule().await?;
        }
        Ok(())
    }

    /// When the next invite waiting runs out, if one waits (for the alarm).
    pub(crate) fn invites_due_at(&self) -> CellResult<Option<i64>> {
        Ok(self.rows("SELECT MIN(expires_at) AS at FROM invites", vec![])?.first().and_then(|r| r["at"].as_i64()))
    }

    /// Its title as its members' lists show it, if it has one.
    pub(crate) fn title(&self) -> CellResult<Option<String>> {
        let face: Option<Value> = self.meta(MetaKey::Face)?.and_then(|f| serde_json::from_str(&f).ok());
        Ok(face.and_then(|f| f["title"].as_str().map(str::to_string)).filter(|t| !t.is_empty()))
    }

    /// An agent fragment's role, as its face keeps it (`face`): none for
    /// any other fragment.
    pub(crate) fn agent_role(&self) -> CellResult<Option<String>> {
        let face: Option<Value> = self.meta(MetaKey::Face)?.and_then(|f| serde_json::from_str(&f).ok());
        Ok(face.and_then(|f| f["role"].as_str().map(str::to_string)))
    }

    /// Shares it with whoever signs in as `body.email` (decisions 48 and
    /// 63): the invite waits on the email here, and the registry keeps it
    /// against the email and mails them, for their yes, which makes them a
    /// member (`claim_invite`), whether they sign in as it already or not
    /// yet. Someone in it already is not invited (a role changes with `PUT
    /// members`). It is written before the registry is asked, so a yes
    /// meanwhile meets it, and taken back when the registry refuses (a
    /// day's mails spent, a mail not sent). No event names the email: a
    /// link's viewers read the events.
    pub(crate) async fn create_invite(&self, caller: &Caller, body: CreateInvite) -> CellResult<Response> {
        let actor = self.require_owner(caller)?;
        if !matches!(body.role, Role::Viewer | Role::Editor) {
            return Err(CellError::invalid("an invite grants viewer or editor"));
        }
        self.connector_role(body.role)?;
        let email = body.email.trim().to_ascii_lowercase();
        if !fragment_core::mail::valid_address(&email) {
            return Err(CellError::invalid(format!("{email:?} is not an email")));
        }
        let name = self.name()?;
        // an invite adds someone; it never changes a member's role
        match self.named(&email).await {
            Ok(person) => {
                // an org's member offboarded since holds nothing to be in by
                self.revoke_stale(&person.id).await?;
                if let Some(role) = self.member_role(&person.id)? {
                    return Err(CellError::invalid(already_in(&email, role)));
                }
            }
            Err(e) if e.code == ErrorCode::NotFound => {}
            Err(e) => return Err(e),
        }
        self.drop_spent_invites().await?;
        let before = self.rows("SELECT email, role, expires_at, created_by, created_at FROM invites WHERE email = ?", vec![email.as_str().into()])?;
        if before.is_empty() && self.count("SELECT COUNT(*) AS n FROM invites")? >= limits::INVITES_MAX as u64 {
            return Err(CellError::invalid(format!("a fragment has at most {} invites waiting", limits::INVITES_MAX)));
        }
        let now = js::now_ms();
        let expires_at = now + limits::INVITE_TTL_S * 1000;
        let write = |role: &str, expires_at: i64, by: &str, at: i64| {
            self.exec(
                "INSERT INTO invites (email, role, expires_at, created_by, created_at) VALUES (?, ?, ?, ?, ?)
                 ON CONFLICT (email) DO UPDATE SET role = excluded.role, expires_at = excluded.expires_at,
                   created_by = excluded.created_by, created_at = excluded.created_at",
                vec![email.as_str().into(), role.into(), SqlStorageValue::Integer(expires_at), by.into(), SqlStorageValue::Integer(at)],
            )
        };
        write(body.role.as_str(), expires_at, &actor.by, now)?;
        let asked = InviteEmail {
            email: email.clone(),
            fragment: name.clone(),
            // an agent's mail counts in its owner's day
            sharer: actor.for_owner.clone().unwrap_or_else(|| actor.by.clone()),
            role: body.role,
            title: self.title()?.unwrap_or_else(|| crate::share::label(&name).to_string()),
            expires_at,
        };
        let answer = crate::ask_registry(&self.env, &asked).await;
        match answer {
            Ok(()) => {
                // the owner's list counts the invites waiting
                self.sharing_changed()?;
                self.event(
                    "invite.created",
                    &format!("an invite waits on an email, as {}{}", body.role.as_str(), actor.said()),
                    actor.noted(json!({ "role": body.role, "expiresAt": expires_at })),
                );
                self.flush_index().await;
                let invite = Invite { email, role: body.role, expires_at, created_by: actor.by.clone() };
                json_response(&Shared::Invited(invite))
            }
            Err(e) => {
                // what this call wrote, unless a yes met it meanwhile
                self.exec("DELETE FROM invites WHERE email = ? AND created_at = ?", vec![email.as_str().into(), SqlStorageValue::Integer(now)])?;
                if let Some(was) = before.first() {
                    let field = |k: &str| was[k].as_str().unwrap_or_default().to_string();
                    let int = |k: &str| was[k].as_i64().unwrap_or_default();
                    write(&field("role"), int("expires_at"), &field("created_by"), int("created_at"))?;
                }
                Err(e)
            }
        }
    }

    pub(crate) async fn invites(&self, caller: &Caller) -> CellResult<Response> {
        self.require_owner(caller)?;
        self.drop_spent_invites().await?;
        let rows = self.rows("SELECT email, role, expires_at, created_by FROM invites ORDER BY created_at, email", vec![])?;
        json_response(&InviteList { invites: rows.iter().map(invite_json).collect() })
    }

    pub(crate) async fn revoke_invite(&self, caller: &Caller, email: &str) -> CellResult<Response> {
        let actor = self.require_owner(caller)?;
        let email = email.trim().to_ascii_lowercase();
        self.drop_spent_invites().await?;
        if self.rows("SELECT email FROM invites WHERE email = ?", vec![email.as_str().into()])?.is_empty() {
            return Err(CellError::new(ErrorCode::NotFound, format!("no invite waits on {email}")));
        }
        self.exec("DELETE FROM invites WHERE email = ?", vec![email.as_str().into()])?;
        // the owner's list counts the invites waiting
        self.sharing_changed()?;
        self.event("invite.revoked", &format!("an invite was revoked{}", actor.said()), actor.noted(json!({})));
        self.flush_index().await;
        // and its person sees it no more: a registry that does not hear it
        // keeps showing it to them until it runs out, or they answer it
        // (their yes then finds none waiting here, and says it is gone)
        let forget = ForgetInvite { email: Some(email.clone()), fragment: self.name()? };
        if let Err(e) = crate::ask_registry(&self.env, &forget).await {
            console_error!("{}: its revoked invite was not forgotten by the registry ({:?}): {}", forget.fragment, e.code, e.message);
        }
        json_response(&json!({ "ok": true, "revoked": email }))
    }

    /// An ended life's invites waiting go from their people's sidebars with
    /// it (the registry's, told at its end: delete or wipe). Best effort: a
    /// registry that does not hear it shows them until they run out or are
    /// answered (a yes then finds no fragment, says it is gone, and forgets it).
    pub(crate) async fn forget_invites(&self, name: &str) {
        let forget = ForgetInvite { email: None, fragment: name.to_string() };
        if let Err(e) = crate::ask_registry(&self.env, &forget).await {
            console_error!("{name}: its ended life's invites were not forgotten by the registry ({:?}): {}", e.code, e.message);
        }
    }

    /// The invite waiting on `email`, declined by the person who signs in
    /// as it (the registry's `AnswerInvite`, a no): it waits no more, and its
    /// owner's list counts it no more. 404 when none waits. Its event
    /// names no one: they are not in it.
    pub(crate) async fn decline_invite(&self, claim: &ClaimInvite) -> CellResult<Value> {
        self.name()?;
        assert!(npub::is_identity(&claim.identity), "the registry names a person by their npub");
        self.drop_spent_invites().await?;
        let gone = self.rows("DELETE FROM invites WHERE email = ? RETURNING role", vec![claim.email.as_str().into()])?;
        if gone.is_empty() {
            return Err(CellError::new(ErrorCode::NotFound, "no invite waits on that email"));
        }
        self.sharing_changed()?;
        self.event("invite.declined", "an invite was declined", json!({}));
        self.flush_index().await;
        Ok(json!({ "ok": true }))
    }

    /// The invite waiting on `email`, met by `identity`, a person who signs
    /// in as it and accepted it (the registry's `AnswerInvite`, a yes; or, for an
    /// invite from before invites waited for a yes, just signed in as it):
    /// a member at the role it said, added by its maker, and a stronger
    /// role they hold kept; either way it is spent, and its owner's list
    /// counts it no more. 404 when none waits (revoked, expired, or met
    /// already), and the registry forgets it. A full fragment keeps it.
    /// Their agents (`claim.agents`, the registry's) come in with them
    /// (decision 64: `agents_join`), or the claim is refused whole.
    pub(crate) async fn claim_invite(&self, claim: &ClaimInvite) -> CellResult<Value> {
        self.name()?;
        assert!(npub::is_identity(&claim.identity), "the registry names a person by their npub");
        self.drop_spent_invites().await?;
        // an org's: they come into its content as they come in here (an
        // invite from before they last left admits no one); asked first, so
        // what follows is read after it
        if let Some(org) = self.owner()?.org() {
            let invited = self.rows("SELECT created_at FROM invites WHERE email = ?", vec![claim.email.as_str().into()])?;
            let Some(at) = invited.first().and_then(|r| r["created_at"].as_i64()) else {
                return Err(CellError::new(ErrorCode::NotFound, "no invite waits on that email"));
            };
            let person = Identity { id: claim.identity.clone(), kind: IdentityKind::Person, owner: None, held: None, orgs: vec![] };
            self.enrolled(org, &person, Some(at)).await?;
        }
        let rows = self.rows("SELECT role, created_by FROM invites WHERE email = ?", vec![claim.email.as_str().into()])?;
        let Some(row) = rows.first() else {
            return Err(CellError::new(ErrorCode::NotFound, "no invite waits on that email"));
        };
        let role = row["role"].as_str().and_then(Role::parse).ok_or_else(|| CellError::host("invites.role"))?;
        let by = row["created_by"].as_str().unwrap_or_default().to_string();
        let who = claim.identity.as_str();
        // what they held from before an offboarding is no grant: they come in afresh
        self.revoke_stale(who).await?;
        let current = self.member_role(who)?;
        let kept = current.is_some_and(|c| c >= role);
        // the owner is in already, with their agents as they chose them
        let owner = current == Some(Role::Owner);
        let agents: &[String] = if owner { &[] } else { &claim.agents };
        self.room_for(u64::from(current.is_none()) + self.not_members(agents)?)?;
        self.exec("DELETE FROM invites WHERE email = ?", vec![claim.email.as_str().into()])?;
        // met, whatever it grants: the owner's list counts it no more
        self.sharing_changed()?;
        let now = if kept { current.expect("kept is a role held") } else { role };
        if !kept {
            self.exec(
                "INSERT INTO members (principal, role, added_by, added_at, kind) VALUES (?, ?, ?, ?, 'person')
                 ON CONFLICT (principal) DO UPDATE SET role = excluded.role",
                vec![who.into(), role.as_str().into(), by.as_str().into(), SqlStorageValue::Integer(js::now_ms())],
            )?;
            self.index_change(who, Some(role))?;
            self.event("member.joined", &format!("{who} joined as {}, invited by email, with their agents", role.as_str()), json!({ "principal": who, "role": role, "by": by }));
            if current.is_some() {
                self.reopen_sockets(&format!("p:{who}"), "your role changed");
            }
        }
        if !owner {
            self.agents_join(who, agents, now)?;
        }
        self.flush_index().await;
        self.flush_joined().await;
        Ok(json!({ "role": now }))
    }

    /// How many of `ids` are no members here.
    fn not_members(&self, ids: &[String]) -> CellResult<u64> {
        let mut n = 0;
        // bounded by the caller's list (a person's agents)
        for id in ids {
            n += u64::from(self.member_role(id)?.is_none());
        }
        Ok(n)
    }

    /// `person`'s agents in with them, at their role (decision 64; Paul,
    /// 2026-10-23: "If I invite a member to a fragment, when they accept
    /// they add their agent... We have to treat them as one entity and work
    /// to secure the whole entity"): each a member as itself, so its own
    /// list names the fragment, a chat's roster has it and its computer
    /// hears it joined; one in already takes the role. No event names
    /// them (the person's says "with their agents"), nor does the sheet
    /// (share.rs `with_their_person`). The caller made the room. Bounded:
    /// a person has at most `AGENTS_PER_OWNER_MAX` agents.
    fn agents_join(&self, person: &str, agents: &[String], role: Role) -> CellResult<()> {
        assert!(agents.len() as u64 <= limits::AGENTS_PER_OWNER_MAX, "a person has a bounded number of agents");
        assert!(matches!(role, Role::Viewer | Role::Editor), "an agent is in at its person's role, never owner (R17)");
        let now = js::now_ms();
        let mut joined = false;
        for agent in agents {
            let current = self.member_role(agent)?;
            if current == Some(role) {
                continue;
            }
            self.exec(
                "INSERT INTO members (principal, role, added_by, added_at, kind, owner) VALUES (?, ?, ?, ?, 'agent', ?)
                 ON CONFLICT (principal) DO UPDATE SET role = excluded.role",
                vec![agent.as_str().into(), role.as_str().into(), person.into(), SqlStorageValue::Integer(now), person.into()],
            )?;
            self.index_change(agent, Some(role))?;
            match current {
                None => {
                    self.agent_added(agent, Some(person), now)?;
                    joined = true;
                }
                Some(_) => self.reopen_sockets(&format!("p:{agent}"), "your role changed"),
            }
        }
        // every list's row names its agents
        if joined {
            self.reindex()?;
        }
        Ok(())
    }

    /// The person whose entity `principal` is part of here (decision 64):
    /// themselves, a member who is not the owner; or, for an agent, its
    /// person, such a member. None for the owner and their own agents
    /// (each in as its owner chose), and for anyone not in.
    fn entity_of(&self, principal: &str) -> CellResult<Option<String>> {
        let rows = self.rows(
            "SELECT m.kind, m.owner, (SELECT role FROM members WHERE principal = m.owner) AS owner_role, m.role FROM members m WHERE m.principal = ?",
            vec![principal.into()],
        )?;
        let Some(r) = rows.first() else { return Ok(None) };
        let in_with = |role: &Value| role.as_str().is_some_and(|r| r != Role::Owner.as_str());
        Ok(match (r["kind"].as_str(), r["owner"].as_str()) {
            (Some("agent"), Some(person)) if in_with(&r["owner_role"]) => Some(person.to_string()),
            (Some("agent"), _) => None,
            _ if in_with(&r["role"]) => Some(principal.to_string()),
            _ => None,
        })
    }

    /// `person`'s agents in with them.
    fn entity_agents(&self, person: &str) -> CellResult<Vec<String>> {
        let rows = self.rows(
            "SELECT principal FROM members WHERE owner = ? AND kind = 'agent' ORDER BY added_at, principal LIMIT ?",
            vec![person.into(), SqlStorageValue::Integer(limits::AGENTS_PER_OWNER_MAX as i64)],
        )?;
        Ok(rows.iter().filter_map(|r| r["principal"].as_str().map(str::to_string)).collect())
    }

    /// `members/agent` (the platform's, as an agent is made: `agent_made`):
    /// `agent`, just made by `person`, comes in beside them when they are a
    /// member here who is not its owner, at their role. Anything else
    /// changes nothing.
    pub(crate) async fn agent_with_person(&self, body: &Value) -> CellResult<Value> {
        self.name()?;
        let (Some(person), Some(agent)) = (body["person"].as_str(), body["agent"].as_str()) else {
            return Err(CellError::invalid("members/agent names a person and their agent"));
        };
        if !npub::is_identity(person) || !npub::is_identity(agent) {
            return Err(CellError::invalid("members/agent names identities"));
        }
        if self.entity_of(person)?.as_deref() != Some(person) {
            return Ok(json!({ "joined": false }));
        }
        let role = self.member_role(person)?.expect("a person in is a member");
        let agents = [agent.to_string()];
        self.room_for(self.not_members(&agents)?)?;
        self.agents_join(person, &agents, role)?;
        self.flush_index().await;
        self.flush_joined().await;
        Ok(json!({ "joined": true }))
    }

    /// The fragment's sharing, as its owner's list carries it: `guests`
    /// counts the members who are neither the owner nor an agent of theirs,
    /// nor an agent in with its person (decision 64: one entity, counted
    /// once), and `invites` those waiting on an email (as of this change: one
    /// that runs out later is counted until the alarm drops it at its
    /// expiry, which sends the row again: `drop_spent_invites`).
    pub(crate) fn sharing_counts(&self) -> CellResult<Sharing> {
        let visibility = self.visibility()?;
        let owner = self.owner()?.name();
        #[derive(serde::Deserialize)]
        struct Counts {
            members: u64,
            guests: u64,
            invites: u64,
        }
        let counts: Vec<Counts> = self.typed(
            "SELECT (SELECT COUNT(*) FROM members) AS members,
                    (SELECT COUNT(*) FROM members m WHERE principal != ? AND (owner IS NULL OR owner != ?)
                       AND NOT EXISTS (SELECT 1 FROM members p WHERE p.principal = m.owner)) AS guests,
                    (SELECT COUNT(*) FROM invites WHERE expires_at > ?) AS invites",
            vec![owner.as_str().into(), owner.as_str().into(), SqlStorageValue::Integer(js::now_ms())],
        )?;
        let counts = counts.into_iter().next().expect("a SELECT without FROM answers one row");
        assert!(counts.guests <= counts.members, "guests are members");
        assert!(counts.invites <= limits::INVITES_MAX as u64, "a fragment holds at most INVITES_MAX invites");
        Ok(Sharing { visibility, members: counts.members, guests: counts.guests, invites: Some(counts.invites) })
    }

    pub(crate) async fn set_visibility(&self, caller: &Caller, body: SetVisibility) -> CellResult<Response> {
        let actor = self.require_owner(caller)?;
        let before = self.visibility()?;
        self.set_meta(MetaKey::Visibility, body.visibility.as_str())?;
        // opened again from members only, it gets a new share link: one
        // that stopped working when it was restricted stays dead
        let renewed = before == Visibility::Members && body.visibility != Visibility::Members;
        if renewed {
            self.set_meta(MetaKey::ViewToken, &js::random_hex::<12>())?;
        }
        self.sharing_changed()?;
        if body.visibility != Visibility::Public {
            self.close_sockets("anon", "the fragment is no longer public");
        }
        if body.visibility == Visibility::Members {
            self.close_sockets("view", "the fragment is now members only");
        }
        self.event(
            "visibility",
            &format!("visibility {} → {}{}{}", before.as_str(), body.visibility.as_str(), if renewed { ", with a new share link" } else { "" }, actor.said()),
            actor.noted(json!({ "from": before, "to": body.visibility, "newLink": renewed })),
        );
        self.flush_index().await;
        json_response(&json!({ "ok": true, "visibility": body.visibility }))
    }

    /// New share-link token or inbox token (default: both). An agent
    /// shares for its owner: it renews them as its owner does.
    pub(crate) fn rotate(&self, caller: &Caller, body: Value) -> CellResult<Response> {
        let actor = self.require_owner(caller)?;
        let all = ["inbox", "view"];
        let want: Vec<String> = match &body["scopes"] {
            Value::Array(a) if !a.is_empty() => a.iter().map(|v| v.as_str().unwrap_or("").to_string()).collect(),
            Value::Null | Value::Array(_) => all.iter().map(|s| s.to_string()).collect(),
            _ => return Err(CellError::invalid("scopes must be an array of inbox, view")),
        };
        if let Some(bad) = want.iter().find(|s| !all.contains(&s.as_str())) {
            return Err(CellError::invalid(format!("unknown scope {bad:?} (inbox, view)")));
        }
        for (scope, key, fresh) in [("inbox", MetaKey::InboxToken, js::random_hex::<16>()), ("view", MetaKey::ViewToken, js::random_hex::<12>())] {
            if want.iter().any(|w| w == scope) {
                self.set_meta(key, &fresh)?;
            }
        }
        if want.iter().any(|w| w == "view") {
            self.close_sockets("view", "the share link changed");
        }
        let rotated: Vec<&str> = all.into_iter().filter(|s| want.iter().any(|w| w == s)).collect();
        self.event("tokens.rotated", &format!("{}{}", rotated.join("+"), actor.said()), actor.noted(json!({ "scopes": rotated })));
        json_response(&Rotated {
            inbox_token: self.must(MetaKey::InboxToken)?,
            view_token: self.must(MetaKey::ViewToken)?,
            rotated: rotated.iter().map(|s| s.to_string()).collect(),
        })
    }

    /// `PUT secrets/<KEY>` `{value, hosts}`: the value sealed, and the
    /// hosts it may go to put in place of the ones before (`may_fill`).
    /// Both come together, so the hosts of a value change only with it.
    pub(crate) async fn put_secret(&self, caller: &Caller, key: &str, body: Vec<u8>) -> CellResult<Response> {
        self.require(caller, false, Role::Editor)?;
        if !fragment_proto::valid_secret_name(key) {
            return Err(CellError::invalid("a secret's name must match ^[A-Z][A-Z0-9_]{0,63}$"));
        }
        let put: PutSecret = serde_json::from_slice(&body).map_err(|e| CellError::invalid(format!("a secret is {{value, hosts}}: {e}")))?;
        if put.value.is_empty() {
            return Err(CellError::invalid("a secret's value is empty"));
        }
        if put.value.len() > limits::SECRET_MAX_BYTES {
            return Err(CellError::too_large("a secret", put.value.len(), limits::SECRET_MAX_BYTES));
        }
        let hosts = secret_hosts(&put.hosts)?;
        // sealed first (reading the host secret may yield): the checks and
        // the write below share one turn
        let sealed = crate::keys::seal(&self.env, &self.scope(), put.value.as_bytes()).await?;
        self.require(caller, false, Role::Editor)?;
        let exists = !self.rows("SELECT name FROM secrets WHERE name = ?", vec![key.into()])?.is_empty();
        if !exists && self.count("SELECT COUNT(*) AS n FROM secrets")? >= limits::SECRETS_MAX as u64 {
            return Err(CellError::invalid(format!("a fragment has at most {} secrets", limits::SECRETS_MAX)));
        }
        let by = self.caller_id(caller)?;
        self.exec(
            "INSERT INTO secrets (name, sealed, set_by, set_at) VALUES (?, ?, ?, ?)
             ON CONFLICT (name) DO UPDATE SET sealed = excluded.sealed, set_by = excluded.set_by, set_at = excluded.set_at",
            vec![key.into(), sealed.into(), by.into(), SqlStorageValue::Integer(js::now_ms())],
        )?;
        self.exec("DELETE FROM secret_hosts WHERE name = ?", vec![key.into()])?;
        for host in &hosts {
            self.exec("INSERT INTO secret_hosts (name, host) VALUES (?, ?)", vec![key.into(), host.as_str().into()])?;
        }
        let stored = self.stored_secret(key)?.map(|s| s.hosts);
        assert_eq!(stored.as_ref(), Some(&hosts), "a secret's hosts are the ones just set");
        // its hosts are its editors' to know (`GET secrets`), never the
        // event log's, which every viewer reads
        self.event("secret.set", &format!("secret {key} set by {}", npub::display(by)), json!({ "name": key }));
        json_response(&json!({ "ok": true, "name": key, "hosts": hosts }))
    }

    pub(crate) fn list_secrets(&self, caller: &Caller) -> CellResult<Response> {
        self.require(caller, false, Role::Editor)?;
        let mut secrets: Vec<SecretInfo> = vec![];
        // at most SECRETS_MAX secrets of SECRET_HOSTS_MAX hosts each
        for r in self.rows("SELECT s.name, h.host FROM secrets s LEFT JOIN secret_hosts h ON h.name = s.name ORDER BY s.name, h.host", vec![])? {
            let name = r["name"].as_str().expect("secrets.name is TEXT NOT NULL");
            if secrets.last().is_none_or(|s| s.name != name) {
                secrets.push(SecretInfo { name: name.to_string(), hosts: vec![] });
            }
            if let Some(host) = r["host"].as_str() {
                secrets.last_mut().expect("pushed above").hosts.push(host.to_string());
            }
        }
        json_response(&SecretList { secrets })
    }

    pub(crate) fn delete_secret(&self, caller: &Caller, key: &str) -> CellResult<Response> {
        self.require(caller, false, Role::Editor)?;
        let removed = !self.rows("SELECT name FROM secrets WHERE name = ?", vec![key.into()])?.is_empty();
        self.exec("DELETE FROM secrets WHERE name = ?", vec![key.into()])?;
        self.exec("DELETE FROM secret_hosts WHERE name = ?", vec![key.into()])?;
        if removed {
            self.event("secret.removed", &format!("secret {key} removed"), json!({ "name": key }));
        }
        json_response(&json!({ "ok": true, "removed": removed }))
    }
}

/// The hosts a `PUT secrets` names, each as a URL names it, without
/// repeats: at least one (a secret that may go nowhere is no use), and at
/// most `SECRET_HOSTS_MAX`.
fn secret_hosts(given: &[String]) -> CellResult<Vec<String>> {
    if given.is_empty() {
        return Err(CellError::invalid("a secret names the hosts it may go to (hosts: [\"api.example.com\"]; the CLI's --to)"));
    }
    if given.len() > limits::SECRET_HOSTS_MAX {
        return Err(CellError::invalid(format!("a secret goes to at most {} hosts", limits::SECRET_HOSTS_MAX)));
    }
    let mut hosts = Vec::with_capacity(given.len());
    for g in given {
        let host = fragment_core::secrets::secret_host(g).map_err(|not| CellError::invalid(not.to_string()))?;
        if !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    hosts.sort();
    assert!(!hosts.is_empty() && hosts.len() <= limits::SECRET_HOSTS_MAX, "a secret's hosts are within their bounds");
    Ok(hosts)
}

/// A fragment's face as its members' lists keep it (`MetaKey::Face`): an
/// agent fragment's with its role and its picture's address, when it has
/// them. Its kind is a manifest's, which names only kinds this release
/// knows: the fallback a newer platform's kind reads as is never stored as
/// one.
pub(crate) fn face(kind: FragmentKind, title: Option<&str>, role: Option<&str>, picture: Option<&str>) -> String {
    assert_ne!(kind, FragmentKind::Other, "a face's kind is its manifest's, one this release knows");
    // a title of nothing but spaces is none: every reader then falls back alike
    let mut face = json!({ "kind": kind, "title": fragment_core::face::title(title) });
    // a role is an agent's alone: the tag beside its name
    if let Some(r) = fragment_core::face::role(role).filter(|_| kind == FragmentKind::Agent) {
        face["role"] = json!(r);
    }
    if let Some(p) = picture {
        face["picture"] = json!(p);
    }
    face.to_string()
}
