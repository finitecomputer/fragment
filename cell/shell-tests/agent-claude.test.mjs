// cell/shell/agent-settings.js's settings of a Claude agent (agent.json
// `runtime: "claude"`): its `claude.model` and `claude.effort`, each set
// with the other kept, none for Claude Code's default; and its writes add
// no `tier`, which is the platform's (Hermes') alone.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { claudeSet, claudeSettings, createAgentSettings, CLAUDE_MODELS, CLAUDE_EFFORTS, DEFAULT_AGENT_TIER } from "../shell/agent-settings.js";

test("a model set keeps its thinking; Claude's default takes the field away, and `claude` with its last", () => {
  const j = { runtime: "claude", claude: { effort: "high" } };
  assert.equal(claudeSet(j, "model", "haiku"), true);
  assert.deepEqual(j.claude, { effort: "high", model: "haiku" });
  assert.equal(claudeSet(j, "model", null), true);
  assert.deepEqual(j.claude, { effort: "high" });
  assert.equal(claudeSet(j, "effort", null), true);
  assert.deepEqual(j, { runtime: "claude" }, "no empty `claude` left");
});

test("the same again changes nothing, nor does a default where none is set", () => {
  const j = { runtime: "claude", claude: { model: "haiku" } };
  assert.equal(claudeSet(j, "model", "haiku"), false);
  assert.equal(claudeSet(j, "effort", null), false);
  assert.deepEqual(j, { runtime: "claude", claude: { model: "haiku" } });
});

test("a `claude` that is no object is read as none and written over", () => {
  for (const bad of ["haiku", ["haiku"], 3, null]) {
    const j = { runtime: "claude", claude: bad };
    assert.deepEqual(claudeSettings(j), {}, JSON.stringify(bad));
    assert.equal(claudeSet(j, "model", "haiku"), true);
    assert.deepEqual(j.claude, { model: "haiku" }, JSON.stringify(bad));
  }
});

test("the choices are Claude Code's aliases and levels, its default first", () => {
  assert.deepEqual(CLAUDE_MODELS.map(([v]) => v), ["", "fable", "opus", "sonnet", "haiku"]);
  assert.deepEqual(CLAUDE_EFFORTS.map(([v]) => v), ["", "low", "medium", "high", "xhigh", "max"]);
});

// agent.json as the platform keeps it, written through `writeAgentJson`
function settingsOver(held) {
  const files = new Map(Object.entries(held));
  const api = async (method, path, body) => {
    const m = path.match(/^\/api\/f\/([^/]+)\/(file\?path=agent\.json|files|deploy)$/);
    assert.ok(m, `${method} ${path}`);
    if (method === "GET") {
      if (!files.has(m[1])) throw Object.assign(new Error("not found"), { status: 404 });
      return structuredClone(files.get(m[1]));
    }
    if (m[2] === "files") files.set(m[1], JSON.parse(body.files[0].text));
    return {};
  };
  const settings = createAgentSettings({
    api, el: () => ({}), seg: (s) => encodeURIComponent(s), byName: () => null, agentFace: () => ({}), titleOf: (n) => n, named() {}, rename() {},
    agents: () => new Map(), load: async () => {}, nameScreen() {}, renderNames() {}, computer: () => null, homePicker: () => [], addAgent: async () => {},
    primaryAgent: () => null, chats: () => [], own: () => true, me: () => null,
  });
  return { settings, files };
}

test("a Claude agent's write adds no tier; a Hermes agent's keeps its own, or the default", async () => {
  const { settings, files } = settingsOver({ clio: { runtime: "claude" }, juniper: {} });
  await settings.writeAgentJson("clio", (next) => (claudeSet(next, "effort", "low") ? "its thinking: Low" : null));
  assert.deepEqual(files.get("clio"), { runtime: "claude", claude: { effort: "low" } });
  await settings.writeAgentJson("juniper", (next) => ((next.home = "garden"), "its home"));
  assert.deepEqual(files.get("juniper"), { tier: DEFAULT_AGENT_TIER, home: "garden" });
});
