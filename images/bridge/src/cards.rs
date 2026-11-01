//! A card outlives its turn (docs/bridge.md, "A card outlives its turn"):
//! an answer to a card that no running turn holds starts a fresh turn of
//! its own, rebuilt from the chat's journal, never the old turn resumed.
//! Pure: records in (driver.rs, `card_history`), the card's closure and the
//! fresh turn out (engine.rs, `card_record`).
use serde_json::{json, Value};

use crate::history::Unavailable;
use crate::limits;
use crate::note;
use crate::records::{self, Closed, Record, Said};
use crate::runtime::Agent;

/// What a late answer does: its card's closure, and the fresh turn when
/// it is the card's first answer.
#[derive(Debug, PartialEq)]
pub struct Resolved {
    pub closed: Value,
    pub fresh: Option<Fresh>,
}

/// A fresh turn's message, the owner's answer alone (the question, the
/// option chosen or the words typed), and its note: what the card's turn
/// was asked, by whom, its steps and replies, which runtimes hand as
/// read-only context, never as the owner's words.
#[derive(Debug, PartialEq)]
pub struct Fresh {
    pub text: String,
    pub note: String,
    /// The answer as its runtime reads it (`runtime::LateCard`).
    pub late: crate::runtime::LateCard,
}

/// `answer`, an owner's `prompt_response` on `chat`, against the agent's
/// records on `work` back to its card's turn's start and `chat` back to
/// the card, both from the answer on; `cause`, the record that started the
/// card's turn, when it was read; `name`, what the chat calls a principal.
/// `None`: nothing to do (not the owner's, not its card's option, or the
/// card closed already). Only a closure this path wrote (`answerSeq`, the
/// answer's seq) lets its answer rebuild the fresh turn again (a retry
/// after a restart, whose claim is the same): a card answered live, then
/// rolled back past, never runs again.
pub fn resolve(agent: &Agent, answer: &Record, work: &[Record], chat: &[Record], cause: Option<&Record>, name: impl Fn(&str) -> String, now: u64) -> Result<Option<Resolved>, Unavailable> {
    let Said::PromptResponse { prompt, option, text, card } = records::said(&answer.body) else { return Ok(None) };
    if answer.principal != agent.owner || card.as_ref().is_some_and(|c| c.agent != agent.identity) {
        return Ok(None);
    }
    let of = |r: &Record, kind: &str| r.principal == agent.identity && r.body["kind"] == kind;
    let mut cards = work.iter().filter(|r| of(r, "turn.prompt") && r.body["prompt"] == prompt.as_str() && card.as_ref().is_none_or(|c| r.body["turn"] == c.turn.as_str()));
    let asked = cards.next().ok_or(Unavailable::Missing)?;
    if cards.next().is_some() {
        // the same prompt id in two turns, and an answer that names neither
        return Err(Unavailable::Corrupt);
    }
    if asked.body["asks"] != agent.owner.as_str() {
        return Ok(None);
    }
    let turn = asked.body["turn"].as_str().ok_or(Unavailable::Corrupt)?;
    if let Some(c) = work.iter().find(|r| of(r, "turn.prompt.closed") && r.body["turn"] == turn && r.body["prompt"] == prompt.as_str()) {
        if c.body["outcome"] != "answered" || c.body["answerSeq"] != answer.seq {
            return Ok(None);
        }
    }
    if work.iter().any(|r| of(r, "turn.end") && r.body["turn"] == turn && r.body["outcome"] == "stopped") {
        return Ok(None);
    }
    let options = asked.body["options"].as_array().ok_or(Unavailable::Corrupt)?;
    let Some(choice) = options.iter().find(|o| o["id"] == option.as_str()) else { return Ok(None) };
    // answered on the computer alone, never by a record (`PromptOption::computer`)
    if choice["computer"] == true {
        return Ok(None);
    }
    let label = choice["label"].as_str().ok_or(Unavailable::Corrupt)?;
    let expires = asked.body["expiresAt"].as_i64().ok_or(Unavailable::Corrupt)?;
    let closed = |how: Closed| records::turn_prompt_closed(turn, &prompt, how, None);
    // the first of its owner's answers given in its life wins, however late read
    let valid = |r: &&Record| {
        r.channel == records::CHAT && r.principal == agent.owner && r.at >= asked.at && r.at < expires
            && matches!(records::said(&r.body), Said::PromptResponse { prompt: p, option: o, card: c, .. } if p == prompt && c.as_ref().is_none_or(|c| c.agent == agent.identity && c.turn == turn) && options.iter().any(|x| x["id"] == o.as_str()))
    };
    match chat.iter().filter(valid).min_by_key(|r| r.seq) {
        None if i64::try_from(now).unwrap_or(i64::MAX) >= expires => return Ok(Some(Resolved { closed: closed(Closed::Expired), fresh: None })),
        Some(first) if first.seq == answer.seq => {}
        _ => return Ok(None),
    }
    let start = work.iter().find(|r| of(r, "turn.start") && r.body["turn"] == turn).ok_or(Unavailable::Missing)?;
    let asker = start.body["asker"].as_str().ok_or(Unavailable::Corrupt)?;
    if chat.iter().any(|r| r.channel == records::CHAT && r.seq < answer.seq && r.at >= asked.at && r.principal == asker && matches!(records::said(&r.body), Said::Stop { turn: Some(t) } if t == turn)) {
        return Ok(Some(Resolved { closed: closed(Closed::Stopped), fresh: None }));
    }
    let cut = note::turn(work, &agent.identity, turn).ok_or(Unavailable::Corrupt)?;
    let question = asked.body["text"].as_str().ok_or(Unavailable::Corrupt)?;
    // a card that takes words is answered by them; no words, its label
    let words = text.filter(|_| choice["words"] == true);
    let text = format!("Answer to your earlier question “{question}”: {}", words.as_deref().unwrap_or(label));
    if text.len() > limits::MESSAGE_TEXT_MAX_BYTES {
        // never a cut of what the owner chose
        return Err(Unavailable::Budget);
    }
    let note = note::late(&cut, &name(asker), cause.and_then(note::asked).as_deref(), &note::replies(chat, &agent.identity, turn));
    let mut closed = records::turn_prompt_closed(turn, &prompt, Closed::Answered, Some((&option, &answer.principal, words.as_deref())));
    closed["answerSeq"] = json!(answer.seq);
    let late = crate::runtime::LateCard { prompt: prompt.clone(), option: option.clone(), request: cause.and_then(note::asked) };
    Ok(Some(Resolved { closed, fresh: Some(Fresh { text, note, late }) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: &str = "npub1owner";
    const ME: &str = "npub1agent";

    fn agent() -> Agent {
        Agent { fragment: "agent".into(), identity: ME.into(), name: "agent".into(), owner: OWNER.into(), credentials: vec![], key: None, runtime: None }
    }

    fn rec(channel: &str, seq: u64, at: i64, by: &str, body: Value) -> Record {
        Record { channel: channel.into(), seq, at, principal: by.into(), kind: "message".into(), body }
    }

    fn response(option: &str) -> Value {
        json!({ "kind": "prompt_response", "prompt": "p", "option": option, "agent": ME, "turn": "old" })
    }

    /// A turn as a real one runs: twelve tool calls, a long reply, then
    /// its card, asked of the owner by their guest's request.
    fn history() -> (Vec<Record>, Vec<Record>) {
        let mut work = vec![rec("work", 1, 10, ME, json!({ "kind": "turn.start", "turn": "old", "asker": "npub1guest", "agent": ME, "cause": { "fragment": "chat", "channel": "chat", "seq": 1 } }))];
        for i in 0..12u64 {
            work.push(rec("work", 2 + i, 11, ME, json!({ "kind": "turn.step", "turn": "old", "tool": "terminal", "args": format!("ls /data/work/draft-{i} && cat notes-{i}.md"), "ok": true, "excerpt": "x".repeat(400) })));
        }
        work.push(rec("work", 14, 12, ME, json!({ "kind": "turn.prompt", "turn": "old", "prompt": "p", "text": "Publish?", "asks": OWNER, "expiresAt": 1000, "options": [{ "id": "yes", "label": "Publish" }, { "id": "no", "label": "Keep private" }, { "id": "other", "label": "Other", "words": true }] })));
        let chat = vec![
            rec("chat", 1, 9, "npub1guest", json!({ "text": format!("Publish my draft after asking Paul. {}", "Details. ".repeat(400)) })),
            rec("chat", 2, 11, ME, json!({ "text": format!("Here is my plan: {}", "a long paragraph. ".repeat(300)), "turn": "old" })),
            rec("chat", 3, 20, OWNER, response("yes")),
        ];
        (work, chat)
    }

    fn name(p: &str) -> String {
        if p == "npub1guest" { "Robin".into() } else { "Paul".into() }
    }

    fn late(work: &[Record], chat: &[Record], answer: &Record, now: u64) -> Result<Option<Resolved>, Unavailable> {
        resolve(&agent(), answer, work, chat, Some(&chat[0]), name, now)
    }

    /// Goal: a realistic turn's late answer starts a fresh turn whose
    /// message is the owner's answer alone, its context (who asked what,
    /// the steps, the reply) a bounded note; a retry rebuilds the same.
    #[test]
    fn a_late_answer_after_a_long_turn_is_the_answer_with_its_turn_as_context() {
        let (work, chat) = history();
        let r = late(&work, &chat, &chat[2], 20).unwrap().unwrap();
        let fresh = r.fresh.as_ref().unwrap();
        assert_eq!(fresh.text, "Answer to your earlier question “Publish?”: Publish");
        assert!(fresh.note.len() <= limits::NOTE_MAX_BYTES, "{}", fresh.note.len());
        for said in ["is not resumed", "It was answering Robin: “Publish my draft after asking Paul.", "(the rest is cut)", "Its steps, as recorded: terminal ls /data/work/draft-0", "and 4 more", "It had replied: “Here is my plan:"] {
            assert!(fresh.note.contains(said), "{said:?} in {}", fresh.note);
        }
        assert!(!fresh.note.contains(&"x".repeat(100)), "no excerpt is said");
        // and the answer as its runtime reads it (Alex on #901): which card,
        // which option, what its turn was asked, never parsed from words
        assert_eq!((fresh.late.prompt.as_str(), fresh.late.option.as_str()), ("p", "yes"));
        assert!(fresh.late.request.as_deref().is_some_and(|r| r.starts_with("Publish my draft after asking Paul.")), "{:?}", fresh.late.request);
        assert_eq!((r.closed["outcome"].as_str(), r.closed["option"].as_str(), r.closed["answerSeq"].as_u64()), (Some("answered"), Some("yes"), Some(3)));
        let mut closed = work.clone();
        closed.push(rec("work", 15, 21, ME, r.closed.clone()));
        assert_eq!(late(&closed, &chat, &chat[2], 20).unwrap(), Some(r), "a retry after a restart rebuilds the same fresh turn: its claim is the same");
        let words = rec("chat", 3, 20, OWNER, json!({ "kind": "prompt_response", "prompt": "p", "option": "other", "text": "only the summary", "agent": ME, "turn": "old" }));
        let r = late(&work, &[chat[0].clone(), chat[1].clone(), words.clone()], &words, 20).unwrap().unwrap();
        assert_eq!(r.fresh.unwrap().text, "Answer to your earlier question “Publish?”: only the summary");
        let no_cause = resolve(&agent(), &chat[2], &work, &chat, None, name, 20).unwrap().unwrap();
        let no_cause = no_cause.fresh.unwrap();
        assert!(no_cause.note.contains("It was answering Robin; what they asked is not in this chat's record."));
        assert_eq!(no_cause.late.request, None, "no request it was not told");
    }

    /// Goal (the rollback's double run): only a closure the late path wrote
    /// names its answer. A card answered live, then a `/data` rolled back
    /// to before the answer, reads the answer again and starts nothing.
    #[test]
    fn a_card_answered_live_never_runs_again() {
        let (mut work, chat) = history();
        work.push(rec("work", 15, 21, ME, records::turn_prompt_closed("old", "p", Closed::Answered, Some(("yes", OWNER, None)))));
        assert_eq!(late(&work, &chat, &chat[2], 20), Ok(None));
        for how in [Closed::Expired, Closed::Stopped, Closed::Unavailable] {
            assert_eq!(serde_json::to_value(how).unwrap(), how.as_str(), "one name on the wire and in the state");
            let mut w = history().0;
            w.push(rec("work", 15, 21, ME, records::turn_prompt_closed("old", "p", how, None)));
            assert_eq!(late(&w, &chat, &chat[2], 20), Ok(None), "{how:?} is final");
        }
    }

    /// Goal: the first valid owner answer in the card's life wins; anyone
    /// else's, another card's, an option not offered, a later one, a Stop
    /// or a stopped end and an expiry each start nothing.
    #[test]
    fn only_the_first_valid_owner_answer_starts_a_turn() {
        let (work, chat) = history();
        for bad in [json!({ "kind": "prompt_response", "prompt": "p", "option": "maybe", "agent": ME, "turn": "old" }), json!({ "kind": "prompt_response", "prompt": "p", "option": "yes", "agent": "npub1other", "turn": "old" })] {
            let r = rec("chat", 3, 20, OWNER, bad);
            assert_eq!(late(&work, &[chat[0].clone(), chat[1].clone(), r.clone()], &r, 20), Ok(None));
        }
        let stranger = Record { principal: "npub1stranger".into(), ..chat[2].clone() };
        assert_eq!(late(&work, &chat, &stranger, 20), Ok(None));
        let second = rec("chat", 4, 21, OWNER, response("no"));
        let both = [chat.clone(), vec![second.clone()]].concat();
        assert_eq!(late(&work, &both, &second, 21), Ok(None), "the first answer won");
        let stop = rec("chat", 3, 19, "npub1guest", json!({ "kind": "stop", "turn": "old" }));
        let answer = rec("chat", 4, 20, OWNER, response("yes"));
        let r = late(&work, &[chat[0].clone(), chat[1].clone(), stop, answer.clone()], &answer, 20).unwrap().unwrap();
        assert_eq!((r.closed["outcome"].as_str(), r.fresh), (Some("stopped"), None));
        let mut stopped = work.clone();
        stopped.push(rec("work", 15, 13, ME, json!({ "kind": "turn.end", "turn": "old", "outcome": "stopped" })));
        assert_eq!(late(&stopped, &chat, &chat[2], 20), Ok(None));
        let after = Record { at: 1000, ..chat[2].clone() };
        let r = late(&work, &[chat[0].clone(), chat[1].clone(), after.clone()], &after, 1000).unwrap().unwrap();
        assert_eq!((r.closed["outcome"].as_str(), r.fresh), (Some("expired"), None));
        assert!(late(&work, &chat, &chat[2], 5000).unwrap().unwrap().fresh.is_some(), "given in time, read late: honored");
    }

    /// Goal: what cannot be rebuilt whole is refused, never guessed: the
    /// card or its turn's start not read, a start with no cause, or the
    /// owner's words past a message's bound.
    #[test]
    fn missing_or_corrupt_history_never_authorizes_a_turn() {
        let (work, chat) = history();
        assert_eq!(late(&work[1..], &chat, &chat[2], 20), Err(Unavailable::Missing), "no start");
        assert_eq!(late(&work[..13], &chat, &chat[2], 20), Err(Unavailable::Missing), "no card");
        let mut legacy = work.clone();
        legacy[0].body.as_object_mut().unwrap().remove("cause");
        assert_eq!(late(&legacy, &chat, &chat[2], 20), Err(Unavailable::Corrupt));
        let long = rec("chat", 3, 20, OWNER, json!({ "kind": "prompt_response", "prompt": "p", "option": "other", "text": "w".repeat(limits::MESSAGE_TEXT_MAX_BYTES), "agent": ME, "turn": "old" }));
        assert_eq!(late(&work, &[chat[0].clone(), chat[1].clone(), long.clone()], &long, 20), Err(Unavailable::Budget));
    }
}
