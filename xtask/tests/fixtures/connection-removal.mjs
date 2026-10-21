// Execute the production section, Settings renderer and HTTP error handling.
// DOM recording models attachment and hidden ancestry, not browser layout.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { createContext, runInContext } from "node:vm";

const shell = readFileSync(new URL("../../../cell/shell/shell.js", import.meta.url), "utf8");
const functions = ["section", "connectionsSection", "renderSettingsPage", "api"].map((name) => {
  const source = shell.match(new RegExp(`^(?:async )?function ${name}\\([^]*?^\\}`, "m"))?.[0];
  assert.ok(source, `the shell declares ${name}; update the fixture if it moves`);
  return source;
}).join("\n");
const apiError = shell.match(/^class ApiError[^]*?^\}/m)?.[0];
const providers = shell.slice(shell.indexOf("const PROVIDER_KIND ="), shell.indexOf("function connectionsSection("));
assert.ok(apiError && providers.startsWith("const PROVIDER_KIND ="));
const html = readFileSync(new URL("../../../cell/shell/index.html", import.meta.url), "utf8");
const css = readFileSync(new URL("../../../cell/shell/shell.css", import.meta.url), "utf8");
assert.match(html, /<div\b[^>]*\bid="frames"[^>]*>\s*<div\b[^>]*\bid="notice"[^>]*>[\s\S]*?<\/div>\s*<\/div>\s*<section\b[^>]*\bid="settings-page"\s+hidden><\/section>/);
assert.match(css, /^\[hidden\]\s*\{\s*display:\s*none\s*!important;/m);

function setup(signIn) {
  const made = [], requests = [], refreshes = [];
  let documentRoot;
  function el(tag, classes, text) {
    const node = {
      tag, classes, textContent: text ?? "", children: [], dataset: {}, disabled: false, hidden: false,
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
        assert.equal(selector, ":scope > h2");
        return this.children.find((child) => child.tag === "h2") ?? null;
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
  const state = { page: "Settings", me: { id: "owner-a" }, computer: null };
  const provider = { provider: "test/provider", kind: "own", state: "set" };
  if (signIn) provider.signIn = { manage: "https://provider.example/keys" };
  const context = createContext({
    el, state, page, linked: { providers: [provider] },
    heard() {},
    fetch: (path, options) => new Promise((resolve, reject) => requests.push({ path, options, resolve, reject })),
    openSettings: async (push) => { refreshes.push(push); page.replaceChildren(el("p", "muted", "Loading…")); },
  });
  runInContext(`${apiError}\nlet writing = 0;\n${providers}\n${functions}\nrenderSettingsPage(page, [connectionsSection(linked, null)]);`, context);
  const row = made.find((node) => node.classes === "provider");
  const button = made.find((node) => node.tag === "button" && node.textContent === (signIn ? "Disconnect" : "Remove"));
  const status = made.find((node) => node.classes?.startsWith("provider-state"));
  const feedback = () => made.find((node) => node.role === "status");
  return { state, page, frames, visible, requests, refreshes, row, button, status, feedback,
    writing: () => runInContext("writing", context) };
}

function respond(request, status) {
  request.resolve({ ok: status === 200, status, statusText: "untrusted status text",
    json: async () => status === 200 ? { provider: "test/provider", state: "not_set" } : { error: "refused", message: "secret-must-not-appear" } });
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

  for (const destination of ["chat", "new Settings section", "another account"]) {
    for (const outcome of [200, 500]) {
      test(`${label}: late ${outcome} after ${destination} cannot refresh or show an old error`, { timeout: 1000 }, async () => {
        const h = setup(signIn);
        const pending = h.button.onclick();
        if (destination === "chat") { h.state.page = null; h.page.hidden = true; }
        if (destination === "new Settings section") h.page.replaceChildren();
        if (destination === "another account") h.state.me = { id: "owner-b" };
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
