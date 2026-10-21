// Actual page presence callbacks and complete notification job. Browser APIs,
// socket identity/fragment scoping and job effects are synthetic boundaries.
// Document visibility is the current policy: CSS-hidden shell frames remain
// a known limitation in docs/technical-debt-ledger.md, not covered as fixed here.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { createContext, runInContext } from "node:vm";

const page = readFileSync(new URL("../../../templates/chat/site/chat.js", import.meta.url), "utf8");
const app = readFileSync(new URL("../../../templates/chat/app.mjs", import.meta.url), "utf8");
const presence = page.match(/^  const onScreen = [^]*?(?=^  input.addEventListener\("input")/m)?.[0];
const initial = page.match(/^  fragment.presence.set\(\{ \.\.\.shared \}\);(?=\n  fragment.presence.on)/m)?.[0];
assert.ok(presence && initial, "extract the real presence block and initial publication");
const context = createContext({ DurableObject: class {} });
runInContext(app.replace(/^import \{ DurableObject \} from "cloudflare:workers";$/m, "").replace("export class App", "class App") + "\nglobalThis.app = new App();", context);
const reply = { principal: "agent-a", body: { turn: "turn-1", text: "  A reply\nwith words  " } };
const members = [{ principal: "person-a", kind: "person" }, { principal: "person-b", kind: "person" }, { principal: "agent-a", kind: "agent" }];
const plain = (value) => JSON.parse(JSON.stringify(value));

function client(visibility = "visible") {
  const publications = [], listeners = new Map(), timers = new Map();
  let timer = 0;
  const document = {
    visibilityState: visibility,
    addEventListener(name, callback) {
      assert.ok(!listeners.has(name), "one presence listener per lifecycle event");
      listeners.set(name, callback);
    },
  };
  const vm = createContext({ document, TYPING_MS: 5000,
    fragment: { presence: { set: (data) => publications.push(plain(data)) } },
    setTimeout(callback) { const id = ++timer; timers.set(id, callback); return id; },
    clearTimeout(id) { timers.delete(id); },
  });
  runInContext(presence + initial + "\nglobalThis.type = setTyping;", vm);
  return {
    publications,
    row(principal = "person-a", id = "socket-a") { return { id, principal, data: publications.at(-1) }; },
    visibility(value) { document.visibilityState = value; listeners.get("visibilitychange")(); },
    type: vm.type,
    expireTyping() { for (const [id, callback] of [...timers]) { timers.delete(id); callback(); } },
    timers,
  };
}

async function notify(rows = [], { fragment = "chat-a", roster = members, record = reply, via = "channel" } = {}) {
  const pushes = [];
  const result = await context.app.notify_reply({ record }, {
    via, fragment,
    members: async () => roster,
    presence: async () => rows,
    people: async () => ({ "agent-a": { title: "Juniper", picture: "/agent-picture" } }),
    push: async (principal, payload) => { pushes.push({ principal, payload: plain(payload) }); return { queued: 1 }; },
  });
  return { pushes, result: plain(result) };
}

for (const visibility of ["visible", "hidden"]) {
  test(`initial ${visibility} document publishes presence used by the notification job`, async () => {
    const c = client(visibility);
    assert.deepEqual(c.publications, [{ typing: false, looking: visibility === "visible" }]);
    const { pushes, result } = await notify([c.row()]);
    assert.deepEqual(pushes.map((p) => p.principal), visibility === "visible" ? ["person-b"] : ["person-a", "person-b"]);
    assert.equal(result.pushed, pushes.length);
    for (const push of pushes) assert.deepEqual(push.payload, {
      title: "Juniper", body: "A reply with words", tag: "chat-a", url: "./", icon: "/agent-picture",
    });
  });
}

test("visible → hidden → visible changes delivery; duplicate visibility events publish nothing", async () => {
  const c = client();
  for (const [visibility, expected] of [["visible", false], ["hidden", true], ["visible", false], ["hidden", true]]) {
    c.visibility(visibility);
    const count = c.publications.length;
    for (let i = 0; i < 3; i++) c.visibility(visibility);
    assert.equal(c.publications.length, count);
    assert.equal((await notify([c.row()])).pushes.some((p) => p.principal === "person-a"), expected);
  }
  assert.equal(c.publications.length, 4);
});

test("typing changes and timer expiry preserve hidden/visible presence", async () => {
  const c = client();
  c.type(true); c.type(true);
  assert.equal(c.timers.size, 1);
  assert.deepEqual(c.publications.at(-1), { typing: true, looking: true });
  c.visibility("hidden");
  assert.deepEqual(c.publications.at(-1), { typing: true, looking: false });
  assert.equal((await notify([c.row()])).pushes.length, 2);
  c.expireTyping();
  assert.deepEqual(c.publications.at(-1), { typing: false, looking: false });
  c.visibility("visible");
  assert.deepEqual(c.publications.at(-1), { typing: false, looking: true });
  assert.deepEqual((await notify([c.row()])).pushes.map((p) => p.principal), ["person-b"]);
});

test("one visible session suppresses only its person even with hidden sessions and another visible principal", async () => {
  const visible = client(), hidden = client("hidden");
  const rows = [hidden.row(), visible.row("person-a", "socket-b"), visible.row("stranger", "socket-c"), visible.row("agent-a", "socket-d")];
  assert.deepEqual((await notify(rows)).pushes.map((p) => p.principal), ["person-b"]);
  rows.splice(1, 1); // The socket service removes the closed session.
  assert.deepEqual((await notify(rows)).pushes.map((p) => p.principal), ["person-a", "person-b"]);
});

test("a fresh person's presence cannot suppress the previous person's notification", async () => {
  const c = client();
  assert.deepEqual((await notify([c.row("person-b")])).pushes.map((p) => p.principal), ["person-a"]);
});

test("separate fragment job inputs do not retain another chat's looking set or payload tag", async () => {
  const c = client();
  const first = await notify([c.row()], { fragment: "chat-a" });
  const second = await notify([], { fragment: "chat-b" });
  assert.deepEqual(first.pushes.map((p) => p.principal), ["person-b"]);
  assert.deepEqual(second.pushes.map((p) => p.principal), ["person-a", "person-b"]);
  assert.ok(first.pushes.every((p) => p.payload.tag === "chat-a"));
  assert.ok(second.pushes.every((p) => p.payload.tag === "chat-b"));
  assert.deepEqual((await notify([c.row()], { fragment: "chat-a" })).pushes, first.pushes);
});

for (const data of [undefined, null, {}, { looking: false }, { looking: "true" }, { looking: 1 }]) {
  test(`only boolean true looking suppresses a person: ${JSON.stringify(data)}`, async () => {
    assert.deepEqual((await notify([{ principal: "person-a", data }])).pushes.map((p) => p.principal), ["person-a", "person-b"]);
  });
}

test("all people looking produces no push, including to the agent or a stranger", async () => {
  const c = client();
  const result = await notify([c.row(), c.row("person-b")]);
  assert.deepEqual(result, { pushes: [], result: { pushed: 0, why: "no one away to tell" } });
});

for (const [label, options] of [
  ["direct invocation", { via: "call" }],
  ["person's message", { record: { ...reply, principal: "person-a" } }],
  ["removed agent", { roster: members.filter((m) => m.kind !== "agent") }],
  ["non-reply record", { record: { ...reply, body: { kind: "stop", turn: "turn-1" } } }],
]) {
  test(`${label} cannot notify even when every person is away`, async () => {
    const { pushes, result } = await notify([], options);
    assert.deepEqual(pushes, []);
    assert.equal(result.pushed, 0);
  });
}
