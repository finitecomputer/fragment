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
//!   from where the last one stopped:
//!   1. **made**: the connector, the person's own (made and bound as
//!      Settings' Set up does, as the person: the platform makes a
//!      fragment for its person, a first, each logged as `simplex-move`),
//!      its inbox's secret set;
//!   2. **imported**: its computer's link handed over (its core stopped for
//!      good there: `computer/simplex-hand`), the answer that door's first
//!      save, streamed into the door (`/door/import`, its SHA-256 checked
//!      as it is stored), then the agent's computer door closed;
//!   3. **connecting**: the connector's `connect`, as the person: its door
//!      starts from that save;
//!   4. **done**: its door's runner linked, its connector told so (its
//!      `check`), and the person's old SimpleX chat archived (kept, its
//!      history searchable).
//!
//!   A person's SimpleX chat with their old agent, staged on their
//!   computer by finite.computer's move and never kept there, moves as
//!   staged (**offered** after its import): never kept on their behalf, its
//!   connector's setup page asks them to keep it or start a new chat.
//! - `back`: the way back from wherever it stopped: its door's newest save
//!   (its planned stop saves last) placed on its computer, which takes its
//!   door key again and runs its core; then the connector deleted (its door
//!   ends with it), the old chat unarchived, the record forgotten.
//!
//! Refused with its reason, nothing changed: an agent not on the person's
//! computer, a link not bound, a deployment that runs no doors; its
//! computer's bridge refuses a link busy (its agent working in that chat,
//! files or messages on their way) or one too large for a door's save.

use fragment_proto::{CreateFragment, Created, ErrorCode, Identity, IdentityKind};
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

    async fn set(&self, record: Value) -> CellResult<Value> {
        crate::computer::ask(self.env, &self.computer, "computer/simplex-moves", &json!({ "op": "set", "agent": self.agent, "record": record })).await
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

    async fn plan(&self) -> CellResult<Value> {
        let read = self.read().await?;
        let next = match text(&read["record"], "step") {
            "" => "make its connector, bind it and hand its link over",
            "binding" => "bind its connector and hand its link over",
            "made" => "hand its link over",
            "imported" => "connect its door",
            "connecting" => "wait for its door to link",
            "offered" => "nothing: its person keeps it, or starts a new chat, on its connector's setup page",
            "done" => "nothing: moved",
            _ => "its record says a step this release does not know",
        };
        Ok(json!({ "agent": self.agent, "computer": self.computer, "awake": read["awake"], "link": read["link"]["said"], "record": read["record"], "next": next, "blocked": self.blocked(&read) }))
    }

    /// Why a move that has not begun may not: its computer asleep (its
    /// handover wakes it), or its link not one its computer holds bound.
    fn blocked(&self, read: &Value) -> Option<String> {
        if !read["record"].is_null() {
            return None;
        }
        if read["awake"] != true {
            return Some("its computer is asleep: `go` wakes it, then ask again".into());
        }
        let said = &read["link"]["said"];
        let movable = said["phase"] == "linked" || (said["phase"] == "off" && said["staged"] == true);
        (read["link"]["status"] != 200 || !movable).then(|| format!("its computer holds no bound SimpleX link for it, nor a staged chat: {said}"))
    }

    async fn go(&self) -> CellResult<Value> {
        let mut read = self.read().await?;
        if read["record"].is_null() {
            if read["awake"] != true {
                // its handover's call wakes it: asked so, it says it is waking
                let woke = self.hand("handover", None, false).await;
                return Err(woke.err().unwrap_or_else(|| CellError::new(ErrorCode::Conflict, "its computer was asleep: ask again")));
            }
            if let Some(why) = self.blocked(&read) {
                return Err(CellError::new(ErrorCode::Conflict, why));
            }
            read["record"] = self.make().await?;
        }
        let mut rec = read["record"].clone();
        if text(&rec, "step") == "binding" {
            rec = self.bind(&rec).await?;
        }
        if text(&rec, "step") == "made" {
            rec = self.hand_over(&rec).await?;
        }
        if text(&rec, "step") == "imported" {
            self.as_person(text(&rec, "connector"), Method::Post, "/api/ops/connect", Some(&json!({ "id": "simplex-move-connect", "input": { "name": label_of(&self.agent) } }))).await?;
            rec = self.set(json!({ "step": "connecting" })).await?;
        }
        if text(&rec, "step") == "connecting" {
            let state = self.door_state(text(&rec, "door")).await?;
            if state["runner"]["phase"] != "linked" {
                return Ok(json!({ "agent": self.agent, "record": rec, "next": "wait for its door to link, then ask again", "door": { "running": state["running"], "runner": state["runner"]["phase"] } }));
            }
            // its connector told whom its door is linked to, as its page asks
            let connector = text(&rec, "connector").to_string();
            self.as_person(&connector, Method::Post, "/api/ops/check", Some(&json!({ "id": "simplex-move-check", "input": {} }))).await?;
            if let Some(chat) = rec["chat"].as_str() {
                crate::principal::ask(self.env, &self.person.id, &crate::principal::calls::SetOwn { fragment: chat.into(), own: crate::principal::calls::Own::Archived, on: true }).await?;
            }
            rec = self.set(json!({ "step": "done" })).await?;
            self.note(format!("{}'s SimpleX moved into {connector}; {} archived", self.agent, rec["chat"].as_str().unwrap_or("no old chat"))).await?;
        }
        let next = match text(&rec, "step") {
            "done" => "nothing: moved",
            "offered" => "nothing: its person keeps it, or starts a new chat, on its connector's setup page",
            _ => "ask again",
        };
        Ok(json!({ "agent": self.agent, "record": rec, "next": next }))
    }

    /// Its connector: made as the person's own from the SimpleX template,
    /// on a deployment whose doors run, recorded as it is made, so no `go`
    /// again makes a second; then bound (`bind`).
    async fn make(&self) -> CellResult<Value> {
        let label = fragment_core::door::connector_label(label_of(&self.agent), TEMPLATE, fragment_proto::label_room(self.cfg.host_label_suffix()));
        let create = CreateFragment { name: String::new(), label: Some(label), visibility: None, template: Some(TEMPLATE.into()), title: None, agent: None };
        let mut made = crate::create_fragment(self.env, self.cfg, self.url, create, self.person.clone()).await?;
        let made: Created = answered(&mut made, "its connector's making").await?;
        let door = fragment_core::door::door_id(&made.name, &made.npub).ok_or_else(|| CellError::host(format!("{} names no door", made.name)))?;
        let rec = self.set(json!({ "connector": made.name, "door": door, "step": "binding" })).await?;
        if self.door_state(&door).await?["doors"] != true {
            self.as_person(&made.name, Method::Delete, "/delete", None).await?;
            self.forget().await?;
            return Err(CellError::new(ErrorCode::Conflict, "this deployment runs no SimpleX doors: nothing moved"));
        }
        self.note(format!("made {} for {}'s SimpleX", made.name, self.agent)).await?;
        Ok(rec)
    }

    /// Its connector bound as the agent's door, as its person would bind it
    /// in Settings, and its inbox's secret set: each the same asked again.
    async fn bind(&self, rec: &Value) -> CellResult<Value> {
        let connector = text(rec, "connector");
        self.as_person(connector, Method::Post, "/api/door", Some(&json!({ "act": "link", "agent": self.agent }))).await?;
        let secret = hex::encode(crate::js::random_bytes::<32>());
        self.as_person(connector, Method::Put, &format!("/api/secrets/{SECRET}"), Some(&json!({ "value": secret, "hosts": [DOOR_HOST] }))).await?;
        self.set(json!({ "step": "made" })).await
    }

    /// Its computer's link handed over and its save given to its door, then
    /// the agent's computer door closed: what its computer signs relays no
    /// one now.
    async fn hand_over(&self, rec: &Value) -> CellResult<Value> {
        let save: worker::worker_sys::web_sys::Response = self.hand("handover", None, false).await?.into();
        let head = |k: &str| save.headers().get(k).ok().flatten().unwrap_or_default();
        let (sha, length, staged) = (head("x-door-sha256"), head("x-door-length"), head("x-simplex-staged") == "1");
        let body = save.body().map(Into::into);
        let headers = [("x-fragment-door", text(rec, "door")), ("x-door-agent", label_of(&self.agent)), ("x-door-sha256", sha.as_str()), ("x-door-length", length.as_str()), ("x-door-staged", if staged { "1" } else { "0" })];
        let mut put = crate::routed::send(self.env, "SIMPLEX_DOOR", text(rec, "door"), crate::routed::internal_stream("door/import", Method::Put, &headers, body)?).await?;
        answer(&mut put, "its door's import").await?;
        crate::fragment::ask(self.env, &self.agent, "computer/simplex-move", &json!({ "computer": self.computer, "act": "closed" })).await?;
        self.note(format!("{}'s SimpleX {} handed over to {}'s door ({sha}, {length} bytes)", self.agent, if staged { "staged chat" } else { "link" }, text(rec, "connector"))).await?;
        // a staged chat waits for its person's word, never connected for them
        self.set(json!({ "step": if staged { "offered" } else { "imported" }, "staged": staged })).await
    }

    /// Its computer's half (`computer/simplex-hand`): a handover, or the
    /// way back with its door's save (`save`, `behind`).
    async fn hand(&self, act: &str, save: Option<worker::wasm_bindgen::JsValue>, behind: bool) -> CellResult<Response> {
        let headers = [("x-door-act", act), ("x-door-agent", self.agent.as_str()), ("x-door-behind", if behind { "1" } else { "0" })];
        let mut r = crate::routed::send(self.env, "COMPUTER", &self.computer, crate::routed::internal_stream("computer/simplex-hand", Method::Post, &headers, save)?).await?;
        if r.status_code() != 200 {
            answer(&mut r, &format!("its computer's {act}")).await?;
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
        if rec["moved"] == true {
            // its door stops, saving last, and hands its newest save back
            let door = text(&rec, "door");
            let mut held = crate::routed::send(self.env, "SIMPLEX_DOOR", door, crate::routed::internal_request("door/handback", &json!({ "door": door }).to_string())?).await?;
            let (save, behind) = match held.status_code() {
                200 => {
                    let raw: worker::worker_sys::web_sys::Response = held.into();
                    let head = |k: &str| raw.headers().get(k).ok().flatten().unwrap_or_default();
                    // a staged chat its door never ran (never kept, nor let go):
                    // its computer keeps it staged as it was, nothing placed;
                    // any other is placed, the save it was given too (its
                    // computer may have gone back to an older save since)
                    match rec["staged"] == true && head("x-door-started") == "0" {
                        true => (None, false),
                        false => (raw.body().map(Into::into), head("x-door-behind") == "1"),
                    }
                }
                204 => (None, false),
                _ => return Err(answer(&mut held, "its door's way back").await.err().unwrap_or_else(|| CellError::host("its door's way back"))),
            };
            self.hand("back", save, behind).await?;
            if let Some(chat) = rec["chat"].as_str() {
                crate::principal::ask(self.env, &self.person.id, &crate::principal::calls::SetOwn { fragment: chat.into(), own: crate::principal::calls::Own::Archived, on: false }).await?;
            }
        }
        if let Some(connector) = rec["connector"].as_str() {
            match self.as_person(connector, Method::Delete, "/delete", None).await {
                Err(e) if e.code != ErrorCode::NotFound => return Err(e),
                _ => {}
            }
        }
        self.forget().await?;
        self.note(format!("{}'s SimpleX went back to its computer; {} deleted", self.agent, rec["connector"].as_str().unwrap_or("no connector"))).await?;
        Ok(json!({ "agent": self.agent, "back": true, "connector": rec["connector"] }))
    }
}

/// An answer's JSON, or its refusal as the platform's error: its code and
/// message, `what` saying whose.
async fn answer(r: &mut Response, what: &str) -> CellResult<Value> {
    answered(r, what).await
}

/// `answer`, read as `T`.
async fn answered<T: serde::de::DeserializeOwned>(r: &mut Response, what: &str) -> CellResult<T> {
    let status = r.status_code();
    let text = r.text().await.unwrap_or_default();
    if (200..300).contains(&status) {
        return crate::json_text::<T>(&text).map_err(|e| CellError::host(format!("{what}: {e}")));
    }
    let body: Value = crate::json_text(&text).unwrap_or(Value::Null);
    let said = body["error"].as_str().unwrap_or("");
    let code = crate::json_text::<ErrorCode>(&format!("{said:?}")).ok().unwrap_or(if status == 409 { ErrorCode::Conflict } else { ErrorCode::UpstreamFailed });
    Err(CellError::new(code, format!("{what}: {status} {said}: {}", body["message"].as_str().unwrap_or(&text))))
}
