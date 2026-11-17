// cell/shell/connections.js's provider cards (FIN-458): a provider's name
// for people is the platform's (`ProviderView.name`), and a new shell may
// read an older release's `/api/connections` (a deploy going out, rolled
// back, a reload the release watcher put off), whose rows have none: it
// says the provider's id then, and Settings still draws.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { createConnections } from "../shell/connections.js";

class Node {
  constructor(tag, cls, text) {
    Object.assign(this, { tag, className: cls ?? "", textContent: text ?? "", children: [], dataset: {}, attrs: {}, hidden: false });
    this.classList = { add: () => {}, remove: () => {}, toggle: () => {} };
    this.style = {};
  }
  append(...c) {
    this.children.push(...c);
  }
  prepend(...c) {
    this.children.unshift(...c);
  }
  replaceChildren(...c) {
    this.children = [...c];
  }
  setAttribute(k, v) {
    this.attrs[k] = v;
  }
  setAttributeNS(_, k, v) {
    this.attrs[k] = v;
  }
  addEventListener() {}
}
const walk = (n) => (n && typeof n === "object" ? [n, ...(n.children ?? []).flatMap(walk)] : []);

function connections() {
  globalThis.MutationObserver ??= class {
    observe() {}
    disconnect() {}
  };
  globalThis.document = {
    createElementNS: (_, tag) => new Node(tag),
    createElement: (tag) => new Node(tag),
    getElementById: () => null,
    querySelector: () => null,
    addEventListener() {},
    removeEventListener() {},
  };
  return createConnections({
    api: async () => ({}),
    el: (tag, cls, text) => new Node(tag, cls, typeof text === "string" ? text : ""),
    section: (title) => new Node("section", null, title),
    named: () => {},
    seg: (s) => s,
    computer: () => ({ agents: [] }),
    getAgents: () => new Map(),
    computerIs: () => false,
    asks: { now: () => 0, current: () => true },
    isOpen: () => true,
    reopenSettings: async () => {},
  });
}

// The cards' names as drawn: each card's `.provider-name`.
const names = (s) => walk(s).filter((n) => n.className === "provider-name").map((n) => n.textContent);

test("an older release's rows, with no name: each card says its provider's id, and Settings draws", () => {
  const { connectionsSection } = connections();
  const old = { providers: [{ provider: "openrouter", kind: "key", state: "unset" }, { provider: "firecrawl", kind: "operator", state: "set", charged: 15000 }] };
  let drawn = null;
  assert.doesNotThrow(() => (drawn = connectionsSection(old, null)));
  assert.deepEqual(names(drawn), ["openrouter", "firecrawl"]);
});

test("this release's rows: each card says the platform's name", () => {
  const { connectionsSection } = connections();
  const rows = { providers: [{ provider: "openrouter", name: "OpenRouter", kind: "key", state: "unset" }] };
  assert.deepEqual(names(connectionsSection(rows, null)), ["OpenRouter"]);
});
