//! simplex-chat's bot API (`simplex-chat -p <port>`, a WebSocket on
//! 127.0.0.1 with no authentication), as v7.0.3 speaks it: the commands the
//! link sends, and what it reads of the core's answers and events. Every
//! shape here was read from frames the real core sent in the lab
//! (tests/simplex.rs; fixtures/ holds some), not from its documentation.
//!
//! A command goes out as `{"corrId": "<n>", "cmd": "<line>"}`; its answer
//! comes back as `{"corrId": "<n>", "resp": {…}}`, and anything the core
//! says on its own as `{"resp": {…}}`. A `resp` is tagged by `type`. The
//! core sends much more than is read here (each contact's whole profile and
//! preferences, the user, its servers): what is not named is ignored.

use serde_json::{json, Value};

/// A contact's id in the core's own database (`contactId`).
pub type ContactId = i64;
/// A connection's id in the core's own database (`pccConnId`, `connId`).
pub type ConnId = i64;

/// A frame from the core, at most (a chat's last items, each with its
/// contact's profile, come to a few KB each).
pub const FRAME_MAX_BYTES: usize = 4 * 1024 * 1024;
/// Items one frame may carry, at most; past it the rest are not read.
pub const ITEMS_MAX: usize = 256;
/// A message's text, at most, as a chat record takes it (docs/chat-records.md:
/// 32 KiB, more is cut).
pub const TEXT_MAX_BYTES: usize = 32 * 1024;
/// A contact's name, at most, as the link keeps and tells it.
pub const NAME_MAX_BYTES: usize = 64;
/// A link the core makes, at most (a one-time invitation is about 700 bytes).
pub const LINK_MAX_BYTES: usize = 4 * 1024;
/// One message the link sends, at most, as its `/_send` line's JSON text.
/// Measured in the lab on v7.0.3: a text of 15,400 bytes was sent and one of
/// 15,600 refused (`errorStore largeMsg`), so a reply is sent in parts of at
/// most this, with room for the escaping JSON adds and the core's envelope.
pub const SEND_JSON_MAX_BYTES: usize = 12_000;
/// What an error says, at most, as the link logs it.
const ERROR_MAX_BYTES: usize = 300;

const _: () = assert!(SEND_JSON_MAX_BYTES < 15_400 && TEXT_MAX_BYTES <= FRAME_MAX_BYTES, "a part is under the size the core took");

/// A command to the core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    /// A one-time invitation (`/connect` with no link): the core answers
    /// `invitation`, its link and its connection. Only the first to accept
    /// it connects; anyone after is refused by the SMP server (`AUTH`).
    Invite,
    /// A text to a contact.
    Send { contact: ContactId, text: String },
    /// Ask the contact to synchronize the ratchet again. Unforced, the core
    /// refuses it unless it found the ratchet out of step itself (`CMD
    /// PROHIBITED`); forced, it always starts one.
    Sync { contact: ContactId, force: bool },
    /// The contact, deleted with its chat; `notify`, told so.
    Delete { contact: ContactId, notify: bool },
    /// A connection that never became a contact (a pending invitation).
    DeleteConn { conn: ConnId },
    /// The contact's newest `count` items, or with `before`, the `count`
    /// just older than that item (a catch-up's next page back).
    Chat { contact: ContactId, count: u16, before: Option<i64> },
}

impl Cmd {
    /// The command's line, as the core reads it.
    pub fn line(&self) -> String {
        match self {
            Cmd::Invite => "/connect".into(),
            Cmd::Send { contact, text } => {
                assert!(*contact > 0 && !text.is_empty(), "a send names a contact and says something");
                let body = json!([{ "msgContent": { "type": "text", "text": text } }]);
                format!("/_send @{contact} json {body}")
            }
            Cmd::Sync { contact, force } => {
                assert!(*contact > 0, "a sync names a contact");
                if *force {
                    format!("/_sync @{contact} force=on")
                } else {
                    format!("/_sync @{contact}")
                }
            }
            Cmd::Delete { contact, notify } => {
                assert!(*contact > 0, "a delete names a contact");
                format!("/_delete @{contact} full notify={}", if *notify { "on" } else { "off" })
            }
            Cmd::DeleteConn { conn } => {
                assert!(*conn > 0, "a delete names a connection");
                format!("/_delete :{conn}")
            }
            Cmd::Chat { contact, count, before } => {
                assert!(*contact > 0 && *count > 0, "a read names a contact and a count");
                match before {
                    Some(item) => format!("/_get chat @{contact} before={item} count={count}"),
                    None => format!("/_get chat @{contact} count={count}"),
                }
            }
        }
    }

    /// Its frame, under `corr`.
    pub fn frame(&self, corr: u64) -> String {
        json!({ "corrId": corr.to_string(), "cmd": self.line() }).to_string()
    }
}

/// `text` in parts the core takes (each at most `SEND_JSON_MAX_BYTES` as
/// JSON), cut at a line's end where one is near, else at a char boundary;
/// none for an empty text.
pub fn parts(text: &str) -> Vec<String> {
    let mut out = vec![];
    let mut rest = text;
    // bounded: each pass takes at least one char off `rest`
    while !rest.is_empty() {
        let mut end = 0;
        let mut json = 2; // the quotes
        for (i, c) in rest.char_indices() {
            let more = json_len(c);
            if json + more > SEND_JSON_MAX_BYTES {
                break;
            }
            json += more;
            end = i + c.len_utf8();
        }
        assert!(end > 0, "a char fits in a part");
        if end < rest.len() {
            // a line's end in the part's last quarter, kept with the part
            if let Some(nl) = rest[..end].rfind('\n').filter(|nl| *nl >= end * 3 / 4) {
                end = nl + 1;
            }
        }
        out.push(rest[..end].to_string());
        rest = &rest[end..];
    }
    out
}

/// What `c` takes in a JSON string, as serde_json writes it.
fn json_len(c: char) -> usize {
    match c {
        '"' | '\\' | '\n' | '\r' | '\t' | '\u{08}' | '\u{0c}' => 2,
        c if (c as u32) < 0x20 => 6,
        c => c.len_utf8(),
    }
}

/// A frame from the core: its answer to `corr`, or (none) an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub corr: Option<u64>,
    pub resp: Resp,
}

/// What the core said, as far as the link reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resp {
    /// A one-time invitation made: its connection, and its link (the full
    /// `simplex:/invitation#…` form, which every SimpleX app reads, and the
    /// short `https://…` one the server serves, when it made one).
    Invitation { conn: ConnId, link: String, short: Option<String> },
    /// A contact connected, over connection `conn` (the invitation's, for
    /// one that came through it).
    Connected { contact: ContactId, conn: ConnId, name: String },
    /// New chat items (`newChatItems`, also a send's answer).
    Items(Vec<Item>),
    /// A page of a chat's items, oldest first (`apiChat`: a catch-up's).
    Page(Vec<Item>),
    /// A contact's ratchet synchronization moved (`contactRatchetSync`).
    Sync { contact: ContactId, status: SyncStatus },
    /// A forced synchronization started (`contactRatchetSyncStarted`).
    SyncStarted { contact: ContactId },
    /// A contact deleted, by this side (`contactDeleted`) or by theirs
    /// (`contactDeletedByContact`).
    Deleted { contact: ContactId, by_them: bool },
    /// A pending connection deleted (`contactConnectionDeleted`).
    ConnDeleted { conn: ConnId },
    /// A command failed (`chatCmdError`), or the core reports an error of
    /// its own (`chatError`): what it said, cut.
    Error { what: String },
    /// Anything else, by its type.
    Other(String),
}

/// A ratchet's synchronization, as the core names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncStatus {
    Ok,
    /// Out of step; a sync may be asked (the core allows it).
    Allowed,
    /// Out of step; nothing can be sent until a sync.
    Required,
    Started,
    Agreed,
}

/// One chat item of a contact's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The core's own id for it (`itemId`): where a catch-up reads on
    /// from. A rollback reuses ids, but takes the link's file back with
    /// the database, so the two agree.
    pub id: i64,
    pub contact: ContactId,
    /// Received (`directRcv`), else sent by this side.
    pub received: bool,
    pub content: Content,
    /// `itemSharedMsgId`: the id its sender's app gave the message, the same
    /// on both sides and kept by both. A rollback of this side's database
    /// reuses its own item ids, never this.
    pub shared: Option<String>,
    /// `itemTs`, as the core wrote it.
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    /// A message (`rcvMsgContent`, `sndMsgContent`): its kind (`text`,
    /// `image`, `file`, `voice`, `link`, …) and its text, cut at
    /// `TEXT_MAX_BYTES`.
    Message { kind: String, text: String },
    /// A message that would not decrypt (`rcvDecryptionError`).
    Undecryptable,
    /// Message ids skipped (`rcvIntegrityError`): messages this side never
    /// read (lost, or read by a life a rollback forgot).
    Skipped,
    /// Anything else (a banner, a feature, a connection event), by its type.
    Other(String),
}

/// Why a frame was not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    TooLarge(usize),
    NotJson(String),
    /// A frame with no `resp` object, or a known type missing a field.
    Shape(String),
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WireError::TooLarge(n) => write!(f, "a frame of {n} bytes, more than {FRAME_MAX_BYTES}"),
            WireError::NotJson(e) => write!(f, "a frame that is not JSON: {e}"),
            WireError::Shape(e) => write!(f, "a frame of another shape: {e}"),
        }
    }
}

/// `s`, cut to at most `max` bytes at a char boundary.
pub fn cut(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Reads one frame.
pub fn parse(text: &str) -> Result<Frame, WireError> {
    if text.len() > FRAME_MAX_BYTES {
        return Err(WireError::TooLarge(text.len()));
    }
    let v: Value = serde_json::from_str(text).map_err(|e| WireError::NotJson(e.to_string()))?;
    let resp = v.get("resp").filter(|r| r.is_object()).ok_or_else(|| WireError::Shape("no resp".into()))?;
    // the corrId of a command the link sent is its number; another's is no answer of ours
    let corr = v.get("corrId").and_then(Value::as_str).and_then(|c| c.parse().ok());
    Ok(Frame { corr, resp: read_resp(resp)? })
}

fn shape(what: &str) -> WireError {
    WireError::Shape(what.into())
}

fn id(v: &Value, what: &str) -> Result<i64, WireError> {
    v.as_i64().filter(|n| *n > 0).ok_or_else(|| shape(what))
}

fn read_resp(r: &Value) -> Result<Resp, WireError> {
    let t = r["type"].as_str().ok_or_else(|| shape("a resp with no type"))?;
    Ok(match t {
        "invitation" => {
            let link = r["connLinkInvitation"]["connFullLink"].as_str().ok_or_else(|| shape("an invitation with no link"))?;
            if link.len() > LINK_MAX_BYTES || !link.starts_with("simplex:/invitation#") {
                return Err(shape("an invitation's link of another form"));
            }
            let short = r["connLinkInvitation"]["connShortLink"].as_str().filter(|s| s.len() <= LINK_MAX_BYTES && s.starts_with("https://")).map(str::to_string);
            Resp::Invitation { conn: id(&r["connection"]["pccConnId"], "an invitation with no connection")?, link: link.into(), short }
        }
        "contactConnected" => {
            let c = &r["contact"];
            let name = c["profile"]["displayName"].as_str().unwrap_or("");
            Resp::Connected { contact: id(&c["contactId"], "a contact with no id")?, conn: id(&c["activeConn"]["connId"], "a contact with no connection")?, name: cut(name, NAME_MAX_BYTES).into() }
        }
        "newChatItems" => {
            let items = r["chatItems"].as_array().ok_or_else(|| shape("new items with no list"))?;
            Resp::Items(items.iter().take(ITEMS_MAX).filter_map(|ci| item(&ci["chatInfo"], &ci["chatItem"])).collect())
        }
        "apiChat" => {
            let chat = &r["chat"];
            let items = chat["chatItems"].as_array().ok_or_else(|| shape("a chat with no items"))?;
            Resp::Page(items.iter().take(ITEMS_MAX).filter_map(|ci| item(&chat["chatInfo"], ci)).collect())
        }
        "contactRatchetSync" => {
            let status = match r["ratchetSyncProgress"]["ratchetSyncStatus"].as_str() {
                Some("ok") => SyncStatus::Ok,
                Some("allowed") => SyncStatus::Allowed,
                Some("required") => SyncStatus::Required,
                Some("started") => SyncStatus::Started,
                Some("agreed") => SyncStatus::Agreed,
                _ => return Err(shape("a sync of another status")),
            };
            Resp::Sync { contact: id(&r["contact"]["contactId"], "a sync with no contact")?, status }
        }
        "contactRatchetSyncStarted" => Resp::SyncStarted { contact: id(&r["contact"]["contactId"], "a sync with no contact")? },
        "contactDeleted" | "contactDeletedByContact" => Resp::Deleted { contact: id(&r["contact"]["contactId"], "a delete with no contact")?, by_them: t == "contactDeletedByContact" },
        "contactConnectionDeleted" => Resp::ConnDeleted { conn: id(&r["connection"]["pccConnId"], "a delete with no connection")? },
        "chatCmdError" | "chatError" => Resp::Error { what: error_of(&r["chatError"]) },
        other => Resp::Other(cut(other, 64).into()),
    })
}

/// A direct chat's item, or none (a group's, or one of another shape).
fn item(info: &Value, ci: &Value) -> Option<Item> {
    if info["type"] != "direct" {
        return None;
    }
    let contact = info["contact"]["contactId"].as_i64().filter(|n| *n > 0)?;
    let received = match ci["chatDir"]["type"].as_str()? {
        "directRcv" => true,
        "directSnd" => false,
        _ => return None,
    };
    let c = &ci["content"];
    let content = match c["type"].as_str()? {
        "rcvMsgContent" | "sndMsgContent" => {
            let m = &c["msgContent"];
            Content::Message { kind: cut(m["type"].as_str().unwrap_or("?"), 32).into(), text: cut(m["text"].as_str().unwrap_or(""), TEXT_MAX_BYTES).into() }
        }
        "rcvDecryptionError" => Content::Undecryptable,
        "rcvIntegrityError" => Content::Skipped,
        other => Content::Other(cut(other, 64).into()),
    };
    let meta = &ci["meta"];
    let shared = meta["itemSharedMsgId"].as_str().filter(|s| !s.is_empty() && s.len() <= 64).map(str::to_string);
    let id = meta["itemId"].as_i64().filter(|n| *n > 0).unwrap_or(0);
    Some(Item { id, contact, received, content, shared, at: cut(meta["itemTs"].as_str().unwrap_or(""), 64).into() })
}

/// An error, as short words: its type and what is inside it.
fn error_of(e: &Value) -> String {
    let mut parts = vec![e["type"].as_str().unwrap_or("error").to_string()];
    for inner in [&e["agentError"], &e["storeError"], &e["errorType"]] {
        if let Some(t) = inner["type"].as_str() {
            parts.push(t.to_string());
        }
        for k in ["cmdErr", "smpErr", "agentErr"] {
            if let Some(t) = inner[k]["type"].as_str() {
                parts.push(t.to_string());
            }
        }
        if let Some(c) = inner["errContext"].as_str() {
            parts.push(c.to_string());
        }
    }
    cut(&parts.join(" "), ERROR_MAX_BYTES).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Resp {
        let text = match name {
            "invitation" => include_str!("fixtures/invitation.json"),
            "contactConnected" => include_str!("fixtures/contactConnected.json"),
            "newChatItems" => include_str!("fixtures/newChatItems.json"),
            "sent" => include_str!("fixtures/sent.json"),
            "apiChat" => include_str!("fixtures/apiChat.json"),
            "contactRatchetSync" => include_str!("fixtures/contactRatchetSync.json"),
            "contactRatchetSyncStarted" => include_str!("fixtures/contactRatchetSyncStarted.json"),
            "skipped" => include_str!("fixtures/skipped.json"),
            "prohibited" => include_str!("fixtures/prohibited.json"),
            "largeMsg" => include_str!("fixtures/largeMsg.json"),
            "contactDeleted" => include_str!("fixtures/contactDeleted.json"),
            "contactConnectionDeleted" => include_str!("fixtures/contactConnectionDeleted.json"),
            other => panic!("no fixture {other}"),
        };
        parse(text).unwrap_or_else(|e| panic!("{name}: {e}")).resp
    }

    /// Goal: every frame the lab's core sent is read as what it is (the
    /// fixtures are v7.0.3's own frames).
    #[test]
    fn the_cores_frames_read_as_what_they_are() {
        let Resp::Invitation { conn, link, short } = fixture("invitation") else { panic!("an invitation") };
        assert_eq!(conn, 1);
        assert!(link.starts_with("simplex:/invitation#/?v=2-7&smp="), "{link}");
        assert!(short.is_some_and(|s| s.starts_with("https://smp.lab.test/i#")));
        assert_eq!(fixture("contactConnected"), Resp::Connected { contact: 3, conn: 1, name: "person".into() });
        let Resp::Items(items) = fixture("newChatItems") else { panic!("items") };
        assert_eq!(items, vec![Item { id: items[0].id, contact: 3, received: true, content: Content::Message { kind: "text".into(), text: "hello agent".into() }, shared: Some("RlBFOStpcVFyc1VrNDJyZQ==".into()), at: items[0].at.clone() }]);
        assert!(items[0].id > 0, "its item id");
        assert!(items[0].at.starts_with("2026-10-23T"), "{}", items[0].at);
        let Resp::Items(sent) = fixture("sent") else { panic!("a send's answer") };
        assert!(matches!(&sent[..], [Item { received: false, content: Content::Message { .. }, shared: Some(_), .. }]), "{sent:?}");
        let Resp::Page(chat) = fixture("apiChat") else { panic!("a chat's page") };
        assert_eq!(chat.len(), 8, "every item of the chat, the banner and the features too");
        assert!(chat.windows(2).all(|w| w[0].id < w[1].id), "oldest first, by id");
        assert!(chat.iter().all(|i| i.contact == 3));
        assert!(matches!(chat[0].content, Content::Other(ref t) if t == "chatBanner"));
        assert!(matches!(chat[7], Item { received: false, content: Content::Message { .. }, .. }));
        assert_eq!(fixture("contactRatchetSync"), Resp::Sync { contact: 3, status: SyncStatus::Ok });
        assert_eq!(fixture("contactRatchetSyncStarted"), Resp::SyncStarted { contact: 3 });
        let Resp::Items(skipped) = fixture("skipped") else { panic!("items") };
        assert_eq!(skipped[0].content, Content::Skipped);
        assert_eq!(skipped[0].shared, None, "a skip is no message of its sender's");
        assert_eq!(fixture("prohibited"), Resp::Error { what: "errorAgent CMD PROHIBITED synchronizeRatchet: not allowed".into() });
        assert_eq!(fixture("largeMsg"), Resp::Error { what: "errorStore largeMsg".into() });
        assert_eq!(fixture("contactDeleted"), Resp::Deleted { contact: 4, by_them: false });
        assert_eq!(fixture("contactConnectionDeleted"), Resp::ConnDeleted { conn: 2 });
    }

    /// Goal: a command's line is what the lab's core took.
    #[test]
    fn commands_are_lines_the_core_takes() {
        assert_eq!(Cmd::Invite.line(), "/connect");
        assert_eq!(Cmd::Sync { contact: 3, force: false }.line(), "/_sync @3");
        assert_eq!(Cmd::Sync { contact: 3, force: true }.line(), "/_sync @3 force=on");
        assert_eq!(Cmd::Delete { contact: 4, notify: false }.line(), "/_delete @4 full notify=off");
        assert_eq!(Cmd::DeleteConn { conn: 2 }.line(), "/_delete :2");
        assert_eq!(Cmd::Chat { contact: 3, count: 20, before: None }.line(), "/_get chat @3 count=20");
        assert_eq!(Cmd::Chat { contact: 3, count: 20, before: Some(41) }.line(), "/_get chat @3 before=41 count=20");
        // a text is JSON, so nothing in it reads as more of the command
        let line = Cmd::Send { contact: 3, text: "hi\" json [] /_delete @3\nthere".into() }.line();
        let body = line.strip_prefix("/_send @3 json ").expect("its prefix");
        let v: Value = serde_json::from_str(body).expect("its body is JSON");
        assert_eq!(v[0]["msgContent"]["text"], "hi\" json [] /_delete @3\nthere");
        let f: Value = serde_json::from_str(&Cmd::Invite.frame(7)).expect("a frame is JSON");
        assert_eq!(f, json!({ "corrId": "7", "cmd": "/connect" }));
    }

    /// Goal: a reply of any length goes in parts the core takes, which say
    /// the whole of it in order; a line's end is kept with its part.
    #[test]
    fn a_long_reply_goes_in_parts_the_core_takes() {
        assert!(parts("").is_empty());
        assert_eq!(parts("hi"), vec!["hi"]);
        for text in ["x".repeat(40_000), "é".repeat(20_000), "\"".repeat(9_000), "\u{01}".repeat(5_000), format!("{}\n{}", "a".repeat(11_000), "b".repeat(3_000)), "日本語".repeat(4_000)] {
            let ps = parts(&text);
            assert_eq!(ps.concat(), text);
            for p in &ps {
                assert!(!p.is_empty());
                let json = serde_json::to_string(p).expect("a string").len();
                assert!(json <= SEND_JSON_MAX_BYTES, "a part of {json} JSON bytes");
            }
        }
        let ps = parts(&format!("{}\n{}", "a".repeat(11_000), "b".repeat(3_000)));
        assert!(ps[0].ends_with('\n') && ps[0].len() == 11_001, "cut at the line's end");
    }

    /// Goal: what is not a frame of the core's is refused, saying why, and
    /// a known type missing what the link needs is too.
    #[test]
    fn other_frames_are_refused_or_passed_over() {
        assert!(matches!(parse("nope"), Err(WireError::NotJson(_))));
        assert!(matches!(parse(&"x".repeat(FRAME_MAX_BYTES + 1)), Err(WireError::TooLarge(_))));
        assert!(matches!(parse(r#"{"corrId":"1"}"#), Err(WireError::Shape(_))));
        assert!(matches!(parse(r#"{"resp":{"type":"invitation"}}"#), Err(WireError::Shape(_))));
        assert!(matches!(parse(r#"{"resp":{"type":"invitation","connLinkInvitation":{"connFullLink":"https://evil.example/"},"connection":{"pccConnId":1}}}"#), Err(WireError::Shape(_))), "a link of another form");
        assert!(matches!(parse(r#"{"resp":{"type":"contactConnected","contact":{"contactId":0,"activeConn":{"connId":1}}}}"#), Err(WireError::Shape(_))));
        assert!(matches!(parse(r#"{"resp":{"type":"contactRatchetSync","contact":{"contactId":3},"ratchetSyncProgress":{"ratchetSyncStatus":"weird"}}}"#), Err(WireError::Shape(_))));
        assert_eq!(parse(r#"{"resp":{"type":"subscriptionStatus"}}"#).expect("read").resp, Resp::Other("subscriptionStatus".into()));
        // a group's items are not a contact's
        let group = r#"{"resp":{"type":"newChatItems","chatItems":[{"chatInfo":{"type":"group","groupInfo":{"groupId":1}},"chatItem":{"chatDir":{"type":"groupRcv"},"content":{"type":"rcvMsgContent","msgContent":{"type":"text","text":"hi"}},"meta":{}}}]}}"#;
        assert_eq!(parse(group).expect("read").resp, Resp::Items(vec![]));
        // a corrId that is no number of ours is an event
        assert_eq!(parse(r#"{"corrId":"x","resp":{"type":"chatStopped"}}"#).expect("read").corr, None);
        // a text past the chat's bound is cut, at a char boundary
        let long = json!({ "resp": { "type": "newChatItems", "chatItems": [{ "chatInfo": { "type": "direct", "contact": { "contactId": 3 } }, "chatItem": { "chatDir": { "type": "directRcv" }, "content": { "type": "rcvMsgContent", "msgContent": { "type": "text", "text": "é".repeat(TEXT_MAX_BYTES) } }, "meta": { "itemSharedMsgId": "abc" } } }] } });
        let Resp::Items(items) = parse(&long.to_string()).expect("read").resp else { panic!("items") };
        let Content::Message { text, .. } = &items[0].content else { panic!("a message") };
        assert_eq!(text.len(), TEXT_MAX_BYTES);
    }
}
