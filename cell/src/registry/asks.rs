//! Requests for access to a fragment (docs/api.md, Asking for access;
//! FIN-502). Someone signed in who is not in a fragment asks its owner for
//! it, on the platform's page (auth/ask.rs): the router asks the fragment
//! whom it names its owner, and the request is kept here against that
//! owner, who sees it among their notices (`asked_of`) and is mailed about
//! it. Their answer is their own invite to the requester's email (`POST
//! /api/f/{fragment}/invites`, which takes the request away: `asks_met`),
//! or Dismiss (`dismiss`). A request gives the requester nothing: an
//! invite still waits for their yes (decision 63).
//!
//! Bounded: one request a fragment and person, which runs out
//! (`ACCESS_ASK_TTL_S`); a person makes at most `ACCESS_ASKS_DAILY_MAX` a
//! day, a fragment holds at most `ACCESS_ASKS_PER_FRAGMENT_MAX`, and an
//! owner is mailed about at most `ACCESS_ASK_MAILS_DAILY_MAX` a day (the
//! rest are their notices alone). One dismissed stays a while
//! (`ACCESS_ASK_QUIET_S`), so the same person asking again tells its owner
//! nothing; the requester is answered as for any request asked again, and
//! never told it was dismissed.

use fragment_core::{mail, npub, registry::said};
use fragment_proto::notice::Notice;
use fragment_proto::{limits, ErrorCode, IdentityKind, Reason};
use serde::{Deserialize, Serialize};
use worker::*;

use super::calls::{By, Call};
use super::{body, reply, RegistryCell};
use crate::error::{CellError, CellResult};
use crate::js;

const DAY_MS: i64 = 24 * 3600 * 1000;

/// `POST /asks/ask`, the fragment's own (members.rs `ask_access`): `by`
/// (a person signed in, not in it) asks for access to `fragment`, the life
/// of it `incarnation` (its creation's time), whose owner is `owner` and
/// title `title`. The fragment sends it while that life is its own, so a
/// request never names a life that ended before it was sent.
#[derive(Serialize, Deserialize)]
pub(crate) struct AskAccess {
    pub by: By,
    pub fragment: String,
    pub incarnation: String,
    pub owner: String,
    pub title: String,
}

/// A request kept: whether one was kept already (`again`: nothing is told
/// its owner this time), and the email its owner sees it by.
#[derive(Serialize, Deserialize)]
pub(crate) struct Asked {
    pub again: bool,
    pub email: String,
}

impl Call for AskAccess {
    const PATH: &'static str = "/asks/ask";
    type Answer = Asked;
}

/// `POST /asks/dismiss`: `fragment`'s owner (or their agent, acting `for`
/// them) dismisses `requester`'s request. 404 when none waits on them.
#[derive(Serialize, Deserialize)]
pub(crate) struct DismissAsk {
    pub by: By,
    pub acting_for: Option<String>,
    pub fragment: String,
    pub requester: String,
}

impl Call for DismissAsk {
    const PATH: &'static str = "/asks/dismiss";
    type Answer = ();
}

#[derive(Deserialize)]
struct LifeRow {
    incarnation: String,
}

#[derive(Deserialize)]
struct OwnerRow {
    owner: String,
}

#[derive(Deserialize)]
struct NoticedRow {
    fragment: String,
    requester: String,
    title: String,
    email: Option<String>,
}

impl RegistryCell {
    /// Requests that ran out, dismissed ones past their quiet, and counts
    /// before yesterday, gone.
    fn sweep_asks(&self, now: i64) -> CellResult<()> {
        self.exec("DELETE FROM access_asks WHERE expires_at <= ?", vec![SqlStorageValue::Integer(now)])?;
        self.exec("DELETE FROM access_ask_days WHERE day < ?", vec![SqlStorageValue::Integer(now / DAY_MS - 1)])
    }

    /// Today's count of `kind` for `who`.
    fn ask_day(&self, who: &str, kind: &str, now: i64) -> CellResult<u64> {
        self.count(
            "SELECT COALESCE(SUM(n), 0) AS n FROM access_ask_days WHERE who = ? AND kind = ? AND day = ?",
            vec![who.into(), kind.into(), SqlStorageValue::Integer(now / DAY_MS)],
        )
    }

    fn count_ask_day(&self, who: &str, kind: &str, now: i64) -> CellResult<()> {
        self.exec(
            "INSERT INTO access_ask_days (who, kind, day, n) VALUES (?, ?, ?, 1) ON CONFLICT (who, kind, day) DO UPDATE SET n = n + 1",
            vec![who.into(), kind.into(), SqlStorageValue::Integer(now / DAY_MS)],
        )
    }

    /// `/asks/ask`: the request kept against its owner, who is told (their
    /// pages, and a mail while their day allows), unless one is kept
    /// already (asked again, or dismissed a while ago: nothing told). Every
    /// check comes before any write.
    async fn ask_access(&self, b: AskAccess) -> CellResult<Asked> {
        if !fragment_proto::valid_fragment_name(&b.fragment) || b.incarnation.is_empty() || !npub::is_identity(&b.owner) || b.title.chars().count() > limits::TITLE_MAX_CHARS {
            return Err(CellError::invalid("a request names a fragment, its life, its owner's identity, and its title"));
        }
        let who = self.by(&b.by)?;
        if who.kind != IdentityKind::Person {
            return Err(CellError::new(ErrorCode::Forbidden, "a person asks for access, on the platform's page: an agent asks its owner"));
        }
        if who.id == b.owner {
            return Err(CellError::new(ErrorCode::Conflict, format!("{} is yours", b.fragment)));
        }
        let owner = self.stored_identity(&b.owner, "a fragment's owner")?;
        if owner.kind != IdentityKind::Person {
            return Err(CellError::host(format!("{} names {} its owner, who is no person", b.fragment, b.owner)));
        }
        let email = self.email_of(&who.id)?.ok_or_else(|| CellError::new(ErrorCode::Forbidden, "sign in with an email to ask for access: its owner sees you by it"))?;
        let now = js::now_ms();
        self.sweep_asks(now)?;
        let key = || vec![SqlStorageValue::from(b.fragment.as_str()), SqlStorageValue::from(who.id.as_str())];
        if let Some(kept) = self.row::<LifeRow>("SELECT incarnation FROM access_asks WHERE fragment = ? AND requester = ?", key())? {
            if kept.incarnation == b.incarnation {
                // asked again, or dismissed a while ago: its owner is told nothing
                return Ok(Asked { again: true, email });
            }
            // asked of an earlier life of the name: this life's owner hears it anew
            self.exec("DELETE FROM access_asks WHERE fragment = ? AND requester = ?", key())?;
        }
        if self.count("SELECT COUNT(*) AS n FROM access_asks WHERE fragment = ?", vec![b.fragment.as_str().into()])? >= limits::ACCESS_ASKS_PER_FRAGMENT_MAX as u64 {
            return Err(CellError::new(ErrorCode::RateLimited, "its owner has many requests waiting: ask again later"));
        }
        if self.ask_day(&who.id, "asked", now)? >= u64::from(limits::ACCESS_ASKS_DAILY_MAX) {
            return Err(CellError::new(ErrorCode::RateLimited, format!("a person asks for access {} times a day at most: ask again tomorrow", limits::ACCESS_ASKS_DAILY_MAX)));
        }
        self.exec(
            "INSERT INTO access_asks (fragment, requester, incarnation, owner, title, asked_at, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
            vec![
                b.fragment.as_str().into(),
                who.id.as_str().into(),
                b.incarnation.as_str().into(),
                owner.id.as_str().into(),
                b.title.as_str().into(),
                SqlStorageValue::Integer(now),
                SqlStorageValue::Integer(now + limits::ACCESS_ASK_TTL_S * 1000),
            ],
        )?;
        self.count_ask_day(&who.id, "asked", now)?;
        self.mail_owner(&owner.id, &email, &b.title, now).await?;
        self.tell_person(&owner.id).await;
        Ok(Asked { again: false, email })
    }

    /// Mails `owner` about a request, while their day allows and they have
    /// an email. The mail is counted before it goes, in the same turn as
    /// the check (as an invite's is): two requests at once never pass one
    /// place left, and nothing is written after the send, so a wipe of the
    /// owner meanwhile leaves no row naming them. The request stands
    /// whether its mail went or not (their notice is the request), so a
    /// mail not sent is logged, not refused, and still counted.
    async fn mail_owner(&self, owner: &str, from: &str, title: &str, now: i64) -> CellResult<()> {
        let Some(to) = self.email_of(owner)? else { return Ok(()) };
        if self.ask_day(owner, "mailed", now)? >= u64::from(limits::ACCESS_ASK_MAILS_DAILY_MAX) {
            return Ok(());
        }
        self.count_ask_day(owner, "mailed", now)?;
        let asked = mail::access_asked(&to, from, title, &format!("{}/", self.cfg.platform()));
        if let Err(e) = crate::mail::send(&self.env, self.cfg, &asked).await {
            console_error!("{}", serde_json::json!({ "event": "asks.mail-failed", "owner": owner, "code": e.code, "message": e.message }));
        }
        Ok(())
    }

    /// `/asks/dismiss`: its owner's no. It stays, dismissed, a while
    /// (`ACCESS_ASK_QUIET_S`), and is in their notices no more.
    async fn dismiss_ask(&self, b: DismissAsk) -> CellResult<()> {
        let who = self.invitee_by(&b.by, b.acting_for.as_deref())?;
        let now = js::now_ms();
        self.sweep_asks(now)?;
        let key = || vec![b.fragment.as_str().into(), b.requester.as_str().into(), who.id.as_str().into()];
        let waits = self.count("SELECT COUNT(*) AS n FROM access_asks WHERE fragment = ? AND requester = ? AND owner = ? AND dismissed_at IS NULL", key())? > 0;
        if !waits {
            return Err(CellError::new(ErrorCode::NotFound, format!("no request for {} from {} waits on you", b.fragment, b.requester)).because(Reason::Gone));
        }
        let mut binds = vec![SqlStorageValue::Integer(now), SqlStorageValue::Integer(now + limits::ACCESS_ASK_QUIET_S * 1000)];
        binds.extend(key());
        self.exec("UPDATE access_asks SET dismissed_at = ?, expires_at = ? WHERE fragment = ? AND requester = ? AND owner = ?", binds)?;
        self.tell_person(&who.id).await;
        Ok(())
    }

    /// The requests waiting on `owner`, as their notices say them, the
    /// newest first, at most `ACCESS_ASKS_NOTICED_MAX`. One whose
    /// requester no one signs in as now (wiped meanwhile) is not told.
    pub(super) fn asked_of(&self, owner: &str) -> CellResult<Vec<Notice>> {
        let rows = self.rows::<NoticedRow>(
            "SELECT a.fragment, a.requester, a.title,
               (SELECT s.email FROM subjects s WHERE s.identity = a.requester ORDER BY s.signed_in_at DESC LIMIT 1) AS email
             FROM access_asks a WHERE a.owner = ? AND a.dismissed_at IS NULL AND a.expires_at > ?
             ORDER BY a.asked_at DESC LIMIT ?",
            vec![owner.into(), SqlStorageValue::Integer(js::now_ms()), SqlStorageValue::Integer(limits::ACCESS_ASKS_NOTICED_MAX as i64)],
        )?;
        assert!(rows.len() <= limits::ACCESS_ASKS_NOTICED_MAX, "an owner's notices hold a bounded number of requests");
        Ok(rows.into_iter().filter_map(|r| r.email.map(|email| said::access_asked(&r.fragment, &r.requester, &email, &r.title))).collect())
    }

    /// The requests an invite answers: `fragment`'s from whoever signs in
    /// as `email` (registry/invites.rs `invite`), or every one of its (it
    /// ended: `email` none), gone, dismissed ones too, and their owners'
    /// pages told.
    pub(super) async fn asks_met(&self, fragment: &str, email: Option<&str>) -> CellResult<()> {
        let owners = match email {
            Some(email) => self.rows::<OwnerRow>(
                "DELETE FROM access_asks WHERE fragment = ? AND requester IN (SELECT identity FROM subjects WHERE email = ?) RETURNING owner",
                vec![fragment.into(), email.into()],
            )?,
            None => self.rows::<OwnerRow>("DELETE FROM access_asks WHERE fragment = ? RETURNING owner", vec![fragment.into()])?,
        };
        assert!(owners.len() <= limits::ACCESS_ASKS_PER_FRAGMENT_MAX, "a fragment holds a bounded number of requests");
        let mut told: Vec<String> = owners.into_iter().map(|o| o.owner).collect();
        told.sort();
        told.dedup();
        for owner in told {
            self.tell_person(&owner).await;
        }
        Ok(())
    }

    /// The registry's routes for requests for access, or `None`.
    pub(super) async fn asks_route(&self, path: &str, bytes: &[u8]) -> Option<CellResult<Response>> {
        Some(match path {
            AskAccess::PATH => reply::<AskAccess>(async { self.ask_access(body(bytes)?).await }.await),
            DismissAsk::PATH => reply::<DismissAsk>(async { self.dismiss_ask(body(bytes)?).await }.await),
            _ => return None,
        })
    }
}
