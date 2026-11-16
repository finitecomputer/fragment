//! The rules' tests, brought over from fragment#1244 with them. The
//! signature and the key's choice are `jws.rs`'s (its tests, and the
//! door's against the fake provider); here, every decision on the claims.

use super::*;

const ISSUER: &str = "https://idp.corp.example/realms/corp";
const CLIENT: &str = "fragment";
const NONCE: &str = "n-0123456789";
const NOW: i64 = 1_790_000_000;
const SECRET: &[u8] = b"a door secret of at least thirty-two bytes, for tests";

fn vars() -> Vars<'static> {
    Vars { issuer: ISSUER, client_id: CLIENT, ..Vars::default() }
}

fn settings() -> Settings {
    Settings::parse(vars()).unwrap()
}

fn s(v: &str) -> Field {
    Field::Str(v.to_string())
}

fn list(v: &[&str]) -> Field {
    Field::List(v.iter().map(|x| x.to_string()).collect())
}

/// A token's claims as `jws::fields_of` reads them (`claim_names`' order).
fn fields() -> Vec<Field> {
    vec![
        s(ISSUER),
        s("user-1"),
        s(CLIENT),
        Field::Absent,
        Field::Num((NOW + 300) as f64),
        Field::Num(NOW as f64),
        Field::Absent,
        s(NONCE),
        s("Jane@Corp.Example"),
        Field::Bool(true),
    ]
}

fn with(i: usize, f: Field) -> Vec<Field> {
    let mut v = fields();
    v[i] = f;
    v
}

const ALGS: [Alg; 2] = [Alg::Rs256, Alg::Es256];

fn verify_with(values: Vec<Field>, settings: &Settings, nonce: &str) -> Result<Verified, IdTokenError> {
    let claims = claims_of(Alg::Rs256, values, &ALGS)?;
    verified(claims, &Expect { settings, nonce, now_s: NOW })
}

fn verify(values: Vec<Field>) -> Result<Verified, IdTokenError> {
    verify_with(values, &settings(), NONCE)
}

// Goal: an id_token that verified, for this client and this sign-in,
// signs in, its subject and email read; several audiences with this
// client the authorized party; a fractional time floored.
#[test]
fn valid_claims_verify() {
    assert_eq!(verify(fields()), Ok(Verified { subject: "user-1".into(), email: Some("Jane@Corp.Example".into()), email_verified: true }));
    let several = verify(with(3, s(CLIENT)).into_iter().enumerate().map(|(i, f)| if i == 2 { list(&[CLIENT, "other"]) } else { f }).collect());
    assert!(several.is_ok(), "{several:?}");
    assert!(verify(with(5, Field::Num(NOW as f64 + 0.75))).is_ok());
    assert_eq!(claim_names(&settings())[8], "email");
}

// Goal: a signature that verified under an algorithm the provider did not
// list is refused here again; claims read without their fields too.
#[test]
fn the_algorithm_is_checked_again() {
    assert_eq!(claims_of(Alg::Es256, fields(), &[Alg::Rs256]), Err(IdTokenError::Alg("ES256".into())), "the provider listed RS256 alone");
    assert!(claims_of(Alg::Rs256, fields(), &[Alg::Rs256]).is_ok());
    assert!(matches!(claims_of(Alg::Rs256, vec![s(ISSUER)], &ALGS), Err(IdTokenError::Malformed(_))));
    assert_eq!(Alg::parse("none"), None);
    assert_eq!(Alg::parse("HS256"), None);
}

// Goal: each claim this sign-in must see is checked: issuer, audience,
// authorized party, the times, the nonce, the subject; and each claim of
// the wrong type is refused, not read as absent.
#[test]
fn wrong_claims_are_refused() {
    assert_eq!(verify(with(0, s("https://idp.corp.example/realms/other"))), Err(IdTokenError::Issuer));
    assert_eq!(verify(with(0, Field::Absent)), Err(IdTokenError::Issuer));
    assert_eq!(verify(with(2, s("another"))), Err(IdTokenError::Audience));
    assert_eq!(verify(with(2, Field::List(vec![]))), Err(IdTokenError::Audience));
    assert_eq!(verify(with(2, list(&[CLIENT, "other"]))), Err(IdTokenError::AuthorizedParty));
    assert_eq!(verify(with(3, s("other"))), Err(IdTokenError::AuthorizedParty));
    assert_eq!(verify(with(4, Field::Num((NOW - SKEW_S) as f64))), Err(IdTokenError::Expired));
    assert!(verify(with(4, Field::Num((NOW - SKEW_S + 1) as f64))).is_ok(), "within the skew");
    assert_eq!(verify(with(5, Field::Num((NOW + SKEW_S + 1) as f64))), Err(IdTokenError::NotYetValid));
    assert_eq!(verify(with(6, Field::Num((NOW + SKEW_S + 1) as f64))), Err(IdTokenError::NotYetValid));
    assert_eq!(verify(with(5, Field::Num((NOW - ID_TOKEN_AGE_MAX_S - SKEW_S - 1) as f64))), Err(IdTokenError::TooOld));
    assert_eq!(verify(with(7, s("n-another-sign-in"))), Err(IdTokenError::Nonce));
    assert_eq!(verify(with(1, s(""))), Err(IdTokenError::Subject));
    assert_eq!(verify(with(1, s(&"u".repeat(SUBJECT_MAX + 1)))), Err(IdTokenError::Subject));
    assert_eq!(verify(with(1, Field::Absent)), Err(IdTokenError::Subject));
    let wrong = [(4, s("soon")), (4, Field::Absent), (5, Field::Absent), (4, Field::Num(f64::INFINITY)), (0, Field::Num(1.0)), (2, Field::Other), (9, s("true")), (8, Field::Bool(true))];
    for (i, f) in wrong {
        assert!(matches!(verify(with(i, f.clone())), Err(IdTokenError::Malformed(_))), "claim {i} as {f:?}");
    }
}

// Goal: an id_token one sign-in received signs no one in through another
// (a replay): its nonce is the first's, and the second's is derived from
// its own state.
#[test]
fn a_replayed_token_signs_no_one_in() {
    let first = SignInSecrets::of(SECRET, "state-a");
    let second = SignInSecrets::of(SECRET, "state-b");
    let token = with(7, s(&first.nonce));
    assert!(verify_with(token.clone(), &settings(), &first.nonce).is_ok());
    assert_eq!(verify_with(token, &settings(), &second.nonce), Err(IdTokenError::Nonce));
}

// Goal: a sign-in's verifier and nonce are the door's alone and its own:
// the same for its state under one secret (a restart in the middle
// finishes), another for another state or secret, a verifier RFC 7636
// takes, and its challenge S256's (RFC 7636's vector).
#[test]
fn a_sign_ins_secrets_are_derived_from_the_doors_secret_and_its_state() {
    let a = SignInSecrets::of(SECRET, "state-c");
    assert_eq!(a, SignInSecrets::of(SECRET, "state-c"), "a restart derives the same");
    assert_ne!(a, SignInSecrets::of(SECRET, "state-d"));
    assert_ne!(a, SignInSecrets::of(b"another door secret of thirty-two bytes or more", "state-c"));
    assert!(valid_verifier(&a.verifier) && a.nonce.len() == 43 && a.nonce != a.verifier);
    assert_eq!(challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
}

fn discovery(issuer: &str) -> Vec<Field> {
    vec![
        s(issuer),
        s(&format!("{issuer}/authorize")),
        s(&format!("{issuer}/token")),
        s(&format!("{issuer}/jwks")),
        list(&["RS256", "HS256"]),
        list(&["client_secret_post", "client_secret_basic"]),
        list(&["S256", "plain"]),
    ]
}

fn changed(issuer: &str, i: usize, f: Field) -> Vec<Field> {
    let mut d = discovery(issuer);
    d[i] = f;
    d
}

// Goal: a provider's metadata is taken only from the issuer it names, its
// endpoints https (http only beside a loopback issuer), signing with RS256
// or ES256, not refusing S256; and the client's authentication is chosen
// from what it lists.
#[test]
fn discovery_is_checked() {
    let p = Provider::from_fields(ISSUER, discovery(ISSUER)).unwrap();
    assert_eq!(p.algs, vec![Alg::Rs256], "HS256 is never taken");
    assert_eq!(p.token_endpoint, format!("{ISSUER}/token"));
    assert_eq!(discovery_url(&format!("{ISSUER}/")), format!("{ISSUER}/.well-known/openid-configuration"));
    assert!(Provider::from_fields("https://idp.corp.example/realms/other", discovery(ISSUER)).is_err(), "another issuer's");
    assert!(Provider::from_fields(ISSUER, changed(ISSUER, 2, s("http://idp.corp.example/token"))).is_err());
    assert!(Provider::from_fields(ISSUER, changed(ISSUER, 3, Field::Absent)).is_err());
    assert!(Provider::from_fields(ISSUER, changed(ISSUER, 3, Field::Num(1.0))).is_err());
    let dev = "http://127.0.0.1:5556";
    assert!(Provider::from_fields(dev, discovery(dev)).is_ok(), "http beside a loopback issuer");
    assert!(Provider::from_fields(dev, changed(dev, 3, s("http://10.0.0.1/jwks"))).is_err());
    assert!(Provider::from_fields(ISSUER, changed(ISSUER, 4, list(&["HS256"]))).is_err());
    assert_eq!(Provider::from_fields(ISSUER, changed(ISSUER, 4, Field::Absent)).unwrap().algs, vec![Alg::Rs256], "RS256, the default");
    assert!(Provider::from_fields(ISSUER, changed(ISSUER, 4, s("RS256"))).is_err(), "a list, not a string");
    assert!(Provider::from_fields(ISSUER, changed(ISSUER, 6, list(&["plain"]))).is_err());
    assert!(Provider::from_fields(ISSUER, vec![s(ISSUER)]).is_err(), "read without its fields");
    assert_eq!(p.client_auth(None), Ok(ClientAuth::Basic), "Basic first when it lists both");
    assert_eq!(p.client_auth(Some(ClientAuth::Post)), Ok(ClientAuth::Post));
    let post_only = Provider::from_fields(ISSUER, changed(ISSUER, 5, list(&["client_secret_post"]))).unwrap();
    assert_eq!(post_only.client_auth(None), Ok(ClientAuth::Post));
    let lists_none = Provider::from_fields(ISSUER, changed(ISSUER, 5, Field::Absent)).unwrap();
    assert_eq!(lists_none.client_auth(None), Ok(ClientAuth::Basic), "the spec's default");
    let jwt_only = Provider::from_fields(ISSUER, changed(ISSUER, 5, list(&["private_key_jwt"]))).unwrap();
    assert!(jwt_only.client_auth(None).is_err());
}

// Goal: the code is exchanged form-encoded, the client authenticated as
// chosen (Basic's id and secret form-encoded before base64), with the
// verifier and the same redirect URI.
#[test]
fn the_token_request_is_form_encoded_and_authenticated() {
    let v = SignInSecrets::of(SECRET, "state-e").verifier;
    let r = token_request(ClientAuth::Basic, "my client", "s:e&c", "co de", "https://door.example/oidc/callback", &v);
    assert_eq!(r.authorization.as_deref(), Some(format!("Basic {}", base64::engine::general_purpose::STANDARD.encode("my+client:s%3Ae%26c")).as_str()));
    assert_eq!(r.body, format!("grant_type=authorization_code&code=co+de&redirect_uri=https%3A%2F%2Fdoor.example%2Foidc%2Fcallback&code_verifier={v}"));
    let r = token_request(ClientAuth::Post, "c", "s", "x", "https://door.example/cb", &v);
    assert!(r.authorization.is_none() && r.body.ends_with("&client_id=c&client_secret=s"));
}

// Goal: the browser goes to the provider with the code flow, this client
// and callback, the scopes, the state, the nonce and PKCE's S256 challenge
// of the verifier (never the verifier, and always S256: tsidp assumes
// `plain` without it), and a hint when given.
#[test]
fn the_request_carries_pkce_state_and_nonce() {
    let st = settings();
    let state = "the-doors-state";
    let secrets = SignInSecrets::of(SECRET, state);
    let a = |hint| authorize_url(&Authorize { endpoint: "https://idp.corp.example/auth?kc=1", settings: &st, redirect_uri: "https://door.example/oidc/callback", state, secrets: &secrets, login_hint: hint });
    let to = url::Url::parse(&a(None)).unwrap();
    let q: std::collections::HashMap<String, String> = to.query_pairs().into_owned().collect();
    assert_eq!(q["kc"], "1", "the endpoint's own query kept");
    assert_eq!((q["response_type"].as_str(), q["client_id"].as_str(), q["redirect_uri"].as_str()), ("code", CLIENT, "https://door.example/oidc/callback"));
    assert_eq!(q["scope"], DEFAULT_SCOPES);
    assert_eq!((q["state"].as_str(), q["nonce"].as_str()), (state, secrets.nonce.as_str()));
    assert_eq!((q["code_challenge"].clone(), q["code_challenge_method"].as_str()), (challenge(&secrets.verifier), "S256"));
    assert!(!q.contains_key("prompt") && !q.contains_key("login_hint") && !to.as_str().contains(&secrets.verifier));
    assert!(a(Some("jane@corp.example")).ends_with("&login_hint=jane%40corp.example"));
}

// Goal: every provider setting out of shape is refused at start, and a
// claim email_verified does not speak for is taken only beside vouched
// domains.
#[test]
fn settings_are_checked() {
    let upn = Settings::parse(Vars { email_claim: Some("upn"), vouches: true, ..vars() }).unwrap();
    assert_eq!(claim_names(&upn)[8], "upn", "the claim it names is the email read");
    assert!(matches!(Settings::parse(Vars { email_claim: Some("upn"), ..vars() }), Err(OidcError::Config(_))), "upn, with no domain vouched for");
    assert!(Settings::parse(Vars { email_claim: Some("sub"), vouches: true, ..vars() }).is_err(), "a claim the rules read");
    assert!(Settings::parse(Vars { scopes: Some("email profile"), ..vars() }).is_err(), "openid is required");
    assert!(Settings::parse(Vars { auth: Some("private_key_jwt"), ..vars() }).is_err());
    assert!(Settings::parse(Vars { auth: Some("none"), ..vars() }).is_err(), "the door is a confidential client");
    assert!(Settings::parse(Vars { client_id: "", ..vars() }).is_err());
    assert!(Settings::parse(Vars { issuer: "http://idp.corp.example", ..vars() }).is_err(), "http off loopback");
    assert!(Settings::parse(Vars { issuer: "https://idp.corp.example/?a=1", ..vars() }).is_err());
    assert!(Settings::parse(Vars { issuer: "http://127.0.0.1:5556/dex", ..vars() }).is_ok());
    assert_eq!(Settings::parse(Vars { scopes: Some("  openid   email "), ..vars() }).unwrap().scopes, "openid email");
}
