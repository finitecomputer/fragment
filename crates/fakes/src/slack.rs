//! Slack's Web API, for the Slack connector's lane (templates/slack):
//! `auth.test`, `users.info`, `conversations.info`, `conversations.history`,
//! `conversations.replies`, `chat.postMessage`, `reactions.add` and
//! `reactions.remove` at `/api/<method>`, each refused (`invalid_auth`) for
//! any bearer token but its bot's. It knows its people and a channel's
//! history as the lane sets them, and records each message the bot sent and
//! each reaction, in order. A lane delivers events to the connector's inbox
//! as Slack would (`event`, `dm`, `mention`), signed by its own hand.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::http::{Handler, Request, Response, Server};

/// Its workspace, and its bot user, as `auth.test` answers them.
pub const TEAM: &str = "T0E2ETEAM";
pub const TEAM_NAME: &str = "E2E Team";
pub const BOT_USER: &str = "U0E2EBOT";
pub const BOT_NAME: &str = "juniper";

#[derive(Default)]
struct World {
    /// Each person's id and display name.
    people: BTreeMap<String, String>,
    /// Each channel's name, and its messages, newest first, as Slack
    /// answers `conversations.history`.
    channels: BTreeMap<String, (String, Vec<Value>)>,
    /// What the bot sent: `{channel, markdown_text, thread_ts}`.
    sent: Vec<Value>,
    /// Its reactions: `{method, channel, timestamp, name}`.
    reactions: Vec<Value>,
}

pub struct Slack {
    pub url: String,
    world: Arc<Mutex<World>>,
    _server: Server,
}

fn ok(mut answer: Value) -> Response {
    answer["ok"] = json!(true);
    Response::json(200, &answer)
}

fn refused(error: &str) -> Response {
    Response::json(200, &json!({ "ok": false, "error": error }))
}

impl Slack {
    /// Its bot's token is `token`.
    pub fn start(token: &str) -> std::io::Result<Slack> {
        let world: Arc<Mutex<World>> = Arc::default();
        let w = Arc::clone(&world);
        let bearer = format!("Bearer {token}");
        let handler: Handler = Arc::new(move |req: &Request| {
            let Some(method) = req.path.strip_prefix("/api/") else {
                return Response::json(404, &json!({ "ok": false, "error": "unknown_method" }));
            };
            if req.header("authorization") != Some(bearer.as_str()) {
                return refused("invalid_auth");
            }
            let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
            let param = |k: &str| req.query.get(k).cloned().or_else(|| body[k].as_str().map(str::to_string)).unwrap_or_default();
            let mut world = w.lock().expect("slack");
            match method {
                "auth.test" => ok(json!({ "team_id": TEAM, "team": TEAM_NAME, "user_id": BOT_USER, "user": BOT_NAME, "bot_id": "B0E2EBOT" })),
                "users.info" => match world.people.get(&param("user")) {
                    Some(name) => ok(json!({ "user": { "id": param("user"), "name": name.to_lowercase(), "profile": { "display_name": name } } })),
                    None => refused("user_not_found"),
                },
                "conversations.info" => match world.channels.get(&param("channel")) {
                    Some((name, _)) => ok(json!({ "channel": { "id": param("channel"), "name": name } })),
                    None => refused("channel_not_found"),
                },
                "conversations.history" => match world.channels.get(&param("channel")) {
                    Some((_, messages)) => ok(json!({ "messages": messages, "has_more": false })),
                    None => refused("channel_not_found"),
                },
                "conversations.replies" => {
                    let ts = param("ts");
                    let Some((_, messages)) = world.channels.get(&param("channel")) else { return refused("channel_not_found") };
                    let mut thread: Vec<Value> = messages.iter().filter(|m| m["ts"] == ts.as_str() || m["thread_ts"] == ts.as_str()).cloned().collect();
                    thread.reverse();
                    ok(json!({ "messages": thread, "has_more": false }))
                }
                "chat.postMessage" => {
                    world.sent.push(json!({ "channel": body["channel"], "markdown_text": body["markdown_text"], "thread_ts": body["thread_ts"] }));
                    ok(json!({ "channel": body["channel"], "ts": format!("1790009{:03}.000100", world.sent.len()) }))
                }
                "reactions.add" | "reactions.remove" => {
                    world.reactions.push(json!({ "method": method, "channel": body["channel"], "timestamp": body["timestamp"], "name": body["name"] }));
                    ok(json!({}))
                }
                _ => Response::json(404, &json!({ "ok": false, "error": "unknown_method" })),
            }
        });
        let server = Server::start(0, handler)?;
        Ok(Slack { url: server.url.clone(), world, _server: server })
    }

    /// A person of its workspace, `id` named `name`.
    pub fn person(&self, id: &str, name: &str) {
        self.world.lock().expect("slack").people.insert(id.into(), name.into());
    }

    /// A channel, `id` named `name`, its messages as `conversations.history`
    /// answers them (newest first).
    pub fn channel(&self, id: &str, name: &str, messages: Vec<Value>) {
        self.world.lock().expect("slack").channels.insert(id.into(), (name.into(), messages));
    }

    /// What the bot sent, in order.
    pub fn sent(&self) -> Vec<Value> {
        self.world.lock().expect("slack").sent.clone()
    }

    /// Its reactions, added and removed, in order.
    pub fn reactions(&self) -> Vec<Value> {
        self.world.lock().expect("slack").reactions.clone()
    }

    /// An Events API delivery of `event`, from its workspace.
    pub fn event(event: Value) -> Value {
        let id = event["ts"].as_str().unwrap_or("0").replace('.', "");
        json!({ "token": "deprecated", "team_id": TEAM, "api_app_id": "A0E2E", "type": "event_callback", "event_id": format!("Ev{id}"), "event_time": 1_790_000_000, "event": event })
    }

    /// A DM to the bot: `text` from `user`, in their DM with it, `D<user>`.
    pub fn dm(user: &str, text: &str, ts: &str) -> Value {
        Slack::event(json!({ "type": "message", "channel_type": "im", "channel": format!("D{user}"), "user": user, "text": text, "ts": ts }))
    }

    /// A message in `channel` that mentions the bot, as `app_mention`.
    pub fn mention(channel: &str, user: &str, text: &str, ts: &str) -> Value {
        Slack::event(json!({ "type": "app_mention", "channel": channel, "user": user, "text": format!("<@{BOT_USER}> {text}"), "ts": ts }))
    }

    /// Slack's check of an app's Request URL.
    pub fn url_check(challenge: &str) -> Value {
        json!({ "token": "deprecated", "challenge": challenge, "type": "url_verification" })
    }
}
