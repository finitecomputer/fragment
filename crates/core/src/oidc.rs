//! Sign-in on OpenID Connect (docs/self-host.md, seam 4): the rules, with
//! no I/O. Any provider signs people in where a deployment names one (on a
//! tailnet, Tailscale's tsidp; on an intranet, the org's own: Entra, Okta,
//! Google Workspace, Keycloak, Authentik), in WorkOS's place and never
//! beside it as a fallback. The router (`cell/src/oidc.rs`) fetches the
//! provider's metadata and keys and asks these functions what they mean.
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
//!   public client, PKCE alone).
//! - **The id_token**: a compact JWS signed RS256 or ES256 by a key in the
//!   provider's JWKS (never a key the token names itself, never `none` or an
//!   HMAC). `signed` reads its header and chooses the key; the runtime's
//!   WebCrypto checks the signature; only then `Signed::verified` reads the
//!   claims: `iss`, `aud` (with `azp`), `exp`, `iat` and `nbf` within
//!   `SKEW_S`, `nonce`, and `sub` (OpenID Connect Core 1.0, 3.1.3.7).
//! - **Who it is**: `(issuer URL, sub)` names the sign-in, which names a
//!   person (the npub the registry makes at their first sign-in, as for
//!   WorkOS's); the email is their handle, so it must be there and verified
//!   (`email_of`): `email_verified: true`, or a provider the deployment says
//!   vouches for the claim it names.
//! - **Caches**: the router keeps the metadata and the JWKS per isolate for
//!   at most `METADATA_TTL_MS`, and fetches the JWKS again for a key it
//!   does not hold, at once, then not again for such a key within
//!   `JWKS_REFETCH_MIN_MS` (`may_refetch`).

use base64::Engine;
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use sha2::Sha256;

/// The largest id_token read (Entra's, with groups, run to a few KiB).
pub const ID_TOKEN_MAX_BYTES: usize = 16 * 1024;
/// The largest discovery document, JWKS, or token answer read.
pub const METADATA_MAX_BYTES: usize = 64 * 1024;
/// Keys one JWKS may hold; a larger set is refused whole.
pub const JWKS_KEYS_MAX: usize = 32;
/// The clock skew allowed between the provider and the cell.
pub const SKEW_S: i64 = 120;
/// How old an id_token may be when it arrives: it is minted at the token
/// request, moments before, and a sign-in lasts ten minutes at most.
pub const ID_TOKEN_AGE_MAX_S: i64 = 15 * 60;
/// RSA keys outside these sizes are not used.
pub const RSA_BITS_MIN: usize = 2048;
pub const RSA_BITS_MAX: usize = 4096;
/// The largest RSA public exponent taken (BoringSSL's bound; 65537 is
/// every provider's).
pub const RSA_EXPONENT_MAX: u64 = (1 << 33) - 1;
/// An ES256 signature: P-256's r and s, 32 bytes each (RFC 7518, 3.4).
const ES256_SIGNATURE_BYTES: usize = 64;
/// A subject's length (OpenID Connect Core 1.0, 2: at most 255 ASCII).
pub const SUBJECT_MAX: usize = 255;
/// The longest authorization code taken (Entra's run to a couple of KiB).
pub const CODE_MAX: usize = 4096;
/// How long the router keeps the provider's metadata and keys.
pub const METADATA_TTL_MS: i64 = 60 * 60 * 1000;
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
/// A setting's value kept in an error, at most.
const SAID_MAX: usize = 200;
/// The claims the rules read themselves: none of them is an email.
const RESERVED_CLAIMS: [&str; 9] = ["iss", "sub", "aud", "azp", "exp", "iat", "nbf", "nonce", "email_verified"];
/// What a sign-in's verifier and nonce are derived from the host secret
/// with (`SignInSecrets`).
const DERIVE_SALT: &[u8] = b"fragment oidc sign-in";
const DERIVE_INFO: &[u8] = b"v1";

const _: () = assert!(RSA_BITS_MIN <= RSA_BITS_MAX && SKEW_S > 0 && JWKS_REFETCH_MIN_MS < METADATA_TTL_MS && ID_TOKEN_AGE_MAX_S > SKEW_S);

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

fn cut(s: &str) -> String {
    s.chars().take(SAID_MAX).collect()
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

/// A signature algorithm the cell verifies. Nothing else is: not `none`,
/// not an HMAC (whose key would be the client secret, or a public key
/// misread as one), not a key the token carries.
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
}

/// A provider's metadata (Discovery 1.0, 3), as much as sign-in reads,
/// checked (`Provider::parse`).
#[derive(Debug, Clone)]
pub struct Provider {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
    /// The algorithms both it and the cell know.
    pub algs: Vec<Alg>,
    /// Its token endpoint's client authentication methods (`None`: it
    /// lists none, so `client_secret_basic`, the default).
    auth_methods: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct ProviderDoc {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
    #[serde(default)]
    token_endpoint_auth_methods_supported: Option<Vec<String>>,
    #[serde(default)]
    id_token_signing_alg_values_supported: Option<Vec<String>>,
    #[serde(default)]
    code_challenge_methods_supported: Option<Vec<String>>,
}

impl Provider {
    /// The metadata `issuer` answered, checked: its `issuer` is exactly
    /// the configured one (Discovery 1.0, 4.3: else another provider could
    /// stand in), its endpoints are https URLs (http only beside a loopback
    /// issuer), it signs id_tokens with an algorithm the cell verifies, and
    /// it does not refuse PKCE's S256.
    pub fn parse(issuer: &str, bytes: &[u8]) -> Result<Provider, OidcError> {
        if bytes.len() > METADATA_MAX_BYTES {
            return Err(provider_err(format!("its metadata is over {METADATA_MAX_BYTES} bytes")));
        }
        let doc: ProviderDoc = serde_json::from_slice(bytes).map_err(|e| provider_err(format!("its metadata is not OpenID Connect discovery JSON: {e}")))?;
        if doc.issuer != issuer {
            return Err(provider_err(format!("its metadata names the issuer {:?}, not {issuer:?} (they must match exactly)", cut(&doc.issuer))));
        }
        let dev = issuer.starts_with("http:");
        let endpoint = |name: &str, raw: String| -> Result<String, OidcError> {
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
        let algs: Vec<Alg> = match &doc.id_token_signing_alg_values_supported {
            Some(listed) => listed.iter().filter_map(|a| Alg::parse(a)).collect(),
            // REQUIRED in the metadata; RS256 is every provider's default (Core 1.0, 15.1)
            None => vec![Alg::Rs256],
        };
        if algs.is_empty() {
            return Err(provider_err("it signs id_tokens with neither RS256 nor ES256"));
        }
        if doc.code_challenge_methods_supported.as_ref().is_some_and(|m| !m.iter().any(|m| m == "S256")) {
            return Err(provider_err("it lists PKCE methods without S256"));
        }
        Ok(Provider {
            authorization_endpoint: endpoint("authorization_endpoint", doc.authorization_endpoint)?,
            token_endpoint: endpoint("token_endpoint", doc.token_endpoint)?,
            jwks_uri: endpoint("jwks_uri", doc.jwks_uri)?,
            algs,
            auth_methods: doc.token_endpoint_auth_methods_supported,
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
                let listed = self.auth_methods.as_deref().unwrap_or_default();
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
            b64().encode(m.finalize().into_bytes())
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

#[derive(Deserialize)]
struct TokenAnswer {
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
}

/// The id_token in the token endpoint's answer (`status`, `bytes`), or
/// what it refused with.
pub fn id_token_of(status: u16, bytes: &[u8]) -> Result<String, OidcError> {
    let answer = serde_json::from_slice::<TokenAnswer>(bytes).ok();
    if status != 200 {
        let (error, description) = answer.map(|a| (a.error.unwrap_or_default(), a.error_description.unwrap_or_default())).unwrap_or_default();
        return Err(OidcError::Refused { status, error: cut(&error), description: cut(&description) });
    }
    let answer = answer.ok_or_else(|| provider_err("its token answer is not JSON"))?;
    answer.id_token.filter(|t| !t.is_empty()).ok_or_else(|| provider_err("its token answer carries no id_token (is `openid` among FRAGMENT_OIDC_SCOPES?)"))
}

// ------------------------------------------------------------------ keys

/// A provider's public key, its numbers as the JWKS gave them, checked for
/// size (`key_of`); the runtime's WebCrypto imports them as a JWK the cell
/// makes from these numbers alone, so nothing else the provider's JWK
/// says reaches it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Public {
    /// RSA's modulus and public exponent, big-endian, no leading zeros.
    Rsa { n: Vec<u8>, e: Vec<u8> },
    /// A P-256 point.
    Ec { x: [u8; 32], y: [u8; 32] },
}

impl Public {
    /// The length of a signature this key makes.
    fn signature_len(&self) -> usize {
        match self {
            Public::Rsa { n, .. } => n.len(),
            Public::Ec { .. } => ES256_SIGNATURE_BYTES,
        }
    }
}

/// One signing key of the provider's.
#[derive(Debug)]
struct Key {
    kid: Option<String>,
    /// The algorithm the key is for, when the JWK names one.
    alg: Option<Alg>,
    public: Public,
}

impl Key {
    fn fits(&self, alg: Alg) -> bool {
        let kind = matches!((&self.public, alg), (Public::Rsa { .. }, Alg::Rs256) | (Public::Ec { .. }, Alg::Es256));
        kind && self.alg.is_none_or(|a| a == alg)
    }
}

/// The provider's signing keys (RFC 7517), the ones the cell can use.
#[derive(Debug)]
pub struct Jwks {
    keys: Vec<Key>,
}

#[derive(Deserialize)]
struct JwkDoc {
    kty: String,
    #[serde(default)]
    kid: Option<String>,
    #[serde(default, rename = "use")]
    use_: Option<String>,
    #[serde(default)]
    alg: Option<String>,
    #[serde(default)]
    n: Option<String>,
    #[serde(default)]
    e: Option<String>,
    #[serde(default)]
    crv: Option<String>,
    #[serde(default)]
    x: Option<String>,
    #[serde(default)]
    y: Option<String>,
}

#[derive(Deserialize)]
struct JwksDoc {
    keys: Vec<JwkDoc>,
}

/// A big-endian unsigned number less its leading zeros (a JWK's may carry one).
fn unsigned(mut bytes: &[u8]) -> &[u8] {
    while let [0, rest @ ..] = bytes {
        bytes = rest;
    }
    bytes
}

/// A JWK the cell can verify with, or `None`: one for encryption, another
/// algorithm or curve, an RSA key outside the sizes or with an exponent no
/// runtime takes, or malformed. A set often holds such keys beside its
/// signing ones. Whether an EC point is on the curve is the runtime's to
/// say, as it imports the key: a point off it verifies nothing.
fn key_of(doc: JwkDoc) -> Option<Key> {
    if doc.use_.as_deref().is_some_and(|u| u != "sig") {
        return None;
    }
    let alg = match doc.alg.as_deref() {
        None => None,
        Some(a) => Some(Alg::parse(a)?),
    };
    let bytes = |s: &Option<String>| s.as_deref().and_then(|v| b64().decode(v).ok());
    let public = match doc.kty.as_str() {
        "RSA" => {
            let (n, e) = (bytes(&doc.n)?, bytes(&doc.e)?);
            let (n, e) = (unsigned(&n), unsigned(&e));
            let bits = n.first().map_or(0, |top| n.len() * 8 - top.leading_zeros() as usize);
            if !(RSA_BITS_MIN..=RSA_BITS_MAX).contains(&bits) {
                return None;
            }
            let exponent = (e.len() <= 8).then(|| e.iter().fold(0u64, |acc, b| acc << 8 | u64::from(*b)))?;
            if exponent < 3 || exponent % 2 == 0 || exponent > RSA_EXPONENT_MAX {
                return None;
            }
            Public::Rsa { n: n.to_vec(), e: e.to_vec() }
        }
        "EC" if doc.crv.as_deref() == Some("P-256") => Public::Ec { x: bytes(&doc.x)?.try_into().ok()?, y: bytes(&doc.y)?.try_into().ok()? },
        _ => return None,
    };
    Some(Key { kid: doc.kid, alg, public })
}

impl Jwks {
    /// The keys at the provider's `jwks_uri`, the usable ones kept. A set
    /// over `JWKS_KEYS_MAX` keys or `METADATA_MAX_BYTES`, or with none the
    /// cell can use, is refused.
    pub fn parse(bytes: &[u8]) -> Result<Jwks, OidcError> {
        if bytes.len() > METADATA_MAX_BYTES {
            return Err(provider_err(format!("its JWKS is over {METADATA_MAX_BYTES} bytes")));
        }
        let doc: JwksDoc = serde_json::from_slice(bytes).map_err(|e| provider_err(format!("its JWKS is not a JSON key set: {e}")))?;
        if doc.keys.len() > JWKS_KEYS_MAX {
            return Err(provider_err(format!("its JWKS holds {} keys, over {JWKS_KEYS_MAX}", doc.keys.len())));
        }
        let keys: Vec<Key> = doc.keys.into_iter().filter_map(key_of).collect();
        if keys.is_empty() {
            return Err(provider_err("its JWKS holds no RS256 or ES256 signing key"));
        }
        Ok(Jwks { keys })
    }

    /// How many usable keys it holds.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// The key a token's header names: by its `kid`, of the algorithm's
    /// kind; without a `kid`, the one key of that kind, if there is one.
    fn select(&self, alg: Alg, kid: Option<&str>) -> Result<&Key, IdTokenError> {
        let mut fitting = self.keys.iter().filter(|k| k.fits(alg));
        match kid {
            Some(kid) => fitting.find(|k| k.kid.as_deref() == Some(kid)).ok_or_else(|| IdTokenError::UnknownKey(Some(cut(kid)))),
            None => match (fitting.next(), fitting.next()) {
                (Some(k), None) => Ok(k),
                _ => Err(IdTokenError::UnknownKey(None)),
            },
        }
    }
}

/// Whether the keys may be fetched again for a key they lack (a
/// rotation), the last such fetch at `refetched_at_ms` (`None`: none yet):
/// at once the first time, then at most once a `JWKS_REFETCH_MIN_MS`. A
/// clock run backwards allows none until it passes that fetch again.
pub fn may_refetch(refetched_at_ms: Option<i64>, now_ms: i64) -> bool {
    refetched_at_ms.is_none_or(|at| now_ms.saturating_sub(at) >= JWKS_REFETCH_MIN_MS)
}

/// Whether metadata or keys fetched at `fetched_at_ms` are past their time.
/// A clock that runs backwards makes them stale, never fresher.
pub fn stale(fetched_at_ms: i64, now_ms: i64) -> bool {
    now_ms.saturating_sub(fetched_at_ms) >= METADATA_TTL_MS || now_ms < fetched_at_ms
}

// -------------------------------------------------------------- id_token

/// Why an id_token is not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdTokenError {
    TooLarge,
    Malformed(&'static str),
    /// Signed with an algorithm the cell does not take (`none`, an HMAC,
    /// one the provider did not list).
    Alg(String),
    /// Its header names an extension the cell does not understand.
    Critical,
    /// No key in the cell's JWKS fits it (the `kid`, if it names one): the
    /// caller may fetch the JWKS again, once (`may_refetch`).
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

#[derive(Deserialize)]
struct Header {
    alg: String,
    #[serde(default)]
    kid: Option<String>,
    #[serde(default)]
    typ: Option<String>,
    #[serde(default)]
    crit: Option<IgnoredAny>,
}

/// An id_token's signed parts and the key its header chose: what the
/// runtime's WebCrypto checks (`key` verifies `signature` over `input` by
/// `alg`), before anything of its claims is read.
#[derive(Debug)]
pub struct Signed<'t, 'k> {
    pub alg: Alg,
    pub key: &'k Public,
    /// The JWS signing input: the header and the claims as they came.
    pub input: &'t [u8],
    pub signature: Vec<u8>,
    claims: &'t str,
}

fn part(s: &str) -> Result<Vec<u8>, IdTokenError> {
    b64().decode(s).map_err(|_| IdTokenError::Malformed("a part is not base64url"))
}

/// How much of the runtime's word for why it could not check a signature
/// is kept (it is logged, and said on a page).
const UNCHECKED_MAX: usize = 200;

/// An id_token's header and key (Core 1.0, 3.1.3.7, 6 and 7): a compact
/// JWS, signed by an algorithm the provider lists (`algs`) and the cell
/// takes, with no extension the cell does not understand, no access token,
/// and a key of the provider's JWKS (never one the token names), whose
/// signature's length it has. The signature is not checked here.
pub fn signed<'t, 'k>(token: &'t str, jwks: &'k Jwks, algs: &[Alg]) -> Result<Signed<'t, 'k>, IdTokenError> {
    if token.len() > ID_TOKEN_MAX_BYTES {
        return Err(IdTokenError::TooLarge);
    }
    let mut parts = token.split('.');
    let (Some(h), Some(p), Some(s), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err(IdTokenError::Malformed("not three dot-separated parts (a compact JWS)"));
    };
    let header: Header = serde_json::from_slice(&part(h)?).map_err(|_| IdTokenError::Malformed("the header is not a JWS header"))?;
    let alg = Alg::parse(&header.alg).filter(|a| algs.contains(a)).ok_or_else(|| IdTokenError::Alg(header.alg.chars().take(16).collect()))?;
    if header.crit.is_some() {
        return Err(IdTokenError::Critical);
    }
    // an access token (RFC 9068) is never taken for an id_token
    if header.typ.as_deref().is_some_and(|t| t.eq_ignore_ascii_case("at+jwt") || t.eq_ignore_ascii_case("application/at+jwt")) {
        return Err(IdTokenError::Malformed("it is an access token (typ at+jwt)"));
    }
    let key = jwks.select(alg, header.kid.as_deref())?;
    let signature = part(s)?;
    if signature.len() != key.public.signature_len() {
        return Err(IdTokenError::Signature);
    }
    Ok(Signed { alg, key: &key.public, input: &token.as_bytes()[..h.len() + 1 + p.len()], signature, claims: p })
}

impl Signed<'_, '_> {
    /// The id_token's claims, once the runtime said whether its signature
    /// verifies (`check`: yes, no, or why it could not tell), checked for
    /// this sign-in: `iss` exactly, `aud` this client (and `azp` it when
    /// there are several audiences), `exp`, `iat` and `nbf` within
    /// `SKEW_S`, an `iat` at most `ID_TOKEN_AGE_MAX_S` old, this sign-in's
    /// nonce, and a `sub`. A signature that does not verify reads nothing.
    pub fn verified(self, check: Result<bool, String>, expect: &Expect<'_>) -> Result<Verified, IdTokenError> {
        let settings = expect.settings;
        assert!(!settings.issuer.is_empty() && !settings.client_id.is_empty() && !expect.nonce.is_empty(), "a sign-in expects an issuer, a client, and a nonce");
        match check {
            Ok(true) => {}
            Ok(false) => return Err(IdTokenError::Signature),
            Err(why) => return Err(IdTokenError::Unchecked(why.chars().take(UNCHECKED_MAX).collect())),
        }
        let bytes = part(self.claims)?;
        let c = ClaimsSeed { email_claim: &settings.email_claim }
            .deserialize(&mut serde_json::Deserializer::from_slice(&bytes))
            .map_err(|_| IdTokenError::Malformed("the claims are not an id_token's (a JSON object, each claim of its type, none twice)"))?;
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
        let exp = c.exp.ok_or(IdTokenError::Malformed("it has no exp"))?;
        if exp.saturating_add(SKEW_S) <= now {
            return Err(IdTokenError::Expired);
        }
        let iat = c.iat.ok_or(IdTokenError::Malformed("it has no iat"))?;
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
}

/// The claims sign-in reads, the email from the claim the settings name;
/// every other claim is skipped unread.
#[derive(Default)]
struct Claims {
    iss: Option<String>,
    sub: Option<String>,
    aud: Vec<String>,
    azp: Option<String>,
    exp: Option<i64>,
    iat: Option<i64>,
    nbf: Option<i64>,
    nonce: Option<String>,
    email: Option<String>,
    email_verified: Option<bool>,
}

struct ClaimsSeed<'a> {
    email_claim: &'a str,
}

/// A NumericDate (seconds, maybe fractional), floored.
struct NumericDate(i64);

impl<'de> Deserialize<'de> for NumericDate {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = NumericDate;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("seconds since the epoch")
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<NumericDate, E> {
                Ok(NumericDate(v))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<NumericDate, E> {
                i64::try_from(v).map(NumericDate).map_err(|_| E::custom("a time past i64"))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<NumericDate, E> {
                match v.is_finite() && v.abs() < 1e15 {
                    true => Ok(NumericDate(v.floor() as i64)),
                    false => Err(E::custom("a time out of range")),
                }
            }
        }
        d.deserialize_any(V)
    }
}

/// `aud`: one audience, or a list of them (Core 1.0, 2).
struct Audiences(Vec<String>);

impl<'de> Deserialize<'de> for Audiences {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Audiences;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an audience, or a list of them")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Audiences, E> {
                Ok(Audiences(vec![v.to_string()]))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Audiences, A::Error> {
                let mut all = Vec::new();
                while let Some(a) = seq.next_element::<String>()? {
                    // bounded: an id_token is at most ID_TOKEN_MAX_BYTES
                    all.push(a);
                }
                Ok(Audiences(all))
            }
        }
        d.deserialize_any(V)
    }
}

impl<'de> DeserializeSeed<'de> for ClaimsSeed<'_> {
    type Value = Claims;

    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<Claims, D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for ClaimsSeed<'_> {
    type Value = Claims;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("an id_token's claims")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Claims, A::Error> {
        fn once<T, E: de::Error>(slot: &mut Option<T>, v: T) -> Result<(), E> {
            match slot.replace(v) {
                None => Ok(()),
                Some(_) => Err(E::custom("a claim given twice")),
            }
        }
        let mut c = Claims::default();
        let mut aud = None;
        while let Some(k) = map.next_key::<String>()? {
            match k.as_str() {
                "iss" => once(&mut c.iss, map.next_value()?)?,
                "sub" => once(&mut c.sub, map.next_value()?)?,
                "aud" => once(&mut aud, map.next_value::<Audiences>()?.0)?,
                "azp" => once(&mut c.azp, map.next_value()?)?,
                "exp" => once(&mut c.exp, map.next_value::<NumericDate>()?.0)?,
                "iat" => once(&mut c.iat, map.next_value::<NumericDate>()?.0)?,
                "nbf" => once(&mut c.nbf, map.next_value::<NumericDate>()?.0)?,
                "nonce" => once(&mut c.nonce, map.next_value()?)?,
                "email_verified" => once(&mut c.email_verified, map.next_value()?)?,
                // the email claim the settings name (never a reserved one: Settings::parse)
                k if k == self.email_claim => once(&mut c.email, map.next_value()?)?,
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        c.aud = aud.unwrap_or_default();
        Ok(c)
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
mod tests {
    use super::*;
    use rsa::pkcs1v15::{SigningKey, VerifyingKey};
    use rsa::signature::{SignatureEncoding, Signer, Verifier};
    use rsa::traits::PublicKeyParts;
    use std::sync::OnceLock;

    const ISSUER: &str = "https://idp.corp.example/realms/corp";
    const CLIENT: &str = "fragment";
    const NONCE: &str = "n-0123456789";
    const NOW: i64 = 1_790_000_000;
    const HOST: &str = "a host secret of at least thirty-two bytes, for tests";

    /// One RSA key per test run: making one is the slow part.
    fn rsa_key() -> &'static rsa::RsaPrivateKey {
        static KEY: OnceLock<rsa::RsaPrivateKey> = OnceLock::new();
        KEY.get_or_init(|| rsa::RsaPrivateKey::new(&mut rand_core::OsRng, 2048).expect("an RSA key"))
    }

    fn ec_key() -> p256::ecdsa::SigningKey {
        p256::ecdsa::SigningKey::from_slice(&[7u8; 32]).expect("an EC key")
    }

    fn rsa_jwk(kid: &str, key: &rsa::RsaPrivateKey) -> String {
        let p = key.to_public_key();
        format!(r#"{{"kty":"RSA","use":"sig","alg":"RS256","kid":"{kid}","n":"{}","e":"{}"}}"#, b64().encode(p.n().to_bytes_be()), b64().encode(p.e().to_bytes_be()))
    }

    fn ec_jwk(kid: &str, key: &p256::ecdsa::SigningKey) -> String {
        let point = key.verifying_key().to_encoded_point(false);
        format!(r#"{{"kty":"EC","use":"sig","alg":"ES256","crv":"P-256","kid":"{kid}","x":"{}","y":"{}"}}"#, b64().encode(point.x().unwrap()), b64().encode(point.y().unwrap()))
    }

    fn jwks() -> Jwks {
        Jwks::parse(format!(r#"{{"keys":[{},{}]}}"#, rsa_jwk("r1", rsa_key()), ec_jwk("e1", &ec_key())).as_bytes()).unwrap()
    }

    fn settings() -> Settings {
        Settings::parse(Vars { issuer: Some(ISSUER), client_id: Some(CLIENT), ..Vars::default() }).unwrap().unwrap()
    }

    fn claims(extra: &str) -> String {
        format!(r#"{{"iss":"{ISSUER}","sub":"user-1","aud":"{CLIENT}","exp":{},"iat":{NOW},"nonce":"{NONCE}","email":"Jane@Corp.Example","email_verified":true{extra}}}"#, NOW + 300)
    }

    fn sign(header: &str, claims: &str) -> String {
        let input = format!("{}.{}", b64().encode(header), b64().encode(claims));
        let sig = if header.contains(r#""alg":"RS256""#) {
            SigningKey::<Sha256>::new(rsa_key().clone()).sign(input.as_bytes()).to_vec()
        } else if header.contains(r#""alg":"ES256""#) {
            let s: p256::ecdsa::Signature = ec_key().sign(input.as_bytes());
            s.to_bytes().to_vec()
        } else {
            vec![]
        };
        format!("{input}.{}", b64().encode(sig))
    }

    fn rs(claims: &str) -> String {
        sign(r#"{"alg":"RS256","typ":"JWT","kid":"r1"}"#, claims)
    }

    /// The runtime's check, stood in for: what WebCrypto's `verify` says.
    fn host_check(s: &Signed<'_, '_>) -> Result<bool, String> {
        match (s.alg, s.key) {
            (Alg::Rs256, Public::Rsa { n, e }) => {
                let key = rsa::RsaPublicKey::new(rsa::BigUint::from_bytes_be(n), rsa::BigUint::from_bytes_be(e)).map_err(|e| e.to_string())?;
                let sig = rsa::pkcs1v15::Signature::try_from(s.signature.as_slice()).map_err(|e| e.to_string())?;
                Ok(VerifyingKey::<Sha256>::new(key).verify(s.input, &sig).is_ok())
            }
            (Alg::Es256, Public::Ec { x, y }) => {
                let point = p256::EncodedPoint::from_affine_coordinates(x.into(), y.into(), false);
                let key = p256::ecdsa::VerifyingKey::from_encoded_point(&point).map_err(|e| e.to_string())?;
                let sig = p256::ecdsa::Signature::from_slice(&s.signature).map_err(|e| e.to_string())?;
                use p256::ecdsa::signature::Verifier as _;
                Ok(key.verify(s.input, &sig).is_ok())
            }
            _ => Err("a key of another kind".into()),
        }
    }

    fn verify_with(token: &str, jwks: &Jwks, settings: &Settings, nonce: &str, now_s: i64) -> Result<Verified, IdTokenError> {
        let s = signed(token, jwks, &[Alg::Rs256, Alg::Es256])?;
        let check = host_check(&s);
        s.verified(check, &Expect { settings, nonce, now_s })
    }

    fn verify(token: &str) -> Result<Verified, IdTokenError> {
        verify_with(token, &jwks(), &settings(), NONCE, NOW)
    }

    // Goal: an id_token signed by the provider's key, for this client and
    // this sign-in, verifies, under RS256 and ES256, its subject and email
    // read. Method: tokens signed here, the runtime stood in for.
    #[test]
    fn valid_tokens_verify() {
        let v = verify(&rs(&claims(""))).unwrap();
        assert_eq!(v, Verified { subject: "user-1".into(), email: Some("Jane@Corp.Example".into()), email_verified: true });
        let es = sign(r#"{"alg":"ES256","kid":"e1"}"#, &claims(""));
        assert_eq!(verify(&es).unwrap().subject, "user-1");
        // several audiences, this client the authorized party; a fractional time; another claim skipped
        let more = claims(r#","azp":"fragment","groups":["a",{"b":1}],"exp2":1.5"#).replace(r#""aud":"fragment""#, r#""aud":["fragment","other"]"#);
        assert!(verify(&rs(&more)).is_ok());
        let frac = claims("").replace(&format!(r#""iat":{NOW}"#), &format!(r#""iat":{NOW}.75"#));
        assert!(verify(&rs(&frac)).is_ok());
        // no kid: the one key of that kind
        let ec_only = Jwks::parse(format!(r#"{{"keys":[{}]}}"#, ec_jwk("e1", &ec_key())).as_bytes()).unwrap();
        assert!(verify_with(&sign(r#"{"alg":"ES256"}"#, &claims("")), &ec_only, &settings(), NONCE, NOW).is_ok());
    }

    // Goal: a signature that does not verify reads nothing, and the
    // runtime is asked only about a token whose header and key passed.
    #[test]
    fn bad_signatures_are_refused() {
        let token = rs(&claims(""));
        let (input, sig) = token.rsplit_once('.').unwrap();
        let mut bytes = b64().decode(sig).unwrap();
        bytes[3] ^= 1;
        assert_eq!(verify(&format!("{input}.{}", b64().encode(&bytes))), Err(IdTokenError::Signature));
        // a signature of the wrong length is refused before the runtime is asked
        bytes.pop();
        let (keys, short) = (jwks(), format!("{input}.{}", b64().encode(&bytes)));
        assert_eq!(signed(&short, &keys, &[Alg::Rs256]).unwrap_err(), IdTokenError::Signature);
        // claims changed under a signature
        let other = rs(&claims(""));
        let forged = format!("{}.{}.{}", other.split('.').next().unwrap(), b64().encode(claims("").replace("user-1", "user-2")), other.rsplit('.').next().unwrap());
        assert_eq!(verify(&forged), Err(IdTokenError::Signature));
        // the runtime could not tell: refused, its word kept short
        let token = rs(&claims(""));
        let s = signed(&token, &keys, &[Alg::Rs256]).unwrap();
        let e = s.verified(Err("x".repeat(500)), &Expect { settings: &settings(), nonce: NONCE, now_s: NOW }).unwrap_err();
        assert!(matches!(&e, IdTokenError::Unchecked(why) if why.len() == UNCHECKED_MAX));
        // a forgery under a published kid, by another key: only the signature tells
        let stranger = rsa::RsaPrivateKey::new(&mut rand_core::OsRng, 2048).unwrap();
        let input = format!("{}.{}", b64().encode(r#"{"alg":"RS256","kid":"r1"}"#), b64().encode(claims("")));
        let sig = SigningKey::<Sha256>::new(stranger).sign(input.as_bytes()).to_vec();
        assert_eq!(verify(&format!("{input}.{}", b64().encode(sig))), Err(IdTokenError::Signature));
    }

    // Goal: no algorithm but the provider's listed RS256/ES256 is taken
    // (`none`, HS256 keyed by anything, an RS256 header over an EC key),
    // nor a header the cell does not understand, nor an access token.
    #[test]
    fn algorithm_confusion_is_refused() {
        let none = format!("{}.{}.", b64().encode(r#"{"alg":"none"}"#), b64().encode(claims("")));
        assert_eq!(verify(&none), Err(IdTokenError::Alg("none".into())));
        let hs = {
            let input = format!("{}.{}", b64().encode(r#"{"alg":"HS256","kid":"r1"}"#), b64().encode(claims("")));
            let mut m = <Hmac<Sha256> as Mac>::new_from_slice(b"the client secret").unwrap();
            m.update(input.as_bytes());
            format!("{input}.{}", b64().encode(m.finalize().into_bytes()))
        };
        assert_eq!(verify(&hs), Err(IdTokenError::Alg("HS256".into())));
        // ES256 where the provider lists RS256 alone
        let es = sign(r#"{"alg":"ES256","kid":"e1"}"#, &claims(""));
        assert_eq!(signed(&es, &jwks(), &[Alg::Rs256]).unwrap_err(), IdTokenError::Alg("ES256".into()));
        // an RS256 header naming the EC key's kid
        assert_eq!(verify(&rs(&claims("")).replacen(&b64().encode(r#"{"alg":"RS256","typ":"JWT","kid":"r1"}"#), &b64().encode(r#"{"alg":"RS256","kid":"e1"}"#), 1)), Err(IdTokenError::UnknownKey(Some("e1".into()))));
        assert_eq!(verify(&sign(r#"{"alg":"RS256","kid":"r1","crit":["exp"]}"#, &claims(""))), Err(IdTokenError::Critical));
        assert!(matches!(verify(&sign(r#"{"alg":"RS256","kid":"r1","typ":"at+jwt"}"#, &claims(""))), Err(IdTokenError::Malformed(_))));
        // a key the token carries is never used
        let carried = sign(r#"{"alg":"RS256","kid":"zz","jwk":{"kty":"RSA"}}"#, &claims(""));
        assert_eq!(verify(&carried), Err(IdTokenError::UnknownKey(Some("zz".into()))));
        assert!(matches!(verify(&sign(r#"{"alg":"RS256","kid":7}"#, &claims(""))), Err(IdTokenError::Malformed(_))));
    }

    // Goal: each claim this sign-in must see is checked: issuer, audience,
    // authorized party, the times, the nonce, the subject.
    #[test]
    fn wrong_claims_are_refused() {
        let with = |from: &str, to: &str| verify(&rs(&claims("").replace(from, to)));
        assert_eq!(with(ISSUER, "https://idp.corp.example/realms/other"), Err(IdTokenError::Issuer));
        assert_eq!(with(&format!(r#""iss":"{ISSUER}","#), ""), Err(IdTokenError::Issuer));
        assert_eq!(with(r#""aud":"fragment""#, r#""aud":"another""#), Err(IdTokenError::Audience));
        assert_eq!(with(r#""aud":"fragment""#, r#""aud":[]"#), Err(IdTokenError::Audience));
        assert_eq!(with(r#""aud":"fragment""#, r#""aud":["fragment","other"]"#), Err(IdTokenError::AuthorizedParty));
        assert_eq!(verify(&rs(&claims(r#","azp":"other""#))), Err(IdTokenError::AuthorizedParty));
        let exp = format!(r#""exp":{}"#, NOW + 300);
        assert_eq!(with(&exp, &format!(r#""exp":{}"#, NOW - SKEW_S)), Err(IdTokenError::Expired));
        assert!(with(&exp, &format!(r#""exp":{}"#, NOW - SKEW_S + 1)).is_ok(), "within the skew");
        let iat = format!(r#""iat":{NOW}"#);
        assert_eq!(with(&iat, &format!(r#""iat":{}"#, NOW + SKEW_S + 1)), Err(IdTokenError::NotYetValid));
        assert_eq!(verify(&rs(&claims(&format!(r#","nbf":{}"#, NOW + SKEW_S + 1)))), Err(IdTokenError::NotYetValid));
        assert_eq!(with(&iat, &format!(r#""iat":{}"#, NOW - ID_TOKEN_AGE_MAX_S - SKEW_S - 1)), Err(IdTokenError::TooOld));
        assert_eq!(with(NONCE, "n-another-sign-in"), Err(IdTokenError::Nonce));
        assert_eq!(with(r#""sub":"user-1""#, r#""sub":"""#), Err(IdTokenError::Subject));
        assert_eq!(with("user-1", &"u".repeat(SUBJECT_MAX + 1)), Err(IdTokenError::Subject));
        assert!(matches!(with(&exp, r#""exp":"soon""#), Err(IdTokenError::Malformed(_))));
        assert!(matches!(with(&exp, &format!(r#""exp":{},"exp":{}"#, NOW + 300, NOW + 9000)), Err(IdTokenError::Malformed(_))), "a claim given twice");
        assert!(matches!(with(r#""email_verified":true"#, r#""email_verified":"true""#), Err(IdTokenError::Malformed(_))), "email_verified is a boolean");
        assert!(matches!(verify(&rs("[1]")), Err(IdTokenError::Malformed(_))));
        assert_eq!(verify(&"x".repeat(ID_TOKEN_MAX_BYTES + 1)), Err(IdTokenError::TooLarge));
        assert!(matches!(verify("a.b"), Err(IdTokenError::Malformed(_))));
    }

    // Goal: an id_token one sign-in received signs no one in through
    // another (a replay): its nonce is the first's, and the second's is
    // derived from its own state.
    #[test]
    fn a_replayed_token_signs_no_one_in() {
        let first = SignInSecrets::of(HOST, &"a".repeat(64));
        let second = SignInSecrets::of(HOST, &"b".repeat(64));
        let token = rs(&claims("").replace(NONCE, &first.nonce));
        assert!(verify_with(&token, &jwks(), &settings(), &first.nonce, NOW).is_ok());
        assert_eq!(verify_with(&token, &jwks(), &settings(), &second.nonce, NOW), Err(IdTokenError::Nonce));
    }

    // Goal: a sign-in's verifier and nonce are the platform's alone and its
    // own: the same for its state under one host secret (a restart in the
    // middle finishes), another for another state or host secret, a
    // verifier RFC 7636 takes, and its challenge S256's (RFC 7636's vector).
    #[test]
    fn a_sign_ins_secrets_are_derived_from_the_host_secret_and_its_state() {
        let state = "c".repeat(64);
        let a = SignInSecrets::of(HOST, &state);
        assert_eq!(a, SignInSecrets::of(HOST, &state), "a restart derives the same");
        assert_ne!(a, SignInSecrets::of(HOST, &"d".repeat(64)));
        assert_ne!(a, SignInSecrets::of(&format!("{HOST}, rotated"), &state));
        assert!(crate::own_signin::valid_token(&a.verifier) && a.nonce.len() == 43 && a.nonce != a.verifier);
        assert_eq!(crate::own_signin::challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    // Goal: a key withdrawn or not yet held is unknown, so the JWKS is
    // fetched again, at most once a JWKS_REFETCH_MIN_MS; and the caches go
    // stale within the hour, or at once on a clock run backwards.
    #[test]
    fn a_rotated_key_is_fetched_again_within_bounds() {
        let rotated = sign(r#"{"alg":"RS256","kid":"r2"}"#, &claims(""));
        assert_eq!(verify(&rotated), Err(IdTokenError::UnknownKey(Some("r2".into()))));
        let after = Jwks::parse(format!(r#"{{"keys":[{},{}]}}"#, rsa_jwk("r1", rsa_key()), rsa_jwk("r2", rsa_key())).as_bytes()).unwrap();
        assert!(verify_with(&rotated, &after, &settings(), NONCE, NOW).is_ok());
        assert!(may_refetch(None, 1_000), "a rotation is picked up at once");
        assert!(!may_refetch(Some(1_000), 1_000 + JWKS_REFETCH_MIN_MS - 1) && may_refetch(Some(1_000), 1_000 + JWKS_REFETCH_MIN_MS) && !may_refetch(Some(1_000), 999));
        assert!(!stale(1_000, 1_000 + METADATA_TTL_MS - 1) && stale(1_000, 1_000 + METADATA_TTL_MS) && stale(1_000, 999));
        // two RSA keys and no kid: none chosen
        assert_eq!(verify_with(&sign(r#"{"alg":"RS256"}"#, &claims("")), &after, &settings(), NONCE, NOW), Err(IdTokenError::UnknownKey(None)));
    }

    // Goal: a JWKS keeps only the keys the cell can use, and refuses a set
    // with none, or too many.
    #[test]
    fn jwks_keeps_only_usable_keys() {
        let small = rsa::RsaPrivateKey::new(&mut rand_core::OsRng, 1024).unwrap();
        let set = format!(
            r#"{{"keys":[{},{},{},{{"kty":"EC","crv":"P-384","x":"AA","y":"AA"}},{{"kty":"oct","k":"c2VjcmV0"}},{},{{"kty":"RSA","n":"AQAB","e":"Ag"}},{}]}}"#,
            rsa_jwk("enc", rsa_key()).replace(r#""use":"sig""#, r#""use":"enc""#),
            rsa_jwk("small", &small),
            rsa_jwk("hs", rsa_key()).replace("RS256", "HS256"),
            rsa_jwk("ok", rsa_key()).replace(r#""use":"sig","alg":"RS256","#, ""),
            ec_jwk("e1", &ec_key()).replace(r#""x5c":"#, "")
        );
        let keys = Jwks::parse(set.as_bytes()).unwrap();
        assert_eq!(keys.len(), 2, "the RSA key with no use or alg, and the P-256 key: {keys:?}");
        assert!(Jwks::parse(br#"{"keys":[{"kty":"oct","k":"c2VjcmV0"}]}"#).is_err());
        let many = format!(r#"{{"keys":[{}]}}"#, vec![ec_jwk("e", &ec_key()); JWKS_KEYS_MAX + 1].join(","));
        assert!(Jwks::parse(many.as_bytes()).is_err());
        assert!(Jwks::parse(b"not json").is_err());
    }

    fn provider_doc(issuer: &str) -> String {
        format!(
            r#"{{"issuer":"{issuer}","authorization_endpoint":"{issuer}/authorize","token_endpoint":"{issuer}/token","jwks_uri":"{issuer}/jwks","id_token_signing_alg_values_supported":["RS256","HS256"],"code_challenge_methods_supported":["S256","plain"],"token_endpoint_auth_methods_supported":["client_secret_post","client_secret_basic"],"end_session_endpoint":"{issuer}/logout"}}"#
        )
    }

    // Goal: a provider's metadata is taken only from the issuer it names,
    // its endpoints https (http only beside a loopback issuer), signing
    // with RS256 or ES256, not refusing S256; and the client's
    // authentication is chosen from what it lists, never without a secret.
    #[test]
    fn discovery_is_checked() {
        let p = Provider::parse(ISSUER, provider_doc(ISSUER).as_bytes()).unwrap();
        assert_eq!(p.algs, vec![Alg::Rs256]);
        assert_eq!(p.token_endpoint, format!("{ISSUER}/token"));
        assert_eq!(discovery_url(&format!("{ISSUER}/")), format!("{ISSUER}/.well-known/openid-configuration"));
        assert!(Provider::parse("https://idp.corp.example/realms/other", provider_doc(ISSUER).as_bytes()).is_err(), "another issuer's");
        assert!(Provider::parse(ISSUER, provider_doc(ISSUER).replace(&format!("{ISSUER}/token"), "http://idp.corp.example/token").as_bytes()).is_err());
        let dev = "http://127.0.0.1:5556";
        assert!(Provider::parse(dev, provider_doc(dev).as_bytes()).is_ok(), "http beside a loopback issuer");
        assert!(Provider::parse(dev, provider_doc(dev).replace(&format!("{dev}/jwks"), "http://10.0.0.1/jwks").as_bytes()).is_err());
        assert!(Provider::parse(ISSUER, provider_doc(ISSUER).replace(r#"["RS256","HS256"]"#, r#"["HS256"]"#).as_bytes()).is_err());
        assert!(Provider::parse(ISSUER, provider_doc(ISSUER).replace(r#"["S256","plain"]"#, r#"["plain"]"#).as_bytes()).is_err());
        assert_eq!(p.client_auth(None, true), Ok(ClientAuth::Basic), "Basic first when it lists both");
        assert_eq!(p.client_auth(None, false), Ok(ClientAuth::None), "no secret: a public client");
        assert_eq!(p.client_auth(Some(ClientAuth::Post), true), Ok(ClientAuth::Post));
        assert!(matches!(p.client_auth(Some(ClientAuth::Basic), false), Err(OidcError::Config(_))));
        let post_only = Provider::parse(ISSUER, provider_doc(ISSUER).replace(r#""client_secret_post","client_secret_basic""#, r#""client_secret_post""#).as_bytes()).unwrap();
        assert_eq!(post_only.client_auth(None, true), Ok(ClientAuth::Post));
        let lists_none = Provider::parse(ISSUER, provider_doc(ISSUER).replace(r#","token_endpoint_auth_methods_supported":["client_secret_post","client_secret_basic"]"#, "").as_bytes()).unwrap();
        assert_eq!(lists_none.client_auth(None, true), Ok(ClientAuth::Basic), "the spec's default");
        let jwt_only = Provider::parse(ISSUER, provider_doc(ISSUER).replace(r#""client_secret_post","client_secret_basic""#, r#""private_key_jwt""#).as_bytes()).unwrap();
        assert!(jwt_only.client_auth(None, true).is_err());
    }

    // Goal: the code is exchanged form-encoded, the client authenticated
    // as chosen (Basic's id and secret form-encoded before base64), with
    // the verifier and the same redirect URI; a refusal says why.
    #[test]
    fn the_token_request_is_form_encoded_and_authenticated() {
        let v = SignInSecrets::of(HOST, &"e".repeat(64)).verifier;
        let r = token_request(ClientAuth::Basic, "my client", Some("s:e&c"), "co de", "https://p.example/auth/callback", &v);
        assert_eq!(r.authorization.as_deref(), Some(format!("Basic {}", base64::engine::general_purpose::STANDARD.encode("my+client:s%3Ae%26c")).as_str()));
        assert_eq!(r.body, format!("grant_type=authorization_code&code=co+de&redirect_uri=https%3A%2F%2Fp.example%2Fauth%2Fcallback&code_verifier={v}"));
        let r = token_request(ClientAuth::Post, "c", Some("s"), "x", "https://p.example/cb", &v);
        assert!(r.authorization.is_none() && r.body.ends_with("&client_id=c&client_secret=s"));
        let r = token_request(ClientAuth::None, "c", None, "x", "https://p.example/cb", &v);
        assert!(r.authorization.is_none() && r.body.ends_with("&client_id=c") && !r.body.contains("secret"));
        assert_eq!(id_token_of(200, br#"{"id_token":"a.b.c","access_token":"x"}"#), Ok("a.b.c".into()));
        assert!(matches!(id_token_of(200, br#"{"access_token":"x"}"#), Err(OidcError::Provider(_))));
        assert_eq!(id_token_of(400, br#"{"error":"invalid_grant","error_description":"used"}"#), Err(OidcError::Refused { status: 400, error: "invalid_grant".into(), description: "used".into() }));
        assert!(matches!(id_token_of(502, b"<html>"), Err(OidcError::Refused { status: 502, .. })));
    }

    // Goal: the browser goes to the provider with the code flow, this
    // client and callback, the scopes, the state, the nonce and PKCE's S256
    // challenge of the verifier (never the verifier), a hint when given,
    // and prompt=login for "Use another account".
    #[test]
    fn the_request_carries_pkce_state_and_nonce() {
        let s = settings();
        let state = "f".repeat(64);
        let secrets = SignInSecrets::of(HOST, &state);
        let a = |hint, again| authorize_url(&Authorize { endpoint: "https://idp.corp.example/auth?kc=1", settings: &s, redirect_uri: "https://p.example/auth/callback", state: &state, secrets: &secrets, login_hint: hint, ask_again: again });
        let to = url::Url::parse(&a(None, false)).unwrap();
        let q: std::collections::HashMap<String, String> = to.query_pairs().into_owned().collect();
        assert_eq!(q["kc"], "1", "the endpoint's own query kept");
        assert_eq!((q["response_type"].as_str(), q["client_id"].as_str(), q["redirect_uri"].as_str()), ("code", CLIENT, "https://p.example/auth/callback"));
        assert_eq!(q["scope"], DEFAULT_SCOPES);
        assert_eq!((q["state"].as_str(), q["nonce"].as_str()), (state.as_str(), secrets.nonce.as_str()));
        assert_eq!((q["code_challenge"].clone(), q["code_challenge_method"].as_str()), (crate::own_signin::challenge(&secrets.verifier), "S256"));
        assert!(!q.contains_key("prompt") && !q.contains_key("login_hint") && !to.as_str().contains(&secrets.verifier));
        let again = a(Some("jane@corp.example"), true);
        assert!(again.contains("&login_hint=jane%40corp.example") && again.ends_with("&prompt=login"));
    }

    // Goal: the email is the person's handle only when the provider
    // verified it, or the deployment says it vouches for the claim it
    // names; the settings refuse a claim email_verified does not speak for,
    // unvouched, and every setting out of shape.
    #[test]
    fn the_email_is_verified_or_vouched_for() {
        let s = settings();
        let v = |email: Option<&str>, verified| Verified { subject: "u".into(), email: email.map(str::to_string), email_verified: verified };
        assert_eq!(email_of(&v(Some(" jane@corp.example "), true), &s), Ok("jane@corp.example"));
        assert!(matches!(email_of(&v(Some("jane@corp.example"), false), &s), Err(EmailRefused::Unverified { .. })));
        assert!(matches!(email_of(&v(None, true), &s), Err(EmailRefused::Absent { .. })));
        assert!(matches!(email_of(&v(Some("  "), true), &s), Err(EmailRefused::Absent { .. })));
        let vars = Vars { issuer: Some(ISSUER), client_id: Some(CLIENT), ..Vars::default() };
        let upn = Settings::parse(Vars { email_claim: Some("upn"), vouched: Some("on"), ..vars }).unwrap().unwrap();
        assert_eq!(email_of(&v(Some("jdoe@corp.example"), false), &upn), Ok("jdoe@corp.example"), "vouched for");
        // the claim it names is the email read
        let token = rs(&claims(r#","upn":"jdoe@corp.example""#).replace(r#","email_verified":true"#, ""));
        let read = verify_with(&token, &jwks(), &upn, NONCE, NOW).unwrap();
        assert_eq!(read.email.as_deref(), Some("jdoe@corp.example"));
        assert!(matches!(Settings::parse(Vars { email_claim: Some("upn"), ..vars }), Err(OidcError::Config(_))), "upn, unvouched");
        assert!(Settings::parse(Vars { email_claim: Some("sub"), vouched: Some("on"), ..vars }).is_err(), "a claim the rules read");
        assert!(Settings::parse(Vars { vouched: Some("yes"), ..vars }).is_err());
        assert!(Settings::parse(Vars { scopes: Some("email profile"), ..vars }).is_err(), "openid is required");
        assert!(Settings::parse(Vars { auth: Some("private_key_jwt"), ..vars }).is_err());
        assert!(Settings::parse(Vars { client_id: None, ..vars }).is_err());
        assert!(Settings::parse(Vars { issuer: Some("http://idp.corp.example"), ..vars }).is_err(), "http off loopback");
        assert!(Settings::parse(Vars { issuer: Some("https://idp.corp.example/?a=1"), ..vars }).is_err());
        assert!(Settings::parse(Vars { issuer: Some("http://127.0.0.1:5556/dex"), ..vars }).unwrap().is_some());
        assert_eq!(Settings::parse(Vars::default()), Ok(None), "no issuer: WorkOS signs people in");
        assert!(Settings::parse(Vars { client_id: Some(CLIENT), ..Vars::default() }).is_err(), "a client and no issuer is a mistake, not WorkOS");
        let scoped = Settings::parse(Vars { scopes: Some("  openid   email "), ..vars }).unwrap().unwrap();
        assert_eq!(scoped.scopes, "openid email");
    }
}
