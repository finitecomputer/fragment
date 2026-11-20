// cell/shell/session-archive.js: its archive's search results as the
// conversations History lists, by key, best first; and the chats History
// lists, the archive's rows and any chat of its index file no row lists.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
globalThis.document ??= { hidden: false, addEventListener() {} };
globalThis.addEventListener ??= () => {};
const { foundSessions, createSessionArchive } = await import("../shell/session-archive.js");

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

// The archive's list as History reads it from a stand-in host: `rows`, a
// page at a time (`ops/sessions`), and `index` (`index/chat.json`, null:
// none).
async function listed({ rows, index, fail = null }) {
  const asked = [];
  const api = async (method, path, body) => {
    asked.push(`${method} ${path.split("?")[0]}`);
    if (path.endsWith("/ops/sessions")) {
      if (fail === "rows") throw Object.assign(new Error("down"), { status: 503 });
      if (rows === null) throw Object.assign(new Error("no such op"), { status: 404 });
      const after = body.input.after;
      const page = rows.filter((e) => e.key > after).slice(0, 1);
      const next = rows.some((e) => e.key > (page.at(-1)?.key ?? "\uffff")) ? page.at(-1).key : null;
      return { result: { sessions: page, next } };
    }
    if (index === null) throw Object.assign(new Error("no file"), { status: 404 });
    return { sessions: index };
  };
  let changed = 0;
  const archive = createSessionArchive({
    api, ui: { renderHeading() {} }, data: { allFragments: () => [{ name: "sessions--a1", kind: "sessions", role: "owner" }], own: (f) => f.role === "owner", seg: (s) => s },
    selection: { getFrame: () => null }, navigation: { openChat() {}, leaveSidebar() {} }, continuing: { ask() {} }, store: {}, changed: () => changed++,
  });
  await archive.load(true);
  return { keys: archive.entries().map((e) => e.key), titles: archive.entries().map((e) => e.title), asked, changed };
}

test("its rows, every page, and a chat of its index file no row lists (an older CLI's)", async () => {
  const rows = [{ key: "hermes-a", kind: "chat", title: "A" }, { key: "hermes-b", kind: "chat", title: "B" }];
  const index = [{ key: "hermes-b", title: "B, as the index says" }, { key: "hermes-c", title: "C, from an older CLI" }];
  const r = await listed({ rows, index });
  assert.deepEqual(r.keys, ["hermes-a", "hermes-b", "hermes-c"]);
  assert.deepEqual(r.titles, ["A", "B", "C, from an older CLI"], "a row wins over the index's copy");
  assert.equal(r.asked.filter((a) => a.endsWith("/ops/sessions")).length, 2, "both pages");
});

test("an archive from before its rows: its index file; a host before the list: the same", async () => {
  const index = [{ key: "hermes-a", title: "A" }];
  assert.deepEqual((await listed({ rows: [], index })).keys, ["hermes-a"]);
  assert.deepEqual((await listed({ rows: null, index })).keys, ["hermes-a"]);
  assert.deepEqual((await listed({ rows: [], index: null })).keys, [], "none, and none");
});

test("a list that fails to read keeps what was read before: nothing changes", async () => {
  const r = await listed({ rows: [], index: [], fail: "rows" });
  assert.equal(r.changed, 0);
});
