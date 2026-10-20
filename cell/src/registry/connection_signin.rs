//! A connection becomes usable at Pipes only after the initiating browser,
//! live platform session, provider and one-use pending state are verified.
use fragment_core::{catalog::{ConnectionOAuth, Kind}, connection_signin, own_signin, roundtrip};
use fragment_proto::{ErrorCode, IdentityKind};
use serde::Deserialize;
use worker::SqlStorageValue;

use super::{RegistryCell, calls::{BeginConnection, ConnectionBegun, ConnectionFinished, FinishConnectionIn, SubjectOf}, signin::{fresh_token, sha}};
use crate::{error::{CellError, CellResult}, js, keys};

#[derive(Deserialize)]
struct Pending {
    issuer: String,
    subject: String,
    config: String,
    sealed: String,
    expires_at: i64,
}

impl RegistryCell {
    fn connection_oauth(&self, provider: &str) -> CellResult<&ConnectionOAuth> {
        self.cfg.providers.get(provider).filter(|p| p.kind == Kind::Connection).and_then(|p| p.connection_oauth.as_ref())
            .ok_or_else(|| CellError::new(ErrorCode::NotConnected, "browser-bound authorization is not configured for this connection"))
    }

    pub(super) async fn begin_connection(&self, b: BeginConnection) -> CellResult<ConnectionBegun> {
        let o = self.connection_oauth(&b.provider)?;
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
        self.exec("INSERT INTO connection_signins (state, session, provider, issuer, subject, config, sealed, expires_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            vec![sha(&state).into(), live.hash.into(), b.provider.into(), issuer.into(), subject.into(), sha(&serde_json::to_string(o).expect("OAuth config serializes")).into(), sealed.into(), (now + roundtrip::TTL_S * 1000).into()])?;
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
        let Some(tokens) = (status == 200).then(|| connection_signin::tokens(&answer, &o.scopes)).flatten() else { return Ok(Failed) };
        // Logout/wipe or subject/config change while the vendor answered is a
        // refusal too. The consumed attempt is never restored on any failure.
        let live = self.live_session(&b.session, None, false)?;
        let subject = self.subject_of(SubjectOf { identity: live.session.identity.id, issuer })?.subject;
        if subject.as_deref() != Some(&pending.subject) || pending.expires_at <= js::now_ms() { return Ok(NotUnderWay); }
        if keys::pipes_import(&self.env, &self.cfg.workos()?.api, &b.provider, &pending.subject, tokens).await? { Ok(Connected) } else { Ok(Failed) }
    }
}
