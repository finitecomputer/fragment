//! A person's word to the deployment's operators (docs/api.md, Feedback):
//! a problem they report or a message, its text, the shell's page they
//! sent it from, and up to `SCREENSHOTS_MAX` screenshots, sent from the
//! shell's Settings as one `multipart/form-data` body. Pure: what one
//! holds, what is refused and why, and the notice an operator is mailed.

use fragment_proto::feedback::FeedbackKind;

use crate::mail::{self, Mail, PRODUCT};
use crate::multipart::Part;

/// A message's text, at most, in characters.
pub const MESSAGE_MAX_CHARS: usize = 5_000;
/// The shell's path it was sent from, at most.
pub const PAGE_MAX_CHARS: usize = 512;
/// The sender's browser (its `User-Agent`), as kept: the rest is cut.
pub const USER_AGENT_MAX_CHARS: usize = 512;
/// Screenshots one holds, at most.
pub const SCREENSHOTS_MAX: usize = 3;
/// One screenshot's bytes, at most: a full-screen PNG of a large display.
pub const SCREENSHOT_MAX_BYTES: usize = 5 * 1024 * 1024;
/// A form's body, at most: its screenshots, and room for its fields.
pub const BODY_MAX_BYTES: usize = SCREENSHOTS_MAX * SCREENSHOT_MAX_BYTES + 64 * 1024;
/// What one person sends in a day, at most (a rolling 24 hours).
pub const PER_DAY_MAX: u64 = 10;
/// The window `PER_DAY_MAX` counts in.
pub const DAY_MS: i64 = 24 * 3600 * 1000;
/// Feedback the registry keeps, at most: past it a new one is refused
/// until an operator deletes some.
pub const KEPT_MAX: u64 = 10_000;
/// The message's characters a notice's mail quotes; the rest is on the
/// admin's page (a mail's text is at most 16 KiB).
pub const MAILED_MAX_CHARS: usize = 2_000;

/// How long a send may take to put its screenshots and be kept: past
/// it, one still `sending` is the registry alarm's to delete, its bytes
/// first. A send is a Worker's request, seconds; an hour leaves a person
/// who asks again with its id (their dialog's) room to finish it.
pub const SENDING_TTL_MS: i64 = 3600 * 1000;
/// Feedback one run of the registry's alarm deletes, at most (each its
/// screenshots' bytes, then its row); more are the next run's, soon.
pub const SWEEP_BATCH: u32 = 16;

/// A feedback's id: 32 lowercase hex digits, its sender's for one send
/// (the shell's dialog makes one each time it opens), or the router's.
pub fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Where a feedback stands. A row is the record of its screenshots' bytes
/// until they are gone, so none is ever left unrecorded: the registry
/// keeps it `Sending` before a byte is put, `Kept` once all are, and
/// `Deleting` once an operator deletes it, or a send was never kept in
/// `SENDING_TTL_MS`, until its alarm has deleted the bytes, then the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Sending,
    Kept,
    Deleting,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Sending => "sending",
            State::Kept => "kept",
            State::Deleting => "deleting",
        }
    }

    pub fn parse(s: &str) -> Option<State> {
        [State::Sending, State::Kept, State::Deleting].into_iter().find(|k| k.as_str() == s)
    }
}

/// Where a feedback's screenshots are kept in BLOBS: under its person's
/// prefix (a wipe deletes it whole), each by its place.
pub fn screenshot_key(person: &str, id: &str, place: usize) -> String {
    assert!(valid_id(id) && place < SCREENSHOTS_MAX, "a screenshot is one of a feedback's few");
    format!("{}{id}/{place}", person_prefix(person))
}

/// Every screenshot a person sent is under this.
pub fn person_prefix(person: &str) -> String {
    assert!(crate::npub::is_identity(person), "a feedback is a person's, named by their npub");
    format!("feedback/{person}/")
}

/// One feedback's screenshots are under this.
pub fn feedback_prefix(person: &str, id: &str) -> String {
    format!("{}{id}/", person_prefix(person))
}

/// A screenshot, its type read from its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shot<'a> {
    pub mime: &'static str,
    pub bytes: &'a [u8],
}

/// A form read: what a person sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent<'a> {
    /// The send's own id, when its sender named one: asked again with it,
    /// it is the same send.
    pub id: Option<String>,
    pub kind: FeedbackKind,
    pub message: String,
    pub page: Option<String>,
    pub screenshots: Vec<Shot<'a>>,
}

/// Why a form is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// A field it does not take.
    Unknown(String),
    /// A field it takes once, twice.
    Twice(&'static str),
    /// No `kind`, or one that is neither.
    Kind,
    /// An `id` that is not 32 lowercase hex digits.
    Id,
    /// A field's bytes are not UTF-8.
    NotText(&'static str),
    /// No message, or only spaces.
    Empty,
    /// A message past `MESSAGE_MAX_CHARS`.
    TooLong(usize),
    /// A message with a control character (a newline and a tab aside).
    Control,
    /// A page that is not one of the shell's paths.
    Page,
    /// More than `SCREENSHOTS_MAX` screenshots.
    TooMany(usize),
    /// A screenshot (by its place, from 0) past `SCREENSHOT_MAX_BYTES`.
    TooLarge(usize, usize),
    /// A screenshot (by its place) that is no image.
    NotAnImage(usize),
}

impl Refused {
    /// What the sender is told.
    pub fn message(&self) -> String {
        match self {
            Refused::Unknown(f) => format!("feedback takes the fields id, kind, message, page and screenshot, not {:?}", truncated(f)),
            Refused::Twice(f) => format!("feedback takes one {f}"),
            Refused::Kind => "feedback's kind is `problem` or `message`".into(),
            Refused::Id => "feedback's id is 32 lowercase hex digits, one for each send".into(),
            Refused::NotText(f) => format!("feedback's {f} is text (UTF-8)"),
            Refused::Empty => "write a message: say what happened, or what you want us to know".into(),
            Refused::TooLong(n) => format!("the message is {n} characters; at most {MESSAGE_MAX_CHARS}: make it shorter"),
            Refused::Control => "the message holds a control character: only text, newlines and tabs".into(),
            Refused::Page => format!("feedback's page is one of the shell's paths (/…), at most {PAGE_MAX_CHARS} characters"),
            Refused::TooMany(n) => format!("{n} screenshots; at most {SCREENSHOTS_MAX}"),
            Refused::TooLarge(i, n) => format!("screenshot {} is {n} bytes; at most {SCREENSHOT_MAX_BYTES} (5 MiB): crop it or send a smaller one", i + 1),
            Refused::NotAnImage(i) => format!("screenshot {} is not an image: a PNG, JPEG, WebP or GIF", i + 1),
        }
    }
}

fn truncated(s: &str) -> String {
    s.chars().take(40).collect()
}

fn text<'a>(part: &Part<'a>, field: &'static str) -> Result<&'a str, Refused> {
    std::str::from_utf8(part.data).map_err(|_| Refused::NotText(field))
}

/// Whether `page` is one of the shell's own paths, as its sender's
/// `location.pathname` names it: `/`, then no scheme, host or control.
pub fn valid_page(page: &str) -> bool {
    page.starts_with('/') && !page.starts_with("//") && page.chars().count() <= PAGE_MAX_CHARS && !page.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// The message as kept: a browser's CRLFs as newlines, trimmed.
fn message_of(raw: &str) -> Result<String, Refused> {
    let m = raw.replace("\r\n", "\n");
    let m = m.trim();
    if m.is_empty() {
        return Err(Refused::Empty);
    }
    let chars = m.chars().count();
    if chars > MESSAGE_MAX_CHARS {
        return Err(Refused::TooLong(chars));
    }
    if m.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
        return Err(Refused::Control);
    }
    Ok(m.to_string())
}

/// What a form's parts send: `kind` and `message` once, `id` and `page`
/// at most once, and each `screenshot` an image by its bytes (its
/// declared type and file name are not trusted).
pub fn read<'a>(parts: &[Part<'a>]) -> Result<Sent<'a>, Refused> {
    let (mut id, mut kind, mut message, mut page) = (None, None, None, None);
    let mut shots = Vec::new();
    for p in parts {
        match p.name.as_str() {
            "id" => {
                if id.is_some() {
                    return Err(Refused::Twice("id"));
                }
                let v = text(p, "id")?.trim();
                if !valid_id(v) {
                    return Err(Refused::Id);
                }
                id = Some(v.to_string());
            }
            "kind" => {
                if kind.is_some() {
                    return Err(Refused::Twice("kind"));
                }
                kind = Some(FeedbackKind::parse(text(p, "kind")?.trim()).ok_or(Refused::Kind)?);
            }
            "message" => {
                if message.is_some() {
                    return Err(Refused::Twice("message"));
                }
                message = Some(message_of(text(p, "message")?)?);
            }
            "page" => {
                if page.is_some() {
                    return Err(Refused::Twice("page"));
                }
                let v = text(p, "page")?.trim();
                if !v.is_empty() && !valid_page(v) {
                    return Err(Refused::Page);
                }
                page = Some(v.to_string());
            }
            "screenshot" => shots.push(p.data),
            other => return Err(Refused::Unknown(other.to_string())),
        }
    }
    if shots.len() > SCREENSHOTS_MAX {
        return Err(Refused::TooMany(shots.len()));
    }
    let mut screenshots = Vec::with_capacity(shots.len());
    for (i, bytes) in shots.into_iter().enumerate() {
        if bytes.len() > SCREENSHOT_MAX_BYTES {
            return Err(Refused::TooLarge(i, bytes.len()));
        }
        let mime = crate::face::picture_type(bytes).ok_or(Refused::NotAnImage(i))?;
        screenshots.push(Shot { mime, bytes });
    }
    Ok(Sent { id, kind: kind.ok_or(Refused::Kind)?, message: message.ok_or(Refused::Empty)?, page: page.filter(|p| !p.is_empty()), screenshots })
}

/// A browser's `User-Agent`, as kept: its first `USER_AGENT_MAX_CHARS`,
/// control characters dropped.
pub fn user_agent(raw: &str) -> Option<String> {
    let ua: String = raw.chars().filter(|c| !c.is_control()).take(USER_AGENT_MAX_CHARS).collect();
    Some(ua.trim().to_string()).filter(|u| !u.is_empty())
}

/// The operators' notice of a new feedback: who sent it, from which page,
/// its message (its first `MAILED_MAX_CHARS`), and where to read it all.
pub struct Notice<'a> {
    pub to: &'a str,
    pub from: Option<&'a str>,
    pub person: &'a str,
    pub kind: FeedbackKind,
    pub message: &'a str,
    pub page: Option<&'a str>,
    pub screenshots: usize,
    pub admin: &'a str,
}

pub fn notice(n: &Notice<'_>) -> Mail {
    let who = n.from.unwrap_or(n.person);
    let what = match n.kind {
        FeedbackKind::Problem => "reported a problem",
        FeedbackKind::Message => "sent a message",
    };
    let subject: String = format!("{PRODUCT}: {who} {what}").chars().take(mail::SUBJECT_MAX_CHARS).collect();
    let quoted: String = n.message.chars().take(MAILED_MAX_CHARS).collect();
    let cut = if quoted.len() < n.message.len() { "\n\n(cut short: the rest is on the admin's page)" } else { "" };
    let page = n.page.map(|p| format!("Page: {p}\n")).unwrap_or_default();
    let shots = match n.screenshots {
        0 => String::new(),
        1 => "1 screenshot, on the admin's page.\n".into(),
        k => format!("{k} screenshots, on the admin's page.\n"),
    };
    let text = format!("{who} ({}) {what} on {PRODUCT}.\n\n{quoted}{cut}\n\n{page}{shots}Read it, and mark it done: {}\n", n.person, n.admin);
    Mail { to: n.to.to_string(), subject, text }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part<'a>(name: &str, data: &'a [u8]) -> Part<'a> {
        Part { name: name.into(), filename: None, content_type: None, data }
    }

    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0];
    const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0];

    /// Valid: a problem with its message (a browser's CRLFs kept as
    /// newlines, trimmed), its page and two screenshots typed by their
    /// bytes, whatever their parts declared; a message alone, no page.
    #[test]
    fn a_form_reads_as_what_its_person_sent() {
        let mut shot = part("screenshot", PNG);
        shot.content_type = Some("image/jpeg".into());
        shot.filename = Some("x.jpg".into());
        let parts = [part("kind", b"problem"), part("message", b"  It broke.\r\nTwice.\t \r\n"), part("page", b"/settings"), shot, part("screenshot", JPEG)];
        let sent = read(&parts).unwrap();
        assert_eq!((sent.kind, sent.message.as_str(), sent.page.as_deref()), (FeedbackKind::Problem, "It broke.\nTwice.", Some("/settings")));
        assert_eq!(sent.screenshots.iter().map(|s| s.mime).collect::<Vec<_>>(), ["image/png", "image/jpeg"]);
        assert_eq!(sent.screenshots[0].bytes, PNG);
        assert_eq!(sent.id, None, "no id: the router makes one");
        let sent = read(&[part("message", b"Thanks!"), part("kind", b"message"), part("page", b"")]).unwrap();
        assert_eq!((sent.kind, sent.page, sent.screenshots.len()), (FeedbackKind::Message, None, 0));
        let id = "0123456789abcdef0123456789abcdef";
        let padded = format!(" {id}\r\n");
        let sent = read(&[part("id", padded.as_bytes()), part("kind", b"message"), part("message", b"hi")]).unwrap();
        assert_eq!(sent.id.as_deref(), Some(id), "the sender's id, its one send");
        // at the limits themselves
        let long = "é".repeat(MESSAGE_MAX_CHARS);
        let page = format!("/{}", "a".repeat(PAGE_MAX_CHARS - 1));
        let mut big = vec![0u8; SCREENSHOT_MAX_BYTES];
        big[..4].copy_from_slice(&PNG[..4]);
        let three = [part("kind", b"problem"), part("message", long.as_bytes()), part("page", page.as_bytes()), part("screenshot", &big), part("screenshot", PNG), part("screenshot", PNG)];
        assert_eq!(read(&three).map(|s| s.screenshots.len()), Ok(SCREENSHOTS_MAX));
    }

    /// Invalid: each way a form is out of shape, saying so in words its
    /// sender acts on.
    #[test]
    fn a_form_out_of_shape_is_refused() {
        fn ok(extra: Vec<Part<'_>>) -> Vec<Part<'_>> {
            [vec![part("kind", b"problem"), part("message", b"hi")], extra].concat()
        }
        assert_eq!(read(&[part("message", b"hi")]), Err(Refused::Kind));
        assert_eq!(read(&[part("kind", b"bug"), part("message", b"hi")]), Err(Refused::Kind));
        assert_eq!(read(&[part("kind", b"problem")]), Err(Refused::Empty));
        assert_eq!(read(&[part("kind", b"problem"), part("message", b" \r\n\t ")]), Err(Refused::Empty));
        let long = "x".repeat(MESSAGE_MAX_CHARS + 1);
        assert_eq!(read(&[part("kind", b"problem"), part("message", long.as_bytes())]), Err(Refused::TooLong(MESSAGE_MAX_CHARS + 1)));
        assert_eq!(read(&[part("kind", b"problem"), part("message", b"a\x1b[2Jb")]), Err(Refused::Control));
        assert_eq!(read(&[part("kind", b"problem"), part("message", b"\xff\xfe")]), Err(Refused::NotText("message")));
        assert_eq!(read(&ok(vec![part("kind", b"message")])), Err(Refused::Twice("kind")));
        assert_eq!(read(&ok(vec![part("message", b"again")])), Err(Refused::Twice("message")));
        assert_eq!(read(&ok(vec![part("email", b"a@b.c")])), Err(Refused::Unknown("email".into())));
        for id in ["", "0123456789ABCDEF0123456789ABCDEF", "0123456789abcdef", "0123456789abcdef0123456789abcdef0", "../../0123456789abcdef0123456789a"] {
            assert_eq!(read(&ok(vec![part("id", id.as_bytes())])), Err(Refused::Id), "{id:?}");
        }
        let id = b"0123456789abcdef0123456789abcdef";
        assert_eq!(read(&ok(vec![part("id", id), part("id", id)])), Err(Refused::Twice("id")));
        for page in ["settings", "//evil.example/", "https://evil.example/", "/a b", "/a\nb"] {
            assert_eq!(read(&ok(vec![part("page", page.as_bytes())])), Err(Refused::Page), "{page:?}");
        }
        let long_page = format!("/{}", "a".repeat(PAGE_MAX_CHARS));
        assert_eq!(read(&ok(vec![part("page", long_page.as_bytes())])), Err(Refused::Page));
        assert_eq!(read(&ok(vec![part("screenshot", PNG); SCREENSHOTS_MAX + 1])), Err(Refused::TooMany(SCREENSHOTS_MAX + 1)));
        assert_eq!(read(&ok(vec![part("screenshot", PNG), part("screenshot", b"<svg onload=alert(1)>")])), Err(Refused::NotAnImage(1)));
        let mut big = vec![0u8; SCREENSHOT_MAX_BYTES + 1];
        big[..4].copy_from_slice(&PNG[..4]);
        assert_eq!(read(&ok(vec![part("screenshot", &big)])), Err(Refused::TooLarge(0, SCREENSHOT_MAX_BYTES + 1)));
        assert!(Refused::TooLarge(0, 9).message().starts_with("screenshot 1 is 9 bytes"));
        assert!(Refused::Unknown("x".repeat(100)).message().len() < 120, "a field's name is cut short");
    }

    /// A row's state reads back as it was written, and nothing else is one.
    #[test]
    fn a_state_reads_as_written() {
        for s in [State::Sending, State::Kept, State::Deleting] {
            assert_eq!(State::parse(s.as_str()), Some(s));
        }
        assert_eq!(State::parse("Kept"), None);
        assert_eq!(State::parse(""), None);
        const { assert!(SENDING_TTL_MS >= 10 * 60 * 1000 && SWEEP_BATCH >= 1) };
    }

    /// The body's limit holds the most screenshots at their most, with room.
    #[test]
    fn the_body_holds_every_screenshot_at_its_largest() {
        const { assert!(BODY_MAX_BYTES > SCREENSHOTS_MAX * SCREENSHOT_MAX_BYTES + MESSAGE_MAX_CHARS * 4 + PAGE_MAX_CHARS * 4) };
    }

    /// A browser's agent is cut short and loses control characters; none
    /// is none.
    #[test]
    fn a_user_agent_is_kept_short() {
        assert_eq!(user_agent("Mozilla/5.0 (Mac)\r\nX: y"), Some("Mozilla/5.0 (Mac)X: y".into()));
        assert_eq!(user_agent(&"a".repeat(600)).map(|u| u.len()), Some(USER_AGENT_MAX_CHARS));
        assert_eq!(user_agent(" \t"), None);
    }

    /// The notice names who sent what, from where, quotes the message
    /// (a long one cut short, saying so), counts the screenshots, links
    /// the admin's page, and is a mail the platform sends.
    #[test]
    fn the_notice_says_who_sent_what_and_where_to_read_it() {
        let n = Notice { to: "ops@finite.computer", from: Some("ivy@example.com"), person: "npub1ivy", kind: FeedbackKind::Problem, message: "It broke.", page: Some("/settings"), screenshots: 2, admin: "https://finite.chat/admin" };
        let m = notice(&n);
        assert_eq!(m.subject, "Finite.Computer: ivy@example.com reported a problem");
        assert!(m.text.starts_with("ivy@example.com (npub1ivy) reported a problem on Finite.Computer.\n\nIt broke.\n\nPage: /settings\n2 screenshots"));
        assert!(m.text.ends_with("Read it, and mark it done: https://finite.chat/admin\n"));
        assert_eq!(mail::refusal(&m), None);
        let long = "ü".repeat(MESSAGE_MAX_CHARS);
        let m = notice(&Notice { from: None, kind: FeedbackKind::Message, message: &long, page: None, screenshots: 0, ..n });
        assert!(m.subject.contains("npub1ivy sent a message") && m.text.contains("(cut short") && !m.text.contains("Page:"));
        assert_eq!(mail::refusal(&m), None, "the longest message's notice is a mail the platform sends");
    }
}
