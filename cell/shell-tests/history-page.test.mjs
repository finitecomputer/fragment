// cell/shell/history-page.js's rows (Paul, 2026-10-31): every chat and
// imported conversation, the newest first; All, Archived and Imported;
// words held by a title (an import's first words too) or found by the
// person's search or their archive's, either standing when the other fails; the search's hint bounded as the API bounds it; a
// row's time; and the page's search following the person's list (FIN-461).
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { answersOf, createHistoryPage, firstOf, historyRows, hintOf, when } from "../shell/history-page.js";

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

// The page itself (FIN-461), its DOM faked as far as History draws: the
// person's search follows their list. Words typed before the message that
// holds them comes find it once their list is read again; the archive's
// search is not asked again for it; an older answer that comes after a
// newer one's is not shown; and a list read while typing has not rested
// asks nothing of its own.
class Node {
  constructor(tag, cls, text) {
    Object.assign(this, { tag, className: cls ?? "", textContent: text ?? "", children: [], dataset: {}, hidden: false, parentNode: null });
  }
  append(...nodes) {
    for (const n of nodes) this.insertBefore(n, null);
  }
  replaceChildren(...nodes) {
    for (const n of [...this.children]) n.remove();
    this.append(...nodes);
  }
  insertBefore(n, ref) {
    n.remove();
    const at = ref ? this.children.indexOf(ref) : -1;
    this.children.splice(at < 0 ? this.children.length : at, 0, n);
    n.parentNode = this;
  }
  remove() {
    this.parentNode?.children.splice(this.parentNode.children.indexOf(this), 1);
    this.parentNode = null;
  }
  get firstElementChild() {
    return this.children[0] ?? null;
  }
  get lastElementChild() {
    return this.children.at(-1) ?? null;
  }
  isEqualNode(o) {
    return !!o && drawn(this) === drawn(o);
  }
  setAttribute(k, v) {
    this[k] = v;
  }
  getAttribute(k) {
    return this[k];
  }
  focus() {}
}
const drawn = (n) => `${n.className}:${n.textContent}[${n.children.map(drawn).join()}]`;
const walk = (n) => [n, ...n.children.flatMap(walk)];
const { setTimeout: realSetTimeout } = globalThis;
const settle = () => new Promise((r) => realSetTimeout(r, 0));

test("words typed before the message that holds them comes find it once the person's list is read again", async () => {
  const timers = new Map();
  let nextTimer = 1;
  const { setTimeout: was, clearTimeout: wasClear } = globalThis;
  globalThis.setTimeout = (fn) => (timers.set(nextTimer, fn), nextTimer++);
  globalThis.clearTimeout = (id) => timers.delete(id);
  const rest = () => {
    for (const [id, fn] of [...timers]) (timers.delete(id), fn());
  };
  try {
    const calls = [];
    const archiveAsked = [];
    let chats = [{ name: "bread--a1", previewAt: 5, preview: "When do I bake?" }];
    const h = {
      api: (method, path) => new Promise((ok) => calls.push({ method, path, ok })),
      el: (tag, cls, text) => new Node(tag, cls, text),
      icon: (name) => new Node("svg", name),
      seg: (s) => s,
      mark: () => new Node("span", "mark"),
      chats: () => chats,
      isThread: () => false,
      own: () => true,
      chatTitle: () => "Bread notes",
      agentsOf: () => [],
      agents: () => new Map(),
      titleOf: () => "",
      archive: { name: () => null, entries: () => [], openSession() {}, search: async (q) => (archiveAsked.push(q), []) },
      openChat() {},
      leaveSidebar() {},
    };
    const root = new Node("div");
    const history = createHistoryPage(root, h);
    history.open();
    const input = walk(root).find((n) => n.tag === "input");
    const type = (words) => ((input.value = words), input.oninput());
    const found = () => walk(root).filter((n) => n.className === "history-preview found").map((n) => n.textContent);
    const empty = () => walk(root).find((n) => n.className === "muted history-empty")?.textContent;
    const searches = () => calls.filter((c) => c.path.startsWith("/api/search?")).map((c) => c.path);

    // asked once typing rests; nothing holds the words yet
    type("warmer");
    rest();
    assert.deepEqual(searches(), ["/api/search?q=warmer"]);
    calls[0].ok({ fragments: [], messages: [] });
    await settle();
    assert.equal(empty(), "Nothing found for “warmer”");
    // the message comes: the list read again asks the person's search again,
    // and not the archive's
    chats = [{ name: "bread--a1", previewAt: 9, preview: "the rye wants a warmer spot" }];
    history.listed();
    assert.equal(calls.length, 2);
    assert.deepEqual(archiveAsked, ["warmer"]);
    calls[1].ok({ fragments: [], messages: [{ fragment: "bread--a1", snippet: "the rye wants a warmer spot" }] });
    await settle();
    assert.deepEqual(found(), ["the rye wants a warmer spot"]);
    // two reads out at once: the older answer, come last, is not shown
    history.listed();
    history.listed();
    calls[3].ok({ fragments: [], messages: [{ fragment: "bread--a1", snippet: "the rye wants a warmer spot" }] });
    calls[2].ok({ fragments: [], messages: [] });
    await settle();
    assert.deepEqual(found(), ["the rye wants a warmer spot"]);
    // typing not yet rested: a list read asks nothing; once it rests, both are asked
    type("warmer spot");
    history.listed();
    assert.equal(calls.length, 4);
    rest();
    assert.deepEqual(searches().slice(4), ["/api/search?q=warmer%20spot"]);
    assert.deepEqual(archiveAsked, ["warmer", "warmer spot"]);
    // closed, a list read asks nothing; opened again, it is asked again
    history.close();
    history.listed();
    assert.equal(calls.length, 5);
    history.open();
    assert.deepEqual(searches().slice(5), ["/api/search?q=warmer%20spot"]);
  } finally {
    Object.assign(globalThis, { setTimeout: was, clearTimeout: wasClear });
  }
});
