// Who presents a deck (templates/slides/live/slides-present.js), driven as
// pages see it: the `present` channel's records in seq order, and presence.
// Valid: one presenter, followed; the latest Present wins, and the page
// that pressed it earlier stops; one presenter's posts, answered late,
// reach the channel in the order of its actions (#1057 review), page
// changes and a stop then a restart alike, so a follower ends where the
// presenter is. Invalid: a page following itself; a presenter no longer
// here, followed or stopping a new one. Not a goal: two pressing Present at
// the same instant (either presses it again).
import { test } from "node:test";
import assert from "node:assert/strict";

const { Outbox, Present } = await import(new URL("../../../templates/slides/live/slides-present.js", import.meta.url).href);

const live = (seq, from, page = 1) => ({ seq, principal: `npub-${from}`, body: { page, live: true, from } });
const here = (...presenting) => [{ id: "a" }, { id: "b" }, { id: "v" }].map((p) => ({ ...p, data: presenting.includes(p.id) ? { presenting: true } : {} }));

test("one presenter: every other page follows its newest page", () => {
  const v = new Present();
  v.record(live(1, "a", 1));
  v.record(live(2, "a", 3));
  assert.deepEqual(v.presenter(here("a"), "v"), { from: "a", page: 3, principal: "npub-a", seq: 2 });
  assert.equal(v.presenter(here("a"), "a"), null, "never its own presenter");
  assert.equal(v.presenter(here(), "v"), null, "a presenter whose page is gone is no one's");
});

test("the latest Present wins: the earlier presenter stops, and viewers follow the later", () => {
  const [a, b, v] = [new Present(), new Present(), new Present()];
  b.start();
  for (const p of [a, b, v]) p.record(live(1, "b", 2));
  a.start(); // pressed after B's record
  for (const p of [a, b, v]) p.record(live(2, "a", 5));
  assert.equal(a.yields(here("a", "b"), "a"), false, "B's record is older than A's start");
  assert.equal(b.yields(here("a", "b"), "b"), true, "A's is newer than B's own");
  b.stop();
  assert.equal(v.presenter(here("a"), "v").from, "a");
  assert.equal(v.presenter(here("a"), "v").page, 5);
});

test("a presenter no longer here stops no one, even with the newest record", () => {
  const a = new Present();
  a.record(live(7, "b", 4)); // B's tab closed without Stop
  a.start();
  assert.equal(a.yields(here("a"), "a"), false);
});

// A channel and a page's transport: each post is held until the test lets
// it through, and is appended (gets its seq) only then, as a slow request
// or one retried after a dropped answer is.
function channel() {
  const records = [];
  const held = [];
  const send = (from) => (state) =>
    new Promise((resolve) => held.push(() => {
      records.push({ seq: records.length + 1, principal: `npub-${from}`, body: { ...state, from } });
      resolve();
    }));
  // lets through the post at `i` of those held now (0 the oldest)
  const release = async (i = 0) => {
    const [go] = held.splice(i, 1);
    go();
    await new Promise((r) => setImmediate(r));
  };
  return { records, held, send, release };
}

test("one presenter's posts reach the channel in the order of its actions, however late each is answered", async () => {
  const ch = channel();
  const out = new Outbox(ch.send("a"));
  out.put({ page: 1, live: true });
  out.put({ page: 2, live: true });
  out.put({ page: 3, live: true });
  assert.equal(ch.held.length, 1, "one post in flight; the rest wait");
  await ch.release();
  assert.equal(ch.held.length, 1, "the waiting page changes went as one: the latest");
  await ch.release();
  assert.deepEqual(ch.records.map((r) => r.body.page), [1, 3], "page 2 was passed by page 3, never posted after it");
  const v = new Present();
  for (const r of ch.records) v.record(r);
  assert.equal(v.presenter(here("a"), "v").page, 3, "a follower ends where the presenter is");
});

test("a stop then a restart are each posted, in order, and the restarted presenter is followed", async () => {
  const ch = channel();
  const a = new Present();
  const out = new Outbox(ch.send("a"));
  a.start();
  out.put({ page: 3, live: true });
  await ch.release();
  // stop, then present again at once, while the stop is still in flight
  a.stop();
  out.put({ page: 3, live: false });
  a.start();
  out.put({ page: 3, live: true });
  out.put({ page: 4, live: true });
  assert.equal(ch.held.length, 1, "the stop is in flight; the restart waits behind it");
  await ch.release();
  await ch.release();
  assert.deepEqual(ch.records.map((r) => [r.body.live, r.body.page]), [[true, 3], [false, 3], [true, 4]], "start, stop, restart: in that order, the restart's page its latest");
  const v = new Present();
  for (const r of ch.records) {
    a.record(r);
    v.record(r);
  }
  assert.deepEqual(v.presenter(here("a"), "v"), { from: "a", page: 4, principal: "npub-a", seq: 3 }, "the follower follows the restarted presenter");
  assert.equal(a.yields(here("a"), "a"), false);
});

test("a post that fails does not stop the next", async () => {
  const sent = [];
  const out = new Outbox(async (state) => {
    sent.push(state.page);
    if (state.page === 1) throw new Error("dropped");
  });
  out.put({ page: 1, live: true });
  out.put({ page: 2, live: true });
  await new Promise((r) => setImmediate(r));
  assert.deepEqual(sent, [1, 2]);
});
