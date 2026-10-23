//! Sign-in on OpenID Connect, the registry's half (docs/self-host.md,
//! seam 4): a provider's code exchanged with the verifier its sign-in began
//! with, its id_token verified (`crate::oidc`), and the person signed in as
//! WorkOS's sign-in signs them in (`signin.rs`: `person_for`, a session,
//! their invitations met). The sign-in's state is spent before anything is
//! awaited, so a callback sent again, or two raced, finish it once; the
//! provider spends the code too.

use fragment_core::oidc::{self as rules, OidcError};
use fragment_core::own_signin;
use fragment_proto::ErrorCode;
use serde::Deserialize;
use worker::SqlStorageValue;

use super::calls::{Exchanged, OidcExchangeIn};
use super::signin::{email_of, sha, stale_login, PersonKey, LOGIN_TTL_MS, SESSION_TTL_MS};
use super::RegistryCell;
use crate::error::{CellError, CellResult};
use crate::{js, keys};

/// A pending sign-in, spent: where it goes back to, and an OpenID Connect
/// one's verifier (sealed) and nonce.
#[derive(Deserialize)]
struct Pending {
    return_to: String,
    verifier: Option<String>,
    nonce: Option<String>,
}

impl RegistryCell {
    /// What a session's id_token is sealed for: this registry's sign-ins.
    pub(super) fn hint_scope(&self) -> String {
        format!("{}:id_token", keys::scope("Registry", &self.state))
    }

    /// An OpenID Connect provider's code, exchanged with the verifier this
    /// sign-in began with (keys.rs adds the client's secret), its id_token
    /// verified (the provider's keys, the issuer, this client, the times,
    /// this sign-in's nonce), its email checked as the person's handle,
    /// then the sign-in finished: the person `(issuer, sub)` names, made at
    /// their first sign-in (`person_for`, as WorkOS's), a session keeping
    /// the id_token for logout, and their invitations met.
    pub(super) async fn oidc_exchange(&self, b: OidcExchangeIn) -> CellResult<Exchanged> {
        let o = self.cfg.oidc().ok_or_else(|| CellError::host("an OpenID Connect sign-in finished where none is configured (FRAGMENT_OIDC_ISSUER)"))?;
        if b.code.is_empty() || b.code.len() > rules::CODE_MAX {
            return Err(CellError::invalid(format!("a provider's code is 1 to {} bytes", rules::CODE_MAX)));
        }
        // spent first, in this turn: nothing below is done twice for one sign-in
        let pending = self
            .row::<Pending>(rules::TAKE_LOGIN, vec![sha(&b.state).into(), SqlStorageValue::Integer(js::now_ms() - LOGIN_TTL_MS)])?
            .ok_or_else(stale_login)?;
        let (Some(sealed), Some(nonce)) = (pending.verifier, pending.nonce) else {
            return Err(CellError::invalid("this sign-in did not begin at the OpenID Connect provider; start again"));
        };
        let opened = keys::open(&self.env, &self.login_scope(&b.state), &sealed).await?;
        let verifier = String::from_utf8(opened.plaintext).ok().filter(|v| own_signin::valid_token(v)).ok_or_else(|| CellError::host("a sign-in's verifier opened as no verifier"))?;
        let provider = crate::oidc::provider(o).await?;
        let (status, answer) = keys::oidc_token(&self.env, &provider, o, &b.code, &crate::oidc::callback_url(self.cfg), &verifier).await?;
        let id_token = rules::id_token_of(status, &answer).map_err(|e| {
            crate::oidc::refused(o, &e.to_string());
            match e {
                // a 400 is the code's: used (a callback sent again), expired,
                // another sign-in's (its verifier is not this one's), or never
                // the provider's. The browser starts again; no outage.
                OidcError::Refused { status: 400, .. } => CellError::invalid(e.to_string()),
                e => CellError::new(ErrorCode::UpstreamFailed, e.to_string()),
            }
        })?;
        let verified = crate::oidc::verify(o, &provider, &id_token, &nonce).await?;
        let raw = o.claims.email(&verified).map_err(|e| {
            crate::oidc::refused(o, &e.to_string());
            let said = match e {
                rules::EmailRefused::Absent { .. } => "the sign-in provider gave no email for you: ask whoever runs it to share your email with fragment".to_string(),
                rules::EmailRefused::Unverified { email, .. } => format!("the sign-in provider has not verified the email {email:?}: verify it there, then sign in again"),
            };
            CellError::new(ErrorCode::Forbidden, said)
        })?;
        let Some(email) = email_of(raw) else {
            crate::oidc::refused(o, &format!("the id_token's email {raw:?} (`{}`) is no address", o.claims.email));
            return Err(CellError::new(ErrorCode::Forbidden, format!("the sign-in provider gave {raw:?} as your email, which is no address")));
        };
        let key = PersonKey::make(&self.env).await?;
        let hint = keys::seal(&self.env, &self.hint_scope(), id_token.as_bytes()).await?;
        self.sweep_by(js::now_ms() + SESSION_TTL_MS).await?;
        // one turn from here to the session: no await between
        let (id, _) = self.person_for(&o.issuer, &verified.subject, &email, key)?;
        let token = self.new_session(&id, None, None, None, None, js::now_ms() + SESSION_TTL_MS)?;
        self.exec("UPDATE sessions SET id_token = ? WHERE hash = ?", vec![hint.into(), sha(&token).into()])?;
        self.meet_invites(&email, &id).await?;
        self.arm_syncs().await?;
        Ok(Exchanged::SignedIn { token, return_to: pending.return_to })
    }
}
