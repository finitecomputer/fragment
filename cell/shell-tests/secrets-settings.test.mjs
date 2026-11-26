// cell/shell/secrets.js's Settings → Secrets section: lists Vault secrets
// without values, toggles per-agent access (refusing to remove the last
// agent), edits allowed hosts keeping the sealed value, replaces a value
// through a masked input cleared once saved, adds a secret (never over one
// already kept), and removes one via two-step confirmation. Every write is
// onto the secret as its sheet shows it (`held`), one at a time.
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

// `vault`: what `GET /api/vault` answers (the secret as it is now); `vaultApi`,
// in its place, a Vault of the test's own
function setup(vault = { secrets: [] }, agents = [{ fragment: "ada--k3x9" }, { fragment: "bea--k3x9" }], vaultApi = null, onReopen = null, asks = { now: () => 1, current: () => true }) {
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
  const api = vaultApi ?? (async (method, path, body) => {
    if (method === "GET") return vault;
    calls.push({ method, path, body });
    return {};
  });
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
    asks,
    isOpen: () => true,
    reopenSettings: async () => {
      reopened++;
      await onReopen?.(ui);
    },
  });
  return { ...ui, calls, reopened: () => reopened };
}

class ApiError extends Error {
  constructor(status, message) {
    super(message);
    this.status = status;
  }
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

const github = () => ({ name: "github", hosts: ["api.github.com"], place: {}, agents: ["ada--k3x9"], setAt: 1700000000000, revision: "rev1" });

test("toggling per-agent access sends PUT without value and refuses removing the last agent", async () => {
  const { secretsSection, calls, reopened } = setup({ secrets: [github()] });
  const s = secretsSection({ secrets: [github()] });
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

test("an edit is onto the secret as its sheet shows it: a change since is refused, never taken in or written over", async () => {
  // the Vault: an agent's Allow since the sheet was drawn added a host and an agent
  const now = { ...github(), hosts: ["api.github.com", "uploads.github.com"], agents: ["ada--k3x9", "cy--k3x9"], revision: "rev2" };
  const sent = [];
  const vaultApi = async (method, path, body) => {
    if (method === "GET") return { secrets: [now] };
    sent.push({ method, path, body });
    throw new ApiError(409, "github changed since you read it: look at it again");
  };
  const drawn = [];
  const { secretsSection, reopened } = setup(null, undefined, vaultApi, (ui) => drawn.push(ui.secretsSection({ secrets: [now] })));
  const card = walk(secretsSection({ secrets: [github()] })).find((n) => n.dataset?.secret === "github");
  await walk(card).find((n) => n.dataset?.agent === "bea--k3x9").onclick();
  // hosts and agents as the sheet showed them, onto the revision it showed
  assert.deepEqual(sent[0].body, { hosts: ["api.github.com"], place: {}, agents: ["ada--k3x9", "bea--k3x9"], held: "rev1" });
  // Replace and Save hosts the same: nothing taken from what changed since
  const replaceForm = walk(card).find((n) => n.className.includes("secret-replace-form"));
  const valueInput = walk(replaceForm).find((n) => n.tag === "input");
  valueInput.value = "ghp_new";
  await replaceForm.onsubmit({ preventDefault() {} });
  assert.deepEqual(sent[1].body, { value: "ghp_new", hosts: ["api.github.com"], place: {}, agents: ["ada--k3x9"], held: "rev1" });
  assert.equal(valueInput.value, "", "a refused paste is not kept");
  const hostsForm = walk(card).find((n) => n.className.includes("secret-hosts-form"));
  walk(hostsForm).find((n) => n.tag === "input").value = "api.github.com";
  await hostsForm.onsubmit({ preventDefault() {} });
  assert.deepEqual(sent[2].body, { hosts: ["api.github.com"], place: {}, agents: ["ada--k3x9"], held: "rev1" });
  // each refusal draws the page again from the Vault, whose sheet says why
  assert.equal(reopened(), 3);
  const newest = walk(drawn[2]).find((n) => n.dataset?.secret === "github");
  assert.deepEqual(walk(newest).find((n) => n.className === "provider-sub")?.textContent, "api.github.com, uploads.github.com", "the sheet shows what the Vault holds now");
  assert.match(walk(newest).find((n) => n.className.includes("secret-said") && !n.hidden)?.textContent ?? "", /changed since you read it/);
});

test("remove names the revision its sheet showed, and a change since is refused", async () => {
  const sent = [];
  const vaultApi = async (method, path) => {
    if (method === "GET") return { secrets: [github()] };
    sent.push({ method, path });
    throw new ApiError(409, "github changed since you read it: look at it again");
  };
  const drawn = [];
  const { secretsSection, reopened } = setup(null, undefined, vaultApi, (ui) => drawn.push(ui.secretsSection({ secrets: [github()] })));
  const card = walk(secretsSection({ secrets: [github()] })).find((n) => n.dataset?.secret === "github");
  walk(card).find((n) => n.dataset?.action === "remove-secret").onclick();
  await walk(card).find((n) => n.dataset?.action === "confirm-remove-secret").onclick();
  assert.deepEqual(sent, [{ method: "DELETE", path: "/api/vault/github?held=rev1" }]);
  assert.equal(reopened(), 1);
  const newest = walk(drawn[0]).find((n) => n.dataset?.secret === "github");
  assert.match(walk(newest).find((n) => n.className.includes("secret-said") && !n.hidden)?.textContent ?? "", /changed since/);
});

test("a write queued behind one that redrew the page says what went wrong in the sheet drawn since", async () => {
  let held = github();
  let n = 1;
  const vaultApi = async (method, _path, body) => {
    if (method === "GET") return { secrets: [structuredClone(held)] };
    await new Promise((r) => setTimeout(r, 5));
    if (body.hosts.length > 8) throw new ApiError(400, "your vault sends secrets to at most 8 hosts, and this would make 9");
    held = { ...held, agents: body.agents, revision: `rev${++n}` };
    return structuredClone(held);
  };
  const drawn = [];
  const { secretsSection } = setup(null, undefined, vaultApi, (ui) => drawn.push(ui.secretsSection({ secrets: [structuredClone(held)] })));
  const card = walk(secretsSection({ secrets: [github()] })).find((x) => x.dataset?.secret === "github");
  const hostsForm = walk(card).find((x) => x.className.includes("secret-hosts-form"));
  walk(hostsForm).find((x) => x.tag === "input").value = Array.from({ length: 9 }, (_, i) => `h${i}.example.com`).join(", ");
  await Promise.all([walk(card).find((x) => x.dataset?.agent === "bea--k3x9").onclick(), hostsForm.onsubmit({ preventDefault() {} })]);
  assert.equal(drawn.length, 1, "the toggle landed and drew the page again");
  const newest = walk(drawn[0]).find((x) => x.dataset?.secret === "github");
  assert.match(walk(newest).find((x) => x.className.includes("secret-said") && !x.hidden)?.textContent ?? "", /at most 8 hosts/);
});

test("a failure while the page is drawn again waits for the sheet drawn next", async () => {
  let held = github();
  let n = 1;
  const vaultApi = async (method, _path, body) => {
    if (method === "GET") return { secrets: [structuredClone(held)] };
    await new Promise((r) => setTimeout(r, 5));
    if (body.hosts.length > 8) throw new ApiError(400, "your vault sends secrets to at most 8 hosts, and this would make 9");
    held = { ...held, agents: body.agents, revision: `rev${++n}` };
    return structuredClone(held);
  };
  const drawn = [];
  let first;
  // the toggle's redraw takes the old page away at once, and draws the new one only later ("Loading…")
  const { secretsSection } = setup(null, undefined, vaultApi, async (ui) => {
    for (const x of walk(first)) x.isConnected = false;
    await new Promise((r) => setTimeout(r, 30));
    drawn.push(ui.secretsSection({ secrets: [structuredClone(held)] }));
  });
  first = secretsSection({ secrets: [github()] });
  const card = walk(first).find((x) => x.dataset?.secret === "github");
  const hostsForm = walk(card).find((x) => x.className.includes("secret-hosts-form"));
  walk(hostsForm).find((x) => x.tag === "input").value = Array.from({ length: 9 }, (_, i) => `h${i}.example.com`).join(", ");
  await Promise.all([walk(card).find((x) => x.dataset?.agent === "bea--k3x9").onclick(), hostsForm.onsubmit({ preventDefault() {} })]);
  const newest = walk(drawn[0]).find((x) => x.dataset?.secret === "github");
  assert.match(walk(newest).find((x) => x.className.includes("secret-said") && !x.hidden)?.textContent ?? "", /at most 8 hosts/);
});

test("a write refused for a secret removed elsewhere says so in the section, with no sheet to say it", async () => {
  const vaultApi = async (method) => {
    if (method === "GET") return { secrets: [] };
    throw new ApiError(409, "github changed since you read it: look at it again");
  };
  const drawn = [];
  const { secretsSection } = setup(null, undefined, vaultApi, (ui) => drawn.push(ui.secretsSection({ secrets: [] })));
  const card = walk(secretsSection({ secrets: [github()] })).find((x) => x.dataset?.secret === "github");
  for (const x of walk(card)) x.isConnected = false;
  const replaceForm = walk(card).find((x) => x.className.includes("secret-replace-form"));
  walk(replaceForm).find((x) => x.tag === "input").value = "ghp_new";
  await replaceForm.onsubmit({ preventDefault() {} });
  assert.equal(drawn.length, 1);
  assert.match(walk(drawn[0]).find((x) => x.dataset?.secretSaid === "github")?.textContent ?? "", /changed since you read it/);
});

test("a refusal that comes after Settings was left says nothing, and opens nothing, on a later visit", async () => {
  let visit = 1;
  const asks = { now: () => visit, current: (v) => v === visit };
  let answer;
  const vaultApi = async (method) => {
    if (method === "GET") return { secrets: [github()] };
    await new Promise((r) => (answer = r));
    throw new ApiError(409, "github changed since you read it: look at it again");
  };
  const { secretsSection, reopened } = setup(null, undefined, vaultApi, null, asks);
  const card = walk(secretsSection({ secrets: [github()] })).find((x) => x.dataset?.secret === "github");
  const toggled = walk(card).find((x) => x.dataset?.agent === "bea--k3x9").onclick();
  while (!answer) await new Promise((r) => setTimeout(r, 1));
  // Settings left and opened again before the Vault answered
  visit = 2;
  const later = secretsSection({ secrets: [github()] });
  answer();
  await toggled;
  assert.equal(reopened(), 0, "nothing drawn again for a visit that ended");
  const again = secretsSection({ secrets: [github()] });
  for (const page of [later, again]) {
    assert.equal(walk(page).some((x) => x.className.includes("secret-said") && !x.hidden), false);
    assert.equal(walk(page).some((x) => x.open), false);
  }
});

test("Add's failure as the page is drawn again is said in the Add form drawn next", async () => {
  let fail;
  const vaultApi = async (method) => {
    if (method === "GET") return { secrets: [] };
    await new Promise((r) => (fail = r));
    throw new ApiError(400, "your vault sends secrets to at most 8 hosts, and this would make 9");
  };
  const { secretsSection } = setup(null, undefined, vaultApi);
  const first = secretsSection({ secrets: [] });
  const form = walk(walk(first).find((x) => x.dataset?.addSecret === "1")).find((x) => x.tag === "form");
  walk(form).find((x) => x.dataset?.field === "name").value = "stripe";
  walk(form).find((x) => x.dataset?.field === "value").value = "sk_test_123";
  const added = form.onsubmit({ preventDefault() {} });
  // another write's redraw takes the page away before Add's answer comes
  for (const x of walk(first)) x.isConnected = false;
  fail();
  await added;
  const next = walk(secretsSection({ secrets: [] })).find((x) => x.dataset?.addSecret === "1");
  assert.match(walk(next).find((x) => x.className.includes("secret-said") && !x.hidden)?.textContent ?? "", /at most 8 hosts/);
});

test("two edits at once go one after the other, each onto what the last left", async () => {
  // the Vault: each PUT makes a new revision, and refuses one read before it
  let held = github();
  let n = 1;
  const refused = [];
  const vaultApi = async (method, _path, body) => {
    if (method === "GET") return { secrets: [structuredClone(held)] };
    await new Promise((r) => setTimeout(r, 5));
    if (body.held !== held.revision) {
      refused.push(body);
      throw new Error("changed");
    }
    held = { ...held, agents: body.agents, revision: `rev${++n}` };
    return structuredClone(held);
  };
  const { secretsSection } = setup(null, [{ fragment: "ada--k3x9" }, { fragment: "bea--k3x9" }, { fragment: "cy--k3x9" }], vaultApi);
  const card = walk(secretsSection({ secrets: [github()] })).find((x) => x.dataset?.secret === "github");
  const bea = walk(card).find((x) => x.dataset?.agent === "bea--k3x9");
  const cy = walk(card).find((x) => x.dataset?.agent === "cy--k3x9");
  await Promise.all([bea.onclick(), cy.onclick()]);
  assert.deepEqual(refused, []);
  assert.deepEqual(held.agents, ["ada--k3x9", "bea--k3x9", "cy--k3x9"], "neither grant lost");
});

test("updating hosts keeps value, replacing value clears masked input, and remove requires confirmation", async () => {
  const { secretsSection, calls } = setup({ secrets: [github()] });
  const s = secretsSection({ secrets: [github()] });
  const card = walk(s).find((n) => n.dataset?.secret === "github");

  // Update hosts
  const hostsForm = walk(card).find((n) => n.className.includes("secret-hosts-form"));
  const hostsInput = walk(hostsForm).find((n) => n.tag === "input");
  hostsInput.value = "api.github.com, uploads.github.com";
  await hostsForm.onsubmit({ preventDefault() {} });
  assert.equal(calls[0].method, "PUT");
  assert.equal("value" in calls[0].body, false);
  assert.deepEqual(calls[0].body.hosts, ["api.github.com", "uploads.github.com"]);

  // Replace value (masked; cleared once saved)
  const replaceForm = walk(card).find((n) => n.className.includes("secret-replace-form"));
  const valueInput = walk(replaceForm).find((n) => n.tag === "input");
  assert.equal(valueInput.type, "password");
  valueInput.value = "ghp_rotated_999";
  await replaceForm.onsubmit({ preventDefault() {} });
  assert.equal(calls[1].body.value, "ghp_rotated_999");
  assert.equal(valueInput.value, "", "masked input cleared once saved");

  // Two-step remove
  const rm = walk(card).find((n) => n.dataset?.action === "remove-secret");
  rm.onclick();
  assert.equal(calls.length, 2, "first click asks for confirmation");
  const confirm = walk(card).find((n) => n.dataset?.action === "confirm-remove-secret");
  assert.ok(confirm, "confirm button appeared");
  await confirm.onclick();
  assert.equal(calls[2].method, "DELETE");
  assert.equal(calls[2].path, "/api/vault/github?held=rev1");
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
  assert.deepEqual(calls[0].body, { value: "sk_test_123", hosts: ["api.stripe.com"], agents: ["ada--k3x9"], held: "none" }, "only as a new secret");
  assert.equal(valueInput.value, "", "value cleared after save");
  assert.equal(reopened(), 1);
});

test("Add secret never takes a name already kept: a PUT would set it whole", async () => {
  const { secretsSection, calls } = setup({ secrets: [github()] });
  const dialog = walk(secretsSection({ secrets: [github()] })).find((n) => n.dataset?.addSecret === "1");
  const form = walk(dialog).find((n) => n.tag === "form");
  walk(form).find((n) => n.dataset?.field === "name").value = "GitHub";
  walk(form).find((n) => n.dataset?.field === "value").value = "ghp_other";
  await form.onsubmit({ preventDefault() {} });
  assert.equal(calls.length, 0);
  const said = walk(form).find((n) => n.className.includes("secret-said") && !n.hidden);
  assert.match(said.textContent, /already keep github/);
});
