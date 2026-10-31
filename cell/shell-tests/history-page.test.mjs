// cell/shell/history-page.js's rows (Paul, 2026-10-31): every chat and
// imported conversation, the newest first; All, Archived and Imported;
// words held by a title (an import's first words too) or found by the
// person's search; the search's hint bounded as the API bounds it; and a
// row's time.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { historyRows, hintOf, when } from "../shell/history-page.js";

const chat = (name, at, extra = {}) => ({ name, title: name, at, archived: false, ...extra });
const entry = (key, ended_at, extra = {}) => ({ key, title: key, ended_at, preview: "", ...extra });
const keys = (rows) => rows.map((r) => r.key);
const chats = [chat("trip", 30), chat("taxes", 10, { archived: true }), chat("made-now", undefined)];
const entries = [entry("old-talk", "1970-01-01T00:00:00.020Z", { preview: "about the garden" }), entry("no-time", undefined)];

test("every chat and import, the newest first: a chat with no message yet first, an import with no time last", () => {
  assert.deepEqual(keys(historyRows({ chats, entries })), ["chat:made-now", "chat:trip", "imported:old-talk", "chat:taxes", "imported:no-time"]);
});

test("Archived lists the archived chats alone; Imported, the archive's conversations alone", () => {
  assert.deepEqual(keys(historyRows({ chats, entries, filter: "archived" })), ["chat:taxes"]);
  assert.deepEqual(keys(historyRows({ chats, entries, filter: "imported" })), ["imported:old-talk", "imported:no-time"]);
});

test("words: a title holding them all, an import's first words, or the search's finds, with its snippet", () => {
  assert.deepEqual(keys(historyRows({ chats, entries, q: "TR" })), ["chat:trip"]);
  assert.deepEqual(keys(historyRows({ chats, entries, q: "the garden" })), ["imported:old-talk"]);
  assert.deepEqual(keys(historyRows({ chats, entries, q: "trip garden" })), []);
  const found = historyRows({ chats, entries, q: "receipts", found: new Map([["taxes", "the receipts are in"]]) });
  assert.deepEqual(found.map((r) => [r.key, r.snippet]), [["chat:taxes", "the receipts are in"]]);
  const imported = historyRows({ chats, entries, q: "lunch", foundImported: new Map([["no-time", "lunch at noon"]]) });
  assert.deepEqual(keys(imported), ["imported:no-time"]);
  // a filter holds over what was found
  assert.deepEqual(keys(historyRows({ chats, entries, q: "receipts", filter: "imported", found: new Map([["taxes", "x"]]) })), []);
});

test("the search's hint: eight words and 256 bytes at most, never a character cut", () => {
  assert.equal(hintOf("  one two  three "), "one two three");
  assert.equal(hintOf("a b c d e f g h i j"), "a b c d e f g h");
  const long = "é".repeat(200);
  assert.ok(new TextEncoder().encode(hintOf(long)).length <= 256);
  assert.equal(hintOf(long), "é".repeat(128));
});

test("a row's time: the time today, the day this year, with its year before it", () => {
  const now = new Date(2026, 9, 31, 15, 0).getTime();
  assert.equal(when(undefined, now), "");
  assert.match(when(new Date(2026, 9, 31, 9, 5).getTime(), now), /9:05/);
  assert.doesNotMatch(when(new Date(2026, 9, 12).getTime(), now), /2026/);
  assert.match(when(new Date(2025, 9, 12).getTime(), now), /2025/);
});
