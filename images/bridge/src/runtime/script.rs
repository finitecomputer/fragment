//! The `script` runtime: a deterministic agent that answers from rules, so
//! the platform's lanes run with no agent runtime at all (the stub image,
//! and the proof that the platform holds nothing Hermes-specific). Each
//! turn is a pure function of its message. A rule is said by a word of it
//! that starts with the rule's (past an opening bracket, parenthesis or
//! quote: `slowly`, `[SILENT]`), never by text inside a word, so what a
//! command carries (a token, a URL) says no rule by chance (`says`):
//!
//! - default: two drafts, then the reply `echo: [<asker>] <text>`;
//! - `tool`: a tool call said before it runs (`turn.call`: `search`, a
//!   `web` one, the message's first 40 characters its args), its words
//!   before it (`Let me look.`), a reply of their own, then (`slow`:
//!   `CALL_PACES` paces on) its step (ok, an excerpt), then the reply;
//! - `approve` or `risky`: a step, then a prompt (`once`, `deny`) the
//!   owner answers; the reply says `(approved)`, `(denied)`, or
//!   `(not approved)` once it expired;
//! - `choose`: a prompt of choices (`c0` Basil, `c1` Mint) and `other`, answered
//!   in words, as Hermes' clarify asks it; the reply says `(chose: <the
//!   label, or the words>)`, or `(chose: nothing)` once it expired. `other`
//!   answered with no words asks "Type your answer:" as `ask-me` does; its
//!   owner's message while it is open is `other` with its words;
//! - `ask-me`: the reply's first part asks "What should I call it?", and the
//!   turn waits for the asker's next message (`Event::Asked`), then ends
//!   saying `(told: <their words>)` (files sent with an answer are lines of
//!   its words, each where it is: the driver's);
//! - `slow`: twenty drafts, `pace` apart, before the reply (for Stop);
//! - `fail`: the turn ends as an error;
//! - `silent`: the turn ends with no reply;
//! - `draw`: the reply carries a file (`drawing.txt`);
//! - `speak`: the reply carries a voice reply (`speech.mp3`, `SPOKEN_SECS`
//!   of silence as an MP3, as Hermes' `text_to_speech` makes one);
//! - `notice`: a notice before the reply (`info`, "Compressing context"),
//!   and one after the turn's end (`memory`, "Self-improvement review:
//!   Memory updated"), as Hermes sends its review once its turn is done;
//! - `fetch <http url> with <value> [in <header> | in query <param> | as
//!   basic <user>]`: the guest's own request, as any SDK sends one, with no
//!   header of ours: `<value>` is `$<NAME>`, the agent's credential in that
//!   environment variable as `GET /api/computer` lists it now, or a literal;
//!   it goes in a header (`authorization: Bearer …` unless named), a query
//!   parameter, or basic auth's password beside `<user>`. The reply is the
//!   answer's status and its first 300 characters (the computer's swap,
//!   decisions 22 and 37);
//! - `credentials`: what the agent's guest is given now (`GET
//!   /api/computer`): each credential's provider, its environment variables
//!   and its placeholder (and `models at <base>` for a provider that serves
//!   models), `; ` between;
//! - `write <path> <text>`: the text in the file `<path>` under the data
//!   root (`/data`), and `read <path>`: that file's text, or `none` (what a
//!   save kept, and left out);
//! - `think <text>`: one model call through the computer's model intercept
//!   (`$FRAGMENT_MODEL`, the cheap tier, as the agent): the reply is the
//!   model's answer; `think as <key> <text>` names its agent by its key
//!   alone, `Authorization: Bearer <key>`, as an OpenAI SDK does; `think on
//!   <model> <text>` names `<model>` (a model's id, or any name) instead of
//!   the cheap tier;
//! - `models`: what the intercept lists (`GET /v1/models`, no agent named:
//!   as Hermes' `/model` asks it) and what the computer's view names by the
//!   route's names (`GET /api/computer`'s `models`): the reply is `models:
//!   <ids, comma between>; given: <the view's, as JSON>`;
//! - `transcribe <words> [as <key>] [header <agent>]`: a voice memo that
//!   says `<words>` (a WAV the Workers AI fake reads them from) sent through
//!   the intercept as an OpenAI SDK sends one, `POST
//!   /v1/audio/transcriptions` with `model` `whisper`, its agent named by
//!   its key (`agent:<itself>` unless `as` names another) and, with
//!   `header`, by `x-fragment-agent` too: the reply is `heard: <the text>`,
//!   or the refusal's status and its first 300 characters;
//! - `decide <text>`: one decision through the intercept, `POST
//!   /v1/decide`, as a tool the agent built would make it, its agent named
//!   by its key (`agent:<itself>`): whether `<text>` is a snack, on
//!   clef-flash; `decide heard <words>`: whether a voice memo saying
//!   `<words>`, sent inline, is, on clef-omni. The reply is `decided:
//!   <model id> snack <probability>`, or `decide refused: <status> <its
//!   first 300 characters>`;
//! - `schedule add <minute> <hour> <day> <month> <weekday> <text>`,
//!   `schedule list`, `schedule rm <id>`: the agent's scheduled records on
//!   its own fragment through the platform, as the agent acting for its
//!   owner (`fragment schedule` on a computer; docs/api.md, "Scheduled
//!   records"), `add` a routine in this chat: the reply is `schedule added
//!   <id>` (`schedule had <id>`: the same one was there), `schedules: <id>
//!   <cron> by <who>; …`, `schedule removed <id>`, or `schedule refused:
//!   <the refusal>`;
//! - `door link <ticket>`, `door say <text>`, `door forge <text>`, `door
//!   other <text>`: an agent's door as SimpleX's is (fragment_core::door),
//!   for the platform's lanes: a link's ticket (its JSON, as the owner's
//!   Settings made it) checked as the bridge checks one, then redeemed with
//!   a door key the script makes (kept while it runs); a relayed record
//!   that key signs, posted on this chat; one with no door's signature
//!   (only the agent's own key's, as every request of its is); one signed
//!   by a key that is no door's; `door bind <fragment>`: the agent asking,
//!   for its owner, to bind that connector as its own door; `door poke
//!   <fragment> <secret|files|token|seen|propose|visibility|invite|rotate|status>
//!   [itself]`: the agent asking, for its owner (or as itself), to do an
//!   editor's or a sharer's thing there (`Api::poke`). The reply is `door
//!   linked`, `door said <seq>`, `door bound`, `door poked <what>` (and
//!   `, its share link shown` when the answer holds it), or `door
//!   refused: <why>`;
//! - `ask <agent> <words>`: another agent of its computer asked `<words>`
//!   with the fragment CLI, `fragment ask --return-to <this chat>`, as the
//!   agent acting for its owner, as our Hermes image's `fragment_ask` runs
//!   it: the question posted in the two agents' chat with their owner, and
//!   its answer back here once that agent's turn is over, as a turn of this
//!   agent's own (docs/chat-records.md, "An ask"). Its post id is the
//!   turn's, so a turn read again asks once. The reply: `asked <its name>`,
//!   or `ask refused: <why>`;
//! - a message with attachments: the reply names them;
//! - a turn told something first (`TurnStart::note`: its agent's turn before
//!   it in the chat was cut by a restart): the reply ends with
//!   `\n\n(told: <the note>)`;
//! - `@<name>` of another agent in the reply's text hands off to it, as
//!   any reply's does (the bridge reads mentions, not this runtime);
//! - `steer-me`: the turn drafts `waiting to be steered…` and waits (at
//!   most `STEER_WAIT`) for a `/steer` said beside it (`Command::Aside`),
//!   then its reply ends `(steered: <its words>)`, or `(steered: nothing)`;
//! - a message quoting another (`TurnStart::quote`): the reply ends
//!   `(quoting: <its text>)`, and `(quoting itself: …)` for its own;
//! - a command of its menu (`MENU`): the reply `ran /<name> <args>`, no
//!   drafts; one said beside a turn that is not a `/steer` into it (`/btw`)
//!   is answered as a message of its own, `aside: <the command>`.
//!
//! A Stop ends the turn as `stopped` at its next draft.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::records::{Call, Category, NoticeCategory, Outcome, PromptOption, Step};
use crate::runtime::{Command, Event, How, LocalFile, MenuItem, Runtime, RuntimeFuture, RuntimeIo, TurnStart};

/// How long `speak`'s voice reply plays.
pub const SPOKEN_SECS: u32 = 3;

/// The scripted agent's commands: one of each way the bridge carries one
/// (`How`), so the platform's lanes run each with no agent runtime.
pub const MENU: &[MenuItem] = &[
    MenuItem { name: "usage", description: "Say what it has done", args: None, how: How::Turn, refuse: &[] },
    MenuItem { name: "model", description: "Show or change the model", args: Some("Model name"), how: How::Turn, refuse: &[("--global", "this chat's alone")] },
    MenuItem { name: "new", description: "Start over in this chat", args: None, how: How::Restart, refuse: &[] },
    MenuItem { name: "stop", description: "Stop what it is doing here", args: None, how: How::Stop, refuse: &[] },
    MenuItem { name: "steer", description: "Tell the running turn something", args: Some("What to tell it"), how: How::Steer, refuse: &[] },
    MenuItem { name: "queue", description: "Queue a message for its next turn", args: Some("The message"), how: How::Message, refuse: &[] },
    MenuItem { name: "btw", description: "Ask something beside what it does", args: Some("The question"), how: How::Aside, refuse: &[] },
];

/// How long a `steer-me` turn waits for its `/steer`.
const STEER_WAIT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct ScriptConfig {
    /// The wait between a turn's drafts.
    pub pace: Duration,
    /// Where files it writes go (scratch).
    pub scratch: PathBuf,
    /// What `write` and `read` name paths under (`/data`: what a computer
    /// keeps).
    pub data: PathBuf,
}

/// A path `write` and `read` take: relative, at most eight parts of
/// letters, digits and `._-`, none of them `.` or `..`.
fn data_path(root: &std::path::Path, rel: &str) -> Option<PathBuf> {
    let parts: Vec<&str> = rel.split('/').collect();
    let part_ok = |p: &&str| !p.is_empty() && *p != "." && *p != ".." && p.len() <= 64 && p.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    (parts.len() <= 8 && parts.iter().all(part_ok)).then(|| root.join(rel))
}

/// `write <path> <text>`: the text in a file under the data root.
async fn write_data(root: &std::path::Path, rest: &str) -> String {
    let (rel, text) = rest.split_once(' ').unwrap_or((rest, ""));
    let Some(path) = data_path(root, rel) else { return format!("write refused: {rel:?} is no path under the data") };
    let wrote = async {
        if let Some(dir) = path.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        tokio::fs::write(&path, text.as_bytes()).await
    };
    match wrote.await {
        Ok(()) => format!("wrote {rel}"),
        Err(e) => format!("write failed: {e}"),
    }
}

/// `read <path>`: a file's text under the data root, or `none`.
async fn read_data(root: &std::path::Path, rel: &str) -> String {
    let Some(path) = data_path(root, rel) else { return format!("read refused: {rel:?} is no path under the data") };
    match tokio::fs::read_to_string(&path).await {
        Ok(text) => format!("read {rel}: {}", text.chars().take(300).collect::<String>()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => format!("read {rel}: none"),
        Err(e) => format!("read failed: {e}"),
    }
}

pub struct Script {
    pub config: ScriptConfig,
}

impl Runtime for Script {
    fn name(&self) -> &'static str {
        "script"
    }

    fn menu(&self) -> &'static [MenuItem] {
        MENU
    }

    fn run(self: Box<Self>, io: RuntimeIo) -> RuntimeFuture {
        Box::pin(run(self.config, io))
    }
}

/// What a running scripted turn hears from the bridge.
enum Heard {
    Stop,
    /// The option (none: expired), and the words of one answered in words.
    Answer(Option<String>, Option<String>),
    Told(String),
    /// A `/steer` said beside it: its words.
    Steered(String),
}

async fn run(cfg: ScriptConfig, mut io: RuntimeIo) -> Result<(), crate::runtime::RuntimeError> {
    let _ = std::fs::create_dir_all(&cfg.scratch);
    let mut turns: HashMap<String, mpsc::Sender<Heard>> = HashMap::new();
    let mut shutdown = io.shutdown.clone();
    // In process, it can take a turn from its start.
    let _ = io.events.send(Event::Connected(true)).await;
    // bounded by the bridge's life: one command per pass
    loop {
        let c = tokio::select! {
            c = io.commands.recv() => c,
            _ = crate::net::stopped(&mut shutdown) => return Ok(()),
        };
        let Some(c) = c else { return Ok(()) };
        turns.retain(|_, tx| !tx.is_closed());
        match c {
            Command::Start(ts) => {
                let (tx, rx) = mpsc::channel(8);
                turns.insert(ts.turn.clone(), tx);
                tokio::spawn(turn(cfg.clone(), *ts, rx, io.events.clone()));
            }
            Command::Stop { turn } => {
                if let Some(tx) = turns.get(&turn) {
                    let _ = tx.send(Heard::Stop).await;
                }
            }
            Command::Answer { turn, option, words, .. } => {
                if let Some(tx) = turns.get(&turn) {
                    let _ = tx.send(Heard::Answer(option, words)).await;
                }
            }
            Command::Forget { turn } => {
                turns.remove(&turn);
            }
            Command::Tell { turn, text, .. } => {
                if let Some(tx) = turns.get(&turn) {
                    let _ = tx.send(Heard::Told(text)).await;
                }
            }
            Command::Aside { agent, fragment, turn, text, .. } => {
                let steering = text.strip_prefix("/steer ").zip(turn.as_ref().and_then(|t| turns.get(t)));
                match steering {
                    Some((words, tx)) => {
                        let _ = tx.send(Heard::Steered(words.to_string())).await;
                    }
                    None => {
                        let _ = io.events.send(Event::Say { agent, fragment, text: format!("aside: {text}"), files: Vec::new() }).await;
                    }
                }
            }
        }
    }
}

/// Where `fetch` puts its value.
#[derive(Debug, PartialEq, Eq)]
enum Place<'a> {
    Header(&'a str),
    Query(&'a str),
    Basic(&'a str),
}

/// `fetch`'s words: the URL, the value (`$NAME` or a literal), and where it goes.
fn fetch_words(text: &str) -> Result<(&str, &str, Place<'_>), String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    match words.as_slice() {
        [_, url, "with", v] => Ok((url, v, Place::Header("authorization"))),
        [_, url, "with", v, "in", "query", q] => Ok((url, v, Place::Query(q))),
        [_, url, "with", v, "as", "basic", user] => Ok((url, v, Place::Basic(user))),
        [_, url, "with", v, "in", h] => Ok((url, v, Place::Header(h))),
        _ => Err("say `fetch <http url> with <$NAME or a value> [in <header> | in query <param> | as basic <user>]`".into()),
    }
}

/// `agent`'s credentials, as `GET /api/computer` lists them now.
async fn credentials_of(agent: &str) -> Result<Vec<crate::runtime::Credential>, String> {
    let api = std::env::var("FRAGMENT_API").map_err(|_| "no FRAGMENT_API".to_string())?;
    let computer = crate::api::Api::new(&api)?.computer(false).await.map_err(|e| format!("GET /api/computer: {e}"))?;
    let a = computer.agents.into_iter().find(|a| a.fragment == agent).ok_or_else(|| format!("{agent} is not on this computer"))?;
    Ok(a.credentials)
}

/// The placeholder in `name` for `agent` (what an image puts in that
/// environment variable).
async fn credential(agent: &str, name: &str) -> Result<String, String> {
    let all = credentials_of(agent).await?;
    all.iter().find(|c| c.env.iter().any(|e| e == name)).map(|c| c.placeholder.clone()).ok_or_else(|| format!("no credential of {agent}'s is in ${name}"))
}

/// `fetch <url> with <value> [in <header> | in query <param> | as basic
/// <user>]`, as `agent`'s guest sends it (no header of ours): the answer's
/// status and body.
async fn fetch(agent: &str, text: &str) -> Result<(u16, String), String> {
    use base64::Engine;
    use http_body_util::{BodyExt, Empty, Limited};
    use hyper_util::client::legacy::Client;
    use hyper_util::rt::TokioExecutor;

    let (url, value, place) = fetch_words(text)?;
    let value = match value.strip_prefix('$') {
        Some(name) => credential(agent, name).await?,
        None => value.to_string(),
    };
    let (url, header, header_value) = match place {
        Place::Header("authorization") => (url.to_string(), "authorization", format!("Bearer {value}")),
        Place::Header(h) => (url.to_string(), h, value),
        Place::Query(q) => (format!("{url}{}{q}={value}", if url.contains('?') { '&' } else { '?' }), "accept", "*/*".to_string()),
        Place::Basic(user) => (url.to_string(), "authorization", format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{user}:{value}")))),
    };
    let base = crate::net::Base::parse(&url)?;
    let req = hyper::Request::get(&url)
        .header("host", base.authority())
        .header(header, header_value)
        .body(Empty::<bytes::Bytes>::new())
        .map_err(|e| e.to_string())?;
    let client = Client::builder(TokioExecutor::new()).build_http();
    let call = async {
        let res = client.request(req).await.map_err(|e| e.to_string())?;
        let status = res.status().as_u16();
        let body = Limited::new(res.into_body(), 64 * 1024).collect().await.map_err(|e| e.to_string())?.to_bytes();
        Ok::<_, String>((status, String::from_utf8_lossy(&body).into_owned()))
    };
    tokio::time::timeout(Duration::from_millis(crate::limits::HTTP_TIMEOUT_MS), call).await.map_err(|_| format!("{url}: no answer"))?
}

/// One model call, `said` as the user's message, on `on` (a model's id or
/// a name; the cheap tier unless named), as `agent`, named by its
/// `x-fragment-agent`, or by `key` alone (`Authorization: Bearer <key>`):
/// the answer's text.
async fn think(agent: &str, key: Option<&str>, on: Option<&str>, said: &str) -> Result<String, String> {
    use http_body_util::{BodyExt, Full, Limited};
    use hyper_util::client::legacy::Client;
    use hyper_util::rt::TokioExecutor;

    let model = std::env::var("FRAGMENT_MODEL").map_err(|_| "no FRAGMENT_MODEL".to_string())?;
    let base = crate::net::Base::parse(&model)?;
    let body = serde_json::json!({ "model": on.unwrap_or("cheap"), "messages": [{ "role": "user", "content": said }] }).to_string();
    let named = match key {
        Some(k) => ("authorization", format!("Bearer {k}")),
        None => ("x-fragment-agent", agent.to_string()),
    };
    let req = hyper::Request::post(base.url("/v1/chat/completions"))
        .header("host", base.authority())
        .header(named.0, named.1)
        .header("content-type", "application/json")
        .header("content-length", body.len())
        .body(Full::new(bytes::Bytes::from(body)))
        .map_err(|e| e.to_string())?;
    let client = Client::builder(TokioExecutor::new()).build_http();
    let call = async {
        let res = client.request(req).await.map_err(|e| e.to_string())?;
        let status = res.status().as_u16();
        let bytes = Limited::new(res.into_body(), 256 * 1024).collect().await.map_err(|e| e.to_string())?.to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
        match v["choices"][0]["message"]["content"].as_str() {
            Some(answer) if status == 200 => Ok(answer.to_string()),
            _ => Err(format!("{status} {}", String::from_utf8_lossy(&bytes).chars().take(300).collect::<String>())),
        }
    };
    tokio::time::timeout(Duration::from_millis(crate::limits::HTTP_TIMEOUT_MS), call).await.map_err(|_| "the model did not answer".to_string())?
}

/// What `schedule …` asks: `add` (its cron's five fields, then its text),
/// `list`, or `rm <id>`.
#[derive(Debug, PartialEq)]
enum ScheduleAsk {
    Add { cron: String, text: String },
    List,
    Rm(u64),
    Unsaid,
}

fn schedule_words(rest: &str) -> ScheduleAsk {
    let words: Vec<&str> = rest.split_whitespace().collect();
    match words.as_slice() {
        ["add", m, h, d, mo, w, text @ ..] if !text.is_empty() => ScheduleAsk::Add { cron: [*m, *h, *d, *mo, *w].join(" "), text: text.join(" ") },
        ["list"] => ScheduleAsk::List,
        ["rm", id] => id.parse().map_or(ScheduleAsk::Unsaid, ScheduleAsk::Rm),
        _ => ScheduleAsk::Unsaid,
    }
}

/// `schedule add <minute> <hour> <day> <month> <weekday> <text>`,
/// `schedule list`, `schedule rm <id>`: the agent's scheduled records on its
/// own fragment (docs/api.md, "Scheduled records"), through the platform as
/// the agent acting for its owner, as `fragment schedule` asks on a
/// computer; `add` a routine in this chat (`{kind: "routine", text, chat}`
/// on its `tasks`, as `--routine` builds it). The reply: `schedule added
/// <id>` (`schedule had <id>`: the same one was there), `schedules: <id>
/// <cron> by <who>; …`, `schedule removed <id>`, or `schedule refused: <the
/// platform's refusal>`.
async fn schedule(agent: &crate::runtime::Agent, chat: &str, rest: &str) -> String {
    let api = match std::env::var("FRAGMENT_API").map_err(|_| "no FRAGMENT_API".to_string()).and_then(|a| crate::api::Api::new(&a)) {
        Ok(api) => api,
        Err(e) => return format!("schedule failed: {e}"),
    };
    let (me, owner) = (agent.fragment.as_str(), agent.owner.as_str());
    let asked = match schedule_words(rest) {
        ScheduleAsk::Add { cron, text } => {
            let routine = serde_json::json!({ "kind": "routine", "text": text, "chat": chat });
            api.add_schedule(me, owner, me, &cron, "tasks", &routine).await.map(|v| {
                let id = &v["schedule"]["id"];
                if v["replayed"] == true { format!("schedule had {id}") } else { format!("schedule added {id}") }
            })
        }
        ScheduleAsk::List => api.schedules(me, owner, me).await.map(|v| {
            let listed: Vec<String> = v["schedules"].as_array().into_iter().flatten().map(|r| format!("{} {} by {}", r["id"], r["cron"].as_str().unwrap_or(""), r["createdBy"].as_str().unwrap_or(""))).collect();
            format!("schedules: {}", listed.join("; "))
        }),
        ScheduleAsk::Rm(id) => api.remove_schedule(me, owner, me, id).await.map(|v| format!("schedule removed {}", v["removed"])),
        ScheduleAsk::Unsaid => return "say `schedule add <minute> <hour> <day> <month> <weekday> <text>`, `schedule list` or `schedule rm <id>`".into(),
    };
    asked.unwrap_or_else(|e| format!("schedule refused: {e}"))
}

/// The fragment CLI the stub image carries (images/stub/Dockerfile).
const CLI: &str = "/usr/local/bin/fragment";
/// An ask is a few platform calls (who may be asked, their chat, made the
/// first time, its post): at most this long.
const ASK_WAIT: Duration = Duration::from_secs(60);

/// `ask <agent> <words>`: the module's list.
async fn ask_another(agent: &crate::runtime::Agent, chat: &str, turn: &str, rest: &str) -> String {
    let Some((who, words)) = rest.trim().split_once(char::is_whitespace) else {
        return "say `ask <agent> <words>`".into();
    };
    let run = tokio::process::Command::new(CLI)
        .args(["--json", "ask", "--return-to", chat, "--id", &format!("ask-{turn}"), "--", who, words.trim()])
        .env("FRAGMENT_AS_AGENT", &agent.fragment)
        .env("FRAGMENT_FOR", &agent.owner)
        .kill_on_drop(true)
        .output();
    let out = match tokio::time::timeout(ASK_WAIT, run).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => return format!("ask refused: the CLI did not run ({e})"),
        Err(_) => return format!("ask refused: no answer in {} s", ASK_WAIT.as_secs()),
    };
    let said: serde_json::Value = String::from_utf8_lossy(&out.stdout).lines().last().and_then(|l| serde_json::from_str(l).ok()).unwrap_or_default();
    if said["ok"] != true {
        let why = said["error"]["message"].as_str().map(str::to_string).unwrap_or_else(|| String::from_utf8_lossy(&out.stderr).chars().take(300).collect());
        return format!("ask refused: {why}");
    }
    let asked = &said["data"]["asked"];
    format!("asked {}", asked["title"].as_str().or(asked["name"].as_str()).unwrap_or(who))
}

/// The door keys `door link` made, by agent, for the process's life.
static DOORS: std::sync::Mutex<BTreeMap<String, crate::simplex::door::Key>> = std::sync::Mutex::new(BTreeMap::new());

/// `door …` (the module's list): an agent's door, as its lanes see it.
/// `relay`: the platform's XFTP relay (`GET /api/computer/relay`), as the
/// platform's lanes check it. `relay to <host>` opens one to `host` on
/// 443 and says whether it was refused; `relay forged <path>` asks
/// `<path>` of the platform as the agent, as an upgrade, and says how it
/// was answered (its status, how many headers of the relay's it kept, its
/// body): an answer that carried forged relay headers must come back as
/// itself, never piped.
async fn relay(agent: &str, rest: &str) -> Result<String, String> {
    use http_body_util::{BodyExt, Empty, Limited};
    use hyper_util::client::legacy::Client;
    use hyper_util::rt::TokioExecutor;
    let base = std::env::var("FRAGMENT_API").map_err(|_| "no FRAGMENT_API".to_string())?;
    let (verb, said) = rest.split_once(' ').unwrap_or((rest, ""));
    match verb {
        "to" => match crate::api::Api::new(&base)?.relay(said.trim(), 443).await {
            Ok(_) => Ok("relay opened".into()),
            Err(e) => Ok(format!("relay refused: {e}")),
        },
        "forged" => {
            let base = crate::net::Base::parse(&base)?;
            let req = hyper::Request::get(base.url(said.trim()))
                .header("host", base.authority())
                .header("x-fragment-agent", agent)
                .header("upgrade", "websocket")
                .header("connection", "Upgrade")
                .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
                .header("sec-websocket-version", "13")
                .body(Empty::<bytes::Bytes>::new())
                .map_err(|e| e.to_string())?;
            let client = Client::builder(TokioExecutor::new()).build_http();
            let call = async {
                let res = client.request(req).await.map_err(|e| e.to_string())?;
                let status = res.status().as_u16();
                let kept = res.headers().keys().filter(|k| k.as_str().starts_with("x-fragment-relay")).count();
                let body = if status == 101 { bytes::Bytes::new() } else { Limited::new(res.into_body(), 4096).collect().await.map_err(|e| e.to_string())?.to_bytes() };
                Ok::<_, String>(format!("relay answered {status}, its headers {kept}: {}", String::from_utf8_lossy(&body).chars().take(80).collect::<String>()))
            };
            tokio::time::timeout(Duration::from_millis(crate::limits::HTTP_TIMEOUT_MS), call).await.map_err(|_| "no answer".to_string())?
        }
        _ => Err("say `relay to <host>` or `relay forged <path>`".into()),
    }
}

async fn door(agent: &crate::runtime::Agent, chat: &str, rest: &str) -> Result<String, String> {
    use crate::simplex::door::{self, Key, Ticket};
    let api = crate::api::Api::new(&std::env::var("FRAGMENT_API").map_err(|_| "no FRAGMENT_API".to_string())?)?;
    let now = crate::log::now_ms() as i64;
    let (verb, said) = rest.split_once(' ').unwrap_or((rest, ""));
    if verb == "bind" {
        api.bind_door(&agent.fragment, &agent.owner, said.trim()).await.map_err(|e| e.to_string())?;
        return Ok("door bound".into());
    }
    if verb == "poke" {
        let words: Vec<&str> = said.split_whitespace().collect();
        let [fragment, what, rest @ ..] = &words[..] else { return Err("say `door poke <fragment> <secret|files|token|seen|propose|visibility|invite|rotate|status> [itself]`".into()) };
        let answer = api.poke(&agent.fragment, &agent.owner, fragment, what, rest == ["itself"]).await.map_err(|e| e.to_string())?;
        let link = if answer["viewToken"].is_string() { ", its share link shown" } else { "" };
        return Ok(format!("door poked {what}{link}"));
    }
    if verb == "link" {
        let ticket: Ticket = serde_json::from_str(said.trim()).map_err(|e| format!("not a ticket: {e}"))?;
        let computer = api.computer(false).await.map_err(|e| e.to_string())?.computer;
        door::check(&ticket, "link", &agent.fragment, &computer, agent.key.as_deref().unwrap_or(""), now).map_err(str::to_string)?;
        let key = Key::generate().map_err(|e| e.to_string())?;
        api.door(&agent.fragment, door::DOOR, &ticket.nonce, &key.public_hex()).await.map_err(|e| e.to_string())?;
        DOORS.lock().expect("not poisoned").insert(agent.fragment.clone(), key);
        return Ok("door linked".into());
    }
    let mut body = crate::records::relayed_message(said, agent, crate::records::VIA_SIMPLEX, &[]);
    let id = crate::records::relayed_id(&agent.fragment, &format!("{verb} {said} {now}"));
    match verb {
        "say" => {
            let key = DOORS.lock().expect("not poisoned").get(&agent.fragment).cloned().ok_or("no door linked here")?;
            key.seal(&mut body, chat, crate::records::CHAT, &id, now);
        }
        "other" => Key::generate().map_err(|e| e.to_string())?.seal(&mut body, chat, crate::records::CHAT, &id, now),
        "forge" => body["relayed"]["at"] = serde_json::json!(now),
        _ => return Err("say `door link <ticket>`, `door bind <fragment>`, `door say <text>`, `door forge <text>` or `door other <text>`".into()),
    }
    let posted = api.post(&agent.fragment, chat, crate::records::CHAT, &id, &body).await.map_err(|e| e.to_string())?;
    Ok(format!("door said {}", posted.seq.unwrap_or(0)))
}

/// `models`: the ids the intercept lists (`GET /v1/models`, naming no
/// agent, as Hermes' `/model` asks it), and the computer's view's.
async fn models() -> Result<String, String> {
    use http_body_util::{BodyExt, Empty, Limited};
    use hyper_util::client::legacy::Client;
    use hyper_util::rt::TokioExecutor;

    let model = std::env::var("FRAGMENT_MODEL").map_err(|_| "no FRAGMENT_MODEL".to_string())?;
    let base = crate::net::Base::parse(&model)?;
    let api = std::env::var("FRAGMENT_API").map_err(|_| "no FRAGMENT_API".to_string())?;
    let view = crate::api::Api::new(&api)?.computer(false).await.map_err(|e| format!("GET /api/computer: {e}"))?;
    let given = serde_json::to_string(&view.models).expect("ids serialize");
    let req = hyper::Request::get(base.url("/v1/models")).header("host", base.authority()).body(Empty::<bytes::Bytes>::new()).map_err(|e| e.to_string())?;
    let client = Client::builder(TokioExecutor::new()).build_http();
    let call = async {
        let res = client.request(req).await.map_err(|e| e.to_string())?;
        let status = res.status().as_u16();
        let bytes = Limited::new(res.into_body(), 64 * 1024).collect().await.map_err(|e| e.to_string())?.to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
        let ids: Vec<&str> = v["data"].as_array().into_iter().flatten().filter_map(|m| m["id"].as_str()).collect();
        match (status, v["object"].as_str()) {
            (200, Some("list")) => Ok(format!("{}; given: {given}", ids.join(", "))),
            _ => Err(format!("{status} {}", String::from_utf8_lossy(&bytes).chars().take(300).collect::<String>())),
        }
    };
    tokio::time::timeout(Duration::from_millis(crate::limits::HTTP_TIMEOUT_MS), call).await.map_err(|_| "the intercept did not answer".to_string())?
}

/// `think [as <key>] [on <model>] <text>`: the key, the model, the text.
fn think_words(said: &str) -> (Option<&str>, Option<&str>, &str) {
    let (key, said) = match said.strip_prefix("as ").and_then(|r| r.split_once(' ')) {
        Some((key, said)) => (Some(key), said),
        None => (None, said),
    };
    match said.strip_prefix("on ").and_then(|r| r.split_once(' ')) {
        Some((on, said)) => (key, Some(on), said),
        None => (key, None, said),
    }
}

/// A second of silence, as a WAV, that says `words` in a chunk of its own
/// (`said`), which the Workers AI fake reads as what was spoken
/// (fragment_fakes::workers_ai::spoken_wav makes the same).
pub fn memo(words: &str) -> Vec<u8> {
    const RATE: u32 = 32_000;
    let mut said = words.as_bytes().to_vec();
    if said.len() % 2 == 1 {
        said.push(0);
    }
    let mut w = Vec::new();
    w.extend(b"RIFF");
    w.extend(((4 + 24 + 8 + said.len() + 8) as u32 + RATE).to_le_bytes());
    w.extend(b"WAVEfmt ");
    w.extend(16u32.to_le_bytes());
    w.extend([1, 0, 1, 0]);
    w.extend(16_000u32.to_le_bytes());
    w.extend(RATE.to_le_bytes());
    w.extend([2, 0, 16, 0]);
    w.extend(b"said");
    w.extend((words.len() as u32).to_le_bytes());
    w.extend(&said);
    w.extend(b"data");
    w.extend(RATE.to_le_bytes());
    w.resize(w.len() + RATE as usize, 0);
    w
}

/// `transcribe <words> [as <key>] [header <agent>]`: the words, the key
/// (`agent:<agent>` unless named) and the header's agent, if any.
fn transcribe_words<'a>(rest: &'a str, agent: &str) -> (&'a str, String, Option<&'a str>) {
    let (rest, header) = match rest.split_once(" header ") {
        Some((r, h)) => (r, Some(h.trim())),
        None => (rest, None),
    };
    match rest.split_once(" as ") {
        Some((words, key)) => (words.trim(), key.trim().to_string(), header),
        None => (rest.trim(), format!("agent:{agent}"), header),
    }
}

/// A voice memo transcribed through the intercept (`transcribe`): the
/// text heard, or the refusal.
async fn transcribe(agent: &str, rest: &str) -> Result<String, String> {
    use http_body_util::{BodyExt, Full, Limited};
    use hyper_util::client::legacy::Client;
    use hyper_util::rt::TokioExecutor;

    let (words, key, header) = transcribe_words(rest, agent);
    let model = std::env::var("FRAGMENT_MODEL").map_err(|_| "no FRAGMENT_MODEL".to_string())?;
    let base = crate::net::Base::parse(&model)?;
    let boundary = "fragment-memo-0f3a9c";
    let mut body = Vec::new();
    for (name, value) in [("model", "whisper"), ("response_format", "json")] {
        body.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes());
    }
    body.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"memo.wav\"\r\nContent-Type: audio/wav\r\n\r\n").as_bytes());
    body.extend(memo(words));
    body.extend(format!("\r\n--{boundary}--\r\n").as_bytes());
    let mut req = hyper::Request::post(base.url("/v1/audio/transcriptions"))
        .header("host", base.authority())
        .header("authorization", format!("Bearer {key}"))
        .header("content-type", format!("multipart/form-data; boundary={boundary}"))
        .header("content-length", body.len());
    if let Some(h) = header {
        req = req.header("x-fragment-agent", h);
    }
    let req = req.body(Full::new(bytes::Bytes::from(body))).map_err(|e| e.to_string())?;
    let client = Client::builder(TokioExecutor::new()).build_http();
    let call = async {
        let res = client.request(req).await.map_err(|e| e.to_string())?;
        let status = res.status().as_u16();
        let bytes = Limited::new(res.into_body(), 64 * 1024).collect().await.map_err(|e| e.to_string())?.to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
        match v["text"].as_str() {
            Some(text) if status == 200 => Ok(text.to_string()),
            _ => Err(format!("{status} {}", String::from_utf8_lossy(&bytes).chars().take(300).collect::<String>())),
        }
    };
    tokio::time::timeout(Duration::from_millis(crate::limits::HTTP_TIMEOUT_MS), call).await.map_err(|_| "the model did not answer".to_string())?
}

/// `decide <text>`: one decision through the intercept (`POST
/// /v1/decide`), as a tool an agent built would make it, its agent named
/// by its key alone (`agent:<itself>`): is `<text>` a snack, on
/// clef-flash; `decide heard <words>`: is the voice memo saying `<words>`
/// (`memo`, inline) a snack, on clef-omni. The answer: the model's id and
/// the predicate's probability, or the refusal.
async fn decide(agent: &str, rest: &str) -> Result<String, String> {
    use base64::Engine;
    use http_body_util::{BodyExt, Full, Limited};
    use hyper_util::client::legacy::Client;
    use hyper_util::rt::TokioExecutor;

    let model = std::env::var("FRAGMENT_MODEL").map_err(|_| "no FRAGMENT_MODEL".to_string())?;
    let base = crate::net::Base::parse(&model)?;
    let snack = serde_json::json!({ "snack": { "type": "predicate", "instructions": "Is it a snack?" } });
    let body = match rest.strip_prefix("heard ") {
        Some(words) => {
            let audio = format!("data:audio/wav;base64,{}", base64::engine::general_purpose::STANDARD.encode(memo(words.trim())));
            serde_json::json!({ "model": "clef-omni", "input": "a voice memo", "questions": snack, "media": [{ "data": audio }] })
        }
        None => serde_json::json!({ "input": rest.trim(), "questions": snack }),
    }
    .to_string();
    let req = hyper::Request::post(base.url("/v1/decide"))
        .header("host", base.authority())
        .header("authorization", format!("Bearer agent:{agent}"))
        .header("content-type", "application/json")
        .header("content-length", body.len())
        .body(Full::new(bytes::Bytes::from(body)))
        .map_err(|e| e.to_string())?;
    let client = Client::builder(TokioExecutor::new()).build_http();
    let call = async {
        let res = client.request(req).await.map_err(|e| e.to_string())?;
        let status = res.status().as_u16();
        let bytes = Limited::new(res.into_body(), 64 * 1024).collect().await.map_err(|e| e.to_string())?.to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
        match (status, v["model"].as_str(), v["answers"]["snack"]["probability"].as_f64()) {
            (200, Some(model), Some(p)) => Ok(format!("{model} snack {p}")),
            _ => Err(format!("{status} {}", String::from_utf8_lossy(&bytes).chars().take(300).collect::<String>())),
        }
    };
    tokio::time::timeout(Duration::from_millis(crate::limits::HTTP_TIMEOUT_MS), call).await.map_err(|_| "the model did not answer".to_string())?
}

/// Waits `pace`, hearing a Stop meanwhile (true).
/// How many paces a slow tool call runs before its step (two seconds at the
/// image's 40 ms: long enough for a page to show it running).
const CALL_PACES: u32 = 50;

async fn pause(pace: Duration, rx: &mut mpsc::Receiver<Heard>) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(pace) => false,
        h = rx.recv() => matches!(h, Some(Heard::Stop) | None),
    }
}

/// Asks the asker something to answer in words, as the reply's next part,
/// and waits for the words (none: the turn was stopped).
async fn ask(events: &mpsc::Sender<Event>, rx: &mut mpsc::Receiver<Heard>, turn: &str, question: &str, part: &mut u32) -> Option<String> {
    let _ = events.send(Event::Reply { turn: turn.into(), part: *part, text: question.into() }).await;
    let _ = events.send(Event::Asked { turn: turn.into() }).await;
    *part += 1;
    loop {
        match rx.recv().await {
            Some(Heard::Told(words)) => return Some(words),
            Some(Heard::Answer(..) | Heard::Steered(_)) => continue,
            Some(Heard::Stop) | None => return None,
        }
    }
}

/// Whether `text` says the rule `key`: one of its words starts with it, past
/// an opening bracket, parenthesis or quote. A storage token's JWT, carried
/// by a `fetch`, once held "tOoL" and made that turn a tool turn (#778's CI).
fn says(text: &str, key: &str) -> bool {
    text.split_whitespace().any(|w| w.trim_start_matches(['[', '(', '"', '\'']).starts_with(key))
}

async fn turn(cfg: ScriptConfig, ts: TurnStart, mut rx: mpsc::Receiver<Heard>, events: mpsc::Sender<Event>) {
    let id = ts.turn.clone();
    let emit = |e: Event| {
        let events = events.clone();
        async move {
            let _ = events.send(e).await;
        }
    };
    let text = ts.text.to_lowercase();
    if ts.command {
        // a command of its menu: what it ran, at once
        emit(Event::Reply { turn: id.clone(), part: 1, text: format!("ran {}", ts.text) }).await;
        emit(Event::End { turn: id, outcome: Outcome::Idle }).await;
        return;
    }
    if says(&text, "fail") {
        emit(Event::End { turn: id, outcome: Outcome::Error("the script was asked to fail".into()) }).await;
        return;
    }
    if says(&text, "silent") {
        emit(Event::End { turn: id, outcome: Outcome::Idle }).await;
        return;
    }
    let mut reply = format!("echo: [{}] {}", ts.asker_name, ts.text);
    if let Some(said) = ts.text.strip_prefix("think ") {
        let (key, on, said) = think_words(said);
        reply = match think(&ts.agent.fragment, key, on, said).await {
            Ok(answer) => format!("thought: {answer}"),
            Err(e) => format!("think failed: {e}"),
        };
    }
    if text.trim() == "models" {
        reply = match models().await {
            Ok(listed) => format!("models: {listed}"),
            Err(e) => format!("models failed: {e}"),
        };
    }
    if text.trim() == "credentials" {
        reply = match credentials_of(&ts.agent.fragment).await {
            Ok(all) if all.is_empty() => "credentials: none".to_string(),
            Ok(all) => format!(
                "credentials: {}",
                all.iter().map(|c| format!("{} {} {}{}", c.provider, c.env.join(","), c.placeholder, c.model_base.as_ref().map(|b| format!(" models at {b}")).unwrap_or_default())).collect::<Vec<_>>().join("; ")
            ),
            Err(e) => format!("credentials failed: {e}"),
        };
    }
    if let Some(rest) = ts.text.strip_prefix("write ") {
        reply = write_data(&cfg.data, rest.trim()).await;
    }
    if let Some(rest) = ts.text.strip_prefix("read ") {
        reply = read_data(&cfg.data, rest.trim()).await;
    }
    if let Some(rest) = ts.text.strip_prefix("transcribe ") {
        reply = match transcribe(&ts.agent.fragment, rest).await {
            Ok(text) => format!("heard: {text}"),
            Err(e) => format!("transcribe refused: {e}"),
        };
    }
    if let Some(rest) = ts.text.strip_prefix("schedule ") {
        reply = schedule(&ts.agent, &ts.fragment, rest).await;
    }
    if let Some(rest) = ts.text.strip_prefix("ask ") {
        reply = ask_another(&ts.agent, &ts.fragment, &id, rest).await;
    }
    if let Some(rest) = ts.text.strip_prefix("relay ") {
        reply = relay(&ts.agent.fragment, rest).await.unwrap_or_else(|e| format!("relay failed: {e}"));
    }
    if let Some(rest) = ts.text.strip_prefix("door ") {
        reply = door(&ts.agent, &ts.fragment, rest).await.unwrap_or_else(|e| format!("door refused: {e}"));
    }
    if let Some(rest) = ts.text.strip_prefix("decide ") {
        reply = match decide(&ts.agent.fragment, rest).await {
            Ok(answer) => format!("decided: {answer}"),
            Err(e) => format!("decide refused: {e}"),
        };
    }
    if text.starts_with("fetch ") {
        reply = match fetch(&ts.agent.fragment, &ts.text).await {
            Ok((status, body)) => format!("fetched {status}: {}", body.chars().take(300).collect::<String>()),
            Err(e) => format!("fetch failed: {e}"),
        };
    }
    let mut part = 1;
    if says(&text, "tool") {
        // its call said before it runs, its words before the call, a reply
        // of their own, then (a slow call runs on a while) the step
        let args: String = ts.text.chars().take(40).collect();
        let call = Call { response: format!("script-{}", &id[..12]), position: 0, tool_call_id: "call-search".into(), tool: "search".into(), args: args.clone(), category: Category::Web };
        emit(Event::Called { turn: id.clone(), call }).await;
        emit(Event::Reply { turn: id.clone(), part, text: "Let me look.".into() }).await;
        part += 1;
        if says(&text, "slow") {
            for _ in 0..CALL_PACES {
                if pause(cfg.pace, &mut rx).await {
                    emit(Event::End { turn: id, outcome: Outcome::Stopped }).await;
                    return;
                }
            }
        }
        let step = Step { tool: "search".into(), args, ok: true, excerpt: "3 results".into(), category: Category::Web, evidence: None };
        emit(Event::Step { turn: id.clone(), step }).await;
    }
    if says(&text, "approve") || says(&text, "risky") {
        emit(Event::Step { turn: id.clone(), step: Step { tool: "terminal".into(), args: "rm -rf ./scratch".into(), ok: true, excerpt: String::new(), category: Category::Shell, evidence: None } }).await;
        let options = vec![
            PromptOption { id: "once".into(), label: "Allow once".into(), style: Some("primary".into()), words: false },
            PromptOption { id: "deny".into(), label: "Deny".into(), style: Some("danger".into()), words: false },
        ];
        let prompt = format!("p-{}", &id[..12]);
        emit(Event::Prompt { turn: id.clone(), prompt, text: "Run `rm -rf ./scratch`?".into(), options, ttl_ms: None }).await;
        let said = match rx.recv().await {
            Some(Heard::Answer(Some(o), _)) if o == "once" => "approved",
            Some(Heard::Answer(Some(_), _)) => "denied",
            Some(Heard::Answer(None, _) | Heard::Told(_) | Heard::Steered(_)) => "not approved",
            Some(Heard::Stop) | None => {
                emit(Event::End { turn: id, outcome: Outcome::Stopped }).await;
                return;
            }
        };
        reply = format!("{reply} ({said})");
    }
    if text.split_whitespace().any(|w| w == "choose") {
        let options = [("c0", "Basil", false), ("c1", "Mint", false), ("other", "Something else", true)].map(|(id, label, words)| PromptOption { id: id.into(), label: label.into(), style: None, words });
        let prompt = format!("c-{}", &id[..12]);
        emit(Event::Prompt { turn: id.clone(), prompt, text: "What should I plant?".into(), options: options.to_vec(), ttl_ms: None }).await;
        let chose = match rx.recv().await {
            Some(Heard::Answer(Some(o), Some(words))) if o == "other" => Some(words),
            Some(Heard::Answer(Some(o), None)) if o == "other" => ask(&events, &mut rx, &id, "Type your answer:", &mut part).await,
            Some(Heard::Answer(Some(o), _)) => Some(options.iter().find(|x| x.id == o).map_or(o, |x| x.label.clone())),
            Some(Heard::Answer(None, _) | Heard::Told(_) | Heard::Steered(_)) => Some("nothing".into()),
            Some(Heard::Stop) | None => None,
        };
        let Some(chose) = chose else {
            emit(Event::End { turn: id, outcome: Outcome::Stopped }).await;
            return;
        };
        reply = format!("{reply} (chose: {chose})");
    }
    if text.split_whitespace().any(|w| w == "ask-me") {
        let Some(told) = ask(&events, &mut rx, &id, "What should I call it?", &mut part).await else {
            emit(Event::End { turn: id, outcome: Outcome::Stopped }).await;
            return;
        };
        reply = format!("{reply} (told: {told})");
    }
    if text.split_whitespace().any(|w| w == "steer-me") {
        emit(Event::Draft { turn: id.clone(), text: "waiting to be steered…".into() }).await;
        let wait = tokio::time::sleep(STEER_WAIT);
        tokio::pin!(wait);
        // bounded by STEER_WAIT
        let steered = loop {
            tokio::select! {
                h = rx.recv() => match h {
                    Some(Heard::Steered(words)) => break words,
                    Some(Heard::Stop) | None => {
                        emit(Event::End { turn: id, outcome: Outcome::Stopped }).await;
                        return;
                    }
                    Some(_) => continue,
                },
                _ = &mut wait => break "nothing".to_string(),
            }
        };
        reply = format!("{reply} (steered: {steered})");
    }
    if let Some(q) = &ts.quote {
        reply = format!("{reply} (quoting{}: {})", if q.own { " itself" } else { "" }, q.text);
    }
    if !ts.files.is_empty() {
        let names: Vec<&str> = ts.files.iter().map(|f| f.name.as_str()).collect();
        reply = format!("{reply} [got {}: {}]", ts.files.len(), names.join(", "));
    }
    // what it was told first (a turn after a cut one), echoed after the reply
    if let Some(note) = &ts.note {
        reply = format!("{reply}\n\n(told: {note})");
    }
    if text.split_whitespace().any(|w| w == "notice") {
        emit(Event::Notice { agent: ts.agent.fragment.clone(), fragment: ts.fragment.clone(), category: NoticeCategory::Info, text: "Compressing context".into() }).await;
    }
    let drafts = if says(&text, "slow") { 20 } else { 2 };
    for i in 1..=drafts {
        let cut = reply.chars().count() * i / (drafts + 1);
        emit(Event::Draft { turn: id.clone(), text: reply.chars().take(cut.max(1)).collect() }).await;
        if pause(cfg.pace, &mut rx).await {
            emit(Event::End { turn: id, outcome: Outcome::Stopped }).await;
            return;
        }
    }
    emit(Event::Reply { turn: id.clone(), part, text: reply }).await;
    if says(&text, "draw") {
        let path = cfg.scratch.join(format!("{id}-drawing.txt"));
        let body = format!("a drawing for {}\n", ts.asker_name);
        if tokio::fs::write(&path, &body).await.is_ok() {
            let file = LocalFile { path, media_type: "text/plain".into(), name: "drawing.txt".into(), size: body.len() as u64 };
            emit(Event::Attachment { turn: id.clone(), part, file }).await;
        }
    }
    if text.split_whitespace().any(|w| w == "speak") {
        let path = cfg.scratch.join(format!("{id}-speech.mp3"));
        let body = crate::simplex::audio::silence_mp3(SPOKEN_SECS);
        if tokio::fs::write(&path, &body).await.is_ok() {
            let file = LocalFile { path, media_type: "audio/mpeg".into(), name: "speech.mp3".into(), size: body.len() as u64 };
            emit(Event::Attachment { turn: id.clone(), part, file }).await;
        }
    }
    let noticed = text.split_whitespace().any(|w| w == "notice");
    emit(Event::End { turn: id, outcome: Outcome::Idle }).await;
    if noticed {
        let (agent, fragment) = (ts.agent.fragment.clone(), ts.fragment.clone());
        emit(Event::Notice { agent, fragment, category: NoticeCategory::Memory, text: "Self-improvement review: Memory updated".into() }).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `fetch`'s grammar: a header (authorization by default), a query
    /// parameter, or basic auth; anything else is refused, saying how.
    #[test]
    fn fetch_reads_where_its_value_goes() {
        assert_eq!(fetch_words("fetch http://a.test/x with $PERPLEXITY_API_KEY").unwrap(), ("http://a.test/x", "$PERPLEXITY_API_KEY", Place::Header("authorization")));
        assert_eq!(fetch_words("fetch http://a.test/x with $K in xi-api-key").unwrap(), ("http://a.test/x", "$K", Place::Header("xi-api-key")));
        assert_eq!(fetch_words("fetch http://a.test/x with $K in query key").unwrap(), ("http://a.test/x", "$K", Place::Query("key")));
        assert_eq!(fetch_words("fetch http://a.test/x with fck_p_00 as basic api").unwrap(), ("http://a.test/x", "fck_p_00", Place::Basic("api")));
        for bad in ["fetch http://a.test/x", "fetch http://a.test/x with", "fetch http://a.test/x with $K in", "fetch http://a.test/x with $K as basic"] {
            assert!(fetch_words(bad).is_err(), "{bad}");
        }
    }

    /// A rule is a word said, never text a word carries: `slowly` and
    /// `[SILENT]` say theirs; a token or a URL with one inside says none.
    #[test]
    fn a_rule_is_a_word_said_never_text_inside_one() {
        for (text, key) in [("tool please", "tool"), ("please use the terminal slowly", "slow"), ("end this turn with exactly [silent]", "silent"), ("do\nthe risky thing", "risky"), ("(approve this)", "approve"), ("the message that fails", "fail")] {
            assert!(says(text, key), "{text} says {key}");
        }
        for (text, key) in [("fetch http://a.test/x with eyjhbgcioijfuzi1nij9.untoolfail.sig", "tool"), ("fetch http://a.test/x with eyjhbgcioijfuzi1nij9.untoolfail.sig", "fail"), ("fetch http://a.test/tools/x with k", "tool"), ("withdrawn", "draw"), ("its -tool", "tool")] {
            assert!(!says(text, key), "{text} does not say {key}");
        }
    }

    /// `schedule`'s grammar: `add` with five cron fields and words, `list`,
    /// `rm <id>`; anything else says how.
    #[test]
    fn schedule_reads_what_it_asks() {
        assert_eq!(schedule_words("add 30 9 * * mon-fri check  the shelf"), ScheduleAsk::Add { cron: "30 9 * * mon-fri".into(), text: "check the shelf".into() });
        assert_eq!(schedule_words("list"), ScheduleAsk::List);
        assert_eq!(schedule_words("rm 7"), ScheduleAsk::Rm(7));
        for unsaid in ["", "add 0 * * * *", "add 0 * * *", "rm", "rm seven", "rm 1 2", "list all", "remove 3"] {
            assert_eq!(schedule_words(unsaid), ScheduleAsk::Unsaid, "{unsaid:?}");
        }
    }

    /// `transcribe`'s grammar: its words, its key (`agent:<itself>` unless
    /// named) and a header's agent; and its memo, a WAV that says them.
    #[test]
    fn think_reads_its_key_and_model() {
        assert_eq!(think_words("hello model"), (None, None, "hello model"));
        assert_eq!(think_words("as agent:j--k3x9 hello"), (Some("agent:j--k3x9"), None, "hello"));
        assert_eq!(think_words("on @cf/zai-org/glm-5.3 hello there"), (None, Some("@cf/zai-org/glm-5.3"), "hello there"));
        assert_eq!(think_words("as k on z-ai/glm-5.3-flash hi"), (Some("k"), Some("z-ai/glm-5.3-flash"), "hi"));
        assert_eq!(think_words("on"), (None, None, "on"), "no model without words after it");
    }

    #[test]
    fn transcribe_reads_its_key_and_header() {
        assert_eq!(transcribe_words("hello from a memo", "juniper--k3x9"), ("hello from a memo", "agent:juniper--k3x9".to_string(), None));
        assert_eq!(transcribe_words("hi as agent:willow--k3x9", "juniper--k3x9"), ("hi", "agent:willow--k3x9".to_string(), None));
        assert_eq!(transcribe_words("hi as agent:willow--k3x9 header juniper--k3x9", "juniper--k3x9"), ("hi", "agent:willow--k3x9".to_string(), Some("juniper--k3x9")));
        let m = memo("hi!");
        assert_eq!((&m[..4], &m[8..16]), (&b"RIFF"[..], &b"WAVEfmt "[..]));
        assert_eq!(u32::from_le_bytes(m[4..8].try_into().unwrap()) as usize, m.len() - 8, "its RIFF size is its bytes after the size");
        let at = m.windows(4).position(|w| w == b"said").unwrap();
        assert_eq!(&m[at + 8..at + 11], b"hi!");
    }

    /// `write` and `read` stay under the data root: a relative path of
    /// plain parts, or nothing; then a file written is read back, and one
    /// never written reads as none.
    #[tokio::test]
    async fn write_and_read_stay_under_the_data() {
        let root = std::env::temp_dir().join(format!("script-data-{}", std::process::id()));
        assert_eq!(data_path(&root, "notes/keep.txt"), Some(root.join("notes/keep.txt")));
        for bad in ["", "/etc/passwd", "../up", "a/../b", "a//b", "./a", "a b", "x/y/z/a/b/c/d/e/f"] {
            assert_eq!(data_path(&root, bad), None, "{bad:?}");
        }
        assert_eq!(write_data(&root, "notes/keep.txt kept, hello").await, "wrote notes/keep.txt");
        assert_eq!(read_data(&root, "notes/keep.txt").await, "read notes/keep.txt: kept, hello");
        assert_eq!(read_data(&root, "notes/gone.txt").await, "read notes/gone.txt: none");
        assert!(write_data(&root, "../escape x").await.starts_with("write refused"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
