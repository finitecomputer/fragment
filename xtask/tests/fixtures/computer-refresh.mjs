// Production refresh, identity transitions, API handling and notice rendering;
// synthetic HTTP/DOM and downstream renderers, without a browser or server.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test as nodeTest } from "node:test";
import { createContext, runInContext } from "node:vm";
const test = (name, run) => nodeTest(name, { timeout: 1000 }, run);

const shell = readFileSync(new URL("../../../cell/shell/shell.js", import.meta.url), "utf8");
const functions = ["api", "load", "identityIs", "renderComputer", "owesFirstAgent", "start", "prewake", "computerCurrent", "computerIs", "computerOf", "restartComputer", "sawNotice", "updateLater", "agentReady"].map((name) => {
  const source = shell.match(new RegExp(`^(?:async )?function ${name}\\([^]*?^\\}`, "m"))?.[0];
  assert.ok(source, `the shell declares ${name}; update the test if it moves`);
  return source;
}).join("\n");
const reads = shell.match(/^const reads = .*;$/m)?.[0];
const apiError = shell.match(/^class ApiError[^]*?^\}/m)?.[0];
const submit = shell.match(/^document.addEventListener\("submit",[^]*?^\}, true\);/m)?.[0];
const computerVars = ["woke", "making", "restarted"].map((name) => shell.match(new RegExp(`^let ${name} = .*;$`, "m"))?.[0]).join("\n");
const profile = shell.match(/^  file.onchange = async[^]*?^  \};/m)?.[0];
const permission = shell.slice(shell.indexOf("function connectionsSection(")).match(/^        chip.onclick = async[^]*?^        \};/m)?.[0];
assert.ok(reads && apiError && submit && profile && permission);
const html = readFileSync(new URL("../../../cell/shell/index.html", import.meta.url), "utf8");
assert.match(html, /<div class="computer-notices" id="computer-notices" role="status" aria-live="polite" hidden><\/div>\s*<div class="frames" id="frames">/,
  "the notice band is outside frames, which Settings hides");
const turn = () => new Promise(setImmediate);
const computer = (id = "computer-a", agent = "agent-a", owner = "owner-a") => ({ computer: id, owner, phase: "asleep", agents: [{ identity: agent, fragment: agent }], notices: [{ kind: "update" }] });
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
  const button = { textContent: "Restart", disabled: false, parentElement: { querySelector: () => null, append() {} } };
  const file = { files: [{ type: "image/png" }] };
  const context = createContext({
    state, el, $: (id) => ids[id], button, file, picture: { querySelector: () => el("span") },
    chip: button, offered: [{ provider: "tools" }], p: { provider: "tools" }, allowed: true,
    fetch: (path, options) => new Promise((resolve, reject) => requests.push({ path, options, resolve, reject })),
    heard() {},
    noticeRow: (n) => { const row = el("div"); row.dataset.kind = n.kind; return row; },
    renderChats: () => rendered.push("chats"), renderApps() {}, renderHeading() {},
    rosterChanged: () => rendered.push("roster"), markSeen() {}, renderAgentFaces() {},
    chats: () => state.fragments.filter((f) => f.kind === "chat"),
    own: (f) => f.role === "owner", UNTITLED: "Untitled", cardsLoad: {}, loadCards: async () => {},
    primaryAgent: () => state.computer?.agents[0],
    document: { hidden: false, addEventListener: (type, fn) => listeners.set(type, fn), querySelectorAll: () => [button] }, HTMLFormElement: Form,
    tab: { drop() {} }, WINDOWS_KEY: "windows", greeted: new Set(),
    location: { pathname: "/settings" }, SETTINGS: "/settings", WELCOME: "/welcome",
    setGuest: (guest) => { state.guest = guest; }, watchList() {}, byName: () => null, shown: () => [],
    openSettings: async () => {}, reopenWindows() {}, signIn: () => rendered.push("sign-in"),
    setup: null, seg: encodeURIComponent,
    notice: () => rendered.push("notice"), creatingAgent: () => { throw new Error("unexpected agent creation"); },
  });
  runInContext(`${apiError}\nlet writing = 0;\n${computerVars}\n${reads}\n${functions}\n${submit}\n${profile}\n${permission}`, context);
  const call = (expression) => runInContext(expression, context);
  call('identityIs({id: "owner-a"})');
  const box = ids["computer-notices"];
  const failure = () => box.children.find((row) => row.dataset.kind === "unavailable");
  return { state, requests, rendered, box, failure, layout: ids.layout,
    path: (value) => { context.location.pathname = value; },
    load: () => call("load()"), start: () => call("start()"),
    call, wake: () => call("prewake()"), profile: () => file.onchange(),
    permission: () => { context.a = state.computer.agents[0]; return button.onclick(); },
    api: (method, path) => { context.requestArgs = [method, path]; return call("api(...requestArgs)"); },
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
  assert.equal(h.retry().disabled, false);
  assert.equal(h.state.computer.computer, "computer-a");
  assert.ok(h.failure());
});

async function startAs(h, id) {
  const at = h.requests.length;
  const pending = h.start();
  respond(h.requests[at], { id });
  await turn();
  complete(h, at + 1, {}, 500);
  respond(h.requests[at + 3], { guest: false });
  await pending;
}

test("the real wake still applies a current identity's confirmed response", async () => {
  const h = setup();
  await seed(h);
  h.wake();
  respond(h.requests[2], { ...computer(), phase: "awake" });
  await turn();
  assert.equal(h.state.computer.phase, "awake");
  assert.equal(h.state.computer.owner, "owner-a");
});

test("the real ready poll accepts the current computer and its matching subscription", async () => {
  const h = setup();
  const pending = h.call('agentReady({chat: "chat-a", id: "agent-a", computer: "computer-a"})');
  respond(h.requests[0], listing({ ...computer(), phase: "awake" }));
  respond(h.requests[1], { subscriptions: [{ wake: true, principal: "agent-a", channel: "chat" }] });
  assert.equal(await pending, true);
  assert.equal(h.state.computer.phase, "awake");
});

for (const wakeFirst of [true, false]) {
  test(`real prewake answer ${wakeFirst ? "before" : "after"} the new identity's failed list cannot be retained`, async () => {
    const h = setup();
    await seed(h);
    h.wake();
    assert.match(h.requests[2].path, /\/wake$/);
    const pending = h.start();
    respond(h.requests[3], { id: "owner-b" });
    await turn();
    if (wakeFirst) { respond(h.requests[2], computer()); await turn(); }
    complete(h, 4, {}, 500);
    respond(h.requests[6], { guest: false });
    await pending;
    if (!wakeFirst) { respond(h.requests[2], computer()); await turn(); }
    assert.equal(h.state.me.id, "owner-b");
    assert.equal(h.state.computer, null);
    assert.equal(h.state.agents.size, 0);
    assert.ok(h.failure());
  });
}

test("wake from a previous session cannot return after actual startup switches away and back", async () => {
  const h = setup();
  await seed(h);
  h.wake();
  await startAs(h, "owner-b");
  await startAs(h, "owner-a");
  respond(h.requests[2], computer());
  await turn();
  assert.equal(h.state.me.id, "owner-a");
  assert.equal(h.state.computer, null);
});

for (const action of ["/auth/logout", "/auth/switch"]) {
  test(`a pending actual startup identity lookup cannot undo ${action}`, async () => {
    const h = setup();
    await seed(h);
    const pending = h.start();
    h.signout(action);
    respond(h.requests[2], { id: "owner-a" });
    await turn();
    assert.equal(h.requests.length, 3, "the old lookup cannot start more account reads");
    await pending;
    assert.equal(h.state.me, null);
    assert.equal(h.state.computer, null);
  });
}

test("an older identity lookup cannot overwrite a newer lookup of the same current person", async () => {
  const h = setup();
  await seed(h);
  const old = h.start(), recent = h.start();
  respond(h.requests[3], { id: "owner-a" });
  await turn();
  complete(h, 4, listing(null));
  respond(h.requests[6], { guest: false });
  await recent;
  respond(h.requests[2], { id: "owner-b" });
  await turn();
  assert.equal(h.requests.length, 7);
  await old;
  assert.equal(h.state.me.id, "owner-a");
});

test("the actual profile handler drops an old upload completion before reading identity", async () => {
  const h = setup();
  await seed(h);
  const pending = h.profile();
  assert.equal(h.requests[2].path, "/api/identities/me/picture");
  h.signout("/auth/logout");
  respond(h.requests[2], {});
  await turn();
  assert.equal(h.requests.length, 3);
  await pending;
  assert.equal(h.state.me, null);
});

test("the actual profile identity read cannot restore the person after sign-out", async () => {
  const h = setup();
  await seed(h);
  const pending = h.profile();
  respond(h.requests[2], {});
  await turn();
  assert.equal(h.requests[3].path, "/api/identities/me");
  const rejected = assert.rejects(pending, { code: "identity_changed" });
  h.signout("/auth/logout");
  respond(h.requests[3], { id: "owner-a" });
  await rejected;
  assert.equal(h.state.me, null);
});

for (const [name, expression] of [
  ["restart", "restartComputer(button)"],
  ["notice seen", 'sawNotice("life", button)'],
  ["notice later", 'updateLater("version", button)'],
  ["agent connections", null],
]) {
  test(`actual ${name} handler cannot reinstall a computer after sign-out and same-identity return`, async () => {
    const h = setup();
    await seed(h);
    const pending = expression ? h.call(expression) : h.permission();
    h.signout("/auth/switch");
    h.identity("owner-a");
    respond(h.requests[2], computer());
    await pending;
    assert.equal(h.state.computer, null);
    assert.equal(h.state.agents.size, 0);
    assert.equal(h.requests.length, 3, "no fallback GET from the old handler");
  });
}

test("computer creation is scoped to the identity and an old rejection cannot clear a newer creation", async () => {
  const h = setup();
  const old = h.call("computerOf()");
  const rejected = assert.rejects(old, { code: "identity_changed" });
  h.identity("owner-b");
  const recent = h.call("computerOf()");
  respond(h.requests[0], computer());
  await rejected;
  assert.equal(h.call("computerOf()"), recent);
  assert.equal(h.requests.length, 2);
  respond(h.requests[1], computer("computer-b", "agent-b", "owner-b"));
  await recent;
  assert.equal(h.state.computer.owner, "owner-b");
});

test("the actual ready/assignment poll cannot install a list response after identity changes", async () => {
  const h = setup();
  const pending = h.call('agentReady({chat: "chat-a", id: "agent-a", computer: "computer-a"})');
  const rejected = assert.rejects(pending, { code: "identity_changed" });
  h.identity("owner-b");
  respond(h.requests[0], listing());
  respond(h.requests[1], { subscriptions: [] });
  await rejected;
  assert.equal(h.state.computer, null);
});

test("a computer already returned by the API still cannot cross the next continuation's identity boundary", async () => {
  const h = setup();
  const pending = h.api("GET", "/api/computers/computer-a");
  respond(h.requests[0], computer());
  const value = await pending;
  h.identity("owner-b");
  h.identity("owner-a");
  // Pass the exact returned object, retaining its API provenance.
  h.state.returned = value;
  assert.equal(h.call("computerIs(state.returned)"), false);
  assert.equal(h.state.computer, null);
});

test("a background failure cannot re-enable a pending manual retry in a replacement notice", async () => {
  const h = setup();
  await seed(h);
  const failed = h.load(); complete(h, 2, {}, 500); await failed;
  const retry = h.retry().onclick();
  const background = h.load(); complete(h, 6, {}, 500); await background;
  assert.equal(h.retry().disabled, true);
  await h.retry().onclick();
  assert.equal(h.requests.length, 8);
  complete(h, 4, listing());
  await retry;
  assert.equal(h.retry().disabled, false);
});
