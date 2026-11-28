//! A place's side of its agents' turns (FIN-720; docs/api.md, "Who asked:
//! mandates"). Whose ask an agent's turn is is the word of the place it
//! was asked in, from its own records, never its agent's computer's: an
//! agent talked into it can neither leave someone's ask out nor offer an
//! earlier one in its place.
//!
//! - **An ask handed** (`asks_handed`): words on `chat` that start a turn
//!   (`mandate::starts_turn`), someone's (`mandate::asked_of`: their own;
//!   an agent's to another of its owner's agents, said on their owner's
//!   own proven ask, that ask carried on, and no one else's: ask_of), to
//!   an agent member there since before
//!   they were said (`mandate::heard_by`: as its bridge hears them), open
//!   that agent's ask of them
//!   as they first reach it: sent on its socket that follows `chat` (in
//!   the append's own step, `asks_noted`), or read forward past its newest
//!   (a page after its cursor, a subscribe's catch-up). Each is handed
//!   once; a read back hands nothing. Words its computer has not been
//!   given yet (it sleeps) hold it to nothing.
//! - **A claim** (`claim_noted`, once its append is taken): the agent's
//!   `turn.start` on `work` for a record here opens that record's ask
//!   again, as the record says whose it is, and names the turn; the other
//!   agents' asks of it that they never claimed close.
//! - **An end** (in the append's step): its `turn.end` closes that turn's
//!   ask and every one it never claimed, each held until its ask's hold
//!   passed (`limits::TURN_HOLD_MS`); a turn it claims later opens again.
//! - **Unclaimed** (FIN-825): an ask its agent hasn't claimed within
//!   `limits::ASK_UNCLAIMED_MS` of its hand-off lets go (its `until`, set
//!   as it opens), whatever its bridge did with the words; its claim,
//!   later, opens it again.
//! - **Told**: each change is a new version of the agent's asks here, and
//!   its agent fragment is told all of them that are open, one turn an
//!   asker, each with the place's mandate for it (`calls::TurnsIn`), which
//!   its egress reads (runs_on.rs `asked`). One not taken is told again
//!   from the alarm (`flush_asks`), doubling its wait.
//! - **Fail closed, never lost**: someone else's ask reaches its agent
//!   only once its agent fragment took it. Its sockets hold what they are
//!   sent meanwhile (live.rs), its reads wait (`asks_settled`: 502 until
//!   then), and so does its claim. Its owner's own asks wait for nothing:
//!   they narrow nothing (decision 64), and are told after.
//! - **Let go**: an agent removed from the place has its asks here closed.

use fragment_core::mandate::{self, Asked, Mandate};
use fragment_core::npub;
use fragment_proto::{limits, valid_op_id, Author, ChannelRecord, ErrorCode};
use serde::Deserialize;
use serde_json::{json, Value};
use worker::*;

use crate::error::{CellError, CellResult};
use crate::fragment::calls::{PlaceTurn, TurnsIn};
use crate::fragment::{FragmentCell, MetaKey};
use crate::js;

/// The channel people ask agents on (docs/chat-records.md).
const CHAT: &str = "chat";
/// The channel an agent claims and ends its turns on.
const WORK: &str = "work";
/// Agents one flush tells, the rest at the next.
const ASKS_FLUSH_MAX: i64 = 16;
/// How long an ended ask is kept: as long as its words could still be
/// handed (`MANDATE_LIFE_MS`), so another agent's catch-up knows it was
/// claimed, and a hand-off said in its turn after it ended carries it on.
const ENDED_KEPT_MS: i64 = limits::MANDATE_LIFE_MS;

#[derive(Deserialize)]
struct Member {
    principal: String,
    owner: Option<String>,
    added_at: i64,
}

/// An agent's asks here, as told to its agent fragment (`asks_told`).
#[derive(Deserialize)]
struct Told {
    agent: String,
    fragment: Option<String>,
    version: i64,
    gate: i64,
    told: i64,
    attempts: i64,
}

/// An asker's open asks of an agent, as told (`asks`): its newest, and
/// until when the last of them holds.
#[derive(Deserialize)]
struct Open {
    asker: String,
    seq: i64,
    until: i64,
}

/// One ask's proof: whether there is one, its time, and the ask it
/// carries on (a hand-off's: `records.carried`).
#[derive(Deserialize)]
struct Proved {
    proven: i64,
    at: i64,
    carried: Option<String>,
}

/// Whose ask a record hands an agent (`ask_of`): its asker, whether a
/// mandate proves it, its time, and the ask it carries on (a hand-off's:
/// its owner's own mandate the words were said on).
struct Ask {
    asker: String,
    proven: bool,
    at: i64,
    carried: Option<String>,
}

#[derive(Deserialize)]
struct Claimed {
    asker: String,
    turn: Option<String>,
    until: Option<i64>,
}

#[derive(Deserialize)]
struct Said {
    principal: String,
    author: Option<String>,
    asker: Option<String>,
    at: i64,
    body: String,
}

impl FragmentCell {
    /// In the append's own step (channels.rs `append_asked`): `r` handed
    /// to each agent member whose socket follows its channel
    /// (`asks_handed`), and an agent's end of its turns. Answers the agents
    /// it handed someone else's ask, whose sockets hold what they are sent
    /// until it is told (live.rs).
    pub(crate) fn asks_noted(&self, r: &ChannelRecord) -> CellResult<Vec<String>> {
        if r.channel == WORK && r.author == Some(Author::Agent) {
            self.ended_noted(r)?;
            return Ok(vec![]);
        }
        if r.channel != CHAT || !mandate::starts_turn(r.body.get()) {
            return Ok(vec![]);
        }
        let mut gated = vec![];
        for m in self.agent_members()? {
            let agent = npub::display(&m.principal);
            if self.follows_live(&agent, CHAT) && self.asks_handed(&agent, std::slice::from_ref(r), r.seq - 1)? {
                gated.push(agent);
            }
        }
        Ok(gated)
    }

    /// `records` of `chat` handed to `agent` (a forward read from `from`,
    /// or one sent live): each past the newest it was handed, its words
    /// someone's ask of it (the module doc), opens that ask. Answers
    /// whether one is someone else's, which its agent waits for
    /// (`asks_settled`).
    pub(crate) fn asks_handed(&self, agent: &str, records: &[ChannelRecord], from: i64) -> CellResult<bool> {
        let agent = npub::display(agent);
        if records.iter().all(|r| r.channel != CHAT) {
            return Ok(false);
        }
        let members = self.agent_members()?;
        let Some(m) = members.iter().find(|m| npub::display(&m.principal) == agent) else { return Ok(false) };
        #[derive(Deserialize)]
        struct Handed {
            handed: i64,
        }
        let handed = match self.typed::<Handed>("SELECT handed FROM asks_told WHERE agent = ?", vec![agent.as_str().into()])?.into_iter().next() {
            Some(h) => h.handed,
            // first handed now: what it read before is not handed by this
            None => {
                self.exec(
                    "INSERT INTO asks_told (agent, handed, version, gate, told, attempts, next_at) VALUES (?, ?, 0, 0, 0, 0, 0)",
                    vec![agent.as_str().into(), SqlStorageValue::Integer(from.max(0))],
                )?;
                from.max(0)
            }
        };
        let agents: Vec<String> = members.iter().map(|m| npub::display(&m.principal)).collect();
        let agents: Vec<&str> = agents.iter().map(String::as_str).collect();
        let (mut newest, mut opened, mut others, now) = (handed, false, false, js::now_ms());
        for r in records.iter().filter(|r| r.channel == CHAT && r.seq > handed) {
            newest = newest.max(r.seq);
            // words older than a mandate's life start nothing new (its claim of them opens them all the same)
            let old = r.at.saturating_add(limits::MANDATE_LIFE_MS) < now;
            if old || r.principal == agent || r.at < m.added_at || !mandate::starts_turn(r.body.get()) {
                continue;
            }
            // to it, as its bridge hears whom words are to: words under an
            // agent member's name (its own, or app code's it called) are
            // an agent's, whoever wrote them (engine.rs `addressed`)
            if !mandate::heard_by(r.body.get(), agents.contains(&r.principal.as_str()), &agents).contains(&agent.as_str()) {
                continue;
            }
            let Some(asked) = mandate::asked_of(&r.principal, r.author, r.asker.as_deref(), r.body.get()) else { continue };
            let ask = self.ask_of(&asked, m, &members, (r.seq, r.at))?;
            if ask.asker == agent {
                continue;
            }
            // one its words did not name (`to`) that another agent claimed
            // already was not for it: its own claim opens it all the same
            let claimed = || self.rows("SELECT 1 AS x FROM asks WHERE channel = ? AND seq = ? AND agent != ? AND turn IS NOT NULL LIMIT 1", vec![CHAT.into(), SqlStorageValue::Integer(r.seq), agent.as_str().into()]);
            if !mandate::named(r.body.get()).contains(&agent) && !claimed()?.is_empty() {
                continue;
            }
            self.open_ask(&agent, r.seq, &ask, None)?;
            opened = true;
            others |= !is_owner(m, &ask.asker);
        }
        if newest > handed {
            self.exec("UPDATE asks_told SET handed = MAX(handed, ?) WHERE agent = ?", vec![SqlStorageValue::Integer(newest), agent.as_str().into()])?;
        }
        if opened {
            self.asks_changed(&agent, others)?;
        }
        Ok(others)
    }

    /// An agent's `turn.end` here: that turn's ask and every one it never
    /// claimed close, each held its hold from its words.
    fn ended_noted(&self, r: &ChannelRecord) -> CellResult<()> {
        let body: Value = crate::json_text(r.body.get()).unwrap_or(Value::Null);
        let Some(turn) = body["turn"].as_str().filter(|t| body["kind"] == "turn.end" && valid_op_id(t)) else { return Ok(()) };
        let now = js::now_ms();
        let closed = self.rows(
            "UPDATE asks SET until = MIN(COALESCE(until, MAX(?, at + ?)), MAX(?, at + ?)) WHERE agent = ? AND (until IS NULL OR (turn IS NULL AND until > ?)) AND (turn IS NULL OR turn = ?) RETURNING seq",
            vec![
                SqlStorageValue::Integer(now),
                SqlStorageValue::Integer(limits::TURN_HOLD_MS),
                SqlStorageValue::Integer(now),
                SqlStorageValue::Integer(limits::TURN_HOLD_MS),
                r.principal.as_str().into(),
                SqlStorageValue::Integer(now),
                turn.into(),
            ],
        )?;
        if !closed.is_empty() {
            self.asks_changed(&r.principal, false)?;
        }
        Ok(())
    }

    /// After a post's append (channels.rs `post`): an agent's claim of a
    /// record here (its `turn.start` on `work`), taken (a claim refused,
    /// another life's under that id, opens nothing), opens that record's
    /// ask again, whoever's the record says it is, naming the turn; its
    /// claim of someone else's is answered only once its agent fragment
    /// holds it.
    pub(crate) async fn claim_noted(&self, agent: &str, channel: &str, body: &Value) -> CellResult<()> {
        if channel != WORK || body["kind"] != "turn.start" {
            return Ok(());
        }
        let (Some(turn), cause) = (body["turn"].as_str().filter(|t| valid_op_id(t)), &body["cause"]) else { return Ok(()) };
        let Some(seq) = cause["seq"].as_i64().filter(|s| *s > 0 && cause["channel"] == CHAT && cause["fragment"].as_str() == self.name().ok().as_deref()) else { return Ok(()) };
        let agent = npub::display(agent);
        let members = self.agent_members()?;
        let Some(m) = members.iter().find(|m| npub::display(&m.principal) == agent) else { return Ok(()) };
        // the record it claims, which may be trimmed since it was handed:
        // its ask, kept, opens all the same
        let said: Option<Said> = self.typed("SELECT principal, author, asker, at, body FROM records WHERE channel = ? AND seq = ?", vec![CHAT.into(), SqlStorageValue::Integer(seq)])?.into_iter().next();
        let (named, said_kept) = (said.as_ref().map(|s| mandate::named(&s.body)).unwrap_or_default(), said.is_some());
        let rows: Vec<Claimed> = self.typed("SELECT asker, turn, until FROM asks WHERE agent = ? AND channel = ? AND seq = ?", vec![agent.as_str().into(), CHAT.into(), SqlStorageValue::Integer(seq)])?;
        let (asker, changed) = match rows.into_iter().next() {
            Some(c) if c.turn.as_deref() == Some(turn) && c.until.is_none() => (c.asker, false),
            Some(c) => {
                self.exec(
                    "UPDATE asks SET turn = ?, until = NULL WHERE agent = ? AND channel = ? AND seq = ?",
                    vec![turn.into(), agent.as_str().into(), CHAT.into(), SqlStorageValue::Integer(seq)],
                )?;
                (c.asker, true)
            }
            None => {
                // gone with no ask kept, said before it joined, or no one's
                // (app code's): nothing to open
                let Some(s) = said.filter(|s| s.at >= m.added_at) else { return Ok(()) };
                let author = s.author.as_deref().and_then(Author::parse);
                let Some(asked) = mandate::asked_of(&npub::display(&s.principal), author, s.asker.as_deref(), &s.body) else { return Ok(()) };
                let ask = self.ask_of(&asked, m, &members, (seq, s.at))?;
                if ask.asker == agent {
                    return Ok(());
                }
                self.open_ask(&agent, seq, &ask, Some(turn))?;
                (ask.asker, true)
            }
        };
        if changed {
            self.asks_changed(&agent, !is_owner(m, &asker))?;
        }
        // the other agents it was handed to, never claimed and not named in
        // its `to`, let it go (words that mention anyone go to every agent:
        // `heard_by`); one that claims it after opens it again
        #[derive(Deserialize)]
        struct Let {
            agent: String,
        }
        // whom its `to` named is read from the record: one trimmed since
        // lets no one else's go (it is not known whom it named)
        let now = js::now_ms();
        let open: Vec<Let> = if said_kept {
            self.typed(
                "SELECT agent FROM asks WHERE channel = ? AND seq = ? AND agent != ? AND turn IS NULL AND (until IS NULL OR until > ?)",
                vec![CHAT.into(), SqlStorageValue::Integer(seq), agent.as_str().into(), SqlStorageValue::Integer(now)],
            )?
        } else {
            vec![]
        };
        for other in open.into_iter().filter(|o| !named.contains(&o.agent)) {
            self.exec(
                "UPDATE asks SET until = MIN(COALESCE(until, MAX(?, at + ?)), MAX(?, at + ?)) WHERE channel = ? AND seq = ? AND agent = ?",
                vec![
                    SqlStorageValue::Integer(now),
                    SqlStorageValue::Integer(limits::TURN_HOLD_MS),
                    SqlStorageValue::Integer(now),
                    SqlStorageValue::Integer(limits::TURN_HOLD_MS),
                    CHAT.into(),
                    SqlStorageValue::Integer(seq),
                    other.agent.as_str().into(),
                ],
            )?;
            self.asks_changed(&other.agent, false)?;
        }
        self.asks_settled(&agent).await
    }

    /// Whose ask `asked` (record `seq` of `chat`, said `at`) is to `to`
    /// here (`Ask`): someone's own words; an agent's to another of their
    /// owner's agents, said on their owner's own proven ask
    /// (`records.carried`, kept in the append's own step), that ask
    /// carried on: the same asker, place, record and day; else the posting
    /// agent's own, unproven, lending nothing; words no one's word proves
    /// (app code's, a door's that relays no one), the place's own, unproven
    /// (an identity every release takes as an asker). Only the owner's own ask is
    /// ever carried on: anyone else's, through another of its agents,
    /// would reach what that agent's cap, grants and lend reach (FIN-615 PR
    /// 16a).
    fn ask_of(&self, asked: &Asked, to: &Member, members: &[Member], (seq, at): (i64, i64)) -> CellResult<Ask> {
        let own = |asker: &str| Ask { asker: asker.to_string(), proven: false, at, carried: None };
        Ok(match asked {
            Asked::By(asker) => Ask { proven: true, ..own(asker) },
            // no one's word proves it: in the place's own name, never proven
            Asked::NoOne => own(&npub::display(&self.must(MetaKey::Npub)?)),
            Asked::Agent(agent) if !members.iter().any(|m| npub::display(&m.principal) == *agent && m.owner.is_some() && m.owner == to.owner) => own(agent),
            Asked::Agent(agent) => {
                #[derive(Deserialize)]
                struct Row {
                    carried: Option<String>,
                }
                let rows: Vec<Row> = self.typed("SELECT carried FROM records WHERE channel = ? AND seq = ?", vec![CHAT.into(), SqlStorageValue::Integer(seq)])?;
                let carried = rows.into_iter().next().and_then(|r| r.carried);
                let m = carried.as_deref().and_then(mandate::kept).filter(|m| npub::display(&m.agent) == *agent && is_owner(to, &npub::display(&m.asker)) && m.until >= js::now_ms());
                match m {
                    Some(m) => Ask { asker: npub::display(&m.asker), proven: true, at: m.until - limits::MANDATE_LIFE_MS, carried },
                    None => own(agent),
                }
            }
        })
    }

    /// `agent`'s ask of record `seq` of `chat`, opened (`claimed`: by that
    /// turn), as `ask_of` says whose it is. One not claimed yet holds until
    /// `limits::ASK_UNCLAIMED_MS` from now (FIN-825: whatever its bridge
    /// did with the words), and its claim, later, opens it again.
    fn open_ask(&self, agent: &str, seq: i64, ask: &Ask, claimed: Option<&str>) -> CellResult<()> {
        let until = match claimed {
            Some(_) => SqlStorageValue::Null,
            None => SqlStorageValue::Integer(js::now_ms() + self.ask_unclaimed_ms()?),
        };
        self.exec(
            "INSERT INTO asks (agent, channel, seq, asker, proven, at, turn, carried, until) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
            vec![
                agent.into(),
                CHAT.into(),
                SqlStorageValue::Integer(seq),
                ask.asker.as_str().into(),
                SqlStorageValue::Integer(i64::from(ask.proven)),
                SqlStorageValue::Integer(ask.at),
                claimed.map_or(SqlStorageValue::Null, Into::into),
                ask.carried.as_deref().map_or(SqlStorageValue::Null, Into::into),
                until,
            ],
        )
    }

    /// How long an ask its agent hasn't claimed holds it: on test fleets,
    /// what the `unclaimed` lever set (ops.rs `test_fragment`).
    fn ask_unclaimed_ms(&self) -> CellResult<i64> {
        if !self.cfg.test_hooks {
            return Ok(limits::ASK_UNCLAIMED_MS);
        }
        Ok(self.meta(MetaKey::TestUnclaimedMs)?.and_then(|v| v.parse().ok()).unwrap_or(limits::ASK_UNCLAIMED_MS))
    }

    /// The agent members, in the order they joined (the first is the
    /// chat's lead), as their bridges read them.
    fn agent_members(&self) -> CellResult<Vec<Member>> {
        self.typed("SELECT principal, owner, added_at FROM members WHERE kind = 'agent' OR owner IS NOT NULL ORDER BY added_at, principal", vec![])
    }

    /// A new version of `agent`'s asks here, due now: `gate`, one that
    /// opened someone else's, which its agent waits for.
    fn asks_changed(&self, agent: &str, gate: bool) -> CellResult<()> {
        let now = js::now_ms();
        let gate = i64::from(gate);
        self.exec(
            "INSERT INTO asks_told (agent, handed, version, gate, told, attempts, next_at) VALUES (?, 0, ?, ? * ?, 0, 0, ?)
             ON CONFLICT (agent) DO UPDATE SET version = MAX(version + 1, excluded.next_at), gate = CASE WHEN ? = 1 THEN MAX(version + 1, excluded.next_at) ELSE gate END, attempts = 0, next_at = excluded.next_at",
            vec![agent.into(), SqlStorageValue::Integer(now), SqlStorageValue::Integer(gate), SqlStorageValue::Integer(now), SqlStorageValue::Integer(now), SqlStorageValue::Integer(gate)],
        )
    }

    fn told_of(&self, agent: &str) -> CellResult<Option<Told>> {
        Ok(self.typed::<Told>("SELECT agent, fragment, version, gate, told, attempts FROM asks_told WHERE agent = ?", vec![agent.into()])?.into_iter().next())
    }

    /// The agents whose someone else's ask is not told yet (live.rs: their
    /// sockets are sent nothing meanwhile).
    pub(crate) fn asks_waiting(&self) -> std::collections::BTreeSet<String> {
        #[derive(Deserialize)]
        struct Row {
            agent: String,
        }
        self.typed::<Row>("SELECT agent FROM asks_told WHERE told < gate", vec![]).map(|rows| rows.into_iter().map(|r| r.agent).collect()).unwrap_or_default()
    }

    /// Before `agent` reads records here (a page, a subscribe): someone
    /// else's ask it was handed is told first, or the read waits (502), so
    /// no one's ask reaches it before its turn is open.
    pub(crate) async fn asks_settled(&self, agent: &str) -> CellResult<()> {
        let agent = npub::display(agent);
        let Some(t) = self.told_of(&agent)?.filter(|t| t.told < t.gate) else { return Ok(()) };
        self.tell(t).await;
        if self.told_of(&agent)?.is_some_and(|t| t.told < t.gate) {
            return Err(CellError::new(ErrorCode::UpstreamFailed, "this agent's turn here isn't open yet: ask again in a moment"));
        }
        Ok(())
    }

    /// The asks due now told, after an append (channels.rs `published`)
    /// and from the alarm (`flush_asks`).
    pub(crate) async fn tell_asks(&self) {
        let Ok(due) = self.typed::<Told>(
            "SELECT agent, fragment, version, gate, told, attempts FROM asks_told WHERE told < version AND next_at <= ? ORDER BY next_at LIMIT ?",
            vec![SqlStorageValue::Integer(js::now_ms()), SqlStorageValue::Integer(ASKS_FLUSH_MAX)],
        ) else {
            return;
        };
        for t in due {
            self.tell(t).await;
        }
    }

    /// The alarm's turn: the asks due told, and the alarm armed for those
    /// still to tell.
    pub(crate) async fn flush_asks(&self) {
        self.tell_asks().await;
    }

    /// When an agent's asks here are next due to be told (the alarm).
    pub(crate) fn asks_due_at(&self) -> CellResult<Option<i64>> {
        Ok(self.rows("SELECT MIN(next_at) AS at FROM asks_told WHERE told < version", vec![])?.first().and_then(|r| r["at"].as_i64()))
    }

    /// An agent left: its asks here close now, told as any change is.
    pub(crate) fn asks_let_go(&self, agent: &str) -> CellResult<()> {
        let agent = npub::display(agent);
        let now = js::now_ms();
        let closed = self.rows("UPDATE asks SET until = ? WHERE agent = ? AND (until IS NULL OR until > ?) RETURNING seq", vec![SqlStorageValue::Integer(now), agent.as_str().into(), SqlStorageValue::Integer(now)])?;
        if !closed.is_empty() {
            self.asks_changed(&agent, false)?;
        }
        Ok(())
    }

    /// `t`'s agent fragment told all its open asks here, as one version:
    /// taken, it is told; its agent gone, its asks here go with it; else
    /// due again, doubling, and never dropped (fail closed). Its sockets
    /// then send what they held, or close (live.rs `asks_released`).
    async fn tell(&self, t: Told) {
        let agent = t.agent.clone();
        let told = self.told_to(t).await;
        let taken = told.as_ref().map_or_else(|e| e.code == ErrorCode::NotFound, |_| true);
        match told {
            Ok(version) => {
                let _ = self.exec("UPDATE asks_told SET told = MAX(told, ?), attempts = 0 WHERE agent = ?", vec![SqlStorageValue::Integer(version), agent.as_str().into()]);
                let _ = self.exec(
                    "DELETE FROM asks WHERE agent = ? AND ((until IS NOT NULL AND at < ?) OR at < ?)",
                    vec![agent.as_str().into(), SqlStorageValue::Integer(js::now_ms() - ENDED_KEPT_MS), SqlStorageValue::Integer(js::now_ms() - limits::TURN_HELD_MS)],
                );
            }
            Err(e) if e.code == ErrorCode::NotFound => {
                let _ = self.exec("DELETE FROM asks WHERE agent = ?", vec![agent.as_str().into()]);
                let _ = self.exec("DELETE FROM asks_told WHERE agent = ?", vec![agent.as_str().into()]);
            }
            Err(e) => {
                let attempts = self.told_of(&agent).ok().flatten().map_or(0, |t| t.attempts) + 1;
                let next_at = js::now_ms() + fragment_core::backoff::outbox_retry_ms(attempts);
                let _ = self.exec("UPDATE asks_told SET attempts = ?, next_at = ? WHERE agent = ?", vec![SqlStorageValue::Integer(attempts), SqlStorageValue::Integer(next_at), agent.as_str().into()]);
                self.event("asks.untold", &format!("{agent}'s turns here were not told: {}", e.message), json!({ "code": e.code, "attempts": attempts }));
                let _ = self.schedule().await;
            }
        }
        self.asks_released(&agent, taken);
    }

    /// The call that tells `t`'s agent fragment its open asks here: one
    /// turn an asker (its newest ask's), with the place's mandate for it
    /// when its words prove it, held while any of its asks is. Answers the
    /// version told.
    async fn told_to(&self, t: Told) -> CellResult<i64> {
        let fragment = match t.fragment {
            Some(f) => f,
            None => {
                let ids = vec![t.agent.clone()];
                let profiles = crate::ask_registry(&self.env, &crate::registry::calls::Profiles { ids, emails_of: vec![] }).await.map_err(|e| CellError::new(e.code, e.message))?.profiles;
                // made from no agent fragment, or that fragment gone: it runs nowhere
                let f = profiles.get(&t.agent).and_then(|p| p.fragment.clone()).ok_or_else(|| CellError::new(ErrorCode::NotFound, "this agent has no agent fragment"))?;
                self.exec("UPDATE asks_told SET fragment = ? WHERE agent = ?", vec![f.as_str().into(), t.agent.as_str().into()])?;
                f
            }
        };
        let (place, place_key, now) = (self.name()?, self.must(MetaKey::Npub)?, js::now_ms());
        // the version and its asks read in one step, after the awaits above
        let version = self.told_of(&t.agent)?.map_or(t.version, |t| t.version);
        // one an asker, every asker (past the most an agent fragment holds,
        // one more, which it holds as no one's)
        let open: Vec<Open> = self.typed(
            "SELECT asker, MAX(seq) AS seq, MAX(COALESCE(until, at + ?)) AS until FROM asks WHERE agent = ? AND channel = ? AND (until IS NULL OR until > ?) AND at >= ? GROUP BY asker ORDER BY seq DESC LIMIT ?",
            vec![
                SqlStorageValue::Integer(limits::TURN_HELD_MS),
                t.agent.as_str().into(),
                CHAT.into(),
                SqlStorageValue::Integer(now),
                SqlStorageValue::Integer(now - limits::TURN_HELD_MS),
                SqlStorageValue::Integer(limits::TURNS_HELD_MAX + 1),
            ],
        )?;
        let mut proved: Vec<Proved> = vec![];
        for o in &open {
            let p: Vec<Proved> =
                self.typed("SELECT proven, at, carried FROM asks WHERE agent = ? AND channel = ? AND seq = ?", vec![t.agent.as_str().into(), CHAT.into(), SqlStorageValue::Integer(o.seq)])?;
            proved.push(p.into_iter().next().ok_or_else(|| CellError::host("an ask read twice in one step is there both times"))?);
        }
        let keys = if proved.iter().any(|p| p.proven == 1) { crate::keys::mandate_keys(&self.env).await? } else { vec![] };
        let turns: Vec<PlaceTurn> = open
            .iter()
            .zip(&proved)
            .map(|(o, p)| {
                // a carried ask names its own words' place, record and day (`Mandate::carried`)
                let made = match &p.carried {
                    Some(text) => mandate::kept(text).and_then(|m| m.carried(&t.agent)),
                    None => Mandate::of_record(&o.asker, &t.agent, &place_key, CHAT, o.seq, p.at),
                };
                let mandate = made.filter(|_| p.proven == 1).map(|m| m.text(&keys[0]));
                PlaceTurn { turn: mandate::turn_id(&fragment, &place, CHAT, o.seq), asker: o.asker.clone(), mandate, until: o.until }
            })
            .collect();
        crate::fragment::call(&self.env, &fragment, &TurnsIn { agent: t.agent.clone(), place, version, turns }).await?;
        Ok(version)
    }
}

/// Whether `asker` is `m`'s owner: its asks narrow nothing.
fn is_owner(m: &Member, asker: &str) -> bool {
    m.owner.as_deref().is_some_and(|o| npub::display(o) == asker)
}
