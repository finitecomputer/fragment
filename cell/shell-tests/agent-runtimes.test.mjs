// cell/shell/agent-runtimes.js: what each of the person's agents' runtime
// offers beyond its chats, from its agent.json's `runtime`: a Claude agent
// no profile and no screen, a Hermes one both; unread, or unreadable, as
// before; a runtime the shell does not know, nothing. The controls drawn
// for it (Settings' Open, its Screens) are hidden as its runtime is read.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { createRuntimes, RUNTIME_OFFERS } from "../shell/agent-runtimes.js";

// Robin's agents on their computer, each agent.json as the platform answers
// it (`null`: a read that fails; absent: none, 404), and Sam's agent, which
// Robin's list holds as another's.
const HELD = { "clio--a1b2": { runtime: "claude" }, "juniper--c3d4": { tier: "cheap" }, "wren--e5f6": null, "quill--g7h8": { runtime: "codex" } };
function world() {
  const roster = ["clio--a1b2", "juniper--c3d4", "wren--e5f6", "quill--g7h8", "rook--j9k2", "sams--m3n4"].map((fragment) => ({ fragment, identity: `npub1${fragment}` }));
  const asked = [];
  const agentJson = async (agent) => {
    asked.push(agent);
    if (!(agent in HELD)) throw Object.assign(new Error("not found"), { status: 404 });
    if (HELD[agent] === null) throw Object.assign(new Error("refused"), { status: 500 });
    return structuredClone(HELD[agent]);
  };
  const runtimes = createRuntimes({
    agentJson, agents: () => new Map(roster.map((a) => [a.identity, a])),
    own: (f) => f.role === "owner", byName: (name) => ({ name, role: name === "sams--m3n4" ? "editor" : "owner" }),
  });
  return { runtimes, asked };
}
const ALL = ["profile", "screen", "skills", "doors"];
const offered = (r, agent) => ALL.filter((what) => r.offers(agent, what));

test("a Claude agent offers neither its profile, its screen nor a door; a Hermes one all; none named is Hermes", async () => {
  const { runtimes } = world();
  await runtimes.loadRuntimes();
  assert.deepEqual(offered(runtimes, "clio--a1b2"), []);
  assert.deepEqual(offered(runtimes, "juniper--c3d4"), ALL);
  assert.deepEqual(offered(runtimes, "rook--j9k2"), ALL, "no agent.json (404): Hermes");
});

test("unread, or its read failed, an agent is offered what it was before; a runtime not known here, nothing", async () => {
  const { runtimes } = world();
  assert.deepEqual(offered(runtimes, "clio--a1b2"), ALL, "not read yet");
  await runtimes.loadRuntimes();
  assert.deepEqual(offered(runtimes, "wren--e5f6"), ALL, "its read failed");
  assert.deepEqual(offered(runtimes, "quill--g7h8"), [], "a runtime this shell does not know");
});

test("each of the person's agents is read once a page, another's never", async () => {
  const { runtimes, asked } = world();
  const first = runtimes.loadRuntimes();
  await runtimes.loadRuntimes(); // its reads begun already: none again
  await first;
  await runtimes.loadRuntimes();
  assert.deepEqual(asked.filter((a) => a !== "wren--e5f6").sort(), ["clio--a1b2", "juniper--c3d4", "quill--g7h8", "rook--j9k2"]);
  assert.ok(!asked.includes("sams--m3n4"), "not theirs");
});

test("a control drawn before its agent's runtime is read is hidden once it is, and a line of them with it", async () => {
  const { runtimes } = world();
  const node = () => ({ dataset: {}, hidden: false });
  const clioOpen = runtimes.markOffer(node(), "clio--a1b2", "profile");
  const clioScreen = runtimes.markOffer(node(), "clio--a1b2", "screen");
  const juniperScreen = runtimes.markOffer(node(), "juniper--c3d4", "screen");
  const screens = { dataset: { offers: "" }, hidden: false, querySelectorAll: () => [clioScreen] };
  assert.equal(clioOpen.hidden, false, "drawn unread: as before");
  globalThis.document = { querySelectorAll: (sel) => (sel === "[data-offer]" ? [clioOpen, clioScreen, juniperScreen] : [screens]) };
  try {
    await runtimes.loadRuntimes();
  } finally {
    delete globalThis.document;
  }
  assert.deepEqual([clioOpen.hidden, clioScreen.hidden, juniperScreen.hidden, screens.hidden], [true, true, false, true]);
});

test("a section of them (Settings' Telegram) with none shown goes, and its link in Settings' nav with it", async () => {
  const { runtimes } = world();
  const node = () => ({ dataset: {}, hidden: false });
  const clio = runtimes.markOffer(node(), "clio--a1b2", "doors");
  const juniper = runtimes.markOffer(node(), "juniper--c3d4", "doors");
  const only = (rows, id) => ({ id, dataset: { offers: "" }, hidden: false, querySelectorAll: () => rows });
  const claudes = only([clio], "settings-telegram");
  const both = only([clio, juniper], "settings-both");
  const links = { "settings-telegram": { hidden: false }, "settings-both": { hidden: false } };
  globalThis.document = {
    querySelectorAll: (sel) => (sel === "[data-offer]" ? [clio, juniper] : [claudes, both]),
    querySelector: (sel) => links[sel.match(/data-section="([^"]+)"/)?.[1]] ?? null,
  };
  try {
    await runtimes.loadRuntimes();
  } finally {
    delete globalThis.document;
  }
  assert.deepEqual([clio.hidden, juniper.hidden], [true, false]);
  assert.deepEqual([claudes.hidden, links["settings-telegram"].hidden], [true, true], "a Claude agent's alone: the section and its link go");
  assert.deepEqual([both.hidden, links["settings-both"].hidden], [false, false], "a Hermes agent's shown: both stay");
});

test("Claude Code offers nothing beyond its chats; Hermes its profile, its screen, its own skills and a hosted door", () => {
  assert.deepEqual(RUNTIME_OFFERS, { hermes: ["profile", "screen", "skills", "doors"], claude: [] });
});
