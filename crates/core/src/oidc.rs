//! Sign-in on OpenID Connect (docs/self-host.md, seam 4): the rules, with
//! no I/O. Any provider signs people in where a deployment names one (on a
//! tailnet, Tailscale's tsidp; on an intranet, the org's own: Entra, Okta,
//! Google Workspace, Keycloak, Authentik), in WorkOS's place and never
//! beside it as a fallback. The router (`cell/src/oidc.rs`) fetches the
//! provider's documents; its JavaScript (`cell/oidc.mjs`) parses them,
//! picks the id_token's key and checks its signature with WebCrypto, and
//! hands back flat values (`Field`); these functions decide what they mean.
//!
//! - **Discovery** (OpenID Connect Discovery 1.0): `<issuer>/.well-known/
//!   openid-configuration`, its `issuer` exactly the configured one (4.3),
//!   its endpoints https (http only beside an issuer on loopback: dev).
//! - **The request**: an authorization code with PKCE (S256, RFC 7636), a
//!   state the router binds to the browser (`roundtrip`), and a nonce the
//!   id_token must carry back. The verifier and the nonce are derived from
//!   the host secret and the state (`SignInSecrets`), so nothing of them is
//!   stored, and only the platform can make them.
//! - **The token request**: form-encoded (RFC 6749 4.1.3), the client
//!   authenticated with `client_secret_basic` (2.3.1: id and secret each
//!   form-encoded before base64), `client_secret_post`, or not at all (a
//!   public client, PKCE alone). The cell's keys.rs makes it; the shim
//!   sends it and reads the answer (`redeemed`).
//! - **The id_token**: the shim answers whether its signature verified by
//!   a key of the provider's JWKS, under an algorithm the rules allow
//!   (`Provider::algs`: RS256 and ES256 alone, never `none` or an HMAC),
//!   and its claims; `id_token_claims` checks the algorithm again and the
//!   claims' types, and `verified` the claims: `iss`, `aud` (with `azp`),
//!   `exp`, `iat` and `nbf` within `SKEW_S`, `nonce`, and `sub` (OpenID
//!   Connect Core 1.0, 3.1.3.7).
//! - **Who it is**: `(issuer URL, sub)` names the sign-in, which names a
//!   person (the npub the registry makes at their first sign-in, as for
//!   WorkOS's); the email is their handle, so it must be there and verified
//!   (`email_of`): `email_verified: true`, or a provider the deployment says
//!   vouches for the claim it names.
//! - **Caches**: the shim keeps the metadata and the JWKS per isolate for
//!   at most `METADATA_TTL_MS`, and fetches the JWKS again for a key it
//!   does not hold, at once, then not again for such a key within
//!   `JWKS_REFETCH_MIN_MS`. These limits are passed to it from here.

use base64::Engine;
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;

/// The largest id_token the shim takes (Entra's, with groups, run to a few KiB).
pub const ID_TOKEN_MAX_BYTES: usize = 16 * 1024;
/// The largest discovery document, JWKS, or token answer read.
pub const METADATA_MAX_BYTES: usize = 64 * 1024;
/// The clock skew allowed between the provider and the cell.
pub const SKEW_S: i64 = 120;
/// How old an id_token may be when it arrives: it is minted at the token
/// request, moments before, and a sign-in lasts ten minutes at most.
pub const ID_TOKEN_AGE_MAX_S: i64 = 15 * 60;
/// A subject's length (OpenID Connect Core 1.0, 2: at most 255 ASCII).
pub const SUBJECT_MAX: usize = 255;
/// The longest authorization code taken (Entra's run to a couple of KiB).
pub const CODE_MAX: usize = 4096;
/// How long the shim keeps the provider's metadata and keys (per isolate).
pub const METADATA_TTL_MS: i64 = 60 * 60 * 1000;
/// A fetch of the provider's metadata or keys, answered whole within this.
pub const FETCH_TIMEOUT_MS: i64 = 15 * 1000;
/// The shortest wait between two fetches of the JWKS for a key it lacks.
/// Only the provider's own token endpoint hands the cell an id_token (for
/// a code and a verifier), so a `kid` the cell lacks is the provider's
/// doing: a rotation is picked up at the first token its new key signs,
/// and a provider naming keys it never published is asked again at most
/// four times a minute per isolate.
pub const JWKS_REFETCH_MIN_MS: i64 = 15 * 1000;
/// The scopes asked for when the deployment names none.
pub const DEFAULT_SCOPES: &str = "openid email profile";
const SCOPES_MAX: usize = 512;
/// A claim's name in `FRAGMENT_OIDC_EMAIL_CLAIM`.
const CLAIM_NAME_MAX: usize = 64;
/// A setting's value, or the shim's word, kept in an error, at most.
const SAID_MAX: usize = 200;
/// The claims the rules read themselves: none of them is an email.
const RESERVED_CLAIMS: [&str; 9] = ["iss", "sub", "aud", "azp", "exp", "iat", "nbf", "nonce", "email_verified"];
/// What a sign-in's verifier and nonce are derived from the host secret
/// with (`SignInSecrets`).
const DERIVE_SALT: &[u8] = b"fragment oidc sign-in";
const DERIVE_INFO: &[u8] = b"v1";

/// The fields of a provider's metadata the shim reads, in this order
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

fn cut(s: &str) -> String {
    s.chars().take(SAID_MAX).collect()
}

/// A value the shim read from a provider's JSON (`cell/oidc.mjs`): a
/// string, a number, a boolean, a list of strings, absent, or something
/// else, which the rules refuse where they read that field.
#[derive(Debug, Clone, PartialEq)]
pub enum Field {
    Absent,
    Other,
    Str(String),
    Num(f64),
    Bool(bool),
    List(Vec<String>),
}

/// What a provider, its answer, or a deployment's settings got wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OidcError {
    /// A setting the deployment gave is not usable (it names which, and why).
    Config(String),
    /// The provider's metadata, keys or answer are not what the specs say.
    Provider(String),
    /// The token endpoint refused the code (its `error` and description).
    Refused { status: u16, error: String, description: String },
}

impl std::fmt::Display for OidcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OidcError::Config(why) => write!(f, "the sign-in settings: {why}"),
            OidcError::Provider(why) => write!(f, "the sign-in provider: {why}"),
            OidcError::Refused { status, error, description } => write!(f, "the sign-in provider refused the code ({status} {error}): {description}"),
        }
    }
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

/// How the client proves itself at the token endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientAuth {
    /// HTTP Basic, the id and secret form-encoded first (RFC 6749, 2.3.1).
    Basic,
    /// The id and secret in the form.
    Post,
    /// A public client: its id in the form, PKCE its only proof.
    None,
}

impl ClientAuth {
    /// The deployment's `FRAGMENT_OIDC_AUTH`, by its registered name.
    pub fn parse(s: &str) -> Result<ClientAuth, OidcError> {
        match s {
            "client_secret_basic" => Ok(ClientAuth::Basic),
            "client_secret_post" => Ok(ClientAuth::Post),
            "none" => Ok(ClientAuth::None),
            other => Err(OidcError::Config(format!("FRAGMENT_OIDC_AUTH is client_secret_basic, client_secret_post or none, not {:?}", cut(other)))),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ClientAuth::Basic => "client_secret_basic",
            ClientAuth::Post => "client_secret_post",
            ClientAuth::None => "none",
        }
    }
}

/// A deployment's sign-in settings (its Worker variables), checked. The
/// client's secret is not here: it is a Secrets Store binding
/// (`secrets_store::OIDC_CLIENT_SECRET`), read only by the cell's keys.rs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// `FRAGMENT_OIDC_ISSUER`: an https URL (http only on loopback), kept
    /// as given, since `iss` is compared to it exactly.
    pub issuer: String,
    /// `FRAGMENT_OIDC_CLIENT_ID`.
    pub client_id: String,
    /// `FRAGMENT_OIDC_SCOPES` (default `DEFAULT_SCOPES`), `openid` among them.
    pub scopes: String,
    /// `FRAGMENT_OIDC_EMAIL_CLAIM` (default `email`): the claim that is the
    /// person's email, their handle (decision 45).
    pub email_claim: String,
    /// `FRAGMENT_OIDC_EMAIL_VOUCHED=on`: the provider vouches for every
    /// address it puts in that claim, as an org's own directory does, so
    /// `email_verified` is not asked for. Off, it must be `true`, and the
    /// claim must be `email`, the one `email_verified` speaks for.
    pub vouched: bool,
    /// `FRAGMENT_OIDC_AUTH`; unset, chosen from what the provider lists
    /// (`Provider::client_auth`).
    pub auth: Option<ClientAuth>,
}

/// The deployment's variables, as given (each `None` when unset).
#[derive(Debug, Default, Clone, Copy)]
pub struct Vars<'a> {
    pub issuer: Option<&'a str>,
    pub client_id: Option<&'a str>,
    pub scopes: Option<&'a str>,
    pub email_claim: Option<&'a str>,
    pub vouched: Option<&'a str>,
    pub auth: Option<&'a str>,
}

impl Settings {
    /// The settings `vars` name, or `None` when no issuer is named (WorkOS
    /// signs people in, as on finite.chat), or what is wrong with them.
    pub fn parse(vars: Vars<'_>) -> Result<Option<Settings>, OidcError> {
        let Some(issuer) = vars.issuer else {
            let named = [vars.client_id, vars.scopes, vars.email_claim, vars.vouched, vars.auth].iter().any(Option::is_some);
            return match named {
                true => Err(OidcError::Config("FRAGMENT_OIDC_* is set but FRAGMENT_OIDC_ISSUER is not".into())),
                false => Ok(None),
            };
        };
        check_issuer(issuer)?;
        let client_id = vars.client_id.filter(|c| !c.is_empty() && c.len() <= 256 && c.bytes().all(|b| b.is_ascii_graphic()));
        let client_id = client_id.ok_or_else(|| OidcError::Config("FRAGMENT_OIDC_CLIENT_ID names the client: 1 to 256 printable characters".into()))?;
        let scopes = vars.scopes.unwrap_or(DEFAULT_SCOPES).split_whitespace().collect::<Vec<_>>().join(" ");
        let scopes_ok = scopes.len() <= SCOPES_MAX && scopes.split(' ').any(|x| x == "openid") && scopes.bytes().all(|b| b == b' ' || (b.is_ascii_graphic() && b != b'"' && b != b'\\'));
        if !scopes_ok {
            return Err(OidcError::Config(format!("FRAGMENT_OIDC_SCOPES is space-separated scopes, `openid` among them, at most {SCOPES_MAX} bytes")));
        }
        let email_claim = vars.email_claim.unwrap_or("email");
        if email_claim.is_empty() || email_claim.len() > CLAIM_NAME_MAX || RESERVED_CLAIMS.contains(&email_claim) {
            return Err(OidcError::Config(format!("FRAGMENT_OIDC_EMAIL_CLAIM names a claim of 1 to {CLAIM_NAME_MAX} bytes that is not one of {RESERVED_CLAIMS:?}")));
        }
        let vouched = match vars.vouched {
            None | Some("off") => false,
            Some("on") => true,
            Some(other) => return Err(OidcError::Config(format!("FRAGMENT_OIDC_EMAIL_VOUCHED is on or off, not {:?}", cut(other)))),
        };
        if email_claim != "email" && !vouched {
            return Err(OidcError::Config(format!(
                "FRAGMENT_OIDC_EMAIL_CLAIM is {email_claim:?}, but email_verified speaks for `email` alone: a provider whose `{email_claim}` is its directory's own says so (FRAGMENT_OIDC_EMAIL_VOUCHED=on)"
            )));
        }
        let auth = vars.auth.map(ClientAuth::parse).transpose()?;
        Ok(Some(Settings { issuer: issuer.to_string(), client_id: client_id.to_string(), scopes, email_claim: email_claim.to_string(), vouched, auth }))
    }
}

/// An issuer the deployment may name: an https URL with no query, fragment
/// or credentials, or http on loopback (a dev provider, the fake).
pub fn check_issuer(issuer: &str) -> Result<(), OidcError> {
    let bad = |why: &str| Err(OidcError::Config(format!("FRAGMENT_OIDC_ISSUER {:?} {why}", cut(issuer))));
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
/// as one); and the shim takes no key the token carries.
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
    /// The metadata `issuer` answered, as the shim read it
    /// (`DISCOVERY_FIELDS`, in order), checked: its `issuer` is exactly the
    /// configured one (Discovery 1.0, 4.3: else another provider could
    /// stand in), its endpoints are https URLs (http only beside a loopback
    /// issuer), it signs id_tokens with an algorithm the rules allow, and
    /// it does not refuse PKCE's S256.
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

    /// How the client authenticates: as configured, else, holding a
    /// secret, the first of Basic and Post the provider lists (Basic when it
    /// lists none, the spec's default), and with no secret, not at all.
    pub fn client_auth(&self, configured: Option<ClientAuth>, has_secret: bool) -> Result<ClientAuth, OidcError> {
        let chosen = match configured {
            Some(c) => c,
            None if !has_secret => ClientAuth::None,
            None => {
                let listed = &self.auth_methods;
                let takes = |a: ClientAuth| listed.is_empty() && a == ClientAuth::Basic || listed.iter().any(|l| l == a.name());
                [ClientAuth::Basic, ClientAuth::Post]
                    .into_iter()
                    .find(|a| takes(*a))
                    .ok_or_else(|| provider_err(format!("its token endpoint takes neither client_secret_basic nor client_secret_post (it lists {listed:?})")))?
            }
        };
        if chosen != ClientAuth::None && !has_secret {
            return Err(OidcError::Config(format!("{} needs the client's secret (bound as OIDC_CLIENT_SECRET)", chosen.name())));
        }
        Ok(chosen)
    }
}

/// A sign-in's PKCE verifier and nonce, derived from the host secret and
/// its state (the round trip's, 64 hex, which the registry keeps only as
/// its hash): HKDF-SHA256 of the host secret under a label of its own, then
/// an HMAC of the state for each. Nothing of them is stored; the browser
/// carries the state, and only the platform can make the verifier from it.
/// A sign-in that spans a rotation of the host secret fails at the
/// provider (another verifier), and the person starts again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInSecrets {
    /// 43 base64url characters (`own_signin::valid_token`): RFC 7636's least.
    pub verifier: String,
    pub nonce: String,
}

impl SignInSecrets {
    pub fn of(host_secret: &str, state: &str) -> SignInSecrets {
        assert!(host_secret.len() >= crate::seal::HOST_SECRET_MIN_BYTES, "a host secret is checked before a sign-in derives from it");
        assert!(crate::roundtrip::valid_state(state), "a sign-in's state is a round trip's");
        let mut key = [0u8; 32];
        Hkdf::<Sha256>::new(Some(DERIVE_SALT), host_secret.as_bytes()).expand(DERIVE_INFO, &mut key).expect("32 bytes is a valid HKDF-SHA256 length");
        let mac = |label: &[u8]| {
            let mut m = <Hmac<Sha256> as Mac>::new_from_slice(&key).expect("HMAC takes any key");
            m.update(label);
            m.update(state.as_bytes());
            crate::own_signin::b64url(&m.finalize().into_bytes())
        };
        let secrets = SignInSecrets { verifier: mac(b"verifier\0"), nonce: mac(b"nonce\0") };
        assert!(crate::own_signin::valid_token(&secrets.verifier) && secrets.verifier != secrets.nonce, "a verifier is 43 base64url characters, and no nonce");
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
    /// Who the person says they are, passed on (Core 1.0's `login_hint`).
    pub login_hint: Option<&'a str>,
    /// "Use another account": the provider asks who is signing in, even
    /// holding a session in the browser (Core 1.0's `prompt=login`).
    pub ask_again: bool,
}

/// Where the browser goes to sign in: the provider's endpoint, its own
/// query kept, with the request's parameters added.
pub fn authorize_url(a: &Authorize<'_>) -> String {
    assert!(!a.state.is_empty() && !a.secrets.nonce.is_empty(), "a sign-in carries a state and a nonce");
    let mut u = url::Url::parse(a.endpoint).expect("Provider::parse checked the endpoint");
    {
        let mut q = u.query_pairs_mut();
        q.append_pair("response_type", "code")
            .append_pair("client_id", &a.settings.client_id)
            .append_pair("redirect_uri", a.redirect_uri)
            .append_pair("scope", &a.settings.scopes)
            .append_pair("state", a.state)
            .append_pair("nonce", &a.secrets.nonce)
            .append_pair("code_challenge", &crate::own_signin::challenge(&a.secrets.verifier))
            .append_pair("code_challenge_method", "S256");
        if let Some(hint) = a.login_hint {
            q.append_pair("login_hint", hint);
        }
        if a.ask_again {
            q.append_pair("prompt", "login");
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

/// The code exchanged (RFC 6749, 4.1.3, with PKCE's verifier). `secret` is
/// the client's, which `Provider::client_auth` checked is there when the
/// method needs it.
pub fn token_request(auth: ClientAuth, client_id: &str, secret: Option<&str>, code: &str, redirect_uri: &str, verifier: &str) -> TokenRequest {
    assert!(crate::own_signin::valid_token(verifier), "the verifier is the one the challenge was made from");
    let mut body = url::form_urlencoded::Serializer::new(String::new());
    body.append_pair("grant_type", "authorization_code").append_pair("code", code).append_pair("redirect_uri", redirect_uri).append_pair("code_verifier", verifier);
    let authorization = match (auth, secret) {
        (ClientAuth::Basic, Some(secret)) => Some(format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", form_enc(client_id), form_enc(secret))))),
        (ClientAuth::Post, Some(secret)) => {
            body.append_pair("client_id", client_id).append_pair("client_secret", secret);
            None
        }
        (ClientAuth::None, _) => {
            body.append_pair("client_id", client_id);
            None
        }
        (ClientAuth::Basic | ClientAuth::Post, None) => panic!("{} needs a secret (Provider::client_auth checked)", auth.name()),
    };
    TokenRequest { body: body.finish(), authorization }
}

// -------------------------------------------------------------- id_token

/// Why an id_token is not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdTokenError {
    TooLarge,
    Malformed(String),
    /// Signed with an algorithm the rules do not allow (`none`, an HMAC,
    /// one the provider did not list).
    Alg(String),
    /// Its header names an extension sign-in does not understand.
    Critical,
    /// No key of the provider's JWKS fits it (the `kid`, if it names one),
    /// after the shim fetched it again where it may.
    UnknownKey(Option<String>),
    Signature,
    /// The runtime could not check the signature (it would not import the
    /// key, or its WebCrypto threw): why.
    Unchecked(String),
    Issuer,
    Audience,
    AuthorizedParty,
    Expired,
    NotYetValid,
    TooOld,
    Nonce,
    Subject,
}

impl std::fmt::Display for IdTokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IdTokenError::TooLarge => write!(f, "the id_token is over {ID_TOKEN_MAX_BYTES} bytes"),
            IdTokenError::Malformed(why) => write!(f, "the id_token is malformed: {why}"),
            IdTokenError::Alg(a) => write!(f, "the id_token is signed {a:?}, which sign-in does not take"),
            IdTokenError::Critical => write!(f, "the id_token's header names an extension sign-in does not understand (crit)"),
            IdTokenError::UnknownKey(Some(kid)) => write!(f, "the id_token is signed by a key ({kid}) the provider's JWKS does not hold"),
            IdTokenError::UnknownKey(None) => write!(f, "the id_token names no key, and the provider's JWKS holds no single one for it"),
            IdTokenError::Signature => write!(f, "the id_token's signature does not verify"),
            IdTokenError::Unchecked(why) => write!(f, "the id_token's signature could not be checked: {why}"),
            IdTokenError::Issuer => write!(f, "the id_token is from another issuer"),
            IdTokenError::Audience => write!(f, "the id_token is for another client"),
            IdTokenError::AuthorizedParty => write!(f, "the id_token was issued to another party (azp)"),
            IdTokenError::Expired => write!(f, "the id_token has expired"),
            IdTokenError::NotYetValid => write!(f, "the id_token is not valid yet (iat or nbf in the future)"),
            IdTokenError::TooOld => write!(f, "the id_token was issued too long ago"),
            IdTokenError::Nonce => write!(f, "the id_token is for another sign-in (its nonce)"),
            IdTokenError::Subject => write!(f, "the id_token names no subject, or one over {SUBJECT_MAX} bytes"),
        }
    }
}

/// The claims the shim reads of an id_token, in this order (`Claims`),
/// the email from the claim the settings name.
pub fn claim_names(settings: &Settings) -> [&str; 10] {
    ["iss", "sub", "aud", "azp", "exp", "iat", "nbf", "nonce", settings.email_claim.as_str(), "email_verified"]
}

/// The algorithms a provider's id_token may be signed with, by name, as
/// the shim takes them (it also refuses any but RS256 and ES256 itself).
pub fn alg_names(algs: &[Alg]) -> Vec<&'static str> {
    algs.iter().map(|a| a.name()).collect()
}

/// An id_token's claims, as the shim read them and the rules checked
/// their types.
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

fn malformed(why: &str) -> IdTokenError {
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

/// What the shim said of an id_token (`cell/oidc.mjs`'s `verify`: its
/// code, its detail, and the claims when it verified), read by the rules:
/// the claims, with their types checked, when the signature verified under
/// an algorithm `algs` allows (asked again here: the shim is asked to
/// refuse any other first); else why not.
pub fn id_token_claims(code: &str, detail: String, values: Option<Vec<Field>>, algs: &[Alg]) -> Result<Claims, IdTokenError> {
    let detail = cut(&detail);
    match code {
        "ok" => {}
        "alg" => return Err(IdTokenError::Alg(detail)),
        "crit" => return Err(IdTokenError::Critical),
        "typ" => return Err(malformed("it is an access token (typ at+jwt)")),
        "malformed" => return Err(IdTokenError::Malformed(detail)),
        "too_large" => return Err(IdTokenError::TooLarge),
        "unknown_key" => return Err(IdTokenError::UnknownKey(Some(detail).filter(|k| !k.is_empty()))),
        "signature" => return Err(IdTokenError::Signature),
        "unchecked" => return Err(IdTokenError::Unchecked(detail)),
        other => return Err(IdTokenError::Unchecked(format!("the shim answered {:?}", cut(other)))),
    }
    if !Alg::parse(&detail).is_some_and(|a| algs.contains(&a)) {
        return Err(IdTokenError::Alg(detail));
    }
    let [iss, sub, aud, azp, exp, iat, nbf, nonce, email, email_verified]: [Field; 10] =
        values.and_then(|v| v.try_into().ok()).ok_or_else(|| malformed("its claims were read without their fields"))?;
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

/// The limits the shim works within, as it takes them: the most bytes of
/// a provider's answer, its deadline, how long a document is kept, the
/// least time between two fetches of the JWKS for a key it lacks, and the
/// largest id_token. Defined here alone, and handed to it.
pub const SHIM_LIMITS: [i64; 5] = [METADATA_MAX_BYTES as i64, FETCH_TIMEOUT_MS, METADATA_TTL_MS, JWKS_REFETCH_MIN_MS, ID_TOKEN_MAX_BYTES as i64];

/// Why a provider's answer to a sign-in's code signs no one in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The token endpoint refused the code (`OidcError::Refused`); a 400
    /// is the code's: used, expired, or another sign-in's.
    Code(OidcError),
    /// The provider could not be reached, or answered out of shape (its
    /// token answer, its keys): its fault, not the person's.
    Provider(String),
    /// The id_token it gave is not one to trust.
    IdToken(IdTokenError),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Code(e) => write!(f, "{e}"),
            Refusal::Provider(why) => write!(f, "the sign-in provider: {why}"),
            Refusal::IdToken(e) => write!(f, "the sign-in provider's answer was not one to trust: {e}"),
        }
    }
}

/// What the shim answered of a sign-in's code (`cell/oidc.mjs`'s
/// `redeem`: the token request, its answer, and its id_token checked), read
/// by the rules: the id_token's claims verified for this sign-in
/// (`id_token_claims`, then `verified`), or why not.
pub fn redeemed(code: &str, detail: String, values: Option<Vec<Field>>, algs: &[Alg], expect: &Expect<'_>) -> Result<Verified, Refusal> {
    match code {
        "token_refused" => {
            let [error, description] = values.and_then(|v| <[Field; 2]>::try_from(v).ok()).unwrap_or([Field::Absent, Field::Absent]);
            let text = |f: Field| match f {
                Field::Str(s) => cut(&s),
                _ => String::new(),
            };
            let status = detail.parse().unwrap_or(0);
            Err(Refusal::Code(OidcError::Refused { status, error: text(error), description: text(description) }))
        }
        "token_fetch" | "token_answer" | "fetch" | "jwks" => Err(Refusal::Provider(cut(&detail))),
        _ => verified(id_token_claims(code, detail, values, algs).map_err(Refusal::IdToken)?, expect).map_err(Refusal::IdToken),
    }
}

// ------------------------------------------------------------- the person

/// Why a verified id_token signs no one in: the email it gives is no handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmailRefused {
    /// The claim is absent or empty.
    Absent { claim: String },
    /// `email_verified` is not `true`, and the deployment does not take the
    /// provider's word for it.
    Unverified { claim: String, email: String },
}

impl std::fmt::Display for EmailRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EmailRefused::Absent { claim } => {
                write!(f, "the id_token carries no email in `{claim}` (is `email` among FRAGMENT_OIDC_SCOPES, and does the provider release it?)")
            }
            EmailRefused::Unverified { claim, email } => write!(
                f,
                "the id_token's email {email:?} (`{claim}`) is not verified: email_verified is not true (a provider that vouches for its addresses is configured so: FRAGMENT_OIDC_EMAIL_VOUCHED=on)"
            ),
        }
    }
}

/// The email a verified id_token gives as the person's handle, as the
/// settings read it: present and verified, or vouched for, as given; the
/// registry checks it is an address and keeps it lower case.
pub fn email_of<'v>(v: &'v Verified, settings: &Settings) -> Result<&'v str, EmailRefused> {
    let email = v.email.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let Some(email) = email else { return Err(EmailRefused::Absent { claim: settings.email_claim.clone() }) };
    if settings.vouched || v.email_verified {
        return Ok(email);
    }
    Err(EmailRefused::Unverified { claim: settings.email_claim.clone(), email: email.chars().take(320).collect() })
}

#[cfg(test)]
mod tests;
