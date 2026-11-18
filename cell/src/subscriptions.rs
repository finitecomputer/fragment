//! Channel subscriptions (phase 7): a member asks the fragment to deliver
//! each new record of a channel it may read to a URL, to wake the
//! computer it runs on (runs_on.rs), or, an agent, to its own fragment (a
//! follow). Records go out through the delivery outbox and queue
//! (deliveries.rs) as `{type: "record", fragment, channel, record}`
//! (`fragment_proto::Delivery`), unsigned: the URL is the subscriber's
//! capability. A computer's wake and a follow go from the outbox to the
//! computer, or the follower's supervisor, itself: a follow's record runs
//! the follower's `followed` triggers (jobs.rs `heard_followed`) at the depth
//! the record's own run at here, so fragments following one another stop
//! at `HOP_DEPTH_MAX`. A member's subscriptions end with its membership.
//!
//! "A subscription wakes a computer" (decision 39) could later be one case
//! of a follow: the agent's fragment hears the record, and wakes its
//! computer only when it uses it.

use fragment_core::{egress, npub};
use fragment_proto::{valid_channel_name, ChannelRecord, DeliveryType, ErrorCode};
use serde::Serialize;
use serde_json::{json, Value};
use worker::*;

use crate::deliveries::{Delivery, DeliveryKind};
use crate::error::{CellError, CellResult};
use crate::fragment::calls::Followed;
use crate::fragment::{json_response, Caller, FragmentCell, MetaKey};

/// What a follow names in place of a URL: its follower's fragment, which no
/// one fetches.
pub(crate) const FOLLOW_PREFIX: &str = "fragment:";

/// What a record of a subscription goes to (`record_delivery`).
pub(crate) enum Delivered {
    /// The queue's, to a URL.
    Queued(Delivery),
    /// A wake of `computer:<id>`, for a record posted `at`.
    Wake { computer: String, at: i64 },
    /// The record, to the fragment that follows the channel.
    Follow { follower: String, call: Followed },
}

#[derive(Serialize)]
struct Following<'a> {
    id: i64,
    channel: &'a str,
    follow: &'a str,
}

/// The subscriptions one fragment holds, and a URL's length.
const SUBS_MAX: u64 = 32;
const SUB_URL_MAX_BYTES: usize = 1024;

impl FragmentCell {
    /// The subscriber: a member whose role may read `channel`, as it reads
    /// it (an agent held below its owner, at its hold).
    fn subscriber(&self, caller: &Caller, channel: &str) -> CellResult<String> {
        let principal = caller.principal().map(str::to_string).ok_or_else(|| CellError::new(ErrorCode::Unauthenticated, "subscribing needs a signed request"))?;
        let Some(role) = self.member_role(&principal)? else {
            return Err(CellError::new(ErrorCode::Forbidden, "only a member subscribes to a channel"));
        };
        if !valid_channel_name(channel) {
            return Err(CellError::invalid("a channel name must match ^[a-z][a-z0-9_-]{0,63}$"));
        }
        let read = self.channel_read_role(channel)?;
        if role < read || self.require(caller, false, read).is_err() {
            return Err(CellError::new(ErrorCode::Forbidden, format!("your role may not read {channel}")));
        }
        Ok(principal)
    }

    /// `POST /api/subscriptions` `{channel, url}` → `{id, channel, url}`;
    /// `{channel, follow: true}`, an agent's, → `{id, channel, follow}`:
    /// its own fragment follows the channel.
    pub(crate) async fn subscribe(&self, caller: &Caller, body: &Value) -> CellResult<Response> {
        let channel = body["channel"].as_str().unwrap_or("");
        let principal = self.subscriber(caller, channel)?;
        if body["follow"] == true {
            if body.get("url").is_some() {
                return Err(CellError::invalid("a follow names no URL: the agent's own fragment follows"));
            }
            // the audit trail is read, never handed on: its records skip the outbox
            if matches!(channel, "events" | "ops") {
                return Err(CellError::invalid(format!("{channel} is read, not followed")));
            }
            let follower = self.follower(&principal).await?;
            // asked again after the registry's answer: a member removed meanwhile follows nothing
            let principal = self.subscriber(caller, channel)?;
            let id = self.add_subscription(&principal, channel, &format!("{FOLLOW_PREFIX}{follower}"))?;
            return json_response(&Following { id, channel, follow: &follower });
        }
        let url = body["url"].as_str().unwrap_or("");
        if url.len() > SUB_URL_MAX_BYTES {
            return Err(CellError::invalid(format!("a subscription URL is at most {SUB_URL_MAX_BYTES} bytes")));
        }
        egress::check(url, self.cfg.egress_local, &self.cfg.egress_allow).map_err(|e| CellError::invalid(format!("url: {e}")))?;
        let id = self.add_subscription(&principal, channel, url)?;
        json_response(&json!({ "id": id, "channel": channel, "url": url }))
    }

    /// The fragment agent `principal` is made from, which follows for it,
    /// as the registry names it: a person, or an agent no fragment made,
    /// follows nothing.
    async fn follower(&self, principal: &str) -> CellResult<String> {
        let asked = crate::registry::calls::Profiles { ids: vec![principal.to_string()], emails_of: vec![] };
        let mut profiles = crate::ask_registry(&self.env, &asked).await?.profiles;
        let none = || CellError::new(ErrorCode::Forbidden, "only an agent's own fragment follows a channel, and the asker is no agent made from one");
        profiles.remove(principal).and_then(|p| p.fragment).ok_or_else(none)
    }

    /// `principal`'s subscription to `channel` at `url` (a receiver's, a
    /// computer's wake: runs_on.rs, or a follow), made once: the same three
    /// again answer the same id.
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
                // a computer's wake subscription, and a follow, have no URL anyone fetches
                let wake = url.starts_with("computer:");
                let follow = url.strip_prefix(FOLLOW_PREFIX);
                let fetched = !wake && follow.is_none();
                json!({ "id": r["id"], "principal": npub::display(r["principal"].as_str().unwrap_or("")), "channel": r["channel"], "url": fetched.then_some(url), "wake": wake, "follow": follow, "createdAt": r["created_at"] })
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

    /// A member's subscriptions end with its membership.
    pub(crate) fn drop_subscriptions(&self, principal: &str) -> CellResult<()> {
        self.exec("DELETE FROM subs WHERE principal = ?", vec![principal.into()])
    }

    /// A follow that ends, saying why.
    pub(crate) fn unfollow(&self, sub: i64, follower: &str, why: &str) -> CellResult<()> {
        self.exec("DELETE FROM subs WHERE id = ?", vec![SqlStorageValue::Integer(sub)])?;
        self.event("subscription.gone", &format!("{follower} follows no more: {why}"), json!({ "sub": sub }));
        Ok(())
    }

    /// A new record's deliveries, one per subscription to its channel,
    /// written to the delivery outbox in the same step as the record (the
    /// caller appends, then calls this, with no await between; channels.rs
    /// `published`), each with the depth its runs here run at (`depth`).
    pub(crate) fn outbox_record(&self, record: &ChannelRecord, depth: u32) -> CellResult<()> {
        self.test_countdown(MetaKey::TestFailOutbox, "the record's outbox write failed after its append")?;
        // a computer is never woken by what its own agents post (its poster
        // holds a wake subscription to the same computer): it was awake to
        // post. The poster is the record's as stored, not as answered: the
        // fragment's own key (its cron's, the platform's records) is shown
        // as its npub, which on an agent's fragment is its agent's identity,
        // yet it is no agent posting, and wakes it. Nor does a follower
        // hear what its own agent posts: it posted it.
        self.exec(
            "INSERT INTO delivery_outbox (kind, sub, channel, seq, depth, next_at) SELECT 'record', id, channel, ?, ?, ? FROM subs WHERE channel = ? \
             AND NOT (url LIKE 'computer:%' AND url IN (SELECT url FROM subs WHERE url LIKE 'computer:%' \
               AND principal = (SELECT principal FROM records WHERE channel = ? AND seq = ?))) \
             AND NOT (url LIKE 'fragment:%' AND principal = (SELECT principal FROM records WHERE channel = ? AND seq = ?))",
            vec![
                SqlStorageValue::Integer(record.seq),
                SqlStorageValue::Integer(depth.into()),
                SqlStorageValue::Integer(crate::js::now_ms()),
                record.channel.as_str().into(),
                record.channel.as_str().into(),
                SqlStorageValue::Integer(record.seq),
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

    /// The delivery of record `seq` of `channel` to subscription `sub`, its
    /// runs here at `depth`, or `None` when there is nothing to send: the
    /// subscription ended, or the record is past its retention.
    pub(crate) fn record_delivery(&self, sub: i64, channel: &str, seq: i64, depth: u32, fragment: &str, incarnation: &str) -> CellResult<Option<Delivered>> {
        let subs = self.rows("SELECT url, principal FROM subs WHERE id = ?", vec![SqlStorageValue::Integer(sub)])?;
        let Some(row) = subs.first() else { return Ok(None) };
        let (url, subscriber) = (row["url"].as_str().expect("subs.url is TEXT"), row["principal"].as_str().expect("subs.principal is TEXT"));
        let record = self.read_channel(channel, seq - 1, 1)?.into_iter().next().filter(|r| r.seq == seq);
        if let Some(follower) = url.strip_prefix(FOLLOW_PREFIX) {
            // a follow is its member's while its role may read the channel
            if !self.member_role(subscriber)?.zip(self.channel_read_role(channel).ok()).is_some_and(|(role, read)| role >= read) {
                self.unfollow(sub, follower, "its agent may read the channel no more")?;
                return Ok(None);
            }
            let Some(record) = record else {
                self.event("delivery.failed", &format!("record {seq} of {channel} was past its retention before {follower} heard it"), json!({ "sub": sub, "seq": seq }));
                return Ok(None);
            };
            let call = Followed { subscriber: subscriber.to_string(), fragment: fragment.to_string(), incarnation: incarnation.to_string(), record, depth };
            return Ok(Some(Delivered::Follow { follower: follower.to_string(), call }));
        }
        let Some(record) = record else { return Ok(None) };
        if url.starts_with(crate::deliveries::WAKE_PREFIX) {
            return Ok(Some(Delivered::Wake { computer: url.to_string(), at: record.at }));
        }
        // serialized as it is: the record's body goes out as the text the cell stored
        let delivery = fragment_proto::Delivery { kind: DeliveryType::Record, fragment: fragment.to_string(), channel: channel.to_string(), record };
        let body = serde_json::to_string(&delivery).expect("a delivery serializes");
        Ok(Some(Delivered::Queued(Delivery::json(fragment, incarnation, DeliveryKind::Record, url.to_string(), &body, Some(sub)))))
    }
}
