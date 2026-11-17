// cell/shell/agent-settings.js's Remove agent (FIN-250) against a fake
// platform: which agents it is offered for, and what it does: the chats
// the agent is the last live agent of archived first (History keeps them,
// to read and continue with another agent), then its fragment deleted.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { createAgentSettings } from "../shell/agent-settings.js";

const ME = "npub1robin";
const MAIN = { fragment: "juniper--a1b2", identity: "npub1juniper" };
const ROOK = { fragment: "rook--c3d4", identity: "npub1rook" };
const WREN = { fragment: "wren--d5e6", identity: "npub1wren" };
// Sam's agent, in a chat of Robin's with Sam
const SAMS = "npub1samsagent";

// Robin's platform: their list (agents and chats), their computer's roster,
// each chat's members, and what was asked of it. A delete leaves the list
// at once; its roster drops the agent then too, or, `stale`, not yet (as
// a computer's does a moment after). `refuse`: the path a call is refused at.
function world({ agents = [MAIN, ROOK, WREN], stale = false, refuse = null } = {}) {
  const calls = [];
  const roster = [...agents];
  const listed = new Set(agents.map((a) => a.fragment));
  const chats = [
    { name: "rook-chat--e5f6", role: "owner", kind: "chat", agents: [ROOK.identity] },
    { name: "group--g7h8", role: "owner", kind: "chat", agents: [MAIN.identity, ROOK.identity] },
    { name: "juniper-chat--j9k2", role: "owner", kind: "chat", agents: [MAIN.identity] },
    { name: "rook-old--m3n4", role: "owner", kind: "chat", agents: [ROOK.identity], archived: true },
    { name: "theirs--p5q6", role: "editor", kind: "chat", agents: [ROOK.identity] },
    { name: "pair--r7s8", role: "owner", kind: "chat", agents: [ROOK.identity, WREN.identity] },
    { name: "with-sam--t9u2", role: "owner", kind: "chat", agents: [ROOK.identity, SAMS] },
    { name: "wren-chat--v3w4", role: "owner", kind: "chat", agents: [WREN.identity] },
  ];
  const chat = (name) => chats.find((f) => f.name === name);
  const settings = createAgentSettings({
    api: async (method, path) => {
      calls.push(`${method} ${path}`);
      if (refuse && path === refuse) throw new Error("refused");
      let m;
      if (method === "PUT" && (m = path.match(/^\/api\/fragments\/([^/]+)\/archived$/))) chat(m[1]).archived = true;
      if (method === "DELETE" && (m = path.match(/^\/api\/f\/([^/]+)$/))) {
        listed.delete(m[1]);
        if (!stale) roster.splice(roster.findIndex((a) => a.fragment === m[1]), 1);
      }
      if (method === "GET" && (m = path.match(/^\/api\/f\/([^/]+)\/members$/))) {
        // a removed agent's membership stays, and its owner with it
        const members = chat(m[1]).agents.map((principal) => ({ principal, role: "editor", kind: "agent", owner: principal === SAMS ? "npub1sam" : ME }));
        return { members: [{ principal: ME, role: "owner", kind: "person" }, ...members] };
      }
      return {};
    },
    el: () => ({}), seg: (s) => encodeURIComponent(s), byName: (name) => (listed.has(name) ? { name, role: "owner", kind: "agent" } : null),
    agentFace: () => ({}), titleOf: (n) => n, named() {}, rename() {},
    agents: () => new Map(roster.map((a) => [a.identity, a])), load: async () => calls.push("load"), nameScreen() {}, renderNames() {},
    computer: () => null, homePicker: () => [], addAgent: async () => {},
    primaryAgent: () => roster[0] ?? null, chats: () => chats, own: (f) => f.role === "owner", me: () => ({ id: ME }),
  });
  return { settings, calls, archived: () => chats.filter((f) => f.archived).map((f) => f.name) };
}

test("offered for any agent but the main one, and never for the only one", () => {
  const three = world().settings;
  assert.equal(three.removable(ROOK), true);
  assert.equal(three.removable(MAIN), false, "the main agent, whom new chats go to");
  assert.equal(world({ agents: [ROOK] }).settings.removable(ROOK), false, "the only agent");
});

test("one removed, not yet off its computer, is not offered again, nor counted toward another's", async () => {
  const w = world({ agents: [ROOK, WREN], stale: true });
  await w.settings.removeAgent(WREN);
  assert.equal(w.settings.removable(WREN), false, "already removed");
  assert.equal(w.settings.removable(ROOK), false, "the only agent left");
});

test("the chats it is the last live agent of are archived first, then it is deleted; one with another live agent, archived or another's, is left", async () => {
  const w = world();
  await w.settings.removeAgent(ROOK);
  assert.deepEqual(w.calls, [
    "GET /api/f/with-sam--t9u2/members",
    "PUT /api/fragments/rook-chat--e5f6/archived",
    "DELETE /api/f/rook--c3d4",
    "load",
  ]);
  assert.deepEqual(w.archived(), ["rook-chat--e5f6", "rook-old--m3n4"], "group (Juniper), pair (Wren) and with-sam (Sam's agent) kept");
});

// Rook then Wren from the chat they shared: Rook's membership stays (its
// words are its), so the chat still names it; removing Wren finds Rook not
// live, its computer let go of it or not yet, and archives the chat.
for (const stale of [false, true]) {
  test(`removed one after another from a shared chat, the last archives it${stale ? " (the computer not yet let go of the first)" : ""}`, async () => {
    const w = world({ stale });
    await w.settings.removeAgent(ROOK);
    assert.ok(!w.archived().includes("pair--r7s8"), "Wren is still in it");
    w.calls.length = 0;
    await w.settings.removeAgent(WREN);
    assert.deepEqual(w.calls, [
      "GET /api/f/pair--r7s8/members",
      "PUT /api/fragments/pair--r7s8/archived",
      "PUT /api/fragments/wren-chat--v3w4/archived",
      "DELETE /api/f/wren--d5e6",
      "load",
    ]);
    assert.ok(!w.archived().includes("group--g7h8") && !w.archived().includes("with-sam--t9u2"), "Juniper's and Sam's agent's chats kept");
  });
}

test("a delete refused leaves it, its chats archived (Remove again finds them so, and deletes it)", async () => {
  const w = world({ refuse: "/api/f/rook--c3d4" });
  await assert.rejects(w.settings.removeAgent(ROOK), /refused/);
  assert.deepEqual(w.calls.filter((c) => !c.startsWith("GET")), ["PUT /api/fragments/rook-chat--e5f6/archived", "DELETE /api/f/rook--c3d4"]);
});

test("an archive refused stops it before the delete: nothing is deleted", async () => {
  const w = world({ refuse: "/api/fragments/rook-chat--e5f6/archived" });
  await assert.rejects(w.settings.removeAgent(ROOK), /refused/);
  assert.deepEqual(w.calls.filter((c) => !c.startsWith("GET")), ["PUT /api/fragments/rook-chat--e5f6/archived"]);
});
