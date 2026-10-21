// Execute the production section, Settings renderer and HTTP error handling.
// DOM recording models attachment and hidden ancestry, not browser layout.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { createContext, runInContext } from "node:vm";

const shell = readFileSync(new URL("../../../cell/shell/shell.js", import.meta.url), "utf8");
const functions = ["section", "line", "actionLine", "connectionsSection", "renderSettingsPage", "api"].map((name) => {
  const source = shell.match(new RegExp(`^(?:async )?function ${name}\\([^]*?^\\}`, "m"))?.[0];
  assert.ok(source, `the shell declares ${name}; update the fixture if it moves`);
  return source;
}).join("\n");
const settingsLoader = shell.match(/^async function openSettings\([^]*?^\}/m)?.[0];
assert.ok(settingsLoader);
const apiError = shell.match(/^class ApiError[^]*?^\}/m)?.[0];
const providers = shell.slice(shell.indexOf("const PROVIDER_KIND ="), shell.indexOf("function connectionsSection("));
assert.ok(apiError && providers.startsWith("const PROVIDER_KIND ="));
const html = readFileSync(new URL("../../../cell/shell/index.html", import.meta.url), "utf8");
const css = readFileSync(new URL("../../../cell/shell/shell.css", import.meta.url), "utf8");
assert.match(html, /<div\b[^>]*\bid="frames"[^>]*>\s*<div\b[^>]*\bid="notice"[^>]*>[\s\S]*?<\/div>\s*<\/div>\s*<section\b[^>]*\bid="settings-page"\s+hidden><\/section>/);
assert.match(css, /^\[hidden\]\s*\{\s*display:\s*none\s*!important;/m);

function setup(signIn, realRefresh = false) {
  const made = [], requests = [], refreshes = [];
  const connectionReads = [];
  let holdConnections = false;
  let documentRoot;
  function el(tag, classes, text) {
    const node = {
      tag, classes, textContent: text ?? "", children: [], dataset: {}, disabled: false, hidden: false,
      classList: { add() {} },
      get lastChild() { return this.children.at(-1); },
      get childElementCount() { return this.children.length; },
      get isConnected() {
        for (let n = this; n; n = n.parent) if (n === documentRoot) return true;
        return false;
      },
      append(...children) {
        for (const child of children) {
          this.children.push(child);
          if (typeof child === "object") child.parent = this;
        }
      },
      replaceChildren(...children) {
        for (const child of this.children) if (typeof child === "object") child.parent = null;
        this.children = [];
        this.append(...children);
      },
      setAttribute(name, value) { this[name] = value; },
      removeAttribute(name) { delete this[name]; },
      querySelector(selector) {
        if (selector === ":scope > h2") return this.children.find((child) => child.tag === "h2") ?? null;
        const match = (node) => {
          if (selector === ".settings-value") return node.classes === "settings-value";
          if (selector === "[data-removal-error]") return node.dataset.removalError !== undefined;
          if (selector.startsWith("[data-action=")) return node.dataset.action === selector.slice(13, -1);
          throw new Error(`unsupported selector ${selector}`);
        };
        const walk = (node) => match(node) ? node : node.children.filter((child) => typeof child === "object").map(walk).find(Boolean);
        return this.children.filter((child) => typeof child === "object").map(walk).find(Boolean) ?? null;
      },
      getBoundingClientRect() { return { top: 0 }; },
    };
    made.push(node);
    return node;
  }
  documentRoot = el("main");
  const frames = el("div"), notice = el("div"), page = el("section");
  documentRoot.append(frames, page);
  frames.append(notice);
  frames.hidden = true;
  const visible = (node) => {
    for (let n = node; n; n = n.parent) {
      if (n.hidden) return false;
      if (n === documentRoot) return true;
    }
    return false;
  };
  const state = { page: "Settings", me: { id: "owner-a" }, computer: null, agents: new Map() };
  const provider = { provider: "test/provider", kind: "own", state: "set" };
  if (signIn) provider.signIn = { manage: "https://provider.example/keys" };
  const backend = realRefresh ? [provider, { ...provider, provider: "second/provider" }] : [provider];
  const context = createContext({
    el, state, page, linked: { providers: structuredClone(backend) },
    $: (id) => ({ layout: documentRoot, frames, "settings-page": page })[id] ?? made.find((node) => node.id === id && node.isConnected),
    heard() {},
    fetch: (path, options) => {
      if (realRefresh && options.method === "GET") {
        const body = path === "/api/connections" ? { providers: structuredClone(backend) } : {};
        assert.ok(["/api/connections", "/api/ledger", "/api/seat", "/api/access"].includes(path));
        if (path === "/api/connections" && holdConnections) {
          return new Promise((resolve) => connectionReads.push(() => resolve({ ok: true, json: async () => body })));
        }
        return Promise.resolve({ ok: true, json: async () => body });
      }
      return new Promise((resolve, reject) => requests.push({ path, options, resolve, reject }));
    },
    openSettings: async (push) => { refreshes.push(push); page.replaceChildren(el("p", "muted", "Loading…")); },
    renderHeading() {}, renderChats() {}, leaveSidebar() {}, renderAgentFaces() {}, SETTINGS: "/settings",
    returning: async () => { refreshes.push(false); return null; },
    usd() {}, noSeatSaid() {}, switchForm: () => el("form"), setGuest: (guest) => { state.guest = guest; },
    billingSections: async () => [], dataSection: () => el("section"), accessSection: () => el("section"),
    skillsSection: async () => el("section"), buildSection: () => el("section"),
  });
  runInContext(`${apiError}\nlet writing = 0;\n${providers}\n${functions}\n${realRefresh ? settingsLoader : ""}\nrenderSettingsPage(page, [connectionsSection(linked, null)]);`, context);
  const row = made.find((node) => node.classes === "provider");
  const button = made.find((node) => node.tag === "button" && node.textContent === (signIn ? "Disconnect" : "Remove"));
  const status = made.find((node) => node.classes?.startsWith("provider-state"));
  const feedback = () => made.find((node) => node.role === "status");
  return { state, page, frames, visible, requests, refreshes, row, button, status, feedback,
    connectionReads, holdRefreshes: () => { holdConnections = true; },
    rowFor: (provider) => made.find((node) => node.dataset.provider === provider && node.isConnected),
    finish: (index, status) => {
      const request = requests[index];
      if (status === 200) backend.find((p) => request.path === `/api/connections/${encodeURIComponent(p.provider)}/key`).state = "not_set";
      respond(request, status);
    },
    writing: () => runInContext("writing", context) };
}

for (const signIn of [true, false]) {
  for (const outcome of [200, 500]) {
    test(`${signIn ? "Disconnect" : "Remove"}: second concurrent ${outcome} reconciles after the first real Settings reload`, { timeout: 1000 }, async () => {
      const h = setup(signIn, true);
      const action = signIn ? "disconnect" : "remove";
      const second = h.rowFor("second/provider").querySelector(`[data-action=${action}]`);
      const firstDone = h.button.onclick(), secondDone = second.onclick();
      assert.equal(h.requests.length, 2, "unrelated providers are not serialized");
      h.finish(0, 200);
      await firstDone;
      assert.equal(h.row.isConnected, false, "the real loader replaced the section");
      assert.equal(h.rowFor("second/provider").dataset.state, "set");
      const currentButton = h.rowFor("second/provider").querySelector(`[data-action=${action}]`);
      assert.equal(currentButton.disabled, true, "the current row still knows its removal is pending");
      await currentButton.onclick();
      assert.equal(h.requests.length, 2);
      h.finish(1, outcome);
      await secondDone;
      const current = h.rowFor("second/provider");
      if (outcome === 200) {
        assert.equal(h.refreshes.length, 2);
        assert.equal(current.dataset.state, "not_set", "confirmed deletion reaches the current page");
      } else {
        assert.equal(h.refreshes.length, 1, "a failed DELETE does not refresh");
        assert.equal(current.dataset.state, "set");
        const feedback = current.querySelector("[data-removal-error]");
        assert.ok(feedback, "the current provider row has failure feedback");
        assert.equal(h.visible(feedback), true);
        assert.equal(feedback.textContent, "Could not confirm removal. Try again.");
        assert.equal(current.querySelector(`[data-action=${action}]`).disabled, false);
      }
    });
  }
  for (const outcome of [200, 500]) {
    test(`${signIn ? "Disconnect" : "Remove"}: second ${outcome} while the first reload is pending reaches the final row`, { timeout: 1000 }, async () => {
      const h = setup(signIn, true);
      h.holdRefreshes();
      const action = signIn ? "disconnect" : "remove";
      const second = h.rowFor("second/provider").querySelector(`[data-action=${action}]`);
      const firstDone = h.button.onclick(), secondDone = second.onclick();
      h.finish(0, 200);
      await new Promise(setImmediate);
      assert.equal(h.connectionReads.length, 1, "first reload captured the second key as still set");
      h.finish(1, outcome);
      await new Promise(setImmediate);
      assert.equal(h.connectionReads.length, 1, "reloads cannot race; DELETEs already ran concurrently");
      h.connectionReads[0]();
      await firstDone;
      if (outcome === 200) {
        await new Promise(setImmediate);
        assert.equal(h.connectionReads.length, 2);
        h.connectionReads[1]();
      }
      await secondDone;
      const current = h.rowFor("second/provider");
      assert.equal(current.dataset.state, outcome === 200 ? "not_set" : "set");
      if (outcome !== 200) {
        const feedback = current.querySelector("[data-removal-error]");
        assert.equal(h.visible(feedback), true);
        assert.equal(feedback.textContent, "Could not confirm removal. Try again.");
        assert.equal(current.querySelector(`[data-action=${action}]`).disabled, false);
      }
    });
  }
}

function respond(request, status) {
  request.resolve({ ok: status === 200, status, statusText: "untrusted status text",
    json: async () => status === 200 ? { provider: "test/provider", state: "not_set" } : { error: "refused", message: "secret-must-not-appear" } });
}

for (const destination of ["chat", "another account"]) {
  test(`a queued successful removal cannot reopen Settings after ${destination}`, { timeout: 1000 }, async () => {
    const h = setup(true, true);
    h.holdRefreshes();
    const second = h.rowFor("second/provider").querySelector("[data-action=disconnect]");
    const firstDone = h.button.onclick(), secondDone = second.onclick();
    h.finish(0, 200);
    await new Promise(setImmediate);
    h.finish(1, 200);
    await new Promise(setImmediate);
    if (destination === "chat") { h.state.page = null; h.page.hidden = true; }
    else h.state.me = { id: "owner-b" };
    h.connectionReads[0]();
    await Promise.all([firstDone, secondDone]);
    assert.equal(h.refreshes.length, 1, "the queued refresh is discarded at execution time");
    assert.equal(h.connectionReads.length, 1);
    if (destination === "chat") assert.equal(h.page.hidden, true);
  });
}

for (const signIn of [true, false]) {
  const label = signIn ? "Disconnect" : "Remove";
  test(`${label}: stays pending until confirmed, then refreshes exactly once`, { timeout: 1000 }, async () => {
    const h = setup(signIn);
    const pending = h.button.onclick();
    assert.equal(h.button.disabled, true);
    assert.equal(h.writing(), 1);
    const duplicate = h.button.onclick();
    await new Promise(setImmediate);
    assert.equal(h.requests.length, 1, "duplicate activation while pending does not write twice");
    await duplicate;
    assert.equal(h.refreshes.length, 0);
    const request = h.requests[0];
    assert.equal(request.path, "/api/connections/test%2Fprovider/key");
    assert.equal(request.options.method, "DELETE");
    assert.equal(request.options.credentials, "same-origin");
    assert.equal(request.options.headers["x-fragment-shell"], "1");
    assert.equal(request.options.body, undefined);
    assert.equal(h.row.dataset.state, "set");
    respond(request, 200);
    await pending;
    assert.deepEqual(h.refreshes, [false]);
    assert.equal(h.writing(), 0);
  });

  for (const failure of [403, 500, "transport"]) {
    test(`${label}: ${failure} preserves state, shows safe local feedback and allows explicit retry`, { timeout: 1000 }, async () => {
      const h = setup(signIn);
      const pending = h.button.onclick();
      if (failure === "transport") h.requests[0].reject(new Error("secret-must-not-appear"));
      else respond(h.requests[0], failure);
      await pending;
      await new Promise(setImmediate);
      assert.equal(h.requests.length, 1, "no automatic retry");
      assert.deepEqual(h.refreshes, [], "failed DELETE never refreshes");
      assert.equal(h.row.dataset.state, "set");
      assert.equal(h.status.textContent, signIn ? "Connected" : "Key set");
      assert.equal(h.button.textContent, label);
      assert.equal(h.button.disabled, false);
      assert.equal(h.writing(), 0);
      const feedback = h.feedback();
      assert.ok(feedback, "failure has section-local feedback");
      assert.equal(feedback.parent, h.row);
      assert.equal(h.visible(feedback), true, "the error is visible despite hidden frames");
      assert.equal(feedback.textContent, "Could not confirm removal. Try again.");
      const retry = h.button.onclick();
      assert.equal(h.requests.length, 2);
      assert.equal(feedback.hidden, true);
      assert.equal(feedback.textContent, "");
      respond(h.requests[1], 200);
      await retry;
      assert.deepEqual(h.refreshes, [false]);
    });
  }

  for (const destination of ["chat", "another account", "sign-out"]) {
    for (const outcome of [200, 500]) {
      test(`${label}: late ${outcome} after ${destination} cannot refresh or show an old error`, { timeout: 1000 }, async () => {
        const h = setup(signIn);
        const pending = h.button.onclick();
        if (destination === "chat") { h.state.page = null; h.page.hidden = true; }
        if (destination === "another account") h.state.me = { id: "owner-b" };
        if (destination === "sign-out") h.state.me = null;
        respond(h.requests[0], outcome);
        await pending;
        assert.deepEqual(h.refreshes, []);
        assert.equal(h.feedback().hidden, true);
        assert.equal(h.feedback().textContent, "");
        assert.equal(h.requests.length, 1);
        assert.equal(h.writing(), 0);
      });
    }
  }
}
