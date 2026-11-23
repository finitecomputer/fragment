// cell/shell/secrets.js's Settings → Secrets section: lists Vault secrets
// without values, toggles per-agent access (refusing to remove the last
// agent), edits allowed hosts keeping the sealed value, replaces a value
// through a masked input that clears immediately, adds a secret, and
// removes one via two-step confirmation.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { createSecrets, parseHosts, placeLabel } from "../shell/secrets.js";

class Node {
  constructor(tag, cls, text) {
    Object.assign(this, {
      tag,
      className: cls ?? "",
      textContent: text ?? "",
      value: "",
      children: [],
      dataset: {},
      attrs: {},
      hidden: false,
      disabled: false,
      open: false,
      isConnected: true,
    });
    this.classList = {
      add: (c) => (this.className += ` ${c}`),
      remove: (c) => (this.className = this.className.replace(c, "").trim()),
      toggle: () => {},
    };
    this.style = {};
  }
  append(...c) {
    this.children.push(...c);
  }
  replaceChildren(...c) {
    this.children = [...c];
  }
  setAttribute(k, v) {
    this.attrs[k] = v;
  }
  getAttribute(k) {
    return this.attrs[k] ?? null;
  }
  setAttributeNS(_, k, v) {
    this.attrs[k] = v;
  }
  addEventListener() {}
  showModal() {
    this.open = true;
  }
  close() {
    this.open = false;
    this.onclose?.();
  }
  focus() {}
  querySelectorAll() {
    return [];
  }
}

const walk = (n) => (n && typeof n === "object" ? [n, ...(n.children ?? []).flatMap(walk)] : []);

function setup(agents = [{ fragment: "ada--k3x9" }, { fragment: "bea--k3x9" }]) {
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
  const calls = [];
  let reopened = 0;
  const api = async (method, path, body) => {
    calls.push({ method, path, body });
    return {};
  };
  const ui = createSecrets({
    api,
    el: (tag, cls, text) => new Node(tag, cls, typeof text === "string" ? text : ""),
    section: (title) => new Node("section", null, title),
    named: (node, fragment, make = (t) => t, attr = "text") => {
      const val = make(fragment);
      if (attr === "text") node.textContent = val;
      else node.setAttribute(attr, val);
      return node;
    },
    computer: () => ({ agents }),
    asks: { now: () => 1, current: () => true },
    isOpen: () => true,
    reopenSettings: async () => {
      reopened++;
    },
  });
  return { ...ui, calls, reopened: () => reopened };
}

test("placeLabel and parseHosts format placements and normalize host lists", () => {
  assert.equal(placeLabel({}), "Authorization: Bearer");
  assert.equal(placeLabel({ header: "authorization", format: "Bearer {}" }), "Authorization: Bearer");
  assert.equal(placeLabel({ header: "x-api-key" }), "Header: x-api-key");
  assert.equal(placeLabel({ query: "key" }), "Query: key");
  assert.equal(placeLabel({ basic: "password" }), "Basic auth (password)");
  assert.equal(placeLabel({ sign: "nip98" }), "NIP-98 signing");
  assert.deepEqual(parseHosts(" API.GitHub.com, uploads.github.com api.github.com "), ["api.github.com", "uploads.github.com"]);
  assert.deepEqual(parseHosts("   "), []);
});

test("toggling per-agent access sends PUT without value and refuses removing the last agent", async () => {
  const { secretsSection, calls, reopened } = setup();
  const s = secretsSection({
    secrets: [{ name: "github", hosts: ["api.github.com"], place: {}, agents: ["ada--k3x9"], setAt: 1700000000000 }],
  });
  const card = walk(s).find((n) => n.dataset?.secret === "github");
  assert.ok(card, "card rendered");
  const switches = walk(card).filter((n) => n.className === "provider-switch");
  assert.equal(switches.length, 2);
  assert.equal(switches[0].getAttribute("aria-pressed"), "true");
  assert.equal(switches[1].getAttribute("aria-pressed"), "false");

  // Removing the last agent is refused locally with no PUT
  await switches[0].onclick();
  assert.equal(calls.length, 0, "did not send PUT with empty agents");
  const err = walk(card).find((n) => n.className.includes("secret-said") && !n.hidden);
  assert.ok(err?.textContent.includes("Keep at least one agent"), err?.textContent);

  // Adding second agent sends PUT without value
  await switches[1].onclick();
  assert.equal(calls.length, 1);
  assert.equal(calls[0].method, "PUT");
  assert.equal(calls[0].path, "/api/vault/github");
  assert.equal("value" in calls[0].body, false, "keeps existing sealed value");
  assert.deepEqual(calls[0].body.agents, ["ada--k3x9", "bea--k3x9"]);
  assert.equal(reopened(), 1);
});

test("updating hosts keeps value, replacing value clears masked input, and remove requires confirmation", async () => {
  const { secretsSection, calls } = setup();
  const s = secretsSection({
    secrets: [{ name: "github", hosts: ["api.github.com"], place: {}, agents: ["ada--k3x9"], setAt: 1700000000000 }],
  });
  const card = walk(s).find((n) => n.dataset?.secret === "github");

  // Update hosts
  const hostsForm = walk(card).find((n) => n.className.includes("secret-hosts-form"));
  const hostsInput = walk(hostsForm).find((n) => n.tag === "input");
  hostsInput.value = "api.github.com, uploads.github.com";
  await hostsForm.onsubmit({ preventDefault() {} });
  assert.equal(calls[0].method, "PUT");
  assert.equal("value" in calls[0].body, false);
  assert.deepEqual(calls[0].body.hosts, ["api.github.com", "uploads.github.com"]);

  // Replace value (masked; cleared immediately)
  const replaceForm = walk(card).find((n) => n.className.includes("secret-replace-form"));
  const valueInput = walk(replaceForm).find((n) => n.tag === "input");
  assert.equal(valueInput.type, "password");
  valueInput.value = "ghp_rotated_999";
  await replaceForm.onsubmit({ preventDefault() {} });
  assert.equal(calls[1].body.value, "ghp_rotated_999");
  assert.equal(valueInput.value, "", "masked input cleared immediately");

  // Two-step remove
  const rm = walk(card).find((n) => n.dataset?.action === "remove-secret");
  rm.onclick();
  assert.equal(calls.length, 2, "first click asks for confirmation");
  const confirm = walk(card).find((n) => n.dataset?.action === "confirm-remove-secret");
  assert.ok(confirm, "confirm button appeared");
  await confirm.onclick();
  assert.equal(calls[2].method, "DELETE");
  assert.equal(calls[2].path, "/api/vault/github");
});

test("Add secret submits name, masked value, hosts, and selected agents, then clears password input", async () => {
  const { secretsSection, calls, reopened } = setup();
  const s = secretsSection({ secrets: [] });
  const dialog = walk(s).find((n) => n.dataset?.addSecret === "1");
  const form = walk(dialog).find((n) => n.tag === "form");
  const nameInput = walk(form).find((n) => n.dataset?.field === "name");
  const valueInput = walk(form).find((n) => n.dataset?.field === "value");
  const hostsInput = walk(form).find((n) => n.dataset?.field === "hosts");
  assert.equal(valueInput.type, "password");
  nameInput.value = "stripe";
  valueInput.value = "sk_test_123";
  hostsInput.value = "api.stripe.com";
  await form.onsubmit({ preventDefault() {} });
  assert.equal(calls.length, 1);
  assert.equal(calls[0].method, "PUT");
  assert.equal(calls[0].path, "/api/vault/stripe");
  assert.deepEqual(calls[0].body, { value: "sk_test_123", hosts: ["api.stripe.com"], agents: ["ada--k3x9"] });
  assert.equal(valueInput.value, "", "value cleared after save");
  assert.equal(reopened(), 1);
});
