// cell/shell/history-page.js's rows (Paul, 2026-10-31): every chat and
// imported conversation, the newest first; All, Archived and Imported;
// words held by a title (an import's first words too) or found by the
// person's search or their archive's, either standing when the other fails; the search's hint bounded as the API bounds it; and a
// row's time.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { answersOf, firstOf, historyRows, hintOf, when } from "../shell/history-page.js";

const chat = (name, at, extra = {}) => ({ name, title: name, at, archived: false, ...extra });
const entry = (key, ended_at, extra = {}) => ({ key, title: key, ended_at, preview: "", ...extra });
const keys = (rows) => rows.map((r) => r.key);
const chats = [chat("trip", 30), chat("taxes", 10, { archived: true }), chat("no-time", undefined)];
const entries = [entry("old-talk", "1970-01-01T00:00:00.020Z", { preview: "about the garden" }), entry("no-time", undefined)];

test("every chat and import, the newest first; one that says no time last, never the newest", () => {
  assert.deepEqual(keys(historyRows({ chats, entries })), ["chat:trip", "imported:old-talk", "chat:taxes", "chat:no-time", "imported:no-time"]);
});

test("a chat's first record: a history card's time and newest words, a message's first line, none nothing", () => {
  const card = { at: 7, body: { kind: "history", title: "t", summary: "the summary\nmore", messages: [{ role: "user", text: "first" }, { role: "assistant", text: "\n  the last words\nand more" }] } };
  assert.deepEqual(firstOf(card), { at: 7, line: "the last words" });
  assert.deepEqual(firstOf({ at: 8, body: { kind: "history", summary: "only a summary", messages: [] } }), { at: 8, line: "only a summary" });
  assert.deepEqual(firstOf({ at: 9, body: { text: "hello there\nsecond" } }), { at: 9, line: "hello there" });
  assert.deepEqual(firstOf({ at: 10, body: { kind: "stop", turn: "x" } }), { at: 10, line: "" });
  assert.equal(firstOf(undefined), null);
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

test("a search's two answers: each found once with its first snippet; one failing, the other's stand and the page says which", () => {
  const chats = { status: "fulfilled", value: { messages: [{ fragment: "trip", snippet: "newest" }, { fragment: "trip", snippet: "older" }, { fragment: "taxes", snippet: "receipts" }] } };
  const imported = { status: "fulfilled", value: [{ key: "old-talk", snippet: "the garden" }, { key: "old-talk", snippet: "worse" }] };
  const down = (message) => ({ status: "rejected", reason: Object.assign(new Error(message), { status: 503 }) });
  const both = answersOf(chats, imported);
  assert.deepEqual([...both.found], [["trip", "newest"], ["taxes", "receipts"]]);
  assert.deepEqual([...both.foundImported], [["old-talk", "the garden"]]);
  assert.equal(both.failed, "");
  const noArchive = answersOf(chats, down("the archive is asleep"));
  assert.deepEqual([...noArchive.found], [...both.found]);
  assert.equal(noArchive.foundImported.size, 0);
  assert.equal(noArchive.failed, "Imported chats were not searched: the archive is asleep");
  const noChats = answersOf(down("search is down"), imported);
  assert.equal(noChats.found.size, 0);
  assert.deepEqual([...noChats.foundImported], [...both.foundImported]);
  assert.equal(noChats.failed, "Messages were not searched: search is down");
  assert.equal(answersOf(down("search is down"), down("asleep")).failed, "Messages and imported chats were not searched: search is down");
  assert.equal(answersOf(chats, { status: "fulfilled", value: [] }).foundImported.size, 0);
});
