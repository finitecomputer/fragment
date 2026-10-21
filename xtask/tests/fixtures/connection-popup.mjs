// Actual Connect handlers and API; synthetic popup, DOM, HTTP and lifecycle.
// No real OAuth navigation, credentials, grants or browser permissions.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test as nodeTest } from "node:test";
import { createContext, runInContext } from "node:vm";
const test = (name, run) => nodeTest(name, { timeout: 1000 }, run);
const shell = readFileSync(new URL("../../../cell/shell/shell.js", import.meta.url), "utf8");
const source = ["connectionsSection", "section", "api"].map((name) => {
  const fn = shell.match(new RegExp(`^(?:async )?function ${name}\\([^]*?^\\}`, "m"))?.[0];
  assert.ok(fn, `production ${name} exists`); return fn;
}).join("\n");
const constants = shell.slice(shell.indexOf("const PROVIDER_KIND ="), shell.indexOf("function connectionsSection("));
const apiError = shell.match(/^class ApiError[^]*?^\}/m)?.[0];
const walk = (node) => [node, ...node.children.filter((n) => typeof n === "object").flatMap(walk)];
const url = "https://provider.example.test/authorize?state=fake-state";

function setup(kind, blocked = false) {
  const requests = [], opens = [], observers = [], timers = new Map(), events = new Map(), refreshes = [];
  let serial = 0, failNavigation = false, root;
  function el(tag, classes, text) {
    return {
      tag, classes, textContent: text ?? "", children: [], dataset: {}, hidden: false, disabled: false,
      get childElementCount() { return this.children.length; },
      get isConnected() { for (let n = this; n; n = n.parent) if (n === root) return true; return false; },
      append(...nodes) { for (const node of nodes) { this.children.push(node); if (typeof node === "object") node.parent = this; } },
      replaceChildren(...nodes) { for (const node of this.children) if (typeof node === "object") node.parent = null; this.children = []; this.append(...nodes); },
      setAttribute(name, value) { this[name] = value; },
    };
  }
  root = el("main");
  const page = el("section"), frames = el("div"); frames.hidden = true; root.append(frames, page);
  const state = { me: { id: "owner-a" }, page: "Settings", computer: null };
  const popup = { closed: false, closes: 0, navigations: [], close() { this.closed = true; this.closes++; }, location: { replace(value) { if (failNavigation) throw new Error("secret-navigation-detail"); popup.navigations.push(value); } } };
  class Observer {
    constructor(callback) { this.callback = callback; this.active = true; observers.push(this); }
    observe(target, options) { assert.equal(target, root); assert.equal(options.subtree, true); }
    disconnect() { this.active = false; }
  }
  const provider = kind === "own" ? { provider: "test/provider", kind, state: "not_set", signIn: { manage: "https://provider.example.test/keys" } } : { provider: "test/provider", kind, state: "not_connected" };
  const context = createContext({
    state, el, URL, $: (id) => ({ layout: root, "settings-page": page })[id], provider,
    window: { open(...args) { opens.push(args); if (blocked === "throw") throw new Error("secret-popup-detail"); return blocked ? null : popup; } },
    fetch: (path, options) => new Promise((resolve, reject) => requests.push({ path, options, resolve, reject })), heard() {},
    MutationObserver: Observer,
    addEventListener: (name, cb) => { if (!events.has(name)) events.set(name, new Set()); events.get(name).add(cb); },
    removeEventListener: (name, cb) => { events.get(name)?.delete(cb); if (!events.get(name)?.size) events.delete(name); },
    setTimeout: (cb, delay) => { assert.equal(delay, 500); const id = ++serial; timers.set(id, cb); return id; }, clearTimeout: (id) => timers.delete(id),
    openSettings: async (push) => { refreshes.push(push); page.replaceChildren(el("p", "muted", "Loading…")); },
  });
  runInContext(`${apiError}\nlet writing = 0;\n${constants}\n${source}`, context);
  function mount() { const section = runInContext("connectionsSection({ providers: [provider] }, null)", context); page.replaceChildren(section); return walk(section).find((n) => n.tag === "button"); }
  const go = mount();
  return {
    state, root, page, popup, requests, opens, observers, timers, events, refreshes, go, mount,
    another() { const section = runInContext('connectionsSection({ providers: [{ ...provider, provider: "second/provider" }] }, null)', context); page.append(section); return walk(section).find((n) => n.tag === "button"); },
    tell: () => walk(page).find((n) => n.role === "status"),
    text: () => walk(page).map((n) => n.textContent).join(" "),
    visible(node) { for (let n = node; n; n = n.parent) { if (n.hidden) return false; if (n === root) return true; } return false; },
    block(value) { blocked = value; }, failNavigation(value = true) { failNavigation = value; },
    notify: () => observers.filter((o) => o.active).forEach((o) => o.callback()),
    event: (name) => [...events.get(name) ?? []].forEach((cb) => cb()),
    tick() { assert.equal(timers.size, 1); const [id, cb] = timers.entries().next().value; timers.delete(id); cb(); },
  };
}
function answer(request, body = { url }, status = 200) {
  request.resolve({ ok: status < 400, status, statusText: "secret-status", json: async () => status < 400 ? body : { message: "secret-server-detail" } });
}
function clean(h) {
  assert.equal(h.timers.size, 0); assert.equal(h.events.size, 0);
  assert.ok(h.observers.every((o) => !o.active));
}
for (const kind of ["connection", "own"]) {
  for (const blocked of [true, "throw"]) {
    test(`${kind}: blocked window (${blocked}) makes no authorization and offers visible retry`, async () => {
      const h = setup(kind, blocked);
      await h.go.onclick();
      assert.equal(h.requests.length, 0);
      assert.equal(h.go.disabled, false);
      assert.match(h.text(), /browser blocked/);
      assert.equal(h.visible(h.tell()), true);
      assert.doesNotMatch(h.text(), /secret-/);
      clean(h);
      h.block(false);
      const retry = h.go.onclick();
      assert.equal(h.requests.length, 1);
      answer(h.requests[0]); await retry;
      assert.deepEqual(h.popup.navigations, [url]);
    });
  }
  test(`${kind}: opens within the click before HTTP, keeps one attempt, refreshes once on close`, async () => {
    const h = setup(kind);
    const pending = h.go.onclick();
    assert.deepEqual(h.opens, [["", "_blank", "popup,width=520,height=720"]]);
    assert.equal(h.requests.length, 1);
    assert.equal(h.requests[0].options.method, "POST");
    assert.equal(h.requests[0].path, "/api/connections/test%2Fprovider/authorize");
    await h.go.onclick();
    assert.equal(h.requests.length, 1);
    answer(h.requests[0]); await pending;
    assert.equal(h.go.disabled, true);
    await h.go.onclick();
    h.event("focus"); h.event("focus");
    assert.equal(h.timers.size, 1);
    assert.equal(h.requests.length, 1);
    assert.equal(h.refreshes.length, 0, "returning focus alone cannot supersede an unfinished authorization");
    h.popup.closed = true;
    h.event("focus"); h.event("focus");
    assert.deepEqual(h.refreshes, [false]);
    clean(h);
  });
  for (const failure of [403, 500, "transport", "missing-url", "invalid-url", "navigation"]) {
    test(`${kind}: ${failure} initialization failure closes the popup and enables explicit retry`, async () => {
      const h = setup(kind);
      if (failure === "navigation") h.failNavigation();
      const pending = h.go.onclick();
      if (failure === "transport") h.requests[0].reject(new Error("secret-transport-detail"));
      else answer(h.requests[0], failure === "missing-url" ? {} : failure === "invalid-url" ? { url: "javascript:secret()" } : { url }, typeof failure === "number" ? failure : 200);
      await pending;
      assert.equal(h.popup.closed, true);
      assert.equal(h.go.disabled, false);
      assert.equal(h.refreshes.length, 0);
      assert.match(h.text(), /Sign-in could not start/);
      assert.doesNotMatch(h.text(), /secret-/);
      assert.equal(h.visible(h.tell()), true);
      clean(h);
      h.popup.closed = false; h.failNavigation(false);
      const retry = h.go.onclick();
      assert.equal(h.requests.length, 2, "only the explicit retry starts another authorization");
      answer(h.requests[1]); await retry;
      assert.deepEqual(h.popup.navigations, [url]);
    });
  }
  test(`${kind}: closing before initialization ignores the late URL and retains the request lock until settled`, async () => {
    const h = setup(kind);
    const pending = h.go.onclick();
    h.popup.closed = true; h.tick();
    clean(h);
    assert.equal(h.go.disabled, true);
    await h.go.onclick();
    assert.equal(h.requests.length, 1);
    answer(h.requests[0]); await pending;
    assert.equal(h.go.disabled, false);
    assert.deepEqual(h.popup.navigations, []);
    assert.equal(h.refreshes.length, 0);
    assert.match(h.text(), /window was closed/);
  });
  for (const departure of ["chat", "replacement", "identity", "signout", "hidden", "pagehide"]) {
    for (const initialized of [false, true]) {
      test(`${kind}: ${departure} cleans up ${initialized ? "initialized" : "pending"} authorization`, async () => {
        const h = setup(kind);
        const pending = h.go.onclick();
        if (initialized) { answer(h.requests[0]); await pending; }
        if (departure === "chat") { h.state.page = null; h.page.hidden = true; }
        if (departure === "replacement") h.page.replaceChildren();
        if (departure === "identity") h.state.me = { id: "owner-b" };
        if (departure === "signout") h.state.me = null;
        if (departure === "hidden") h.root.hidden = true;
        if (departure === "pagehide") h.event("pagehide");
        else h.notify();
        clean(h);
        assert.equal(h.popup.closed, true);
        if (!initialized) { answer(h.requests[0]); await pending; assert.deepEqual(h.popup.navigations, []); }
        h.event("focus");
        assert.equal(h.refreshes.length, 0);
        assert.equal(h.requests.length, 1);
      });
    }
  }
  test(`${kind}: account change with no DOM callback is caught by the next poll`, async () => {
    const h = setup(kind);
    const pending = h.go.onclick(); answer(h.requests[0]); await pending;
    h.state.me = { id: "owner-b" }; h.tick();
    clean(h); assert.equal(h.popup.closed, true); assert.equal(h.refreshes.length, 0);
  });
  test(`${kind}: another provider cannot supersede the active browser-bound attempt`, async () => {
    const h = setup(kind), second = h.another();
    const pending = h.go.onclick();
    const blocked = second.onclick();
    assert.equal(h.requests.length, 1);
    await blocked;
    answer(h.requests[0]); await pending;
    const stillBlocked = second.onclick();
    assert.equal(h.requests.length, 1);
    await stillBlocked;
    assert.equal(h.opens.length, 1);
    h.popup.closed = true; h.tick();
    clean(h);
    assert.deepEqual(h.refreshes, [false]);
  });
  test(`${kind}: an immediately closed popup makes no authorization`, async () => {
    const h = setup(kind);
    h.popup.closed = true;
    await h.go.onclick();
    assert.equal(h.requests.length, 0);
    assert.equal(h.go.disabled, false);
    assert.match(h.text(), /window was closed/);
    clean(h);
  });
  test(`${kind}: replacing Settings during a pending POST cannot start a second authorization`, async () => {
    const h = setup(kind);
    const pending = h.go.onclick();
    const newButton = h.mount(); h.notify();
    await newButton.onclick();
    assert.equal(h.requests.length, 1);
    answer(h.requests[0]); await pending;
    h.popup.closed = false;
    const retry = newButton.onclick();
    assert.equal(h.requests.length, 2);
    // A queued callback from the old observer cannot clear the new attempt.
    h.observers[0].callback();
    await newButton.onclick();
    assert.equal(h.requests.length, 2);
    answer(h.requests[1]); await retry;
    assert.deepEqual(h.popup.navigations, [url]);
  });
}
