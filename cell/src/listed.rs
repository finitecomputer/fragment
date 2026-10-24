//! Listed records (docs/api.md, "Listed records"; `fragment_core::listed`):
//! a channel that declares `listed` puts each member's newest record of the
//! kinds it names on its members' rows in their lists. The fragment keeps
//! each member's newest here, and the index outbox carries them with each
//! row (members.rs `flush_index`).
//!
//! The records are the source of truth and `listed` a copy of those that
//! came while their kind was listed, written in each record's append's
//! turn (`list_record`). What a row carries is decided as it is sent, from
//! what is installed then (`listed_records`): only the channels that list
//! now, the kinds and fields they list now, and only those the row's role
//! may read (`channel_read_role`, the one read check). So no row carries
//! what its member may no longer read, whatever the copy holds. When what
//! the channels list, or who may read them, changes at an install
//! (`listed_fence`), what is listed no more goes from the copy and every row
//! goes again. History is never read back: a kind listed later starts from
//! its next record.
//!
//! A record that shows nothing new moves nothing, and a replayed post
//! appends nothing, so it moves nothing. One that moves something sends the
//! rows of the members who read its channel: at once after a quiet
//! `LISTED_MIN_GAP_MS`, else at that gap's end from the alarm, carrying
//! every move till then (`listed_send`), so a turn's records cost a few
//! fan-outs, not one each. A member who leaves takes theirs with them
//! (`listed_left`).

use fragment_core::listed::{self, Kept, Shown};
use fragment_core::npub;
use fragment_proto::{limits, ListedDecl, ListedRecord, Role};
use serde::Deserialize;
use serde_json::{json, Value};
use worker::*;

use crate::error::{CellError, CellResult};
use crate::fragment::{FragmentCell, MetaKey};
use crate::js;

/// The records `listed` holds at most: each channel's posters, for every
/// channel a fragment may declare.
const KEPT_MAX: usize = limits::CHANNELS_MAX * limits::LISTED_POSTERS_MAX;

/// A `listed` row.
#[derive(Deserialize)]
struct Row {
    channel: String,
    principal: String,
    seq: i64,
    at: i64,
    kind: String,
    fields: String,
}

impl Row {
    fn shown(&self) -> CellResult<Shown> {
        let fields = serde_json::from_str(&self.fields).map_err(|e| CellError::host(format!("listed {}/{}: its stored fields: {e}", self.channel, self.principal)))?;
        Ok(Shown { kind: self.kind.clone(), fields })
    }
}

impl FragmentCell {
    /// A record just appended to `channel`, which lists `decl`'s kinds, in
    /// its append's turn: kept as its poster's newest when it shows
    /// something the lists do not (`fragment_core::listed::moves`), and then
    /// the rows of the members who read the channel go (`listed_send`). One
    /// that shows nothing new is still seen: its poster is not the first to
    /// go from a full channel. Only a member's records count: a list shows
    /// what the people and agents in a fragment are doing.
    pub(crate) fn list_record(&self, channel: &str, decl: &ListedDecl, principal: &str, seq: i64, at: i64, body: &Value) -> CellResult<()> {
        assert!(seq >= 1, "a listed record has its place");
        let Some(shown) = listed::shown(decl, body) else { return Ok(()) };
        if self.member_role(principal)?.is_none() {
            return Ok(());
        }
        let kept = self.listed_kept(channel, principal)?;
        if !listed::moves(kept.as_ref(), &shown, seq, at) {
            self.exec("UPDATE listed SET last = MAX(last, ?) WHERE channel = ? AND principal = ?", vec![SqlStorageValue::Integer(seq), channel.into(), principal.into()])?;
            return Ok(());
        }
        keep(&self.sql(), channel, principal, &shown, seq, at)?;
        let read = self.channel_read_role(channel)?;
        self.listed_send(read)
    }

    /// A member's newest listed record of `channel`, as kept.
    fn listed_kept(&self, channel: &str, principal: &str) -> CellResult<Option<Kept>> {
        let rows: Vec<Row> = self.typed("SELECT channel, principal, seq, at, kind, fields FROM listed WHERE channel = ? AND principal = ?", vec![channel.into(), principal.into()])?;
        let Some(row) = rows.into_iter().next() else { return Ok(None) };
        Ok(Some(Kept { shown: row.shown()?, seq: row.seq, at: row.at }))
    }

    /// The rows of the members whose role is at least `read` go again: at
    /// once when none went in the last `LISTED_MIN_GAP_MS`, else at that
    /// gap's end, from the alarm (`listed_send_due`), with the weakest role
    /// any move till then asked for. Each row is made as it is sent, so the
    /// one that waits carries the newest records.
    fn listed_send(&self, read: Role) -> CellResult<()> {
        let now = js::now_ms();
        let sent = self.meta(MetaKey::ListedSentAt)?.and_then(|s| s.parse::<i64>().ok());
        match (self.listed_due()?, sent) {
            (Some((at, waiting)), _) => self.set_meta(MetaKey::ListedDue, &due(at, waiting.min(read)))?,
            (None, Some(sent)) if now - sent < limits::LISTED_MIN_GAP_MS => self.set_meta(MetaKey::ListedDue, &due(sent + limits::LISTED_MIN_GAP_MS, read))?,
            (None, _) => {
                self.reindex_readers(read)?;
                self.set_meta(MetaKey::ListedSentAt, &now.to_string())?;
            }
        }
        // after the answer: the rows queued now, and the alarm for one that waits
        self.listed_moved.set(true);
        Ok(())
    }

    /// The send waiting for its gap's end, if one waits: when, and the
    /// weakest role whose rows it sends.
    fn listed_due(&self) -> CellResult<Option<(i64, Role)>> {
        let Some(text) = self.meta(MetaKey::ListedDue)? else { return Ok(None) };
        let parsed = text.split_once(' ').and_then(|(at, role)| Some((at.parse::<i64>().ok()?, Role::parse(role)?)));
        parsed.map(Some).ok_or_else(|| CellError::host(format!("the listed send waiting is stored as {text:?}")))
    }

    /// When the send waiting for its gap's end is due (for the alarm).
    pub(crate) fn listed_due_at(&self) -> CellResult<Option<i64>> {
        Ok(self.listed_due()?.map(|(at, _)| at))
    }

    /// From the alarm, before its flush: the send that waited for its gap's
    /// end queues its rows now, which the flush sends.
    pub(crate) fn listed_send_due(&self) -> CellResult<()> {
        let now = js::now_ms();
        let Some((at, read)) = self.listed_due()? else { return Ok(()) };
        if at > now {
            return Ok(());
        }
        self.reindex_readers(read)?;
        self.set_meta(MetaKey::ListedSentAt, &now.to_string())?;
        self.del_meta(MetaKey::ListedDue)?;
        assert!(self.listed_due()?.is_none(), "the send that waited is sent");
        Ok(())
    }

    /// Every listed record a row may carry, the newest first, each with the
    /// role that may read its channel as `channel_read_role` answers it now
    /// (asked once a channel): of the copy, only the channels that list now,
    /// the kinds they list now, and the fields they list now
    /// (`fragment_core::listed::still`). What each row's role takes from it
    /// is `fragment_core::listed::for_role`.
    pub(crate) fn listed_records(&self) -> CellResult<Vec<(Role, ListedRecord)>> {
        let mut installed: Vec<(String, Role, ListedDecl)> = vec![];
        // bounded by the manifest's channels
        for (channel, decl) in self.declared_channels()? {
            if let Some(l) = decl.listed {
                let read = self.channel_read_role(&channel)?;
                installed.push((channel, read, l));
            }
        }
        if installed.is_empty() {
            return Ok(vec![]);
        }
        let rows: Vec<Row> =
            self.typed("SELECT channel, principal, seq, at, kind, fields FROM listed ORDER BY at DESC, seq DESC LIMIT ?", vec![SqlStorageValue::Integer(KEPT_MAX as i64)])?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let Some((_, read, decl)) = installed.iter().find(|(c, _, _)| *c == row.channel) else { continue };
            let Some(shown) = listed::still(decl, row.shown()?) else { continue };
            out.push((*read, ListedRecord { channel: row.channel, principal: npub::display(&row.principal), kind: shown.kind, fields: shown.fields, at: row.at }));
        }
        assert!(out.len() <= KEPT_MAX, "the records kept are bounded");
        Ok(out)
    }

    /// Keeps the lists true to what the installed code lists, and to who may
    /// read it: when the channels that list, their kinds and fields, or
    /// their read roles changed since the rows last went (an install), what
    /// is listed no more goes from the copy (a kind listed again starts from
    /// its next record, never an old one), and every row goes again, made
    /// from what is installed now. Runs after each install and before each
    /// index flush; a fragment from before channels listed records lists
    /// none. Each step is idempotent and the fence is recorded last, so one
    /// cut short is done again by the next flush.
    pub(crate) fn listed_fence(&self) -> CellResult<()> {
        let mut now = vec![];
        let mut channels = vec![];
        // bounded by the manifest's channels
        for (channel, decl) in self.declared_channels()? {
            if let Some(l) = decl.listed {
                let read = self.channel_read_role(&channel)?;
                now.push(json!({ "channel": channel, "read": read, "kinds": l.kinds, "fields": l.fields }));
                channels.push((channel, l.kinds));
            }
        }
        let now = serde_json::to_string(&now).expect("a fence serializes");
        let was = self.meta(MetaKey::Listed)?.unwrap_or_else(|| "[]".to_string());
        if was == now {
            return Ok(());
        }
        let names: Vec<&str> = channels.iter().map(|(c, _)| c.as_str()).collect();
        let names = serde_json::to_string(&names).expect("names serialize");
        self.exec("DELETE FROM listed WHERE channel NOT IN (SELECT value FROM json_each(?))", vec![names.into()])?;
        for (channel, kinds) in &channels {
            let kinds = serde_json::to_string(kinds).expect("kinds serialize");
            self.exec("DELETE FROM listed WHERE channel = ? AND kind NOT IN (SELECT value FROM json_each(?))", vec![channel.as_str().into(), kinds.into()])?;
        }
        self.reindex()?;
        self.set_meta(MetaKey::Listed, &now)
    }

    /// A member who left takes their listed records with them: when they
    /// had any, every row goes again without them.
    pub(crate) fn listed_left(&self, principal: &str) -> CellResult<()> {
        let gone = self.rows("DELETE FROM listed WHERE principal = ? RETURNING channel", vec![principal.into()])?;
        assert!(gone.len() <= limits::CHANNELS_MAX, "a member has a record a listing channel at most");
        if gone.is_empty() {
            return Ok(());
        }
        self.reindex()
    }
}

/// A send waiting for its gap's end, as `MetaKey::ListedDue` holds it.
fn due(at: i64, read: Role) -> String {
    format!("{at} {}", read.as_str())
}

/// Keeps a member's newest listed record of `channel`; the channel keeps
/// its `LISTED_POSTERS_MAX` posters seen latest (a record that moved nothing
/// still counts as seen), the one seen longest ago going first.
fn keep(sql: &SqlStorage, channel: &str, principal: &str, shown: &Shown, seq: i64, at: i64) -> CellResult<()> {
    let fields = serde_json::to_string(&shown.fields).expect("fields serialize");
    sql.exec(
        "INSERT INTO listed (channel, principal, seq, at, kind, fields, last) VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (channel, principal) DO UPDATE SET seq = excluded.seq, at = excluded.at, kind = excluded.kind, fields = excluded.fields,
           last = MAX(listed.last, excluded.last)",
        vec![channel.into(), principal.into(), SqlStorageValue::Integer(seq), SqlStorageValue::Integer(at), shown.kind.as_str().into(), fields.into(), SqlStorageValue::Integer(seq)],
    )?;
    sql.exec(
        "DELETE FROM listed WHERE channel = ? AND principal NOT IN (SELECT principal FROM listed WHERE channel = ? ORDER BY last DESC LIMIT ?)",
        vec![channel.into(), channel.into(), SqlStorageValue::Integer(limits::LISTED_POSTERS_MAX as i64)],
    )?;
    Ok(())
}
