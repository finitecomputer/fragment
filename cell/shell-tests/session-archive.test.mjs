// cell/shell/session-archive.js's search: its archive's search results as
// the conversations History lists, by key, best first.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { foundSessions } from "../shell/session-archive.js";

test("each conversation once, best first, its key from its file and its best snippet", () => {
  const results = [
    { rank: 1, source: "sessions/hermes-beans.json", section: "t2", snippet: "in spring" },
    { rank: 2, source: "sessions/finitechat-trip.json", section: "t1", snippet: "flights" },
    { rank: 3, source: "sessions/hermes-beans.json", section: "t1", snippet: "plant beans?" },
  ];
  assert.deepEqual(foundSessions(results), [{ key: "hermes-beans", snippet: "in spring" }, { key: "finitechat-trip", snippet: "flights" }]);
});

test("a result of any other source is left out; none, nothing", () => {
  const results = [
    { source: "index/chat.json", snippet: "x" },
    { source: "sessions/a/b.json", snippet: "x" },
    { source: "sessions/.json", snippet: "x" },
    { source: "media/abc.png", snippet: "x" },
    { snippet: "no source" },
    { source: "sessions/kept.json" },
  ];
  assert.deepEqual(foundSessions(results), [{ key: "kept", snippet: "" }]);
  assert.deepEqual(foundSessions(undefined), []);
  assert.deepEqual(foundSessions([]), []);
});
