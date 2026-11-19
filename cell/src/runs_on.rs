//! A fragment's side of computers (docs/computers.md): an agent fragment
//! running on one, and a fragment whose channel wakes one.
//!
//! - **An agent fragment** is assigned to its owner's computer. Its own
//!   key becomes the agent's identity (registered to the fragment's owner),
//!   and it signs the guest's requests (NIP-98) only for the computer it is
//!   assigned to: the computer's egress asks, naming itself.
//! - **A wake subscription** (`{channel, wake: true}` from a computer's
//!   guest, which only its egress can ask for) is a subscription whose URL
//!   is `computer:<id>`: each new record on the channel wakes it, and the
//!   guest reads the channel itself. A page opening the fragment, which a
//!   computer may be about to hear from, wakes it early (a pre-wake).
//! - **An agent added as a member** (Paul, 2026-10-03: agents are woken
//!   eagerly, and no template must remember a convention): the platform
//!   tells its agent, and the agent its computer. The membership's write
//!   keeps a row in this fragment's `joined_outbox` in the same turn;
//!   delivering it asks the agent's own fragment (the registry names it),
//!   which posts `{kind: "joined", fragment}` on its `tasks` (keyed by
//!   this membership, so a retry posts nothing twice) while it runs on a
//!   computer, and names that computer; this fragment then wakes it
//!   (`Wake::Joined`) on its own. The computer is found from the agent,
//!   never from its owner. A row that fails for a passing reason is tried
//!   again from the alarm; an agent that runs on no computer is told
//!   nothing.

use fragment_core::grants::Grants;
use fragment_core::{connectors::Action, door, npub};
use fragment_proto::{valid_channel_name, valid_fragment_name, ErrorCode, IdentityKind, Role};
use serde::Deserialize;
use serde_json::{json, Value};
use worker::*;

use crate::error::{CellError, CellResult};
use crate::fragment::calls::{AgentIdentity, AgentJoined, DoorActor, DoorCheck, JoinedAnswer, Named, Reach, Routine};
use crate::fragment::{FragmentCell, MetaKey};
use crate::js;
use crate::registry::calls::{self, By};

/// At most one pre-wake a computer from one fragment this often.
const PREWAKE_EVERY_MS: i64 = 30_000;
/// The agent fragment's channel its computer follows for its routines and
/// its new fragments (docs/chat-records.md, `tasks`).
pub(crate) const TASKS: &str = "tasks";
/// Notices one flush sends: a member's change makes one, so a flush has
/// few; the rest wait for the next.
const JOINED_FLUSH_MAX: i64 = 16;
/// Tries at one notice before it is dropped, with an event: with the wait
/// doubling to ten minutes, about two and a half hours of a computer that
/// does not answer.
const JOINED_ATTEMPTS_MAX: i64 = 20;

/// A `joined_outbox` row, as `agent_added` wrote it (its `owner` is no
/// longer read: the agent names its computer).
#[derive(Deserialize)]
struct Notice {
    principal: String,
    added_at: i64,
    attempts: i64,
}

/// A failure that trying again may change: the registry, or the agent's
/// fragment, did not answer, or a deploy is going out. Asking again is
/// safe: the registry's answer is a read, and the agent's notice is keyed
/// by its membership, so a second ask posts nothing twice.
fn passing(code: ErrorCode) -> bool {
    matches!(code, ErrorCode::HostFailed | ErrorCode::UpstreamFailed | ErrorCode::NodeFull | ErrorCode::RateLimited | ErrorCode::RegistryUnavailable | ErrorCode::Deploying)
}

#[derive(Deserialize)]
struct Assign {
    computer: String,
    owner: String,
}

#[derive(Deserialize)]
struct Sign {
    computer: String,
    method: String,
    url: String,
    payload: Option<String>,
}

#[derive(Deserialize)]
struct Subscribe {
    computer: String,
    identity: String,
    channel: String,
}

/// A wake subscription's URL: no one fetches it.
pub(crate) fn wake_url(computer: &str) -> String {
    format!("computer:{}", computer.trim_start_matches("computer:"))
}

impl FragmentCell {
    /// Whether this agent's owner may `action` its door `door` now: the
    /// deployment's connector policy (fragment_core::connectors), asked at
    /// each ticket, redemption and relayed record's check. A door the
    /// policy no longer allows is refused at once, and still unlinked.
    fn door_allowed(&self, door: &str, action: fragment_core::connectors::Action) -> CellResult<()> {
        if !fragment_core::connectors::id_ok(door) {
            return Err(CellError::invalid("door: a connector's id (1 to 32 lower-case letters, digits and single dashes)"));
        }
        let (owner, agent) = (self.must(MetaKey::Owner)?, self.name()?);
        let policy = self.connector_policy()?;
        policy.authorize(&owner, &agent, door, action).map_err(|why| CellError::new(ErrorCode::Forbidden, format!("door {door} {why}")))
    }

    /// `calls::Picture`: an agent fragment's picture, for `GET
    /// /api/identities/<npub>/picture`: its bytes from the commit live's
    /// install checked them at (`MetaKey::Picture`: `<sha256> <type>
    /// <commit>`), so a later commit never serves bytes no install checked.
    pub(crate) async fn picture(&self) -> CellResult<Response> {
        let none = || CellError::new(ErrorCode::NotFound, "this agent has no picture");
        self.must(MetaKey::CreatedAt).map_err(|_| none())?;
        let stored = self.meta(MetaKey::Picture)?.ok_or_else(none)?;
        let [_, mime, commit] = stored.splitn(3, ' ').collect::<Vec<_>>()[..] else { return Err(CellError::host(format!("its stored picture {stored:?} is not `<sha256> <type> <commit>`"))) };
        let upstream = self.cs()?.stream(&self.must(MetaKey::Repo)?, commit, fragment_core::face::AGENT_PICTURE).await?;
        let headers = Headers::new();
        headers.set("content-type", mime)?;
        headers.set("cache-control", "public, max-age=300")?;
        headers.set("x-content-type-options", "nosniff")?;
        let (_, body) = upstream.into_parts();
        Ok(Response::from_body(body)?.with_headers(headers))
    }

    /// `computer/grants`: what this agent may use of its owner's
    /// (`fragment_core::grants`), asked by the computer it runs on at each
    /// use (`{computer, identity}`): what it holds, or, holding none of its
    /// own, that computer's row from before the move (`taken`), taken once;
    /// then its owner's change, in person (`owner` with `connections`, or
    /// with `provider` and `account`: their computer's routes, which a
    /// signed person alone reaches). Read field by field, as this file's
    /// other bodies are (the cell's Wasm budget).
    pub(crate) fn grants(&self, body: &[u8]) -> CellResult<Grants> {
        use fragment_core::grants::{answered, Change, Row};
        self.must(MetaKey::CreatedAt).map_err(|_| CellError::new(ErrorCode::NotFound, "no such fragment"))?;
        let body: Value = crate::json_body(body)?;
        let text = |k: &str| body[k].as_str().unwrap_or("");
        self.assigned_to(text("computer"))?;
        if self.meta(MetaKey::AgentIdentity)?.as_deref() != Some(text("identity")) {
            return Err(CellError::new(ErrorCode::Forbidden, "that computer's row is another agent's: this fragment was made again since"));
        }
        let held = match self.meta(MetaKey::Grants)? {
            Some(stored) => Some(Grants::read(&stored).map_err(|e| CellError::host(format!("its stored grants: {e}")))?),
            None => None,
        };
        let taken = match &body["taken"] {
            Value::Null => None,
            v => Some(Row::of(v).ok_or_else(|| CellError::invalid("a row is its connections as stored and its accounts"))?),
        };
        let invalid = || CellError::invalid("grants name at most a catalog's providers, sorted and once, each account a Pipes id");
        let change = match (body.get("connections"), body["provider"].as_str()) {
            (None, None) => None,
            (Some(Value::Null), _) => Some(Change::Connections(None)),
            (Some(Value::Array(list)), _) => Some(Change::Connections(Some(list.iter().map(|p| p.as_str().map(str::to_string)).collect::<Option<Vec<_>>>().ok_or_else(invalid)?))),
            (None, Some(provider)) => Some(Change::Account { provider: provider.to_string(), account: body["account"].as_str().map(str::to_string) }),
            _ => return Err(invalid()),
        };
        let changed = change.is_some();
        if changed && text("owner") != self.must(MetaKey::Owner)? {
            return Err(CellError::new(ErrorCode::Forbidden, "only the agent's owner, in person, changes what it may use"));
        }
        let grants = answered(held.as_ref(), taken.as_ref(), change).map_err(|e| CellError::invalid(format!("{e}: {}", invalid().message)))?;
        // a row taken is held from then on, the default too: none taken again
        let keep = match &held {
            Some(held) => *held != grants,
            None => taken.is_some() || changed,
        };
        if keep {
            self.set_meta(MetaKey::Grants, &grants.stored())?;
            let by = if changed { "changed by its owner" } else { "taken from its computer's row" };
            self.event("agent.grants", &format!("what it may use of its owner's, {by}"), json!({ "grants": grants, "moved": !changed }));
        }
        Ok(grants)
    }

    /// `computer/…`: the internal calls a computer's routes and egress make.
    pub(crate) async fn runs_on(&self, route: &str, body: &[u8]) -> CellResult<Value> {
        self.created()?;
        match route {
            "assign" => {
                let b: Assign = crate::json_body(body)?;
                if self.must(MetaKey::Owner)? != b.owner {
                    return Err(CellError::new(ErrorCode::Forbidden, "only the agent fragment's owner runs it on their computer"));
                }
                let identity = match self.meta(MetaKey::AgentIdentity)? {
                    Some(id) => id,
                    None => {
                        let npub = self.must(MetaKey::Npub)?;
                        let key = npub::parse(&npub).ok_or_else(|| CellError::host("the fragment's npub does not decode"))?;
                        // the agent is named for its fragment (a page shows it so: `__people`)
                        let register = calls::RegisterAgent { owner: By::Identity(b.owner.clone()), key, fragment: Some(self.name()?) };
                        let agent = crate::ask_registry(&self.env, &register).await?;
                        self.set_meta(MetaKey::AgentIdentity, &agent.id)?;
                        agent.id
                    }
                };
                self.set_meta(MetaKey::Computer, &b.computer)?;
                // the agent keeps its own state here (its SOUL.md, memories,
                // skills, routines): it is an editor of its own fragment
                if self.member_role(&identity)?.is_none() {
                    let as_owner = self.as_owner()?;
                    self.set_member(&as_owner, &identity, fragment_proto::SetRole { role: Role::Editor }).await?;
                }
                self.event("computer.assigned", &format!("runs on {}", b.computer), json!({ "computer": b.computer, "identity": identity }));
                Ok(json!({ "identity": identity }))
            }
            "unassign" => {
                let b: Assign = crate::json_body(body)?;
                if self.must(MetaKey::Owner)? != b.owner {
                    return Err(CellError::new(ErrorCode::Forbidden, "only the agent fragment's owner moves it"));
                }
                if self.meta(MetaKey::Computer)?.as_deref() == Some(b.computer.as_str()) {
                    self.del_meta(MetaKey::Computer)?;
                    self.event("computer.unassigned", &format!("no longer runs on {}", b.computer), json!({ "computer": b.computer }));
                }
                Ok(json!({ "ok": true }))
            }
            "identity" => {
                let body: Value = crate::json_body(body)?;
                let computer = body["computer"].as_str().unwrap_or("");
                self.assigned_to(computer)?;
                Ok(json!({ "identity": self.must(MetaKey::AgentIdentity)? }))
            }
            "sign" => {
                let b: Sign = crate::json_body(body)?;
                self.assigned_to(&b.computer)?;
                let header = self.own_keys().await?.header_for_payload(&b.method, &b.url, b.payload.as_deref(), crate::js::now_ms() / 1000);
                Ok(json!({ "header": header }))
            }
            // an agent's door (fragment_core::door): its owner's ticket, and
            // the guest's redeeming of a link's (a relayed record's check is
            // `door_check`). Their bodies are read field by field, with no
            // derive: the cell's wasm budget (xtask/src/build.rs)
            "door-ticket" => {
                let body: Value = crate::json_body(body)?;
                let text = |k: &str| body[k].as_str().unwrap_or("");
                if self.must(MetaKey::Owner)? != text("owner") {
                    return Err(CellError::new(ErrorCode::Forbidden, "only the agent's owner opens or closes its door"));
                }
                self.assigned_to(text("computer"))?;
                // an adoption (FIN-463: its owner keeps their SimpleX chat with
                // their old agent) opens the door as a link does
                let act = match text("act") {
                    "link" => door::Act::Link,
                    "unlink" => door::Act::Unlink,
                    "adopt" => door::Act::Adopt,
                    _ => return Err(CellError::invalid("act: link, unlink or adopt")),
                };
                self.door_allowed(text("door"), if act.opens() { Action::Link } else { Action::Unlink })?;
                // a door is reach: none while the agent's link opens it
                // (reach.rs); an unlink always closes
                if act.opens() {
                    self.reach_while_private()?;
                }
                let nonce = hex::encode(js::random_bytes::<16>());
                let ticket = door::Ticket { door: text("door").into(), act, agent: self.name()?, computer: text("computer").into(), nonce, exp: js::now_ms() + door::TICKET_MS, sig: String::new() };
                assert!(ticket.well_formed(), "a ticket the platform makes is well formed");
                match act {
                    door::Act::Link | door::Act::Adopt => self.set_meta(MetaKey::DoorTicket, &format!("{} {} {}", ticket.door, ticket.nonce, ticket.exp))?,
                    // closed the moment its owner asks: its door signs nothing more
                    door::Act::Unlink => {
                        self.del_meta(MetaKey::DoorTicket)?;
                        if self.meta(MetaKey::Door)?.is_some() {
                            self.del_meta(MetaKey::Door)?;
                            self.event("door.closed", "its door closed by its owner", json!({ "door": ticket.door }));
                            self.sharing_changed()?;
                        }
                    }
                }
                let sig = self.own_keys().await?.sign_digest(&ticket.digest());
                Ok(json!({ "door": ticket.door, "act": act.as_str(), "agent": ticket.agent, "computer": ticket.computer, "nonce": ticket.nonce, "exp": ticket.exp, "sig": sig }))
            }
            "door-key" => {
                let body: Value = crate::json_body(body)?;
                let text = |k: &str| body[k].as_str().unwrap_or("");
                self.assigned_to(text("computer"))?;
                let (door, key) = (text("door"), text("key"));
                self.door_allowed(door, Action::Redeem)?;
                if !npub::is_hex_key(key) {
                    return Err(CellError::invalid("a door's key is 64 hex"));
                }
                // its owner's latest link ticket, once, while it holds
                let pending = self.meta(MetaKey::DoorTicket)?.unwrap_or_default();
                let mut parts = pending.splitn(3, ' ');
                let (was, nonce, exp) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().and_then(|e| e.parse::<i64>().ok()).unwrap_or(0));
                if was != door || nonce.is_empty() || nonce != text("nonce") || exp < js::now_ms() {
                    return Err(CellError::new(ErrorCode::Forbidden, "that ticket is not the link its owner started last, or it is past its time: they start one from Settings"));
                }
                // opened by its link since its ticket was made: no door (reach.rs)
                self.reach_while_private()?;
                self.del_meta(MetaKey::DoorTicket)?;
                self.set_meta(MetaKey::Door, &format!("{door} {key}"))?;
                self.sharing_changed()?;
                self.event("door.opened", "its door opened by its owner", json!({ "door": door, "key": key }));
                Ok(json!({ "door": door, "key": key }))
            }
            "subscribe" => {
                let b: Subscribe = crate::json_body(body)?;
                if !valid_channel_name(&b.channel) {
                    return Err(CellError::invalid("a channel name must match ^[a-z][a-z0-9_-]{0,63}$"));
                }
                let Some(role) = self.member_role(&b.identity)? else {
                    return Err(CellError::new(ErrorCode::Forbidden, "only a member subscribes to a channel"));
                };
                if role < self.channel_read_role(&b.channel)? {
                    return Err(CellError::new(ErrorCode::Forbidden, format!("that agent's role may not read {}", b.channel)));
                }
                let id = self.add_subscription(&b.identity, &b.channel, &wake_url(&b.computer))?;
                Ok(json!({ "id": id, "channel": b.channel, "wake": true }))
            }
            r => Err(CellError::new(ErrorCode::NotFound, format!("no route computer/{r}"))),
        }
    }

    fn created(&self) -> CellResult<()> {
        self.must(MetaKey::CreatedAt).map(drop).map_err(|_| CellError::new(ErrorCode::NotFound, "no such fragment"))
    }

    /// A relayed record's check (`DoorCheck`).
    pub(crate) fn door_check(&self, c: DoorCheck) -> CellResult<()> {
        self.created()?;
        let refused = |why: &str| CellError::new(ErrorCode::Forbidden, format!("a relayed record {why}"));
        if self.meta(MetaKey::AgentIdentity)?.as_deref() != Some(c.identity.as_str()) {
            return Err(refused("is posted by the agent whose door it came through"));
        }
        self.door_allowed(&c.door, Action::Check)?;
        let open = self.meta(MetaKey::Door)?.unwrap_or_default();
        let Some(key) = open.strip_prefix(c.door.as_str()).and_then(|k| k.strip_prefix(' ')) else {
            return Err(refused("came through a door its owner has not opened, or closed"));
        };
        let mut digest = [0u8; 32];
        hex::decode_to_slice(&c.digest, &mut digest).map_err(|_| CellError::invalid("a digest is 64 hex"))?;
        if !fragment_nip98::verify_digest(key, &digest, &c.sig) {
            return Err(refused("is signed by its door's key, and this one is not"));
        }
        Ok(())
    }

    /// A connector of its owner's bound as one of its doors (`DoorActor`).
    pub(crate) fn door_actor(&self, a: DoorActor) -> CellResult<AgentIdentity> {
        self.created()?;
        if self.must(MetaKey::Owner)? != a.owner {
            return Err(CellError::new(ErrorCode::Forbidden, "only the agent's owner binds a connector as its door"));
        }
        let identity = self.meta(MetaKey::AgentIdentity)?.ok_or_else(|| CellError::new(ErrorCode::Forbidden, "that fragment is no agent that has run on a computer"))?;
        self.door_allowed(&a.door, Action::Link)?;
        self.event("door.hosted", &format!("its {} door is {}, bound by its owner", a.door, a.actor), json!({ "door": a.door, "actor": a.actor }));
        Ok(AgentIdentity { identity })
    }

    /// Its face (fragment/calls.rs `FaceOf`).
    pub(crate) fn face_of(&self) -> CellResult<Named> {
        self.created()?;
        Ok(self.meta(MetaKey::Face)?.and_then(|f| crate::json_text(&f).ok()).unwrap_or_default())
    }

    /// Its reach (fragment/calls.rs `ReachOf`).
    pub(crate) fn reach_of(&self) -> CellResult<Reach> {
        self.created()?;
        Ok(Reach { kind: self.kind()?, reach: self.reaches()?, beyond: self.sharing_counts()?.beyond() })
    }

    /// The finite.computer move's routine (`Routine`), with no chat,
    /// as a join's notice is; nothing when it declares no postable `tasks`.
    pub(crate) async fn move_routine(&self, r: Routine) -> CellResult<()> {
        if self.declared_channel(TASKS)?.is_none_or(|d| d.post.is_none()) {
            return Ok(());
        }
        let routine = json!({ "kind": "routine", "text": r.text });
        self.append_own(TASKS, &routine, &r.key).await?;
        Ok(())
    }

    /// Its agent was added to a fragment (`AgentJoined`, from that
    /// fragment's outbox): while it runs on a computer, its `tasks` say so
    /// (once by the membership: a retry posts nothing twice), so a guest
    /// that is awake lists its fragments again, and the computer is named
    /// for its caller to wake (on its own: a member's change never waits
    /// for a start). An agent that runs on none, or is no longer this
    /// fragment's (made again since), has nothing to tell; nor does its
    /// own fragment, which it is made an editor of as it is assigned
    /// (`assign`), before its computer holds it: its home, never a join.
    pub(crate) async fn agent_joined(&self, j: AgentJoined) -> CellResult<JoinedAnswer> {
        self.created()?;
        if !npub::is_identity(&j.identity) || !valid_fragment_name(&j.fragment) || j.at <= 0 {
            return Err(CellError::invalid("a join names an identity, a fragment and when it joined"));
        }
        let own = j.fragment == self.name()?;
        let computer = match !own && self.meta(MetaKey::AgentIdentity)?.as_deref() == Some(j.identity.as_str()) {
            true => self.meta(MetaKey::Computer)?,
            false => None,
        };
        let Some(computer) = computer else { return Ok(JoinedAnswer { computer: None, posted: false }) };
        if self.declared_channel(TASKS)?.is_none_or(|d| d.post.is_none()) {
            return Ok(JoinedAnswer { computer: Some(computer), posted: false });
        }
        let notice = json!({ "kind": "joined", "fragment": j.fragment });
        let (record, _) = self.append_own(TASKS, &notice, &format!("joined:{}:{}", j.fragment, j.at)).await?;
        assert_eq!(record.channel, TASKS, "the notice is on tasks");
        Ok(JoinedAnswer { computer: Some(computer), posted: true })
    }

    /// An agent became a member here (`set_member`, `join`): its computer
    /// is to hear so, in the turn of the membership's write (the row is
    /// sent by `flush_joined`). A deployment without computers tells no one.
    pub(crate) fn agent_added(&self, identity: &str, owner: Option<&str>, added_at: i64) -> CellResult<()> {
        if self.cfg.computer_image.is_none() {
            return Ok(());
        }
        let owner = owner.ok_or_else(|| CellError::host(format!("agent {identity} has no owner")))?;
        assert!(npub::is_identity(identity), "a notice names an agent's identity, not {identity:?}");
        assert!(added_at > 0, "a notice names when its agent joined");
        self.exec(
            "INSERT INTO joined_outbox (principal, owner, added_at, attempts, next_at) VALUES (?, ?, ?, 0, ?)
             ON CONFLICT (principal) DO UPDATE SET owner = excluded.owner, added_at = excluded.added_at, attempts = 0, next_at = excluded.next_at",
            vec![identity.into(), owner.into(), SqlStorageValue::Integer(added_at), SqlStorageValue::Integer(js::now_ms())],
        )
    }

    /// A member that left takes its notice with it.
    pub(crate) fn agent_removed(&self, identity: &str) -> CellResult<()> {
        self.exec("DELETE FROM joined_outbox WHERE principal = ?", vec![identity.into()])
    }

    /// When the joined outbox next has a row due (for the alarm): none
    /// while its joins wait (`Stage::joins_wait`), whose end tells them:
    /// the seed that installs it (publish.rs `seed`), else the alarm's pass
    /// after its seed.
    pub(crate) fn joined_due_at(&self) -> CellResult<Option<i64>> {
        if self.stage()?.joins_wait() {
            return Ok(None);
        }
        Ok(self.rows("SELECT MIN(next_at) AS at FROM joined_outbox", vec![])?.first().and_then(|r| r["at"].as_i64()))
    }

    /// Sends the due notices, oldest first: each agent's own fragment (as
    /// the registry names it, one ask a flush) posts `joined` and names the
    /// computer it runs on, which is then woken on its own, so the member's
    /// change never waits for a start (as a pre-wake does not). A row goes
    /// once the agent answered, or for good when trying again cannot help
    /// (the agent runs nowhere, or is gone); a passing failure waits,
    /// doubling, and after its last try goes with an event (never
    /// silently).
    pub(crate) async fn flush_joined(&self) {
        let Ok(name) = self.name() else { return };
        // a template still to land: told once it is in (`Stage::joins_wait`)
        if self.stage().map_or(true, |s| s.joins_wait()) {
            return;
        }
        let Ok(due) = self.typed::<Notice>(
            "SELECT principal, added_at, attempts FROM joined_outbox WHERE next_at <= ? ORDER BY next_at LIMIT ?",
            vec![SqlStorageValue::Integer(js::now_ms()), SqlStorageValue::Integer(JOINED_FLUSH_MAX)],
        ) else {
            return;
        };
        if due.is_empty() {
            return;
        }
        // each agent's fragment: the computer is found from the agent, never from its owner
        let ids: Vec<String> = due.iter().map(|n| n.principal.clone()).collect();
        let profiles = crate::ask_registry(&self.env, &calls::Profiles { ids, emails_of: vec![] }).await.map(|a| a.profiles);
        let mut waiting = false;
        for n in due {
            let told = match &profiles {
                Err(e) => Err(CellError::new(e.code, e.message.clone())),
                Ok(profiles) => match profiles.get(&n.principal).and_then(|p| p.fragment.clone()) {
                    // made from no agent fragment, or that fragment gone: it runs nowhere
                    None => Ok((None, JoinedAnswer { computer: None, posted: false })),
                    Some(agent) => {
                        let joined = AgentJoined { identity: n.principal.clone(), fragment: name.clone(), at: n.added_at };
                        crate::fragment::call(&self.env, &agent, &joined).await.map(|a| (Some(agent), a))
                    }
                },
            };
            // only this membership's row: a newer one (left and added again) stays
            let done = |cell: &FragmentCell| {
                let _ = cell.exec("DELETE FROM joined_outbox WHERE principal = ? AND added_at = ?", vec![n.principal.as_str().into(), SqlStorageValue::Integer(n.added_at)]);
            };
            match told {
                Ok((agent, JoinedAnswer { computer: Some(computer), posted })) => {
                    done(self);
                    let heard = if posted { "its tasks say so" } else { "its fragment has no tasks to say so on" };
                    self.event(
                        "agent.told",
                        &format!("{}'s computer is told it joined, and woken ({heard})", npub::display(&n.principal)),
                        json!({ "principal": npub::display(&n.principal), "computer": computer, "agent": agent, "posted": posted }),
                    );
                    let env = self.env.clone();
                    worker::wasm_bindgen_futures::spawn_local(async move {
                        // its join's notice, posted just now on its agent's tasks
                        if let Err(e) = crate::computer::ask(&env, &computer, "computer/wake", &json!({ "why": "joined", "at": crate::js::now_ms() })).await {
                            console_log!("{}", json!({ "fragment": "joined-wake", "computer": computer, "error": e.message }));
                        }
                    });
                }
                Ok(_) => done(self),
                Err(e) if !passing(e.code) => {
                    done(self);
                    if e.code != ErrorCode::NotFound {
                        self.event("agent.untold", &format!("{} was not told it joined: {}", npub::display(&n.principal), e.message), json!({ "code": e.code }));
                    }
                }
                Err(e) => {
                    let attempts = n.attempts + 1;
                    if attempts >= JOINED_ATTEMPTS_MAX {
                        done(self);
                        self.event("agent.untold", &format!("{} was not told it joined in {attempts} tries: {}", npub::display(&n.principal), e.message), json!({ "code": e.code }));
                        continue;
                    }
                    waiting = true;
                    let next_at = js::now_ms() + fragment_core::backoff::outbox_retry_ms(attempts);
                    let _ = self.exec(
                        "UPDATE joined_outbox SET attempts = ?, next_at = ? WHERE principal = ? AND added_at = ?",
                        vec![SqlStorageValue::Integer(attempts), SqlStorageValue::Integer(next_at), n.principal.as_str().into(), SqlStorageValue::Integer(n.added_at)],
                    );
                }
            }
        }
        if waiting {
            let _ = self.schedule().await;
        }
    }

    /// The fragment's owner as a caller, for what the platform does on the
    /// owner's behalf here (adding the agent as a member).
    pub(crate) fn as_owner(&self) -> CellResult<crate::fragment::Caller> {
        let owner = self.must(MetaKey::Owner)?;
        let identity = fragment_proto::Identity { id: owner, kind: IdentityKind::Person, owner: None, held: None };
        let signed = crate::routed::Signed::new(identity, None);
        Ok(crate::fragment::Caller { signed: Some(signed), unresolved: None, url: url::Url::parse("https://fragment.internal/").expect("a URL"), site: false })
    }

    /// Its own key, opened from its seal (resealed in place when the seal
    /// moved on).
    async fn own_keys(&self) -> CellResult<fragment_nip98::Keys> {
        let sealed = self.must(MetaKey::FragmentSecret)?;
        let opened = crate::keys::open(&self.env, &self.scope(), &sealed).await?;
        if let Some(fresh) = opened.resealed {
            // only over the value opened (the store's read may yield)
            self.exec("UPDATE meta SET value = ? WHERE key = ? AND value = ?", vec![fresh.into(), MetaKey::FragmentSecret.key().into(), sealed.as_str().into()])?;
        }
        let secret = std::str::from_utf8(&opened.plaintext).map_err(|_| CellError::host("the fragment's key is not text"))?;
        fragment_nip98::Keys::from_secret_hex(secret).ok_or_else(|| CellError::host("the fragment's sealed key is not a nostr key"))
    }

    /// This agent fragment runs on `computer`, or refuses.
    fn assigned_to(&self, computer: &str) -> CellResult<()> {
        if self.meta(MetaKey::Computer)?.as_deref() != Some(computer) {
            return Err(CellError::new(ErrorCode::Forbidden, "this agent does not run on that computer"));
        }
        Ok(())
    }

    /// The computer `principal`'s typing on this fragment pre-wakes: their
    /// own, when its channels wake it (`fragment_core::computer::prewoken`;
    /// at most `SUBS_MAX` rows).
    fn woken_computer(&self, principal: &str) -> CellResult<Option<String>> {
        let rows = self.rows("SELECT principal, url FROM subs WHERE url LIKE 'computer:%' ORDER BY id", vec![])?;
        let subs: Vec<(&str, &str)> = rows.iter().filter_map(|r| Some((r["principal"].as_str()?, r["url"].as_str()?))).collect();
        Ok(fragment_core::computer::prewoken(&subs, principal).map(|u| format!("computer:{}", u.trim_start_matches("computer:"))))
    }

    /// `principal` began typing on a page of this fragment (its socket's
    /// presence, `fragment_core::live::typing_began`): their own computer,
    /// when this fragment's channels wake it, may hear from them soon, so it
    /// starts now, held as any wake is (decision 39), at most every
    /// `PREWAKE_EVERY_MS`. No one else's typing wakes it, and a socket's
    /// opening is no use of it: a tab left open reconnects on its own. The
    /// wake runs on its own, so the socket never waits for a start; a
    /// failed pre-wake costs only the wait.
    pub(crate) fn prewake(&self, principal: &str) {
        let now = crate::js::now_ms();
        let due = self.meta(MetaKey::PrewakeAt).ok().flatten().and_then(|v| v.parse::<i64>().ok()).is_none_or(|at| now >= at);
        if !due {
            return;
        }
        let Ok(Some(c)) = self.woken_computer(principal) else { return };
        let _ = self.set_meta(MetaKey::PrewakeAt, &(now + PREWAKE_EVERY_MS).to_string());
        let env = self.env.clone();
        worker::wasm_bindgen_futures::spawn_local(async move {
            if let Err(e) = crate::computer::ask(&env, &c, "computer/wake", &json!({ "why": "presence" })).await {
                console_log!("{{\"fragment\":\"prewake\",\"computer\":{},\"error\":{}}}", json!(c), json!(e.message));
            }
        });
    }
}
