// Production dataSection and API wrapper; synthetic DOM, HTTP, timers and
// MutationObserver delivery. No archive is generated and no browser runs.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { createContext, runInContext } from "node:vm";

const shell = readFileSync(new URL("../../../cell/shell/shell.js", import.meta.url), "utf8");
const functions = ["dataSection", "section", "line", "api"].map((name) => {
  const source = shell.match(new RegExp(`^(?:async )?function ${name}\\([^]*?^\\}`, "m"))?.[0];
  assert.ok(source, `the shell declares ${name}; update the fixture if it moves`);
  return source;
}).join("\n");
const apiError = shell.match(/^class ApiError[^]*?^\}/m)?.[0];
const constants = shell.slice(shell.indexOf("const EXPORT_STATE ="), shell.indexOf("function dataSection("));
assert.ok(apiError && constants.startsWith("const EXPORT_STATE ="));
const html = readFileSync(new URL("../../../cell/shell/index.html", import.meta.url), "utf8");
assert.match(html, /id="frames"[^>]*>[\s\S]*?<section class="page" id="settings-page" hidden>/);
const css = readFileSync(new URL("../../../cell/shell/shell.css", import.meta.url), "utf8");
assert.match(css, /^\[hidden\]\s*\{\s*display:\s*none\s*!important;/m);
const turn = () => new Promise(setImmediate);
const building = { id: "same-export", state: "building", progress: { done: 2, total: 7, bytes: 1024 } };
const ready = { id: "same-export", state: "ready", download: "/api/export/same-export/download", size: 4096, expiresAt: 1800000000000 };

function setup() {
  const requests = [], observers = [], timers = new Map(), events = new Map();
  let serial = 0, root;
  function el(tag, classes, text) {
    return {
      tag, classes, textContent: text ?? "", children: [], dataset: {}, hidden: false, disabled: false,
      get isConnected() { for (let n = this; n; n = n.parent) if (n === root) return true; return false; },
      append(...nodes) { for (const node of nodes) { this.children.push(node); if (typeof node === "object") node.parent = this; } },
      replaceChildren(...nodes) { for (const node of this.children) if (typeof node === "object") node.parent = null; this.children = []; this.append(...nodes); },
      setAttribute(name, value) { this[name] = value; },
    };
  }
  root = el("main");
  const page = el("section"), frames = el("div");
  frames.hidden = true;
  root.append(frames, page);
  const state = { page: "Settings", me: { id: "owner-a" } };
  class Observer {
    constructor(callback) { this.callback = callback; this.active = true; observers.push(this); }
    observe(target, options) { assert.equal(target, root); assert.equal(options.subtree, true); assert.deepEqual([...options.attributeFilter], ["hidden"]); }
    disconnect() { this.active = false; }
  }
  const context = createContext({
    el, state, $: (id) => ({ layout: root, "settings-page": page })[id], MutationObserver: Observer,
    setTimeout: (callback, delay) => { assert.equal(delay, 2000); const id = ++serial; timers.set(id, callback); return id; },
    clearTimeout: (id) => timers.delete(id),
    addEventListener: (name, callback) => events.set(name, callback),
    removeEventListener: (name, callback) => { if (events.get(name) === callback) events.delete(name); },
    fetch: (path, options) => new Promise((resolve, reject) => requests.push({ path, options, resolve, reject })), heard() {},
  });
  runInContext(`${apiError}\nlet writing = 0;\n${constants}\n${functions}`, context);
  const section = runInContext("dataSection()", context);
  const walk = (node) => [node, ...node.children.filter((child) => typeof child === "object").flatMap(walk)];
  const find = (action) => walk(section).find((node) => node.dataset.action === action);
  const shown = walk(section).find((node) => node.classes === "settings-export");
  const feedback = find("export-status-retry").parent;
  const notify = () => observers.filter((o) => o.active).forEach((o) => o.callback());
  const visible = (node) => { for (let n = node; n; n = n.parent) { if (n.hidden) return false; if (n === root) return true; } return false; };
  return { state, section, page, root, requests, timers, events, observers, shown, feedback, find, notify, visible,
    mount: () => { page.append(section); notify(); },
    tick: () => { assert.equal(timers.size, 1); const [id, callback] = timers.entries().next().value; timers.delete(id); return callback(); },
    text: () => walk(section).map((node) => node.textContent).join(" "),
  };
}
function answer(request, value, status = 200) {
  request.resolve({ ok: status < 400, status, statusText: "untrusted detail", json: async () => status < 400 ? value : { message: "secret-must-not-appear" } });
}
async function first(h, value) {
  h.mount();
  answer(h.requests[0], { export: value });
  await turn();
}

for (const failure of [500, "transport"]) {
  test(`building → ${failure} → explicit GET retry → ready preserves the same export`, async () => {
    const h = setup();
    assert.equal(h.requests.length, 0, "constructing an unmounted panel does not start polling");
    await first(h, building);
    const rows = h.shown.children;
    assert.equal(h.timers.size, 1);
    const poll = h.tick();
    if (failure === "transport") h.requests[1].reject(new Error("secret-must-not-appear"));
    else answer(h.requests[1], {}, failure);
    await poll;
    assert.equal(h.shown.children, rows, "the progress rows are not replaced by the error");
    assert.equal(h.shown.dataset.state, "building");
    assert.match(h.text(), /2 of 7 parts/);
    assert.equal(h.find("export").disabled, true);
    assert.equal(h.visible(h.feedback), true, "feedback is in Settings, outside hidden frames");
    assert.doesNotMatch(h.text(), /secret-must-not-appear/);
    assert.equal(h.timers.size, 0, "a failed status read waits for explicit retry");
    const retry = h.find("export-status-retry");
    const pending = retry.onclick();
    assert.equal(retry.disabled, true);
    await retry.onclick();
    assert.equal(h.requests.length, 3, "duplicate retry cannot create another poll loop");
    answer(h.requests[2], { export: ready });
    await pending;
    assert.equal(h.shown.dataset.state, "ready");
    assert.equal(h.find("export-download").href, ready.download);
    assert.equal(h.feedback.hidden, true);
    assert.equal(h.timers.size, 0);
    assert.ok(h.requests.every((r) => r.path === "/api/export" && r.options.method === "GET"));
  });
}

test("initial status failure offers GET-only retry without offering to create a replacement", async () => {
  const h = setup();
  h.mount();
  answer(h.requests[0], {}, 403);
  await turn();
  assert.equal(h.find("export"), undefined);
  assert.equal(h.visible(h.feedback), true);
  const pending = h.find("export-status-retry").onclick();
  answer(h.requests[1], { export: building });
  await pending;
  assert.equal(h.timers.size, 1);
  assert.equal(h.shown.dataset.state, "building");
});

test("retry while still building schedules one poll, and terminal failure stops polling", async () => {
  const h = setup();
  await first(h, building);
  // Even a direct repeated activation cannot leave the old timer running.
  const pending = h.find("export-status-retry").onclick();
  assert.equal(h.timers.size, 0);
  await h.find("export-status-retry").onclick();
  answer(h.requests[1], { export: building });
  await pending;
  assert.equal(h.timers.size, 1);
  const poll = h.tick();
  answer(h.requests[2], { export: { state: "failed", error: { message: "Archive could not be built" } } });
  await poll;
  assert.equal(h.shown.dataset.state, "failed");
  assert.equal(h.timers.size, 0);
});

const leave = {
  chat: (h) => { h.state.page = null; h.page.hidden = true; },
  replacement: (h) => h.page.replaceChildren(),
  identity: (h) => { h.state.me = { id: "owner-b" }; },
  "sign-out": (h) => { h.state.me = null; },
  welcome: (h) => { h.root.hidden = true; },
  pagehide: (h) => h.events.get("pagehide")(),
};
for (const [name, depart] of Object.entries(leave)) {
  test(`${name}: disconnect observers/listeners and cancel the scheduled poll`, async () => {
    const h = setup();
    await first(h, building);
    depart(h); h.notify();
    assert.equal(h.timers.size, 0);
    assert.ok(h.observers.every((o) => !o.active));
    assert.equal(h.events.size, 0);
    await h.find("export-status-retry").onclick();
    assert.equal(h.requests.length, 1);
  });
  test(`${name}: an in-flight response cannot update or restart the old panel`, async () => {
    const h = setup();
    await first(h, building);
    const poll = h.tick();
    const rows = h.shown.children;
    depart(h); h.notify();
    answer(h.requests[1], { export: ready });
    await poll;
    assert.equal(h.shown.children, rows);
    assert.equal(h.shown.dataset.state, "building");
    assert.equal(h.timers.size, 0);
    assert.equal(h.events.size, 0);
  });
}

test("identity change without a DOM callback is also checked before another GET", async () => {
  const h = setup();
  await first(h, building);
  h.state.me = { id: "owner-b" };
  await h.tick();
  assert.equal(h.requests.length, 1);
  assert.equal(h.timers.size, 0);
  assert.equal(h.events.size, 0);
});

test("leaving before the section mounts starts no request and releases its observer", () => {
  const h = setup();
  h.root.hidden = true;
  h.notify();
  assert.equal(h.requests.length, 0);
  assert.ok(h.observers.every((o) => !o.active));
  assert.equal(h.events.size, 0);
  h.root.hidden = false;
  h.mount();
  assert.equal(h.requests.length, 0, "a stopped section cannot restart on a later mount");
});

test("explicit export creation stays a single POST and its late result is discarded", async () => {
  const h = setup();
  await first(h, null);
  const go = h.find("export");
  const pending = go.onclick();
  await go.onclick();
  assert.equal(h.requests.length, 2);
  assert.equal(h.requests[1].options.method, "POST");
  h.state.page = null; h.page.hidden = true; h.notify();
  answer(h.requests[1], building);
  await pending;
  assert.equal(h.shown.dataset.state, "none");
  assert.equal(h.timers.size, 0);
});
