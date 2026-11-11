// cell/shell/schedules-page.js (Paul, 2026-11-10): every scheduled job of
// the person's own fragments in one list. A row's name (a routine's first
// line, an app cron's job, another record's channel), the soonest first and
// the paused last; the fragments asked; and the page itself: a few read at
// once, a failed one a quiet line while the rest stand, an earlier visit's
// answers dropped, and an empty list one line.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { asked, createSchedulesPage, nextSaid, READS_AT_ONCE, scheduleName, scheduleRows } from "../shell/schedules-page.js";

const routine = (id, text, nextAt, extra = {}) => ({ id, cron: "0 9 * * *", said: "every day at 09:00 UTC", channel: "tasks", record: { kind: "routine", text, chat: "c" }, paused: false, nextAt, ...extra });
const cron = (run, nextAt, extra = {}) => ({ cron: "*/15 * * * *", run, said: "every 15 minutes", paused: false, nextAt, ...extra });

test("a row's name: a routine's first line, cut short; any other record's channel", () => {
  assert.equal(scheduleName(routine(1, "\n  water the plants\nand the beans")), "water the plants");
  assert.equal(scheduleName(routine(1, "x".repeat(200))).length, 80);
  assert.match(scheduleName(routine(1, "x".repeat(200))), /…$/);
  assert.equal(scheduleName(routine(1, "   ")), "tasks", "a routine saying nothing is named by its channel");
  assert.equal(scheduleName({ id: 2, channel: "pings", record: { ping: true } }), "pings");
  assert.equal(scheduleName({ id: 3, channel: "talk", record: "hello" }), "talk");
});

test("every job of every fragment, the soonest first, the paused last; a failed fragment named, and the rest stand", () => {
  const answers = new Map([
    ["maple--a1", { schedules: [routine(1, "water the plants", 300), routine(2, "call mum", 100, { paused: true, nextAt: undefined })], triggers: [] }],
    ["garden--b2", { schedules: [{ id: 7, cron: "0 0 1 1 *", said: "every year on Jan 1 at 00:00 UTC", channel: "pings", record: {}, paused: false, nextAt: 900 }], triggers: [cron("tick", 200), { channel: "inbox", run: "ingest", paused: false }, cron("sweep", undefined, { paused: true })] }],
    ["notes--c3", { failed: true }],
  ]);
  const { rows, failed } = scheduleRows(answers);
  assert.deepEqual(
    rows.map((r) => [r.name, r.fragment, r.said, r.nextAt, r.paused]),
    [
      ["tick", "garden--b2", "every 15 minutes", 200, false],
      ["water the plants", "maple--a1", "every day at 09:00 UTC", 300, false],
      ["pings", "garden--b2", "every year on Jan 1 at 00:00 UTC", 900, false],
      ["sweep", "garden--b2", "every 15 minutes", null, true],
      ["call mum", "maple--a1", "every day at 09:00 UTC", null, true],
    ],
  );
  assert.deepEqual(failed, ["notes--c3"]);
  // a cron trigger the platform says no words for says its cron
  assert.equal(scheduleRows(new Map([["a", { schedules: [], triggers: [{ cron: "1 2 3 4 5", run: "x", paused: false, nextAt: 1 }] }]])).rows[0].said, "1 2 3 4 5");
  assert.deepEqual(scheduleRows(new Map()), { rows: [], failed: [] });
});

test("what is asked: the person's own fragments, but chats, skills and session archives", () => {
  const f = (kind, role = "owner") => ({ name: kind, kind, role });
  assert.deepEqual([f("app"), f("agent"), f("brain"), f("other"), f("chat"), f("skills"), f("sessions"), f("app", "editor"), f("agent", "viewer")].filter(asked).map((x) => x.kind), ["app", "agent", "brain", "other"]);
});

test("a next run in the person's own time: today's its time, another day's its day", () => {
  const now = new Date(2026, 10, 10, 8, 0).getTime();
  assert.match(nextSaid(new Date(2026, 10, 10, 9, 0).getTime(), now), /^Today /);
  const later = nextSaid(new Date(2026, 10, 12, 9, 0).getTime(), now);
  assert.doesNotMatch(later, /Today|2026/);
  assert.match(nextSaid(new Date(2027, 0, 1, 0, 0).getTime(), now), /2027/);
});

// The page itself, its DOM faked as far as it draws.
class Node {
  constructor(tag, cls, text) {
    Object.assign(this, { tag, className: cls ?? "", textContent: text ?? "", children: [], dataset: {}, parentNode: null });
  }
  append(...nodes) {
    for (const n of nodes) {
      n.parentNode = this;
      this.children.push(n);
    }
  }
  replaceChildren(...nodes) {
    this.children = [];
    this.append(...nodes);
  }
}
const walk = (n) => [n, ...n.children.flatMap(walk)];
const settle = () => new Promise((r) => setTimeout(r, 0));

function page(fragments) {
  const calls = [];
  const opened = [];
  const h = {
    api: (method, path) => new Promise((ok, fail) => calls.push({ method, path, ok, fail })),
    el: (tag, cls, text) => new Node(tag, cls, text),
    seg: (s) => s,
    art: () => new Node("span", "app-icon"),
    named: (node, name, make = (t) => t) => ((node.textContent = make(`Title of ${name}`)), node),
    fragments: () => fragments,
    openApp: (name) => opened.push(name),
    leaveSidebar() {},
  };
  const root = new Node("section");
  const p = createSchedulesPage(root, h);
  const rows = () => walk(root).filter((n) => n.className === "schedules-row").map((r) => walk(r).filter((n) => n.className === "label" || n.className === "schedules-fragment" || n.className === "schedules-chip").map((n) => n.textContent));
  const quiet = () => walk(root).filter((n) => n.className === "muted").map((n) => n.textContent);
  const empty = () => walk(root).find((n) => n.className === "muted schedules-empty")?.textContent;
  // answers every read out for `name`
  const answer = (name, schedules, triggers = []) => {
    for (const c of calls.filter((c) => c.path.startsWith(`/api/f/${name}/`) && !c.done)) {
      c.done = true;
      c.ok(c.path.endsWith("/schedules") ? { schedules } : { triggers, paused: [] });
    }
  };
  const fail = (name) => {
    for (const c of calls.filter((c) => c.path.startsWith(`/api/f/${name}/`) && !c.done)) {
      c.done = true;
      c.fail(new Error("asleep"));
    }
  };
  return { p, root, calls, opened, rows, quiet, empty, answer, fail };
}

test("the page: a few fragments read at once, each drawn as it answers; a failed one a quiet line; an earlier visit's answers dropped", async () => {
  const own = (name, kind = "app") => ({ name, kind, role: "owner" });
  const fragments = [own("maple", "agent"), own("garden"), own("notes"), own("todo"), own("brain", "brain"), own("chat-1", "chat"), { name: "theirs", kind: "app", role: "editor" }];
  const t = page(fragments);
  t.p.open();
  // READS_AT_ONCE fragments asked, two reads each; chats and others' not at all
  assert.equal(READS_AT_ONCE, 4);
  assert.deepEqual(t.calls.map((c) => c.path), ["maple", "garden", "notes", "todo"].flatMap((n) => [`/api/f/${n}/schedules`, `/api/f/${n}/triggers`]));
  assert.equal(t.empty(), undefined, "nothing said while reads are out");
  t.answer("garden", [], [cron("tick", 200)]);
  await settle();
  assert.deepEqual(t.rows(), [["tick", "Title of garden"]]);
  // the next is asked as one answers
  assert.ok(t.calls.some((c) => c.path === "/api/f/brain/schedules"));
  t.answer("maple", [routine(1, "water the plants", 100), routine(2, "call mum", undefined, { paused: true })]);
  t.fail("notes");
  t.answer("todo", []);
  t.answer("brain", []);
  await settle();
  assert.deepEqual(t.rows(), [["water the plants", "Title of maple"], ["tick", "Title of garden"], ["call mum", "Paused", "Title of maple"]]);
  assert.deepEqual(t.quiet(), ["Title of notes was not read"]);
  assert.equal(t.empty(), undefined);
  // a row opens its fragment
  walk(t.root).find((n) => n.className === "schedules-row").onclick();
  assert.deepEqual(t.opened, ["maple"]);

  // opened again: read again; the first visit's late answers change nothing
  const again = page([own("slow")]);
  again.p.open();
  again.p.close();
  again.p.open();
  assert.equal(again.calls.length, 4);
  again.calls[0].done = again.calls[1].done = true;
  again.calls[0].ok({ schedules: [routine(9, "stale", 1)] });
  again.calls[1].ok({ triggers: [] });
  await settle();
  assert.deepEqual(again.rows(), []);
  again.answer("slow", []);
  await settle();
  assert.equal(again.empty(), "No schedules");
});

test("a fragment's one read failing holds its place until the other answers: never more than READS_AT_ONCE out", async () => {
  const own = (name) => ({ name, kind: "app", role: "owner" });
  const t = page(["a", "b", "c", "d", "e"].map(own));
  t.p.open();
  const schedules = t.calls.find((c) => c.path === "/api/f/a/schedules");
  schedules.done = true;
  schedules.fail(new Error("asleep"));
  await settle();
  assert.ok(!t.calls.some((c) => c.path.startsWith("/api/f/e/")), "e waits for a's triggers");
  t.answer("a", [], []);
  await settle();
  assert.ok(t.calls.some((c) => c.path === "/api/f/e/schedules"));
  assert.deepEqual(t.quiet(), ["Title of a was not read"]);
});

test("no fragment to ask: one line, at once", () => {
  const t = page([{ name: "c", kind: "chat", role: "owner" }]);
  t.p.open();
  assert.equal(t.calls.length, 0);
  assert.equal(t.empty(), "No schedules");
});
