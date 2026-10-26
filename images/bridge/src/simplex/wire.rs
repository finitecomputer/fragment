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

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// A contact's id in the core's own database (`contactId`).
pub type ContactId = i64;
/// A connection's id in the core's own database (`pccConnId`, `connId`).
pub type ConnId = i64;
/// A file's id in the core's own database (`fileId`).
pub type FileId = i64;

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

/// An image's preview, which SimpleX's image content carries and its apps
/// show until the image itself is received: SimpleX's own CLI's (its
/// `/image` sends every image with this one, a 64-pixel PNG), read from
/// v7.0.3 in the lab. A preview of the image itself would need it decoded
/// and scaled here.
pub const IMAGE_PREVIEW: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAYAAACqaXHeAAAAAXNSR0IArs4c6QAAAKVJREFUeF7t1kENACEUQ0FQhnVQ9lfGO+xggITQdvbMzArPey+8fa3tAfwAEdABZQspQStgBssEcgAIkSAJkiAJljtEgiRIgmUCSZAESZAESZAEyx0iQRIkwTKBJEiCv5fgvTd1wDmn7QAP4AeIgA4oW0gJWgEzWCZwbQ7gAA7ggLKFOIADOKBMIAeAEAmSIAmSYLlDJEiCJFgmkARJkARJ8N8S/ADTZUewBvnTOQAAAABJRU5ErkJggg==";

/// What a file the link sends is, as SimpleX's apps show it: a voice
/// message (its length, which they show), an image, or any other file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Kind {
    Voice { secs: u32 },
    Image,
    File,
}

impl Kind {
    /// What it is, in the words a note says.
    pub fn words(self) -> &'static str {
        match self {
            Kind::Voice { .. } => "a voice message",
            Kind::Image => "an image",
            Kind::File => "a file",
        }
    }
}

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
    /// Receive a file a contact sent (`/freceive`): over XFTP, once its
    /// description is ready (`FileReady`); done, `FileDone`.
    Receive { file: FileId },
    /// Give up on receiving it (`/fcancel`): one waited for too long.
    CancelFile { file: FileId },
    /// A file to a contact, from `path` (absolute): its message goes at
    /// once, its file is uploaded over XFTP after (`FileSent`, or
    /// `FileUnsent`). The core sends it as a voice message, an image or a
    /// file (`kind`), with no caption.
    SendFile { contact: ContactId, path: String, kind: Kind },
    /// Give up on sending it (`/fcancel`): one not sent in time. Its answer
    /// settles nothing: the link let go of the file before it asked.
    CancelSend { file: FileId },
    /// The agent's own profile's display name (`/p '<name>'`), the rest of
    /// it (a bot's, its preferences) kept: the core sends it to its
    /// contacts (the owner's app shows it), and answers `userProfileUpdated`,
    /// or `userProfileNoChange` for the name it holds. `name` is one
    /// `name_ok` takes.
    Profile { name: String },
}

/// A display name the core takes as it is: a subset of what v7.0.3's
/// `mkValidName` (src/Simplex/Chat/Library/Commands.hs) leaves unchanged,
/// which it holds a new profile and a rename to (`invalidDisplayName`
/// otherwise, fixtures/invalidDisplayName.json; a first start's profile
/// that fails it is never made). Words of `name_char`s, one space between,
/// a `-` or `_` only just after one of them, at most `NAME_MAX_BYTES`. The
/// core never takes `.,;/\#@'"`~`, and `/p` reads a name with a space only
/// quoted, by `'`.
pub fn name_ok(name: &str) -> bool {
    let word_ok = |w: &str| {
        let mut after_char = false;
        !w.is_empty()
            && w.chars().all(|c| {
                let ok = name_char(c) || (matches!(c, '-' | '_') && after_char);
                after_char = name_char(c);
                ok
            })
    };
    !name.is_empty() && name.len() <= NAME_MAX_BYTES && name.split(' ').all(word_ok)
}

/// A char a display name may hold anywhere: an ASCII letter or digit, or
/// another letter, as Rust tells one without Unicode's categories: its
/// alphabetic chars less its numeric ones (letter numbers, which the core
/// drops). Those are letters and a few symbols, which the core takes
/// anywhere, and some combining signs, which it takes only after a letter,
/// as they come in any word.
pub fn name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || (!c.is_ascii() && c.is_alphabetic() && !c.is_numeric())
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
            Cmd::Receive { file } => {
                assert!(*file > 0, "a receive names a file");
                format!("/freceive {file}")
            }
            Cmd::CancelFile { file } | Cmd::CancelSend { file } => {
                assert!(*file > 0, "a cancel names a file");
                format!("/fcancel {file}")
            }
            Cmd::SendFile { contact, path, kind } => {
                assert!(*contact > 0 && path.starts_with('/'), "a file's send names a contact and a path of its own");
                let content = match kind {
                    Kind::Voice { secs } => {
                        assert!(*secs > 0, "a voice message plays for a while");
                        json!({ "type": "voice", "text": "", "duration": secs })
                    }
                    Kind::Image => json!({ "type": "image", "text": "", "image": IMAGE_PREVIEW }),
                    Kind::File => json!({ "type": "file", "text": "" }),
                };
                let body = json!([{ "filePath": path, "msgContent": content }]);
                format!("/_send @{contact} json {body}")
            }
            Cmd::Profile { name } => {
                assert!(name_ok(name), "a profile's name the core takes as it is");
                format!("/p '{name}'")
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
    /// A file's description is ready: it may be received now
    /// (`rcvFileDescrReady`).
    FileReady { file: FileId },
    /// A file received whole (`rcvFileComplete`), at `path` in the core's
    /// files folder.
    FileDone { file: FileId, path: String },
    /// A file that will not arrive (`rcvFileError`, its sender's cancel).
    FileFailed { file: FileId },
    /// A file the link sent, uploaded whole (`sndFileCompleteXFTP`): its
    /// receiver can fetch it now.
    FileSent { file: FileId },
    /// A file the link sent that will not be (`sndFileError`, or the core's
    /// own cancel, `sndFileCancelledXFTP`).
    FileUnsent { file: FileId },
    /// The agent's profile after a `Profile`: the display name it shows now,
    /// and whether that changed it (`userProfileUpdated`, sent on to its
    /// contacts) or it was the name already (`userProfileNoChange`).
    Profile { name: String, changed: bool },
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
    /// The file it carries (a voice message's, a photo's), when it has one.
    pub file: Option<FileRef>,
}

/// A file an item carries, as the core names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRef {
    pub id: FileId,
    /// Its name as its sender gave it, cut.
    pub name: String,
    pub size: u64,
    /// Received whole already (`rcvComplete`), at `path` in the files folder.
    pub path: Option<String>,
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
        "rcvFileDescrReady" => Resp::FileReady { file: id(&r["chatItem"]["chatItem"]["file"]["fileId"], "a file with no id")? },
        "rcvFileComplete" => {
            let file = &r["chatItem"]["chatItem"]["file"];
            let path = file["fileSource"]["filePath"].as_str().filter(|p| file_path_ok(p)).ok_or_else(|| shape("a file received to no path"))?;
            Resp::FileDone { file: id(&file["fileId"], "a file with no id")?, path: path.into() }
        }
        "rcvFileError" | "rcvFileSndCancelled" | "rcvFileAcceptedSndCancelled" => Resp::FileFailed { file: id(&r["chatItem"]["chatItem"]["file"]["fileId"], "a file with no id")? },
        // a sent file's events all carry its transfer (`chatItem_` may be none)
        "sndFileCompleteXFTP" => Resp::FileSent { file: id(&r["fileTransferMeta"]["fileId"], "a sent file with no id")? },
        "sndFileError" | "sndFileCancelledXFTP" => Resp::FileUnsent { file: id(&r["fileTransferMeta"]["fileId"], "a sent file with no id")? },
        "userProfileUpdated" | "userProfileNoChange" => {
            let changed = t == "userProfileUpdated";
            let p = if changed { &r["toProfile"] } else { &r["user"]["profile"] };
            let name = p["displayName"].as_str().ok_or_else(|| shape("a profile with no name"))?;
            Resp::Profile { name: cut(name, NAME_MAX_BYTES).into(), changed }
        }
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
    let f = &ci["file"];
    let file = f["fileId"].as_i64().filter(|n| *n > 0).map(|id| FileRef {
        id,
        name: cut(f["fileName"].as_str().unwrap_or("file"), 128).into(),
        size: f["fileSize"].as_u64().unwrap_or(0),
        path: if f["fileStatus"]["type"] == "rcvComplete" { f["fileSource"]["filePath"].as_str().filter(|p| file_path_ok(p)).map(str::to_string) } else { None },
    });
    let id = meta["itemId"].as_i64().filter(|n| *n > 0).unwrap_or(0);
    Some(Item { id, contact, received, content, shared, at: cut(meta["itemTs"].as_str().unwrap_or(""), 64).into(), file })
}

/// A path the core names in its files folder: relative, one name or a few,
/// none of them `.` or `..`.
pub fn file_path_ok(p: &str) -> bool {
    !p.is_empty() && p.len() <= 512 && !p.starts_with('/') && p.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}

/// An error, as short words: its type and what is inside it.
fn error_of(e: &Value) -> String {
    let mut parts = vec![e["type"].as_str().unwrap_or("error").to_string()];
    for inner in [&e["agentError"], &e["storeError"], &e["errorType"]] {
        if let Some(t) = inner["type"].as_str() {
            parts.push(t.to_string());
        }
        for k in ["cmdErr", "smpErr", "agentErr", "brokerErr"] {
            if let Some(t) = inner[k]["type"].as_str() {
                parts.push(t.to_string());
            }
        }
        // a server not reached: how (`NETWORK` `NETLSError`, `TIMEOUT`, …)
        if let Some(t) = inner["brokerErr"]["networkError"]["type"].as_str() {
            parts.push(t.to_string());
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
            "voiceItem" => include_str!("fixtures/voiceItem.json"),
            "fileReady" => include_str!("fixtures/fileReady.json"),
            "fileDone" => include_str!("fixtures/fileDone.json"),
            "sentVoice" => include_str!("fixtures/sentVoice.json"),
            "fileSent" => include_str!("fixtures/fileSent.json"),
            "sendCancelled" => include_str!("fixtures/sendCancelled.json"),
            "profileUpdated" => include_str!("fixtures/profileUpdated.json"),
            "profileNoChange" => include_str!("fixtures/profileNoChange.json"),
            "invalidDisplayName" => include_str!("fixtures/invalidDisplayName.json"),
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
        assert_eq!(items, vec![Item { id: items[0].id, contact: 3, received: true, content: Content::Message { kind: "text".into(), text: "hello agent".into() }, shared: Some("RlBFOStpcVFyc1VrNDJyZQ==".into()), at: items[0].at.clone(), file: None }]);
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
        // a server not reached says how (a preview computer's, 2026-10-25)
        let broker = r#"{"corrId":"2","resp":{"type":"chatCmdError","chatError":{"type":"errorAgent","agentError":{"type":"BROKER","brokerAddress":"smp://a@smp5.simplex.im","brokerErr":{"type":"NETWORK","networkError":{"type":"NETLSError","tlsError":"HandshakeFailed"}}}}}}"#;
        assert_eq!(parse(broker).expect("a frame").resp, Resp::Error { what: "errorAgent BROKER NETWORK NETLSError".into() });
        assert_eq!(fixture("contactDeleted"), Resp::Deleted { contact: 4, by_them: false });
        assert_eq!(fixture("contactConnectionDeleted"), Resp::ConnDeleted { conn: 2 });
        // a voice message and its file, received (the lab's `voice_against_the_real_core`)
        let Resp::Items(voice) = fixture("voiceItem") else { panic!("items") };
        assert!(matches!(&voice[0].content, Content::Message { kind, text } if kind == "voice" && text.is_empty()), "{voice:?}");
        assert_eq!(voice[0].file, Some(FileRef { id: 1, name: "short.m4a".into(), size: 82_659, path: None }));
        assert!(voice[0].shared.is_some());
        assert_eq!(fixture("fileReady"), Resp::FileReady { file: 1 });
        assert_eq!(fixture("fileDone"), Resp::FileDone { file: 1, path: "short.m4a".into() });
        // a voice reply sent (the lab's exploration of v7.0.3's sends): its
        // item at once, its file's id, then the upload's end
        let Resp::Items(sent) = fixture("sentVoice") else { panic!("a send's answer") };
        assert!(matches!(&sent[..], [Item { received: false, content: Content::Message { kind, .. }, file: Some(FileRef { id: 1, size: 47_647, path: None, .. }), .. }] if kind == "voice"), "{sent:?}");
        assert_eq!(fixture("fileSent"), Resp::FileSent { file: 1 });
        // the answer to a send's cancel settles nothing of the link's
        assert_eq!(fixture("sendCancelled"), Resp::Other("sndFileCancelled".into()));
        // a sent file that fails, as v7.0.3's source names it (hand-made: the
        // lab's servers never failed one), and one the core cancels itself
        let failed = r#"{"resp":{"type":"sndFileError","user":{},"chatItem_":null,"fileTransferMeta":{"fileId":4,"fileName":"a.mp3","filePath":"/x/a.mp3","fileSize":9,"chunkSize":9,"cancelled":false},"errorMessage":"XFTP error"}}"#;
        assert_eq!(parse(failed).expect("a frame").resp, Resp::FileUnsent { file: 4 });
        let theirs = r#"{"resp":{"type":"sndFileCancelledXFTP","user":{},"fileTransferMeta":{"fileId":5}}}"#;
        assert_eq!(parse(theirs).expect("a frame").resp, Resp::FileUnsent { file: 5 });
        // a warning is the core retrying: nothing ends
        let warned = r#"{"resp":{"type":"sndFileWarning","user":{},"fileTransferMeta":{"fileId":4},"errorMessage":"retrying"}}"#;
        assert_eq!(parse(warned).expect("a frame").resp, Resp::Other("sndFileWarning".into()));
        assert!(matches!(parse(r#"{"resp":{"type":"sndFileCompleteXFTP","fileTransferMeta":{}}}"#), Err(WireError::Shape(_))), "a sent file with no id");
        // the agent's profile renamed (the lab's `a_rename_reaches_the_owners_app`):
        // changed, then asked again for the name it holds
        assert_eq!(fixture("profileUpdated"), Resp::Profile { name: "Waffle Bot".into(), changed: true });
        assert_eq!(fixture("profileNoChange"), Resp::Profile { name: "Waffle Bot".into(), changed: false });
        assert_eq!(fixture("invalidDisplayName"), Resp::Error { what: "error invalidDisplayName".into() });
        assert!(matches!(parse(r#"{"resp":{"type":"userProfileUpdated","toProfile":{}}}"#), Err(WireError::Shape(_))), "a profile with no name");
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
        assert_eq!(Cmd::Receive { file: 7 }.line(), "/freceive 7");
        assert_eq!(Cmd::CancelFile { file: 7 }.line(), "/fcancel 7");
        // its answer, a receiving file cancelled: nothing waits on it (the
        // bridge let go of the file before it asked)
        let cancelled = r#"{"corrId":"9","resp":{"type":"rcvFileCancelled","user":{},"rcvFileTransfer":{"fileId":7}}}"#;
        assert_eq!(parse(cancelled).expect("a frame").resp, Resp::Other("rcvFileCancelled".into()));
        // a text is JSON, so nothing in it reads as more of the command
        let line = Cmd::Send { contact: 3, text: "hi\" json [] /_delete @3\nthere".into() }.line();
        let body = line.strip_prefix("/_send @3 json ").expect("its prefix");
        let v: Value = serde_json::from_str(body).expect("its body is JSON");
        assert_eq!(v[0]["msgContent"]["text"], "hi\" json [] /_delete @3\nthere");
        let f: Value = serde_json::from_str(&Cmd::Invite.frame(7)).expect("a frame is JSON");
        assert_eq!(f, json!({ "corrId": "7", "cmd": "/connect" }));
        // a file: as a voice message with its length, an image with SimpleX
        // CLI's preview, or a file, its path JSON (the lab took each)
        let sent = |kind: Kind| -> Value {
            let line = Cmd::SendFile { contact: 3, path: "/data/simplex/j/out/1-0/a \"b\".mp3".into(), kind }.line();
            serde_json::from_str(line.strip_prefix("/_send @3 json ").expect("its prefix")).expect("its body is JSON")
        };
        assert_eq!(sent(Kind::Voice { secs: 7 }), json!([{ "filePath": "/data/simplex/j/out/1-0/a \"b\".mp3", "msgContent": { "type": "voice", "text": "", "duration": 7 } }]));
        assert_eq!(sent(Kind::Image)[0]["msgContent"], json!({ "type": "image", "text": "", "image": IMAGE_PREVIEW }));
        assert_eq!(sent(Kind::File)[0]["msgContent"], json!({ "type": "file", "text": "" }));
        assert!(IMAGE_PREVIEW.starts_with("data:image/png;base64,") && IMAGE_PREVIEW.len() == 338, "the CLI's own");
        assert_eq!(Cmd::CancelSend { file: 9 }.line(), "/fcancel 9");
        assert_eq!(serde_json::to_value(Kind::Voice { secs: 3 }).expect("a kind"), json!({ "kind": "voice", "secs": 3 }));
        // a profile's name, quoted: one with a space reads whole
        assert_eq!(Cmd::Profile { name: "Waffle Bot".into() }.line(), "/p 'Waffle Bot'");
        assert_eq!(Cmd::Profile { name: "Waffle".into() }.line(), "/p 'Waffle'");
    }

    /// Goal: a profile's name is asked only in a shape the core takes as it
    /// is; anything that would end the quote, or that the core refuses or
    /// changes, never is.
    #[test]
    fn a_profiles_name_is_one_the_core_takes() {
        for ok in ["Waffle", "Waffle Bot", "pulsar-360", "Zoë", "алиса", "日本語 ボット", "a_b-c", "a- b", &"a".repeat(NAME_MAX_BYTES)] {
            assert!(name_ok(ok), "{ok}");
        }
        for bad in ["", "Paul's Bot", "Dr. Bot", "a.b", "'", "a  b", " a", "a ", "-a", "a--b", "a-_b", "a -b", "#a", "@a", "a#b", "a,b", "🧇", "x²", "Ⅻ", "a\tb", &"a".repeat(NAME_MAX_BYTES + 1)] {
            assert!(!name_ok(bad), "{bad:?}");
            assert!(std::panic::catch_unwind(|| Cmd::Profile { name: bad.into() }.line()).is_err(), "never asked: {bad:?}");
        }
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
        // a file the core says it put outside its folder is no file of ours
        for bad in ["/etc/passwd", "../x", "a/../b", "", "a//b"] {
            assert!(!file_path_ok(bad), "{bad}");
            let done = json!({ "resp": { "type": "rcvFileComplete", "chatItem": { "chatItem": { "file": { "fileId": 1, "fileSource": { "filePath": bad } } } } } });
            assert!(matches!(parse(&done.to_string()), Err(WireError::Shape(_))), "{bad}");
        }
        assert!(file_path_ok("voice_1.m4a") && file_path_ok("sub/voice.m4a"));
        // a corrId that is no number of ours is an event
        assert_eq!(parse(r#"{"corrId":"x","resp":{"type":"chatStopped"}}"#).expect("read").corr, None);
        // a text past the chat's bound is cut, at a char boundary
        let long = json!({ "resp": { "type": "newChatItems", "chatItems": [{ "chatInfo": { "type": "direct", "contact": { "contactId": 3 } }, "chatItem": { "chatDir": { "type": "directRcv" }, "content": { "type": "rcvMsgContent", "msgContent": { "type": "text", "text": "é".repeat(TEXT_MAX_BYTES) } }, "meta": { "itemSharedMsgId": "abc" } } }] } });
        let Resp::Items(items) = parse(&long.to_string()).expect("read").resp else { panic!("items") };
        let Content::Message { text, .. } = &items[0].content else { panic!("a message") };
        assert_eq!(text.len(), TEXT_MAX_BYTES);
    }
}
