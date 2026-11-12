//! The rules' tests. The signature and the key's choice are the shim's
//! (cell/oidc.test.mjs, and the e2e through workerd and celld); here, every
//! decision on what it answers.

use super::*;

const ISSUER: &str = "https://idp.corp.example/realms/corp";
const CLIENT: &str = "fragment";
const NONCE: &str = "n-0123456789";
const NOW: i64 = 1_790_000_000;
const HOST: &str = "a host secret of at least thirty-two bytes, for tests";

fn settings() -> Settings {
    Settings::parse(Vars { issuer: Some(ISSUER), client_id: Some(CLIENT), ..Vars::default() }).unwrap().unwrap()
}

fn s(v: &str) -> Field {
    Field::Str(v.to_string())
}

fn list(v: &[&str]) -> Field {
    Field::List(v.iter().map(|x| x.to_string()).collect())
}

/// A token's claims as the shim reads them (`claim_names`' order).
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
    let claims = id_token_claims("ok", "RS256".into(), Some(values), &ALGS)?;
    verified(claims, &Expect { settings, nonce, now_s: NOW })
}

fn verify(values: Vec<Field>) -> Result<Verified, IdTokenError> {
    verify_with(values, &settings(), NONCE)
}

// Goal: an id_token the shim verified, for this client and this sign-in,
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

// Goal: whatever the shim answers but `ok` signs no one in, each as its
// error; and an `ok` under an algorithm the provider did not list (or
// none the rules take) is refused here again, whatever the shim said.
#[test]
fn the_shims_refusals_and_algorithms_are_the_rules() {
    let answer = |code: &str, detail: &str| id_token_claims(code, detail.into(), Some(fields()), &[Alg::Rs256]);
    assert_eq!(answer("signature", ""), Err(IdTokenError::Signature));
    assert_eq!(answer("alg", "none"), Err(IdTokenError::Alg("none".into())));
    assert_eq!(answer("crit", ""), Err(IdTokenError::Critical));
    assert!(matches!(answer("typ", ""), Err(IdTokenError::Malformed(_))));
    assert_eq!(answer("unknown_key", "k2"), Err(IdTokenError::UnknownKey(Some("k2".into()))));
    assert_eq!(answer("unknown_key", ""), Err(IdTokenError::UnknownKey(None)));
    assert_eq!(answer("too_large", ""), Err(IdTokenError::TooLarge));
    assert_eq!(answer("unchecked", &"x".repeat(500)), Err(IdTokenError::Unchecked("x".repeat(SAID_MAX))));
    assert!(matches!(answer("whatever", ""), Err(IdTokenError::Unchecked(_))));
    // the shim's ok, re-checked: an algorithm not allowed, none, an HMAC
    assert_eq!(answer("ok", "ES256"), Err(IdTokenError::Alg("ES256".into())), "the provider listed RS256 alone");
    assert_eq!(answer("ok", "none"), Err(IdTokenError::Alg("none".into())));
    assert_eq!(answer("ok", "HS256"), Err(IdTokenError::Alg("HS256".into())));
    assert!(answer("ok", "RS256").is_ok());
    assert!(matches!(id_token_claims("ok", "RS256".into(), None, &ALGS), Err(IdTokenError::Malformed(_))));
    assert_eq!(alg_names(&ALGS), vec!["RS256", "ES256"]);
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
    let first = SignInSecrets::of(HOST, &"a".repeat(64));
    let second = SignInSecrets::of(HOST, &"b".repeat(64));
    let token = with(7, s(&first.nonce));
    assert!(verify_with(token.clone(), &settings(), &first.nonce).is_ok());
    assert_eq!(verify_with(token, &settings(), &second.nonce), Err(IdTokenError::Nonce));
}

// Goal: a sign-in's verifier and nonce are the platform's alone and its
// own: the same for its state under one host secret (a restart in the
// middle finishes), another for another state or host secret, a verifier
// RFC 7636 takes, and its challenge S256's (RFC 7636's vector).
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

// Goal: a provider's metadata is taken only from the issuer it names,
// its endpoints https (http only beside a loopback issuer), signing with
// RS256 or ES256, not refusing S256; and the client's authentication is
// chosen from what it lists, never without a secret.
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
    assert_eq!(p.client_auth(None, true), Ok(ClientAuth::Basic), "Basic first when it lists both");
    assert_eq!(p.client_auth(None, false), Ok(ClientAuth::None), "no secret: a public client");
    assert_eq!(p.client_auth(Some(ClientAuth::Post), true), Ok(ClientAuth::Post));
    assert!(matches!(p.client_auth(Some(ClientAuth::Basic), false), Err(OidcError::Config(_))));
    let post_only = Provider::from_fields(ISSUER, changed(ISSUER, 5, list(&["client_secret_post"]))).unwrap();
    assert_eq!(post_only.client_auth(None, true), Ok(ClientAuth::Post));
    let lists_none = Provider::from_fields(ISSUER, changed(ISSUER, 5, Field::Absent)).unwrap();
    assert_eq!(lists_none.client_auth(None, true), Ok(ClientAuth::Basic), "the spec's default");
    let jwt_only = Provider::from_fields(ISSUER, changed(ISSUER, 5, list(&["private_key_jwt"]))).unwrap();
    assert!(jwt_only.client_auth(None, true).is_err());
}

// Goal: the code is exchanged form-encoded, the client authenticated as
// chosen (Basic's id and secret form-encoded before base64), with the
// verifier and the same redirect URI; a refusal says why.
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
}

// Goal: the browser goes to the provider with the code flow, this client
// and callback, the scopes, the state, the nonce and PKCE's S256 challenge
// of the verifier (never the verifier), a hint when given, and
// prompt=login for "Use another account".
#[test]
fn the_request_carries_pkce_state_and_nonce() {
    let st = settings();
    let state = "f".repeat(64);
    let secrets = SignInSecrets::of(HOST, &state);
    let a = |hint, again| authorize_url(&Authorize { endpoint: "https://idp.corp.example/auth?kc=1", settings: &st, redirect_uri: "https://p.example/auth/callback", state: &state, secrets: &secrets, login_hint: hint, ask_again: again });
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

// Goal: the email is the person's handle only when the provider verified
// it, or the deployment says it vouches for the claim it names; the
// settings refuse a claim email_verified does not speak for, unvouched,
// and every setting out of shape.
#[test]
fn the_email_is_verified_or_vouched_for() {
    let st = settings();
    let v = |email: Option<&str>, verified| Verified { subject: "u".into(), email: email.map(str::to_string), email_verified: verified };
    assert_eq!(email_of(&v(Some(" jane@corp.example "), true), &st), Ok("jane@corp.example"));
    assert!(matches!(email_of(&v(Some("jane@corp.example"), false), &st), Err(EmailRefused::Unverified { .. })));
    assert!(matches!(email_of(&v(None, true), &st), Err(EmailRefused::Absent { .. })));
    assert!(matches!(email_of(&v(Some("  "), true), &st), Err(EmailRefused::Absent { .. })));
    let vars = Vars { issuer: Some(ISSUER), client_id: Some(CLIENT), ..Vars::default() };
    let upn = Settings::parse(Vars { email_claim: Some("upn"), vouched: Some("on"), ..vars }).unwrap().unwrap();
    assert_eq!(email_of(&v(Some("jdoe@corp.example"), false), &upn), Ok("jdoe@corp.example"), "vouched for");
    assert_eq!(claim_names(&upn)[8], "upn", "the claim it names is the email the shim reads");
    let read = verify_with(with(9, Field::Absent), &upn, NONCE).unwrap();
    assert!(!read.email_verified && email_of(&read, &upn).is_ok());
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

// Goal: what the shim answers of a code's redemption is decided here: the
// token endpoint's refusal (a 400 is the code's), the provider's faults
// (no answer, no id_token, keys that cannot be had), an id_token not to
// trust, and a good one verified for this sign-in.
#[test]
fn a_redemption_is_decided_by_the_rules() {
    let st = settings();
    let expect = Expect { settings: &st, nonce: NONCE, now_s: NOW };
    let r = |code: &str, detail: &str, values: Option<Vec<Field>>| redeemed(code, detail.into(), values, &ALGS, &expect);
    assert_eq!(
        r("token_refused", "400", Some(vec![s("invalid_grant"), s("used")])),
        Err(Refusal::Code(OidcError::Refused { status: 400, error: "invalid_grant".into(), description: "used".into() }))
    );
    assert_eq!(r("token_refused", "502", None), Err(Refusal::Code(OidcError::Refused { status: 502, error: String::new(), description: String::new() })));
    for code in ["token_fetch", "token_answer", "fetch", "jwks"] {
        assert_eq!(r(code, "why", None), Err(Refusal::Provider("why".into())), "{code}");
    }
    assert_eq!(r("signature", "", None), Err(Refusal::IdToken(IdTokenError::Signature)));
    assert_eq!(r("ok", "RS256", Some(with(7, s("n-another"))), ), Err(Refusal::IdToken(IdTokenError::Nonce)));
    assert_eq!(r("ok", "RS256", Some(fields())).map(|v| v.subject), Ok("user-1".into()));
    assert_eq!(SHIM_LIMITS, [METADATA_MAX_BYTES as i64, FETCH_TIMEOUT_MS, METADATA_TTL_MS, JWKS_REFETCH_MIN_MS, ID_TOKEN_MAX_BYTES as i64]);
}
