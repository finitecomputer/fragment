// Actual Settings loader, Connections builder, usage loader, API and navigation.
// Synthetic DOM/HTTP/MutationObserver; unrelated sections are stubbed. No browser.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test as nodeTest } from "node:test";
import { createContext, runInContext } from "node:vm";
const test = (name, run) => nodeTest(name, { timeout: 1000 }, run);
const shell = readFileSync(new URL("../../../cell/shell/shell.js", import.meta.url), "utf8");
const names = ["openSettings", "renderSettingsPage", "connectionsSection", "loadConnectionUsage", "section", "line", "actionLine", "api", "openChat", "start", "setGuest", "signIn", "firstRun"];
const source = names.map((name) => {
  const found = shell.match(new RegExp(`^(?:async )?function ${name}\\([^]*?^\\}`, "m"))?.[0];
  assert.ok(found, `production ${name} exists`);
  return found;
}).join("\n");
const apiError = shell.match(/^class ApiError[^]*?^\}/m)?.[0];
// Execute identity/action helpers when the separately reviewed guards compose
// with this change; never replace their identity transition with a fixture stub.
const identity = shell.match(/^function identityIs\([^]*?^\}/m)?.[0] ?? "";
const identityState = identity ? [shell.match(/^const reads = .*;$/m)?.[0], ...["woke", "making", "restarted"].map((name) => shell.match(new RegExp(`^let ${name} = .*;$`, "m"))?.[0])].join("\n") : "";
const actionCurrent = shell.match(/^function settingsActionCurrent\([^]*?^\}/m)?.[0] ?? "";
const money = shell.match(/^const money = .*;$/m)?.[0];
assert.ok(apiError && money);
const html = readFileSync(new URL("../../../cell/shell/index.html", import.meta.url), "utf8");
assert.match(html, /id="frames"[^>]*>[\s\S]*?<section class="page" id="settings-page" hidden>/);
const css = readFileSync(new URL("../../../cell/shell/shell.css", import.meta.url), "utf8");
assert.match(css, /^\[hidden\]\s*\{\s*display:\s*none\s*!important;/m);
const turn = () => new Promise(setImmediate);
const walk = (node) => [node, ...node.children.filter((n) => typeof n === "object").flatMap(walk)];
const text = (node) => walk(node).map((n) => n.textContent).join(" ");
const providers = [{ provider: "paid", kind: "operator", state: "offered" }, { provider: "other", kind: "operator", state: "offered" }];
const usage = { computer: "computer-a", month: "2026-10", uses: [{ provider: "paid", agent: "agent-a", calls: 3, micros: 7500 }] };
function answer(request, body, status = 200) {
  request.resolve({ ok: status < 400, status, statusText: "private-detail", json: async () => status < 400 ? body : { message: "secret-must-not-appear" } });
}
function setup({ computer = true, offered = providers } = {}) {
  const requests = [], observers = [], events = new Map();
  let identityStarting = false;
  let root;
  function el(tag, classes, value) {
    return {
      tag, classes, textContent: value ?? "", children: [], dataset: {}, hidden: false, disabled: false,
      classList: { add() {}, remove() {} },
      get isConnected() { for (let n = this; n; n = n.parent) if (n === root) return true; return false; },
      append(...nodes) { for (const n of nodes) { this.children.push(n); if (typeof n === "object") n.parent = this; } },
      replaceChildren(...nodes) { for (const n of this.children) if (typeof n === "object") n.parent = null; this.children = []; this.append(...nodes); },
      get lastChild() { return this.children.at(-1); }, get childElementCount() { return this.children.length; },
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
  root = nodes.layout; root.append(nodes.frames, nodes["settings-page"]);
  const state = { me: { id: "owner-a", email: "a@example.test" }, page: null, computer: computer ? { computer: "computer-a", phase: "asleep", agents: [] } : null, agents: new Map(), frames: new Map([["chat", el("iframe")]]), fragments: [{}], guest: false };
  class Observer {
    constructor(callback) { this.callback = callback; this.active = true; observers.push(this); }
    observe(target, options) { assert.equal(target, root); assert.equal(options.subtree, true); assert.deepEqual([...options.attributeFilter], ["hidden"]); }
    disconnect() { this.active = false; }
  }
  const context = createContext({
    state, el, $: (id) => nodes[id], heard() {}, MutationObserver: Observer,
    addEventListener: (name, callback) => { if (!events.has(name)) events.set(name, new Set()); events.get(name).add(callback); },
    removeEventListener: (name, callback) => events.get(name)?.delete(callback),
    fetch: (path, options) => {
      if (path.endsWith("/uses") || path === "/api/identities/me" || path === "/api/seat" && identityStarting) return new Promise((resolve, reject) => requests.push({ path, options, resolve, reject }));
      assert.equal(options.method, "GET");
      const body = path === "/api/seat" ? { guest: false } : path === "/api/connections" ? { providers: offered } : {};
      return Promise.resolve({ ok: true, json: async () => body });
    },
    returning: async () => null, billingSections: async () => [el("section")], skillsSection: async () => el("section"),
    dataSection: () => el("section"), accessSection: () => el("section"), buildSection: () => el("section"),
    renderHeading() {}, renderChats() {}, renderApps() {}, leaveSidebar() {}, renderAgentFaces() {}, renderComputer() {},
    usd() {}, noSeatSaid() {}, switchForm: () => el("form"), seg: encodeURIComponent, providerName: (p) => p, titleOf: (n) => n,
    PHASE: {}, STOPPED: {}, WHY: {}, PROVIDER_KIND: {}, PROVIDER_STATE: {}, SETTINGS: "/settings", WELCOME: "/welcome", PITCH: "Sign in", SUBTITLE: "Welcome",
    at() {}, store: { set() {} }, CURRENT: "current", byName: () => ({}), markSeen() {}, prewake() {}, navigated() {},
    location: { pathname: "/", search: "" }, load: async () => {}, watchList() {}, reopenWindows() {}, clearSplashEdge() {},
  });
  runInContext(`${apiError}\nlet writing = 0;\n${identityState}\n${identity}\n${actionCurrent}\n${money}\n${source}`, context);
  const notify = () => observers.filter((o) => o.active).forEach((o) => o.callback());
  const section = () => walk(nodes["settings-page"]).find((n) => n.id === "settings-connections");
  const rows = () => walk(section()).filter((n) => n.classes === "provider-uses");
  const retry = () => walk(section()).find((n) => n.dataset.action === "connection-usage-retry");
  return {
    state, nodes, requests, observers, events, notify, section, rows, retry, context,
    async open() { await runInContext("openSettings(false)", context); notify(); },
    chat() { runInContext('openChat("chat", false)', context); notify(); },
    pagehide() { for (const callback of events.get("pagehide") ?? []) callback(); },
    async beginIdentity(id) {
      identityStarting = true;
      const pending = runInContext('start("chat")', context);
      answer(requests.at(-1), { id, email: `${id}@example.test` });
      await turn();
      assert.equal(state.me.id, id, "production start changes the identity");
      const seat = requests.at(-1);
      assert.equal(seat.path, "/api/seat");
      identityStarting = false;
      return async () => { answer(seat, { guest: false }); await pending; notify(); };
    },
    async identity(id) {
      const finish = await this.beginIdentity(id);
      await finish();
    },
    async signout() {
      const pending = runInContext('start("chat")', context);
      answer(requests.at(-1), {}, 401);
      await pending; notify();
      assert.equal(nodes.layout.hidden, true, "production sign-in hides Settings");
    },
    text: () => text(section()),
    visible(node) { for (let n = node; n; n = n.parent) { if (n.hidden) return false; if (n === root) return true; } return false; },
    usageRequests: () => requests.filter((r) => r.path.endsWith("/uses")),
  };
}

test("Settings mounts while usage is pending; all providers share one GET", async () => {
  const h = setup(); await h.open();
  assert.equal(h.nodes.frames.hidden, true);
  assert.equal(h.visible(h.section()), true);
  assert.equal(h.rows().length, 2);
  assert.ok(h.rows().every((r) => r.dataset.state === "loading"));
  assert.doesNotMatch(h.text(), /No calls yet|could not be read/);
  assert.equal(h.usageRequests().length, 1);
  h.notify(); h.notify();
  assert.equal(h.usageRequests().length, 1);
  answer(h.usageRequests()[0], usage); await turn();
  assert.match(h.text(), /3 calls, \$0.0075/);
  assert.equal(h.rows()[0].dataset.state, "ready");
  assert.match(text(h.rows()[1]), /No calls yet/);
  assert.ok(h.observers.every((o) => !o.active));
  assert.equal(h.events.get("pagehide").size, 0);
});

for (const failure of [403, 500, "transport", "malformed", "wrong-computer"]) {
  test(`${failure}: unavailable is not empty; explicit retry reads only the same computer`, async () => {
    const h = setup(); await h.open();
    const request = h.usageRequests()[0];
    if (failure === "transport") request.reject(new Error("secret-must-not-appear"));
    else if (failure === "malformed") answer(request, {});
    else if (failure === "wrong-computer") answer(request, { ...usage, computer: "computer-b" });
    else answer(request, {}, failure);
    await turn();
    assert.ok(h.rows().every((r) => r.dataset.state === "error"));
    assert.doesNotMatch(h.text(), /No calls yet|secret-must-not-appear|private-detail/);
    const retry = h.retry();
    assert.equal(h.visible(retry), true);
    assert.equal(retry.disabled, false);
    const pending = retry.onclick();
    assert.equal(retry.disabled, true);
    assert.ok(h.rows().every((r) => r.dataset.state === "loading"));
    const repeated = retry.onclick(); h.notify();
    assert.equal(h.usageRequests().length, 2, "no duplicate retry or observer-triggered GET");
    await repeated;
    answer(h.usageRequests()[1], { ...usage, uses: [] }); await pending;
    assert.ok(h.rows().every((r) => r.dataset.state === "ready" && text(r).includes("No calls yet")));
    assert.doesNotMatch(h.text(), /Usage unavailable|could not be read|Loading usage/);
    await retry.onclick();
    assert.equal(h.usageRequests().length, 2, "completed reads have no leftover retry action");
    assert.ok(h.usageRequests().every((r) => r.path === "/api/computers/computer-a/uses" && r.options.method === "GET" && r.options.body === undefined));
  });
}

const departures = {
  chat: async (h) => h.chat(),
  "away and back": async (h) => { h.chat(); await h.open(); },
  "Settings refresh": async (h) => h.open(),
  "account switch": async (h) => h.identity("owner-b"),
  "same-id identity replacement": async (h) => h.identity("owner-a"),
  "account switch and return": async (h) => { await h.identity("owner-b"); await h.identity("owner-a"); await h.open(); },
  signout: async (h) => h.signout(),
  "computer replacement": async (h) => { h.state.computer = { computer: "computer-b", agents: [] }; h.notify(); },
  pagehide: async (h) => h.pagehide(),
};
for (const [name, depart] of Object.entries(departures)) {
  for (const failed of [false, true]) test(`${name}: a late ${failed ? "failure" : "success"} cannot update the old section`, async () => {
    const h = setup(); await h.open();
    const old = h.section(), oldRows = h.rows(), content = oldRows.map((r) => r.children);
    const request = h.usageRequests()[0];
    await depart(h);
    if (failed) request.reject(new Error("secret-must-not-appear")); else answer(request, usage);
    await turn();
    oldRows.forEach((r, i) => assert.ok(r.children === content[i], "the old row children stay unchanged"));
    assert.doesNotMatch(text(old), /3 calls|could not be read|secret-must-not-appear/);
    assert.equal(h.observers[0].active, false);
  });
}

test("retry captured before navigation cannot issue a GET from the old view", async () => {
  const h = setup(); await h.open(); answer(h.usageRequests()[0], {}, 500); await turn();
  const retry = h.retry(); h.chat(); await h.open();
  const count = h.usageRequests().length;
  await retry.onclick(); assert.equal(h.usageRequests().length, count);
});

for (const id of ["owner-a", "owner-b"]) test(`${id}: identity acceptance alone invalidates the read before navigation finishes`, async () => {
  const h = setup(); await h.open();
  const rows = h.rows(), content = rows.map((r) => r.children);
  const request = h.usageRequests()[0];
  const finish = await h.beginIdentity(id);
  assert.equal(h.state.page, "Settings");
  assert.equal(h.visible(h.section()), true);
  answer(request, usage); await turn();
  rows.forEach((r, i) => assert.ok(r.children === content[i], "identity acceptance prevents the old row update"));
  assert.doesNotMatch(h.text(), /3 calls/);
  assert.equal(h.observers[0].active, false);
  await finish();
});

test("no computer or no offered provider does not start a usage request", async () => {
  for (const options of [{ computer: false }, { offered: [] }]) {
    const h = setup(options); await h.open();
    assert.equal(h.usageRequests().length, 0);
    assert.equal(h.observers.length, 0);
  }
});
