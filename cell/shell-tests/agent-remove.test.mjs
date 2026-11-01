// cell/shell/agent-settings.js's Remove agent (FIN-250) against a fake
// platform: which agents it is offered for, and what it does: the chats
// the agent is the only agent of archived first (History keeps them, to
// read and continue with another agent), then its fragment deleted.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { createAgentSettings } from "../shell/agent-settings.js";

const MAIN = { fragment: "juniper--a1b2", identity: "npub1juniper" };
const ROOK = { fragment: "rook--c3d4", identity: "npub1rook" };

// Robin's agents and chats, and what was asked of the platform; `refuse`
// the path a call is refused at.
function world({ agents = [MAIN, ROOK], refuse = null } = {}) {
  const calls = [];
  const chats = [
    { name: "rook-chat--e5f6", role: "owner", kind: "chat", agents: [ROOK.identity] },
    { name: "group--g7h8", role: "owner", kind: "chat", agents: [MAIN.identity, ROOK.identity] },
    { name: "juniper-chat--j9k2", role: "owner", kind: "chat", agents: [MAIN.identity] },
    { name: "rook-old--m3n4", role: "owner", kind: "chat", agents: [ROOK.identity], archived: true },
    { name: "theirs--p5q6", role: "editor", kind: "chat", agents: [ROOK.identity] },
  ];
  const settings = createAgentSettings({
    api: async (method, path) => {
      calls.push(`${method} ${path}`);
      if (refuse && path === refuse) throw new Error("refused");
      return {};
    },
    el: () => ({}), seg: (s) => encodeURIComponent(s), byName: () => null, agentFace: () => ({}), titleOf: (n) => n, named() {}, rename() {},
    agents: () => new Map(agents.map((a) => [a.identity, a])), load: async () => calls.push("load"), nameScreen() {}, renderNames() {},
    computer: () => null, homePicker: () => [], addAgent: async () => {},
    primaryAgent: () => agents[0] ?? null, chats: () => chats, own: (f) => f.role === "owner",
  });
  return { settings, calls };
}

test("offered for any agent but the main one, and never for the only one", () => {
  const two = world().settings;
  assert.equal(two.removable(ROOK), true);
  assert.equal(two.removable(MAIN), false, "the main agent, whom new chats go to");
  assert.equal(world({ agents: [ROOK] }).settings.removable(ROOK), false, "the only agent");
});

test("its chats alone are archived first, then it is deleted; a chat shared with another agent, one archived or another's, is left", async () => {
  const { settings, calls } = world();
  await settings.removeAgent(ROOK);
  assert.deepEqual(calls, ["PUT /api/fragments/rook-chat--e5f6/archived", "DELETE /api/f/rook--c3d4", "load"]);
});

test("a delete refused leaves it, its chats archived (Remove again finds them so, and deletes it)", async () => {
  const w = world({ refuse: "/api/f/rook--c3d4" });
  await assert.rejects(w.settings.removeAgent(ROOK), /refused/);
  assert.deepEqual(w.calls, ["PUT /api/fragments/rook-chat--e5f6/archived", "DELETE /api/f/rook--c3d4"]);
});

test("an archive refused stops it before the delete: nothing is deleted", async () => {
  const w = world({ refuse: "/api/fragments/rook-chat--e5f6/archived" });
  await assert.rejects(w.settings.removeAgent(ROOK), /refused/);
  assert.deepEqual(w.calls, ["PUT /api/fragments/rook-chat--e5f6/archived"]);
});
