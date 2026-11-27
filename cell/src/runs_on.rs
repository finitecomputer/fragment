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
//!   guest reads the channel itself. A page opening the fragment, or
//!   anything its presence says, wakes none: a page acts on its own, and
//!   only its person's shell pre-wakes their computer (docs/computers.md).
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
use fragment_core::mandate::{self, Proof};

use crate::fragment::calls::LendOf;
use fragment_core::{connectors::Action, door, npub};
use fragment_proto::{limits, valid_channel_name, valid_fragment_name, ErrorCode, IdentityKind, Lend, Role};
use serde::Deserialize;
use serde_json::{json, Value};
use worker::*;

use crate::error::{CellError, CellResult};
use crate::fragment::calls::{AgentIdentity, AgentJoined, DoorActor, DoorCheck, JoinedAnswer, Named, PlaceTurn, Reach, Routine, TurnsIn};
use crate::fragment::{FragmentCell, MetaKey};
use crate::js;
use crate::registry::calls::{self, By};

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
    /// The fragment it goes to, when it goes to one (its `/api/f/…` or
    /// `/f/…`): where an agent asked by no one proven still works.
    fragment: Option<String>,
}

/// The `life` of a turn its place told (`turns_in`). A row a bridge told
/// before this release (named by its bridge's life) is read too, so a turn
/// running across the deploy stays held, until its bridge ends it or a new
/// bridge life takes over (`turns`): no new one is taken.
const PLACE_OPENED: &str = "";

/// An open turn as its row holds it (`turns`, Fragment step 15).
#[derive(Deserialize)]
struct TurnRow {
    place: String,
    asker: String,
    mandate: Option<String>,
}

/// A turn's row read as `mandate::proof` reads one: its mandate only while
/// that holds (a day from its words), the turn itself while its row does.
fn turn_of(r: &TurnRow, now: i64) -> mandate::Turn<'_> {
    let mandate = r.mandate.as_deref().and_then(|text| Some(mandate::Held { text, until: mandate::until_of(text)?, place: mandate::place_of(text)? })).filter(|m| m.until >= now);
    mandate::Turn { place: &r.place, asker: &r.asker, mandate }
}

/// `url` without its `for`.
fn without_for(url: &url::Url) -> String {
    let mut out = url.clone();
    let kept: Vec<(String, String)> = url.query_pairs().filter(|(k, _)| k != "for").map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    if kept.is_empty() {
        out.set_query(None);
    } else {
        out.query_pairs_mut().clear().extend_pairs(kept);
    }
    out.to_string()
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
    pub(crate) async fn grants(&self, body: &[u8]) -> CellResult<Grants> {
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
        // narrowed, at this use, by someone else's open ask (FIN-720)
        Ok(Grants { lend: self.lent_now().await?, ..grants })
    }

    /// Its open turns' rows, as its places told them (`turns_in`), and a
    /// bridge before them (`PLACE_OPENED`), not yet past
    /// `limits::TURN_HELD_MS`.
    fn turn_rows(&self, now: i64) -> CellResult<Vec<TurnRow>> {
        // a place's row holds until its `until`; a bridge's, past its
        // mandate's day too (held unproven), as it always did
        self.typed(
            "SELECT place, asker, mandate FROM turns WHERE at >= ? AND (until > ? OR life != ?) ORDER BY turn LIMIT ?",
            vec![SqlStorageValue::Integer(now - limits::TURN_HELD_MS), SqlStorageValue::Integer(now), PLACE_OPENED.into(), SqlStorageValue::Integer(limits::TURNS_HELD_MAX)],
        )
    }

    /// The turns a place holds open for this agent (`TurnsIn`): the one
    /// word on them, its place's own (FIN-720), replacing what it said
    /// before; an older version than the one held changes nothing. A turn
    /// of no one's (its asker empty: words no one's word proves,
    /// `mandate::Asked::NoOne`), and its rows and every other place's past
    /// `TURNS_HELD_MAX`, it holds as one ask no one proves: the agent acts
    /// as itself there alone (`Proof::Itself`), narrower than any, never
    /// refused.
    pub(crate) fn turns_in(&self, t: TurnsIn) -> CellResult<()> {
        if self.meta(MetaKey::AgentIdentity)?.as_deref() != Some(t.agent.as_str()) {
            return Err(CellError::new(ErrorCode::NotFound, "no such agent here: this fragment was made again since"));
        }
        if self.cfg.test_hooks && self.meta(MetaKey::TestRefuseTurns)?.is_some() {
            return Err(CellError::new(ErrorCode::HostFailed, "refused by a test hook (`refuse-turns`)"));
        }
        let asker = |p: &PlaceTurn| npub::is_identity(&p.asker) || (p.asker.is_empty() && p.mandate.is_none());
        let well_formed = |p: &PlaceTurn| fragment_proto::valid_op_id(&p.turn) && asker(p) && p.mandate.as_ref().is_none_or(|m| m.len() <= limits::MANDATE_MAX_BYTES);
        if !valid_fragment_name(&t.place) || t.version <= 0 || !t.turns.iter().all(well_formed) {
            return Err(CellError::invalid("a place's turns are its name, their version, and each its id, asker and mandate"));
        }
        let held = self.rows("SELECT version FROM turn_places WHERE place = ?", vec![t.place.as_str().into()])?.first().and_then(|r| r["version"].as_i64());
        if held.is_some_and(|v| v >= t.version) {
            return Ok(());
        }
        let now = js::now_ms();
        self.exec(
            "DELETE FROM turns WHERE at < ? OR (until <= ? AND life = ?)",
            vec![SqlStorageValue::Integer(now - limits::TURN_HELD_MS), SqlStorageValue::Integer(now), PLACE_OPENED.into()],
        )?;
        self.exec("DELETE FROM turns WHERE place = ? AND life = ?", vec![t.place.as_str().into(), PLACE_OPENED.into()])?;
        let others = self.count_of("SELECT COUNT(*) AS n FROM turns", vec![])?;
        let mut turns = t.turns;
        if others + turns.len() as u64 > limits::TURNS_HELD_MAX as u64 {
            let until = turns.iter().map(|p| p.until).max().unwrap_or(now);
            turns = vec![PlaceTurn { turn: turns[0].turn.clone(), asker: String::new(), mandate: None, until }];
            self.event("turns.crowded", &format!("{} holds more open asks than it may: it acts as itself in {} until they end", npub::display(&t.agent), t.place), json!({ "place": t.place, "others": others }));
        }
        for p in turns {
            self.exec(
                "INSERT INTO turns (turn, place, asker, mandate, until, life, at) VALUES (?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT (turn) DO UPDATE SET place = excluded.place, asker = excluded.asker, mandate = excluded.mandate, until = excluded.until, life = excluded.life, at = excluded.at",
                vec![p.turn.as_str().into(), t.place.as_str().into(), p.asker.as_str().into(), p.mandate.map_or(SqlStorageValue::Null, Into::into), SqlStorageValue::Integer(p.until), PLACE_OPENED.into(), SqlStorageValue::Integer(now)],
            )?;
        }
        self.exec(
            "INSERT INTO turn_places (place, version) VALUES (?, ?) ON CONFLICT (place) DO UPDATE SET version = excluded.version",
            vec![t.place.as_str().into(), SqlStorageValue::Integer(t.version)],
        )
    }

    /// `computer/turns`, as a bridge tells its turns: whose ask a turn is
    /// is its place's to say (`turns_in`), never its computer's (FIN-720),
    /// so nothing it opens is taken. Its end of a turn it told before this
    /// release, or a new life's taking of them, lets those go
    /// (`PLACE_OPENED`), and no place's. The route goes once no computer
    /// runs a bridge that calls it (FIN-615 PR 8).
    fn turns(&self, body: &Value) -> CellResult<Value> {
        match body["op"].as_str() {
            Some("close") => self.exec("DELETE FROM turns WHERE turn = ? AND life != ?", vec![body["turn"].as_str().unwrap_or("").into(), PLACE_OPENED.into()])?,
            Some("take") => self.exec("DELETE FROM turns WHERE life != ?", vec![PLACE_OPENED.into()])?,
            _ => {}
        }
        Ok(json!({ "ok": true }))
    }

    /// The request its computer's egress signs for this agent (FIN-720):
    /// its presence in its chats (`mandate::presence`) as it came, its own.
    /// Any other call is work, held to whose ask is open over the turns its
    /// places opened (`mandate::proof`), signed for its owner or as itself:
    /// none open, or only its owner's own, as it came (for its owner, their
    /// newest mandate in place of `for`); one other person's, as them,
    /// their mandate in place of `for`; no single one proven, the agent
    /// itself in those turns' places alone, and nowhere else. Answers its
    /// URL and the mandate it carries.
    fn asked(&self, method: &str, at: &str, fragment: Option<&str>) -> CellResult<(String, Option<String>)> {
        let url = url::Url::parse(at).map_err(|e| CellError::invalid(format!("url: {e}")))?;
        // a fragment's site is signed at its own origin (its app's `/api/…`
        // too): its path as the guest asked for it, `/f/<fragment>/…`, is
        // what the rules read
        let site = fragment.filter(|_| !url.host_str().is_some_and(|h| self.cfg.is_platform_host(h)));
        let path = site.map_or_else(|| url.path().to_string(), |f| format!("/f/{f}{}", url.path()));
        let owner = self.must(MetaKey::Owner)?;
        let for_owner = url.query_pairs().any(|(k, v)| k == "for" && npub::display(&v) == npub::display(&owner));
        if !for_owner && mandate::presence(method, &path, &self.name()?) {
            return Ok((at.to_string(), None));
        }
        let now = js::now_ms();
        let rows = self.turn_rows(now)?;
        let turns: Vec<mandate::Turn> = rows.iter().map(|r| turn_of(r, now)).collect();
        let proof = mandate::proof(&npub::display(&owner), &turns);
        // an invite waiting on its owner's email is theirs to answer: the
        // platform's route takes `for` and no mandate (the registry's
        // `invitee_by`), so its owner's own ask keeps `for` there, and no
        // one else's goes (a fragment's own route of that name is its app's)
        if for_owner && fragment.is_none() && (path == "/api/invites" || path.starts_with("/api/invites/")) {
            return match proof {
                Proof::Bare | Proof::Owner(_) => Ok((at.to_string(), None)),
                Proof::Asker { .. } | Proof::Itself(_) => Err(CellError::new(ErrorCode::Forbidden, "an agent answers its owner's invites for its owner alone, never on someone else's ask")),
            };
        }
        // a fragment's site (`/f/…`) takes no one's word but its signer's
        // (`for` and a mandate are not honored there, lib.rs): on someone
        // else's ask it is reached only as the agent itself, in their
        // turns' places
        if site.is_some() {
            return match proof {
                Proof::Bare | Proof::Owner(_) => Ok((at.to_string(), None)),
                Proof::Itself(places) if fragment.is_some_and(|f| places.contains(&f)) => Ok((at.to_string(), None)),
                Proof::Asker { at: places, .. } if fragment.is_some_and(|f| places.iter().any(|(p, _)| *p == f)) => Ok((at.to_string(), None)),
                Proof::Asker { .. } | Proof::Itself(_) => Err(CellError::new(
                    ErrorCode::Forbidden,
                    "on someone else's ask, an agent reaches a fragment's site only in the chats they asked it in: call its API (/api/f/…) to act as them",
                )),
            };
        }
        match proof {
            Proof::Bare => Ok((at.to_string(), None)),
            Proof::Owner(m) if for_owner => Ok((without_for(&url), Some(m.text.to_string()))),
            Proof::Owner(_) => Ok((at.to_string(), None)),
            Proof::Asker { held: m, .. } => Ok((without_for(&url), Some(m.text.to_string()))),
            Proof::Itself(places) if fragment.is_some_and(|f| places.contains(&f)) => Ok((without_for(&url), None)),
            Proof::Itself(_) => Err(CellError::new(
                ErrorCode::Forbidden,
                "more than one other person has asked this agent, or one without proof: it acts only in their chats, as itself, until those turns end",
            )),
        }
    }

    /// What of its owner's its open turns lend it now: `None` while only
    /// its owner's own ask is open (or none is), everything of theirs; one
    /// other person's, the narrowest of what each place their turns are in
    /// lends it now, read there (`LendOf`: a lend is kept on its member row
    /// alone, so a switch turned off holds from the next use); no one
    /// proven's, nothing. A place that does not answer lends nothing.
    async fn lent_now(&self) -> CellResult<Option<Lend>> {
        let now = js::now_ms();
        let rows = self.turn_rows(now)?;
        let turns: Vec<mandate::Turn> = rows.iter().map(|r| turn_of(r, now)).collect();
        let owner = npub::display(&self.must(MetaKey::Owner)?);
        let at = match mandate::proof(&owner, &turns) {
            Proof::Bare | Proof::Owner(_) => return Ok(None),
            Proof::Itself(_) => return Ok(Some(Lend::NONE)),
            Proof::Asker { at, .. } => at,
        };
        let agent = self.must(MetaKey::AgentIdentity)?;
        // bounded: the places of its open turns, at most `TURNS_HELD_MAX`
        let calls: Vec<(&str, LendOf)> = at.iter().map(|(name, place)| (*name, LendOf { agent: agent.clone(), place: place.to_string() })).collect();
        let asked = calls.iter().map(|(name, call)| crate::fragment::call(&self.env, name, call));
        let lent = futures_util::future::join_all(asked).await.into_iter().fold(Lend::OWNER, |lent, here| {
            let here = here.unwrap_or(Lend::NONE);
            Lend { spend: lent.spend && here.spend, accounts: lent.accounts && here.accounts }
        });
        Ok(Some(lent))
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
                        // made an agent: its owner's alone from now (alone.rs),
                        // refused before the registry is asked and again in
                        // the write's own turn
                        self.alone_while_shared()?;
                        let npub = self.must(MetaKey::Npub)?;
                        let key = npub::parse(&npub).ok_or_else(|| CellError::host("the fragment's npub does not decode"))?;
                        // the agent is named for its fragment (a page shows it so: `__people`)
                        let register = calls::RegisterAgent { owner: By::Identity(b.owner.clone()), key, fragment: Some(self.name()?) };
                        let agent = crate::ask_registry(&self.env, &register).await?;
                        self.alone_while_shared()?;
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
                let (url, mandate) = self.asked(&b.method, &b.url, b.fragment.as_deref())?;
                let header = self.own_keys().await?.header_for_payload(&b.method, &url, b.payload.as_deref(), crate::js::now_ms() / 1000);
                Ok(json!({ "header": header, "url": url, "mandate": mandate }))
            }
            "turns" => {
                let body: Value = crate::json_body(body)?;
                self.assigned_to(body["computer"].as_str().unwrap_or(""))?;
                self.turns(&body)
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
                self.door_ticket(text("door"), act, text("computer")).await
            }
            // the operators' move of its computer's SimpleX link into its
            // connector's door (docs/bridge.md, "SimpleX"), its Computer
            // object's alone to ask: a handover's ticket while its computer's
            // link holds its door open, the way back's (it opens again, as a
            // link's does), and its door closed once its connector's door
            // holds the link
            "simplex-move" => {
                let body: Value = crate::json_body(body)?;
                let text = |k: &str| body[k].as_str().unwrap_or("");
                self.assigned_to(text("computer"))?;
                let act = match text("act") {
                    "handover" => door::Act::Handover,
                    "back" => door::Act::Back,
                    "closed" => return Ok(json!({ "closed": self.close_door(door::SIMPLEX, "its SimpleX link moved into its own chat's door")? })),
                    _ => return Err(CellError::invalid("act: handover, back or closed")),
                };
                self.door_allowed(door::SIMPLEX, if act.opens() { Action::Link } else { Action::Unlink })?;
                if act.opens() {
                    self.reach_while_private()?;
                }
                self.door_ticket(door::SIMPLEX, act, text("computer")).await
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
        // its sharing read once: its reach, its link, and past its owner
        let sharing = self.sharing_counts()?;
        let shared = self.alone()?.is_some() && crate::alone::past_owner(&sharing);
        Ok(Reach { kind: self.kind()?, reach: sharing.reach, beyond: sharing.beyond(), shared })
    }

    /// The finite.computer move's routine (`Routine`), with no chat,
    /// as a join's notice is; nothing when it declares no postable `tasks`.
    pub(crate) async fn move_routine(&self, r: Routine) -> CellResult<()> {
        if self.declared_channel(TASKS)?.is_none_or(|d| d.post.is_none()) {
            return Ok(());
        }
        let routine = json!({ "kind": "routine", "text": r.text });
        // its owner's ask: they said yes to the move's offer
        let owner = self.must(MetaKey::Owner)?;
        self.append_own(TASKS, &routine, &r.key, Some(&owner)).await?;
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
        let (record, _) = self.append_own(TASKS, &notice, &format!("joined:{}:{}", j.fragment, j.at), None).await?;
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

    /// A member that left takes its notice with it, and its asks here
    /// close (asks.rs).
    pub(crate) fn agent_removed(&self, identity: &str) -> CellResult<()> {
        self.exec("DELETE FROM joined_outbox WHERE principal = ?", vec![identity.into()])?;
        self.asks_let_go(identity)
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
    /// change never waits for a start. A row goes
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
    /// A ticket for `act` on its `door`, for `computer`, signed as this
    /// agent: a link's, an adoption's and a way back's open the door for
    /// the key redeemed with it; an unlink's closes it at once; a
    /// handover's leaves it open until its connector's door holds the link.
    async fn door_ticket(&self, door: &str, act: door::Act, computer: &str) -> CellResult<Value> {
        let nonce = hex::encode(js::random_bytes::<16>());
        let ticket = door::Ticket { door: door.into(), act, agent: self.name()?, computer: computer.into(), nonce, exp: js::now_ms() + door::TICKET_MS, sig: String::new() };
        assert!(ticket.well_formed(), "a ticket the platform makes is well formed");
        match act {
            door::Act::Link | door::Act::Adopt | door::Act::Back => self.set_meta(MetaKey::DoorTicket, &format!("{} {} {}", ticket.door, ticket.nonce, ticket.exp))?,
            // closed the moment its owner asks: its door signs nothing more
            door::Act::Unlink => {
                self.close_door(&ticket.door, "its door closed by its owner")?;
            }
            door::Act::Handover => {}
        }
        let sig = self.own_keys().await?.sign_digest(&ticket.digest());
        Ok(json!({ "door": ticket.door, "act": act.as_str(), "agent": ticket.agent, "computer": ticket.computer, "nonce": ticket.nonce, "exp": ticket.exp, "sig": sig }))
    }

    /// Its door closed: no ticket pending, no key, so nothing its computer
    /// signs relays its owner again. Whether it was open.
    fn close_door(&self, door: &str, why: &str) -> CellResult<bool> {
        self.del_meta(MetaKey::DoorTicket)?;
        if self.meta(MetaKey::Door)?.is_none() {
            return Ok(false);
        }
        self.del_meta(MetaKey::Door)?;
        self.event("door.closed", why, json!({ "door": door }));
        self.sharing_changed()?;
        Ok(true)
    }

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
}
