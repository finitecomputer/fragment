//! One ask (FIN-615; docs/api.md, "One ask"): where a call needs someone's
//! yes, the platform asks them on one card, in the chat it was asked in
//! (`fragment_core::permit::Ask`), and the call is refused, saying so.
//!
//! - An agent's dangerous action asks its fragment's owner to confirm it,
//!   each time, and nothing is kept: their Confirm is their own request,
//!   from the shell, in person (`confirm_first`).
//! - Someone else's ask that needs what its place doesn't lend asks the
//!   agent's owner (`refused`, from the swap): Allow once lends it to the
//!   asker's next request there, Always here is the place's lend, and No
//!   writes nothing.
//!
//! The asker asks again once it is answered: an answer resumes nothing and
//! starts no turn (`permit::ask_answer`; the debt ledger, "A card's answer
//! resumes nothing").
//!
//! Three objects take part. The agent's fragment finds where to ask
//! (`ask_for`): the chat its asker's newest turn runs in, else, where whom
//! it asks is not in that chat, their own newest chat with the agent, so a
//! card never lands where its answerer can't see it. That chat posts the
//! card (`post_ask`: `kind: "ask"` on `work`, the platform's own record)
//! and takes its answer (`ask_answered`). A lend's place keeps what the answer
//! lends (`lend_answer`, `lent_once`).

use fragment_core::mandate;
use fragment_core::npub;
use fragment_core::permit::{self, Action, Answer, Ask, Danger, Lent, Verdict, Whom};
use fragment_proto::{limits, ErrorCode, FragmentKind, Lend, Role};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use worker::*;

use crate::error::{CellError, CellResult};
use crate::fragment::calls::{AskFor, Card, LendAnswer, LendOf, Named, PostAsk};
use crate::fragment::{Caller, FragmentCell, MetaKey};
use crate::js;

/// The channel a chat's cards are on: its agents' progress.
const WORK: &str = "work";
/// The channel a chat's people say things on: an asker's requests.
const CHAT: &str = "chat";

/// The refusal a call that asked gets: whom it asked, and that its asker
/// asks again once they answer; with no chat to ask in, why not (a lend's
/// as before the ask, `unasked`). Asked on `agent`'s behalf (its fragment's
/// name), by `asker` (none: a lend's, read from its open turns), of `asks`
/// (`owner`: the agent's own owner). It names whom it asked by what they
/// are to the agent, never their email: an agent passes its refusals on,
/// to people who may not see it.
pub(crate) async fn refused(env: &Env, agent: &str, asker: Option<&str>, (asks, owner): (&str, bool), card: Card, unasked: &str) -> CellError {
    let confirm = Lent::of_what(&card.what).is_none();
    let asked = crate::fragment::call(env, agent, &AskFor { asker: asker.map(str::to_string), asks: asks.to_string(), card }).await;
    let whom = if owner { "your owner" } else { "the person who asked" };
    match asked {
        Ok(Some(_)) if confirm => CellError::new(ErrorCode::Forbidden, format!("Asked {whom} to confirm it, on a card in the chat.")),
        Ok(Some(_)) => CellError::new(ErrorCode::Forbidden, format!("Asked {whom}, on a card in the chat. Ask again once they answer.")),
        Ok(None) if confirm => CellError::new(ErrorCode::Forbidden, no_chat(whom)),
        Ok(None) => CellError::new(ErrorCode::Forbidden, unasked),
        Err(e) => e,
    }
}

/// The card of a lend's ask: where it is kept, its ask finds (`ask_for`).
pub(crate) fn lend_card(ask: permit::Ask) -> Card {
    Card { what: ask.what().to_string(), fragment: String::new(), key: None, title: None, micros: None }
}

/// A confirm's refusal where no chat has its asker's turn open.
fn no_chat(whom: &str) -> String {
    format!("This needs {whom} to confirm it in a chat: ask there.")
}

/// A turn of the agent's a place opened, as `ask_for` reads one.
#[derive(Deserialize)]
struct OpenTurn {
    turn: String,
    place: String,
    asker: String,
    mandate: Option<String>,
}

/// A card of the platform's here, as `ask_answered` reads it.
#[derive(Deserialize)]
pub(crate) struct Asked {
    seq: i64,
    agent: String,
    asker: String,
    what: String,
    about: String,
    key: Option<String>,
    until: i64,
    answer: Option<String>,
    done: Option<i64>,
}

impl Asked {
    /// What it asks (`permit::Ask::what`).
    pub(crate) fn what(&self) -> &str {
        &self.what
    }

    /// Whether it was answered `answer` (a retry of that answer).
    pub(crate) fn answered_as(&self, answer: Answer) -> bool {
        self.answer.as_deref() == Some(answer.as_str())
    }
}

impl FragmentCell {
    /// What is dangerous here (`Danger`; `micros`, a cap's), asked by an
    /// agent for someone: a confirm card for this fragment's owner
    /// (`decide`'s rule 8), and the call refused, saying so. Anything else
    /// goes on to its route's own `require`.
    pub(crate) async fn confirm_first(&self, caller: &Caller, danger: Danger, micros: Option<i64>) -> CellResult<()> {
        // only an agent acting for someone asks (a person's is their own)
        let Some(signed) = caller.signed.as_ref().filter(|s| s.acting_for.is_some()) else { return Ok(()) };
        // a fragment not made (or gone) is 404, as its route's `require` says
        let facts = permit::Facts { visibility: self.facts()?.visibility, standing: self.standing(caller, false)?, action: Action::Dangerous(danger) };
        let Verdict::Ask(ask @ Ask::Confirm { of, .. }) = permit::decide(&facts) else { return Ok(()) };
        let asks = match of {
            Whom::Asker => signed.acting_for.as_deref(),
            Whom::AgentsOwner => signed.owner.as_deref(),
        };
        let asks = asks.expect("decide asks whom the request names");
        let owner = signed.owner.as_deref().is_some_and(|o| npub::display(o) == npub::display(asks));
        // where its turns are kept: the agent fragment it was made from
        let profiles = crate::ask_registry(&self.env, &crate::registry::calls::Profiles { ids: vec![signed.id.clone()], emails_of: vec![] }).await?;
        let Some(agent) = profiles.profiles.get(&signed.id).and_then(|p| p.fragment.clone()) else {
            // a key with no agent fragment runs no turn: no chat to ask in
            return Err(CellError::new(ErrorCode::Forbidden, no_chat(if owner { "your owner" } else { "the person who asked" })));
        };
        let title = self.meta(MetaKey::Face)?.and_then(|f| crate::json_text::<Named>(&f).ok()).and_then(|n| n.title);
        let card = Card { what: ask.what().to_string(), fragment: self.name()?, key: None, title, micros };
        Err(refused(&self.env, &agent, signed.acting_for.as_deref(), (asks, owner), card, "").await)
    }

    /// `AskFor` (an agent fragment's): where to ask, and the card posted
    /// there. The asker's turns are read newest first; a lend is asked in
    /// a place of theirs that lends it not (what each lends now, read
    /// there, taking nothing allowed once), a confirm in the newest. Answers
    /// the chat it asked in, or none: no turn of theirs is open, or no chat
    /// of whom it asks has them both.
    pub(crate) async fn ask_for(&self, a: AskFor) -> CellResult<Option<String>> {
        self.created()?;
        let now = js::now_ms();
        // the turns its places opened, open now (runs_on.rs `turns_in`): a
        // place's word on whose ask each is, never its computer's
        let rows: Vec<OpenTurn> = self.typed(
            "SELECT turn, place, asker, mandate FROM turns WHERE life = ? AND until > ? ORDER BY at DESC LIMIT ?",
            vec![crate::runs_on::PLACE_OPENED.into(), SqlStorageValue::Integer(now), SqlStorageValue::Integer(limits::TURNS_HELD_MAX)],
        )?;
        let agent = self.must(MetaKey::AgentIdentity)?;
        let asker = match &a.asker {
            Some(asker) => npub::display(asker),
            // a lend's: the one other person whose turns are open (more than
            // one lends nothing of the owner's, and asks no one: `mandate::proof`)
            None => {
                let owner = npub::display(&self.must(MetaKey::Owner)?);
                let others: std::collections::BTreeSet<String> = rows.iter().map(|r| npub::display(&r.asker)).filter(|x| *x != owner).collect();
                match others.len() {
                    1 => others.into_iter().next().expect("one"),
                    n => {
                        self.event("ask.nowhere", &format!("{n} people's turns are open: none is whose ask it is"), json!({ "askers": others, "what": a.card.what }));
                        return Ok(None);
                    }
                }
            }
        };
        let lent = Lent::of_what(&a.card.what);
        let mut card = a.card.clone();
        let mut at = None;
        for r in rows.iter().filter(|r| npub::display(&r.asker) == asker) {
            if let Some(lent) = lent {
                // a lend is kept where the ask's mandate was made, and asked there
                let Some(key) = r.mandate.as_deref().filter(|m| mandate::until_of(m).is_some_and(|u| u >= now)).and_then(mandate::place_of) else { continue };
                let here = crate::fragment::call(&self.env, &r.place, &LendOf { agent: agent.clone(), place: key.to_string(), asker: r.asker.clone(), request: None }).await;
                if lent.in_lend(here.unwrap_or(Lend::NONE)) {
                    continue;
                }
                card.fragment = r.place.clone();
                card.key = Some(key.to_string());
            }
            at = Some(r);
            break;
        }
        let Some(turn) = at else {
            self.event("ask.nowhere", &format!("no turn of {asker}'s is open to ask {} in", a.asks), json!({ "asker": asker, "what": a.card.what }));
            return Ok(None);
        };
        let post = PostAsk { agent: agent.clone(), asker: turn.asker.clone(), asks: a.asks.clone(), turn: turn.turn.clone(), card };
        if crate::fragment::call(&self.env, &turn.place, &post).await? {
            return Ok(Some(turn.place.clone()));
        }
        // whom it asks is not in that chat: their own newest chat with the
        // agent, its card about another chat (a lend's: there alone is it
        // Always here, so this one is not), never naming it: its title is
        // its own people's, not this chat's
        let list = crate::principal::ask(&self.env, &npub::display(&a.asks), &crate::principal::calls::List::default()).await?;
        let own = list.fragments.into_iter().filter(|f| f.kind == FragmentKind::Chat && f.role == Role::Owner && !f.archived && f.agents.iter().any(|x| npub::display(x) == npub::display(&agent)));
        let Some(own) = own.max_by_key(|f| f.preview_at.unwrap_or(0)) else {
            self.event("ask.nowhere", &format!("{} is not in {} and has no chat with this agent", a.asks, turn.place), json!({ "asker": asker, "what": a.card.what, "place": turn.place }));
            return Ok(None);
        };
        Ok(crate::fragment::call(&self.env, &own.name, &post).await?.then_some(own.name))
    }

    /// `PostAsk` (a chat's): the card, the platform's own record on `work`,
    /// or the same one still open; `false` where whom it asks is not in
    /// here (or this is no chat), so its asker looks elsewhere. At most
    /// `ASKS_OPEN_MAX` are open here at once.
    pub(crate) async fn post_ask(&self, p: PostAsk) -> CellResult<bool> {
        let (agent, asker, asks) = (npub::display(&p.agent), npub::display(&p.asker), npub::display(&p.asks));
        let p = PostAsk { agent, asker, asks, ..p };
        let Some(answers) = permit::answers_of(&p.card.what) else { return Err(CellError::invalid(format!("no card asks {:?}", p.card.what))) };
        // a lend's card in another chat than the one it is kept in: no Always here
        let elsewhere = Lent::of_what(&p.card.what).is_some() && p.card.fragment != self.name()?;
        let answers: Vec<&str> = answers.iter().map(|a| a.as_str()).filter(|a| !(elsewhere && *a == Answer::Always.as_str())).collect();
        if self.member_role(&p.agent)?.is_none() {
            return Err(CellError::new(ErrorCode::Forbidden, "an agent asks only in a chat it is in"));
        }
        if self.declared_channel(WORK)?.is_none() || self.member_role(&p.asks)?.is_none() {
            return Ok(false);
        }
        let now = js::now_ms();
        let about = match (p.card.what.as_str(), p.card.micros) {
            ("cap", Some(micros)) => format!("{} {micros}", p.card.fragment),
            ("cap", None) => format!("{} default", p.card.fragment),
            _ => p.card.fragment.clone(),
        };
        let open = "FROM owner_asks WHERE answer IS NULL AND until > ?";
        let same = self.count_of(&format!("SELECT COUNT(*) AS n {open} AND agent = ? AND asker = ? AND asks = ? AND what = ? AND about = ?"), vec![
            SqlStorageValue::Integer(now),
            p.agent.as_str().into(),
            p.asker.as_str().into(),
            p.asks.as_str().into(),
            p.card.what.as_str().into(),
            about.as_str().into(),
        ])?;
        if same > 0 {
            return Ok(true);
        }
        if self.count_of(&format!("SELECT COUNT(*) AS n {open}"), vec![SqlStorageValue::Integer(now)])? >= limits::ASKS_OPEN_MAX {
            return Err(CellError::new(ErrorCode::RateLimited, "this chat has as many open cards as it holds: answer one first"));
        }
        let until = now + limits::ASK_LIFE_MS;
        let mut body = json!({
            "kind": "ask", "turn": p.turn, "agent": p.agent, "asker": p.asker, "asks": p.asks,
            "what": p.card.what, "fragment": p.card.fragment, "options": answers, "expiresAt": until,
        });
        if elsewhere {
            body["elsewhere"] = json!(true);
        }
        if let Some(title) = &p.card.title {
            body["title"] = json!(title);
        }
        if p.card.what == "cap" {
            body["micros"] = json!(p.card.micros);
        }
        let digest = Sha256::digest(format!("{}|{}|{}|{about}|{now}", p.agent, p.asker, p.card.what).as_bytes());
        let key = format!("ask:{}", &hex::encode(digest)[..32]);
        // whose ask it is is named in its body, never as its record's
        // `asker`: a card is no one's words, and asks no agent anything
        let (record, _) = self.append_own(WORK, &body, &key, None).await?;
        self.exec(
            "INSERT OR IGNORE INTO owner_asks (seq, agent, asker, asks, what, about, key, until) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            vec![
                SqlStorageValue::Integer(record.seq),
                p.agent.as_str().into(),
                p.asker.as_str().into(),
                p.asks.as_str().into(),
                p.card.what.as_str().into(),
                about.as_str().into(),
                p.card.key.map_or(SqlStorageValue::Null, Into::into),
                SqlStorageValue::Integer(until),
            ],
        )?;
        self.event("ask.posted", &format!("{} asked {} for {}", p.agent, p.asks, p.card.what), json!({ "seq": record.seq, "what": p.card.what }));
        Ok(true)
    }

    /// The platform's card a post's `answers` names, if it names one here.
    pub(crate) fn platform_ask(&self, channel: &str, seq: i64) -> CellResult<Option<Asked>> {
        if channel != WORK {
            return Ok(None);
        }
        let rows: Vec<Asked> = self.typed("SELECT seq, agent, asker, what, about, key, until, answer, done FROM owner_asks WHERE seq = ?", vec![SqlStorageValue::Integer(seq)])?;
        Ok(rows.into_iter().next())
    }

    /// Whether `body` answers `ask` now: its shape (`permit::ask_answer`),
    /// and the card still open. Read with no await before its append.
    pub(crate) fn ask_open(&self, ask: &Asked, body: &Value) -> CellResult<Answer> {
        let answer = permit::ask_answer(body, &ask.what).map_err(|why| CellError::invalid(format!("an answer to the platform's card {why}")))?;
        if answer == Answer::Always && ask.about != self.name()? {
            return Err(CellError::invalid("Always here is answered on a card in the chat it is kept in"));
        }
        if ask.answer.is_some() {
            return Err(CellError::new(ErrorCode::Conflict, "this card was answered already"));
        }
        if ask.until <= js::now_ms() {
            return Err(CellError::new(ErrorCode::Conflict, "this card ran out: its asker asks again"));
        }
        Ok(answer)
    }

    /// `answer`, `by`'s, taken on the card `ask` (right after its append,
    /// with no await between: a second answer finds it answered), then
    /// written where its fact lives, once (`done`): a retry of the answer
    /// finishes a first try that failed before its write, and writes nothing
    /// again after one that did (Always here switched off since stays off,
    /// Allow once taken stays taken). The card is closed either way.
    pub(crate) async fn ask_answered(&self, ask: &Asked, answer: Answer, by: &str) -> CellResult<()> {
        self.exec("UPDATE owner_asks SET answer = ? WHERE seq = ? AND answer IS NULL", vec![answer.as_str().into(), SqlStorageValue::Integer(ask.seq)])?;
        if let (Some(key), Answer::Once | Answer::Always, None) = (&ask.key, answer, ask.done) {
            let lent = LendAnswer { agent: ask.agent.clone(), asker: ask.asker.clone(), key: key.clone(), what: ask.what.clone(), always: answer == Answer::Always, by: by.to_string() };
            match ask.about == self.name()? {
                true => self.lend_answer(&lent).await?,
                false => crate::fragment::call(&self.env, &ask.about, &lent).await?,
            }
        }
        self.exec("UPDATE owner_asks SET done = 1 WHERE seq = ?", vec![SqlStorageValue::Integer(ask.seq)])?;
        let turn = self.rows("SELECT json_extract(body, '$.turn') AS turn FROM records WHERE channel = ? AND seq = ?", vec![WORK.into(), SqlStorageValue::Integer(ask.seq)])?;
        let body = json!({ "kind": "ask.closed", "ask": ask.seq, "turn": turn.first().map(|r| r["turn"].clone()), "agent": npub::display(&ask.agent), "option": answer.as_str(), "by": npub::display(by) });
        self.append_own(WORK, &body, &format!("ask.closed:{}", ask.seq), None).await?;
        Ok(())
    }

    /// `LendAnswer` (a lend's place): an owner's Allow once or Always here,
    /// for their own agent member, in this life.
    pub(crate) async fn lend_answer(&self, a: &LendAnswer) -> CellResult<()> {
        let lent = Lent::of_what(&a.what).ok_or_else(|| CellError::invalid(format!("no lend is of {:?}", a.what)))?;
        let (agent, asker) = (npub::display(&a.agent), npub::display(&a.asker));
        let rows = self.rows("SELECT owner FROM members WHERE principal = ?", vec![agent.as_str().into()])?;
        let owner = rows.first().and_then(|r| r["owner"].as_str()).map(npub::display);
        if self.meta(MetaKey::Npub)?.as_deref() != Some(a.key.as_str()) || owner.as_deref() != Some(npub::display(&a.by).as_str()) {
            return Err(CellError::new(ErrorCode::Forbidden, "an agent's lend here is its owner's, while it is in here"));
        }
        self.writable().await?;
        match a.always {
            true => {
                let lend = lent.added_to(self.lend_of(&agent)?);
                self.exec("UPDATE members SET lend = ? WHERE principal = ?", vec![SqlStorageValue::Integer(i64::from(lend.bits())), agent.as_str().into()])?;
                self.event("member.lend", &format!("{agent} lent spend {}, accounts {}, on a card", lend.spend, lend.accounts), json!({ "agent": agent, "spend": lend.spend, "accounts": lend.accounts }));
            }
            // lent to the asker's first words here from now: after the chat's
            // newest record now
            false => {
                let newest = self.rows("SELECT seq FROM records WHERE channel = ? ORDER BY seq DESC LIMIT 1", vec![CHAT.into()])?;
                let after = newest.first().and_then(|r| r["seq"].as_i64()).unwrap_or(0);
                self.exec(
                    "INSERT OR REPLACE INTO lent_once (agent, asker, what, after) VALUES (?, ?, ?, ?)",
                    vec![agent.as_str().into(), asker.as_str().into(), a.what.as_str().into(), SqlStorageValue::Integer(after)],
                )?;
                self.event("member.lent-once", &format!("{agent} lent {} once, for {asker}", a.what), json!({ "agent": agent, "asker": asker, "what": a.what }));
            }
        }
        Ok(())
    }

    /// `lend` with what its owner allowed once for `asked`'s asker: lent to
    /// their first words in the chat after the answer, that request alone
    /// (each of its calls), and to no other, before or after.
    pub(crate) fn lent_once(&self, asked: &LendOf, mut lend: Lend) -> CellResult<Lend> {
        let Some((channel, seq)) = asked.request.as_ref().filter(|(c, _)| c == CHAT) else { return Ok(lend) };
        #[derive(Deserialize)]
        struct Row {
            what: String,
            after: i64,
        }
        let (agent, asker) = (npub::display(&asked.agent), npub::display(&asked.asker));
        let rows: Vec<Row> = self.typed("SELECT what, after FROM lent_once WHERE agent = ? AND asker = ?", vec![agent.as_str().into(), asker.as_str().into()])?;
        for r in rows {
            let Some(lent) = Lent::of_what(&r.what) else { continue };
            // their first record since: a seek from `after`, to their next words
            let first = self.rows("SELECT seq FROM records WHERE channel = ? AND seq > ? AND principal = ? ORDER BY seq LIMIT 1", vec![channel.as_str().into(), SqlStorageValue::Integer(r.after), asker.as_str().into()])?;
            if first.first().and_then(|f| f["seq"].as_i64()) == Some(*seq) {
                lend = lent.added_to(lend);
            }
        }
        Ok(lend)
    }
}
