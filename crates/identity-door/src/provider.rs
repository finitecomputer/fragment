//! The door's calls to its OpenID Connect provider: its metadata, its
//! keys, and a code's redemption, each answered whole within
//! `FETCH_TIMEOUT_MS` and `METADATA_MAX_BYTES`, following no redirect.
//! Over HTTPS (rustls, Mozilla's roots, and the operator's CA when the
//! config names one), or plain HTTP to a loopback issuer alone (the rules'
//! `check_issuer`: dev and the e2e's fake).
//!
//! The metadata and the JWKS are kept `METADATA_TTL_MS`. A token signed by
//! a key the JWKS does not hold fetches it again at once (a rotation), and
//! then not again for such a key within `JWKS_REFETCH_MIN_MS`.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::jws::{self, Jwks};
use crate::oidc::{self, claim_names, cut, Expect, OidcError, Provider, Refusal, Settings, SignInSecrets, Verified, DISCOVERY_FIELDS, METADATA_MAX_BYTES};

const FETCH_TIMEOUT: Duration = Duration::from_millis(oidc::FETCH_TIMEOUT_MS as u64);
const METADATA_TTL: Duration = Duration::from_millis(oidc::METADATA_TTL_MS as u64);
const JWKS_REFETCH_MIN: Duration = Duration::from_millis(oidc::JWKS_REFETCH_MIN_MS as u64);

#[derive(Default)]
struct Cache {
    provider: Option<(Arc<Provider>, Instant)>,
    jwks: Option<(Arc<Jwks>, Instant)>,
    /// When the JWKS was last fetched again for a key it lacked.
    refetched: Option<Instant>,
}

/// Why the door's HTTP client could not be made.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("the provider's CA file is no PEM certificate: {0}")]
    Ca(reqwest::Error),
    #[error("the door's HTTP client: {0}")]
    Build(reqwest::Error),
}

pub struct ProviderClient {
    pub settings: Settings,
    secret: String,
    http: reqwest::Client,
    cache: Mutex<Cache>,
}

impl ProviderClient {
    /// A client of the provider `settings` name, authenticating with
    /// `secret`, trusting `ca_pem` beside Mozilla's roots when given.
    pub fn new(settings: Settings, secret: String, ca_pem: Option<&[u8]>) -> Result<ProviderClient, ClientError> {
        assert!(!secret.is_empty(), "the door is a confidential client");
        let mut builder = reqwest::Client::builder().timeout(FETCH_TIMEOUT).connect_timeout(FETCH_TIMEOUT).redirect(reqwest::redirect::Policy::none()).use_rustls_tls();
        if let Some(pem) = ca_pem {
            builder = builder.add_root_certificate(reqwest::Certificate::from_pem(pem).map_err(ClientError::Ca)?);
        }
        let http = builder.build().map_err(ClientError::Build)?;
        Ok(ProviderClient { settings, secret, http, cache: Mutex::new(Cache::default()) })
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, Cache> {
        self.cache.lock().expect("the provider cache's lock is never poisoned")
    }

    /// An answer's status and body, read whole within `METADATA_MAX_BYTES`.
    async fn send(&self, request: reqwest::RequestBuilder, what: &str) -> Result<(u16, Vec<u8>), String> {
        let mut answer = request.send().await.map_err(|e| format!("{what} did not answer: {e}"))?;
        let status = answer.status().as_u16();
        let mut body = Vec::new();
        // bounded: each chunk adds to `body`, which stops past the limit
        while let Some(chunk) = answer.chunk().await.map_err(|e| format!("{what}'s answer broke off: {e}"))? {
            if body.len() + chunk.len() > METADATA_MAX_BYTES {
                return Err(format!("{what} answered more than {METADATA_MAX_BYTES} bytes"));
            }
            body.extend_from_slice(&chunk);
        }
        Ok((status, body))
    }

    async fn get_json(&self, url: &str, what: &str) -> Result<Vec<u8>, String> {
        let (status, body) = self.send(self.http.get(url).header("accept", "application/json"), what).await?;
        if status != 200 {
            return Err(format!("{what} answered {status}"));
        }
        Ok(body)
    }

    /// The provider's metadata, checked (`Provider::from_fields`).
    pub async fn provider(&self) -> Result<Arc<Provider>, Refusal> {
        if let Some((p, at)) = &self.cache().provider {
            if at.elapsed() < METADATA_TTL {
                return Ok(p.clone());
            }
        }
        let issuer = &self.settings.issuer;
        let body = self.get_json(&oidc::discovery_url(issuer), "its metadata").await.map_err(Refusal::Provider)?;
        let doc: serde_json::Map<String, Value> = serde_json::from_slice(&body).map_err(|e| Refusal::Provider(format!("its metadata is not a JSON object: {e}")))?;
        let provider = Arc::new(Provider::from_fields(issuer, jws::fields_of(&doc, &DISCOVERY_FIELDS)).map_err(|e| Refusal::Provider(e.to_string()))?);
        self.cache().provider = Some((provider.clone(), Instant::now()));
        Ok(provider)
    }

    async fn fetch_jwks(&self, provider: &Provider) -> Result<Arc<Jwks>, Refusal> {
        let body = self.get_json(&provider.jwks_uri, "its JWKS").await.map_err(Refusal::Provider)?;
        let jwks = Arc::new(Jwks::parse(&body).map_err(Refusal::Provider)?);
        self.cache().jwks = Some((jwks.clone(), Instant::now()));
        Ok(jwks)
    }

    /// The provider's keys for a token signed by `kid`: those kept, or
    /// fetched again when they are old, or lack `kid` and the last such
    /// fetch was `JWKS_REFETCH_MIN` ago or more.
    async fn jwks_for(&self, provider: &Provider, kid: Option<&str>) -> Result<Arc<Jwks>, Refusal> {
        let kept = self.cache().jwks.as_ref().filter(|(_, at)| at.elapsed() < METADATA_TTL).map(|(j, _)| j.clone());
        let Some(kept) = kept else { return self.fetch_jwks(provider).await };
        let lacks = kid.is_some_and(|k| !kept.holds(k));
        if !lacks {
            return Ok(kept);
        }
        let may = {
            let mut cache = self.cache();
            let may = cache.refetched.is_none_or(|at| at.elapsed() >= JWKS_REFETCH_MIN);
            if may {
                cache.refetched = Some(Instant::now());
            }
            may
        };
        match may {
            true => self.fetch_jwks(provider).await,
            false => Ok(kept),
        }
    }

    /// The provider's `code`, redeemed for this sign-in (its `secrets`,
    /// sent to `redirect_uri`): the id_token it answers, its signature and
    /// claims verified.
    pub async fn redeem(&self, code: &str, redirect_uri: &str, secrets: &SignInSecrets, now_s: i64) -> Result<Verified, Refusal> {
        let provider = self.provider().await?;
        let auth = provider.client_auth(self.settings.auth).map_err(|e| Refusal::Provider(e.to_string()))?;
        let req = oidc::token_request(auth, &self.settings.client_id, &self.secret, code, redirect_uri, &secrets.verifier);
        let mut request = self.http.post(&provider.token_endpoint).header("content-type", "application/x-www-form-urlencoded").header("accept", "application/json").body(req.body);
        if let Some(authorization) = req.authorization {
            request = request.header("authorization", authorization);
        }
        let (status, body) = self.send(request, "its token endpoint").await.map_err(Refusal::Provider)?;
        let answer: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        if status != 200 {
            let text = |k: &str| answer.get(k).and_then(Value::as_str).map(cut).unwrap_or_default();
            return Err(Refusal::Code(OidcError::Refused { status, error: text("error"), description: text("error_description") }));
        }
        let Some(token) = answer.get("id_token").and_then(Value::as_str) else {
            return Err(Refusal::Provider("its token answer has no id_token (is `openid` among the scopes?)".into()));
        };
        let header = jws::header_of(token, &provider.algs).map_err(Refusal::IdToken)?;
        let jwks = self.jwks_for(&provider, header.kid.as_deref()).await?;
        let (alg, claims) = jws::verify(token, &jwks, &provider.algs).map_err(Refusal::IdToken)?;
        let claims = oidc::claims_of(alg, jws::fields_of(&claims, &claim_names(&self.settings)), &provider.algs).map_err(Refusal::IdToken)?;
        oidc::verified(claims, &Expect { settings: &self.settings, nonce: &secrets.nonce, now_s }).map_err(Refusal::IdToken)
    }
}
