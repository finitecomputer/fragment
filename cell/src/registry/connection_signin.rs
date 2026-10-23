//! A connection becomes usable at Pipes only after the initiating browser,
//! live platform session, provider and one-use pending state are verified.
use fragment_core::{catalog::{ConnectionOAuth, Kind}, connection_accounts::{self as accounts, Import, Imported, Intent, Lasting}, connection_signin::{self, Account}, own_signin, roundtrip};
use fragment_proto::{ErrorCode, IdentityKind};
use serde::Deserialize;
use worker::SqlStorageValue;

use super::{RegistryCell, calls::{BeginConnection, ConnectionBegun, ConnectionFences, ConnectionFinished, FenceConnection, FencedAccount, FencedProviders, FinishConnectionIn, SubjectAnswer, SubjectOf}, signin::{fresh_token, sha}};
use crate::{error::{CellError, CellResult}, js, keys};

/// How often a callback waiting for another's decision at the same
/// account asks again (`RegistryCell::turn`).
const TURN_POLL_MS: u64 = 250;

/// A callback's turn deciding at an account of several (`connection_accounts::
/// Deciding`), given back however its finish ends, an error or a dropped
/// call included: with its import's outcome unknown (`uncertain`, set
/// before the import and cleared at a known answer), held a while longer.
struct Turn<'a> {
    deciding: &'a std::cell::RefCell<accounts::Deciding>,
    id: u64,
    uncertain: std::cell::Cell<Option<i64>>,
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        self.deciding.borrow_mut().release(self.id, self.uncertain.get());
    }
}

#[derive(Deserialize)]
struct Pending {
    issuer: String,
    subject: String,
    config: String,
    sealed: String,
    expires_at: i64,
    intent: String,
    account: Option<String>,
}

impl RegistryCell {
    fn connection_oauth(&self, provider: &str) -> CellResult<&ConnectionOAuth> {
        self.cfg.providers.get(provider).filter(|p| p.kind == Kind::Connection).and_then(|p| p.connection_oauth.as_ref())
            .ok_or_else(|| CellError::new(ErrorCode::NotConnected, "browser-bound authorization is not configured for this connection"))
    }

    pub(super) async fn begin_connection(&self, b: BeginConnection) -> CellResult<ConnectionBegun> {
        let o = self.connection_oauth(&b.provider)?;
        if b.intent != Intent::Connect && !self.cfg.providers.get(&b.provider).is_some_and(|p| p.several()) {
            return Err(CellError::invalid(format!("{} connects one account here", b.provider)));
        }
        if !keys::bound(&self.env, &fragment_core::secrets_store::connection_secret(&b.provider)) {
            return Err(CellError::host("the connection's OAuth client secret is not bound"));
        }
        let issuer = keys::workos(&self.env, self.cfg).await?.issuer();
        let state = fresh_token();
        let verifier = own_signin::b64url(&js::random_bytes::<32>());
        let scope = format!("{}:connection:{}", keys::scope("Registry", &self.state), sha(&state));
        let sealed = keys::seal(&self.env, &scope, verifier.as_bytes()).await?;
        self.sweep_by(js::now_ms() + roundtrip::TTL_S * 1000).await?;
        // All checks and the insert are synchronous after the last await:
        // concurrent starts cannot exceed the bound or resurrect a session.
        let live = self.live_session(&b.session, None, false)?;
        if live.session.identity.kind != IdentityKind::Person {
            return Err(CellError::new(ErrorCode::Forbidden, "only a person's platform session begins a connection"));
        }
        let subject = self.subject_of(SubjectOf { identity: live.session.identity.id, issuer: issuer.clone() })?.subject
            .ok_or_else(|| CellError::new(ErrorCode::NotConnected, "sign in through the platform before connecting"))?;
        let now = js::now_ms();
        self.exec("DELETE FROM connection_signins WHERE expires_at <= ?", vec![now.into()])?;
        if self.count("SELECT COUNT(*) AS n FROM connection_signins WHERE session = ?", vec![live.hash.as_str().into()])? >= connection_signin::PENDING_PER_SESSION_MAX
            || self.count("SELECT COUNT(*) AS n FROM connection_signins", vec![])? >= self.cfg.signins_pending_max {
            return Err(CellError::new(ErrorCode::RateLimited, "too many connection authorizations under way; finish one or wait ten minutes"));
        }
        let (intent, account) = b.intent.columns();
        self.exec("INSERT INTO connection_signins (state, session, provider, issuer, subject, config, sealed, expires_at, intent, account) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            vec![sha(&state).into(), live.hash.into(), b.provider.as_str().into(), issuer.into(), subject.into(), sha(&serde_json::to_string(o).expect("OAuth config serializes")).into(), sealed.into(), (now + roundtrip::TTL_S * 1000).into(), intent.into(), account.map_or(SqlStorageValue::Null, |a| a.into())])?;
        Ok(ConnectionBegun { state, challenge: own_signin::challenge(&verifier) })
    }

    pub(super) async fn finish_connection(&self, b: FinishConnectionIn) -> CellResult<ConnectionFinished> {
        use ConnectionFinished::*;
        let o = self.connection_oauth(&b.provider)?;
        let live = self.live_session(&b.session, None, false)?;
        // A different session/provider cannot consume the original browser's
        // pending attempt. A valid attempt is spent before any network await,
        // including a decline. Replay and concurrent callbacks exchange once.
        let pending = self.row::<Pending>(
            connection_signin::TAKE_PENDING,
            vec![sha(&b.state).into(), live.hash.into(), b.provider.as_str().into(), SqlStorageValue::Integer(js::now_ms())])?;
        let Some(pending) = pending else { return Ok(NotUnderWay) };
        // under way from the take, in its turn: a disconnect from here on
        // fences it (`fence_connection`), whatever it answers
        let account = Account { issuer: pending.issuer.clone(), subject: pending.subject.clone(), provider: b.provider.clone() };
        let id = self.finishing.borrow_mut().begin(account);
        let finished = self.finish_taken(b, o, pending, id).await;
        self.finishing.borrow_mut().end(id);
        finished
    }

    /// A callback's attempt, its pending row spent (`finish_connection`):
    /// its code exchanged and its grant imported, unless a disconnect of
    /// its account fenced it meanwhile.
    async fn finish_taken(&self, b: FinishConnectionIn, o: &ConnectionOAuth, pending: Pending, id: u64) -> CellResult<ConnectionFinished> {
        use ConnectionFinished::*;
        let issuer = keys::workos(&self.env, self.cfg).await?.issuer();
        if pending.config != sha(&serde_json::to_string(o).expect("OAuth config serializes")) || pending.issuer != issuer { return Ok(NotUnderWay); }
        let Some(code) = b.code.filter(|c| own_signin::valid_code(c)) else { return Ok(Declined) };
        let scope = format!("{}:connection:{}", keys::scope("Registry", &self.state), sha(&b.state));
        let verifier = keys::open(&self.env, &scope, &pending.sealed).await?;
        let verifier = std::str::from_utf8(&verifier.plaintext).map_err(|_| CellError::host("invalid connection verifier"))?;
        if !own_signin::valid_token(verifier) { return Err(CellError::host("invalid connection verifier")); }
        let callback = format!("{}/api/connections/{}/callback", self.cfg.platform(), b.provider);
        let (status, answer) = keys::connection_exchange(&self.env, &b.provider, o, &callback, &code, verifier).await?;
        if (400..500).contains(&status) { return Ok(CodeRefused); }
        if status != 200 { return Ok(Failed); }
        // a partial grant is imported as granted; one of signing in alone
        // gives the person's agents nothing, and is said so
        let tokens = match connection_signin::tokens(&answer, &o.scopes) {
            Ok(tokens) => tokens,
            Err(connection_signin::Refused::NoScopes) => return Ok(NoScopes),
            Err(connection_signin::Refused::Invalid) => return Ok(Failed),
        };
        // Logout/wipe or subject/config change while the vendor answered is a
        // refusal too. The consumed attempt is never restored on any failure.
        let live = self.live_session(&b.session, None, false)?;
        let subject = self.subject_of(SubjectOf { identity: live.session.identity.id, issuer })?.subject;
        if subject.as_deref() != Some(&pending.subject) || pending.expires_at <= js::now_ms() { return Ok(NotUnderWay); }
        let api = &self.cfg.workos()?.api;
        let label = connection_signin::account_label(&answer, &o.client_id);
        // of several accounts, which import: another, one connected before
        // (the same identifier is never a second), or the first
        let Some(intent) = Intent::of_columns(&pending.intent, pending.account.as_deref()) else { return Ok(NotUnderWay) };
        let several = self.cfg.providers.get(&b.provider).filter(|p| p.several()).map(|p| p.accounts_max());
        let identifier = several.and_then(|_| connection_signin::account_identifier(&answer, &o.client_id));
        // one callback at a time reads the person's list there and imports,
        // so two finishing together neither add one account twice nor both
        // pass `accounts_max`; the second reads the list the first left
        let account = Account { issuer: pending.issuer.clone(), subject: pending.subject.clone(), provider: b.provider.clone() };
        let turn = match several {
            Some(_) => match self.turn(&account, id).await {
                Some(turn) => Some(turn),
                None => return Ok(Finishing),
            },
            None => None,
        };
        let import = match several {
            Some(max) => {
                let listed = accounts::listed(&keys::pipes_accounts(&self.env, api, &pending.subject).await?, &b.provider).map_err(|e| CellError::new(ErrorCode::UpstreamFailed, e))?;
                match accounts::import_of(&intent, &listed, identifier.as_deref(), max) {
                    Ok(import) => Some(import),
                    Err(accounts::Full) => return Ok(AccountLimit),
                }
            }
            // begun as another account, and the row takes one now
            None if intent != Intent::Connect => return Ok(NotUnderWay),
            None => None,
        };
        // the last check before the import, in its turn: a disconnect that
        // fenced the account while the code was exchanged cancels it (its
        // tokens are dropped, never revoked: `Finishing`), and one from
        // here on is refused until this answers
        if !self.finishing.borrow_mut().may_import(id) { return Ok(NotUnderWay); }
        // An import that fails, or whose outcome is unknown (a timeout, a
        // 5xx), leaves a disconnect's lasting fence standing: it may yet
        // land at Pipes, and is never taken for a connection. Of several,
        // one whose outcome is unknown keeps its turn a while (no answer, or
        // an error, keeps it too), so no other decides on a list that lacks it.
        if let Some(t) = &turn {
            t.uncertain.set(Some(js::now_ms() + accounts::UNCERTAIN_HOLD_MS));
        }
        let imported = keys::pipes_import(&self.env, api, &b.provider, &pending.subject, tokens, label.as_deref(), import.as_ref().map(|i: &Import| (i, identifier.as_deref()))).await?;
        if let Some(t) = turn.as_ref().filter(|_| imported != Imported::Failed) {
            t.uncertain.set(None);
        }
        match imported {
            Imported::Done => {}
            Imported::Full => return Ok(AccountLimit),
            Imported::Unavailable => return Ok(OneAccount),
            Imported::Conflict | Imported::Refused | Imported::Failed => return Ok(Failed),
        }
        // known to have imported, in the turn it answered and before it is
        // no longer under way, so no disconnect comes between: the fence of
        // the provider goes, and of several, that account's
        let row = || -> Vec<SqlStorageValue> { vec![pending.issuer.as_str().into(), pending.subject.as_str().into(), b.provider.as_str().into()] };
        self.exec(connection_signin::UNFENCE, row())?;
        if let (Some(_), Some(identifier)) = (&import, &identifier) {
            self.exec(connection_signin::UNFENCE_ACCOUNT, [row(), vec![identifier.as_str().into()]].concat())?;
        }
        Ok(Connected)
    }

    /// Callback `id`'s turn deciding at `account`: at once when no other
    /// callback decides there, else once it answers (asked again each
    /// `TURN_POLL_MS`, up to `connection_accounts::DECIDE_WAIT_MS`), since
    /// its person's code is spent already. None: refused, while an import
    /// there has no known outcome, or past the wait.
    async fn turn(&self, account: &Account, id: u64) -> Option<Turn<'_>> {
        let until = js::now_ms() + accounts::DECIDE_WAIT_MS;
        loop {
            match self.deciding.borrow_mut().take(account, id, js::now_ms()) {
                accounts::Turn::Taken => return Some(Turn { deciding: &self.deciding, id, uncertain: std::cell::Cell::new(None) }),
                accounts::Turn::Uncertain => return None,
                accounts::Turn::Busy => {}
            }
            if js::now_ms() >= until {
                return None;
            }
            worker::Delay::from(std::time::Duration::from_millis(TURN_POLL_MS)).await;
        }
    }

    /// A disconnect's fence (`calls::FenceConnection`), in one turn, before
    /// it revokes or deletes anything: each attempt at the person's account
    /// still pending ended (in every session of theirs), and each callback
    /// still exchanging its code cancelled, so no attempt begun before the
    /// disconnect connects the account after it. A callback importing now
    /// refuses it (429) and nothing is ended: what it sent Pipes cannot be
    /// called back, and the disconnect, asked again once it answered, then
    /// deletes it. And the account stays fenced (`connection_signin::FENCE`)
    /// until a connect is known to import again, so an import whose outcome
    /// was unknown, landing at Pipes after this disconnect deleted the
    /// account, is never read or swapped as a connection, even across a
    /// restart. Of several accounts, only the one disconnected stays fenced
    /// so (`connection_signin::FENCE_ACCOUNT`), by its identifier, and the
    /// person's others there stay connected (one that names none, nothing,
    /// while others are there: `connection_accounts::lasting`); what is
    /// pending or exchanging is ended at the whole provider, as with one. Answers the person's
    /// WorkOS user (none: no account, and nothing under way, since a
    /// connect begins only with one).
    pub(super) fn fence_connection(&self, b: FenceConnection) -> CellResult<SubjectAnswer> {
        let answer = self.subject_of(SubjectOf { identity: b.identity, issuer: b.issuer.clone() })?;
        let Some(subject) = answer.subject.clone() else { return Ok(answer) };
        let account = Account { issuer: b.issuer, subject, provider: b.provider };
        self.finishing.borrow_mut().fence(&account).map_err(|connection_signin::Importing| {
            CellError::new(ErrorCode::RateLimited, format!("a connect of your {} account is finishing now: disconnect again in a moment", account.provider))
        })?;
        let row = || -> Vec<SqlStorageValue> { vec![account.issuer.as_str().into(), account.subject.as_str().into(), account.provider.as_str().into()] };
        self.exec(connection_signin::FENCE_PENDING, row())?;
        let now = SqlStorageValue::Integer(js::now_ms());
        match &b.lasting {
            Lasting::Account { identifier, account } => {
                self.exec(connection_signin::FENCE_ACCOUNT, [row(), vec![identifier.as_str().into(), account.as_str().into(), now]].concat())?;
                self.exec(connection_signin::TRIM_ACCOUNT_FENCES, row())?;
            }
            Lasting::Provider => self.exec(connection_signin::FENCE, [row(), vec![now]].concat())?,
            Lasting::Nothing => {}
        }
        Ok(answer)
    }

    /// The providers an account stands fenced at (`calls::ConnectionFences`).
    pub(super) fn connection_fences(&self, b: ConnectionFences) -> CellResult<FencedProviders> {
        #[derive(Deserialize)]
        struct Row {
            provider: String,
        }
        let rows = self.rows::<Row>(connection_signin::FENCED, vec![b.issuer.as_str().into(), b.subject.as_str().into()])?;
        let accounts = self.rows::<FencedAccount>(connection_signin::FENCED_ACCOUNTS, vec![b.issuer.into(), b.subject.into()])?;
        Ok(FencedProviders { providers: rows.into_iter().map(|r| r.provider).collect(), accounts })
    }
}
