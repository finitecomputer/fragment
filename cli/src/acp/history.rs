//! A person's chats with an agent as an ACP client's sessions: which
//! there are (`session/list`), and what one has said so far, replayed as
//! the session updates it would have been (`session/load`). Pure: each
//! reads what the platform answered.
//!
//! A replay tells what a person reads in the chat: what was said, by whom,
//! each tool call that ended (its step), and each card's question. A turn's
//! claim, its end, a call no step ended and a page's own kinds are not
//! told: a client is told what happened, never a guess at it.

use fragment_core::export::{Names, Said};
use serde_json::{json, Value};

use super::turn::{card, chunk, files, stepped, Links, Record};
use crate::ask::UNTITLED_CHAT;

/// A session's title at most, in characters: the shell's own for a chat
/// it titles from its first message (cell/shell/shell.js).
const TITLE_MAX_CHARS: usize = 48;

/// The person's chats that `agent` (its identity) answers in, the newest
/// first, as ACP's `SessionInfo`s. One the person archived, or may not
/// post in, is none. A chat belongs to no folder, so each is in `cwd`:
/// whichever the client asks about.
pub fn sessions(listed: &Value, agent: &str, cwd: &str) -> Vec<Value> {
    let rows = listed["fragments"].as_array().map(Vec::as_slice).unwrap_or_default();
    let mut chats: Vec<&Value> = rows
        .iter()
        .filter(|f| f["kind"] == "chat" && f["archived"] != true && matches!(f["role"].as_str(), Some("owner" | "editor")))
        .filter(|f| f["agents"].as_array().is_some_and(|a| a.iter().any(|x| x == agent)))
        .filter(|f| f["name"].is_string())
        .collect();
    // newest first; one nothing was said in, last (the shell's order)
    chats.sort_by_key(|f| (std::cmp::Reverse(f["previewAt"].as_i64().unwrap_or(i64::MIN)), f["name"].as_str().unwrap_or("").to_string()));
    chats
        .into_iter()
        .map(|f| {
            let mut session = json!({ "sessionId": f["name"], "cwd": cwd, "title": titled(f) });
            if let Some(at) = f["previewAt"].as_i64().and_then(|ms| jiff::Timestamp::from_millisecond(ms).ok()) {
                session["updatedAt"] = json!(at.to_string());
            }
            session
        })
        .collect()
}

/// What a chat is called: its title; one the shell has not titled yet
/// (`UNTITLED_CHAT`), its newest message's first line; else its name.
fn titled(f: &Value) -> String {
    let said = |key: &str| f[key].as_str().map(str::trim).filter(|t| !t.is_empty());
    let title = said("title").filter(|t| *t != UNTITLED_CHAT).or(said("preview")).or(said("name")).unwrap_or("");
    match title.chars().count() > TITLE_MAX_CHARS {
        true => format!("{}…", title.chars().take(TITLE_MAX_CHARS - 1).collect::<String>().trim_end()),
        false => title.to_string(),
    }
}

/// Who is who in a chat replayed.
pub struct Who {
    /// The agent the session is with (its identity): its words are the
    /// agent's.
    pub agent: String,
    /// The person the client is: their words are the user's.
    pub me: String,
    /// What anyone else is called: the person's other agents by their
    /// names, and the rest by a short identity (`Names::of`).
    pub names: Names,
}

/// What a chat has said so far, as the session updates a client is told
/// on `session/load`, in the order it happened (a message before the work
/// of the same moment, as a person's export reads it:
/// `fragment_core::export`). The agent's own words are the agent's, and
/// the person's the user's. ACP has those two sides only, so anyone
/// else's (another agent of the chat, another person) are told on the
/// user's side with who said them, `[Maple] …`, never as the person's
/// own. A message through a door (`relayed`) is whose words it carries.
pub fn replay(chat: &[Record], work: &[Record], who: &Who, links: &Links) -> Vec<Value> {
    let agent = who.agent.as_str();
    let mut all: Vec<&Record> = chat.iter().chain(work).collect();
    all.sort_by_key(|r| Said { channel: &r.channel, seq: r.seq, at: r.at, principal: &r.principal, body: &r.body }.order());
    let mut told = Told::default();
    for r in all {
        let own = r.principal == agent;
        match (r.channel.as_str(), r.body["kind"].as_str()) {
            ("chat", None | Some("message")) => {
                let Some(text) = r.said() else { continue };
                // a door carries its owner's words: they are whose it says
                let speaker = r.body["relayed"]["for"].as_str().unwrap_or(&r.principal);
                if speaker == agent {
                    told.say("agent_message_chunk", text);
                    told.updates.extend(files(&r.body, &links.blobs));
                } else {
                    told.say("user_message_chunk", &who.said_by(speaker, text));
                }
            }
            ("chat", Some("command")) if !own => {
                let (command, args) = (r.body["command"].as_str().unwrap_or(""), r.body["args"].as_str().unwrap_or(""));
                told.say("user_message_chunk", &who.said_by(&r.principal, format!("/{command} {args}").trim_end()));
            }
            ("work", Some("turn.step")) if own => {
                let turn = r.body["turn"].as_str().unwrap_or("");
                told.updates.push(stepped(&format!("{turn}:{}", r.seq), &r.body));
                told.last = "";
            }
            ("work", Some("turn.prompt")) if own => told.say("agent_message_chunk", &card(&r.body, &links.chat)),
            _ => {}
        }
    }
    told.updates
}

impl Who {
    /// `text` as the user's side tells it: the person's own as it is,
    /// anyone else's with who said it.
    fn said_by(&self, speaker: &str, text: &str) -> String {
        match speaker == self.me {
            true => text.to_string(),
            false => format!("[{}] {text}", self.names.of(speaker)),
        }
    }
}

/// A replay as it is told.
#[derive(Default)]
struct Told {
    updates: Vec<Value>,
    /// Whose words the last update was: two messages running are two
    /// paragraphs, since a client may show a side's chunks as one message.
    last: &'static str,
}

impl Told {
    fn say(&mut self, kind: &'static str, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        let lead = if self.last == kind { "\n\n" } else { "" };
        self.updates.push(chunk(kind, &format!("{lead}{text}")));
        self.last = kind;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AGENT: &str = "npub1fred";
    const ME: &str = "npub1paul";

    /// Who is who: the agent replayed, the person, and their other agent.
    fn who() -> Who {
        Who { agent: AGENT.into(), me: ME.into(), names: Names([("npub1maple".to_string(), "Maple".to_string())].into()) }
    }

    fn rec(channel: &str, seq: i64, at: i64, principal: &str, body: Value) -> Record {
        Record { channel: channel.into(), seq, at, principal: principal.into(), body }
    }

    /// Goal: an agent's chats are its person's sessions, the newest first,
    /// each titled as a person knows it. Invalid: an app, another agent's
    /// chat, an archived one, and one the person only reads are none.
    #[test]
    fn an_agents_chats_are_its_sessions() {
        let listed = json!({ "fragments": [
            { "name": "fred--k3x9", "kind": "agent", "role": "owner", "agents": [] },
            { "name": "fred-chat--a2b3", "kind": "chat", "role": "owner", "title": "Beans", "agents": [AGENT], "previewAt": 1_790_000_000_000_i64, "preview": "ok" },
            { "name": "fred-chat-2--c4d5", "kind": "chat", "role": "owner", "title": "New chat", "agents": [AGENT, "npub1maple"], "previewAt": 1_790_000_600_000_i64, "preview": "What is on Friday?" },
            { "name": "fred-chat-3--e6f7", "kind": "chat", "role": "owner", "title": "New chat", "agents": [AGENT] },
            { "name": "maple-chat--g8h9", "kind": "chat", "role": "owner", "title": "Maple's", "agents": ["npub1maple"], "previewAt": 1_790_000_900_000_i64 },
            { "name": "fred-chat-4--j2k3", "kind": "chat", "role": "owner", "title": "Put away", "agents": [AGENT], "archived": true },
            { "name": "fred-chat-5--m4n5", "kind": "chat", "role": "viewer", "title": "Another's", "agents": [AGENT] },
            { "name": "todo--p6q7", "kind": "app", "role": "owner", "title": "Todo", "agents": [AGENT] },
        ] });
        assert_eq!(
            sessions(&listed, AGENT, "/work/repo"),
            [
                json!({ "sessionId": "fred-chat-2--c4d5", "cwd": "/work/repo", "title": "What is on Friday?", "updatedAt": "2026-09-21T14:23:20Z" }),
                json!({ "sessionId": "fred-chat--a2b3", "cwd": "/work/repo", "title": "Beans", "updatedAt": "2026-09-21T14:13:20Z" }),
                json!({ "sessionId": "fred-chat-3--e6f7", "cwd": "/work/repo", "title": "fred-chat-3--e6f7" }),
            ]
        );
        assert_eq!(sessions(&json!({}), AGENT, "/"), Vec::<Value>::new());
        let long = json!({ "fragments": [{ "name": "c--a2b3", "kind": "chat", "role": "editor", "title": "x".repeat(60), "agents": [AGENT] }] });
        assert_eq!(sessions(&long, AGENT, "/")[0]["title"], format!("{}…", "x".repeat(47)));
    }

    /// Goal: a chat is replayed in the order it happened, each record as
    /// the update it would have been: the person's words, the agent's, a
    /// step as an ended tool call, a card's question. Invalid: a turn's
    /// claim and end, a call no step ended, a stop, another agent's step
    /// and an empty message tell nothing.
    #[test]
    fn a_chat_is_replayed_as_it_happened() {
        let links = Links::of("https://x", "c--k3x9");
        let chat = [
            rec("chat", 1, 100, ME, json!({ "text": "hi fred", "to": [AGENT] })),
            rec("chat", 2, 200, AGENT, json!({ "text": "Let me look.", "turn": "t1", "interim": true })),
            rec("chat", 3, 300, AGENT, json!({ "text": "echo: hi fred", "turn": "t1" })),
            rec("chat", 4, 400, ME, json!({ "kind": "stop", "turn": "t1" })),
            rec("chat", 5, 500, ME, json!("a bare string")),
            rec("chat", 6, 500, ME, json!({ "text": "and again" })),
            rec("chat", 7, 600, AGENT, json!({ "text": "from my phone", "to": [AGENT], "relayed": { "via": "simplex", "for": ME, "agent": "fred--k3x9" } })),
            rec("chat", 8, 700, ME, json!({ "kind": "command", "command": "new", "to": [AGENT] })),
            rec("chat", 9, 800, ME, json!({ "text": "  " })),
        ];
        let work = [
            rec("work", 1, 150, AGENT, json!({ "kind": "turn.start", "turn": "t1", "agent": AGENT, "cause": { "channel": "chat", "seq": 1 } })),
            rec("work", 2, 200, AGENT, json!({ "kind": "turn.call", "turn": "t1", "response": "r1", "tool": "search", "category": "web", "args": "hi" })),
            rec("work", 3, 200, AGENT, json!({ "kind": "turn.step", "turn": "t1", "step": 1, "response": "r1", "tool": "search", "category": "web", "args": "hi", "ok": true, "status": "completed" })),
            rec("work", 4, 250, "npub1maple", json!({ "kind": "turn.step", "turn": "t9", "step": 1, "tool": "terminal", "category": "shell", "args": "ls", "ok": true })),
            rec("work", 5, 350, AGENT, json!({ "kind": "turn.end", "turn": "t1", "outcome": "idle" })),
            rec("work", 6, 650, AGENT, json!({ "kind": "turn.prompt", "turn": "t2", "prompt": "p1", "text": "Go on?", "options": [{ "id": "c0", "label": "Yes" }] })),
        ];
        let told: Vec<(String, String)> = replay(&chat, &work, &who(), &links)
            .iter()
            .map(|u| (u["sessionUpdate"].as_str().unwrap_or("").to_string(), u["content"]["text"].as_str().or(u["title"].as_str()).unwrap_or("").to_string()))
            .collect();
        let told: Vec<(&str, &str)> = told.iter().map(|(k, t)| (k.as_str(), t.as_str())).collect();
        assert_eq!(
            told,
            [
                ("user_message_chunk", "hi fred"),
                // the words before a call come before its step, said at the same moment
                ("agent_message_chunk", "Let me look."),
                ("tool_call", "search: hi"),
                ("agent_message_chunk", "echo: hi fred"),
                ("user_message_chunk", "a bare string"),
                ("user_message_chunk", "\n\nand again"),
                ("user_message_chunk", "\n\nfrom my phone"),
                ("agent_message_chunk", "Go on?\n- Yes\n\nAnswer in the chat: https://x/?chat=c--k3x9"),
                ("user_message_chunk", "/new"),
            ]
        );
        assert_eq!(replay(&[], &[], &who(), &links), Vec::<Value>::new());
    }

    /// Goal: in a chat of several, nobody's words are told as another's:
    /// another agent's and another person's are said with who said them,
    /// by the name the person knows an agent by, and a message through a
    /// door is whose words it carries.
    #[test]
    fn others_words_are_said_with_who_said_them() {
        let links = Links::of("https://x", "c--k3x9");
        let ada = "npub1adaadaadaadaadaadaadaadaadaadaadaadaadaadaadaadaadaadaada";
        let chat = [
            rec("chat", 1, 100, ME, json!({ "text": "what do you two think?" })),
            rec("chat", 2, 200, "npub1maple", json!({ "text": "I think so.", "turn": "t9" })),
            rec("chat", 3, 300, AGENT, json!({ "text": "Agreed.", "turn": "t1" })),
            rec("chat", 4, 400, ada, json!({ "text": "me too" })),
            rec("chat", 5, 500, "npub1telegram", json!({ "text": "from my phone", "relayed": { "via": "telegram", "for": ME, "agent": "fred--k3x9" } })),
            rec("chat", 6, 600, "npub1maple", json!({ "kind": "command", "command": "new" })),
        ];
        let told: Vec<Value> = replay(&chat, &[], &who(), &links);
        let told: Vec<(&str, &str)> = told.iter().map(|u| (u["sessionUpdate"].as_str().unwrap_or(""), u["content"]["text"].as_str().unwrap_or(""))).collect();
        assert_eq!(
            told,
            [
                ("user_message_chunk", "what do you two think?"),
                ("user_message_chunk", "\n\n[Maple] I think so."),
                ("agent_message_chunk", "Agreed."),
                ("user_message_chunk", "[npub1adaadaa…adaada] me too"),
                ("user_message_chunk", "\n\nfrom my phone"),
                ("user_message_chunk", "\n\n[Maple] /new"),
            ]
        );
    }
}
