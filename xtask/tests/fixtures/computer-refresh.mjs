// Production refresh, identity transitions, API handling and notice rendering;
// synthetic HTTP/DOM and downstream renderers, without a browser or server.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { createContext, runInContext } from "node:vm";

const shell = readFileSync(new URL("../../../cell/shell/shell.js", import.meta.url), "utf8");
const functions = ["api", "load", "identityIs", "renderComputer", "owesFirstAgent", "start"].map((name) => {
  const source = shell.match(new RegExp(`^(?:async )?function ${name}\\([^]*?^\\}`, "m"))?.[0];
  assert.ok(source, `the shell declares ${name}; update the test if it moves`);
  return source;
}).join("\n");
const reads = shell.match(/^const reads = .*;$/m)?.[0];
const apiError = shell.match(/^class ApiError[^]*?^\}/m)?.[0];
const submit = shell.match(/^document.addEventListener\("submit",[^]*?^\}, true\);/m)?.[0];
assert.ok(reads && apiError && submit);
const html = readFileSync(new URL("../../../cell/shell/index.html", import.meta.url), "utf8");
assert.match(html, /<div class="computer-notices" id="computer-notices" role="status" aria-live="polite" hidden><\/div>\s*<div class="frames" id="frames">/,
  "the notice band is outside frames, which Settings hides");
const turn = () => new Promise(setImmediate);
const computer = (id = "computer-a", agent = "agent-a") => ({ computer: id, agents: [{ identity: agent, fragment: agent }], notices: [{ kind: "update" }] });
const listing = (value = computer()) => ({ computers: value ? [value] : [], support: "https://support.example" });

function setup() {
  const requests = [], rendered = [], listeners = new Map();
  const state = { me: null, guest: false, fragments: [], computer: null, agents: new Map(), frames: new Map() };
  const el = (tag, classes, text) => ({
    tag, classes, textContent: text ?? "", children: [], dataset: {}, hidden: false, disabled: false,
    append(...nodes) { this.children.push(...nodes); },
    replaceChildren(...nodes) { this.children = nodes; },
  });
  const ids = Object.fromEntries(["computer-notices", "layout", "first-run"].map((id) => [id, el("div")]));
  class Form {
    constructor(action) { this.action = action; }
    getAttribute(name) { assert.equal(name, "action"); return this.action; }
  }
  const context = createContext({
    state, el, $: (id) => ids[id],
    fetch: (path, options) => new Promise((resolve, reject) => requests.push({ path, options, resolve, reject })),
    heard() {},
    noticeRow: (n) => { const row = el("div"); row.dataset.kind = n.kind; return row; },
    renderChats: () => rendered.push("chats"), renderApps() {}, renderHeading() {},
    rosterChanged: () => rendered.push("roster"), markSeen() {}, renderAgentFaces() {},
    chats: () => state.fragments.filter((f) => f.kind === "chat"),
    own: (f) => f.role === "owner", UNTITLED: "Untitled", cardsLoad: {}, loadCards: async () => {},
    primaryAgent: () => state.computer?.agents[0],
    document: { addEventListener: (type, fn) => listeners.set(type, fn) }, HTMLFormElement: Form,
    tab: { drop() {} }, WINDOWS_KEY: "windows", greeted: new Set(),
    location: { pathname: "/settings" }, SETTINGS: "/settings", WELCOME: "/welcome",
    setGuest: (guest) => { state.guest = guest; }, watchList() {}, byName: () => null, shown: () => [],
    openSettings: async () => {}, reopenWindows() {}, prewake() {}, signIn: () => rendered.push("sign-in"),
    notice: () => rendered.push("notice"), creatingAgent: () => { throw new Error("unexpected agent creation"); },
  });
  runInContext(`${apiError}\nlet writing = 0;\n${reads}\n${functions}\n${submit}`, context);
  const call = (expression) => runInContext(expression, context);
  call('identityIs({id: "owner-a"})');
  const box = ids["computer-notices"];
  const failure = () => box.children.find((row) => row.dataset.kind === "unavailable");
  return { state, requests, rendered, box, failure, layout: ids.layout,
    path: (value) => { context.location.pathname = value; },
    load: () => call("load()"), start: () => call("start()"),
    owes: () => call("owesFirstAgent()"),
    identity: (id) => { context.nextIdentity = id === null ? null : { id }; call("identityIs(nextIdentity)"); },
    signout: (action) => listeners.get("submit")({ target: new Form(action) }),
    retry: () => failure().children[1].children[0],
  };
}

function respond(request, body, status = 200) {
  request.resolve({ ok: status < 400, status, statusText: "secret-error-text", json: async () => body });
}
function complete(h, offset, computers, status = 200, fragments = []) {
  assert.equal(h.requests[offset].path, "/api/fragments");
  assert.equal(h.requests[offset + 1].path, "/api/computers");
  respond(h.requests[offset], { fragments });
  if (status === "transport") h.requests[offset + 1].reject(new Error("secret-error-text"));
  else respond(h.requests[offset + 1], computers, status);
}
async function seed(h) {
  const pending = h.load();
  complete(h, 0, listing());
  await pending;
}

for (const status of [403, 500, "transport"]) {
  test(`${status}: retain the same person's computer, agents and support, then recover through retry`, async () => {
    const h = setup();
    await seed(h);
    const previous = { computer: h.state.computer, agents: h.state.agents, support: h.state.support };
    const pending = h.load();
    complete(h, 2, { message: "secret-error-text" }, status, [{ name: "new-app", kind: "app" }]);
    await pending;
    assert.equal(h.state.computer, previous.computer);
    assert.equal(h.state.agents, previous.agents);
    assert.equal(h.state.support, previous.support);
    assert.equal(h.state.fragments[0].name, "new-app", "the successful fragment list still updates");
    assert.equal(h.box.hidden, false);
    assert.deepEqual(h.box.children.map((r) => r.dataset.kind), ["update", "unavailable"]);
    assert.match(h.failure().children[0].children[1].textContent, /last confirmed state/);
    assert.doesNotMatch(JSON.stringify(h.box), /secret-error-text/);
    const retry = h.retry();
    const recovery = retry.onclick();
    assert.equal(retry.disabled, true);
    await retry.onclick();
    await turn();
    assert.equal(h.requests.length, 6, "one explicit retry, no automatic or duplicate request");
    complete(h, 4, listing(computer("computer-new", "agent-new")));
    await recovery;
    assert.equal(h.state.computer.computer, "computer-new");
    assert.deepEqual([...h.state.agents.keys()], ["agent-new"]);
    assert.equal(h.failure(), undefined);
    assert.equal(retry.disabled, false);
    assert.ok(h.requests.every((r) => r.options.method === "GET"));
  });
}

test("first-load failure stays unknown, offers retry and does not infer a first agent is owed", async () => {
  const h = setup();
  const pending = h.load();
  complete(h, 0, {}, 500);
  await pending;
  assert.equal(h.state.computer, null);
  assert.equal(h.state.agents.size, 0);
  assert.equal(h.box.hidden, false);
  assert.equal(h.failure().children[0].children[1].textContent, "Try again to check your computer and agents.");
  assert.equal(await h.owes(), false);
  const retry = h.retry().onclick();
  complete(h, 2, listing(null));
  await retry;
  assert.equal(h.box.hidden, true);
  assert.equal(await h.owes(), true, "a confirmed empty response still allows the existing first-agent flow");
});

test("a genuine empty response clears a previously loaded computer and agents", async () => {
  const h = setup();
  await seed(h);
  const pending = h.load();
  complete(h, 2, { computers: [] });
  await pending;
  assert.equal(h.state.computer, null);
  assert.equal(h.state.agents.size, 0);
  assert.equal(h.state.support, null);
  assert.equal(h.box.hidden, true);
});

test("home startup with no confirmed computer shows the retry band without creating an agent", async () => {
  const h = setup();
  h.path("/");
  h.layout.hidden = true;
  const pending = h.start();
  respond(h.requests[0], { id: "owner-a" });
  await turn();
  complete(h, 1, {}, 500);
  respond(h.requests[3], { guest: false });
  await pending;
  assert.equal(h.layout.hidden, false);
  assert.equal(h.box.hidden, false);
  assert.ok(h.failure());
  assert.equal(h.state.computer, null);
  assert.equal(h.requests.length, 4);
  assert.ok(h.requests.every((r) => r.options.method === "GET"));
});

for (const olderFails of [true, false]) {
  test(`older ${olderFails ? "failure" : "success"} cannot replace the newer result or its feedback`, async () => {
    const h = setup();
    await seed(h);
    const old = h.load(), recent = h.load();
    complete(h, 4, listing(computer("newest", "newest-agent")), olderFails ? 200 : 500);
    await recent;
    const before = { computer: h.state.computer, agents: h.state.agents, rows: h.box.children };
    complete(h, 2, listing(computer("old", "old-agent")), olderFails ? 500 : 200);
    await old;
    assert.equal(h.state.computer, before.computer);
    assert.equal(h.state.agents, before.agents);
    assert.equal(h.box.children, before.rows);
    assert.equal(!!h.failure(), !olderFails);
  });
}

test("an older result may still display while a newer request is pending, as the existing guard permits", async () => {
  const h = setup();
  const old = h.load(), recent = h.load();
  complete(h, 0, listing());
  await old;
  assert.equal(h.state.computer.computer, "computer-a");
  complete(h, 2, listing(computer("newest", "newest-agent")));
  await recent;
  assert.equal(h.state.computer.computer, "newest");
});

test("a same-identity metadata refresh retains the confirmed view", async () => {
  const h = setup();
  await seed(h);
  const previous = h.state.computer;
  h.identity("owner-a");
  const pending = h.load();
  complete(h, 2, {}, 500);
  await pending;
  assert.equal(h.state.computer, previous);
});

for (const action of ["/auth/logout", "/auth/switch"]) {
  test(`${action} clears the retained view and invalidates an outstanding response`, async () => {
    const h = setup();
    await seed(h);
    const pending = h.load();
    h.signout(action);
    assert.equal(h.state.me, null);
    assert.equal(h.state.computer, null);
    assert.equal(h.state.agents.size, 0);
    assert.equal(h.state.support, null);
    complete(h, 2, listing());
    await pending;
    assert.equal(h.state.computer, null);
    assert.equal(h.box.hidden, true);
    await h.load();
    assert.equal(h.requests.length, 4, "signed-out list events do not read another session");
  });
}

test("startup identity change clears old state before a new person's failed refresh", async () => {
  const h = setup();
  await seed(h);
  const old = h.load();
  const start = h.start();
  assert.equal(h.requests[4].path, "/api/identities/me");
  respond(h.requests[4], { id: "owner-b" });
  await turn();
  assert.equal(h.state.computer, null);
  assert.equal(h.state.agents.size, 0);
  complete(h, 5, {}, 500);
  assert.equal(h.requests[7].path, "/api/seat");
  respond(h.requests[7], { guest: false });
  await start;
  complete(h, 2, listing());
  await old;
  assert.equal(h.state.me.id, "owner-b");
  assert.equal(h.state.computer, null);
  assert.equal(h.state.agents.size, 0);
  assert.ok(h.failure());
});

test("identity changes away and back still invalidate the original request", async () => {
  const h = setup();
  await seed(h);
  const old = h.load();
  h.identity("owner-b");
  h.identity("owner-a");
  complete(h, 2, listing());
  await old;
  assert.equal(h.state.computer, null);
  assert.equal(h.state.agents.size, 0);
});

test("startup 401 clears the previous identity and computer before sign-in", async () => {
  const h = setup();
  await seed(h);
  const pending = h.start();
  respond(h.requests[2], {}, 401);
  await pending;
  assert.equal(h.state.me, null);
  assert.equal(h.state.computer, null);
  assert.equal(h.state.agents.size, 0);
  assert.ok(h.rendered.includes("sign-in"));
});

test("a retry whose fragment request fails stays available without discarding the computer", async () => {
  const h = setup();
  await seed(h);
  const pending = h.load();
  complete(h, 2, {}, 500);
  await pending;
  const retry = h.retry();
  const again = retry.onclick();
  respond(h.requests[4], {}, 500);
  respond(h.requests[5], listing());
  await again;
  assert.equal(retry.disabled, false);
  assert.equal(h.state.computer.computer, "computer-a");
  assert.ok(h.failure());
});
