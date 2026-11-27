//! Channel subscriptions (phase 7): a member asks the fragment to deliver
//! each new record of a channel it may read to a URL, or to wake the
//! computer it runs on (runs_on.rs). Records go out through the delivery
//! outbox and queue (deliveries.rs) as `{type: "record", fragment,
//! channel, record}` (`fragment_proto::Delivery`), unsigned: the URL is
//! the subscriber's capability; a computer's wake goes from the outbox to
//! the computer itself. A member's subscriptions end with its membership.
//! Whether a subscriber may read its channel is asked when it subscribes
//! and again for each delivery, when the outbox is drained and when the
//! queue's consumer is about to send it (`Deliverable`): one lowered
//! since, held since, or on a channel a deploy raised or dropped since,
//! is sent nothing, its subscription kept for when it may read again
//! (`fragment_core::subscription`).

use fragment_core::permit::Standing;
use fragment_core::{egress, npub, subscription};
use fragment_proto::{valid_channel_name, ChannelRecord, DeliveryType, ErrorCode, IdentityKind, Role};
use serde_json::{json, Value};
use worker::*;

use crate::deliveries::{Delivery, DeliveryKind};
use crate::error::{CellError, CellResult};
use crate::fragment::calls::Deliverable;
use crate::fragment::{json_response, Caller, FragmentCell, MetaKey};

/// Where a subscriber's hold comes from: its signed request's, when it
/// subscribes itself; else the registry's, asked only of an agent beside
/// its person (the one standing a hold caps: `permit` rule 4).
pub(crate) enum Hold {
    Signed(Option<Role>),
    Registry,
}

/// The subscriptions one fragment holds, and a URL's length.
const SUBS_MAX: u64 = 32;
const SUB_URL_MAX_BYTES: usize = 1024;

impl FragmentCell {
    /// The subscriber: a member who may read `channel` (`may_read`, with
    /// the hold its signed request carries).
    async fn subscriber(&self, caller: &Caller, channel: &str) -> CellResult<String> {
        let signed = caller.signed.as_ref().ok_or_else(|| CellError::new(ErrorCode::Unauthenticated, "subscribing needs a signed request"))?;
        let principal = signed.id.clone();
        if self.member_role(&principal)?.is_none() {
            return Err(CellError::new(ErrorCode::Forbidden, "only a member subscribes to a channel"));
        }
        if !valid_channel_name(channel) {
            return Err(CellError::invalid("a channel name must match ^[a-z][a-z0-9_-]{0,63}$"));
        }
        self.channel_read_role(channel)?;
        if !self.may_read(&principal, channel, Hold::Signed(signed.held)).await? {
            return Err(CellError::new(ErrorCode::Forbidden, format!("your role may not read {channel}")));
        }
        Ok(principal)
    }

    /// `POST /api/subscriptions` `{channel, url}` → `{id, channel, url}`.
    pub(crate) async fn subscribe(&self, caller: &Caller, body: &Value) -> CellResult<Response> {
        let channel = body["channel"].as_str().unwrap_or("");
        let principal = self.subscriber(caller, channel).await?;
        let url = body["url"].as_str().unwrap_or("");
        if url.len() > SUB_URL_MAX_BYTES {
            return Err(CellError::invalid(format!("a subscription URL is at most {SUB_URL_MAX_BYTES} bytes")));
        }
        egress::check(url, self.cfg.egress_local, &self.cfg.egress_allow).map_err(|e| CellError::invalid(format!("url: {e}")))?;
        let id = self.add_subscription(&principal, channel, url)?;
        json_response(&json!({ "id": id, "channel": channel, "url": url }))
    }

    /// `principal`'s subscription to `channel` at `url` (a receiver's, or a
    /// computer's wake: runs_on.rs), made once: the same three again answer
    /// the same id.
    pub(crate) fn add_subscription(&self, principal: &str, channel: &str, url: &str) -> CellResult<i64> {
        let existing = self.rows("SELECT id FROM subs WHERE principal = ? AND channel = ? AND url = ?", vec![principal.into(), channel.into(), url.into()])?;
        if let Some(id) = existing.first().and_then(|r| r["id"].as_i64()) {
            return Ok(id);
        }
        if self.count("SELECT COUNT(*) AS n FROM subs")? >= SUBS_MAX {
            return Err(CellError::new(ErrorCode::RateLimited, format!("a fragment holds at most {SUBS_MAX} subscriptions")));
        }
        let rows = self.rows(
            "INSERT INTO subs (principal, channel, url, created_at) VALUES (?, ?, ?, ?) RETURNING id",
            vec![principal.into(), channel.into(), url.into(), SqlStorageValue::Integer(crate::js::now_ms())],
        )?;
        let id = rows.first().and_then(|r| r["id"].as_i64()).ok_or_else(|| CellError::host("a subscription insert returned no id"))?;
        self.event("subscription.added", &format!("{} to {channel}", npub::display(principal)), json!({ "id": id, "channel": channel }));
        Ok(id)
    }

    /// `GET /api/subscriptions`: the caller's (the owner sees every one).
    pub(crate) fn subscriptions(&self, caller: &Caller) -> CellResult<Response> {
        let principal = caller.principal().map(str::to_string).ok_or_else(|| CellError::new(ErrorCode::Unauthenticated, "listing subscriptions needs a signed request"))?;
        let owner = self.must(MetaKey::Owner)? == principal;
        let rows = self.rows(
            "SELECT id, principal, channel, url, created_at FROM subs WHERE principal = ? OR ? ORDER BY id",
            vec![principal.as_str().into(), SqlStorageValue::Integer(owner as i64)],
        )?;
        let subs: Vec<Value> = rows
            .iter()
            .map(|r| {
                let url = r["url"].as_str().unwrap_or("");
                // a computer's wake subscription has no URL anyone fetches (runs_on.rs)
                let wake = url.starts_with("computer:");
                json!({ "id": r["id"], "principal": npub::display(r["principal"].as_str().unwrap_or("")), "channel": r["channel"], "url": (!wake).then_some(url), "wake": wake, "createdAt": r["created_at"] })
            })
            .collect();
        json_response(&json!({ "subscriptions": subs }))
    }

    /// `DELETE /api/subscriptions/{id}`: its subscriber or the owner.
    pub(crate) fn unsubscribe(&self, caller: &Caller, id: &str) -> CellResult<Response> {
        let principal = caller.principal().map(str::to_string).ok_or_else(|| CellError::new(ErrorCode::Unauthenticated, "unsubscribing needs a signed request"))?;
        let id: i64 = id.parse().map_err(|_| CellError::invalid("a subscription id is a number"))?;
        let owner = self.must(MetaKey::Owner)? == principal;
        let gone = self.rows("DELETE FROM subs WHERE id = ? AND (principal = ? OR ?) RETURNING id", vec![SqlStorageValue::Integer(id), principal.as_str().into(), SqlStorageValue::Integer(owner as i64)])?;
        json_response(&json!({ "ok": true, "removed": gone.len() }))
    }

    /// Whether `principal`, as itself, may read `channel` now: a member
    /// whose standing reads the channel as it is declared now
    /// (`subscription::delivers`), with its hold where one applies. Asked
    /// when it subscribes and before each delivery.
    pub(crate) async fn may_read(&self, principal: &str, channel: &str, hold: Hold) -> CellResult<bool> {
        #[derive(serde::Deserialize)]
        struct Row {
            role: Option<String>,
            kind: Option<String>,
            person_role: Option<String>,
            door: Option<String>,
            owner: Option<String>,
        }
        let rows: Vec<Row> = self.typed(
            "SELECT m.role, m.kind, (SELECT role FROM members WHERE principal = m.owner) AS person_role, \
             (SELECT value FROM meta WHERE key = ?) AS door, (SELECT value FROM meta WHERE key = ?) AS owner FROM members m WHERE m.principal = ?",
            vec![MetaKey::DoorTemplate.key().into(), MetaKey::Owner.key().into(), principal.into()],
        )?;
        let Some(row) = rows.into_iter().next() else { return Ok(false) };
        let read = match self.channel_read_role(channel) {
            Ok(read) => Some(read),
            Err(gone) if gone.code == ErrorCode::NotFound => None,
            Err(e) => return Err(e),
        };
        let role = |r: Option<String>| r.as_deref().and_then(Role::parse);
        // an agent's row names its person (members.rs `entity_of`): one in
        // here, not as its owner, is beside it (fragment.rs `standing`)
        let beside_person = role(row.person_role).is_some_and(|r| r != Role::Owner);
        let held = match (beside_person, hold) {
            (false, _) => None,
            (true, Hold::Signed(held)) => held,
            (true, Hold::Registry) => crate::ask_registry(&self.env, &crate::registry::calls::Lookup { who: principal.to_string() }).await?.held,
        };
        let person = row.kind.as_deref() == Some(IdentityKind::Person.as_str());
        let owner_in_person = person && row.owner.as_deref().is_some_and(|o| o == principal || npub::display(o) == principal);
        let standing = Standing {
            member: role(row.role),
            signed: true,
            held,
            beside_person,
            connector: row.door.is_some(),
            owner_in_person,
            ..Standing::default()
        };
        Ok(subscription::delivers(self.visibility()?, standing, read))
    }

    /// The queue's consumer asks before it sends a record delivery
    /// (deliveries.rs `consume_one`): whether subscription `sub` of this
    /// fragment's life `incarnation` is still there and its subscriber may
    /// read its channel now (`may_read`). A fragment made again since, or
    /// none at all, answers no.
    pub(crate) async fn deliverable(&self, d: Deliverable) -> CellResult<bool> {
        if self.meta(MetaKey::CreatedAt)?.as_deref() != Some(d.incarnation.as_str()) {
            return Ok(false);
        }
        let subs = self.rows("SELECT principal, channel FROM subs WHERE id = ?", vec![SqlStorageValue::Integer(d.sub)])?;
        let Some(row) = subs.first() else { return Ok(false) };
        let principal = row["principal"].as_str().expect("subs.principal is TEXT");
        let channel = row["channel"].as_str().expect("subs.channel is TEXT");
        self.may_read(principal, channel, Hold::Registry).await
    }

    /// A member's subscriptions end with its membership.
    pub(crate) fn drop_subscriptions(&self, principal: &str) -> CellResult<()> {
        self.exec("DELETE FROM subs WHERE principal = ?", vec![principal.into()])
    }

    /// A new record's deliveries, one per subscription to its channel,
    /// written to the delivery outbox in the same step as the record (the
    /// caller appends, then calls this, with no await between; channels.rs
    /// `published`).
    pub(crate) fn outbox_record(&self, record: &ChannelRecord) -> CellResult<()> {
        self.test_countdown(MetaKey::TestFailOutbox, "the record's outbox write failed after its append")?;
        // a computer is never woken by what its own agents post (its poster
        // holds a wake subscription to the same computer): it was awake to
        // post. The poster is the record's as stored, not as answered: the
        // fragment's own key (its cron's, the platform's records) is shown
        // as its npub, which on an agent's fragment is its agent's identity,
        // yet it is no agent posting, and wakes it.
        self.exec(
            "INSERT INTO delivery_outbox (kind, sub, channel, seq, next_at) SELECT 'record', id, channel, ?, ? FROM subs WHERE channel = ? \
             AND NOT (url LIKE 'computer:%' AND url IN (SELECT url FROM subs WHERE url LIKE 'computer:%' \
               AND principal = (SELECT principal FROM records WHERE channel = ? AND seq = ?)))",
            vec![
                SqlStorageValue::Integer(record.seq),
                SqlStorageValue::Integer(crate::js::now_ms()),
                record.channel.as_str().into(),
                record.channel.as_str().into(),
                SqlStorageValue::Integer(record.seq),
            ],
        )?;
        self.exec(
            "UPDATE records SET outboxed = 1 WHERE channel = ? AND seq = ?",
            vec![record.channel.as_str().into(), SqlStorageValue::Integer(record.seq)],
        )
    }

    /// Whether a record's deliveries were written (`outbox_record`).
    pub(crate) fn outboxed(&self, record: &ChannelRecord) -> CellResult<bool> {
        let rows = self.rows(
            "SELECT outboxed FROM records WHERE channel = ? AND seq = ?",
            vec![record.channel.as_str().into(), SqlStorageValue::Integer(record.seq)],
        )?;
        let row = rows.first().ok_or_else(|| CellError::host(format!("record {} of {:?} is gone", record.seq, record.channel)))?;
        Ok(row["outboxed"].as_i64().expect("records.outboxed is INTEGER NOT NULL") != 0)
    }

    /// When record `seq` of `channel` was posted (ms), if it is there.
    pub(crate) fn record_at(&self, channel: &str, seq: i64) -> Option<i64> {
        self.read_channel(channel, seq - 1, 1).ok()?.into_iter().find(|r| r.seq == seq).map(|r| r.at)
    }

    /// The delivery of record `seq` of `channel` to subscription `sub`, or
    /// `None` when there is nothing to send: the subscription ended, its
    /// subscriber may not read the channel now (`may_read`), or the
    /// record is past its retention.
    pub(crate) async fn record_delivery(&self, sub: i64, channel: &str, seq: i64, fragment: &str, incarnation: &str) -> CellResult<Option<Delivery>> {
        let subs = self.rows("SELECT principal, url FROM subs WHERE id = ?", vec![SqlStorageValue::Integer(sub)])?;
        let Some(row) = subs.first() else { return Ok(None) };
        let principal = row["principal"].as_str().expect("subs.principal is TEXT");
        let url = row["url"].as_str().expect("subs.url is TEXT").to_string();
        if !self.may_read(principal, channel, Hold::Registry).await? {
            return Ok(None);
        }
        let Some(record) = self.read_channel(channel, seq - 1, 1)?.into_iter().next().filter(|r| r.seq == seq) else { return Ok(None) };
        // serialized as it is: the record's body goes out as the text the cell stored
        let delivery = fragment_proto::Delivery { kind: DeliveryType::Record, fragment: fragment.to_string(), channel: channel.to_string(), record };
        let body = serde_json::to_string(&delivery).expect("a delivery serializes");
        Ok(Some(Delivery::json(fragment, incarnation, DeliveryKind::Record, url, &body, Some(sub))))
    }
}
