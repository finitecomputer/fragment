//! Late card answers are new turns, reconstructed from the journal rather
//! than a saved bridge cache. The first valid owner response is the memo;
//! its chat seq is the fresh turn's cause, protected by the usual claim.
use serde_json::{json, Value};

use crate::records::{self, Closed, Record, Said};
use crate::runtime::Agent;
use crate::history::Unavailable;

/// A new message and the card's closure, or just its expiry. Missing or
/// ambiguous history returns an explicit error for a visible terminal card. Only records signed by the agent own
/// cards, steps and closures; only its owner may choose an option.
pub fn resolve(agent: &Agent, answer: &Record, work: &[Record], chat: &[Record], now: u64) -> Result<Option<(Option<Value>, Value)>, Unavailable> {
    let Said::PromptResponse { prompt, option, text, card } = records::said(&answer.body) else { return Ok(None) };
    if answer.principal != agent.owner || card.as_ref().is_some_and(|c| c.agent != agent.identity) { return Ok(None) }
    let own = |r: &&Record| r.principal == agent.identity;
    let mut cards = work.iter().filter(own).filter(|r| r.body["kind"] == "turn.prompt" && r.body["prompt"] == prompt && card.as_ref().is_none_or(|c| r.body["turn"] == c.turn));
    let asked = cards.next().ok_or(Unavailable::Missing)?;
    if cards.next().is_some() || asked.body["asks"] != agent.owner { return Err(Unavailable::Corrupt) }
    let turn = asked.body["turn"].as_str().ok_or(Unavailable::Corrupt)?;
    let closed = work.iter().filter(own).find(|r| r.body["kind"] == "turn.prompt.closed" && r.body["turn"] == turn && r.body["prompt"] == prompt);
    if let Some(closed) = closed {
        if closed.body["outcome"] != "answered" || closed.body["answerSeq"] != answer.seq { return Ok(None) }
    }
    if work.iter().filter(own).any(|r| r.body["kind"] == "turn.end" && r.body["turn"] == turn && r.body["outcome"] == "stopped") { return Ok(None) }
    let expiry = asked.body["expiresAt"].as_u64().ok_or(Unavailable::Corrupt)?;
    let options = asked.body["options"].as_array().ok_or(Unavailable::Corrupt)?;
    if options.is_empty() || options.len() > crate::limits::PROMPT_OPTIONS_MAX || options.iter().any(|o| o["id"].as_str().is_none() || o["label"].as_str().is_none()) { return Err(Unavailable::Corrupt) }
    let Some(choice) = options.iter().find(|o| o["id"] == option) else { return Ok(None) };
    if !chat.iter().any(|r| r.channel == answer.channel && r.seq == answer.seq && r.principal == answer.principal && r.body == answer.body) { return Err(Unavailable::Missing) }
    let valid = |r: &&Record| {
        if r.channel != records::CHAT || r.principal != agent.owner || r.at < asked.at || u64::try_from(r.at).unwrap_or(u64::MAX) >= expiry { return false }
        match records::said(&r.body) {
            Said::PromptResponse { prompt: p, option: o, card: c, .. } => p == prompt && c.is_none_or(|c| c.agent == agent.identity && c.turn == turn) && options.iter().any(|x| x["id"] == o),
            _ => false,
        }
    };
    let first = chat.iter().filter(valid).min_by_key(|r| r.seq);
    if first.is_none() && now >= expiry { return Ok(Some((None, records::turn_prompt_closed(turn, &prompt, Closed::Expired, None)))) }
    if first.ok_or(Unavailable::Missing)?.seq != answer.seq { return Ok(None) }
    let start = work.iter().filter(own).find(|r| r.body["kind"] == "turn.start" && r.body["turn"] == turn).ok_or(Unavailable::Missing)?;
    let asker = start.body["asker"].as_str().ok_or(Unavailable::Corrupt)?;
    if chat.iter().any(|r| r.channel == records::CHAT && r.seq < answer.seq && r.at >= asked.at && r.principal == asker && matches!(records::said(&r.body), Said::Stop { turn: Some(t) } if t == turn)) {
        return Ok(Some((None, records::turn_prompt_closed(turn, &prompt, Closed::Stopped, None))));
    }
    let cause: records::Cause = serde_json::from_value(start.body["cause"].clone()).map_err(|_| Unavailable::Corrupt)?;
    let original = chat.iter().find(|r| cause.channel == r.channel && cause.seq == r.seq).and_then(crate::note::asked).ok_or(Unavailable::Missing)?;
    let steps: Vec<&Value> = work.iter().filter(own).filter(|r| r.body["turn"] == turn && r.body["kind"] == "turn.step").map(|r| &r.body).collect();
    if steps.iter().any(|s| s["tool"].as_str().is_none() || s["args"].as_str().is_none() || s["ok"].as_bool().is_none()) { return Err(Unavailable::Corrupt) }
    let question = asked.body["text"].as_str().ok_or(Unavailable::Corrupt)?;
    let label = choice["label"].as_str().ok_or(Unavailable::Corrupt)?;
    let words = text.filter(|_| choice["words"] == true);
    if choice["words"] == true && words.as_ref().is_none_or(|s| s.trim().is_empty()) { return Err(Unavailable::Missing) }
    let context = serde_json::to_string(&json!({ "steps": steps, "replies": crate::note::replies(chat, &agent.identity, turn) })).expect("context serializes");
    // Never silently cut off required authorization or the original ask.
    if original.len() > 8192 || question.len() > 4096 || label.len() > 512 || words.as_ref().is_some_and(|s| s.len() > 8192) || context.len() > crate::limits::NOTE_MAX_BYTES { return Err(Unavailable::Budget) }
    let owner_words = words.as_ref().map(|s| format!("\nOwner's words: {s}")).unwrap_or_default();
    let message = format!("A late answer to your earlier question. This is a fresh turn; do not resume or replay the old turn. Check what already happened before acting, and do not ask the same question again.\nQuestion: {question}\nChosen answer: {label} ({option}){owner_words}\nOriginal request: {original}\nCut-turn steps: {context}");
    let mut closed = records::turn_prompt_closed(turn, &prompt, Closed::Answered, Some((&option, &answer.principal, words.as_deref())));
    closed["answerSeq"] = json!(answer.seq);
    Ok(Some((Some(json!({ "text": message, "to": [agent.identity] })), closed)))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn record(seq: u64, by: &str, body: Value) -> Record {
        Record { channel: "chat".into(), seq, at: 10, principal: by.into(), kind: "message".into(), body }
    }
    fn fixture() -> (Agent, Vec<Record>, Vec<Record>) {
        let a = Agent { fragment: "agent".into(), identity: "npub1agent".into(), name: "agent".into(), owner: "npub1owner".into(), credentials: vec![], key: None };
        let work = vec![record(1, &a.identity, json!({"kind":"turn.start","turn":"old","asker":"npub1owner","cause":{"fragment":"chat","channel":"chat","seq":1}})), record(2, &a.identity, json!({"kind":"turn.prompt","turn":"old","prompt":"p","text":"Publish?","asks":a.owner,"expiresAt":1000,"options":[{"id":"yes","label":"Publish"},{"id":"no","label":"Keep private"}]}))];
        let chat = vec![record(1, &a.owner, json!({"text":"Publish my draft after asking me"})), record(2, &a.owner, json!({"kind":"prompt_response","prompt":"p","option":"yes","agent":a.identity,"turn":"old"}))];
        (a, work, chat)
    }
    #[test]
    fn late_answer_has_question_choice_and_original_request() {
        let (a, work, chat) = fixture();
        let (message, closed) = resolve(&a, &chat[1], &work, &chat, 20).unwrap().unwrap();
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
            assert!(resolve(&a, &r, &work, &[chat[0].clone(), r.clone()], 20).unwrap().is_none());
        }
        let mut stranger = chat[1].clone(); stranger.principal = "npub1stranger".into();
        assert!(resolve(&a, &stranger, &work, &chat, 20).unwrap().is_none());
        let second = record(3, &a.owner, json!({"kind":"prompt_response","prompt":"p","option":"no"}));
        chat.push(second.clone());
        assert!(resolve(&a, &second, &work, &chat, 20).unwrap().is_none());
        work.push(record(3, &a.identity, json!({"kind":"turn.end","turn":"old","outcome":"stopped"})));
        assert!(resolve(&a, &chat[1], &work, &chat, 20).unwrap().is_none());
        work.pop();
        let mut expired = chat[1].clone(); expired.at = 1000;
        let result = resolve(&a, &expired, &work, &[chat[0].clone(), expired.clone()], 1000).unwrap().unwrap();
        assert!(result.0.is_none()); assert_eq!(result.1["outcome"], "expired");
    }
}

#[cfg(test)]
mod missing_context_tests {
    use super::*;
    fn r(seq:u64, body:Value) -> Record {Record {channel:records::CHAT.into(),seq,at:10,principal:"npub1agent".into(),kind:"message".into(),body}}
    #[test]
    fn legacy_missing_or_corrupt_context_never_authorizes_a_fresh_turn() {
        let a = Agent {fragment:"agent".into(),identity:"npub1agent".into(),name:"Agent".into(),owner:"npub1owner".into(),credentials:vec![],key:None};
        let start = r(1,json!({"kind":"turn.start","turn":"old","asker":a.owner,"cause":{"fragment":"chat","channel":"chat","seq":1}}));
        let card = r(2,json!({"kind":"turn.prompt","turn":"old","prompt":"p","text":"Publish?","asks":a.owner,"expiresAt":1000,"options":[{"id":"yes","label":"Publish"}]}));
        let ask = Record {principal:a.owner.clone(),..r(1,json!({"text":"Publish the draft after asking me"}))};
        let answer = Record {principal:a.owner.clone(),..r(2,json!({"kind":"prompt_response","agent":a.identity,"turn":"old","prompt":"p","option":"yes"}))};
        let work = vec![start.clone(),card.clone()];
        let chat = vec![ask.clone(),answer.clone()];
        assert!(resolve(&a,&answer,&work,&chat,20).unwrap().is_some());
        assert_eq!(resolve(&a,&answer,std::slice::from_ref(&card),&chat,20),Err(Unavailable::Missing));
        assert_eq!(resolve(&a,&answer,&work,std::slice::from_ref(&answer),20),Err(Unavailable::Missing));
        let mut legacy = start.clone(); legacy.body.as_object_mut().unwrap().remove("cause");
        assert_eq!(resolve(&a,&answer,&[legacy,card.clone()],&chat,20),Err(Unavailable::Corrupt));
        let large = Record {body:json!({"text":"x".repeat(8193)}),..ask};
        assert_eq!(resolve(&a,&answer,&work,&[large,answer.clone()],20),Err(Unavailable::Budget));
        let mut legacy = card; legacy.body["options"][0].as_object_mut().unwrap().remove("label");
        assert_eq!(resolve(&a,&answer,&[start,legacy],&chat,20),Err(Unavailable::Corrupt));
    }
}
