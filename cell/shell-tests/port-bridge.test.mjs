// The Computer Durable Object's port bridge (cell/entry.mjs, ContainerHost's
// `port` and `closeTabs`): a sleep closes its port tabs as it begins, before
// its hold, so nothing a page sends after reaches the container its save
// holds (docs/computers.md, Ports). Review (#1353, 2026-11-18): a socket
// admitted before a sleep began, its upgrade still on its way as the sleep
// closed the tabs, was registered after and forwarded its page's words to
// the container mid-save. Each case runs the real class, on Node, its
// platform modules stood in for and the upstream upgrade held until told.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { register } from "node:module";

// the Workers modules entry.mjs imports, stood in for: only what its
// Computer object and ContainerHost touch is real enough to call
const stubs = {
  "cloudflare:workers": "export class DurableObject { constructor(ctx, env) { this.ctx = ctx; this.env = env; } } export class WorkerEntrypoint {} export class WorkflowEntrypoint {}",
  "cloudflare:sockets": "export function connect() { throw new Error('no sockets in this test'); }",
  "@cloudflare/sandbox": "export class DirectoryBackup {} export class DirectoryBackupGateway {} export class SandboxBackupError extends Error {}",
  "./build/index.js":
    "export default {}; export const InternalRoute = { request: (env, path, body) => ({ path, body: JSON.parse(body) }) }; export class ComputerCell { fetch(r) { globalThis.reports.push(r); return Promise.resolve({ ok: true }); } }",
};
const hooks = `const stubs = ${JSON.stringify(stubs)};
export async function resolve(specifier, context, next) {
  if (specifier in stubs) return { url: "data:text/javascript," + encodeURIComponent(stubs[specifier]), shortCircuit: true };
  return next(specifier, context);
}`;
register("data:text/javascript," + encodeURIComponent(hooks));

// A WebSocket end: what it sent, whether it was accepted, how it closed;
// its peer hears its sends and its close.
class End {
  constructor() {
    this.listeners = {};
    this.sent = [];
    this.closed = null;
    this.accepted = false;
  }
  addEventListener(type, f) {
    (this.listeners[type] ??= []).push(f);
  }
  emit(type, e) {
    for (const f of this.listeners[type] ?? []) f(e);
  }
  accept() {
    this.accepted = true;
  }
  send(data) {
    if (this.closed) throw new Error("closed for sending");
    this.sent.push(data);
    this.peer?.emit("message", { data });
  }
  // its close frame closes its peer too, which hears it
  close(code, reason) {
    if (this.closed) return;
    this.closed = { code, reason };
    if (this.peer && !this.peer.closed) {
      this.peer.closed = { code, reason };
      this.peer.emit("close", { code, reason });
    }
  }
}
globalThis.WebSocketPair = class {
  constructor() {
    const [a, b] = [new End(), new End()];
    a.peer = b;
    b.peer = a;
    Object.assign(this, { 0: a, 1: b });
  }
};
// a 101, which Node's own Response refuses to make
globalThis.Response = class {
  constructor(body, init = {}) {
    this.status = init.status ?? 200;
    this.webSocket = init.webSocket;
  }
};
globalThis.reports = [];

const { Computer } = await import("../entry.mjs");

// A computer whose container's port answers its upgrades, oldest first,
// as `answer` is called (which answers the container's end of that one).
function computer() {
  const waiting = [];
  const upgrade = () => new Promise((r) => waiting.push(() => {
    const upstream = new End();
    r({ webSocket: upstream });
    return upstream;
  }));
  const ctx = { id: "computer-under-test", exports: { DirectoryBackupGateway: class {} }, container: { running: true, getTcpPort: () => ({ fetch: upgrade }) } };
  new Computer(ctx, {});
  globalThis.reports.length = 0;
  return { host: ctx.computerHost, answer: () => waiting.shift()() };
}
const tabReports = () => globalThis.reports.filter((r) => r.path === "computer/tab").map((r) => r.body);
const settle = () => new Promise((r) => setTimeout(r, 0));

test("an upgrade under way as a sleep or a restart begins is closed with its why, never registered, and its page's words never reach the container", async () => {
  for (const why of ["asleep", "restarting"]) {
    const { host, answer } = computer();
    const asked = host.port(6080, {}, 60_000);
    await settle();
    assert.equal(host.closeTabs(why), 0, "no tab registered yet");
    const upstream = answer();
    const resp = await asked;
    assert.equal(resp.status, 101, "answered as a socket, so its page hears why");
    const page = resp.webSocket;
    assert.deepEqual(page.closed, { code: 4000, reason: why }, `the page's end closed: ${why}`);
    assert.deepEqual(upstream.closed, { code: 4000, reason: why }, "and the container's");
    try {
      page.send("a key, after the sleep began");
    } catch {}
    await settle();
    assert.deepEqual(upstream.sent, [], "nothing reaches the container its save holds");
    assert.deepEqual(tabReports(), [], "no tab opened, used or closed: none was ours");
    assert.equal(host.closeTabs(why), 0, "and none is left to close");
  }
});

test("an ordinary upgrade is a tab: opened, its page's words reach the container and are its use, and a sleep closes it with its why", async () => {
  const { host, answer } = computer();
  const asked = host.port(6080, {}, 60_000);
  await settle();
  const upstream = answer();
  const resp = await asked;
  assert.equal(resp.status, 101);
  const page = resp.webSocket;
  assert.deepEqual(tabReports(), [{ open: true }]);
  page.send("a key");
  await settle();
  assert.deepEqual(upstream.sent, ["a key"], "forwarded");
  assert.deepEqual(tabReports(), [{ open: true }, { open: true, used: true }], "its use told at once");
  assert.equal(host.closeTabs("asleep"), 1, "the sleep closes it");
  assert.deepEqual([page.closed, upstream.closed], [{ code: 4000, reason: "asleep" }, { code: 4000, reason: "asleep" }]);
  assert.deepEqual(tabReports().at(-1), { open: false, used: false }, "its close told");
  // one admitted after that sleep began (its computer woken again) is a tab
  const again = host.port(6080, {}, 60_000);
  await settle();
  const next = answer();
  const reopened = (await again).webSocket;
  assert.equal(reopened.closed, null, "admitted after the sleep began: open");
  reopened.send("a key, on its next life");
  await settle();
  assert.deepEqual(next.sent, ["a key, on its next life"]);
  // its page leaves: the use not told yet goes with its close (no timer left)
  reopened.close(1000, "");
  assert.deepEqual(tabReports().at(-1), { open: false, used: true });
});
