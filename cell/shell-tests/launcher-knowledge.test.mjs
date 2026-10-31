// cell/shell/launcher.js's Knowledge (FIN-238, decision 6: the launcher
// searches across brains): what its question asks, as `fragment search
// --all` does, and where a hit opens.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { knowledgeIn, knownAt } from "../shell/launcher.js";

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
