//! Invites by email (docs/cloudflare-v1.md, decisions 48 and 63). A
//! fragment shares with an email: the invite waits on it in the fragment
//! (members.rs), and here, kept against the email, which the platform
//! mails a link to its own page. Nobody joins someone else's fragment
//! without their own yes (Paul, 2026-10-22: an invite "shouldn't auto add
//! to their app list"), as nobody joins an org without theirs (offers.rs).
//! Whoever signs in with the email, verified, sees it (`GET /api/invites`:
//! the fragment's title as it was sent, its role, who shared it) and
//! accepts it, which makes them a member there (the fragment's
//! `invites/claim`), or declines it, which takes it from the fragment
//! (`invites/decline`). Signing in never makes a share, nor meets one. The
//! mails a person has sent are capped by the day.
//!
//! An invite mailed before invites waited for a yes (its row names no
//! role) said "sign in to open it": a sign-in as its email still meets it
//! (`meet_invites`), each fragment making its person a member, until the
//! last of them runs out (docs/technical-debt-ledger.md, "Invites from
//! before their yes"). A claim of one that fails (the fragment
//! unreachable, or full) keeps its invite's row, and its try is kept
//! (`invite_claims`): the registry's alarm tries it again for whoever holds
//! the email then, with the outboxes' backoff, at most `CLAIM_TRIES_MAX`
//! tries in all (`fragment_core::backoff::claim_retry_ms`). Past them it
//! waits on its person's next sign-in, which tries it afresh.

use fragment_core::{backoff, mail};
use fragment_proto::{limits, ErrorCode, Identity, IdentityKind, Reason, Role};
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use worker::*;

use super::calls::{By, Call, ClaimInvite, InviteEmail};
use super::{body, reply, RegistryCell};
use crate::error::{CellError, CellResult};
use crate::js;

const DAY_MS: i64 = 24 * 3600 * 1000;

/// Fragments a sign-in, or a run of the alarm, tells at once that it met
/// their invites.
const CLAIMS_AT_ONCE: usize = 32;

/// How soon the alarm's kept claims run again after a run that failed.
const CLAIMS_LATER_MS: i64 = 60_000;

/// `POST /invites/waiting`: the invites waiting on the emails the asker (a
/// person, or their agent acting `for` them: `invitee_by`) signs in with.
#[derive(Serialize, Deserialize)]
pub(crate) struct InvitesFor {
    pub by: By,
    pub acting_for: Option<String>,
}

impl Call for InvitesFor {
    const PATH: &'static str = "/invites/waiting";
    type Answer = serde_json::Value;
}

/// `POST /invites/answer`: the asker's yes (`yes`) or no to `fragment`'s
/// invite waiting on an email of theirs. Answers the role they hold there
/// after a yes, none after a no; 404 when none waits (taken back,
/// answered, run out). One call for both: the cell's Wasm carries one
/// round trip, not two.
#[derive(Serialize, Deserialize)]
pub(crate) struct AnswerInvite {
    pub by: By,
    pub acting_for: Option<String>,
    pub fragment: String,
    pub yes: bool,
}

impl Call for AnswerInvite {
    const PATH: &'static str = "/invites/answer";
    type Answer = Option<Role>;
}

/// `POST /invites/forget`: `fragment`'s owner took back its invite waiting
/// on `email` (members.rs `revoke_invite`), or, `email` none, the fragment
/// ended (ended.rs): forgotten here, and the pages of whoever holds each
/// email told.
#[derive(Serialize, Deserialize)]
pub(crate) struct ForgetInvite {
    pub email: Option<String>,
    pub fragment: String,
}

impl Call for ForgetInvite {
    const PATH: &'static str = "/invites/forget";
    type Answer = ();
}

#[derive(Deserialize)]
struct FragmentRow {
    fragment: String,
}

#[derive(Deserialize)]
struct EmailRow {
    email: String,
}

/// A kept claim that is due, as the alarm reads it: whether its invite
/// still waits, and who holds its email now.
#[derive(Deserialize)]
struct KeptRow {
    email: String,
    fragment: String,
    failed: i64,
    waits: i64,
    identity: Option<String>,
}

#[derive(Deserialize)]
struct DueRow {
    due: Option<i64>,
}

impl RegistryCell {
    /// `/invite`: the invite kept against the email, for its person's yes,
    /// and mailed. The row is written before the mail goes, so a list read
    /// meanwhile shows it, and taken back when the mail is not sent. Whoever
    /// holds the email now has their open pages told.
    pub(super) async fn invite(&self, i: InviteEmail) -> CellResult<()> {
        if !mail::valid_address(&i.email) || i.email != i.email.to_ascii_lowercase() {
            return Err(CellError::invalid(format!("{:?} is not a lower-case email", i.email)));
        }
        let now = js::now_ms();
        if i.expires_at <= now {
            return Err(CellError::invalid("an invite expires later than now"));
        }
        let from = self.email_of(&i.sharer)?.ok_or_else(|| CellError::new(ErrorCode::Forbidden, "only a person who signs in shares by email"))?;
        self.exec("DELETE FROM invites_waiting WHERE expires_at <= ?", vec![SqlStorageValue::Integer(now)])?;
        let waiting = self.count(
            "SELECT COUNT(*) AS n FROM invites_waiting WHERE email = ? AND fragment != ?",
            vec![i.email.as_str().into(), i.fragment.as_str().into()],
        )?;
        if waiting >= limits::INVITES_PER_EMAIL_MAX as u64 {
            return Err(CellError::new(ErrorCode::RateLimited, format!("{} invites wait on {} already", limits::INVITES_PER_EMAIL_MAX, i.email)));
        }
        self.count_mail(&i.sharer, now)?;
        self.exec(
            "INSERT INTO invites_waiting (email, fragment, expires_at, role, title, sharer) VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT (email, fragment) DO UPDATE SET expires_at = excluded.expires_at, role = excluded.role, title = excluded.title, sharer = excluded.sharer",
            vec![
                i.email.as_str().into(),
                i.fragment.as_str().into(),
                SqlStorageValue::Integer(i.expires_at),
                i.role.as_str().into(),
                i.title.as_str().into(),
                i.sharer.as_str().into(),
            ],
        )?;
        // the platform's own page, where its person accepts it: never the
        // fragment's, which is no one's to open until they do
        let link = format!("{}/", self.cfg.platform());
        let days = limits::INVITE_TTL_S / (24 * 3600);
        let invite = mail::invite(&i.email, &from, &i.title, i.role == Role::Editor, &link, days);
        let sent = crate::mail::send(&self.env, self.cfg, &invite).await;
        if let Err(e) = sent {
            self.exec(
                "DELETE FROM invites_waiting WHERE email = ? AND fragment = ? AND expires_at = ?",
                vec![i.email.as_str().into(), i.fragment.as_str().into(), SqlStorageValue::Integer(i.expires_at)],
            )?;
            return Err(e);
        }
        self.tell_invitee(&i.email).await;
        Ok(())
    }

    /// Counts a mail in `sharer`'s day, or refuses it (429), counting
    /// nothing. Days before yesterday are forgotten.
    fn count_mail(&self, sharer: &str, now: i64) -> CellResult<()> {
        let day = now / DAY_MS;
        let mailed = self.count(
            "SELECT COALESCE(SUM(mailed), 0) AS n FROM invite_mails WHERE sharer = ? AND day = ?",
            vec![sharer.into(), SqlStorageValue::Integer(day)],
        )?;
        if mailed >= u64::from(limits::INVITE_MAILS_DAILY_MAX) {
            return Err(CellError::new(
                ErrorCode::RateLimited,
                format!("you have had {} invites mailed today, the most a day: invite more tomorrow", limits::INVITE_MAILS_DAILY_MAX),
            ));
        }
        self.exec(
            "INSERT INTO invite_mails (sharer, day, mailed) VALUES (?, ?, 1) ON CONFLICT (sharer, day) DO UPDATE SET mailed = mailed + 1",
            vec![sharer.into(), SqlStorageValue::Integer(day)],
        )?;
        self.exec("DELETE FROM invite_mails WHERE day < ?", vec![SqlStorageValue::Integer(day - 1)])
    }

    /// Tells the open pages of whoever signs in as `email` now that the
    /// invites waiting on it changed, so each reads them again. Best
    /// effort: a page that missed it reads them as it reconnects.
    async fn tell_invitee(&self, email: &str) {
        if let Ok(Some(person)) = self.person_by_email(email) {
            self.tell_person(&person).await;
        }
    }

    async fn tell_person(&self, person: &str) {
        if let Err(e) = crate::principal::tell_invited(&self.env, person).await {
            console_error!("{}", serde_json::json!({ "invites-told": person, "failed": e.message, "code": e.code }));
        }
    }

    /// The person whose invites a call reads or answers: a person, as
    /// themselves; or their own agent, acting `for` them and not held below
    /// them (Paul, 2026-10-22: "It can already publish for you and edit
    /// your own stuff! Why shouldn't it be able to handle 'skyler invited
    /// me to edit a fragment, do you see it?'"), as an agent shares for its
    /// owner (`access::agent_shares`). Any other agent is 403; a person
    /// naming `for`, 400.
    fn invitee_by(&self, by: &By, acting_for: Option<&str>) -> CellResult<Identity> {
        let who = self.by(by)?;
        match (who.kind, who.owner.as_deref()) {
            (IdentityKind::Person, _) if acting_for.is_none() => Ok(who),
            (IdentityKind::Person, _) => Err(CellError::invalid("a person answers their own invites: `for` is an agent's, acting for its owner")),
            (IdentityKind::Agent, Some(owner)) if acting_for == Some(owner) && who.held.is_none() => self.stored_identity(owner, "an agent's owner"),
            (IdentityKind::Agent, _) => Err(CellError::new(
                ErrorCode::Forbidden,
                "an agent reads and answers its owner's invites only acting for them (`for` its owner), and not held below them",
            )),
        }
    }

    /// The invites waiting for `by`'s yes: those on the emails they sign in
    /// with, the soonest to run out first, each with who shared it (their
    /// latest sign-in's email). Bounded: at most `INVITES_PER_EMAIL_MAX`
    /// (an email holds that many; a person has at most 8 emails, and sees
    /// the soonest of them).
    fn waiting_for(&self, b: InvitesFor) -> CellResult<serde_json::Value> {
        let who = self.invitee_by(&b.by, b.acting_for.as_deref())?;
        // read as the wire names them (proto's `WaitingInvite`; a row with a
        // role has its title: they are written together), and passed on as
        // read: the cell's Wasm carries no visitor of its own for them
        let invites = self.rows::<serde_json::Value>(
            "SELECT w.fragment, w.role, COALESCE(w.title, w.fragment) AS title, w.email, w.expires_at AS \"expiresAt\",
               (SELECT s.email FROM subjects s WHERE s.identity = w.sharer ORDER BY s.signed_in_at DESC LIMIT 1) AS \"from\"
             FROM invites_waiting w
             WHERE w.role IN ('viewer', 'editor') AND w.expires_at > ? AND w.email IN (SELECT email FROM subjects WHERE identity = ?)
             ORDER BY w.expires_at, w.fragment LIMIT ?",
            vec![SqlStorageValue::Integer(js::now_ms()), who.id.as_str().into(), SqlStorageValue::Integer(limits::INVITES_PER_EMAIL_MAX as i64)],
        )?;
        Ok(serde_json::json!({ "invites": invites }))
    }

    /// The email of `person`'s that `fragment`'s invite waits on for their
    /// yes, or 404.
    fn invite_to(&self, person: &str, fragment: &str) -> CellResult<String> {
        let row = self.row::<EmailRow>(
            "SELECT email FROM invites_waiting WHERE fragment = ? AND role IS NOT NULL AND expires_at > ?
               AND email IN (SELECT email FROM subjects WHERE identity = ?) ORDER BY expires_at LIMIT 1",
            vec![fragment.into(), SqlStorageValue::Integer(js::now_ms()), person.into()],
        )?;
        let gone = || CellError::new(ErrorCode::NotFound, format!("no invite to {fragment} waits on you: it was taken back, answered, or ran out")).because(Reason::Gone);
        row.map(|r| r.email).ok_or_else(gone)
    }

    /// Forgets `fragment`'s invite waiting on `email`: whether one went.
    fn forget(&self, email: &str, fragment: &str) -> CellResult<bool> {
        let key = || vec![SqlStorageValue::from(email), SqlStorageValue::from(fragment)];
        let had = self.count("SELECT COUNT(*) AS n FROM invites_waiting WHERE email = ? AND fragment = ?", key())? > 0;
        self.exec("DELETE FROM invites_waiting WHERE email = ? AND fragment = ?", key())?;
        self.exec("DELETE FROM invite_claims WHERE email = ? AND fragment = ?", key())?;
        Ok(had)
    }

    /// Their yes, or their no. A yes: the fragment makes them a member at
    /// the role its invite said (a stronger role they hold is kept), the
    /// role answered. A no: the fragment holds the invite no more (its
    /// owner's list counts it no more). Either way it waits no more here.
    /// One the fragment holds no more (taken back meanwhile, or the
    /// fragment deleted) is forgotten, a yes to it 404; one it cannot take
    /// or be told of now (full, or unreachable) is kept, and its refusal
    /// answered.
    async fn answer_invite(&self, b: AnswerInvite) -> CellResult<Option<Role>> {
        let who = self.invitee_by(&b.by, b.acting_for.as_deref())?;
        let email = self.invite_to(&who.id, &b.fragment)?;
        let claim = serde_json::to_value(ClaimInvite { email: email.clone(), identity: who.id.clone() }).expect("a claim serializes");
        let route = if b.yes { "invites/claim" } else { "invites/decline" };
        let met = match crate::fragment::ask(&self.env, &b.fragment, route, &claim).await {
            Ok(met) => Some(met),
            Err(e) if e.code == ErrorCode::NotFound => None,
            Err(e) => return Err(e),
        };
        // answered, or none waiting there: it waits here no more either,
        // and their open pages (another tab, their agent's answer) read
        // their invites again
        self.forget(&email, &b.fragment)?;
        self.tell_person(&who.id).await;
        match (b.yes, met) {
            (false, _) => Ok(None),
            (true, Some(met)) => met["role"].as_str().and_then(Role::parse).map(Some).ok_or_else(|| CellError::host(format!("{}'s claim named no role: {met}", b.fragment))),
            (true, None) => Err(CellError::new(ErrorCode::NotFound, format!("the invite to {} is gone: it was taken back, or it was deleted", b.fragment)).because(Reason::Gone)),
        }
    }

    /// Its owner took it back, or the fragment ended (every invite of its,
    /// `email` none): forgotten, and the pages of whoever holds each email
    /// told, at most `INVITES_MAX` (a fragment holds no more).
    async fn forget_invite(&self, f: ForgetInvite) -> CellResult<()> {
        let emails: Vec<String> = match f.email {
            Some(email) => self.forget(&email, &f.fragment)?.then_some(email).into_iter().collect(),
            None => {
                self.exec("DELETE FROM invite_claims WHERE fragment = ?", vec![f.fragment.as_str().into()])?;
                let gone = self.rows::<EmailRow>("DELETE FROM invites_waiting WHERE fragment = ? RETURNING email", vec![f.fragment.as_str().into()])?;
                gone.into_iter().take(limits::INVITES_MAX).map(|r| r.email).collect()
            }
        };
        join_all(emails.iter().map(|email| self.tell_invitee(email))).await;
        Ok(())
    }

    /// The registry's routes for invites, or `None`.
    pub(super) async fn invites_route(&self, path: &str, bytes: &[u8]) -> Option<CellResult<Response>> {
        Some(match path {
            InviteEmail::PATH => reply::<InviteEmail>(async { self.invite(body(bytes)?).await }.await),
            InvitesFor::PATH => reply::<InvitesFor>(body(bytes).and_then(|b| self.waiting_for(b))),
            AnswerInvite::PATH => reply::<AnswerInvite>(async { self.answer_invite(body(bytes)?).await }.await),
            ForgetInvite::PATH => reply::<ForgetInvite>(async { self.forget_invite(body(bytes)?).await }.await),
            _ => return None,
        })
    }

    /// A sign-in that verified `email` for the person `identity` meets the
    /// invites from before invites waited for a yes (rows with no role):
    /// each fragment makes them a member, as their mail said. A fragment
    /// that has none waiting (revoked, met, deleted) is forgotten; one that
    /// fails keeps its row, and its claim is kept for the alarm (this try
    /// its first), and the sign-in goes on. The invites since wait for
    /// their yes (`accept_invite`).
    pub(super) async fn meet_invites(&self, email: &str, identity: &str) -> CellResult<()> {
        let now = js::now_ms();
        self.exec("DELETE FROM invites_waiting WHERE expires_at <= ?", vec![SqlStorageValue::Integer(now)])?;
        let waiting = self.rows::<FragmentRow>(
            "SELECT fragment FROM invites_waiting WHERE email = ? AND role IS NULL ORDER BY expires_at LIMIT ?",
            vec![email.into(), SqlStorageValue::Integer(limits::INVITES_PER_EMAIL_MAX as i64)],
        )?;
        let claim = serde_json::to_value(ClaimInvite { email: email.to_string(), identity: identity.to_string() }).expect("a claim serializes");
        for batch in waiting.chunks(CLAIMS_AT_ONCE) {
            let answers = join_all(batch.iter().map(|w| crate::fragment::ask(&self.env, &w.fragment, "invites/claim", &claim))).await;
            for (w, answer) in batch.iter().zip(answers) {
                self.claimed(email, &w.fragment, 0, answer)?;
            }
        }
        self.arm_claims().await
    }

    /// What a claim's answer leaves, `failed` tries having failed before it:
    /// met, or none waiting there, its invite is forgotten here, and its
    /// kept claim with it; failed, its claim is kept, due after its
    /// backoff, or, past its tries, left to its person's next sign-in.
    fn claimed(&self, email: &str, fragment: &str, failed: i64, answer: CellResult<serde_json::Value>) -> CellResult<()> {
        let failure = match answer {
            Ok(_) => None,
            Err(e) if e.code == ErrorCode::NotFound => None,
            Err(e) => Some(e),
        };
        if let Some(e) = failure {
            let failed = failed.saturating_add(1);
            console_error!("{}", serde_json::json!({ "invite-claim": fragment, "tries": failed, "failed": e.message, "code": e.code }));
            return match backoff::claim_retry_ms(failed) {
                Some(wait) => self.exec(
                    "INSERT INTO invite_claims (email, fragment, due, failed) VALUES (?, ?, ?, ?)
                     ON CONFLICT (email, fragment) DO UPDATE SET due = excluded.due, failed = excluded.failed",
                    vec![email.into(), fragment.into(), SqlStorageValue::Integer(js::now_ms() + wait), SqlStorageValue::Integer(failed)],
                ),
                None => self.exec("DELETE FROM invite_claims WHERE email = ? AND fragment = ?", vec![email.into(), fragment.into()]),
            };
        }
        self.forget(email, fragment).map(|_| ())
    }

    /// The alarm's claims: a batch of the kept ones that are due, each for
    /// whoever holds its email now, all at once. One whose invite waits no
    /// more (met or expired), or whose email no one holds now (their wipe
    /// ended them), or whose person a wipe of whom runs, is forgotten: its
    /// invite, if it waits, waits on a sign-in again. Then the alarm is
    /// armed for the next.
    pub(super) async fn claims_alarm(&self) -> CellResult<()> {
        let now = SqlStorageValue::Integer(js::now_ms());
        let due = self.rows::<KeptRow>(
            "SELECT c.email, c.fragment, c.failed,
                    EXISTS (SELECT 1 FROM invites_waiting w WHERE w.email = c.email AND w.fragment = c.fragment AND w.expires_at > ? AND w.role IS NULL) AS waits,
                    (SELECT identity FROM subjects WHERE email = c.email LIMIT 1) AS identity
             FROM invite_claims c WHERE c.due <= ? ORDER BY c.due LIMIT ?",
            vec![now.clone(), now, SqlStorageValue::Integer(CLAIMS_AT_ONCE as i64)],
        )?;
        let mut tried = Vec::with_capacity(due.len());
        for kept in due {
            match kept.identity {
                Some(person) if kept.waits != 0 && !self.wiping(&person)? => tried.push((kept.email, kept.fragment, kept.failed, person)),
                _ => self.exec("DELETE FROM invite_claims WHERE email = ? AND fragment = ?", vec![kept.email.as_str().into(), kept.fragment.as_str().into()])?,
            }
        }
        // bounded: at most CLAIMS_AT_ONCE claims a run
        let answers = join_all(tried.iter().map(|(email, fragment, _, person)| {
            let claim = serde_json::to_value(ClaimInvite { email: email.clone(), identity: person.clone() }).expect("a claim serializes");
            async move { crate::fragment::ask(&self.env, fragment, "invites/claim", &claim).await }
        }))
        .await;
        for ((email, fragment, failed, _), answer) in tried.iter().zip(answers) {
            self.claimed(email, fragment, *failed, answer)?;
        }
        self.arm_claims().await
    }

    /// Arms the alarm for the earliest kept claim, unless it is armed sooner.
    pub(super) async fn arm_claims(&self) -> CellResult<()> {
        match self.row::<DueRow>("SELECT MIN(due) AS due FROM invite_claims", vec![])?.and_then(|r| r.due) {
            Some(due) => self.arm_at(due).await,
            None => Ok(()),
        }
    }

    /// After a failed run: the next one soon, unless one is armed sooner.
    pub(super) async fn claims_later(&self) -> CellResult<()> {
        self.arm_at(js::now_ms() + CLAIMS_LATER_MS).await
    }
}
