//! An id_token's signature (a compact JWS, RFC 7515), checked by a key of
//! the provider's JWKS (RFC 7517) with ring: RSASSA-PKCS1-v1_5 SHA-256
//! (RS256, keys of 2048 to 4096 bits) and ECDSA P-256 SHA-256 (ES256),
//! nothing else. ring is the crypto beneath the door's TLS already; no
//! signature arithmetic is the door's own.
//!
//! The key is chosen by the token's `kid`, among the JWKS's keys of the
//! algorithm's type; a token naming no `kid` takes the one such key, or
//! none when there are several. A key the token carries (`jwk`, `jku`,
//! `x5u`, `x5c`) is never read. A header with `crit` is refused (no
//! extension is understood), and so is an access token (`typ: at+jwt`).

use serde_json::{Map, Value};

use crate::oidc::{malformed, Alg, Field, IdTokenError, ID_TOKEN_MAX_BYTES};

/// The most keys of a JWKS read (a provider publishes two or three).
pub const JWKS_KEYS_MAX: usize = 64;
/// RSA moduli taken, in bits.
const RSA_BITS_MIN: usize = 2048;
const RSA_BITS_MAX: usize = 4096;

/// A public key the provider publishes for signatures.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Key {
    Rsa { n: Vec<u8>, e: Vec<u8> },
    /// An uncompressed P-256 point: 0x04, x, y.
    P256 { point: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Jwk {
    kid: Option<String>,
    /// The algorithm it names, if any (a key for RS256 alone, say).
    alg: Option<String>,
    key: Key,
}

/// The provider's keys, as many as the door can use: RSA and P-256 keys
/// for signatures; any other (`oct`, other curves, `use: enc`) is skipped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Jwks {
    keys: Vec<Jwk>,
}

fn b64(s: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s).ok()
}

impl Jwks {
    /// A JWKS document (`{"keys": [...]}`), or why it is none.
    pub fn parse(bytes: &[u8]) -> Result<Jwks, String> {
        let doc: Value = serde_json::from_slice(bytes).map_err(|e| format!("its JWKS is not JSON: {e}"))?;
        let Some(listed) = doc.get("keys").and_then(Value::as_array) else {
            return Err("its JWKS has no keys list".into());
        };
        if listed.len() > JWKS_KEYS_MAX {
            return Err(format!("its JWKS holds more than {JWKS_KEYS_MAX} keys"));
        }
        let text = |k: &Value, name: &str| k.get(name).and_then(Value::as_str).map(str::to_string);
        let mut keys = Vec::with_capacity(listed.len());
        for k in listed {
            if text(k, "use").is_some_and(|u| u != "sig") {
                continue;
            }
            let key = match (text(k, "kty").as_deref(), text(k, "crv").as_deref()) {
                (Some("RSA"), _) => match (text(k, "n").as_deref().and_then(b64), text(k, "e").as_deref().and_then(b64)) {
                    (Some(n), Some(e)) => Key::Rsa { n: n.into_iter().skip_while(|b| *b == 0).collect(), e },
                    _ => return Err("an RSA key of its JWKS has no n or e".into()),
                },
                (Some("EC"), Some("P-256")) => match (text(k, "x").as_deref().and_then(b64), text(k, "y").as_deref().and_then(b64)) {
                    (Some(x), Some(y)) if x.len() == 32 && y.len() == 32 => Key::P256 { point: [&[4u8][..], &x, &y].concat() },
                    _ => return Err("a P-256 key of its JWKS has no 32-byte x and y".into()),
                },
                _ => continue,
            };
            keys.push(Jwk { kid: text(k, "kid"), alg: text(k, "alg"), key });
        }
        Ok(Jwks { keys })
    }

    /// Whether a key of this `kid` is published.
    pub fn holds(&self, kid: &str) -> bool {
        self.keys.iter().any(|k| k.kid.as_deref() == Some(kid))
    }

    /// The key a token signed `alg` by `kid` is checked with.
    fn key_for(&self, alg: Alg, kid: Option<&str>) -> Result<&Key, IdTokenError> {
        let fits = |k: &&Jwk| {
            let kind = matches!((&k.key, alg), (Key::Rsa { .. }, Alg::Rs256) | (Key::P256 { .. }, Alg::Es256));
            kind && k.alg.as_deref().is_none_or(|a| a == alg.name())
        };
        let mut fitting = self.keys.iter().filter(fits);
        match kid {
            Some(kid) => fitting.find(|k| k.kid.as_deref() == Some(kid)).map(|k| &k.key).ok_or_else(|| IdTokenError::UnknownKey(Some(kid.into()))),
            None => match (fitting.next(), fitting.next()) {
                (Some(only), None) => Ok(&only.key),
                _ => Err(IdTokenError::UnknownKey(None)),
            },
        }
    }
}

/// A token's header, as much as is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub alg: Alg,
    pub kid: Option<String>,
}

/// The parts of a compact JWS: header, payload, signature, each base64url.
fn parts(token: &str) -> Result<[&str; 3], IdTokenError> {
    if token.len() > ID_TOKEN_MAX_BYTES {
        return Err(IdTokenError::TooLarge);
    }
    let mut it = token.split('.');
    match (it.next(), it.next(), it.next(), it.next()) {
        (Some(h), Some(p), Some(s), None) if !h.is_empty() && !p.is_empty() => Ok([h, p, s]),
        _ => Err(malformed("it is not three base64url parts")),
    }
}

fn object(part: &str, what: &str) -> Result<Map<String, Value>, IdTokenError> {
    let bytes = b64(part).ok_or_else(|| malformed(&format!("its {what} is not base64url")))?;
    match serde_json::from_slice(&bytes) {
        Ok(Value::Object(o)) => Ok(o),
        _ => Err(malformed(&format!("its {what} is not a JSON object"))),
    }
}

/// A token's header, checked: an algorithm the rules and the provider
/// allow (`algs`), no `crit`, not an access token.
pub fn header_of(token: &str, algs: &[Alg]) -> Result<Header, IdTokenError> {
    let [h, _, _] = parts(token)?;
    let header = object(h, "header")?;
    let alg = header.get("alg").and_then(Value::as_str).ok_or_else(|| malformed("its header names no alg"))?;
    let alg = Alg::parse(alg).filter(|a| algs.contains(a)).ok_or_else(|| IdTokenError::Alg(crate::oidc::cut(alg)))?;
    if header.contains_key("crit") {
        return Err(IdTokenError::Critical);
    }
    if header.get("typ").and_then(Value::as_str).is_some_and(|t| t.eq_ignore_ascii_case("at+jwt") || t.eq_ignore_ascii_case("application/at+jwt")) {
        return Err(malformed("it is an access token (typ at+jwt)"));
    }
    let kid = match header.get("kid") {
        None => None,
        Some(Value::String(k)) => Some(k.clone()),
        Some(_) => return Err(malformed("its kid is not a string")),
    };
    Ok(Header { alg, kid })
}

/// The token's claims, once its signature verified by a key of `jwks`
/// under its header's algorithm (`header_of`'s checks first).
pub fn verify(token: &str, jwks: &Jwks, algs: &[Alg]) -> Result<(Alg, Map<String, Value>), IdTokenError> {
    let header = header_of(token, algs)?;
    let [h, p, s] = parts(token)?;
    let signature = b64(s).ok_or_else(|| malformed("its signature is not base64url"))?;
    let signed = &token.as_bytes()[..h.len() + 1 + p.len()];
    let checked = match jwks.key_for(header.alg, header.kid.as_deref())? {
        Key::Rsa { n, e } => {
            let bits = n.len() * 8;
            if !(RSA_BITS_MIN..=RSA_BITS_MAX).contains(&bits) {
                return Err(IdTokenError::Unchecked(format!("its RSA key is {bits} bits, not {RSA_BITS_MIN} to {RSA_BITS_MAX}")));
            }
            ring::signature::RsaPublicKeyComponents { n, e }.verify(&ring::signature::RSA_PKCS1_2048_8192_SHA256, signed, &signature)
        }
        Key::P256 { point } => {
            if signature.len() != 64 {
                return Err(IdTokenError::Signature);
            }
            ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_FIXED, point).verify(signed, &signature)
        }
    };
    if checked.is_err() {
        return Err(IdTokenError::Signature);
    }
    Ok((header.alg, object(p, "payload")?))
}

/// `names`' values in a JSON object, as the rules read them.
pub fn fields_of(o: &Map<String, Value>, names: &[&str]) -> Vec<Field> {
    names
        .iter()
        .map(|n| match o.get(*n) {
            None | Some(Value::Null) => Field::Absent,
            Some(Value::String(s)) => Field::Str(s.clone()),
            Some(Value::Number(n)) => Field::Num(n.as_f64().unwrap_or(f64::NAN)),
            Some(Value::Bool(b)) => Field::Bool(*b),
            Some(Value::Array(a)) if a.iter().all(Value::is_string) => Field::List(a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()),
            Some(_) => Field::Other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALGS: [Alg; 2] = [Alg::Rs256, Alg::Es256];

    fn token(header: &Value, payload: &Value, sig: &[u8]) -> String {
        format!("{}.{}.{}", crate::oidc::b64url(header.to_string().as_bytes()), crate::oidc::b64url(payload.to_string().as_bytes()), crate::oidc::b64url(sig))
    }

    /// A P-256 key pair from ring, and its JWK.
    fn p256(kid: &str) -> (ring::signature::EcdsaKeyPair, Value) {
        use ring::signature::KeyPair;
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(&ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let pair = ring::signature::EcdsaKeyPair::from_pkcs8(&ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng).unwrap();
        let point = pair.public_key().as_ref().to_vec();
        let jwk = serde_json::json!({ "kty": "EC", "crv": "P-256", "use": "sig", "kid": kid, "x": crate::oidc::b64url(&point[1..33]), "y": crate::oidc::b64url(&point[33..]) });
        (pair, jwk)
    }

    fn signed(pair: &ring::signature::EcdsaKeyPair, header: &Value, payload: &Value) -> String {
        let input = format!("{}.{}", crate::oidc::b64url(header.to_string().as_bytes()), crate::oidc::b64url(payload.to_string().as_bytes()));
        let sig = pair.sign(&ring::rand::SystemRandom::new(), input.as_bytes()).unwrap();
        format!("{input}.{}", crate::oidc::b64url(sig.as_ref()))
    }

    /// Goal: an ES256 token verifies by its kid's key, and by the one key
    /// of its kind when it names none; a changed byte, another key, or a
    /// kid no key holds does not. (RS256 is proven against the fake
    /// provider's keys: tests/door.rs.)
    #[test]
    fn a_signature_verifies_by_its_key_alone() {
        let (a, ja) = p256("a");
        let (b, jb) = p256("b");
        let jwks = Jwks::parse(serde_json::json!({ "keys": [ja.clone(), jb, { "kty": "oct", "k": "c2VjcmV0" }] }).to_string().as_bytes()).unwrap();
        assert!(jwks.holds("a") && jwks.holds("b") && !jwks.holds("c"), "the oct key is skipped");
        let claims = serde_json::json!({ "sub": "u", "n": 1 });
        let good = signed(&a, &serde_json::json!({ "alg": "ES256", "kid": "a" }), &claims);
        let (alg, payload) = verify(&good, &jwks, &ALGS).unwrap();
        assert_eq!((alg, payload["sub"].clone()), (Alg::Es256, serde_json::json!("u")));
        let lying = signed(&b, &serde_json::json!({ "alg": "ES256", "kid": "a" }), &claims);
        assert_eq!(verify(&lying, &jwks, &ALGS), Err(IdTokenError::Signature), "another key, under a's kid");
        let (head, rest) = good.split_once('.').unwrap();
        let tampered = format!("{head}.{}{}", crate::oidc::b64url(serde_json::json!({ "sub": "v", "n": 1 }).to_string().as_bytes()), &rest[rest.find('.').unwrap()..]);
        assert_eq!(verify(&tampered, &jwks, &ALGS), Err(IdTokenError::Signature));
        let unknown = signed(&a, &serde_json::json!({ "alg": "ES256", "kid": "z" }), &claims);
        assert_eq!(verify(&unknown, &jwks, &ALGS), Err(IdTokenError::UnknownKey(Some("z".into()))));
        let no_kid = signed(&a, &serde_json::json!({ "alg": "ES256" }), &claims);
        assert_eq!(verify(&no_kid, &jwks, &ALGS), Err(IdTokenError::UnknownKey(None)), "two keys could be meant");
        let one = Jwks::parse(serde_json::json!({ "keys": [ja] }).to_string().as_bytes()).unwrap();
        assert!(verify(&no_kid, &one, &ALGS).is_ok(), "the one key of its kind");
    }

    /// Goal: a header the rules refuse is refused before any key is used:
    /// `none`, an HMAC, an algorithm the provider did not list, `crit`, an
    /// access token, a malformed token, an oversized one.
    #[test]
    fn a_header_is_checked_first() {
        let payload = serde_json::json!({ "sub": "u" });
        let jwks = Jwks::default();
        let alg = |a: &str| verify(&token(&serde_json::json!({ "alg": a }), &payload, b"x"), &jwks, &ALGS);
        assert_eq!(alg("none"), Err(IdTokenError::Alg("none".into())));
        assert_eq!(alg("HS256"), Err(IdTokenError::Alg("HS256".into())));
        assert_eq!(verify(&token(&serde_json::json!({ "alg": "ES256" }), &payload, b"x"), &jwks, &[Alg::Rs256]), Err(IdTokenError::Alg("ES256".into())));
        assert_eq!(verify(&token(&serde_json::json!({ "alg": "RS256", "crit": ["b64"] }), &payload, b"x"), &jwks, &ALGS), Err(IdTokenError::Critical));
        assert!(matches!(verify(&token(&serde_json::json!({ "alg": "RS256", "typ": "at+jwt" }), &payload, b"x"), &jwks, &ALGS), Err(IdTokenError::Malformed(_))));
        assert!(matches!(verify("a.b", &jwks, &ALGS), Err(IdTokenError::Malformed(_))));
        assert!(matches!(verify("a.b.c.d", &jwks, &ALGS), Err(IdTokenError::Malformed(_))));
        assert_eq!(verify(&"a".repeat(ID_TOKEN_MAX_BYTES + 1), &jwks, &ALGS), Err(IdTokenError::TooLarge));
        assert!(Jwks::parse(b"{}").is_err() && Jwks::parse(b"[]").is_err());
        assert!(Jwks::parse(serde_json::json!({ "keys": [{ "kty": "RSA", "n": "AQAB" }] }).to_string().as_bytes()).is_err(), "an RSA key with no e");
    }
}
