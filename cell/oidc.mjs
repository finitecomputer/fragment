// Sign-in by an OpenID Connect provider (docs/self-host.md, seam 4): what
// JavaScript does for free, so the cell's Wasm need not. The router
// (cell/src/oidc.rs) hands this the provider's URLs, the token request
// keys.rs made, and the limits (`fragment_core::oidc::SHIM_LIMITS`, passed
// in, never copied here: [answer bytes, answer ms, kept ms, ms between two
// fetches of the JWKS for a key it lacks, id_token bytes]); this fetches
// the provider's metadata and keys (bounded, cached per isolate), sends the
// token request, parses the answers (JSON.parse), picks the id_token's key
// and checks its signature (WebCrypto), and answers flat values. Every decision about them
// is Rust's (`fragment_core::oidc`): the issuer, the endpoints, the
// algorithms (asked again there), the audience, the times, the nonce, the
// subject and the email.
//
// A value is answered as it came when it is a string, a number, a boolean,
// or a list of strings (an Array); `undefined` when absent; `null` when it
// is anything else, which Rust refuses where it reads that field.
//
// The caches (`kept`), so their contract. Source: the provider's
// metadata and its JWKS. Invalidation: each fetched again once `ttlMs` old
// (or on a clock run backwards); the JWKS also when a token names a key it
// lacks (a rotation): at once, then not again for such a key within
// `refetchMinMs`. Stale reads: a key the provider withdrew is trusted
// until its copy is `ttlMs` old; only the provider's own token endpoint
// hands the cell an id_token, so a withdrawn key would also need that
// endpoint to sign with it. A new isolate starts with neither.

// The only algorithms ever verified, whatever the caller allows: never
// `none`, never an HMAC (keyed by the client's secret, or a public key
// misread as one).
const ALGS = { RS256: "RSA", ES256: "EC" };
const KEYS_MAX = 32;
const RSA_BITS_MIN = 2048;
const RSA_BITS_MAX = 4096;
const ES256_SIGNATURE_BYTES = 64;
const SAID_MAX = 200;
// One deployment's provider has two documents: more is a bug, not a cache.
const KEPT_MAX = 4;

const said = (s) => String(s).slice(0, SAID_MAX);

function bytes(b64url) {
  if (typeof b64url !== "string" || !/^[A-Za-z0-9_-]*$/.test(b64url)) return null;
  const b64 = b64url.replace(/-/g, "+").replace(/_/g, "/");
  try {
    return Uint8Array.from(atob(b64 + "=".repeat((4 - (b64.length % 4)) % 4)), (c) => c.charCodeAt(0));
  } catch {
    return null;
  }
}

function object(text) {
  let v;
  try {
    v = JSON.parse(text);
  } catch {
    return null;
  }
  return v !== null && typeof v === "object" && !Array.isArray(v) ? v : null;
}

function value(v) {
  if (v === undefined) return undefined;
  if (typeof v === "string" || typeof v === "number" || typeof v === "boolean") return v;
  if (Array.isArray(v) && v.every((x) => typeof x === "string")) return v;
  return null;
}

function read(o, names) {
  return names.map((n) => value(Object.hasOwn(o, n) ? o[n] : undefined));
}

/// A request for a provider's JSON (`init`: a GET unless it says
/// otherwise): its status and text, the whole answer within `timeoutMs`, at
/// most `maxBytes`, no redirect followed; another status than 200 throws
/// unless `anyStatus`. Throws why not.
async function fetchText(url, init, maxBytes, timeoutMs, anyStatus) {
  const ctrl = new AbortController();
  let late = false;
  const timer = setTimeout(() => {
    late = true;
    ctrl.abort();
  }, timeoutMs);
  try {
    const resp = await fetch(url, { headers: { accept: "application/json" }, ...init, redirect: "manual", signal: ctrl.signal });
    if (resp.status !== 200 && !anyStatus) throw new Error(`it answered ${resp.status}`);
    if (Number(resp.headers.get("content-length") ?? 0) > maxBytes) throw new Error(`it answered more than ${maxBytes} bytes`);
    const chunks = [];
    let total = 0;
    if (resp.body !== null) {
      const reader = resp.body.getReader();
      for (;;) {
        const { done, value: chunk } = await reader.read();
        if (done) break;
        total += chunk.length;
        if (total > maxBytes) throw new Error(`it answered more than ${maxBytes} bytes`);
        chunks.push(chunk);
      }
    }
    const all = new Uint8Array(total);
    let at = 0;
    for (const c of chunks) {
      all.set(c, at);
      at += c.length;
    }
    return { status: resp.status, text: new TextDecoder("utf-8", { fatal: true }).decode(all) };
  } catch (e) {
    throw new Error(late ? `no whole answer within ${timeoutMs} ms` : said(e?.message ?? e));
  } finally {
    clearTimeout(timer);
    ctrl.abort();
  }
}

const kept = new Map();

/// `url`'s text: the isolate's copy while fresh (unless `again`), else
/// fetched. Answers whether it was fetched now.
async function cached(url, maxBytes, timeoutMs, ttlMs, again) {
  const now = Date.now();
  const k = kept.get(url);
  if (!again && k !== undefined && now >= k.at && now - k.at < ttlMs) return { text: k.text, now: false };
  const { text } = await fetchText(url, {}, maxBytes, timeoutMs, false);
  if (kept.size >= KEPT_MAX && !kept.has(url)) kept.clear();
  kept.set(url, { text, at: now });
  return { text, now: true };
}

/// The provider's metadata at `url` (its discovery document), cached:
/// `["ok", "", names' values]`, or `["fetch", why]`.
export async function discover(url, names, limits) {
  const [maxBytes, timeoutMs, ttlMs] = limits;
  let got;
  try {
    got = await cached(url, maxBytes, timeoutMs, ttlMs, false);
  } catch (e) {
    return ["fetch", said(e.message)];
  }
  const o = object(got.text);
  if (o === null) {
    kept.delete(url);
    return ["fetch", "its metadata is not a JSON object"];
  }
  return ["ok", "", read(o, names)];
}

/// A key of the JWKS that fits `alg`: its kind, its curve, its use and
/// its own `alg`, an RSA modulus of an accepted size; as a JWK made from
/// its numbers alone (nothing else the provider's JWK says reaches
/// WebCrypto), and the length of a signature it makes.
function usable(k, alg) {
  if (k === null || typeof k !== "object" || k.kty !== ALGS[alg]) return null;
  if (k.use !== undefined && k.use !== "sig") return null;
  if (k.alg !== undefined && k.alg !== alg) return null;
  if (alg === "RS256") {
    const n = bytes(k.n);
    const e = bytes(k.e);
    if (n === null || e === null) return null;
    let i = 0;
    while (i < n.length && n[i] === 0) i++;
    const len = n.length - i;
    const bits = len === 0 ? 0 : len * 8 - Math.clz32(n[i]) + 24;
    if (bits < RSA_BITS_MIN || bits > RSA_BITS_MAX) return null;
    return { jwk: { kty: "RSA", n: k.n, e: k.e }, length: len, params: { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" } };
  }
  const x = bytes(k.x);
  const y = bytes(k.y);
  if (k.crv !== "P-256" || x?.length !== 32 || y?.length !== 32) return null;
  return { jwk: { kty: "EC", crv: "P-256", x: k.x, y: k.y }, length: ES256_SIGNATURE_BYTES, params: { name: "ECDSA", namedCurve: "P-256", hash: "SHA-256" } };
}

/// A token's signed parts checked against one JWKS's text: the key, its
/// signature's length, WebCrypto's verdict, then the claims.
async function checked(parts, alg, kid, jwksText, names) {
  const jwks = object(jwksText);
  if (jwks === null || !Array.isArray(jwks.keys)) return ["jwks", "its JWKS is not a JSON key set"];
  if (jwks.keys.length > KEYS_MAX) return ["jwks", `its JWKS holds ${jwks.keys.length} keys, over ${KEYS_MAX}`];
  const fitting = jwks.keys.map((k) => [k?.kid, usable(k, alg)]).filter(([, u]) => u !== null);
  const chosen = kid !== undefined ? fitting.filter(([k]) => k === kid) : fitting;
  if (chosen.length !== 1) return ["unknown_key", kid === undefined ? "" : said(kid)];
  const key = chosen[0][1];
  const signature = bytes(parts[2]);
  if (signature === null || signature.length !== key.length) return ["signature", ""];
  let verified;
  try {
    const imported = await crypto.subtle.importKey("jwk", key.jwk, key.params, false, ["verify"]);
    verified = await crypto.subtle.verify(key.params, imported, signature, new TextEncoder().encode(`${parts[0]}.${parts[1]}`));
  } catch (e) {
    return ["unchecked", said(e?.message ?? e)];
  }
  if (verified !== true) return ["signature", ""];
  const payload = bytes(parts[1]);
  const claims = payload === null ? null : object(new TextDecoder().decode(payload));
  if (claims === null) return ["malformed", "the claims are not a JSON object"];
  return ["ok", alg, read(claims, names)];
}

let refetchedAt = null;

/// An id_token (a compact JWS) checked against the provider's keys at
/// `jwksUri`: its header first (an algorithm among `algs` and this file's
/// own two, no `crit`, no access token), before any key is fetched; then a
/// key of the JWKS that fits it (by its `kid`, or the one key of its
/// kind), fetched again at once for a key it lacks unless that was done
/// within `refetchMinMs`; the signature's length, the signature by
/// WebCrypto; only then its claims are read, `names`' values.
///
/// Answers `[code, detail, values]`: `ok` (detail: the algorithm; values:
/// the claims), or why not: `too_large`, `malformed`, `alg`, `crit`, `typ`,
/// `fetch` or `jwks` (the provider's keys could not be had: why),
/// `unknown_key` (detail: the kid, or ""), `signature`, `unchecked` (the
/// runtime could not check it: why).
export async function verify(token, jwksUri, algs, names, limits) {
  const [maxBytes, timeoutMs, ttlMs, refetchMinMs, tokenMaxBytes] = limits;
  if (typeof token !== "string" || token.length > tokenMaxBytes) return ["too_large", ""];
  const parts = token.split(".");
  if (parts.length !== 3) return ["malformed", "not three dot-separated parts (a compact JWS)"];
  const raw = bytes(parts[0]);
  const header = raw === null ? null : object(new TextDecoder().decode(raw));
  if (header === null) return ["malformed", "the header is not a JSON object"];
  const alg = header.alg;
  if (typeof alg !== "string" || !Object.hasOwn(ALGS, alg) || !algs.includes(alg)) return ["alg", said(alg)];
  if (Object.hasOwn(header, "crit")) return ["crit", ""];
  if (typeof header.typ === "string" && /^(application\/)?at\+jwt$/i.test(header.typ)) return ["typ", ""];
  const kid = header.kid;
  if (kid !== undefined && typeof kid !== "string") return ["malformed", "its kid is not a string"];
  let got;
  try {
    got = await cached(jwksUri, maxBytes, timeoutMs, ttlMs, false);
  } catch (e) {
    return ["fetch", `its JWKS: ${said(e.message)}`];
  }
  let answer = await checked(parts, alg, kid, got.text, names);
  const now = Date.now();
  if (answer[0] === "unknown_key" && !got.now && (refetchedAt === null || now - refetchedAt >= refetchMinMs)) {
    refetchedAt = now;
    try {
      got = await cached(jwksUri, maxBytes, timeoutMs, ttlMs, true);
    } catch (e) {
      return ["fetch", `its JWKS: ${said(e.message)}`];
    }
    answer = await checked(parts, alg, kid, got.text, names);
  }
  return answer;
}

/// A sign-in's code redeemed: the token request keys.rs made (`body`, and
/// its `authorization` header, "" for none) sent to the provider's token
/// endpoint, its answer read, and its id_token checked (`verify`). Answers
/// as `verify` does, or why there is no id_token to check:
/// `token_refused` (detail: the status; values: its `error` and
/// `error_description`), `token_fetch` (no answer: why), `token_answer`
/// (no id_token in it: why).
export async function redeem(tokenEndpoint, body, authorization, jwksUri, algs, names, limits) {
  const [maxBytes, timeoutMs] = limits;
  const headers = { "content-type": "application/x-www-form-urlencoded", accept: "application/json" };
  if (authorization !== "") headers.authorization = authorization;
  let got;
  try {
    got = await fetchText(tokenEndpoint, { method: "POST", headers, body }, maxBytes, timeoutMs, true);
  } catch (e) {
    return ["token_fetch", `its token endpoint: ${said(e.message)}`];
  }
  const o = object(got.text);
  if (got.status !== 200) return ["token_refused", String(got.status), o === null ? [undefined, undefined] : read(o, ["error", "error_description"])];
  if (o === null) return ["token_answer", "its token answer is not a JSON object"];
  if (typeof o.id_token !== "string" || o.id_token === "") return ["token_answer", "its token answer carries no id_token (is `openid` among FRAGMENT_OIDC_SCOPES?)"];
  return verify(o.id_token, jwksUri, algs, names, limits);
}
