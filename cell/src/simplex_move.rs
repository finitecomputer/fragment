//! The operators' move of an agent's SimpleX link from its computer into a
//! SimpleX connector of its person's own, on a door of its own (D4,
//! FIN-731; docs/bridge.md, "SimpleX"): `POST /api/admin/people/{person}/
//! simplex-move {agent, act}`, the admin Worker's alone, so it costs no
//! fragment's isolate anything. Nothing for the person to do: their
//! SimpleX app keeps the same contact, and the move carries the link's
//! data (its databases), never a new link.
//!
//! - `plan`: what `go` would do next, and whether it may; nothing changes.
//! - `go`: its next steps, each recorded in its computer's object
//!   (`computer/simplex-moves`) before the next, so a `go` again goes on
//!   from where the last one stopped; each record names the step it moves
//!   on from, so two `go`s at once never both act on one (the first claims
//!   it, **making**, before any connector is made):
//!   1. **made**: the connector, the person's own (made as Settings' Set
//!      up makes one, as the person: the platform makes a fragment for its
//!      person, a first, each logged as `simplex-move`), its door fenced
//!      until the link is given (`/door/expect`: no connect begins it as a
//!      new profile first, its setup page opened meanwhile included), its
//!      inbox's secret set; not yet bound;
//!   2. **imported**: its computer's link handed over (its core stopped for
//!      good there: `computer/simplex-hand`), the answer that door's first
//!      save, streamed into the door (`/door/import`, its SHA-256 checked
//!      as it is stored), then the agent's computer door closed. An import
//!      that fails (its copy damaged on its way each time it was asked)
//!      goes back at once: one end always runs the link;
//!   3. **bound**: the connector bound as the agent's door, as Settings
//!      binds one: only now does Settings show it, its door already
//!      holding the link, so a setup page opened then starts that door
//!      with its link, never a new one;
//!   4. **connecting**: the connector's `connect`, as the person: its door
//!      starts from that save;
//!   5. **done**: its door's runner linked, its connector told so (its
//!      `check`), and the person's old SimpleX chat archived (kept, its
//!      history searchable).
//!
//!   A person's SimpleX chat with their old agent, staged on their
//!   computer by finite.computer's move and never kept there, moves as
//!   staged (**offered** once bound): never kept on their behalf, its
//!   connector's setup page asks them to keep it or start a new chat.
//! - `back`: the way back from wherever it stopped: its door's newest save
//!   (its planned stop saves last, once what it took reached its
//!   connector) placed on its computer, which takes its door key again and
//!   runs its core, carrying what its door took and had not sent; then the
//!   connector, once it has published what its door delivered, unbound (its
//!   door ends) and archived, its history kept; the old chat unarchived;
//!   the record forgotten. A door that was given the link and holds
//!   nothing now (its person unlinked it, so it ended) has no way back:
//!   refused, its computer's old copy never run again.
//!
//! Refused with its reason, nothing changed: an agent not on the person's
//! computer, a link not bound, a deployment that runs no doors; its
//! computer's bridge refuses a link busy (its agent working in that chat,
//! files or messages on their way) or one too large for a door's save.

use fragment_proto::{CreateFragment, ErrorCode, Identity, IdentityKind};
use serde_json::{json, Value};
use worker::{Env, Method, Request, Response, Url};

use crate::config::Config;
use crate::error::{CellError, CellResult};
use crate::registry::admin;
use crate::routed::{Routed, Signed};

/// The connector's template, its inbox's secret and that secret's host
/// (templates/simplex `fragment.json`, its README).
const TEMPLATE: &str = fragment_core::door::SIMPLEX;
const SECRET: &str = "SIMPLEX_DOOR_SECRET";
const DOOR_HOST: &str = "door.fragment.internal";
/// A handover's and a way back's asks, a copy found damaged on its way
/// (FIN-775) asked for again (`hand_over`, `back`).
const HAND_ASKS: u32 = 3;

pub(crate) async fn simplex_move(env: &Env, cfg: &Config, url: &Url, by: &str, person: &str, bytes: &[u8]) -> CellResult<Value> {
    if !fragment_core::npub::is_identity(person) {
        return Err(CellError::new(ErrorCode::NotFound, format!("no person {person}")));
    }
    let asked: Value = crate::orgs::body(bytes)?;
    let agent = asked["agent"].as_str().filter(|a| fragment_proto::valid_fragment_name(a)).ok_or_else(|| CellError::invalid("agent: the agent fragment's name"))?.to_string();
    let m = Move { env, cfg, url, by, person: Signed::new(Identity { id: person.into(), kind: IdentityKind::Person, owner: None, held: None }, None), computer: fragment_core::computer::default_computer_of(person), agent };
    match asked["act"].as_str().unwrap_or("") {
        "plan" => m.plan().await,
        "go" => m.go().await,
        "back" => m.back().await,
        act => Err(CellError::invalid(format!("act {act:?}: plan, go or back"))),
    }
}

struct Move<'a> {
    env: &'a Env,
    cfg: &'a Config,
    url: &'a Url,
    by: &'a str,
    person: Signed,
    computer: String,
    agent: String,
}

/// An agent fragment's label (`juniper--k3x9`: `juniper`).
fn label_of(agent: &str) -> &str {
    fragment_proto::split_fragment_name(agent).map_or(agent, |(label, _)| label)
}

/// A record's string field, or "".
fn text<'v>(v: &'v Value, k: &str) -> &'v str {
    v[k].as_str().unwrap_or("")
}

impl Move<'_> {
    /// Its record and its computer's word on its link (`computer/simplex-moves`).
    async fn read(&self) -> CellResult<Value> {
        crate::computer::ask(self.env, &self.computer, "computer/simplex-moves", &json!({ "op": "read", "agent": self.agent })).await.map_err(|e| match e.code {
            ErrorCode::NotFound => CellError::new(ErrorCode::NotFound, format!("{} is not on {}'s computer: {}", self.agent, self.person.id, e.message)),
            _ => e,
        })
    }

    /// Its record moved on from step `from` ("" for none), refused when
    /// another `go` moved it on first, or, naming `claim`, when it is no
    /// longer this `go`'s claim.
    async fn set(&self, from: &str, claim: Option<&str>, record: Value) -> CellResult<Value> {
        crate::computer::ask(self.env, &self.computer, "computer/simplex-moves", &json!({ "op": "set", "agent": self.agent, "from": from, "claim": claim, "record": record })).await
    }

    async fn note(&self, detail: String) -> CellResult<()> {
        crate::ask_registry(self.env, &admin::Note { by: self.by.to_string(), action: "simplex-move".into(), target: self.person.id.clone(), detail, request: None }).await?;
        Ok(())
    }

    /// The person's own request of fragment `name` (`inner`, as the router
    /// hands it on), signed as them: Settings' own calls, made for them.
    async fn as_person(&self, name: &str, method: Method, inner: &str, body: Option<&Value>) -> CellResult<Value> {
        let bare = Request::new(self.url.as_str(), method)?;
        let routed = Routed { name: name.into(), url: self.url.clone(), signed: Some(self.person.clone()), credential: None };
        let sent = body.map(|b| b.to_string().into_bytes()).unwrap_or_default();
        let mut r = crate::forward(self.env, &bare, crate::bytes_body(sent), crate::Forward { routed, inner: inner.into(), extra: vec![] }).await?;
        answer(&mut r, &format!("{name} {inner}")).await
    }

    /// Its door's state (cell/door.mjs `/door/state`): `doors`, whether the
    /// deployment runs them; `runner`, its runner's own while it runs.
    async fn door_state(&self, door: &str) -> CellResult<Value> {
        let mut r = crate::routed::send(self.env, "SIMPLEX_DOOR", door, crate::routed::internal_request("door/state", "{}")?).await?;
        answer(&mut r, "its door's state").await
    }

    /// What it is and what blocks it; the CLI words its next step from
    /// its record's (cli/src/main.rs `simplex_next`).
    async fn plan(&self) -> CellResult<Value> {
        let read = self.read().await?;
        Ok(json!({ "agent": self.agent, "computer": self.computer, "awake": read["awake"], "link": read["link"]["said"], "record": read["record"], "blocked": self.blocked(&read) }))
    }

    /// Why a move that has not begun may not: its computer asleep (an
    /// always-on one wakes by itself), or its link not one its computer
    /// holds bound.
    fn blocked(&self, read: &Value) -> Option<String> {
        if !read["record"].is_null() {
            return None;
        }
        if read["awake"] != true {
            return Some("its computer is asleep: ask again once it wakes (an always-on one wakes by itself)".into());
        }
        let said = &read["link"]["said"];
        let movable = said["phase"] == "linked" || (said["phase"] == "off" && said["staged"] == true);
        (read["link"]["status"] != 200 || !movable).then(|| format!("no SimpleX link or staged chat to move: {said}"))
    }

    async fn go(&self) -> CellResult<Value> {
        let mut read = self.read().await?;
        if read["record"].is_null() {
            if let Some(why) = self.blocked(&read) {
                return Err(CellError::new(ErrorCode::Conflict, why));
            }
            // claimed first, by a claim of its own: a `go` beside this one
            // finds it claimed, and one after a `back` let this claim go
            // never has its record finished by this one
            let claim = hex::encode(crate::js::random_bytes::<8>());
            self.set("", None, json!({ "step": "making", "claim": claim })).await?;
            read["record"] = self.make(&claim).await?;
        }
        let mut rec = read["record"].clone();
        if text(&rec, "step") == "making" {
            return Err(CellError::new(ErrorCode::Conflict, "a `go` is making its connector: ask again (one that stopped: `back`)"));
        }
        // one failure handler (Rule B): from its handover until its door is
        // connected, any failure but another `go`'s moving it on goes back,
        // unless its computer's own moved mark is read and stands not (its
        // core never stopped). A mark unread is not known: the way back is
        // tried, and if it fails too, it says to ask `--back` again (Codex's
        // round on 6f714bbde).
        rec = match self.onward(rec).await {
            Err(e) if !e.message.contains(crate::computer::MOVED_ON) && self.read().await.map_or(true, |r| r["record"]["moved"] == true) => return Err(self.went_back(e).await),
            r => r?,
        };
        if text(&rec, "step") == "connecting" {
            // its connect over (held: given up), read before its door, which
            // then never started (refused, or its job failed): its link runs
            // at neither end, so it goes back (Codex's round on b898dcf13
            // and its fix-check)
            let over = !self.unfinished(text(&rec, "connector"), "connect", &["queued", "running", "blocked"]).await?;
            let state = self.door_state(text(&rec, "door")).await?;
            if state["runner"]["phase"] != "linked" {
                if over && state["door"]["started"] == 0 {
                    return Err(self.went_back(CellError::new(ErrorCode::Conflict, "its connect ended unstarted")).await);
                }
                return Ok(json!({ "agent": self.agent, "record": rec }));
            }
            // its connector told whom its door is linked to, as its page asks
            let connector = text(&rec, "connector").to_string();
            self.as_person(&connector, Method::Post, "/api/ops/check", Some(&json!({ "id": "simplex-move-check", "input": {} }))).await?;
            if let Some(chat) = rec["chat"].as_str() {
                crate::principal::ask(self.env, &self.person.id, &crate::principal::calls::SetOwn { fragment: chat.into(), own: crate::principal::calls::Own::Archived, on: true }).await?;
            }
            rec = self.set("connecting", None, json!({ "step": "done" })).await?;
            self.note(format!("{}'s SimpleX moved into {connector}; {} archived", self.agent, rec["chat"].as_str().unwrap_or("no old chat"))).await?;
        }
        Ok(json!({ "agent": self.agent, "record": rec }))
    }

    /// Its link handed over and imported, its connector bound, then its
    /// door connected (a staged chat offered instead, waiting for its
    /// person's word).
    async fn onward(&self, mut rec: Value) -> CellResult<Value> {
        if text(&rec, "step") == "made" {
            rec = self.hand_over(&rec).await?;
        }
        if text(&rec, "step") == "imported" {
            rec = self.bind(&rec).await?;
        }
        if text(&rec, "step") == "bound" {
            // its SimpleX name its title, as Settings connects one and as its
            // computer named it, so its person's app shows no new name
            let status = self.as_person(&self.agent, Method::Get, "/api/status", None).await?;
            let title = status["title"].as_str().map(str::trim).filter(|t| !t.is_empty()).unwrap_or(label_of(&self.agent));
            let name: String = title.chars().take(64).collect();
            self.as_person(text(&rec, "connector"), Method::Post, "/api/ops/connect", Some(&json!({ "id": "simplex-move-connect", "input": { "name": name } }))).await?;
            rec = self.set("bound", None, json!({ "step": "connecting" })).await?;
        }
        Ok(rec)
    }

    /// Its connector: made as the person's own from the SimpleX template,
    /// on a deployment whose doors run, its inbox's secret set, recorded
    /// as it is made (the claim's `go` alone makes one). Bound only once
    /// its door holds the link (`bind`).
    async fn make(&self, claim: &str) -> CellResult<Value> {
        let label = fragment_core::door::connector_label(label_of(&self.agent), TEMPLATE, fragment_proto::label_room(self.cfg.host_label_suffix()));
        let create = CreateFragment { name: String::new(), label: Some(label), visibility: None, template: Some(TEMPLATE.into()), title: None, agent: None };
        let mut made = crate::create_fragment(self.env, self.cfg, self.url, create, self.person.clone()).await?;
        // read by its two fields alone (a whole `Created` decoder costs the
        // admin Worker kilobytes for nothing more)
        let made = answer(&mut made, "its connector's making").await?;
        let (name, npub) = (text(&made, "name").to_string(), text(&made, "npub"));
        let door = fragment_core::door::door_id(&name, npub).ok_or_else(|| CellError::host(format!("{name} names no door")))?;
        // its door fenced from now until the link is given (cell/door.mjs
        // `EXPECTING`): its setup page, opened meanwhile, starts no new
        // profile. Refused (a deployment that runs no doors), its
        // connector goes, nothing moved.
        let mut expected = crate::routed::send(self.env, "SIMPLEX_DOOR", &door, crate::routed::internal_request("door/expect", &json!({ "door": door }).to_string())?).await?;
        if let Err(e) = answer(&mut expected, "its door's fence").await {
            self.as_person(&name, Method::Delete, "/delete", None).await?;
            self.forget().await?;
            return Err(e);
        }
        let secret = hex::encode(crate::js::random_bytes::<32>());
        self.as_person(&name, Method::Put, &format!("/api/secrets/{SECRET}"), Some(&json!({ "value": secret, "hosts": [DOOR_HOST] }))).await?;
        let rec = match self.set("making", Some(claim), json!({ "connector": name, "door": door, "step": "made" })).await {
            Ok(rec) => rec,
            // its claim let go meanwhile (a `back`): the connector it made
            // is no one's move, so it goes, its door with it
            Err(e) if e.code == ErrorCode::Conflict => {
                self.as_person(&name, Method::Delete, "/delete", None).await?;
                return Err(e);
            }
            Err(e) => return Err(e),
        };
        self.note(format!("made {name} for {}'s SimpleX", self.agent)).await?;
        Ok(rec)
    }

    /// Its connector bound as the agent's door, as its person would bind it
    /// in Settings, once its door holds the link: the same asked again. A
    /// staged chat is then offered on its setup page.
    async fn bind(&self, rec: &Value) -> CellResult<Value> {
        let connector = text(rec, "connector");
        self.as_person(connector, Method::Post, "/api/door", Some(&json!({ "act": "link", "agent": self.agent }))).await?;
        self.set("imported", None, json!({ "step": if rec["staged"] == true { "offered" } else { "bound" } })).await
    }

    /// Its computer's link handed over and its save given to its door, then
    /// the agent's computer door closed: what its computer signs relays no
    /// one now. A failure goes back once its core may have stopped (`go`).
    async fn hand_over(&self, rec: &Value) -> CellResult<Value> {
        // its save, streamed out of its computer's container into its door,
        // whose bucket checks its SHA-256 as it stores it: a copy damaged on
        // its way (FIN-775, seen on Cloudflare) is kept by no one, and asked
        // for again (its computer answers its handover's ticket again with
        // the same save), at most `HAND_ASKS` times
        let mut asks = 0;
        let (sha, length, staged) = loop {
            asks += 1;
            let save: worker::worker_sys::web_sys::Response = self.hand("handover", None, false, "").await?.into();
            let head = |k: &str| save.headers().get(k).ok().flatten().unwrap_or_default();
            let (sha, length, staged) = (head("x-door-sha256"), head("x-door-length"), head("x-simplex-staged") == "1");
            let body = save.body().map(Into::into);
            let headers = [("x-fragment-door", text(rec, "door")), ("x-door-agent", label_of(&self.agent)), ("x-door-sha256", sha.as_str()), ("x-door-length", length.as_str()), ("x-door-staged", if staged { "1" } else { "0" })];
            let put = match crate::routed::send(self.env, "SIMPLEX_DOOR", text(rec, "door"), crate::routed::internal_stream("door/import", Method::Put, &headers, body)?).await {
                Ok(mut put) => answer(&mut put, "its door's import").await,
                Err(e) => Err(e),
            };
            match put {
                Err(e) if e.message.contains(" 422 not_its_sha:") && asks < HAND_ASKS => continue,
                r => {
                    r?;
                    break (sha, length, staged);
                }
            }
        };
        crate::fragment::ask(self.env, &self.agent, "computer/simplex-move", &json!({ "computer": self.computer, "act": "closed" })).await?;
        self.note(format!("{}'s SimpleX {} handed over to {}'s door ({sha}, {length} bytes)", self.agent, if staged { "staged chat" } else { "link" }, text(rec, "connector"))).await?;
        // a staged chat waits for its person's word once bound, never
        // connected for them
        self.set("made", None, json!({ "step": "imported", "staged": staged })).await
    }

    /// The one way out of a handover that failed after its core may have
    /// stopped: the way back (`back`: its door fenced, its computer given
    /// the save its door took, or told to run its own copy again, both its
    /// marks, its bridge's and its computer's, let go), then `e`, and
    /// whether it went back.
    async fn went_back(&self, e: CellError) -> CellError {
        let back = self.back().await.err().map_or_else(|| "went back: its computer runs its link again".into(), |b| format!("its way back failed too ({}): ask `--back` again", b.message));
        CellError::new(e.code, format!("{}; {back}", e.message))
    }

    /// Its computer's half (`computer/simplex-hand`): a handover, or the
    /// way back with its door's save (`save`, `behind`).
    async fn hand(&self, act: &str, save: Option<worker::wasm_bindgen::JsValue>, behind: bool, sha: &str) -> CellResult<Response> {
        let headers = [("x-door-act", act), ("x-door-agent", self.agent.as_str()), ("x-door-behind", if behind { "1" } else { "0" }), ("x-door-sha256", sha)];
        let mut r = crate::routed::send(self.env, "COMPUTER", &self.computer, crate::routed::internal_stream("computer/simplex-hand", Method::Post, &headers, save)?).await?;
        if r.status_code() != 200 {
            answer(&mut r, act).await?;
        }
        Ok(r)
    }

    async fn forget(&self) -> CellResult<Value> {
        crate::computer::ask(self.env, &self.computer, "computer/simplex-moves", &json!({ "op": "forget", "agent": self.agent })).await
    }

    async fn back(&self) -> CellResult<Value> {
        let read = self.read().await?;
        let rec = read["record"].clone();
        if rec.is_null() {
            return Err(CellError::new(ErrorCode::Conflict, format!("no SimpleX move of {} to go back from", self.agent)));
        }
        // its computer runs SimpleX only awake and always on (its own word,
        // never its bridge's, which reads it a minute late): else its door
        // keeps the link, never stopped for nothing (Codex's round on
        // 8cacd32d1; the hosted check on 8c0ad40ec)
        if rec["moved"] == true && (read["awake"] != true || read["alwaysOn"] != true) {
            return Err(CellError::new(ErrorCode::Conflict, "its computer must be awake and always on: turn always-on on, then `--back` again (its door keeps the link)"));
        }
        // its door stops, saving last, and hands its newest save back, with
        // its SHA-256; a copy its computer finds damaged on its way (seen
        // once on Cloudflare) changed nothing there, and is asked for again
        // (the door answers the same save), at most `HAND_ASKS` times
        for asks in 1..=HAND_ASKS {
            if rec["moved"] != true {
                break;
            }
            let door = text(&rec, "door");
            let mut held = crate::routed::send(self.env, "SIMPLEX_DOOR", door, crate::routed::internal_request("door/handback", &json!({ "door": door }).to_string())?).await?;
            let (save, behind, sha) = match held.status_code() {
                // its save, whatever it is, the one it was given too (its
                // computer may have gone back to an older save since): the
                // bridge places a link's, and keeps a staged chat its door
                // never kept staged as it was
                200 => {
                    let raw: worker::worker_sys::web_sys::Response = held.into();
                    let head = |k: &str| raw.headers().get(k).ok().flatten().unwrap_or_default();
                    let (behind, sha) = (head("x-door-behind") == "1", head("x-door-sha256"));
                    (raw.body().map(Into::into), behind, sha)
                }
                // none: a door never given the link, its computer's copy
                // the link still; one given it that holds nothing now ended
                // with it (its person unlinked it): no way back, its
                // computer's old copy never run again
                204 if matches!(text(&rec, "step"), "making" | "made") => (None, false, String::new()),
                204 => return Err(CellError::new(ErrorCode::Conflict, format!("{door} was unlinked, so ended: no way back; {}'s copy on its computer stays stopped", self.agent))),
                _ => return Err(answer(&mut held, "its door's way back").await.err().unwrap_or_else(|| CellError::host("its door's way back"))),
            };
            match self.hand("back", save, behind, &sha).await {
                Err(e) if e.message.contains(" 422 damaged:") && asks < HAND_ASKS => continue,
                Err(e) => return Err(e),
                Ok(_) => {}
            }
            if let Some(chat) = rec["chat"].as_str() {
                crate::principal::ask(self.env, &self.person.id, &crate::principal::calls::SetOwn { fragment: chat.into(), own: crate::principal::calls::Own::Archived, on: false }).await?;
            }
            break;
        }
        // its connector kept, its chat's history with it: unbound (its door
        // ends) once it has taken what its door delivered, and archived
        if let Some(connector) = rec["connector"].as_str() {
            if self.unfinished(connector, "intake", &["queued", "running", "held", "blocked"]).await? {
                return Err(CellError::new(ErrorCode::Conflict, format!("{connector} has delivered messages not yet published (an intake run not done): ask again, or replay them")));
            }
            match self.as_person(connector, Method::Post, "/api/door", Some(&json!({ "act": "unlink" }))).await {
                Err(e) if e.code != ErrorCode::NotFound => return Err(e),
                Err(_) => {}
                Ok(_) => crate::principal::ask(self.env, &self.person.id, &crate::principal::calls::SetOwn { fragment: connector.into(), own: crate::principal::calls::Own::Archived, on: true }).await?,
            }
        }
        self.forget().await?;
        self.note(format!("{}'s SimpleX went back to its computer; {} unbound and archived", self.agent, rec["connector"].as_str().unwrap_or("no connector"))).await?;
        Ok(json!({ "agent": self.agent, "back": true, "connector": rec["connector"] }))
    }

    /// Whether `connector` has a run of `op` in one of the `live` statuses:
    /// for its `intake`, one that has not published what its door
    /// delivered (queued or under way, held for a replay, or blocked),
    /// whose file its door must keep until it is replayed or let go.
    async fn unfinished(&self, connector: &str, op: &str, live: &[&str]) -> CellResult<bool> {
        // its op's runs counted by status, every one of them (never a
        // page: one held behind many newer is counted), in one read, so a
        // run replayed meanwhile is seen in one status or another
        let mut url = self.url.clone();
        url.set_query(Some(&format!("op={op}&limit=1")));
        let bare = Request::new(url.as_str(), Method::Get)?;
        let routed = Routed { name: connector.into(), url, signed: Some(self.person.clone()), credential: None };
        let mut r = crate::forward(self.env, &bare, None, crate::Forward { routed, inner: "/api/runs".into(), extra: vec![] }).await?;
        let runs = answer(&mut r, &format!("{connector}'s runs")).await?;
        Ok(live.iter().any(|s| runs["counts"][s].as_u64().unwrap_or(0) > 0))
    }
}

/// An answer's JSON, or its refusal as the platform's error: its code and
/// message, `what` saying whose.
async fn answer(r: &mut Response, what: &str) -> CellResult<Value> {
    let status = r.status_code();
    let text = r.text().await.unwrap_or_default();
    if (200..300).contains(&status) {
        return crate::json_text::<Value>(&text).map_err(|e| CellError::host(format!("{what}: {e}")));
    }
    let body: Value = crate::json_text(&text).unwrap_or(Value::Null);
    let said = body["error"].as_str().unwrap_or("");
    let code = crate::json_text::<ErrorCode>(&format!("{said:?}")).ok().unwrap_or(if status == 409 { ErrorCode::Conflict } else { ErrorCode::UpstreamFailed });
    Err(CellError::new(code, format!("{what}: {status} {said}: {}", body["message"].as_str().unwrap_or(&text))))
}
