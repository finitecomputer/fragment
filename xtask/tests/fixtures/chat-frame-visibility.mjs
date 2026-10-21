// Execute production navigation, visibility messaging, presence callbacks and
// notify_reply. Window scheduling, DOM, HTTP, socket identity and push are fake.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { createContext, runInContext } from "node:vm";
const shell = readFileSync(new URL("../../../cell/shell/shell.js", import.meta.url), "utf8");
const page = readFileSync(new URL("../../../templates/chat/site/chat.js", import.meta.url), "utf8");
const app = readFileSync(new URL("../../../templates/chat/app.mjs", import.meta.url), "utf8");
function fn(source, name) {
  const found = source.match(new RegExp(`^(?:async )?function ${name}\\([^]*?^\\}`, "m"));
  assert.ok(found, name);
  return found[0];
}
const shellVisibility = shell.slice(shell.indexOf("const chatVisibility ="), shell.indexOf("// A frame that could not sign in"));
const pagePresence = page.match(/^  let shellVisible[^]*?(?=^  input.addEventListener\("input")/m)?.[0];
const initialPresence = page.match(/^  fragment.presence.set\(\{ \.\.\.shared \}\);(?=\n  fragment.presence.on)/m)?.[0];
const hello = page.match(/^  fragment.me\(\).then\(\(hello\) => \{\n    state.me = hello;\n    askVisibility\(\);/m)?.[0];
assert.ok(shellVisibility.startsWith("const chatVisibility =") && pagePresence && initialPresence && hello);
const tick = () => new Promise(setImmediate);
const plain = (value) => JSON.parse(JSON.stringify(value));
function hub() {
  const listeners = new Map();
  return {
    addEventListener(name, callback) { const list = listeners.get(name) ?? []; list.push(callback); listeners.set(name, list); },
    emit(name, event = {}) { return (listeners.get(name) ?? []).map((callback) => callback(event)); },
  };
}

function setup({ standalone = false, holdOrigins = false, releaseProbe = false, visibilityProtocol = true } = {}) {
  const errors = [], outbound = [], queue = [], originReads = [], clients = new Map();
  const shellEvents = hub(), shellDocument = { ...hub(), hidden: false, querySelector: () => null };
  const release = { mine: "old", next: null, reloading: false }, releaseTimers = [];
  const navigation = { reloads: 0, stops: 0 };
  const nodes = Object.fromEntries(["layout", "settings-page", "frames", "notice", "first-run", "first-run-card"].map((id) => [id, {
    hidden: id === "settings-page", classList: { add() {}, remove() {} }, replaceChildren() {}, append() {},
  }]));
  const state = { me: { id: "person-a" }, current: null, page: null, frames: new Map(), fragments: [{ name: "chat-a" }, { name: "chat-b" }] };
  const call = (results) => { for (const result of results) Promise.resolve(result).catch((error) => errors.push(error)); };
  const shellContext = createContext({
    state, document: shellDocument, URL, addEventListener: shellEvents.addEventListener,
    $: (id) => nodes[id], byName: (name) => state.fragments.find((f) => f.name === name), seg: encodeURIComponent,
    api: (_method, path) => {
      const name = decodeURIComponent(path.split("/")[3]);
      const answer = { urls: { canonical: `https://${name}.example.test/` } };
      if (holdOrigins) return new Promise((resolve, reject) => originReads.push({ resolve: () => resolve(answer), reject }));
      return Promise.resolve(answer);
    },
    at() {}, store: { set() {} }, CURRENT: "current", SETTINGS: "/settings", titleOf: (name) => name,
    renderHeading() {}, renderChats() {}, markSeen() {}, prewake() {}, navigated() {}, leaveSidebar() {},
    el() { return {}; }, section() {}, line() {}, actionLine() {}, usd() {}, noSeatSaid() {}, switchForm() {},
    // Pause after the real Settings navigation code; billing/rendering is unrelated.
    returning: () => new Promise(() => {}), clearSplashEdge() {},
    release, typedIn: new Set(), menu: { hidden: true }, writing: 0, composing: new Set(), pendingLaunch: null, starting: false,
    setTimeout(callback) { releaseTimers.push(callback); }, lastReload: () => null,
    tab: { set: () => true, drop() {} }, WINDOWS_KEY: "windows", RELOADED_KEY: "release", openWindows: () => [],
    RELOAD_SETTLE_MS: 100, NAVIGATED_SETTLE_MS: 400,
    location: { reload() { navigation.reloads++; } }, window: { stop() { navigation.stops++; } },
  });
  runInContext(`const origins = new Map();\n${fn(shell, "originOf")}\n${visibilityProtocol ? shellVisibility : "function tellChatsVisibility() {}"}\n${["openChat", "openSettings", "firstRun"].map((name) => fn(shell, name)).join("\n")}`, shellContext);
  if (releaseProbe) {
    const busy = shell.slice(shell.indexOf("const greeted ="), shell.indexOf("// Reloads for `release.next`"));
    assert.ok(busy.startsWith("const greeted =") && busy.includes("const chatsBusy"));
    runInContext(busy + ["shellHolds", "reloadWhenSafe", "stopReload", "navigated"].map((name) => fn(shell, name)).join("\n"), shellContext);
    shellDocument.addEventListener("visibilitychange", () => shellContext.reloadWhenSafe("hidden"));
  }

  for (const name of ["chat-a", "chat-b"]) {
    const events = hub(), document = { ...hub(), visibilityState: "visible" }, publications = [], timers = new Map();
    let id = 0;
    const source = {}, window = {};
    const chatState = { me: null, typed: new Map(), answering: new Set(), sending: new Set(), failed: new Set(), replyTo: null };
    const vm = createContext({
      state: chatState, document, window, framed: !standalone, TYPING_MS: 5000,
      crypto: { randomUUID: () => `${name}-document-${++id}` }, addEventListener: events.addEventListener,
      setTimeout(callback) { const timer = {}; timers.set(timer, callback); return timer; }, clearTimeout(timer) { timers.delete(timer); },
      fragment: { presence: { set(data) { publications.push(plain(data)); } }, me: async () => ({ principal: "person-a", platform: "https://platform.example.test" }) },
    });
    // Each window sees its own proxy to the parent, retaining source identity.
    vm.window.parent = { postMessage(data, target) {
      outbound.push({ from: name, target, data: plain(data) });
      if (target === "https://platform.example.test") queue.push(() => call(shellEvents.emit("message", { source, origin: `https://${name}.example.test`, data })));
    } };
    const parentProxy = vm.window.parent;
    source.postMessage = (data, target) => {
      outbound.push({ to: name, target, data: plain(data) });
      if (target === `https://${name}.example.test`) queue.push(() => call(events.emit("message", { source: parentProxy, origin: "https://platform.example.test", data })));
    };
    const frame = { hidden: true, isConnected: true, dataset: { fragment: name }, contentWindow: source };
    state.frames.set(name, frame);
    runInContext(pagePresence + initialPresence + hello + "\n  });", vm);
    runInContext("const attachments = []; let recording = false, sending = false, answersOnTheirWay = 0, busyTold;\n" + fn(page.replace(/^  /gm, ""), "tellBusy"), vm);
    clients.set(name, {
      frame, events, document, publications, state: chatState, parent: parentProxy,
      looking: () => publications.at(-1).looking,
      nonce: () => outbound.filter((m) => m.from === name && m.data.fragment === "chat-visibility?").at(-1)?.data.nonce,
      receive: (data, origin = "https://platform.example.test", from = parentProxy) => call(events.emit("message", { data, origin, source: from })),
      lifecycle: (event) => call(events.emit(event)),
      visibility(value) { document.visibilityState = value; call(document.emit("visibilitychange")); },
      type(value) { vm.setTyping(value); },
      file(value) { runInContext(`attachments.length = ${value ? 1 : 0}; tellBusy();`, vm); },
    });
  }
  return {
    clients, state, nodes, outbound, originReads, release, navigation,
    settleRelease() { while (releaseTimers.length) releaseTimers.shift()(); },
    open: (name) => shellContext.openChat(name), settings: () => shellContext.openSettings(), firstRun: () => shellContext.firstRun(),
    message: (event) => call(shellEvents.emit("message", event)),
    lifecycle: (event) => call(shellEvents.emit(event)),
    visibility(hidden) { shellDocument.hidden = hidden; call(shellDocument.emit("visibilitychange")); },
    async flush() { for (let i = 0; i < 5; i++) { while (queue.length) queue.shift()(); await tick(); } assert.deepEqual(errors, []); },
    async notify(name) {
      const context = createContext({ DurableObject: class {} });
      runInContext(app.replace(/^import \{ DurableObject \} from "cloudflare:workers";$/m, "").replace("export class App", "class App") + "\nglobalThis.app = new App();", context);
      const pushes = [];
      await context.app.notify_reply({ record: { principal: "agent", body: { turn: "one", text: "Reply" } } }, {
        via: "channel", fragment: name, members: async () => [{ principal: "person-a", kind: "person" }, { principal: "agent", kind: "agent" }],
        presence: async () => [{ principal: "person-a", data: clients.get(name).publications.at(-1) }], people: async () => ({}),
        push: async (who, payload) => { pushes.push({ who, payload: plain(payload) }); return { queued: 1 }; },
      });
      return pushes;
    },
  };
}

test("active chat suppresses its push; CSS-hidden inactive chat receives its unchanged notification", async () => {
  const h = setup(); h.open("chat-a"); await h.flush();
  assert.equal(h.clients.get("chat-a").document.visibilityState, "visible");
  assert.equal(h.clients.get("chat-b").document.visibilityState, "visible");
  assert.deepEqual(await h.notify("chat-a"), []);
  assert.deepEqual(await h.notify("chat-b"), [{ who: "person-a", payload: { title: "Your agent", body: "Reply", tag: "chat-b", url: "./" } }]);
  h.open("chat-b"); await h.flush();
  assert.equal((await h.notify("chat-a")).length, 1);
  assert.deepEqual(await h.notify("chat-b"), []);
});

for (const where of ["settings", "firstRun"]) {
  test(`${where} stops suppression and returning to chat restores it`, async () => {
    const h = setup(); h.open("chat-a"); await h.flush();
    h[where](); await h.flush();
    assert.equal((await h.notify("chat-a")).length, 1);
    h.nodes.layout.hidden = false; h.open("chat-a"); await h.flush();
    assert.deepEqual(await h.notify("chat-a"), []);
  });
}

test("shell visibility and page lifecycle events update presence without duplicate publications", async () => {
  const h = setup(); h.open("chat-a"); await h.flush(); const c = h.clients.get("chat-a");
  h.visibility(true); await h.flush(); assert.equal(c.looking(), false);
  h.visibility(false); await h.flush(); assert.equal(c.looking(), true);
  const count = c.publications.length;
  h.visibility(false); h.lifecycle("pageshow"); await h.flush(); assert.equal(c.publications.length, count);
  h.lifecycle("pagehide"); await h.flush(); assert.equal(c.looking(), false);
  h.lifecycle("pageshow"); await h.flush(); assert.equal(c.looking(), true);
});

test("a same-person identity refresh keeps the frame valid, another account does not", async () => {
  const h = setup(); h.open("chat-a"); await h.flush(); const c = h.clients.get("chat-a");
  h.state.me = { id: "person-a", picture: "/new-picture" }; h.settings(); await h.flush();
  h.open("chat-a"); await h.flush(); assert.equal(c.looking(), true);
  h.state.me = { id: "person-b" }; h.open("chat-a"); await h.flush(); assert.equal(c.looking(), false);
  h.state.me = null; h.firstRun(); await h.flush(); assert.equal(c.looking(), false);
});

test("typing publications preserve whether the shell actually shows the chat", async () => {
  const h = setup(); h.open("chat-a"); await h.flush(); const a = h.clients.get("chat-a"), b = h.clients.get("chat-b");
  a.type(true); b.type(true);
  assert.deepEqual(a.publications.at(-1), { typing: true, looking: true });
  assert.deepEqual(b.publications.at(-1), { typing: true, looking: false });
  h.open("chat-b"); await h.flush();
  assert.deepEqual(a.publications.at(-1), { typing: true, looking: false });
  assert.deepEqual(b.publications.at(-1), { typing: true, looking: true });
});

test("failed origin discovery never claims a framed chat is visible", async () => {
  const h = setup({ holdOrigins: true }); h.open("chat-a"); await h.flush();
  for (const read of h.originReads) read.reject(new Error("offline")); await h.flush();
  assert.equal(h.clients.get("chat-a").looking(), false);
  assert.ok(!h.outbound.some((m) => m.to));
});

test("child pagehide retires its nonce; pageshow asks afresh and ignores stale replies", async () => {
  const h = setup(); h.open("chat-a"); await h.flush(); const c = h.clients.get("chat-a"), old = c.nonce();
  c.lifecycle("pagehide"); c.receive({ fragment: "chat-visibility", nonce: old, visible: true });
  assert.equal(c.looking(), false);
  c.lifecycle("pageshow"); assert.notEqual(c.nonce(), old);
  c.receive({ fragment: "chat-visibility", nonce: old, visible: true }); assert.equal(c.looking(), false);
  await h.flush(); assert.equal(c.looking(), true);
  c.visibility("hidden"); await h.flush(); assert.equal(c.looking(), false);
  c.visibility("visible"); await h.flush(); assert.equal(c.looking(), true);
});

test("a fresh visibility request clears old confirmation until the shell answers", async () => {
  const h = setup(); h.open("chat-a"); await h.flush(); const c = h.clients.get("chat-a"), old = c.nonce();
  c.visibility("visible");
  assert.equal(c.looking(), false);
  c.receive({ fragment: "chat-visibility", nonce: old, visible: true }); assert.equal(c.looking(), false);
  h.settings(); await h.flush(); assert.equal(c.looking(), false);
});

for (const attack of ["origin", "source", "nonce", "type"]) {
  test(`child rejects a visibility reply with wrong ${attack}`, async () => {
    const h = setup(); h.open("chat-a"); await h.flush(); const c = h.clients.get("chat-b");
    const data = { fragment: "chat-visibility", nonce: attack === "nonce" ? "stale" : c.nonce(), visible: attack === "type" ? "true" : true };
    c.receive(data, attack === "origin" ? "https://stranger.example.test" : undefined, attack === "source" ? {} : undefined);
    assert.equal(c.looking(), false);
  });
}

for (const change of ["settings", "other-chat", "detached", "replaced", "account", "removed-membership", "window"]) {
  test(`an origin lookup finishing after ${change} cannot activate the old chat`, async () => {
    const h = setup({ holdOrigins: true }); h.open("chat-a"); await h.flush(); const c = h.clients.get("chat-a");
    if (change === "settings") h.settings();
    if (change === "other-chat") h.open("chat-b");
    if (change === "detached") c.frame.isConnected = false;
    if (change === "replaced") h.state.frames.set("chat-a", { ...c.frame });
    if (change === "account") h.state.me = { id: "person-b" };
    if (change === "removed-membership") h.state.fragments = [];
    if (change === "window") c.frame.contentWindow = {};
    for (const read of h.originReads) read.resolve(); await h.flush();
    assert.equal(c.looking(), false);
  });
}

test("a newer request supersedes an older pending request for the same frame", async () => {
  const h = setup({ holdOrigins: true }); h.open("chat-a"); await h.flush(); const c = h.clients.get("chat-a");
  const old = c.nonce(); c.lifecycle("pageshow"); await h.flush();
  for (const read of h.originReads) read.resolve(); await h.flush();
  assert.equal(c.looking(), true);
  assert.ok(!h.outbound.some((m) => m.to === "chat-a" && m.data.nonce === old));
});

for (const attack of ["origin", "source", "empty-nonce", "long-nonce"]) {
  test(`shell rejects a request with wrong ${attack}`, async () => {
    const h = setup(); h.open("chat-a"); await h.flush(); const c = h.clients.get("chat-a"), before = h.outbound.length;
    h.message({ source: attack === "source" ? {} : c.frame.contentWindow, origin: attack === "origin" ? "https://stranger.example.test" : "https://chat-a.example.test",
      data: { fragment: "chat-visibility?", nonce: attack === "empty-nonce" ? "" : attack === "long-nonce" ? "x".repeat(65) : "fake-request" } });
    await h.flush(); assert.equal(h.outbound.length, before);
  });
}

test("standalone chat retains document visibility behavior and sends no shell requests", async () => {
  const h = setup({ standalone: true }); const c = h.clients.get("chat-a"); await h.flush();
  assert.equal(c.looking(), true); c.visibility("hidden"); assert.equal(c.looking(), false);
  c.visibility("visible"); assert.equal(c.looking(), true);
  c.receive({ fragment: "chat-visibility", visible: false, nonce: "fake" }); assert.equal(c.looking(), true);
  assert.deepEqual(h.outbound, []);
});

test("visibility messages carry no identity, roster or notification content", async () => {
  const h = setup(); h.open("chat-a"); await h.flush();
  for (const message of h.outbound) {
    assert.notEqual(message.target, "*");
    assert.deepEqual(Object.keys(message.data).sort(), (message.from ? ["fragment", "nonce"] : ["fragment", "nonce", "visible"]).sort());
  }
});

for (const visibilityProtocol of [false, true]) {
  test(`release interaction (visibility protocol ${visibilityProtocol ? "on" : "off"}): a file blocks reload; picked during reload, it stops it`, async () => {
    const h = setup({ releaseProbe: true, visibilityProtocol });
    h.open("chat-a"); await h.flush();
    const frame = h.state.frames.get("chat-a"), child = h.clients.get("chat-a");
    h.release.next = "new";
    child.file(true); await h.flush();
    h.visibility(true); await h.flush(); h.settleRelease();
    assert.equal(h.navigation.reloads, 0, "presence does not erase a real busy notification");
    h.visibility(false); child.file(false); await h.flush(); h.settleRelease();
    h.visibility(true); await h.flush(); h.settleRelease();
    assert.equal(h.navigation.reloads, 1, "visibility alone does not hold a safe reload");
    assert.ok(h.release.reloading);
    child.file(true); await h.flush();
    assert.equal(h.navigation.stops, 1, "the production busy callback stops an in-flight reload");
    assert.equal(h.release.reloading, false);
    assert.equal(h.state.frames.get("chat-a"), frame, "presence neither removes nor replaces the frame");
    assert.equal(frame.hidden, false);
    h.visibility(false); child.file(false); await h.flush(); h.settleRelease();
    h.visibility(true); await h.flush(); h.settleRelease();
    assert.equal(h.navigation.reloads, 2, "reload remains available after the file is removed");
  });
}
