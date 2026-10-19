//! Listed records (docs/api.md, "Listed records"; `fragment_core::listed`):
//! a channel that declares `listed` puts each member's newest record of the
//! kinds it names on its members' rows in their lists. The fragment keeps
//! each member's newest here, as it was last sent, and the index outbox
//! carries them with each row (members.rs `flush_index`), each row only
//! those of the channels its role may read (`channel_read_role`, the one
//! read check).
//!
//! The records are the source of truth and `listed` a copy of them: written
//! in each record's append's turn (`list_record`), and made again from the
//! records when what the channels list, or who may read them, changed at an
//! install (`listed_fence`). A member who leaves takes theirs with them
//! (`listed_left`). Back-to-back records that show nothing new move
//! nothing, and a replayed post appends nothing, so it moves nothing.

use fragment_core::listed::{self, Kept, Shown};
use fragment_core::npub;
use fragment_proto::{limits, ErrorCode, ListedDecl, ListedRecord, Role};
use serde::Deserialize;
use serde_json::{json, Value};
use worker::*;

use crate::error::{CellError, CellResult};
use crate::fragment::{FragmentCell, MetaKey};

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
    /// the rows of the members who read the channel go again, after the
    /// answer (`listed_moved`). Only a member's records count: a list shows
    /// what the people and agents in a fragment are doing, and a member's
    /// leaving takes theirs (`listed_left`).
    pub(crate) fn list_record(&self, channel: &str, decl: &ListedDecl, principal: &str, seq: i64, at: i64, body: &Value) -> CellResult<()> {
        assert!(seq >= 1, "a listed record has its place");
        let Some(shown) = listed::shown(decl, body) else { return Ok(()) };
        if self.member_role(principal)?.is_none() {
            return Ok(());
        }
        let kept = self.listed_kept(channel, principal)?;
        if !listed::moves(kept.as_ref(), &shown, seq, at) {
            return Ok(());
        }
        self.listed_keep(channel, principal, &shown, seq, at)?;
        let read = self.channel_read_role(channel)?;
        self.reindex_readers(read)?;
        self.listed_moved.set(true);
        Ok(())
    }

    /// A member's newest listed record of `channel`, as kept.
    fn listed_kept(&self, channel: &str, principal: &str) -> CellResult<Option<Kept>> {
        let rows: Vec<Row> = self.typed("SELECT channel, principal, seq, at, kind, fields FROM listed WHERE channel = ? AND principal = ?", vec![channel.into(), principal.into()])?;
        let Some(row) = rows.into_iter().next() else { return Ok(None) };
        Ok(Some(Kept { shown: row.shown()?, seq: row.seq, at: row.at }))
    }

    /// Keeps a member's newest listed record of `channel`; the channel keeps
    /// its `LISTED_POSTERS_MAX` latest posters', the one who posted longest
    /// ago going first.
    fn listed_keep(&self, channel: &str, principal: &str, shown: &Shown, seq: i64, at: i64) -> CellResult<()> {
        let fields = serde_json::to_string(&shown.fields).expect("fields serialize");
        self.exec(
            "INSERT INTO listed (channel, principal, seq, at, kind, fields) VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT (channel, principal) DO UPDATE SET seq = excluded.seq, at = excluded.at, kind = excluded.kind, fields = excluded.fields",
            vec![channel.into(), principal.into(), SqlStorageValue::Integer(seq), SqlStorageValue::Integer(at), shown.kind.as_str().into(), fields.into()],
        )?;
        self.exec(
            "DELETE FROM listed WHERE channel = ? AND principal NOT IN (SELECT principal FROM listed WHERE channel = ? ORDER BY seq DESC LIMIT ?)",
            vec![channel.into(), channel.into(), SqlStorageValue::Integer(limits::LISTED_POSTERS_MAX as i64)],
        )?;
        let held = self.count_of("SELECT COUNT(*) AS n FROM listed WHERE channel = ?", vec![channel.into()])?;
        assert!((1..=limits::LISTED_POSTERS_MAX as u64).contains(&held), "a channel keeps its newest posters' records, this one's among them");
        Ok(())
    }

    /// Every listed record kept, the newest first, each with the role that
    /// may read its channel as `channel_read_role` answers it now: what
    /// each row's role takes from (`fragment_core::listed::for_role`). A
    /// record of a channel no longer declared is read by no one (the fence
    /// drops it at the install that undeclared it).
    pub(crate) fn listed_records(&self) -> CellResult<Vec<(Role, ListedRecord)>> {
        let rows: Vec<Row> =
            self.typed("SELECT channel, principal, seq, at, kind, fields FROM listed ORDER BY at DESC, seq DESC LIMIT ?", vec![SqlStorageValue::Integer(KEPT_MAX as i64)])?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let read = match self.channel_read_role(&row.channel) {
                Ok(read) => read,
                Err(e) if e.code == ErrorCode::NotFound => continue,
                Err(e) => return Err(e),
            };
            let shown = row.shown()?;
            out.push((read, ListedRecord { channel: row.channel, principal: npub::display(&row.principal), kind: shown.kind, fields: shown.fields, at: row.at }));
        }
        assert!(out.len() <= KEPT_MAX, "the records kept are bounded");
        Ok(out)
    }

    /// Keeps the listed records true to what the installed code lists, and
    /// to who may read it: when the channels that list, their kinds and
    /// fields, or their read roles changed since the rows last carried them
    /// (an install), the records kept are made again from the channels'
    /// records and every row goes again, so a read role tightened takes
    /// them from the rows that may no longer read them. Runs after each
    /// install and before each index flush; a fragment from before
    /// channels listed records lists none, and moves nothing.
    pub(crate) fn listed_fence(&self) -> CellResult<()> {
        let mut now = vec![];
        // bounded by the manifest's channels
        for (channel, decl) in self.declared_channels()? {
            if let Some(l) = decl.listed {
                let read = self.channel_read_role(&channel)?;
                now.push(json!({ "channel": channel, "read": read, "kinds": l.kinds, "fields": l.fields }));
            }
        }
        let now = serde_json::to_string(&now).expect("a fence serializes");
        let was = self.meta(MetaKey::Listed)?.unwrap_or_else(|| "[]".to_string());
        if was == now {
            return Ok(());
        }
        self.set_meta(MetaKey::Listed, &now)?;
        self.relist()?;
        self.reindex()
    }

    /// The listed records kept, made again from the channels' records: for
    /// each channel that lists, each member's newest record of a kind it
    /// lists, the latest `LISTED_POSTERS_MAX` posters'. Bounded by the
    /// records a channel keeps (`POSTED_KEPT` for one people post to).
    fn relist(&self) -> CellResult<()> {
        self.exec("DELETE FROM listed", vec![])?;
        #[derive(Deserialize)]
        struct Newest {
            principal: String,
            seq: i64,
            at: i64,
            body: String,
        }
        for (channel, decl) in self.declared_channels()? {
            let Some(l) = decl.listed else { continue };
            let kinds = serde_json::to_string(&l.kinds).expect("kinds serialize");
            // SQLite takes a bare column from the row that holds the one
            // MAX(): each member's newest record of a listed kind
            let newest: Vec<Newest> = self.typed(
                "SELECT principal, MAX(seq) AS seq, at, body FROM records
                 WHERE channel = ? AND json_extract(body, '$.kind') IN (SELECT value FROM json_each(?)) AND principal IN (SELECT principal FROM members)
                 GROUP BY principal ORDER BY seq DESC LIMIT ?",
                vec![channel.as_str().into(), kinds.into(), SqlStorageValue::Integer(limits::LISTED_POSTERS_MAX as i64)],
            )?;
            assert!(newest.len() <= limits::LISTED_POSTERS_MAX, "a channel's newest posters are bounded");
            for n in newest {
                let body: Value = serde_json::from_str(&n.body).map_err(|e| CellError::host(format!("record {channel}#{}: its stored body: {e}", n.seq)))?;
                // the SQL chose it by its kind: it shows
                let shown = listed::shown(&l, &body).ok_or_else(|| CellError::host(format!("record {channel}#{} has a listed kind, and shows none", n.seq)))?;
                self.listed_keep(&channel, &n.principal, &shown, n.seq, n.at)?;
            }
        }
        Ok(())
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
