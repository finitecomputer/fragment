//! Telegram's Bot API, for the Telegram connector's lane (templates/telegram):
//! `getMe`, `setWebhook` and `sendMessage` at `/bot<token>/<method>`, each
//! refused (401) for any token but its bot's. It keeps the webhook its bot
//! was given (its URL and `secret_token`: a lane delivers updates there as
//! Telegram would, the secret in its header), and records each message the
//! bot sent.

use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::http::{Handler, Request, Response, Server};

/// The bot's id and username, as `getMe` answers them.
pub const BOT_ID: i64 = 7_001_000;
pub const BOT_USERNAME: &str = "e2e_fragment_bot";

#[derive(Default)]
struct World {
    /// The webhook's URL and `secret_token`, as `setWebhook` set them last.
    webhook: Option<(String, String)>,
    /// What the bot sent: `{chat_id, text}`.
    sent: Vec<Value>,
}

pub struct Telegram {
    pub url: String,
    world: Arc<Mutex<World>>,
    _server: Server,
}

fn answer(result: Value) -> Response {
    Response::json(200, &json!({ "ok": true, "result": result }))
}

impl Telegram {
    /// Its bot's token is `token`.
    pub fn start(token: &str) -> std::io::Result<Telegram> {
        let world: Arc<Mutex<World>> = Arc::default();
        let w = Arc::clone(&world);
        let prefix = format!("/bot{token}/");
        let handler: Handler = Arc::new(move |req: &Request| {
            let Some(method) = req.path.strip_prefix(&prefix) else {
                return Response::json(401, &json!({ "ok": false, "error_code": 401, "description": "Unauthorized" }));
            };
            let body: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
            let param = |k: &str| req.query.get(k).cloned().or_else(|| body[k].as_str().map(str::to_string));
            match method {
                "getMe" => answer(json!({ "id": BOT_ID, "is_bot": true, "first_name": "Fragment", "username": BOT_USERNAME })),
                "setWebhook" => {
                    let (Some(url), Some(secret)) = (param("url"), param("secret_token")) else {
                        return Response::json(400, &json!({ "ok": false, "description": "Bad Request: url and secret_token" }));
                    };
                    w.lock().expect("telegram").webhook = Some((url, secret));
                    answer(json!(true))
                }
                "sendMessage" => {
                    let sent = json!({ "chat_id": body["chat_id"], "text": body["text"] });
                    let mut world = w.lock().expect("telegram");
                    world.sent.push(sent);
                    answer(json!({ "message_id": world.sent.len() }))
                }
                _ => Response::json(404, &json!({ "ok": false, "description": "Not Found" })),
            }
        });
        let server = Server::start(0, handler)?;
        Ok(Telegram { url: server.url.clone(), world, _server: server })
    }

    /// The webhook its bot was given last: its URL and secret.
    pub fn webhook(&self) -> Option<(String, String)> {
        self.world.lock().expect("telegram").webhook.clone()
    }

    /// What the bot sent, in order.
    pub fn sent(&self) -> Vec<Value> {
        self.world.lock().expect("telegram").sent.clone()
    }

    /// A direct message, as an update: `text` from user `from`, in its
    /// private chat with the bot (whose id is the user's).
    pub fn message(update_id: i64, from: i64, name: &str, text: &str) -> Value {
        json!({ "update_id": update_id, "message": {
            "message_id": update_id, "date": 1_790_000_000,
            "from": { "id": from, "is_bot": false, "first_name": name },
            "chat": { "id": from, "type": "private", "first_name": name },
            "text": text,
        } })
    }
}
