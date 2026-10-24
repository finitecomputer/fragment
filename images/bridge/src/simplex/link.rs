//! One agent's link to its owner's SimpleX app, as pure rules: what the
//! core says (`wire::Resp`) and what the owner and the agent ask, in;
//! commands to the core, messages for the agent's chat and notices for its
//! owner, out. Nothing here does I/O, so every rule is tested on its own.
//!
//! - **Linking.** The owner asks (`Want`); the core makes a one-time
//!   invitation, which the owner's page shows as a QR code. The contact
//!   that connects over that invitation's connection is the owner's: the
//!   link is bound to it. Only one can (the SMP server refuses the spent
//!   link, `AUTH`), and any other contact the core ever reports, or any
//!   message from one, is deleted, unanswered: the agent talks on SimpleX
//!   to its owner alone.
//! - **Messages.** The owner's messages are forwarded to the agent's chat,
//!   each by the id its sender's app gave it (`itemSharedMsgId`), so a
//!   message read twice (a catch-up, a restart) is one record. The core's
//!   own item ids are never used: a rollback of its database reuses them.
//!   The agent's replies are sent in parts the core takes.
//! - **What a rollback does, and the cure** (measured in the lab on v7.0.3,
//!   tests/simplex.rs). A core whose database went back in time may not know
//!   it: across received messages alone it reads the next one fine (a
//!   skipped-id item), and its own next sends reach the owner's app as
//!   duplicates, dropped there (`A_DUPLICATE`) with nothing said on this
//!   side; across sends too, its next message may still decrypt, and an
//!   unforced sync is refused (`PROHIBITED`: it sees nothing wrong). A forced
//!   sync, asked as it starts, healed both directions in about 3 s every
//!   time. So every start that went back in time (the platform says so:
//!   `restored.rollback` in the guest's view) forces one before anything is
//!   sent, and replies wait for it (`outbox`); its owner is told a message
//!   may have been lost. A start that did not go back in time (a sleep that
//!   saved, a restart) needs none: the hold stops the core before the save,
//!   so the save is exact. A ratchet the core itself finds out of step
//!   (`required`, `allowed`, an undecryptable message) is synchronized the
//!   same way, unforced.
//! - **A start** reads back, newest page first (`CATCHUP_ITEMS` a page,
//!   `SCAN_MAX` at most), until it reaches what its chat confirmed posted
//!   (`delivered`, which the bridge acknowledges message by message) or the
//!   chat's start, and forwards the owner's messages after it, oldest first:
//!   a core killed mid-burst had committed messages whose events died with
//!   it (the spike's `crash` runs), and a message read but never posted (a
//!   crash, a refusal) is read again. Forwarding is idempotent, so this
//!   costs reads. An anchor the core no longer has ends the scan with what
//!   it read.
//! - **A reply the core will not send now** (`PROHIBITED` while its ratchet
//!   synchronizes, which an unforced sync after a restart cannot tell
//!   apart from one in step) waits in the outbox until the ratchet is `ok`
//!   or the next start, never dropped. A forced sync the core refuses keeps
//!   replies waiting too, and is asked again (`Resync`) until it is `ok`.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use super::wire::{self, Cmd, ConnId, ContactId, Content, FileRef, Resp, SyncStatus};

/// The newest items a start reads back, to forward what a killed core
/// committed but never announced.
pub const CATCHUP_ITEMS: u16 = 32;
/// Items one start's catch-up reads back, at most.
pub const SCAN_MAX: usize = 8 * CATCHUP_ITEMS as usize;
/// Parts of replies held while the ratchet synchronizes, at most; past it
/// the oldest go (and are counted).
pub const OUTBOX_MAX: usize = 32;

/// Where the link is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Phase {
    /// Not linked, and not asked to be.
    #[default]
    Off,
    /// An invitation asked of the core, its answer not yet read.
    Asking,
    /// A one-time invitation made, shown to the owner until it is used.
    Inviting { conn: ConnId, link: String, short: Option<String> },
    /// Bound to the owner's contact.
    Linked { contact: ContactId, name: String },
}

/// One agent's link: written whole beside the core's database, in the same
/// save (the hold stops the core and copies both), so a rollback takes
/// both back together.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    /// Flat in its JSON: `{"phase": "linked", "contact": 3, "name": …}`.
    #[serde(flatten)]
    pub phase: Phase,
    /// A sync asked and not yet `ok`: replies wait in the outbox.
    #[serde(default)]
    pub syncing: bool,
    /// That sync was forced (a start that went back in time): a restart
    /// before it is `ok` forces it again, since an unforced one is refused
    /// by a core that sees nothing wrong.
    #[serde(default)]
    pub forced: bool,
    #[serde(default)]
    pub outbox: VecDeque<String>,
    /// Parts the outbox dropped, full, since the link was made.
    #[serde(default)]
    pub dropped: u64,
    /// The newest message of the bound contact its chat confirmed posted
    /// (the core's item id; `Input::Delivered`): a start's catch-up reads
    /// back to it. A rollback takes it back with the database.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub delivered: i64,
    /// A start's catch-up, under way: in no file, and in no comparison.
    #[serde(skip)]
    pub scan: Scan,
    /// What an unlink still has the core let go (the contact, told, or an
    /// unused invitation): asked again at each start until it is done, so
    /// an unlink while the core cannot run (under a hold, not always on)
    /// is finished when it next does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<Cleanup>,
}

/// What an unlink has the core let go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cleanup {
    Contact(ContactId),
    Conn(ConnId),
}

impl Cleanup {
    fn cmd(self) -> Cmd {
        match self {
            Cleanup::Contact(contact) => Cmd::Delete { contact, notify: true },
            Cleanup::Conn(conn) => Cmd::DeleteConn { conn },
        }
    }
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

/// A start's catch-up, under way: what it read so far, oldest first
/// (forwarded when it ends), and whether it runs (what was delivered stays
/// put until it ends: a live message's post must not move it past older
/// ones the catch-up has not forwarded yet). Transient: equal to any other,
/// so a link's state compares by what its file holds.
#[derive(Debug, Clone, Default)]
pub struct Scan {
    pub items: Vec<wire::Item>,
    pub running: bool,
}

impl PartialEq for Scan {
    fn eq(&self, _: &Scan) -> bool {
        true
    }
}

impl Eq for Scan {}

/// What reaches the link.
#[derive(Debug, Clone, Copy)]
pub enum Input<'a> {
    /// Its owner asked to link SimpleX (the page's "Link").
    Want,
    /// Its owner asked to unlink.
    Unlink,
    /// The core came up; `rollback`: this start went back in time (the
    /// guest's view, `restored.rollback`).
    Started { rollback: bool },
    /// What the core said: an event, or an answer that was no error.
    Said(&'a Resp),
    /// The core refused a command the link asked for, saying `what`.
    Failed { cmd: &'a Cmd, what: &'a str },
    /// The agent's reply to its owner, to send.
    Reply(&'a str),
    /// The owner's message of this item is on their chat with the agent.
    Delivered(i64),
    /// A forced sync still waits for `ok`: ask it again.
    Resync,
}

/// What the link asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// A command to the core.
    Ask(Cmd),
    /// The owner's message, for the agent's chat.
    Forward(Inbound),
    /// Something to tell the owner, in their chat with the agent.
    Tell(Notice),
    /// The link changed what a lost life would lose (a contact bound or
    /// let go, an invitation made): a save is wanted soon.
    Save,
}

/// The owner's message, as the agent's chat is to carry it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inbound {
    /// Its sender's id for it (`itemSharedMsgId`), the same however often
    /// it is read.
    pub id: String,
    /// Its kind (`text`, `image`, `voice`, `file`, `link`, …): only text is
    /// carried; any other's text is its caption.
    pub kind: String,
    pub text: String,
    /// When its sender sent it, as the core wrote it.
    pub at: String,
    /// The core's id for its item: acknowledged once its chat has it
    /// (`Input::Delivered`).
    pub item: i64,
    /// The file it carries (a voice message's, a photo's): the bridge
    /// receives it, then carries it as the message's attachment.
    pub file: Option<FileRef>,
}

/// What the owner is told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// The link is bound to their SimpleX contact, by its name.
    Linked { name: String },
    /// Another contact connected or wrote, and was deleted.
    Refused,
    /// The link went back in time, or found its encryption out of step: a
    /// message sent about now may not have arrived, either way.
    MaybeLost,
    /// The encryption is in step again, and held replies are sent.
    Resynced,
    /// The link is let go: by them (they deleted the agent in their app), or
    /// by the owner here.
    Unlinked { by_them: bool },
    /// The core refused something the link asked, saying `what`.
    Failed { what: String },
}

impl Link {
    /// The link's rules: the state moves, and what to do, in order.
    pub fn apply(&mut self, input: Input<'_>) -> Vec<Action> {
        let mut out = vec![];
        match input {
            Input::Want => {
                if self.phase == Phase::Off {
                    self.phase = Phase::Asking;
                    out.push(Action::Ask(Cmd::Invite));
                }
            }
            Input::Unlink => match std::mem::take(&mut self.phase) {
                Phase::Linked { contact, .. } => {
                    self.let_go();
                    self.cleanup = Some(Cleanup::Contact(contact));
                    out.extend([Action::Ask(Cmd::Delete { contact, notify: true }), Action::Tell(Notice::Unlinked { by_them: false }), Action::Save]);
                }
                Phase::Inviting { conn, .. } => {
                    self.cleanup = Some(Cleanup::Conn(conn));
                    out.extend([Action::Ask(Cmd::DeleteConn { conn }), Action::Save]);
                }
                // its answer, when it comes, is an invitation no one wants
                Phase::Asking | Phase::Off => {}
            },
            Input::Started { rollback } => {
                // an unlink the last life could not finish, first
                out.extend(self.cleanup.map(|c| Action::Ask(c.cmd())));
                match &self.phase {
                    Phase::Off => {}
                    // the answer died with the last life
                    Phase::Asking => out.push(Action::Ask(Cmd::Invite)),
                    // the lost life may have spent it: a new one to show
                    Phase::Inviting { conn, .. } if rollback => {
                        out.extend([Action::Ask(Cmd::DeleteConn { conn: *conn }), Action::Ask(Cmd::Invite)]);
                        self.phase = Phase::Asking;
                    }
                    Phase::Inviting { .. } => {}
                    Phase::Linked { contact, .. } => {
                        let contact = *contact;
                        if rollback {
                            self.syncing = true;
                            self.forced = true;
                            out.extend([Action::Ask(Cmd::Sync { contact, force: true }), Action::Tell(Notice::MaybeLost)]);
                        } else if self.syncing {
                            // a sync the last life asked: asked again (forced again
                            // if it was), refused if done
                            out.push(Action::Ask(Cmd::Sync { contact, force: self.forced }));
                        }
                        self.scan.items.clear();
                        self.scan.running = true;
                        out.push(Action::Ask(Cmd::Chat { contact, count: CATCHUP_ITEMS, before: None }));
                    }
                }
            }
            Input::Said(resp) => self.said(resp, &mut out),
            Input::Failed { cmd, what } => {
                let what = wire::cut(what, 300).to_string();
                match cmd {
                    Cmd::Invite if self.phase == Phase::Asking => {
                        self.phase = Phase::Off;
                        out.push(Action::Tell(Notice::Failed { what }));
                    }
                    // a forced sync refused heals nothing: replies wait on,
                    // and it is asked again (`Resync`)
                    Cmd::Sync { contact, force: true } if self.linked_to(*contact) && self.syncing => out.push(Action::Tell(Notice::Failed { what })),
                    // the catch-up's next page, gone (its anchor deleted): what it read goes
                    Cmd::Chat { contact, .. } if self.linked_to(*contact) => self.finish_scan(false, &mut out),
                    // gone already, or not the core's to let go: not asked again
                    Cmd::Delete { contact, .. } if self.cleanup == Some(Cleanup::Contact(*contact)) => self.cleanup = None,
                    Cmd::DeleteConn { conn } if self.cleanup == Some(Cleanup::Conn(*conn)) => self.cleanup = None,
                    // not needed (in step), or not possible: what waits goes
                    Cmd::Sync { contact, .. } if self.linked_to(*contact) && self.syncing => {
                        self.syncing = false;
                        self.forced = false;
                        self.flush(&mut out);
                    }
                    // not sent now (a ratchet mid-sync refuses sends): it
                    // waits, and what comes after it, until the ratchet is
                    // ok or the next start
                    Cmd::Send { contact, text } if self.linked_to(*contact) => {
                        self.syncing = true;
                        self.hold(text.clone());
                        out.push(Action::Tell(Notice::Failed { what }));
                    }
                    _ => {}
                }
            }
            Input::Delivered(item) => {
                if self.contact().is_some() && !self.scan.running {
                    self.delivered = self.delivered.max(item);
                }
            }
            Input::Resync => {
                if let (Some(contact), true) = (self.contact(), self.syncing && self.forced) {
                    out.push(Action::Ask(Cmd::Sync { contact, force: true }));
                }
            }
            Input::Reply(text) => {
                if let Phase::Linked { contact, .. } = self.phase {
                    for part in wire::parts(text) {
                        if self.syncing {
                            self.hold(part);
                        } else {
                            out.push(Action::Ask(Cmd::Send { contact, text: part }));
                        }
                    }
                }
            }
        }
        self.check();
        out
    }

    fn said(&mut self, resp: &Resp, out: &mut Vec<Action>) {
        match resp {
            Resp::Invitation { conn, link, short } => {
                if self.phase == Phase::Asking {
                    self.phase = Phase::Inviting { conn: *conn, link: link.clone(), short: short.clone() };
                    out.push(Action::Save);
                } else {
                    out.push(Action::Ask(Cmd::DeleteConn { conn: *conn }));
                }
            }
            Resp::Connected { contact, conn, name } => match &self.phase {
                Phase::Inviting { conn: invited, .. } if invited == conn => {
                    self.phase = Phase::Linked { contact: *contact, name: name.clone() };
                    self.let_go();
                    out.extend([Action::Tell(Notice::Linked { name: name.clone() }), Action::Save]);
                }
                Phase::Linked { contact: linked, .. } if linked == contact => {}
                _ => out.extend([Action::Ask(Cmd::Delete { contact: *contact, notify: false }), Action::Tell(Notice::Refused)]),
            },
            Resp::Items(items) => self.items(items, out),
            Resp::Page(items) => self.page(items, out),
            Resp::Sync { contact, status } if self.linked_to(*contact) => match status {
                SyncStatus::Required | SyncStatus::Allowed => self.sync(*contact, out),
                SyncStatus::Ok if self.syncing => {
                    self.syncing = false;
                    self.forced = false;
                    out.push(Action::Tell(Notice::Resynced));
                    self.flush(out);
                }
                SyncStatus::Ok | SyncStatus::Started | SyncStatus::Agreed => {}
            },
            Resp::Deleted { contact, by_them } if self.linked_to(*contact) => {
                self.phase = Phase::Off;
                self.let_go();
                out.extend([Action::Tell(Notice::Unlinked { by_them: *by_them }), Action::Save]);
            }
            // an unlink's let-go, done
            Resp::Deleted { contact, .. } if self.cleanup == Some(Cleanup::Contact(*contact)) => {
                self.cleanup = None;
                out.push(Action::Save);
            }
            Resp::ConnDeleted { conn } if self.cleanup == Some(Cleanup::Conn(*conn)) => {
                self.cleanup = None;
                out.push(Action::Save);
            }
            // a file's own events are the bridge's to act on (it receives the
            // file a forwarded message carries), never the link's
            Resp::Sync { .. } | Resp::SyncStarted { .. } | Resp::Deleted { .. } | Resp::ConnDeleted { .. } | Resp::FileReady { .. } | Resp::FileDone { .. } | Resp::FileFailed { .. } | Resp::Error { .. } | Resp::Other(_) => {}
        }
    }

    /// A catch-up's page: anyone else's items let go at once; the owner's
    /// kept, and the next older page asked while this one is full, newer
    /// than what was delivered, and within `SCAN_MAX`; else it ends.
    fn page(&mut self, items: &[wire::Item], out: &mut Vec<Action>) {
        let Some(contact) = self.contact() else { return };
        let (mine, others): (Vec<wire::Item>, Vec<wire::Item>) = items.iter().cloned().partition(|i| i.contact == contact);
        self.items(&others, out);
        let oldest = mine.iter().map(|i| i.id).filter(|id| *id > 0).min();
        let mut scan = mine;
        scan.append(&mut self.scan.items);
        self.scan.items = scan;
        let more = items.len() >= usize::from(CATCHUP_ITEMS) && oldest.is_some_and(|o| o > self.delivered);
        match oldest {
            Some(oldest) if more && self.scan.items.len() < SCAN_MAX => {
                out.push(Action::Ask(Cmd::Chat { contact, count: CATCHUP_ITEMS, before: Some(oldest) }));
            }
            // older ones remain past the bound: it ends short of what was delivered
            _ => self.finish_scan(!more, out),
        }
    }

    /// The catch-up's end: what it read after what was delivered,
    /// forwarded oldest first. One that ended short of it (past `SCAN_MAX`,
    /// or its next page's anchor gone) tells the owner a message may not
    /// have arrived: what came before it in that gap is not carried.
    fn finish_scan(&mut self, reached: bool, out: &mut Vec<Action>) {
        let mut scan = std::mem::take(&mut self.scan.items);
        self.scan.running = false;
        scan.retain(|i| i.id > self.delivered);
        scan.sort_by_key(|i| i.id);
        self.items(&scan, out);
        if !reached {
            out.push(Action::Tell(Notice::MaybeLost));
        }
    }

    /// Chat items read (new ones, or a catch-up's): the owner's messages
    /// forwarded, anyone else let go.
    fn items(&mut self, items: &[wire::Item], out: &mut Vec<Action>) {
        {
                let mut refused: Vec<ContactId> = vec![];
                for item in items.iter().filter(|i| i.received) {
                    if !self.linked_to(item.contact) {
                        // anyone else who writes is let go, once a frame
                        if matches!(item.content, Content::Message { .. }) && !refused.contains(&item.contact) {
                            refused.push(item.contact);
                            out.extend([Action::Ask(Cmd::Delete { contact: item.contact, notify: false }), Action::Tell(Notice::Refused)]);
                        }
                        continue;
                    }
                    match &item.content {
                        Content::Message { kind, text } => {
                            // a message has its sender's id; one without is no message to carry
                            if let Some(id) = &item.shared {
                                out.push(Action::Forward(Inbound { id: id.clone(), kind: kind.clone(), text: text.clone(), at: item.at.clone(), item: item.id, file: item.file.clone() }));
                            }
                        }
                        Content::Undecryptable => self.sync(item.contact, out),
                        Content::Skipped | Content::Other(_) => {}
                    }
                }
        }
    }

    /// A part held until the ratchet is in step; past `OUTBOX_MAX` the
    /// oldest goes, counted.
    fn hold(&mut self, part: String) {
        if self.outbox.len() == OUTBOX_MAX {
            self.outbox.pop_front();
            self.dropped += 1;
        }
        self.outbox.push_back(part);
    }

    /// An unforced sync, once until it is answered.
    fn sync(&mut self, contact: ContactId, out: &mut Vec<Action>) {
        if !self.syncing {
            self.syncing = true;
            out.extend([Action::Ask(Cmd::Sync { contact, force: false }), Action::Tell(Notice::MaybeLost)]);
        }
    }

    /// What waited, sent.
    fn flush(&mut self, out: &mut Vec<Action>) {
        if let Phase::Linked { contact, .. } = self.phase {
            out.extend(self.outbox.drain(..).map(|text| Action::Ask(Cmd::Send { contact, text })));
        }
    }

    /// A contact bound or let go: nothing of the last one's waits.
    fn let_go(&mut self) {
        self.syncing = false;
        self.forced = false;
        self.outbox.clear();
        self.delivered = 0;
        self.scan.items.clear();
        self.scan.running = false;
    }

    fn linked_to(&self, contact: ContactId) -> bool {
        matches!(self.phase, Phase::Linked { contact: c, .. } if c == contact)
    }

    /// Whether the link is bound, and to whom.
    pub fn contact(&self) -> Option<ContactId> {
        match self.phase {
            Phase::Linked { contact, .. } => Some(contact),
            _ => None,
        }
    }

    /// A link that contradicts itself is a bug: refused loudly, never kept.
    fn check(&self) {
        let linked = matches!(self.phase, Phase::Linked { .. });
        assert!(linked || (!self.syncing && self.outbox.is_empty()), "only a bound link syncs or holds replies");
        assert!(self.syncing || !self.forced, "a forced sync is one under way");
        assert!(self.outbox.len() <= OUTBOX_MAX, "the outbox is bounded");
    }

    /// The link as written beside the core's database.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("a link serializes")
    }

    /// A link read back; one that does not read, or contradicts itself, is
    /// refused (a fresh one would forget the owner's contact and let it go
    /// unanswered, so the caller says so rather than guess).
    pub fn from_json(text: &str) -> Result<Link, String> {
        let link: Link = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let linked = matches!(link.phase, Phase::Linked { .. });
        if !(linked || (!link.syncing && link.outbox.is_empty())) || link.outbox.len() > OUTBOX_MAX || (link.forced && !link.syncing) {
            return Err("a link that contradicts itself".into());
        }
        Ok(link)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simplex::wire::Item;

    fn invited(link: &mut Link) {
        assert_eq!(link.apply(Input::Want), vec![Action::Ask(Cmd::Invite)]);
        let inv = Resp::Invitation { conn: 1, link: "simplex:/invitation#/?v=2-7".into(), short: None };
        assert_eq!(link.apply(Input::Said(&inv)), vec![Action::Save]);
    }

    fn linked() -> Link {
        let mut link = Link::default();
        invited(&mut link);
        let c = Resp::Connected { contact: 3, conn: 1, name: "paul".into() };
        assert_eq!(link.apply(Input::Said(&c)), vec![Action::Tell(Notice::Linked { name: "paul".into() }), Action::Save]);
        link
    }

    fn msg(contact: ContactId, id: Option<&str>, text: &str) -> Item {
        Item { id: 1, contact, received: true, content: Content::Message { kind: "text".into(), text: text.into() }, shared: id.map(str::to_string), at: "2026-10-22T10:00:00Z".into(), file: None }
    }

    fn forwards(actions: &[Action]) -> Vec<(String, String)> {
        actions.iter().filter_map(|a| if let Action::Forward(i) = a { Some((i.id.clone(), i.text.clone())) } else { None }).collect()
    }

    fn sends(actions: &[Action]) -> Vec<String> {
        actions.iter().filter_map(|a| if let Action::Ask(Cmd::Send { text, .. }) = a { Some(text.clone()) } else { None }).collect()
    }

    /// Goal (valid): the owner links, writes and is answered; each message
    /// is forwarded by its sender's id, and a long reply goes in parts.
    #[test]
    fn a_link_is_made_bound_and_carries_both_ways() {
        let mut link = linked();
        assert_eq!(link.contact(), Some(3));
        let items = Resp::Items(vec![msg(3, Some("AAA="), "hi agent"), Item { received: false, ..msg(3, Some("BBB="), "an echo of ours") }]);
        assert_eq!(forwards(&link.apply(Input::Said(&items))), vec![("AAA=".into(), "hi agent".into())], "the owner's message alone");
        assert_eq!(link.apply(Input::Reply("hello")), vec![Action::Ask(Cmd::Send { contact: 3, text: "hello".into() })]);
        let long = "x".repeat(30_000);
        let parts = sends(&link.apply(Input::Reply(&long)));
        assert_eq!(parts.len(), 3);
        assert_eq!(parts.concat(), long);
        // a file's caption is carried, by its kind, with the file
        let cat = FileRef { id: 5, name: "cat.jpg".into(), size: 1_000, path: None };
        let photo = Resp::Items(vec![Item { content: Content::Message { kind: "image".into(), text: "my cat".into() }, file: Some(cat.clone()), ..msg(3, Some("CCC="), "") }]);
        assert!(matches!(&link.apply(Input::Said(&photo))[..], [Action::Forward(Inbound { kind, text, file: Some(f), .. })] if kind == "image" && text == "my cat" && *f == cat));
        // a file's own events move nothing of the link's
        for r in [Resp::FileReady { file: 5 }, Resp::FileDone { file: 5, path: "cat.jpg".into() }, Resp::FileFailed { file: 5 }] {
            assert!(link.apply(Input::Said(&r)).is_empty());
        }
    }

    /// Goal (invalid): no one but the owner talks to the agent here: a
    /// contact not made by the link's invitation, or a message from one, is
    /// deleted; nothing of theirs is forwarded; a reply with no link goes
    /// nowhere; an invitation no one asked for is deleted.
    #[test]
    fn anyone_but_the_owner_is_let_go() {
        let mut link = Link::default();
        let stranger = Resp::Connected { contact: 9, conn: 7, name: "eve".into() };
        assert_eq!(link.apply(Input::Said(&stranger)), vec![Action::Ask(Cmd::Delete { contact: 9, notify: false }), Action::Tell(Notice::Refused)]);
        invited(&mut link);
        assert_eq!(link.apply(Input::Said(&stranger)), vec![Action::Ask(Cmd::Delete { contact: 9, notify: false }), Action::Tell(Notice::Refused)], "another connection than the invitation's");
        assert!(matches!(link.phase, Phase::Inviting { .. }), "still waiting for the owner");
        let mut link = linked();
        assert_eq!(link.apply(Input::Said(&stranger)), vec![Action::Ask(Cmd::Delete { contact: 9, notify: false }), Action::Tell(Notice::Refused)]);
        let theirs = Resp::Items(vec![msg(9, Some("EVE="), "hi"), msg(9, Some("EVE2="), "hi again"), msg(3, Some("OWN="), "mine")]);
        let a = link.apply(Input::Said(&theirs));
        assert_eq!(forwards(&a), vec![("OWN=".into(), "mine".into())]);
        assert_eq!(a.iter().filter(|x| matches!(x, Action::Ask(Cmd::Delete { contact: 9, .. }))).count(), 1, "let go once a frame");
        // a message with no sender's id is not carried
        assert!(forwards(&link.apply(Input::Said(&Resp::Items(vec![msg(3, None, "?")])))).is_empty());
        let mut off = Link::default();
        assert!(off.apply(Input::Reply("hello")).is_empty());
        let inv = Resp::Invitation { conn: 4, link: "simplex:/invitation#/x".into(), short: None };
        assert_eq!(off.apply(Input::Said(&inv)), vec![Action::Ask(Cmd::DeleteConn { conn: 4 })]);
        assert_eq!(link.apply(Input::Said(&inv)), vec![Action::Ask(Cmd::DeleteConn { conn: 4 })]);
        // a sync of another contact moves nothing
        assert!(link.apply(Input::Said(&Resp::Sync { contact: 9, status: SyncStatus::Required })).is_empty());
        assert!(!link.syncing);
    }

    /// Goal (replay): what is said twice does once: a second Want asks no
    /// second invitation, the same contact connecting again binds nothing
    /// new, and the same message read again is the same forward.
    #[test]
    fn what_is_said_twice_does_once() {
        let mut link = Link::default();
        assert_eq!(link.apply(Input::Want), vec![Action::Ask(Cmd::Invite)]);
        assert!(link.apply(Input::Want).is_empty(), "one invitation asked at a time");
        let inv = Resp::Invitation { conn: 1, link: "simplex:/invitation#/a".into(), short: None };
        link.apply(Input::Said(&inv));
        assert!(link.apply(Input::Want).is_empty(), "the same invitation shown");
        let c = Resp::Connected { contact: 3, conn: 1, name: "paul".into() };
        link.apply(Input::Said(&c));
        assert!(link.apply(Input::Said(&c)).is_empty());
        assert!(link.apply(Input::Want).is_empty(), "linked already");
        let items = Resp::Items(vec![msg(3, Some("AAA="), "hi")]);
        assert_eq!(forwards(&link.apply(Input::Said(&items))), forwards(&link.apply(Input::Said(&items))));
        let ok = Resp::Sync { contact: 3, status: SyncStatus::Ok };
        assert!(link.apply(Input::Said(&ok)).is_empty(), "in step already: nothing to tell");
    }

    /// Goal (Alex's review of #523): a start reads back, page by page, to
    /// what its chat confirmed posted, and forwards what is after it oldest
    /// first: a first catch-up reads to the chat's start; a message read
    /// but never confirmed (a crash before its post, a refusal) is read and
    /// forwarded again; an anchor the core no longer has ends the scan with
    /// what it read; the scan is bounded.
    #[test]
    fn a_catch_up_reads_back_to_what_was_delivered() {
        let item = |id: i64, received: bool| Item { id, received, shared: Some(format!("m{id}")), ..msg(3, None, &format!("text {id}")) };
        let page = |ids: std::ops::RangeInclusive<i64>| Resp::Page(ids.map(|id| item(id, id % 3 != 0)).collect());
        let forwarded = |actions: &[Action]| actions.iter().filter_map(|a| if let Action::Forward(i) = a { Some(i.item) } else { None }).collect::<Vec<_>>();
        // the first: nothing delivered yet, read back to the chat's start
        let mut link = linked();
        assert_eq!(link.apply(Input::Started { rollback: false }), vec![Action::Ask(Cmd::Chat { contact: 3, count: CATCHUP_ITEMS, before: None })]);
        assert_eq!(link.apply(Input::Said(&page(12..=43))), vec![Action::Ask(Cmd::Chat { contact: 3, count: CATCHUP_ITEMS, before: Some(12) })]);
        let got = link.apply(Input::Said(&page(1..=11)));
        assert_eq!(forwarded(&got), (1..=43).filter(|id| id % 3 != 0).collect::<Vec<_>>(), "every message of the chat, oldest first");
        // delivered to 43: the next start reads back to it, and forwards what is newer
        link.apply(Input::Delivered(43));
        let mut link = Link::from_json(&link.to_json()).expect("it reads back");
        assert_eq!(link.delivered, 43);
        link.apply(Input::Started { rollback: false });
        let got = link.apply(Input::Said(&page(20..=51)));
        assert_eq!(forwarded(&got), (44..=51).filter(|id| id % 3 != 0).collect::<Vec<_>>(), "no older page: it reached what was delivered");
        // a live message's post while a catch-up runs moves nothing (older ones are not forwarded yet)
        link.apply(Input::Started { rollback: false });
        link.apply(Input::Delivered(60));
        assert_eq!(link.delivered, 43, "held while the catch-up runs");
        link.apply(Input::Said(&page(20..=51)));
        // 52 read live, never confirmed (a crash before its post, or refused): read again
        let live = link.apply(Input::Said(&Resp::Items(vec![item(52, true)])));
        assert_eq!(forwarded(&live), vec![52]);
        link.apply(Input::Started { rollback: false });
        assert_eq!(forwarded(&link.apply(Input::Said(&page(21..=52)))), vec![44, 46, 47, 49, 50, 52], "what was never confirmed, again");
        // its anchor gone (a disappearing message): the scan ends with what it read
        link.apply(Input::Started { rollback: false });
        link.apply(Input::Said(&page(100..=131)));
        let gone = link.apply(Input::Failed { cmd: &Cmd::Chat { contact: 3, count: CATCHUP_ITEMS, before: Some(100) }, what: "errorStore chatItemNotFound" });
        assert_eq!(forwarded(&gone).first(), Some(&100));
        assert_eq!(gone.last(), Some(&Action::Tell(Notice::MaybeLost)), "short of what was delivered: the owner is told");
        assert!(link.scan.items.is_empty());
        // bounded: past SCAN_MAX it asks no older page
        link.apply(Input::Started { rollback: false });
        let mut asked = 0;
        let mut top = 10_000;
        loop {
            let got = link.apply(Input::Said(&page(top - 31..=top)));
            if !got.iter().any(|a| matches!(a, Action::Ask(Cmd::Chat { .. }))) {
                assert_eq!(got.last(), Some(&Action::Tell(Notice::MaybeLost)), "past the bound: the owner is told");
                break;
            }
            asked += 1;
            top -= 32;
        }
        assert_eq!(asked + 1, SCAN_MAX / usize::from(CATCHUP_ITEMS));
        // a new contact starts from nothing delivered
        link.apply(Input::Unlink);
        assert_eq!(link.delivered, 0);
    }
    /// Goal (Alex's review of #523): a reply the core will not send now (a
    /// restart mid-sync, the peer away: the unforced sync is refused as it
    /// is when in step, and the send then too) waits, and is sent once the
    /// ratchet is ok, never dropped; a sync refused when in step sends what
    /// waited at once.
    #[test]
    fn a_reply_the_core_refuses_waits_for_the_ratchet() {
        let mut link = linked();
        link.apply(Input::Said(&Resp::Sync { contact: 3, status: SyncStatus::Required }));
        assert!(link.apply(Input::Reply("must survive restart")).is_empty());
        let mut link = Link::from_json(&link.to_json()).expect("it reads back");
        link.apply(Input::Started { rollback: false });
        let prohibited = "errorAgent CMD PROHIBITED synchronizeRatchet: not allowed";
        let flushed = link.apply(Input::Failed { cmd: &Cmd::Sync { contact: 3, force: false }, what: prohibited });
        assert_eq!(sends(&flushed), vec!["must survive restart"]);
        // the core is mid-sync after all: the send is refused, and the reply waits
        let send = Cmd::Send { contact: 3, text: "must survive restart".into() };
        link.apply(Input::Failed { cmd: &send, what: "errorAgent CMD PROHIBITED" });
        assert_eq!((link.syncing, link.outbox.iter().cloned().collect::<Vec<_>>()), (true, vec!["must survive restart".to_string()]));
        assert!(link.apply(Input::Reply("and this")).is_empty(), "what comes after it waits behind it");
        let ok = link.apply(Input::Said(&Resp::Sync { contact: 3, status: SyncStatus::Ok }));
        assert_eq!(sends(&ok), vec!["must survive restart", "and this"]);
        assert!(!link.syncing && link.outbox.is_empty());
        // a forced sync that fails (a rollback's) heals nothing: the core may
        // take a send its peer drops, so nothing is sent; it is asked again
        // until the ratchet is ok
        let mut link = linked();
        link.apply(Input::Started { rollback: true });
        link.apply(Input::Reply("held again"));
        let failed = link.apply(Input::Failed { cmd: &Cmd::Sync { contact: 3, force: true }, what: "errorStore SEInternalError" });
        assert!(sends(&failed).is_empty(), "nothing sent: {failed:?}");
        assert!(link.syncing && link.outbox.len() == 1);
        assert_eq!(link.apply(Input::Resync), vec![Action::Ask(Cmd::Sync { contact: 3, force: true })]);
        let ok = link.apply(Input::Said(&Resp::Sync { contact: 3, status: SyncStatus::Ok }));
        assert_eq!(sends(&ok), vec!["held again"]);
        assert!(link.apply(Input::Resync).is_empty(), "in step: nothing to ask");
    }

    /// Goal (restart): every start reads back the newest items; a start
    /// that went back in time forces a sync before anything is sent, tells
    /// the owner, and holds replies until it is in step; a pending
    /// invitation it may have spent is made anew.
    #[test]
    fn a_start_catches_up_and_a_rollback_forces_a_sync() {
        let mut link = linked();
        assert_eq!(link.apply(Input::Started { rollback: false }), vec![Action::Ask(Cmd::Chat { contact: 3, count: CATCHUP_ITEMS, before: None })]);
        assert_eq!(
            link.apply(Input::Started { rollback: true }),
            vec![Action::Ask(Cmd::Sync { contact: 3, force: true }), Action::Tell(Notice::MaybeLost), Action::Ask(Cmd::Chat { contact: 3, count: CATCHUP_ITEMS, before: None })]
        );
        assert!(link.syncing);
        assert!(link.apply(Input::Reply("held")).is_empty(), "nothing is sent out of step");
        assert!(link.apply(Input::Said(&Resp::Sync { contact: 3, status: SyncStatus::Agreed })).is_empty());
        let ok = link.apply(Input::Said(&Resp::Sync { contact: 3, status: SyncStatus::Ok }));
        assert_eq!(ok, vec![Action::Tell(Notice::Resynced), Action::Ask(Cmd::Send { contact: 3, text: "held".into() })]);
        // a restart while a forced sync was under way forces it again
        let mut cut = linked();
        cut.apply(Input::Started { rollback: true });
        let mut cut = Link::from_json(&cut.to_json()).expect("it reads back");
        assert!(cut.forced);
        assert_eq!(cut.apply(Input::Started { rollback: false }), vec![Action::Ask(Cmd::Sync { contact: 3, force: true }), Action::Ask(Cmd::Chat { contact: 3, count: CATCHUP_ITEMS, before: None })]);
        // a restart while a sync was under way asks it again, unforced
        link.apply(Input::Said(&Resp::Sync { contact: 3, status: SyncStatus::Required }));
        let back = Link::from_json(&link.to_json()).expect("it reads back");
        assert_eq!(back, link);
        let mut back = back;
        assert_eq!(back.apply(Input::Started { rollback: false }), vec![Action::Ask(Cmd::Sync { contact: 3, force: false }), Action::Ask(Cmd::Chat { contact: 3, count: CATCHUP_ITEMS, before: None })]);
        // refused as needless (in step after all): what waited goes
        back.apply(Input::Reply("after"));
        let sent = back.apply(Input::Failed { cmd: &Cmd::Sync { contact: 3, force: false }, what: "errorAgent CMD PROHIBITED synchronizeRatchet: not allowed" });
        assert_eq!(sends(&sent), vec!["after"]);
        assert!(!back.syncing);
        // an invitation: kept across a plain restart, made anew after a rollback
        let mut inviting = Link::default();
        invited(&mut inviting);
        assert!(inviting.apply(Input::Started { rollback: false }).is_empty());
        assert_eq!(inviting.apply(Input::Started { rollback: true }), vec![Action::Ask(Cmd::DeleteConn { conn: 1 }), Action::Ask(Cmd::Invite)]);
        assert_eq!(inviting.phase, Phase::Asking);
        // an invitation asked and never answered is asked again
        assert_eq!(inviting.apply(Input::Started { rollback: false }), vec![Action::Ask(Cmd::Invite)]);
        assert!(Link::default().apply(Input::Started { rollback: true }).is_empty(), "nothing linked, nothing to do");
    }

    /// Goal: a ratchet the core finds out of step is synchronized once,
    /// unforced, and the owner told once.
    #[test]
    fn a_ratchet_out_of_step_is_synchronized_once() {
        let mut link = linked();
        let bad = Resp::Items(vec![Item { content: Content::Undecryptable, shared: None, ..msg(3, None, "") }]);
        assert_eq!(link.apply(Input::Said(&bad)), vec![Action::Ask(Cmd::Sync { contact: 3, force: false }), Action::Tell(Notice::MaybeLost)]);
        assert!(link.apply(Input::Said(&Resp::Sync { contact: 3, status: SyncStatus::Required })).is_empty(), "asked already");
        assert!(link.apply(Input::Said(&bad)).is_empty());
        // a skipped id is no reason to sync (a rollback across received messages leaves one)
        let mut quiet = linked();
        assert!(quiet.apply(Input::Said(&Resp::Items(vec![Item { content: Content::Skipped, shared: None, ..msg(3, None, "") }]))).is_empty());
        assert_eq!(quiet.apply(Input::Said(&Resp::Sync { contact: 3, status: SyncStatus::Allowed })), vec![Action::Ask(Cmd::Sync { contact: 3, force: false }), Action::Tell(Notice::MaybeLost)]);
    }

    /// Goal: the outbox is bounded: past it the oldest parts go, counted.
    #[test]
    fn the_outbox_is_bounded() {
        let mut link = linked();
        link.apply(Input::Started { rollback: true });
        for i in 0..OUTBOX_MAX + 5 {
            link.apply(Input::Reply(&format!("part {i}")));
        }
        assert_eq!(link.outbox.len(), OUTBOX_MAX);
        assert_eq!(link.dropped, 5);
        assert_eq!(link.outbox.front().map(String::as_str), Some("part 5"));
    }

    /// Goal: unlinking lets the contact go (told, in their app too); their
    /// own delete does the same here; an invitation is withdrawn.
    #[test]
    fn unlinking_lets_go_from_either_side() {
        let mut link = linked();
        assert_eq!(link.apply(Input::Unlink), vec![Action::Ask(Cmd::Delete { contact: 3, notify: true }), Action::Tell(Notice::Unlinked { by_them: false }), Action::Save]);
        assert_eq!(link.phase, Phase::Off);
        assert_eq!(link.cleanup, Some(Cleanup::Contact(3)), "its let-go waits for the core's answer");
        assert_eq!(link.apply(Input::Said(&Resp::Deleted { contact: 3, by_them: false })), vec![Action::Save], "our own delete's answer: done");
        assert_eq!(link.cleanup, None);
        let mut link = linked();
        link.apply(Input::Started { rollback: true });
        link.apply(Input::Reply("held"));
        assert_eq!(link.apply(Input::Said(&Resp::Deleted { contact: 3, by_them: true })), vec![Action::Tell(Notice::Unlinked { by_them: true }), Action::Save]);
        assert!(link.outbox.is_empty() && !link.syncing);
        let mut inviting = Link::default();
        invited(&mut inviting);
        assert_eq!(inviting.apply(Input::Unlink), vec![Action::Ask(Cmd::DeleteConn { conn: 1 }), Action::Save]);
        assert_eq!(inviting.apply(Input::Said(&Resp::ConnDeleted { conn: 1 })), vec![Action::Save]);
        assert_eq!(inviting.cleanup, None);
        let mut asking = Link::default();
        asking.apply(Input::Want);
        assert!(asking.apply(Input::Unlink).is_empty());
        let late = Resp::Invitation { conn: 2, link: "simplex:/invitation#/late".into(), short: None };
        assert_eq!(asking.apply(Input::Said(&late)), vec![Action::Ask(Cmd::DeleteConn { conn: 2 })], "its answer, withdrawn");
        assert!(Link::default().apply(Input::Unlink).is_empty());
    }

    /// Goal (Alex's review of #546): an unlink while the core cannot run
    /// (under a hold, not always on), whose delete was never sent, is
    /// finished at the core's next start, from its file; a relink meanwhile
    /// does not lose it; a delete the core refuses (gone already) is not
    /// asked again.
    #[test]
    fn an_unlink_is_finished_when_the_core_next_runs() {
        let mut link = linked();
        link.apply(Input::Unlink);
        let mut back = Link::from_json(&link.to_json()).expect("it reads back");
        assert_eq!(back.apply(Input::Started { rollback: false }), vec![Action::Ask(Cmd::Delete { contact: 3, notify: true })]);
        back.apply(Input::Want);
        assert_eq!(back.apply(Input::Started { rollback: false }), vec![Action::Ask(Cmd::Delete { contact: 3, notify: true }), Action::Ask(Cmd::Invite)], "asked again beside a new link");
        back.apply(Input::Failed { cmd: &Cmd::Delete { contact: 3, notify: true }, what: "errorStore contactNotFound" });
        assert_eq!(back.cleanup, None);
        assert_eq!(back.apply(Input::Started { rollback: false }), vec![Action::Ask(Cmd::Invite)]);
    }

    /// Goal: a failed invitation goes back to off and says why; a failed
    /// send says so; a link that contradicts itself is refused as it is read.
    #[test]
    fn failures_are_told_and_bad_state_refused() {
        let mut link = Link::default();
        link.apply(Input::Want);
        assert_eq!(link.apply(Input::Failed { cmd: &Cmd::Invite, what: "errorAgent BROKER" }), vec![Action::Tell(Notice::Failed { what: "errorAgent BROKER".into() })]);
        assert_eq!(link.phase, Phase::Off);
        let mut link = linked();
        let send = Cmd::Send { contact: 3, text: "x".into() };
        assert_eq!(link.apply(Input::Failed { cmd: &send, what: "errorStore largeMsg" }), vec![Action::Tell(Notice::Failed { what: "errorStore largeMsg".into() })]);
        assert!(Link::from_json(r#"{"phase":"off","syncing":true}"#).is_err());
        assert!(Link::from_json(r#"{"phase":"off","outbox":["x"]}"#).is_err());
        assert!(Link::from_json("nope").is_err());
        assert!(Link::from_json(r#"{"phase":"linked"}"#).is_err(), "a bound link names its contact");
        assert_eq!(Link::from_json(r#"{"phase":"off"}"#), Ok(Link::default()));
        assert_eq!(linked().to_json(), r#"{"phase":"linked","contact":3,"name":"paul","syncing":false,"forced":false,"outbox":[],"dropped":0}"#);
        assert!(Link::from_json(r#"{"phase":"linked","contact":3,"name":"p","forced":true}"#).is_err(), "forced, but no sync under way");
    }

    /// Goal: whatever arrives in whatever order, the link never contradicts
    /// itself and never forwards anyone's message but its contact's
    /// (deterministic, seeded).
    #[test]
    fn any_order_keeps_the_rules() {
        let mut seed = 0x5eed_u64;
        let mut rng = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..200 {
            let mut link = Link::default();
            for _ in 0..60 {
                let contact = (rng() % 3 + 3) as i64;
                let resp = match rng() % 9 {
                    0 => Resp::Invitation { conn: (rng() % 3 + 1) as i64, link: "simplex:/invitation#/r".into(), short: None },
                    1 => Resp::Connected { contact, conn: (rng() % 3 + 1) as i64, name: "n".into() },
                    2 => Resp::Items(vec![msg(contact, Some("ID="), "t")]),
                    3 => Resp::Items(vec![Item { content: Content::Undecryptable, ..msg(contact, None, "") }]),
                    4 => Resp::Sync { contact, status: [SyncStatus::Ok, SyncStatus::Required, SyncStatus::Agreed][(rng() % 3) as usize] },
                    5 => Resp::Deleted { contact, by_them: rng() % 2 == 0 },
                    _ => Resp::Other("x".into()),
                };
                let before = link.contact();
                let actions = match rng() % 7 {
                    0 => link.apply(Input::Want),
                    1 => link.apply(Input::Unlink),
                    2 => link.apply(Input::Started { rollback: rng() % 2 == 0 }),
                    3 => link.apply(Input::Reply("r")),
                    4 => link.apply(Input::Failed { cmd: &Cmd::Sync { contact, force: false }, what: "x" }),
                    _ => link.apply(Input::Said(&resp)),
                };
                if let Resp::Items(items) = &resp {
                    if actions.iter().any(|a| matches!(a, Action::Forward(_))) {
                        assert_eq!(before, Some(items[0].contact), "only the bound contact's messages are forwarded");
                    }
                }
                for a in &actions {
                    if let Action::Ask(Cmd::Send { contact, .. }) = a {
                        assert_eq!(link.contact(), Some(*contact), "only the bound contact is written to");
                    }
                }
                assert_eq!(Link::from_json(&link.to_json()).as_ref(), Ok(&link));
            }
        }
    }
}
