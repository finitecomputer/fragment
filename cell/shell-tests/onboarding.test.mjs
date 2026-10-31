// cell/shell/onboarding.js's Add agent (Settings' Agents) against a fake
// platform: its calls answered as docs/api.md says, one of them cut (as a
// lost connection: before it is done, or done with its answer lost), and a
// reload as a page of its own, made again from what the platform holds.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";

// What the module reaches for beyond what it is handed: the first run's
// card listens to the page, and a making puts the address back at home.
globalThis.matchMedia = () => ({ matches: false });
globalThis.addEventListener = () => {};
globalThis.history = { replaceState() {} };
const { createOnboarding } = await import("../shell/onboarding.js");

const SUFFIX = "abcdefghjkmnpqrstuvwxyz23456789";
const labelOf = (name) => name.replace(/--[a-km-np-z2-9]{4}$/, "");

// The platform: fragments as the list says them, files at main, members,
// one computer, and each agent's `tasks`.
function platform() {
  const rows = [];
  const files = new Map();
  const members = new Map();
  const tasks = new Map();
  const computer = { computer: "c1", phase: "awake", agents: [] };
  let made = 0;
  let cut = null;
  const writes = [];
  const row = (name) => rows.find((f) => f.name === name);
  const make = (label, kind, title) => {
    const n = made++;
    const name = `${label}--${[0, 1, 2, 3].map((i) => SUFFIX[Math.floor(n / SUFFIX.length ** i) % SUFFIX.length]).join("")}`;
    rows.push({ name, role: "owner", kind, title, agents: [] });
    files.set(name, new Map([["fragment.json", JSON.stringify({ template: kind, meta: { title } })]]));
    members.set(name, new Set());
    return name;
  };
  const join = (chat, id) => {
    members.get(chat).add(id);
    if (!row(chat).agents.includes(id)) row(chat).agents.push(id);
  };
  const answer = (method, path, body) => {
    let m;
    if (method === "POST" && path === "/api/fragments") return { name: make(body.label, body.template, body.title) };
    if (method === "GET" && path === "/api/computers") return { computers: [computer] };
    if ((m = path.match(/^\/api\/computers\/c1\/agents\/([^/]+)$/)) && method === "PUT") {
      if (!computer.agents.some((a) => a.fragment === m[1])) computer.agents.push({ fragment: m[1], identity: `npub1${labelOf(m[1])}` });
      return { agents: computer.agents };
    }
    if (path === "/api/computers/c1/wake") return computer;
    if (!(m = path.match(/^\/api\/f\/([^/]+)\/([^?]+)(?:\?(.*))?$/))) throw new Error(`no route: ${method} ${path}`);
    const [, name, rest, query] = m;
    assert.ok(row(name), `${method} ${path}: no such fragment`);
    if (method === "GET" && rest === "file") {
      const text = files.get(name).get(new URLSearchParams(query).get("path"));
      if (text === undefined) throw Object.assign(new Error("not found"), { status: 404 });
      return { text };
    }
    if (method === "GET" && rest === "manifest") return JSON.parse(files.get(name).get("fragment.json"));
    if (method === "POST" && rest === "files") {
      for (const f of body.files) files.get(name).set(f.path, f.text);
      return { commit: "c" };
    }
    if (method === "POST" && rest === "deploy") {
      const role = JSON.parse(files.get(name).get("fragment.json")).meta?.role;
      if (role) row(name).agentRole = role;
      return {};
    }
    if (method === "GET" && rest === "members") return { members: [...members.get(name)].map((principal) => ({ principal })) };
    if (method === "PUT" && rest.startsWith("members/")) return (join(name, decodeURIComponent(rest.slice("members/".length))), {});
    if (method === "GET" && rest === "subscriptions") return { subscriptions: [...members.get(name)].map((principal) => ({ wake: true, principal, channel: "chat" })) };
    if (method === "POST" && rest === "channels/tasks") {
      const kept = tasks.get(name) ?? new Map();
      tasks.set(name, kept);
      if (!kept.has(body.id)) kept.set(body.id, body.body);
      return {};
    }
    throw new Error(`no route: ${method} ${path}`);
  };
  const api = async (method, path, body) => {
    if (method !== "GET") writes.push(`${method} ${path}`);
    if (cut && cut.when(method, path, body)) {
      const { done } = cut;
      cut = null;
      if (done) answer(method, path, body);
      throw new Error("Failed to fetch");
    }
    return structuredClone(answer(method, path, body));
  };
  // Their first agent, made and talked to: on their computer, in its chat.
  const first = make("xbt-2000", "agent", "XBT-2000");
  const firstChat = make("xbt-2000-chat", "chat", "Your first chat");
  computer.agents.push({ fragment: first, identity: "npub1xbt" });
  files.get(first).set("agent.json", "{}");
  join(firstChat, "npub1xbt");
  Object.assign(row(firstChat), { preview: "Hello!", previewAt: 1 });
  return {
    api,
    rows: () => structuredClone(rows),
    computer: () => structuredClone(computer),
    files: (name) => files.get(name),
    tasks: (name) => tasks.get(name),
    members: (name) => members.get(name),
    make,
    // what was asked to change since the last ask
    writes: () => writes.splice(0),
    // the next call `when` names fails; `done`, after it was done
    cut: (when, done = false) => { cut = { when, done }; },
  };
}

// A page of the shell over it: its own reads of the platform, and the
// onboarding made as shell.js makes it.
function page(world) {
  const state = { fragments: [], computer: null, agents: new Map(), frames: new Set() };
  const load = async () => {
    state.fragments = world.rows();
    state.computer = world.computer();
    state.agents = new Map(state.computer.agents.map((a) => [a.identity, a]));
  };
  const node = () => ({ style: { setProperty() {} }, classList: { add() {}, remove() {} }, addEventListener() {}, replaceChildren() {}, append() {}, remove() {}, hidden: false });
  const own = (f) => f.role === "owner";
  const chats = () => state.fragments.filter((f) => f.kind === "chat");
  const titleOf = (name) => state.fragments.find((f) => f.name === name)?.title || labelOf(name);
  const onboarding = createOnboarding({
    api: world.api,
    ui: { $: node, el: node, notice() {} },
    account: { person: () => ({ email: "kim@e2e.test" }), isGuest: () => false, setGuest() {}, hasInvites: () => 0 },
    fragments: {
      allFragments: () => state.fragments,
      chats,
      own,
      labelOf,
      titleOf,
      primaryAgent: () => {
        const first = [...state.agents.values()][0];
        return first ? { fragment: first.fragment, identity: first.identity } : null;
      },
      agentsOwnChat: (agent) => chats().find((f) => own(f) && labelOf(f.name) === `${labelOf(agent)}-chat`) ?? null,
      freeLabel: (base) => {
        const taken = new Set(state.fragments.map((f) => labelOf(f.name)));
        for (let i = 1; ; i++) {
          const l = i === 1 ? base : `${base}-${i}`;
          if (!taken.has(l) && !taken.has(`${l}-chat`)) return l;
        }
      },
      slug: (s) => s.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, ""),
      seg: (s) => encodeURIComponent(s),
    },
    computer: { computerView: () => state.computer, computerFailed: () => false, agents: () => state.agents, computerOf: async () => state.computer, computerIs() {} },
    navigation: { start() {}, openChat: (name) => state.frames.add(name), atHome: () => true, hasFrame: (name) => state.frames.has(name), welcomePath: "/welcome" },
    reads: () => ({ listFailed: false, updating: false }),
    load,
    skillsFragment: async () => ({}),
    rosterChanged() {},
  });
  // A page as the shell opens it: read, then a making owed or cut short
  // goes on (shell.js `start`).
  const opened = async () => {
    await load();
    if (await onboarding.owesFirstAgent()) await onboarding.creatingAgent();
    else await onboarding.resumeFirstRun();
    await load();
  };
  return { onboarding, opened, load };
}

// Wren made whole: one agent of that name with its role and job, on the
// computer, in one chat of its own, its hello posted there; and nothing
// else made (no default agent, no second chat).
function madeWhole(world) {
  const rows = world.rows();
  const wrens = rows.filter((f) => f.kind === "agent" && f.title === "Wren");
  assert.equal(wrens.length, 1, "one agent named Wren");
  const [wren] = wrens;
  const chats = rows.filter((f) => f.kind === "chat" && labelOf(f.name).startsWith("wren"));
  assert.deepEqual(chats.map((f) => labelOf(f.name)), ["wren-chat"], "one chat of Wren's");
  assert.equal(rows.length, 4, "nothing else made");
  const id = world.computer().agents.find((a) => a.fragment === wren.name)?.identity;
  assert.ok(id, "Wren is on the computer");
  assert.ok(world.members(chats[0].name).has(id), "Wren is in its chat");
  assert.equal(JSON.parse(world.files(wren.name).get("fragment.json")).meta.role, "Researcher", "its role");
  assert.match(world.files(wren.name).get("SOUL.md"), /one of kim@e2e\.test's agents/, "its job");
  assert.equal(world.tasks(wren.name)?.get("hello")?.chat, chats[0].name, "its hello, in its chat");
}

const adding = async (p) => p.onboarding.addAgent({ name: "Wren", role: "Researcher" });
const create = (label) => (method, path, body) => method === "POST" && path === "/api/fragments" && body.label === label;
const at = (re, method) => (m, path) => m === method && re.test(path);

test("Add makes the agent whole, uncut", async () => {
  const world = platform();
  const p = page(world);
  await p.load();
  await adding(p);
  madeWhole(world);
});

// Each place a reload may cut it: the page that asked is gone, and the
// next one opened finishes it from what was made.
for (const [where, when, done] of [
  ["its agent made, its answer lost", create("wren"), true],
  ["its job and role written, its answer lost", at(/\/files$/, "POST"), true],
  ["its chat made, its answer lost", create("wren-chat"), true],
  ["its chat made, before its computer took it", at(/\/agents\/wren--/, "PUT"), false],
  ["on its computer, before its deploy", at(/^\/api\/f\/wren--[^/]+\/deploy$/, "POST"), false],
  ["deployed, before it joined its chat", at(/\/members\/npub1wren$/, "PUT"), false],
  ["in its chat, before its hello", at(/^\/api\/f\/wren--[^/]+\/channels\/tasks$/, "POST"), false],
]) {
  test(`a reload with Add cut short (${where}) finishes it`, async () => {
    const world = platform();
    const first = page(world);
    await first.load();
    world.cut(when, done);
    await adding(first);
    // made before its chat, the agent cut short is the one Add makes on
    const again = page(world);
    await again.opened();
    if (!world.rows().some((f) => f.kind === "chat" && labelOf(f.name) === "wren-chat")) await adding(again);
    madeWhole(world);
  });
}

test("Try again on the same page finishes what failed", async () => {
  const world = platform();
  const p = page(world);
  await p.load();
  world.cut(at(/\/members\/npub1wren$/, "PUT"));
  await adding(p);
  const chat = p.onboarding.setupChat();
  assert.ok(chat && p.onboarding.setupFor(chat).error, "its chat says what failed");
  await assert.rejects(adding(p), /Another agent is being made/);
  p.onboarding.retrySetup(chat);
  await p.onboarding.creatingAgent();
  madeWhole(world);
});

// Made whole, its chat still quiet (its hello not answered yet): a page
// opened then posts its hello again, the same record, and nothing else; a
// second chat of its label (a create whose answer was lost) is left be.
for (const stray of [false, true]) {
  test(`a page opened on a made agent's quiet chat makes nothing${stray ? ", nor of a stray chat of its label" : ""}`, async () => {
    const world = platform();
    const p = page(world);
    await p.load();
    await adding(p);
    if (stray) world.make("wren-chat", "chat", "Your first chat");
    world.writes();
    const before = JSON.stringify([world.rows(), world.computer()]);
    await page(world).opened();
    assert.equal(JSON.stringify([world.rows(), world.computer()]), before);
    assert.deepEqual(world.writes().map((w) => w.replace(/--[a-z0-9]{4}/g, "")), ["POST /api/f/wren/channels/tasks"]);
  });
}
