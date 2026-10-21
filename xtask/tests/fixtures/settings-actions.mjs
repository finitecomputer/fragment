// Production Settings callbacks, loader, navigation and billing module.
// Fake DOM/HTTP only: no uploads, keys, grants, skills or seats are created.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test as nodeTest } from "node:test";
import { createContext, runInContext } from "node:vm";
const test = (name, run) => nodeTest(name, { timeout: 1000 }, run);
const shell = readFileSync(new URL("../../../cell/shell/shell.js", import.meta.url), "utf8");
const billing = readFileSync(new URL("../../../cell/shell/billing.js", import.meta.url), "utf8").replace(/^export /gm, "");
const functions = ["openSettings", "renderSettingsPage", "section", "line", "actionLine", "api", "openChat", "firstRun", "signIn", "connectionsSection", "skillsSection", "skillsFragment", "skillNames", "skillsIn"].map((name) => {
  const source = shell.match(new RegExp(`^(?:async )?function ${name}\\([^]*?^\\}`, "m"))?.[0];
  assert.ok(source, `production ${name} exists`); return source;
}).join("\n");
const guard = shell.match(/^function settingsActionCurrent\([^]*?^\}/m)?.[0] ?? "";
// Execute the pending identity repair's helper when composing these changes;
// current master still assigns the profile response directly.
const identity = shell.match(/^function identityIs\([^]*?^\}/m)?.[0] ?? "";
const identityState = identity ? [shell.match(/^const reads = .*;$/m)?.[0], ...["woke", "making", "restarted"].map((name) => shell.match(new RegExp(`^let ${name} = .*;$`, "m"))?.[0])].join("\n") : "";
const constants = shell.slice(shell.indexOf("const PROVIDER_KIND ="), shell.indexOf("function connectionsSection("));
const apiError = shell.match(/^class ApiError[^]*?^\}/m)?.[0];
const skillsOf = shell.match(/^const skillsFragmentOf = .*;$/m)?.[0];
const walk = (node) => [node, ...node.children.filter((n) => typeof n === "object").flatMap(walk)];
const turn = () => new Promise(setImmediate);
function setup() {
  const requests = [], mutations = [], identities = [];
  let holdIdentity = false;
  function el(tag, classes, text) {
    return {
      tag, classes, textContent: text ?? "", children: [], dataset: {}, hidden: false, value: "", disabled: false,
      classList: { add() {}, remove() {} },
      append(...children) { for (const node of children) { this.children.push(node); if (typeof node === "object") node.parent = this; } },
      replaceChildren(...children) { this.children = []; this.append(...children); },
      get lastChild() { return this.children.at(-1); }, get childElementCount() { return this.children.length; },
      setAttribute(name, value) { this[name] = value; }, removeAttribute(name) { delete this[name]; },
      getBoundingClientRect() { return { top: 0 }; },
      querySelector(selector) {
        if (selector === ":scope > h2") return this.children.find((n) => n.tag === "h2");
        if (selector === ".settings-value") return walk(this).find((n) => n.classes === "settings-value");
        if (selector === "span") return walk(this).find((n) => n.tag === "span");
        throw new Error(`unexpected selector ${selector}`);
      },
    };
  }
  const nodes = Object.fromEntries(["layout", "frames", "settings-page", "notice", "first-run", "first-run-card"].map((id) => [id, el("div")]));
  const state = { me: { id: "owner-a", email: "a@example.test" }, page: null, guest: false, computer: null, agents: new Map(), frames: new Map([["chat", el("iframe")]]), fragments: [] };
  const seat = { guest: false, admin: true, pays: false, offer: {}, offered: [{ id: "offer-a", org: { id: "org-a", name: "Example" } }] };
  const linked = { providers: [{ provider: "test/provider", kind: "own", state: "not_set" }] };
  const context = createContext({
    state, el, URLSearchParams, location: { pathname: "/settings", search: "" }, confirm: () => true,
    $: (id) => nodes[id], heard() {},
    fetch: (path, options) => {
      requests.push({ path, options });
      if (options.method !== "GET") return new Promise((resolve) => mutations.push({ path, resolve }));
      if (path === "/api/identities/me" && holdIdentity) return new Promise((resolve) => identities.push({ path, resolve }));
      const body = path === "/api/connections" ? linked : path === "/api/seat" ? seat : path === "/api/org" ? { name: "Example", members: [], payer: "payer" } : path === "/api/identities/me" ? { ...state.me, picture: "/fake-picture" } : {};
      return Promise.resolve({ ok: true, json: async () => body });
    },
    renderHeading() {}, renderChats() {}, renderApps() {}, leaveSidebar() {}, renderAgentFaces() {}, renderComputer() {},
    usd: () => "$0", noSeatSaid: () => el("p"), switchForm: () => el("form"), setGuest: (guest) => { state.guest = guest; },
    dataSection: () => el("section"), accessSection: () => el("section"), buildSection: () => el("section"),
    own: () => true, freeLabel: () => "skills", load: async () => {},
    SETTINGS: "/settings", WELCOME: "/welcome", PITCH: "Sign in", SUBTITLE: "Welcome", at() {},
    store: { set() {} }, CURRENT: "current", byName: () => ({}), markSeen() {}, prewake() {}, navigated() {}, clearSplashEdge() {},
  });
  runInContext(`${apiError}\nlet writing = 0;\n${identityState}\n${identity}\n${constants}\n${skillsOf}\n${guard}\n${functions}\n${billing}`, context);
  const find = (predicate) => walk(nodes["settings-page"]).find(predicate);
  return {
    state, nodes, requests, mutations, identities,
    open: () => runInContext("openSettings(false)", context),
    chat: () => runInContext('openChat("chat", false)', context), signIn: () => runInContext("signIn()", context),
    holdIdentity: () => { holdIdentity = true; },
    loads: () => requests.filter((r) => r.path === "/api/ledger").length,
    action(name) {
      if (name === "profile") { const file = find((n) => n.tag === "input" && n.type === "file"); file.files = [{ type: "image/png" }]; return file.onchange(); }
      if (name === "key") { const form = find((n) => n.classes === "provider-key"); form.children[0].value = "fake-test-key"; return form.onsubmit({ preventDefault() {} }); }
      if (name === "skills") return find((n) => n.textContent === "Add the managed skills").onclick();
      if (name === "decline") return find((n) => n.textContent === "Decline").onclick();
      if (name === "add-admin") { const button = find((n) => n.textContent === "Add an admin"); button.parent.children[0].value = "fake@example.test"; return button.parent.onsubmit({ preventDefault() {} }); }
      throw new Error(`unknown action ${name}`);
    },
  };
}
const answer = (request, body = {}) => request.resolve({ ok: true, json: async () => body });
for (const action of ["profile", "key", "skills", "decline", "add-admin"]) {
  for (const change of ["stay", "refresh", "chat", "chat-return", "identity", "identity-return", "sign-in-return"]) {
    test(`${action}: completion after ${change} only refreshes the initiating visit`, async () => {
      const h = setup(); await h.open();
      const pending = h.action(action);
      assert.equal(h.mutations.length, 1);
      if (change === "refresh") await h.open();
      if (change === "chat" || change === "chat-return") h.chat();
      if (change === "chat-return") await h.open();
      if (change === "identity" || change === "identity-return") {
        h.state.me = { id: "owner-b" }; await h.open();
      }
      if (change === "identity-return") { h.state.me = { id: "owner-a" }; await h.open(); }
      if (change === "sign-in-return") { h.signIn(); h.nodes.layout.hidden = false; await h.open(); }
      const before = h.loads(), me = h.state.me;
      answer(h.mutations[0], { name: "fake-skills" }); await pending; await turn();
      const allowed = change === "stay" || change === "refresh";
      assert.equal(h.loads(), before + Number(allowed), "a stale callback cannot replace the current page");
      if (!allowed) assert.equal(h.state.me, me, "a stale profile callback cannot replace the current identity");
      if (change === "chat") { assert.equal(h.state.page, null); assert.equal(h.nodes["settings-page"].hidden, true); }
      assert.equal(h.mutations.length, 1, "the mutation is neither repeated nor undone");
    });
  }
}
for (const change of ["chat", "chat-return", "identity"]) {
  test(`profile identity GET finishing after ${change} does not reopen Settings`, async () => {
    const h = setup(); await h.open(); h.holdIdentity();
    const pending = h.action("profile"); answer(h.mutations[0]); await turn();
    assert.equal(h.identities.length, 1);
    if (change === "identity") { h.state.me = { id: "owner-b" }; await h.open(); }
    else { h.chat(); if (change === "chat-return") await h.open(); }
    const before = h.loads(), me = h.state.me;
    answer(h.identities[0], { id: "owner-a", picture: "/late-picture" }); await pending; await turn();
    assert.equal(h.loads(), before); assert.equal(h.state.me, me);
  });
}
