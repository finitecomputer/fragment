//! Late card answers are new turns, reconstructed from the journal rather
//! than a saved bridge cache. The first valid owner response is the memo;
//! its chat seq is the fresh turn's cause, protected by the usual claim.
use serde_json::{json, Value};

use crate::records::{self, Closed, Record, Said};
use crate::runtime::Agent;

/// A new message and the card's closure, or just its expiry. Missing or
/// ambiguous history fails closed. Only records signed by the agent own
/// cards, steps and closures; only its owner may choose an option.
pub fn resolve(agent: &Agent, answer: &Record, work: &[Record], chat: &[Record], now: u64) -> Option<(Option<Value>, Value)> {
    let Said::PromptResponse { prompt, option, text, card } = records::said(&answer.body) else { return None };
    if answer.principal != agent.owner || card.as_ref().is_some_and(|c| c.agent != agent.identity) {
        return None;
    }
    let own = |r: &&Record| r.principal == agent.identity;
    let mut cards = work.iter().filter(own).filter(|r| r.body["kind"] == "turn.prompt" && r.body["prompt"] == prompt && card.as_ref().is_none_or(|c| r.body["turn"] == c.turn));
    let asked = cards.next()?;
    if cards.next().is_some() || asked.body["asks"] != agent.owner {
        return None;
    }
    let turn = asked.body["turn"].as_str()?;
    let expiry = asked.body["expiresAt"].as_u64()?;
    let options = asked.body["options"].as_array()?;
    let choice = options.iter().find(|o| o["id"] == option)?;
    let valid = |r: &&Record| {
        if r.principal != agent.owner || r.at < asked.at || u64::try_from(r.at).unwrap_or(u64::MAX) >= expiry { return false }
        match records::said(&r.body) {
            Said::PromptResponse { prompt: p, option: o, card: c, .. } => p == prompt && c.is_none_or(|c| c.agent == agent.identity && c.turn == turn) && options.iter().any(|x| x["id"] == o),
            _ => false,
        }
    };
    // The driver reads through the current answer and back through the
    // card's creation. Never let a second answer win after a cache loss.
    let first = chat.iter().filter(valid).min_by_key(|r| r.seq);
    let closed = work.iter().filter(own).find(|r| r.body["kind"] == "turn.prompt.closed" && r.body["turn"] == turn && r.body["prompt"] == prompt);
    if let Some(closed) = closed {
        if closed.body["outcome"] != "answered" || closed.body["answerSeq"] != answer.seq { return None }
    }
    if first.is_none() && now >= expiry {
        return Some((None, records::turn_prompt_closed(turn, &prompt, Closed::Expired, None)));
    }
    if first?.seq != answer.seq { return None }
    // A stopped turn never lends permission to a later turn, even if its
    // prompt closure failed to land before the stop.
    if work.iter().filter(own).any(|r| r.body["kind"] == "turn.end" && r.body["turn"] == turn && r.body["outcome"] == "stopped") { return None }
    let words = text.filter(|_| choice["words"] == true);
    let mut closed = records::turn_prompt_closed(turn, &prompt, Closed::Answered, Some((&option, &answer.principal, words.as_deref())));
    closed["answerSeq"] = json!(answer.seq);
    let start = work.iter().filter(own).find(|r| r.body["kind"] == "turn.start" && r.body["turn"] == turn)?;
    if chat.iter().any(|r| r.seq < answer.seq && r.at >= asked.at && r.principal == start.body["asker"].as_str().unwrap_or("") && matches!(records::said(&r.body), Said::Stop { turn: Some(t) } if t == turn)) {
        return Some((None, records::turn_prompt_closed(turn, &prompt, Closed::Stopped, None)));
    }
    let cause = &start.body["cause"];
    let original = chat.iter().find(|r| cause["channel"] == r.channel && cause["seq"] == r.seq).and_then(crate::note::asked)?;
    let steps: Vec<&Value> = work.iter().filter(own).filter(|r| r.body["turn"] == turn && r.body["kind"] == "turn.step").map(|r| &r.body).collect();
    let original = records::cut_bytes(&original, 8192);
    let question = records::cut_bytes(asked.body["text"].as_str()?, 4096);
    let label = records::cut_bytes(choice["label"].as_str()?, 512);
    let owner_words = words.as_ref().map(|s| format!("\nOwner's words: {}", records::cut_bytes(s, 8192))).unwrap_or_default();
    let context = records::cut_bytes(&serde_json::to_string(&json!({ "steps": steps, "replies": crate::note::replies(chat, &agent.identity, turn) })).expect("context serializes"), crate::limits::NOTE_MAX_BYTES);
    let message = format!("A late answer to your earlier question. This is a fresh turn; do not resume or replay the old turn. Check what already happened before acting, and do not ask the same question again.\nQuestion: {}\nChosen answer: {} ({option}){}\nOriginal request: {original}\nCut-turn steps: {context}", question, label, owner_words);
    Some((Some(json!({ "text": message, "to": [agent.identity] })), closed))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn record(seq: u64, by: &str, body: Value) -> Record {
        Record { channel: "chat".into(), seq, at: 10, principal: by.into(), kind: "message".into(), body }
    }
    fn fixture() -> (Agent, Vec<Record>, Vec<Record>) {
        let a = Agent { fragment: "agent".into(), identity: "npub1agent".into(), name: "agent".into(), owner: "npub1owner".into(), credentials: vec![], key: None };
        let work = vec![record(1, &a.identity, json!({"kind":"turn.start","turn":"old","cause":{"fragment":"chat","channel":"chat","seq":1}})), record(2, &a.identity, json!({"kind":"turn.prompt","turn":"old","prompt":"p","text":"Publish?","asks":a.owner,"expiresAt":1000,"options":[{"id":"yes","label":"Publish"},{"id":"no","label":"Keep private"}]}))];
        let chat = vec![record(1, &a.owner, json!({"text":"Publish my draft after asking me"})), record(2, &a.owner, json!({"kind":"prompt_response","prompt":"p","option":"yes","agent":a.identity,"turn":"old"}))];
        (a, work, chat)
    }
    #[test]
    fn late_answer_has_question_choice_and_original_request() {
        let (a, work, chat) = fixture();
        let (message, closed) = resolve(&a, &chat[1], &work, &chat, 20).unwrap();
        let text = message.unwrap()["text"].as_str().unwrap().to_owned();
        for expected in ["Publish?", "Publish (yes)", "Publish my draft", "fresh turn"] { assert!(text.contains(expected), "{text}") }
        assert_eq!(closed["answerSeq"], 2);
        let mut saved = work.clone();
        saved.push(record(3, &a.identity, closed));
        assert_eq!(resolve(&a, &chat[1], &saved, &chat, 20), resolve(&a, &chat[1], &work, &chat, 20), "replay reconstructs the same turn after cache loss");
    }
    #[test]
    fn only_first_valid_owner_answer_wins_and_stop_and_expiry_hold() {
        let (a, mut work, mut chat) = fixture();
        for bad in [json!({"kind":"prompt_response","prompt":"p","option":"invalid"}), json!({"kind":"prompt_response","prompt":"p","option":"yes","agent":"npub1other","turn":"old"})] {
            let r = record(2, &a.owner, bad);
            assert!(resolve(&a, &r, &work, &[chat[0].clone(), r.clone()], 20).is_none());
        }
        let mut stranger = chat[1].clone(); stranger.principal = "npub1stranger".into();
        assert!(resolve(&a, &stranger, &work, &chat, 20).is_none());
        let second = record(3, &a.owner, json!({"kind":"prompt_response","prompt":"p","option":"no"}));
        chat.push(second.clone());
        assert!(resolve(&a, &second, &work, &chat, 20).is_none());
        work.push(record(3, &a.identity, json!({"kind":"turn.end","turn":"old","outcome":"stopped"})));
        assert!(resolve(&a, &chat[1], &work, &chat, 20).is_none());
        work.pop();
        let mut expired = chat[1].clone(); expired.at = 1000;
        let result = resolve(&a, &expired, &work, &[chat[0].clone(), expired.clone()], 1000).unwrap();
        assert!(result.0.is_none()); assert_eq!(result.1["outcome"], "expired");
    }
}
