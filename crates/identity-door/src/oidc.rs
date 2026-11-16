//! OpenID Connect's rules, with no I/O: the door's half of a sign-in at
//! the provider its configuration names (on a tailnet, Tailscale's tsidp;
//! on an intranet, the org's own: Entra, Okta, Google Workspace, Keycloak,
//! Authentik, Dex). Brought over from fragment#1244, which put them in the
//! cell (`fragment_core::oidc`); here they are the door's alone, and the
//! cell has none of them. `provider.rs` fetches; `jws.rs` checks an
//! id_token's signature and reads its claims; these functions decide.
//!
//! - **Discovery** (OpenID Connect Discovery 1.0): `<issuer>/.well-known/
//!   openid-configuration`, its `issuer` exactly the configured one (4.3),
//!   its endpoints https (http only beside an issuer on loopback: dev and
//!   the e2e's fake).
//! - **The request**: an authorization code with PKCE (S256, RFC 7636:
//!   always sent, since tsidp assumes `plain` without it), the door's
//!   state (`state.rs`), and a nonce the id_token must carry back. The
//!   verifier and the nonce are derived from the door's secret and its
//!   state (`SignInSecrets`): nothing of them is stored, a door restarted
//!   mid sign-in derives the same, and only the door can make them.
//! - **The token request**: form-encoded (RFC 6749 4.1.3), the client
//!   authenticated with `client_secret_basic` (2.3.1: id and secret each
//!   form-encoded before base64) or `client_secret_post`.
//! - **The id_token**: signed RS256 or ES256 alone (`Alg`: never `none`,
//!   never an HMAC), by a key of the provider's JWKS (`jws.rs`); its claims
//!   checked (`verified`): `iss`, `aud` (with `azp`), `exp`, `iat` and `nbf`
//!   within `SKEW_S`, `nonce`, and `sub` (OpenID Connect Core 1.0, 3.1.3.7).
//! - **Caches**: the metadata and the JWKS are kept `METADATA_TTL_MS`; the
//!   JWKS is fetched again for a key it does not hold, at once, then not
//!   again for such a key within `JWKS_REFETCH_MIN_MS`.

use base64::Engine;
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

/// The largest id_token taken (Entra's, with groups, run to a few KiB).
pub const ID_TOKEN_MAX_BYTES: usize = 16 * 1024;
/// The largest discovery document, JWKS, or token answer read.
pub const METADATA_MAX_BYTES: usize = 64 * 1024;
/// The clock skew allowed between the provider and the door.
pub const SKEW_S: i64 = 120;
/// How old an id_token may be when it arrives: it is minted at the token
/// request, moments before, and a sign-in lasts ten minutes at most.
pub const ID_TOKEN_AGE_MAX_S: i64 = 15 * 60;
/// A subject's length (OpenID Connect Core 1.0, 2: at most 255 ASCII).
pub const SUBJECT_MAX: usize = 255;
/// The longest authorization code taken (Entra's run to a couple of KiB).
pub const CODE_MAX: usize = 4096;
/// How long the provider's metadata and keys are kept.
pub const METADATA_TTL_MS: i64 = 60 * 60 * 1000;
/// A fetch of the provider's metadata, keys or token, answered whole within this.
pub const FETCH_TIMEOUT_MS: i64 = 15 * 1000;
/// The shortest wait between two fetches of the JWKS for a key it lacks.
/// Only the provider's own token endpoint hands the door an id_token (for
/// a code and a verifier), so a `kid` the door lacks is the provider's
/// doing: a rotation is picked up at the first token its new key signs,
/// and a provider naming keys it never published is asked again at most
/// four times a minute.
pub const JWKS_REFETCH_MIN_MS: i64 = 15 * 1000;
/// The scopes asked for when the config names none.
pub const DEFAULT_SCOPES: &str = "openid email profile";
const SCOPES_MAX: usize = 512;
/// A claim's name in `provider.email_claim`.
const CLAIM_NAME_MAX: usize = 64;
/// A setting's value, or a provider's word, kept in an error, at most.
pub(crate) const SAID_MAX: usize = 200;
/// The claims the rules read themselves: none of them is an email.
const RESERVED_CLAIMS: [&str; 9] = ["iss", "sub", "aud", "azp", "exp", "iat", "nbf", "nonce", "email_verified"];
/// What a sign-in's verifier and nonce are derived from the door's secret
/// with (`SignInSecrets`).
const DERIVE_SALT: &[u8] = b"fragment identity door oidc sign-in";
const DERIVE_INFO: &[u8] = b"v1";
/// The door's secret at least (`SignInSecrets`, `state.rs`).
pub const SECRET_MIN_BYTES: usize = 32;

/// The fields of a provider's metadata the door reads, in this order
/// (`Provider::from_fields`).
pub const DISCOVERY_FIELDS: [&str; 7] = [
    "issuer",
    "authorization_endpoint",
    "token_endpoint",
    "jwks_uri",
    "id_token_signing_alg_values_supported",
    "token_endpoint_auth_methods_supported",
    "code_challenge_methods_supported",
];

const _: () = assert!(SKEW_S > 0 && JWKS_REFETCH_MIN_MS < METADATA_TTL_MS && FETCH_TIMEOUT_MS > 0 && ID_TOKEN_AGE_MAX_S > SKEW_S && ID_TOKEN_MAX_BYTES < METADATA_MAX_BYTES);

pub(crate) fn cut(s: &str) -> String {
    s.chars().take(SAID_MAX).collect()
}

/// base64url, no padding (JOSE's, and PKCE's).
pub fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// PKCE's S256 challenge of a verifier (RFC 7636, 4.2).
pub fn challenge(verifier: &str) -> String {
    b64url(&Sha256::digest(verifier.as_bytes()))
}

/// A verifier RFC 7636 takes: 43 to 128 unreserved characters.
fn valid_verifier(v: &str) -> bool {
    (43..=128).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
}

/// A value read from a provider's JSON (`jws::fields_of`): a string, a
/// number, a boolean, a list of strings, absent, or something else, which
/// the rules refuse where they read that field.
#[derive(Debug, Clone, PartialEq)]
pub enum Field {
    Absent,
    Other,
    Str(String),
    Num(f64),
    Bool(bool),
    List(Vec<String>),
}

/// What a provider, its answer, or the door's settings got wrong.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OidcError {
    /// A setting the config gave is not usable (it names which, and why).
    #[error("the sign-in settings: {0}")]
    Config(String),
    /// The provider's metadata, keys or answer are not what the specs say.
    #[error("the sign-in provider: {0}")]
    Provider(String),
    /// The token endpoint refused the code (its `error` and description).
    #[error("the sign-in provider refused the code ({status} {error}): {description}")]
    Refused { status: u16, error: String, description: String },
}

fn provider_err(why: impl Into<String>) -> OidcError {
    OidcError::Provider(why.into())
}

/// Whether `host` is this machine's own (a dev provider may be plain http there).
fn loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "[::1]") || host.ends_with(".localhost")
}

/// Where an issuer's metadata is: its URL less a trailing `/`, then
/// `/.well-known/openid-configuration` (Discovery 1.0, 4).
pub fn discovery_url(issuer: &str) -> String {
    format!("{}/.well-known/openid-configuration", issuer.trim_end_matches('/'))
}

/// How the door proves itself at the token endpoint. A secret is always
/// held: tsidp takes no public client, and the door is a confidential one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientAuth {
    /// HTTP Basic, the id and secret form-encoded first (RFC 6749, 2.3.1).
    Basic,
    /// The id and secret in the form.
    Post,
}

impl ClientAuth {
    /// `provider.auth`, by its registered name.
    pub fn parse(s: &str) -> Result<ClientAuth, OidcError> {
        match s {
            "client_secret_basic" => Ok(ClientAuth::Basic),
            "client_secret_post" => Ok(ClientAuth::Post),
            other => Err(OidcError::Config(format!("provider.auth is client_secret_basic or client_secret_post, not {:?}", cut(other)))),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ClientAuth::Basic => "client_secret_basic",
            ClientAuth::Post => "client_secret_post",
        }
    }
}

/// The provider settings of the door's config, checked. The client's
/// secret is not here: `config.rs` reads it from its file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// An https URL (http only on loopback), kept as given, since `iss` is
    /// compared to it exactly.
    pub issuer: String,
    pub client_id: String,
    /// `DEFAULT_SCOPES` unless named, `openid` among them.
    pub scopes: String,
    /// The claim that is the person's email (default `email`).
    pub email_claim: String,
    /// Unset: chosen from what the provider lists (`Provider::client_auth`).
    pub auth: Option<ClientAuth>,
}

/// The provider settings as the config gives them (each `None` when unset).
#[derive(Debug, Default, Clone, Copy)]
pub struct Vars<'a> {
    pub issuer: &'a str,
    pub client_id: &'a str,
    pub scopes: Option<&'a str>,
    pub email_claim: Option<&'a str>,
    pub auth: Option<&'a str>,
    /// Whether the config vouches for any email domain (`person::Rules`):
    /// an email claim other than `email` is taken only so, since
    /// `email_verified` speaks for `email` alone.
    pub vouches: bool,
}

impl Settings {
    pub fn parse(vars: Vars<'_>) -> Result<Settings, OidcError> {
        check_issuer(vars.issuer)?;
        let client_ok = !vars.client_id.is_empty() && vars.client_id.len() <= 256 && vars.client_id.bytes().all(|b| b.is_ascii_graphic());
        if !client_ok {
            return Err(OidcError::Config("provider.client_id names the client: 1 to 256 printable characters".into()));
        }
        let scopes = vars.scopes.unwrap_or(DEFAULT_SCOPES).split_whitespace().collect::<Vec<_>>().join(" ");
        let scopes_ok = scopes.len() <= SCOPES_MAX && scopes.split(' ').any(|x| x == "openid") && scopes.bytes().all(|b| b == b' ' || (b.is_ascii_graphic() && b != b'"' && b != b'\\'));
        if !scopes_ok {
            return Err(OidcError::Config(format!("provider.scopes is space-separated scopes, `openid` among them, at most {SCOPES_MAX} bytes")));
        }
        let email_claim = vars.email_claim.unwrap_or("email");
        if email_claim.is_empty() || email_claim.len() > CLAIM_NAME_MAX || RESERVED_CLAIMS.contains(&email_claim) {
            return Err(OidcError::Config(format!("provider.email_claim names a claim of 1 to {CLAIM_NAME_MAX} bytes that is not one of {RESERVED_CLAIMS:?}")));
        }
        if email_claim != "email" && !vars.vouches {
            return Err(OidcError::Config(format!(
                "provider.email_claim is {email_claim:?}, but email_verified speaks for `email` alone: name the domains the door vouches for (vouched_domains)"
            )));
        }
        let auth = vars.auth.map(ClientAuth::parse).transpose()?;
        Ok(Settings { issuer: vars.issuer.to_string(), client_id: vars.client_id.to_string(), scopes, email_claim: email_claim.to_string(), auth })
    }
}

/// An issuer the config may name: an https URL with no query, fragment or
/// credentials, or http on loopback (a dev provider, the fake).
pub fn check_issuer(issuer: &str) -> Result<(), OidcError> {
    let bad = |why: &str| Err(OidcError::Config(format!("provider.issuer {:?} {why}", cut(issuer))));
    let Ok(u) = url::Url::parse(issuer) else { return bad("is not a URL") };
    if u.query().is_some() || u.fragment().is_some() || !u.username().is_empty() || u.password().is_some() {
        return bad("has a query, a fragment, or credentials");
    }
    match (u.scheme(), u.host_str()) {
        ("https", Some(_)) => Ok(()),
        ("http", Some(h)) if loopback(h) => Ok(()),
        _ => bad("is not https (http only on loopback)"),
    }
}

/// A signature algorithm sign-in takes. Nothing else is: not `none`, not
/// an HMAC (whose key would be the client secret, or a public key misread
/// as one); and no key the token carries is taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alg {
    Rs256,
    Es256,
}

impl Alg {
    pub fn parse(s: &str) -> Option<Alg> {
        match s {
            "RS256" => Some(Alg::Rs256),
            "ES256" => Some(Alg::Es256),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Alg::Rs256 => "RS256",
            Alg::Es256 => "ES256",
        }
    }
}

/// A provider's metadata (Discovery 1.0, 3), as much as sign-in reads,
/// checked (`Provider::from_fields`).
#[derive(Debug, Clone)]
pub struct Provider {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
    /// The algorithms both it and the rules allow (RS256, ES256).
    pub algs: Vec<Alg>,
    /// Its token endpoint's client authentication methods (empty: it
    /// lists none, so `client_secret_basic`, the default).
    auth_methods: Vec<String>,
}

/// A list of strings, or none (`Absent`); anything else is the provider's fault.
fn list(f: Field, name: &str) -> Result<Option<Vec<String>>, OidcError> {
    match f {
        Field::Absent => Ok(None),
        Field::List(l) => Ok(Some(l)),
        _ => Err(provider_err(format!("its {name} is not a list of strings"))),
    }
}

impl Provider {
    /// The metadata `issuer` answered (`DISCOVERY_FIELDS`, in order),
    /// checked: its `issuer` is exactly the configured one (Discovery 1.0,
    /// 4.3: else another provider could stand in), its endpoints are https
    /// URLs (http only beside a loopback issuer), it signs id_tokens with an
    /// algorithm the rules allow, and it does not refuse PKCE's S256.
    pub fn from_fields(issuer: &str, fields: Vec<Field>) -> Result<Provider, OidcError> {
        let [iss, authorization, token, jwks, algs, methods, pkce]: [Field; 7] =
            fields.try_into().map_err(|_| provider_err("its metadata was read without its fields"))?;
        if iss != Field::Str(issuer.to_string()) {
            return Err(provider_err(format!("its metadata names another issuer than {issuer:?} (they must match exactly)")));
        }
        let dev = issuer.starts_with("http:");
        let endpoint = |name: &str, f: Field| -> Result<String, OidcError> {
            let Field::Str(raw) = f else { return Err(provider_err(format!("its metadata has no {name}"))) };
            let ok = url::Url::parse(&raw).is_ok_and(|u| {
                u.fragment().is_none()
                    && match u.scheme() {
                        "https" => true,
                        "http" => dev && u.host_str().is_some_and(loopback),
                        _ => false,
                    }
            });
            match ok {
                true => Ok(raw),
                false => Err(provider_err(format!("its {name} {:?} is not an https URL", cut(&raw)))),
            }
        };
        let algs: Vec<Alg> = match list(algs, DISCOVERY_FIELDS[4])? {
            Some(listed) => listed.iter().filter_map(|a| Alg::parse(a)).collect(),
            // REQUIRED in the metadata; RS256 is every provider's default (Core 1.0, 15.1)
            None => vec![Alg::Rs256],
        };
        if algs.is_empty() {
            return Err(provider_err("it signs id_tokens with neither RS256 nor ES256"));
        }
        if list(pkce, DISCOVERY_FIELDS[6])?.is_some_and(|m| !m.iter().any(|m| m == "S256")) {
            return Err(provider_err("it lists PKCE methods without S256"));
        }
        Ok(Provider {
            authorization_endpoint: endpoint(DISCOVERY_FIELDS[1], authorization)?,
            token_endpoint: endpoint(DISCOVERY_FIELDS[2], token)?,
            jwks_uri: endpoint(DISCOVERY_FIELDS[3], jwks)?,
            algs,
            auth_methods: list(methods, DISCOVERY_FIELDS[5])?.unwrap_or_default(),
        })
    }

    /// How the door authenticates: as configured, else the first of Basic
    /// and Post the provider lists (Basic when it lists none, the spec's
    /// default).
    pub fn client_auth(&self, configured: Option<ClientAuth>) -> Result<ClientAuth, OidcError> {
        if let Some(c) = configured {
            return Ok(c);
        }
        let listed = &self.auth_methods;
        let takes = |a: ClientAuth| listed.is_empty() && a == ClientAuth::Basic || listed.iter().any(|l| l == a.name());
        [ClientAuth::Basic, ClientAuth::Post]
            .into_iter()
            .find(|a| takes(*a))
            .ok_or_else(|| provider_err(format!("its token endpoint takes neither client_secret_basic nor client_secret_post (it lists {listed:?})")))
    }
}

/// A sign-in's PKCE verifier and nonce, derived from the door's secret and
/// its state (`state.rs`'s sealed one): HKDF-SHA256 of the secret under a
/// label of its own, then an HMAC of the state for each. Nothing of them is
/// stored; the browser carries the state, and only the door can make the
/// verifier from it. A sign-in that spans a change of the door's secret
/// fails (its state no longer opens), and the person starts again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInSecrets {
    /// 43 base64url characters: RFC 7636's least.
    pub verifier: String,
    pub nonce: String,
}

impl SignInSecrets {
    pub fn of(secret: &[u8], state: &str) -> SignInSecrets {
        assert!(secret.len() >= SECRET_MIN_BYTES, "the door's secret is checked before a sign-in derives from it");
        assert!(!state.is_empty(), "a sign-in's secrets are its state's");
        let mut key = [0u8; 32];
        Hkdf::<Sha256>::new(Some(DERIVE_SALT), secret).expand(DERIVE_INFO, &mut key).expect("32 bytes is a valid HKDF-SHA256 length");
        let mac = |label: &[u8]| {
            let mut m = <Hmac<Sha256> as Mac>::new_from_slice(&key).expect("HMAC takes any key");
            m.update(label);
            m.update(state.as_bytes());
            b64url(&m.finalize().into_bytes())
        };
        let secrets = SignInSecrets { verifier: mac(b"verifier\0"), nonce: mac(b"nonce\0") };
        assert!(valid_verifier(&secrets.verifier) && secrets.verifier != secrets.nonce, "a verifier is 43 base64url characters, and no nonce");
        secrets
    }
}

/// The authorization request (Core 1.0, 3.1.2.1, with PKCE).
pub struct Authorize<'a> {
    pub endpoint: &'a str,
    pub settings: &'a Settings,
    pub redirect_uri: &'a str,
    pub state: &'a str,
    pub secrets: &'a SignInSecrets,
    /// Who the person says they are, passed on (Core 1.0's `login_hint`;
    /// tsidp ignores it).
    pub login_hint: Option<&'a str>,
}

/// Where the browser goes to sign in: the provider's endpoint, its own
/// query kept, with the request's parameters added.
pub fn authorize_url(a: &Authorize<'_>) -> String {
    assert!(!a.state.is_empty() && !a.secrets.nonce.is_empty(), "a sign-in carries a state and a nonce");
    let mut u = url::Url::parse(a.endpoint).expect("Provider::from_fields checked the endpoint");
    {
        let mut q = u.query_pairs_mut();
        q.append_pair("response_type", "code")
            .append_pair("client_id", &a.settings.client_id)
            .append_pair("redirect_uri", a.redirect_uri)
            .append_pair("scope", &a.settings.scopes)
            .append_pair("state", a.state)
            .append_pair("nonce", &a.secrets.nonce)
            .append_pair("code_challenge", &challenge(&a.secrets.verifier))
            .append_pair("code_challenge_method", "S256");
        if let Some(hint) = a.login_hint {
            q.append_pair("login_hint", hint);
        }
    }
    u.into()
}

fn form_enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// A token request, ready to POST as `application/x-www-form-urlencoded`.
#[derive(Debug, PartialEq, Eq)]
pub struct TokenRequest {
    pub body: String,
    /// The `Authorization` header (`client_secret_basic`'s).
    pub authorization: Option<String>,
}

/// The code exchanged (RFC 6749, 4.1.3, with PKCE's verifier).
pub fn token_request(auth: ClientAuth, client_id: &str, secret: &str, code: &str, redirect_uri: &str, verifier: &str) -> TokenRequest {
    assert!(valid_verifier(verifier), "the verifier is the one the challenge was made from");
    assert!(!secret.is_empty(), "the door is a confidential client");
    let mut body = url::form_urlencoded::Serializer::new(String::new());
    body.append_pair("grant_type", "authorization_code").append_pair("code", code).append_pair("redirect_uri", redirect_uri).append_pair("code_verifier", verifier);
    let authorization = match auth {
        ClientAuth::Basic => Some(format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", form_enc(client_id), form_enc(secret))))),
        ClientAuth::Post => {
            body.append_pair("client_id", client_id).append_pair("client_secret", secret);
            None
        }
    };
    TokenRequest { body: body.finish(), authorization }
}

// -------------------------------------------------------------- id_token

/// Why an id_token is not accepted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdTokenError {
    #[error("the id_token is over {ID_TOKEN_MAX_BYTES} bytes")]
    TooLarge,
    #[error("the id_token is malformed: {0}")]
    Malformed(String),
    /// Signed with an algorithm the rules do not allow (`none`, an HMAC,
    /// one the provider did not list).
    #[error("the id_token is signed {0:?}, which sign-in does not take")]
    Alg(String),
    /// Its header names an extension sign-in does not understand.
    #[error("the id_token's header names an extension sign-in does not understand (crit)")]
    Critical,
    /// No key of the provider's JWKS fits it (the `kid`, if it names one),
    /// after the door fetched it again where it may.
    #[error("the id_token is signed by a key {} the provider's JWKS does not hold", .0.as_deref().map_or_else(|| "(none named)".to_string(), |k| format!("({k})")))]
    UnknownKey(Option<String>),
    #[error("the id_token's signature does not verify")]
    Signature,
    /// The key its JWKS holds could not be used (its size, its encoding): why.
    #[error("the id_token's signature could not be checked: {0}")]
    Unchecked(String),
    #[error("the id_token is from another issuer")]
    Issuer,
    #[error("the id_token is for another client")]
    Audience,
    #[error("the id_token was issued to another party (azp)")]
    AuthorizedParty,
    #[error("the id_token has expired")]
    Expired,
    #[error("the id_token is not valid yet (iat or nbf in the future)")]
    NotYetValid,
    #[error("the id_token was issued too long ago")]
    TooOld,
    #[error("the id_token is for another sign-in (its nonce)")]
    Nonce,
    #[error("the id_token names no subject, or one over {SUBJECT_MAX} bytes")]
    Subject,
}

/// The claims read of an id_token, in this order (`Claims`), the email
/// from the claim the settings name.
pub fn claim_names(settings: &Settings) -> [&str; 10] {
    ["iss", "sub", "aud", "azp", "exp", "iat", "nbf", "nonce", settings.email_claim.as_str(), "email_verified"]
}

/// An id_token's claims, their types checked.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Claims {
    pub iss: Option<String>,
    pub sub: Option<String>,
    pub aud: Vec<String>,
    pub azp: Option<String>,
    pub exp: Option<i64>,
    pub iat: Option<i64>,
    pub nbf: Option<i64>,
    pub nonce: Option<String>,
    pub email: Option<String>,
    pub email_verified: Option<bool>,
}

pub(crate) fn malformed(why: &str) -> IdTokenError {
    IdTokenError::Malformed(why.to_string())
}

fn string(f: Field, name: &str) -> Result<Option<String>, IdTokenError> {
    match f {
        Field::Absent => Ok(None),
        Field::Str(s) => Ok(Some(s)),
        _ => Err(malformed(&format!("its {name} is not a string"))),
    }
}

/// A NumericDate (seconds, maybe fractional), floored.
fn seconds(f: Field, name: &str) -> Result<Option<i64>, IdTokenError> {
    match f {
        Field::Absent => Ok(None),
        Field::Num(n) if n.is_finite() && n.abs() < 1e15 => Ok(Some(n.floor() as i64)),
        _ => Err(malformed(&format!("its {name} is not a time"))),
    }
}

/// An id_token's claims (`claim_names`' values), once its signature
/// verified under `alg`: checked again that `alg` is one `algs` allows,
/// then each claim's type.
pub fn claims_of(alg: Alg, values: Vec<Field>, algs: &[Alg]) -> Result<Claims, IdTokenError> {
    if !algs.contains(&alg) {
        return Err(IdTokenError::Alg(alg.name().into()));
    }
    let [iss, sub, aud, azp, exp, iat, nbf, nonce, email, email_verified]: [Field; 10] = values.try_into().map_err(|_| malformed("its claims were read without their fields"))?;
    Ok(Claims {
        iss: string(iss, "iss")?,
        sub: string(sub, "sub")?,
        aud: match aud {
            Field::Absent => vec![],
            Field::Str(a) => vec![a],
            Field::List(l) => l,
            _ => return Err(malformed("its aud is not an audience, or a list of them")),
        },
        azp: string(azp, "azp")?,
        exp: seconds(exp, "exp")?,
        iat: seconds(iat, "iat")?,
        nbf: seconds(nbf, "nbf")?,
        nonce: string(nonce, "nonce")?,
        email: string(email, "email claim")?,
        email_verified: match email_verified {
            Field::Absent => None,
            Field::Bool(b) => Some(b),
            _ => return Err(malformed("its email_verified is not a boolean")),
        },
    })
}

/// What an id_token must say to sign someone in.
pub struct Expect<'a> {
    pub settings: &'a Settings,
    /// The nonce this sign-in sent.
    pub nonce: &'a str,
    pub now_s: i64,
}

/// An id_token that verified: its subject, and the email claim the
/// settings name, with whether the provider says it verified it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    pub subject: String,
    pub email: Option<String>,
    pub email_verified: bool,
}

/// An id_token's claims checked for this sign-in (Core 1.0, 3.1.3.7):
/// `iss` exactly, `aud` this client (and `azp` it when there are several
/// audiences), `exp`, `iat` and `nbf` within `SKEW_S`, an `iat` at most
/// `ID_TOKEN_AGE_MAX_S` old, this sign-in's nonce, and a `sub`.
pub fn verified(c: Claims, expect: &Expect<'_>) -> Result<Verified, IdTokenError> {
    let settings = expect.settings;
    assert!(!settings.issuer.is_empty() && !settings.client_id.is_empty() && !expect.nonce.is_empty(), "a sign-in expects an issuer, a client, and a nonce");
    if c.iss.as_deref() != Some(settings.issuer.as_str()) {
        return Err(IdTokenError::Issuer);
    }
    if !c.aud.contains(&settings.client_id) {
        return Err(IdTokenError::Audience);
    }
    // more audiences than this client: the party it was issued to must be this one
    if c.azp.as_deref().is_some_and(|a| a != settings.client_id) || (c.aud.len() > 1 && c.azp.is_none()) {
        return Err(IdTokenError::AuthorizedParty);
    }
    let now = expect.now_s;
    let exp = c.exp.ok_or_else(|| malformed("it has no exp"))?;
    if exp.saturating_add(SKEW_S) <= now {
        return Err(IdTokenError::Expired);
    }
    let iat = c.iat.ok_or_else(|| malformed("it has no iat"))?;
    if iat > now.saturating_add(SKEW_S) || c.nbf.is_some_and(|n| n > now.saturating_add(SKEW_S)) {
        return Err(IdTokenError::NotYetValid);
    }
    if iat < now.saturating_sub(ID_TOKEN_AGE_MAX_S + SKEW_S) {
        return Err(IdTokenError::TooOld);
    }
    if c.nonce.as_deref() != Some(expect.nonce) {
        return Err(IdTokenError::Nonce);
    }
    let subject = c.sub.filter(|s| !s.is_empty() && s.len() <= SUBJECT_MAX).ok_or(IdTokenError::Subject)?;
    Ok(Verified { subject, email: c.email, email_verified: c.email_verified == Some(true) })
}

/// Why a provider's answer to a sign-in's code signs no one in.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// The token endpoint refused the code; a 400 is the code's: used,
    /// expired, or another sign-in's.
    #[error("{0}")]
    Code(OidcError),
    /// The provider could not be reached, or answered out of shape (its
    /// metadata, its token answer, its keys): its fault, not the person's.
    #[error("the sign-in provider: {0}")]
    Provider(String),
    /// The id_token it gave is not one to trust.
    #[error("the sign-in provider's answer was not one to trust: {0}")]
    IdToken(IdTokenError),
}

#[cfg(test)]
mod tests;
