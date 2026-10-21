// Production Settings loader/renderer, API, start, and chat/sign-in navigation.
// Synthetic DOM, HTTP and unrelated section builders; no browser or vendor calls.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test as nodeTest } from "node:test";
import { createContext, runInContext } from "node:vm";
const test = (name, run) => nodeTest(name, { timeout: 1000 }, run);
const shell = readFileSync(new URL("../../../cell/shell/shell.js", import.meta.url), "utf8");
const source = ["openSettings", "renderSettingsPage", "section", "line", "actionLine", "api", "openChat", "start", "setGuest", "signIn", "firstRun"].map((name) => {
  const found = shell.match(new RegExp(`^(?:async )?function ${name}\\([^]*?^\\}`, "m"))?.[0];
  assert.ok(found, `production ${name} exists`);
  return found;
}).join("\n");
const apiError = shell.match(/^class ApiError[^]*?^\}/m)?.[0];
// When the pending identity repair is present, execute its real transition
// helper too. On master start currently assigns the identity itself.
const identity = shell.match(/^function identityIs\([^]*?^\}/m)?.[0] ?? "";
const identityState = identity ? [shell.match(/^const reads = .*;$/m)?.[0], ...["woke", "making", "restarted"].map((name) => shell.match(new RegExp(`^let ${name} = .*;$`, "m"))?.[0])].join("\n") : "";
const turn = () => new Promise(setImmediate);
const deferred = () => { let resolve; const promise = new Promise((r) => { resolve = r; }); return { promise, resolve }; };
const walk = (node) => [node, ...node.children.filter((n) => typeof n === "object").flatMap(walk)];
const text = (node) => walk(node).map((n) => n.textContent).join(" ");
function setup(stage, guest = false) {
  const gates = [], requests = [], data = [], guests = [];
  let active = true, identityStarting = false;
  const pause = async (where, value) => {
    if (active && stage === where) { const gate = deferred(); gates.push(gate); await gate.promise; }
    return value;
  };
  function el(tag, classes, value) {
    return {
      tag, classes, textContent: value ?? "", children: [], dataset: {}, hidden: false,
      classList: { add() {}, remove() {} },
      append(...nodes) { this.children.push(...nodes); }, replaceChildren(...nodes) { this.children = nodes; },
      get lastChild() { return this.children.at(-1); },
      setAttribute(name, value) { this[name] = value; }, removeAttribute(name) { delete this[name]; },
      getBoundingClientRect() { return { top: 0 }; },
      querySelector(selector) {
        if (selector === ":scope > h2") return this.children.find((n) => n.tag === "h2");
        assert.equal(selector, ".settings-value");
        return walk(this).find((n) => n.classes === "settings-value");
      },
    };
  }
  const nodes = Object.fromEntries(["layout", "frames", "settings-page", "notice", "first-run", "first-run-card"].map((id) => [id, el("div")]));
  const state = { me: { id: "owner-a", email: "a@example.test" }, page: null, computer: null, agents: new Map(), frames: new Map([["chat", el("iframe")]]), fragments: [{}], guest };
  const context = createContext({
    state, el, $: (id) => nodes[id], heard() {},
    fetch: (path) => {
      if (path === "/api/identities/me" || path === "/api/seat" && identityStarting) {
        const gate = deferred(); requests.push({ path, ...gate }); return gate.promise;
      }
      const body = path === "/api/seat" ? { guest } : {};
      return pause("reads", null).then(() => ({ ok: true, json: async () => body }));
    },
    returning: () => pause("returning", null), billingSections: () => pause("billing", [el("section", "billing-marker", state.me.id)]),
    skillsSection: () => pause("skills", el("section")),
    dataSection: () => { const node = el("section", "export-marker"); data.push(node); return node; },
    accessSection: () => el("section"), connectionsSection: () => el("section"), buildSection: () => el("section"),
    renderHeading() { guests.push(state.guest); }, renderChats() {}, renderApps() {}, leaveSidebar() {}, renderAgentFaces() {}, renderComputer() {},
    usd() {}, noSeatSaid() {}, switchForm: () => el("form"),
    SETTINGS: "/settings", WELCOME: "/welcome", PITCH: "Sign in", SUBTITLE: "Welcome",
    at() {}, store: { set() {} }, CURRENT: "current", byName: () => ({}), markSeen() {}, prewake() {}, navigated() {},
    location: { pathname: "/", search: "" }, load: async () => {}, watchList() {}, reopenWindows() {}, clearSplashEdge() {},
  });
  runInContext(`${apiError}\nlet writing = 0;\n${identityState}\n${identity}\n${source}`, context);
  const answer = (r, body) => r.resolve({ ok: true, json: async () => body });
  return {
    state, nodes, gates, data, guests,
    open: () => runInContext("openSettings(false)", context), chat: () => runInContext('openChat("chat", false)', context), signIn: () => runInContext("signIn()", context),
    release: () => { active = false; for (const gate of gates) gate.resolve(); },
    async identity(id) {
      identityStarting = true;
      const pending = runInContext('start("chat")', context);
      const request = requests.at(-1);
      assert.equal(request.path, "/api/identities/me");
      answer(request, { id, email: `${id}@example.test` });
      await turn();
      assert.equal(state.me.id, id, "production start accepts the identity response");
      const seat = requests.at(-1);
      assert.equal(seat.path, "/api/seat");
      identityStarting = false;
      return async () => { answer(seat, { guest: false }); await pending; identityStarting = false; };
    },
  };
}
for (const stage of ["returning", "reads", "billing", "skills"]) {
  test(`${stage}: an older Settings load cannot replace a newer completed load`, async () => {
    const h = setup(stage);
    const old = h.open(); await turn();
    assert.ok(h.gates.length);
    const oldCount = h.gates.length;
    const latest = h.open(); await turn();
    for (const gate of h.gates.slice(oldCount)) gate.resolve();
    await latest;
    const shown = h.nodes["settings-page"].children;
    h.release(); await old;
    assert.ok(h.nodes["settings-page"].children === shown, "the current page DOM was not replaced");
    assert.equal(h.data.length, 1, "discarded loads do not construct polling export sections");
  });
  for (const leave of ["chat", "signIn"]) {
    test(`${stage}: production ${leave} navigation prevents late rendering`, async () => {
      const h = setup(stage);
      const pending = h.open(); await turn();
      h[leave]();
      const shown = h.nodes["settings-page"].children, count = h.guests.length;
      h.release(); await pending;
      assert.ok(h.nodes["settings-page"].children === shown, "the current page DOM was not replaced");
      assert.equal(h.guests.length, count, "stale seat data cannot run setGuest");
      assert.equal(h.data.length, 0);
    });
  }
  test(`${stage}: production start accepts a new identity before the old Settings response arrives`, async () => {
    const h = setup(stage);
    const pending = h.open(); await turn();
    const finish = await h.identity("owner-b");
    const shown = h.nodes["settings-page"].children;
    h.release(); await pending;
    assert.ok(h.nodes["settings-page"].children === shown, "the current page DOM was not replaced");
    assert.equal(h.data.length, 0);
    await finish();
  });
}
for (const guest of [true, false]) {
  test(`current ${guest ? "guest" : "member"} Settings response renders normally`, async () => {
    const h = setup(null, guest);
    await h.open();
    assert.match(text(h.nodes["settings-page"]), /a@example.test/);
    assert.equal(h.data.length, 1);
    assert.equal(h.state.guest, guest);
  });
}

for (const stage of ["returning", "reads", "billing", "skills"]) {
  test(`${stage}: an old response arriving first leaves the newer load pending`, async () => {
    const h = setup(stage);
    const old = h.open(); await turn();
    const oldGates = [...h.gates];
    const latest = h.open(); await turn();
    const loading = h.nodes["settings-page"].children;
    for (const gate of oldGates) gate.resolve();
    await old;
    assert.ok(h.nodes["settings-page"].children === loading);
    assert.equal(h.data.length, 0);
    h.release(); await latest;
    assert.match(text(h.nodes["settings-page"]), /a@example.test/);
    assert.equal(h.data.length, 1);
  });
}
test("leaving and returning to Settings does not revive the prior load", async () => {
  const h = setup("reads");
  const old = h.open(); await turn();
  h.chat();
  const latest = h.open(); await turn();
  h.release(); await Promise.all([old, latest]);
  assert.equal(h.data.length, 1);
  assert.equal(h.state.page, "Settings");
});
test("an old guest response cannot change the new identity's seat", async () => {
  const h = setup("reads", true);
  const old = h.open(); await turn();
  const finish = await h.identity("owner-b");
  h.state.guest = false;
  h.release(); await old;
  assert.equal(h.state.guest, false);
  assert.equal(h.data.length, 0);
  await finish();
});
test("switching away and back to the same identity does not revive its old request", async () => {
  const h = setup("reads");
  const old = h.open(); await turn();
  const finishB = await h.identity("owner-b"); await finishB();
  const finishA = await h.identity("owner-a"); await finishA();
  const latest = h.open(); await turn();
  h.release(); await Promise.all([old, latest]);
  assert.equal(h.data.length, 1);
  assert.match(text(h.nodes["settings-page"]), /owner-a@example.test/);
});
test("a fresh response for the same identity retires the prior identity snapshot", async () => {
  const h = setup("reads");
  const old = h.open(); await turn();
  const finish = await h.identity("owner-a");
  const loading = h.nodes["settings-page"].children;
  h.release(); await old;
  assert.ok(h.nodes["settings-page"].children === loading);
  assert.equal(h.data.length, 0);
  await finish();
});
test("signed-out Settings activation starts no section requests", async () => {
  const h = setup("reads");
  h.state.me = null;
  await h.open();
  assert.equal(h.gates.length, 0);
  assert.equal(h.data.length, 0);
});
