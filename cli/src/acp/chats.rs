//! What `fragment acp` asks of the platform, each as the CLI's own client
//! asks it (this machine's key signs): which agent, a chat made for a
//! session, a message posted, a turn stopped, a chat's records read. A
//! session is a chat, named in full, so nothing here keeps anything.

use anyhow::{Context, Result};
use fragment_proto::{Created, Posted, Role};
use serde_json::{json, Value};

use super::turn::Record;
use crate::api::{Client, Code};
use crate::ask::{self, Agent};

/// A chat's newest records read as a prompt comes, for the turn that waits
/// for an answer (`turn::asking`): a question older than this many records
/// has gone on, or been answered, long since.
pub const ASKING_BACK_RECORDS: i64 = 200;
/// The numbers a new chat's label is drawn from: `<agent>-chat-2` on, the
/// shell's own four digits' room (cell/shell/shell.js, `chatBase`).
const CHAT_NUMBER_MAX: u32 = 9_999;

/// The agent `who` names, among the person's own, as `fragment ask` names
/// one: by its label, its fragment, its identity or its name.
pub fn agent(c: &Client, who: &str) -> Result<Agent> {
    if let Some(mode) = c.agent() {
        return Err(ask::coded(Code::InvalidUsage, format!("{} is an agent: `fragment acp` serves a person's agent to their own tools (an agent asks another with `fragment ask`)", mode.agent)));
    }
    let (_, mut agents) = ask::roster(c)?;
    ask::named(c, None, &mut agents);
    match ask::pick(&agents, who) {
        Ok(a) => Ok(a.clone()),
        Err(many) if many.len() > 1 => {
            let fragments: Vec<&str> = many.iter().map(|a| a.fragment.as_str()).collect();
            Err(ask::coded(Code::Conflict, format!("{who:?} names {} agents ({}): name one by its fragment", many.len(), fragments.join(", "))))
        }
        Err(_) => {
            let names: Vec<String> = agents.iter().map(|a| format!("{} ({})", a.shown(), a.fragment)).collect();
            Err(ask::coded(Code::NotFound, format!("{who:?} is none of your agents ({})", if names.is_empty() { "you have none".into() } else { names.join(", ") })))
        }
    }
}

/// Who the signer is: the person's identity, as their records name them
/// (docs/api.md, Identities). It is none of their keys': a person holds
/// several, and each machine's CLI signs with its own.
pub fn me(c: &Client) -> Result<String> {
    let who = c.call(c.get("/api/identities/me")?).context("reading who you are")?;
    who["id"].as_str().map(str::to_string).context("the platform's answer names no identity")
}

/// The person's list, as the platform answers it (`GET /api/fragments`).
pub fn listed(c: &Client) -> Result<Value> {
    c.call(c.get("/api/fragments")?).context("reading your fragments")
}

/// A new chat's label: its agent's, `-chat` and the first number from 2
/// that no fragment of the person's list has, as the shell numbers one
/// (`maple-chat-2`). The agent's own chat (`<label>-chat`) stays its own:
/// `fragment ask` finds it by that label.
pub fn free_label(agent: &str, listed: &Value) -> Result<String> {
    let room = fragment_proto::limits::LABEL_ROOM_MIN_BYTES - "-chat-9999".len();
    let base = format!("{}-chat", ask::cut_label(ask::label(agent), room));
    let rows = listed["fragments"].as_array().map(Vec::as_slice).unwrap_or_default();
    let taken: std::collections::BTreeSet<&str> = rows.iter().filter_map(|f| f["name"].as_str()).map(ask::label).collect();
    // bounded: CHAT_NUMBER_MAX numbers
    let free = (2..=CHAT_NUMBER_MAX).map(|n| format!("{base}-{n}")).find(|l| !taken.contains(l.as_str()));
    let label = free.ok_or_else(|| ask::coded(Code::Conflict, format!("every chat label from {base}-2 to {base}-{CHAT_NUMBER_MAX} is taken: archive or delete some")))?;
    assert!(fragment_proto::valid_label(&label), "a chat's label is a label: {label}");
    Ok(label)
}

/// A new chat with `agent` in it, as the shell's New chat makes one: its
/// name in full, the session's id.
pub fn make(c: &Client, agent: &Agent) -> Result<String> {
    let label = free_label(&agent.fragment, &listed(c)?)?;
    let made = c.post_json("/api/fragments", &json!({ "label": label, "template": "chat", "title": ask::UNTITLED_CHAT, "agent": agent.identity }))?;
    Ok(c.call_as::<Created>(made).with_context(|| format!("making a chat with {}", agent.shown()))?.name)
}

/// The newest seq of a chat's two channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seqs {
    pub chat: i64,
    pub work: i64,
}

/// A chat's newest seqs. A fragment that is no chat (no `chat` the caller
/// may post on, or no `work`) is refused.
pub fn seqs(c: &Client, chat: &str) -> Result<Seqs> {
    let channels = c.call(c.get(&format!("/api/f/{chat}/channels"))?)?;
    let listed = channels["channels"].as_array().map(Vec::as_slice).unwrap_or_default();
    let of = |name: &str| listed.iter().find(|ch| ch["name"] == name);
    match (of("chat").filter(|ch| ch["post"].is_string()), of("work")) {
        (Some(said), Some(work)) => Ok(Seqs { chat: said["seq"].as_i64().unwrap_or(0), work: work["seq"].as_i64().unwrap_or(0) }),
        _ => Err(ask::coded(Code::InvalidRequest, format!("{chat} is no chat you can post in: a session is a chat (its channels `chat` and `work`)"))),
    }
}

/// Whether `agent` answers in `chat`: an editor there (its turns are an
/// editor's records on `work`). None when there is no such chat.
pub fn answers(c: &Client, chat: &str, agent: &Agent) -> Result<Option<bool>> {
    Ok(ask::members(c, chat)?.map(|list| list.members.iter().any(|m| m.principal == agent.identity && m.role >= Role::Editor)))
}

/// Posts `text` to `agent` in `chat`: the record, whose seq causes the turn.
pub fn say(c: &Client, chat: &str, text: &str, agent: &Agent) -> Result<Record> {
    let id = format!("acp-{:016x}", rand::random::<u64>());
    let post = fragment_proto::PostRecord { id: id.clone(), body: json!({ "text": text, "to": [agent.identity] }) };
    let posted: Posted = c.post_json_by_id(&format!("/api/f/{chat}/channels/chat"), &post).and_then(|r| c.call_as(r)).map_err(|e| e.context(crate::CallId::post(&id)))?;
    let record = serde_json::to_value(&posted.record)?;
    Record::of(&record).context("the platform's answer is no record")
}

/// Stops `turn` in `chat`, as the chat's page does (docs/chat-records.md,
/// Stop): the same id again posts nothing.
pub fn stop(c: &Client, chat: &str, turn: &str) -> Result<()> {
    let post = fragment_proto::PostRecord { id: format!("stop:{turn}"), body: json!({ "kind": "stop", "turn": turn }) };
    let r = c.post_json_by_id(&format!("/api/f/{chat}/channels/chat"), &post)?;
    c.call(r).map(|_| ())
}

/// The records of `channel` after `after`, and the seq of the last read:
/// a socket subscribed after it tells the next, with none between missed.
pub fn records(c: &Client, chat: &str, channel: &str, after: i64) -> Result<(Vec<Record>, i64)> {
    let (page, cursor) = ask::records_after(c, chat, channel, after)?;
    let records: Vec<Record> = page.iter().filter_map(Record::of).collect();
    let last = records.iter().map(|r| r.seq).fold(cursor.max(after), i64::max);
    Ok((records, last))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Goal: a new chat's label is its agent's, numbered from 2 past any
    /// the person has, so the agent's own chat (`<label>-chat`) stays the
    /// one `fragment ask` finds. Invalid: a long agent label is cut to fit
    /// any deployment's hosts, never ending in `-`.
    #[test]
    fn a_new_chats_label_is_the_first_free_number() {
        let listed = |names: &[&str]| json!({ "fragments": names.iter().map(|n| json!({ "name": n })).collect::<Vec<_>>() });
        assert_eq!(free_label("fred--k3x9", &listed(&[])).unwrap(), "fred-chat-2");
        assert_eq!(free_label("fred--k3x9", &listed(&["fred-chat--a2b3", "fred--k3x9"])).unwrap(), "fred-chat-2", "its own chat is not numbered");
        assert_eq!(free_label("fred--k3x9", &listed(&["fred-chat-2--a2b3", "fred-chat-3--c4d5", "maple-chat-4--e6f7"])).unwrap(), "fred-chat-4");
        assert_eq!(free_label("fred--k3x9", &json!({})).unwrap(), "fred-chat-2", "a list that names none");
        let long = format!("{}-x--k3x9", "z".repeat(60));
        let label = free_label(&long, &listed(&[])).unwrap();
        assert!(fragment_proto::valid_label(&label) && label.len() <= fragment_proto::limits::LABEL_ROOM_MIN_BYTES && label.ends_with("-chat-2"), "{label}");
    }
}
