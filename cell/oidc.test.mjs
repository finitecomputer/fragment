// oidc.mjs: what the cell's JavaScript checks of an OpenID Connect
// provider's id_token on its own (docs/self-host.md, seam 4), before Rust
// reads a claim. Valid: RS256 and ES256 by a key of the JWKS, its claims
// answered flat. Invalid: `none`, HMACs and algorithms not allowed refused
// before any key is fetched; a bad signature, a forgery under a published
// kid, a key not published or not usable, `crit`, an access token, each
// refused. Fetching: bounded (status, size, time), kept per isolate, the
// JWKS fetched again at once for a rotated key and then not again within
// its bound. What `discover` answers of a provider's metadata, and what
// `redeem` makes of a token endpoint's answer.
import { test, after } from "node:test";
import assert from "node:assert/strict";
import http from "node:http";
import { discover, redeem, verify } from "./oidc.mjs";

const enc = (b) => Buffer.from(b).toString("base64url");
const json = (o) => enc(JSON.stringify(o));
const NAMES = ["iss", "sub", "aud", "azp", "exp", "iat", "nbf", "nonce", "email", "email_verified"];
const CLAIMS = { iss: "https://idp.corp.example", sub: "user-1", aud: "fragment", exp: 1790000300, iat: 1790000000.5, nonce: "n-1", email: "jane@corp.example", email_verified: true, groups: [{ a: 1 }] };
const RS = { name: "RSASSA-PKCS1-v1_5", modulusLength: 2048, publicExponent: new Uint8Array([1, 0, 1]), hash: "SHA-256" };
const EC = { name: "ECDSA", namedCurve: "P-256" };
const BOTH = ["RS256", "ES256"];
// the limits Rust hands it (fragment_core::oidc::SHIM_LIMITS): bytes, ms to
// answer, ms kept, ms between refetches, id_token bytes
const MAX = 65536;
const TOKEN_MAX = 16384;
const LIMITS = [MAX, 2000, 3600000, 15000, TOKEN_MAX];

// a provider's documents, each at a path of its own; and how often each was asked
const served = new Map();
const hits = new Map();
const posted = [];
const server = http.createServer(async (req, res) => {
  hits.set(req.url, (hits.get(req.url) ?? 0) + 1);
  if (req.method === "POST") {
    let body = "";
    for await (const chunk of req) body += chunk;
    posted.push({ body, type: req.headers["content-type"], authorization: req.headers.authorization });
  }
  const s = served.get(req.url);
  if (s === undefined) return res.writeHead(404).end();
  const answer = () => res.writeHead(s.status ?? 200, { "content-type": "application/json" }).end(s.body);
  return s.delayMs ? setTimeout(answer, s.delayMs) : answer();
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
after(() => server.close());
let paths = 0;
function serve(body, extra = {}) {
  const path = `/doc-${++paths}`;
  served.set(path, { body, ...extra });
  return `http://127.0.0.1:${server.address().port}${path}`;
}
const asked = (url) => hits.get(new URL(url).pathname) ?? 0;

async function keyPair(params, kid) {
  const k = await crypto.subtle.generateKey(params, true, ["sign", "verify"]);
  const jwk = await crypto.subtle.exportKey("jwk", k.publicKey);
  return { priv: k.privateKey, jwk: { ...jwk, kid, use: "sig", alg: params.name === "ECDSA" ? "ES256" : "RS256" } };
}

async function sign(header, claims, key) {
  const input = `${json(header)}.${json(claims)}`;
  const params = header.alg === "ES256" ? { name: "ECDSA", hash: "SHA-256" } : { name: "RSASSA-PKCS1-v1_5" };
  const sig = await crypto.subtle.sign(params, key, new TextEncoder().encode(input));
  return `${input}.${enc(new Uint8Array(sig))}`;
}

const rsa = await keyPair(RS, "r1");
const ec = await keyPair(EC, "e1");
const stranger = await keyPair(RS, "r1");
const JWKS = JSON.stringify({ keys: [rsa.jwk, ec.jwk] });
const check = (token, url, algs = BOTH, limits = LIMITS) => verify(token, url, algs, NAMES, limits);

test("RS256 and ES256 by a key of the JWKS verify, and the claims are answered flat", async () => {
  const url = serve(JWKS);
  const [code, alg, values] = await check(await sign({ alg: "RS256", typ: "JWT", kid: "r1" }, CLAIMS, rsa.priv), url);
  assert.deepEqual([code, alg], ["ok", "RS256"]);
  assert.deepEqual(values, ["https://idp.corp.example", "user-1", "fragment", undefined, 1790000300, 1790000000.5, undefined, "n-1", "jane@corp.example", true]);
  const es = await check(await sign({ alg: "ES256", kid: "e1" }, { ...CLAIMS, aud: ["fragment", "other"], email_verified: "true" }, ec.priv), url);
  assert.equal(es[0], "ok");
  assert.deepEqual(es[2][2], ["fragment", "other"], "a list of audiences, as it came");
  assert.equal(es[2][9], "true", "a string where a boolean belongs, as it came: Rust refuses it");
  assert.equal(asked(url), 1, "the JWKS fetched once, then kept");
  // no kid: the one key of that kind
  assert.equal((await check(await sign({ alg: "ES256" }, CLAIMS, ec.priv), serve(JSON.stringify({ keys: [ec.jwk] }))))[0], "ok");
});

test("alg none, an HMAC, or an algorithm not allowed is refused on its own, before any key is fetched", async () => {
  const url = serve(JWKS);
  const none = `${json({ alg: "none" })}.${json(CLAIMS)}.`;
  assert.deepEqual(await check(none, url), ["alg", "none"]);
  // allowed or not by the caller, the shim takes RS256 and ES256 alone
  assert.deepEqual(await check(none, url, ["none", ...BOTH]), ["alg", "none"]);
  const hmacKey = await crypto.subtle.importKey("raw", new TextEncoder().encode("the client secret"), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const input = `${json({ alg: "HS256", kid: "r1" })}.${json(CLAIMS)}`;
  const hs = `${input}.${enc(new Uint8Array(await crypto.subtle.sign("HMAC", hmacKey, new TextEncoder().encode(input))))}`;
  assert.deepEqual(await check(hs, url), ["alg", "HS256"]);
  assert.deepEqual(await check(hs, url, ["HS256", ...BOTH]), ["alg", "HS256"]);
  // ES256 where the provider lists RS256 alone
  assert.deepEqual(await check(await sign({ alg: "ES256", kid: "e1" }, CLAIMS, ec.priv), url, ["RS256"]), ["alg", "ES256"]);
  assert.deepEqual(await check(`${json({ kid: "r1" })}.${json(CLAIMS)}.x`, url), ["alg", "undefined"], "no alg at all");
  assert.equal(asked(url), 0, "no key was fetched for any of them");
});

test("a bad signature, a forgery under a published kid, and claims changed under a signature are refused", async () => {
  const url = serve(JWKS);
  const token = await sign({ alg: "RS256", kid: "r1" }, CLAIMS, rsa.priv);
  const [h, p, s] = token.split(".");
  const flipped = Buffer.from(s, "base64url");
  flipped[3] ^= 1;
  assert.deepEqual(await check(`${h}.${p}.${enc(flipped)}`, url), ["signature", ""]);
  assert.deepEqual(await check(`${h}.${p}.${enc(flipped.subarray(1))}`, url), ["signature", ""], "a signature of the wrong length");
  assert.deepEqual(await check(`${h}.${json({ ...CLAIMS, sub: "user-2" })}.${s}`, url), ["signature", ""]);
  assert.deepEqual(await check(await sign({ alg: "RS256", kid: "r1" }, CLAIMS, stranger.priv), url), ["signature", ""]);
  const esForged = await sign({ alg: "ES256", kid: "e1" }, CLAIMS, (await keyPair(EC, "e1")).priv);
  assert.deepEqual(await check(esForged, url), ["signature", ""]);
});

test("a key not published, not usable, or not single is unknown; a key the token carries is never used", async () => {
  const fresh = () => serve(JWKS);
  assert.deepEqual(await check(await sign({ alg: "RS256", kid: "r2" }, CLAIMS, rsa.priv), fresh()), ["unknown_key", "r2"]);
  const carried = await sign({ alg: "RS256", kid: "zz", jwk: stranger.jwk }, CLAIMS, stranger.priv);
  assert.deepEqual(await check(carried, fresh()), ["unknown_key", "zz"]);
  // the RS256 header naming the EC key's kid
  assert.deepEqual(await check(await sign({ alg: "RS256", kid: "e1" }, CLAIMS, rsa.priv), fresh()), ["unknown_key", "e1"]);
  const two = serve(JSON.stringify({ keys: [rsa.jwk, { ...stranger.jwk, kid: "r9" }] }));
  assert.deepEqual(await check(await sign({ alg: "RS256" }, CLAIMS, rsa.priv), two), ["unknown_key", ""], "no kid, two keys of its kind");
  const small = await keyPair({ ...RS, modulusLength: 1024 }, "s1");
  assert.deepEqual(await check(await sign({ alg: "RS256", kid: "s1" }, CLAIMS, small.priv), serve(JSON.stringify({ keys: [small.jwk] }))), ["unknown_key", "s1"], "an RSA key under 2048 bits");
  const encKey = serve(JSON.stringify({ keys: [{ ...rsa.jwk, use: "enc" }] }));
  assert.deepEqual(await check(await sign({ alg: "RS256", kid: "r1" }, CLAIMS, rsa.priv), encKey), ["unknown_key", "r1"], "an encryption key");
  const curve = serve(JSON.stringify({ keys: [{ ...ec.jwk, crv: "P-384" }] }));
  assert.deepEqual(await check(await sign({ alg: "ES256", kid: "e1" }, CLAIMS, ec.priv), curve), ["unknown_key", "e1"], "another curve");
  assert.equal((await check(await sign({ alg: "RS256", kid: "r1" }, CLAIMS, rsa.priv), serve("not json")))[0], "jwks");
  assert.equal((await check(await sign({ alg: "RS256", kid: "r1" }, CLAIMS, rsa.priv), serve(JSON.stringify({ keys: Array(33).fill(rsa.jwk) }))))[0], "jwks");
});

test("crit, an access token, and malformed tokens are refused", async () => {
  const url = serve(JWKS);
  assert.deepEqual(await check(await sign({ alg: "RS256", kid: "r1", crit: ["exp"] }, CLAIMS, rsa.priv), url), ["crit", ""]);
  assert.deepEqual(await check(await sign({ alg: "RS256", kid: "r1", typ: "at+jwt" }, CLAIMS, rsa.priv), url), ["typ", ""]);
  assert.deepEqual(await check(await sign({ alg: "RS256", kid: "r1", typ: "application/AT+JWT" }, CLAIMS, rsa.priv), url), ["typ", ""]);
  assert.equal((await check("a.b", url))[0], "malformed");
  assert.equal((await check("!!.b.c", url))[0], "malformed");
  assert.equal((await check(`${json([1])}.${json(CLAIMS)}.x`, url))[0], "malformed");
  assert.equal((await check(await sign({ alg: "RS256", kid: 7 }, CLAIMS, rsa.priv), url))[0], "malformed");
  assert.equal((await check(await sign({ alg: "RS256", kid: "r1" }, [1], rsa.priv), url))[0], "malformed", "claims that are no object, under a good signature");
});

test("a rotated key is fetched again at once, then no key it lacks is fetched for again within the bound", async () => {
  const url = serve(JSON.stringify({ keys: [rsa.jwk] }));
  assert.equal((await check(await sign({ alg: "RS256", kid: "r1" }, CLAIMS, rsa.priv), url))[0], "ok");
  const next = await keyPair(RS, "r2");
  served.get(new URL(url).pathname).body = JSON.stringify({ keys: [rsa.jwk, next.jwk] });
  assert.equal((await check(await sign({ alg: "RS256", kid: "r2" }, CLAIMS, next.priv), url))[0], "ok", "the new key, picked up at once");
  assert.equal(asked(url), 2);
  assert.deepEqual(await check(await sign({ alg: "RS256", kid: "r3" }, CLAIMS, next.priv), url), ["unknown_key", "r3"]);
  assert.equal(asked(url), 2, "a key never published is not fetched for again within the bound");
  assert.equal((await check(await sign({ alg: "RS256", kid: "r3" }, CLAIMS, next.priv), url, BOTH, [MAX, 2000, 3600000, 0, TOKEN_MAX]))[0], "unknown_key");
  assert.equal(asked(url), 3, "past the bound, once more");
  // past its time, kept no longer
  assert.equal((await check(await sign({ alg: "RS256", kid: "r1" }, CLAIMS, rsa.priv), url, BOTH, [MAX, 2000, 0, 15000, TOKEN_MAX]))[0], "ok");
  assert.equal(asked(url), 4);
});

test("a provider's keys that cannot be had: another status, too large, too slow", async () => {
  const token = await sign({ alg: "RS256", kid: "r1" }, CLAIMS, rsa.priv);
  assert.deepEqual(await check(token, serve("{}", { status: 503 })), ["fetch", "its JWKS: it answered 503"]);
  assert.deepEqual(await check(token, serve(JWKS), BOTH, [100, 2000, 3600000, 15000, TOKEN_MAX]), ["fetch", "its JWKS: it answered more than 100 bytes"]);
  assert.deepEqual(await check(token, serve(JWKS, { delayMs: 500 }), BOTH, [MAX, 100, 3600000, 15000, TOKEN_MAX]), ["fetch", "its JWKS: no whole answer within 100 ms"]);
  assert.deepEqual(await check(token, serve(JWKS), BOTH, [MAX, 2000, 3600000, 15000, token.length - 1]), ["too_large", ""], "an id_token over its bound, before anything");
});

test("discover: a provider's metadata read flat, each value as it came, absent or other, and kept", async () => {
  const doc = JSON.stringify({ issuer: "https://idp", algs: ["RS256", "ES256"], mixed: ["a", 1], n: 2, b: false, o: { x: 1 }, nul: null, toString: "x" });
  const names = ["issuer", "algs", "mixed", "n", "b", "o", "nul", "absent", "constructor", "toString"];
  const want = ["https://idp", ["RS256", "ES256"], null, 2, false, null, null, undefined, undefined, "x"];
  const url = serve(doc);
  assert.deepEqual(await discover(url, names, LIMITS), ["ok", "", want], "own properties alone");
  assert.deepEqual(await discover(url, names, LIMITS), ["ok", "", want]);
  assert.equal(asked(url), 1, "kept");
  assert.deepEqual(await discover(serve("[1]"), names, LIMITS), ["fetch", "its metadata is not a JSON object"]);
  assert.deepEqual(await discover(serve("", { status: 404 }), names, LIMITS), ["fetch", "it answered 404"]);
});

test("redeem: the token request sent as made, its answer read, its id_token checked", async () => {
  const jwks = serve(JWKS);
  const good = await sign({ alg: "RS256", kid: "r1" }, CLAIMS, rsa.priv);
  const body = "grant_type=authorization_code&code=c&redirect_uri=x&code_verifier=v";
  const r = (endpoint, authorization = "Basic eDp5") => redeem(endpoint, body, authorization, jwks, BOTH, NAMES, LIMITS);
  assert.deepEqual((await r(serve(JSON.stringify({ id_token: good, access_token: "a" }))))[0], "ok");
  assert.deepEqual(posted.at(-1), { body, type: "application/x-www-form-urlencoded", authorization: "Basic eDp5" }, "sent as keys.rs made it");
  await r(serve(JSON.stringify({ id_token: good })), "");
  assert.equal(posted.at(-1).authorization, undefined, "a public client sends no authorization");
  assert.deepEqual(await r(serve(JSON.stringify({ error: "invalid_grant", error_description: "used" }), { status: 400 })), ["token_refused", "400", ["invalid_grant", "used"]]);
  assert.deepEqual(await r(serve("<html>", { status: 502 })), ["token_refused", "502", [undefined, undefined]]);
  assert.equal((await r(serve("not json")))[0], "token_answer");
  assert.equal((await r(serve(JSON.stringify({ access_token: "a" }))))[0], "token_answer", "no id_token (no openid scope)");
  assert.deepEqual(await r(serve(JSON.stringify({ id_token: "a.b" }))), ["malformed", "not three dot-separated parts (a compact JWS)"]);
  const slow = serve("{}", { delayMs: 500 });
  assert.deepEqual(await redeem(slow, body, "", jwks, BOTH, NAMES, [MAX, 100, 3600000, 15000, TOKEN_MAX]), ["token_fetch", "its token endpoint: no whole answer within 100 ms"]);
});
