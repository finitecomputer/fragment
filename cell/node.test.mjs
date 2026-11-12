// node.mjs: what reaches a computer's bindings from `/api/nodes/egress`
// (docs/self-host.md, seam 2), and an exec's first frame. The egress route
// is public: anyone may send it a request naming a computer, so what is
// read of a request before its node's signature is checked is bounded,
// and a request that carries no signature in its window is refused before
// its body is read at all (Codex's review of #1243).
import { test } from "node:test";
import assert from "node:assert/strict";
import { EGRESS_BODY_MAX, NodeContainer, nodeEgress, nodeOf } from "./node.mjs";

const SECRET = "s".repeat(40);
const CONTAINER = "computer:00112233445566778899aabb";
const enc = new TextEncoder();
const hex = (buf) => [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, "0")).join("");

function node() {
  return nodeOf({ FRAGMENT_NODE_SECRET_BOX: { get: async () => SECRET } }, { id: "box", url: "http://127.0.0.1:1", secret: "FRAGMENT_NODE_SECRET_BOX" });
}

// a container whose intercept 0 is `seen`'s binding
function container(seen) {
  return { name: CONTAINER, binding: (i) => (i === 0 ? { fetch: async (r) => { seen.push(r); return new Response("reached"); } } : null) };
}

// The node's signature over an intercepted request (sandcastle's docs/node.md, Auth).
async function signature(e, body, t = Math.floor(Date.now() / 1000)) {
  const key = await crypto.subtle.importKey("raw", enc.encode(SECRET), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const hash = hex(await crypto.subtle.digest("SHA-256", body));
  const canonical = `sandcastle-egress-v1\n${e.method}\n${e.container}\n${e.intercept}\n${e.scheme}\n${e.host}\n${e.path}\n${t}\n${hash}`;
  return `t=${t},sig=${hex(await crypto.subtle.sign("HMAC", key, enc.encode(canonical)))}`;
}

function headers(auth) {
  const h = { "x-sandcastle-container": CONTAINER, "x-sandcastle-intercept": "0", "x-sandcastle-host": "api.fragment.internal", "x-sandcastle-scheme": "http", "x-sandcastle-path": "/api/computer" };
  if (auth) h["x-sandcastle-auth"] = auth;
  return h;
}

// A chunked body (no length declared) of `chunks` chunks of 1 MiB, which
// counts what was pulled of it and whether it was cancelled.
function counted(chunks) {
  const seen = { pulled: 0, cancelled: false };
  const chunk = new Uint8Array(1 << 20);
  const body = new ReadableStream({
    pull(c) {
      if (seen.pulled >= chunks * chunk.length) return c.close();
      seen.pulled += chunk.length;
      c.enqueue(chunk);
    },
    cancel() {
      seen.cancelled = true;
    },
  });
  return { body, seen };
}

const post = (h, body) => new Request("https://computer.internal/node-egress", { method: "POST", headers: h, body, duplex: "half" });

test("a request with no signature, a malformed one, or one out of its window is refused before its body is read", async () => {
  const old = Math.floor(Date.now() / 1000) - 3600;
  for (const auth of [null, "nonsense", `t=${old},sig=${"a".repeat(64)}`]) {
    const seen = [];
    const { body, seen: read } = counted(64);
    const r = await nodeEgress(post(headers(auth), body), node(), container(seen));
    assert.equal(r.status, 401, String(auth));
    // a stream pulls its first chunk as it is made: nothing past it read
    assert.ok(read.pulled <= 1 << 20, `${auth}: its body read (${read.pulled} bytes)`);
    assert.equal(seen.length, 0);
  }
});

test("a body past the bound, chunked with no length, is read no further than the bound, then refused 413", async () => {
  const seen = [];
  const { body, seen: read } = counted(64);
  // in its window and well formed, so its body is read: but no node made it
  const r = await nodeEgress(post(headers(`t=${Math.floor(Date.now() / 1000)},sig=${"a".repeat(64)}`), body), node(), container(seen));
  assert.equal(r.status, 413);
  assert.ok(read.pulled <= EGRESS_BODY_MAX + (1 << 20), `read ${read.pulled}, past the bound ${EGRESS_BODY_MAX} by more than a chunk`);
  assert.ok(read.cancelled, "the rest of its body cancelled");
  assert.equal(seen.length, 0);
});

test("a declared length past the bound is refused unread; a bad signature over a small body is refused", async () => {
  const { body, seen: read } = counted(1);
  const r = await nodeEgress(post({ ...headers(`t=${Math.floor(Date.now() / 1000)},sig=${"a".repeat(64)}`), "content-length": String(EGRESS_BODY_MAX + 1) }, body), node(), container([]));
  assert.equal(r.status, 413);
  assert.ok(read.pulled <= 1 << 20, `its body read (${read.pulled} bytes)`);
  const small = enc.encode("{}");
  const bad = await nodeEgress(post(headers(`t=${Math.floor(Date.now() / 1000)},sig=${"b".repeat(64)}`), small), node(), container([]));
  assert.equal(bad.status, 401);
});

test("the node's signed request reaches its binding, as the guest sent it, the platform's own headers gone", async () => {
  const seen = [];
  const body = enc.encode('{"hello":true}');
  const e = { method: "POST", container: CONTAINER, intercept: "0", scheme: "http", host: "api.fragment.internal", path: "/api/computer" };
  const h = { ...headers(await signature(e, body)), "x-fragment-agent": "juniper--k3x9", "x-fragment-release": "r1", "x-fragment-computer": "node-egress", "content-type": "application/json" };
  const r = await nodeEgress(post(h, body), node(), container(seen));
  assert.equal(r.status, 200);
  assert.equal(await r.text(), "reached");
  assert.equal(seen.length, 1);
  const got = seen[0];
  assert.equal(got.url, "http://api.fragment.internal/api/computer");
  assert.equal(await got.text(), '{"hello":true}');
  assert.equal(got.headers.get("x-fragment-agent"), "juniper--k3x9", "the guest's own header passes");
  for (const k of ["x-fragment-release", "x-fragment-computer", "x-sandcastle-auth", "x-sandcastle-container"]) assert.equal(got.headers.get(k), null, k);
  // another computer's intercept is refused
  const other = await nodeEgress(post({ ...h, "x-sandcastle-container": "computer:ffffffffffffffffffffffff" }, body), node(), container([]));
  assert.equal(other.status, 403);
});

test("an exec's first message is its spec, before a string stdin and its end", async () => {
  const sent = [];
  const socket = new EventTarget();
  socket.accept = () => {};
  socket.send = (m) => sent.push(m);
  socket.close = () => {};
  const key = crypto.subtle.importKey("raw", enc.encode(SECRET), { name: "HMAC", hash: "SHA-256" }, false, ["sign", "verify"]);
  const n = { id: "box", key, fetch: async () => ({ webSocket: socket, status: 101 }) };
  await new NodeContainer(n, CONTAINER, {}).exec(["cat"], { stdin: "hello" });
  assert.equal(typeof sent[0], "string", "the spec first");
  assert.deepEqual(JSON.parse(sent[0]).cmd, ["cat"]);
  assert.ok(sent.length >= 3 && sent.slice(1).every((m) => m instanceof Uint8Array), "then its stdin frames and its end");
});
