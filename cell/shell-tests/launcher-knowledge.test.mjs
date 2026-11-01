// cell/shell/launcher.js's Knowledge (FIN-238, decision 6: the launcher
// searches across brains): what its question asks, as `fragment search
// --all` does, and where a hit opens.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { knowledgeIn, knownAt, knownOf } from "../shell/launcher.js";

const row = (name, kind, archived = false) => ({ name, kind, ...(archived ? { archived } : {}) });

test("the question asks their brains and Session Archive, in their list's order, never an archived one nor anything else", () => {
  const list = [row("todo--a1b2", "app"), row("product--c3d4", "brain"), row("talk--e5f6", "chat"), row("old--g7h8", "brain", true), row("archive--j2k3", "sessions"), row("me--m4n5", "agent"), row("sales--p6q7", "brain")];
  assert.deepEqual(knowledgeIn(list), ["product--c3d4", "archive--j2k3", "sales--p6q7"]);
  assert.deepEqual(knowledgeIn([row("todo--a1b2", "app")]), []);
});

test("at most 32 of them (proto's search::IN_MAX), the first ones", () => {
  const list = Array.from({ length: 40 }, (_, i) => row(`b${String(i).padStart(2, "0")}--k3x9`, "brain"));
  const asked = knowledgeIn(list);
  assert.equal(asked.length, 32);
  assert.equal(asked[0], "b00--k3x9");
  assert.equal(asked.at(-1), "b31--k3x9");
});

test("a brain's page opens in the desktop at that page; an archive's conversation in its reader", () => {
  assert.deepEqual(knownAt({ source: "wiki/sso/requirements.md" }, "brain"), { app: "/#/wiki%2Fsso%2Frequirements.md" });
  assert.deepEqual(knownAt({ source: "notes/a b&c.md" }, "brain"), { app: "/#/notes%2Fa%20b%26c.md" });
  assert.deepEqual(knownAt({ source: "sessions/hermes-20260930_101500_rb12cd.json" }, "sessions"), { chat: "/?s=hermes-20260930_101500_rb12cd" });
  // a source of the archive that is no conversation: its list
  assert.deepEqual(knownAt({ source: "index/chat.json" }, "sessions"), { chat: "/" });
});

// Alex on #885: the answer comes (200) though a fragment of it failed; that
// is said, never taken for nothing found.
const asked = (name, state, message) => ({ name, state, ...(message ? { message } : {}) });
const hit = (fragment) => ({ fragment, source: "wiki/a.md", title: "A", snippet: "a" });

test("every fragment failed: no hits, each named as not searched", () => {
  const k = knownOf({ results: [], fragments: [asked("product--c3d4", "failed", "the index did not answer"), asked("sales--p6q7", "failed", "refused")], semantic: "on" });
  assert.deepEqual(k, { hits: [], unsearched: [{ name: "product--c3d4", message: "the index did not answer" }, { name: "sales--p6q7", message: "refused" }], unindexed: [] });
});

test("some failed, some answered: the hits, and the failed ones named beside them", () => {
  const k = knownOf({ results: [hit("product--c3d4")], fragments: [asked("product--c3d4", "searched"), asked("sales--p6q7", "failed")], semantic: "on" });
  assert.deepEqual(k.hits.map((h) => h.fragment), ["product--c3d4"]);
  assert.deepEqual(k.unsearched, [{ name: "sales--p6q7", message: "" }]);
});

test("all searched, nothing found, or no index yet: no failure to say, the not indexed named apart (and a fragment gone since is none either)", () => {
  assert.deepEqual(knownOf({ results: [], fragments: [asked("product--c3d4", "searched"), asked("archive--j2k3", "no_index"), asked("gone--x2y3", "missing"), asked("theirs--z4a5", "denied")], semantic: "on" }), { hits: [], unsearched: [], unindexed: ["archive--j2k3"] });
  assert.deepEqual(knownOf({}), { hits: [], unsearched: [], unindexed: [] });
});

// The Session Archive's rehearsal: one conversation showed three times, once a turn.
test("a page's matches are one row: its best (the first the merge ranked) with how many more, five pages shown", () => {
  const at = (fragment, source, snippet) => ({ fragment, source, title: source, snippet });
  const results = [
    at("archive--j2k3", "sessions/garden.json", "best turn"),
    at("product--c3d4", "wiki/a.md", "a"),
    at("archive--j2k3", "sessions/garden.json", "second turn"),
    at("product--c3d4", "wiki/b.md", "b"),
    at("archive--j2k3", "sessions/garden.json", "third turn"),
    // the same path in another fragment is another page
    at("sales--p6q7", "wiki/a.md", "sales a"),
    at("product--c3d4", "wiki/c.md", "c"),
    at("product--c3d4", "wiki/d.md", "d"),
  ];
  const k = knownOf({ results, fragments: [] });
  assert.deepEqual(k.hits.map((h) => [h.fragment, h.source, h.snippet, h.more]), [
    ["archive--j2k3", "sessions/garden.json", "best turn", 2],
    ["product--c3d4", "wiki/a.md", "a", 0],
    ["product--c3d4", "wiki/b.md", "b", 0],
    ["sales--p6q7", "wiki/a.md", "sales a", 0],
    ["product--c3d4", "wiki/c.md", "c", 0],
  ]);
  assert.equal(knownOf({ results }, 2).hits.length, 2);
});
