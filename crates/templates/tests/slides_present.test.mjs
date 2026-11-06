// Who presents a deck (templates/slides/live/slides-present.js), driven as
// pages see it: the `present` channel's records in seq order, and presence.
// Valid: one presenter, followed; a newer start takes over and the older
// gives way. Crossed starts (#1057 review): two editors start before either
// hears the other, in every order the two records can reach each of them;
// exactly the newer claim presents, and every page, the two and a viewer's,
// follows it. One presenter's own posts (#1057 review): delayed and
// answered late, they still reach the channel in the order of its actions,
// page changes and a stop then a restart alike, so a follower ends where
// the presenter is and the restarted presenter is followed. Invalid: a
// page yielding to a claim before it knows its own, a page following
// itself, a presenter no longer here, a page change dropped across a stop.
import { test } from "node:test";
import assert from "node:assert/strict";

const { Outbox, Present } = await import(new URL("../../../templates/slides/live/slides-present.js", import.meta.url).href);

const live = (seq, from, page = 1) => ({ seq, principal: `npub-${from}`, body: { page, live: true, from } });
const ended = (seq, from) => ({ seq, principal: `npub-${from}`, body: { page: 1, live: false, from } });
const here = (...presenting) => [{ id: "a" }, { id: "b" }, { id: "v" }].map((p) => ({ ...p, data: presenting.includes(p.id) ? { presenting: true } : {} }));

test("one presenter: every other page follows its newest page", () => {
  const a = new Present();
  const v = new Present();
  a.start();
  for (const p of [a, v]) {
    p.record(live(1, "a", 1), p === a ? "a" : "v");
    p.record(live(2, "a", 3), p === a ? "a" : "v");
  }
  assert.equal(a.mine, 1, "its claim, from its own first record");
  assert.equal(a.yields(here("a"), "a"), false);
  assert.equal(a.presenter(here("a"), "a"), null, "never its own presenter");
  assert.deepEqual(v.presenter(here("a"), "v"), { from: "a", page: 3, principal: "npub-a", claim: 1 });
  assert.equal(v.presenter(here(), "v"), null, "a presenter whose page is gone is no one's");
});

test("a newer start takes over, and the older gives way", () => {
  const a = new Present();
  a.start();
  a.record(live(1, "a"), "a");
  a.record(live(2, "b", 2), "a");
  assert.equal(a.yields(here("a", "b"), "a"), true);
  a.stop();
  a.record(ended(3, "a"), "a");
  assert.equal(a.presenter(here("b"), "a").from, "b");
  // a new start here is a new claim, newer than b's
  a.start();
  a.record(live(4, "a"), "a");
  assert.equal(a.mine, 4);
  assert.equal(a.yields(here("a", "b"), "a"), false);
});

test("crossed starts: the newer claim presents, in every order, and the older gives way", () => {
  // each page sees the records in seq order (A's claim 5, B's 6); what
  // differs is how the two pages' handling interleaves
  const interleavings = [["a", "a", "b", "b"], ["a", "b", "a", "b"], ["a", "b", "b", "a"], ["b", "a", "a", "b"], ["b", "a", "b", "a"], ["b", "b", "a", "a"]];
  for (const order of interleavings) {
    const pages = { a: new Present(), b: new Present() };
    const seen = { a: 0, b: 0 };
    const presence = new Set(["a", "b"]);
    const list = () => here(...presence);
    pages.a.start();
    pages.b.start();
    const yielded = [];
    for (const who of order) {
      const p = pages[who];
      p.record([live(5, "a"), live(6, "b", 2)][seen[who]++], who);
      if (p.yields(list(), who)) {
        p.stop();
        presence.delete(who);
        yielded.push(who);
      }
    }
    assert.deepEqual(yielded, ["a"], `only the older claim gives way: ${order}`);
    assert.equal(pages.b.presenting, true, `the newer presents: ${order}`);
    assert.equal(pages.a.presenter(list(), "a")?.from, "b", `the older follows the newer: ${order}`);
    const viewer = new Present();
    viewer.record(live(5, "a"), "v");
    viewer.record(live(6, "b", 2), "v");
    assert.equal(viewer.presenter(list(), "v")?.from, "b", `a viewer follows the newer: ${order}`);
  }
});

test("a page that has started yields to nothing until it knows its own claim", () => {
  const b = new Present();
  b.start();
  b.record(live(5, "a"), "b");
  assert.equal(b.yields(here("a", "b"), "b"), false, "its own claim may be the newer");
  b.record(live(6, "b"), "b");
  assert.equal(b.yields(here("a", "b"), "b"), false, "and it is");
  const a = new Present();
  a.start();
  a.record(live(5, "a"), "a");
  a.record(live(6, "b"), "a");
  assert.equal(a.yields(here("a", "b"), "a"), true, "its own record told it its claim, the older");
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
  for (const r of ch.records) v.record(r, "v");
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
    a.record(r, "a");
    v.record(r, "v");
  }
  assert.equal(a.mine, 3, "the restart's claim is its record's seq, as every page reckons it");
  assert.deepEqual(v.presenter(here("a"), "v"), { from: "a", page: 4, principal: "npub-a", claim: 3 }, "the follower follows the restarted presenter");
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
