// Who presents a deck (templates/slides/live/slides-present.js), driven as
// pages see it: the `present` channel's records in seq order, each page's
// own post answered before or after the records reach it, and presence.
// Valid: one presenter, followed; a newer start takes over and the older
// gives way. Crossed starts (#1057 review): two editors start before either
// hears the other, in every order their answers and the records can
// arrive; exactly the newer claim presents, and every page, the two and a
// viewer's, follows it. Invalid: a page yielding to a claim before it knows
// its own claim's seq, a page following itself, a presenter no longer here.
import { test } from "node:test";
import assert from "node:assert/strict";

const { Present } = await import(new URL("../../../templates/slides/live/slides-present.js", import.meta.url).href);

const live = (seq, from, page = 1) => ({ seq, principal: `npub-${from}`, body: { page, live: true, from } });
const ended = (seq, from) => ({ seq, principal: `npub-${from}`, body: { page: 1, live: false, from } });
const here = (...presenting) => [{ id: "a" }, { id: "b" }, { id: "v" }].map((p) => ({ ...p, data: presenting.includes(p.id) ? { presenting: true } : {} }));

test("one presenter: every other page follows its newest page", () => {
  const a = new Present();
  const v = new Present();
  a.start();
  a.posted(1);
  for (const p of [a, v]) {
    p.record(live(1, "a", 1), p === a ? "a" : "v");
    p.record(live(2, "a", 3), p === a ? "a" : "v");
  }
  assert.equal(a.yields(here("a"), "a"), false);
  assert.equal(a.presenter(here("a"), "a"), null, "never its own presenter");
  assert.deepEqual(v.presenter(here("a"), "v"), { from: "a", page: 3, principal: "npub-a", claim: 1 });
  assert.equal(v.presenter(here(), "v"), null, "a presenter whose page is gone is no one's");
});

test("a newer start takes over, and the older gives way", () => {
  const a = new Present();
  a.start();
  a.posted(1);
  a.record(live(1, "a"), "a");
  a.record(live(2, "b", 2), "a");
  assert.equal(a.yields(here("a", "b"), "a"), true);
  a.stop();
  a.record(ended(3, "a"), "a");
  assert.equal(a.presenter(here("b"), "a").from, "b");
  // a new start here is a new claim, newer than b's
  a.start();
  a.posted(4);
  a.record(live(4, "a"), "a");
  assert.equal(a.yields(here("a", "b"), "a"), false);
});

// Every order the two posts' answers and the two records can reach each
// presenting page, the records always in seq order (A's claim 5, B's 6).
function orders() {
  const out = [];
  const events = ["answer", "rec5", "rec6"];
  const permute = (rest, acc) => {
    if (!rest.length) {
      if (acc.indexOf("rec5") < acc.indexOf("rec6")) out.push(acc);
      return;
    }
    rest.forEach((e, i) => permute([...rest.slice(0, i), ...rest.slice(i + 1)], [...acc, e]));
  };
  permute(events, []);
  return out;
}

test("crossed starts: the newer claim presents, in every order, and the older gives way", () => {
  for (const orderA of orders()) {
    for (const orderB of orders()) {
      const pages = { a: new Present(), b: new Present() };
      const presence = new Set(["a", "b"]);
      const list = () => here(...presence);
      pages.a.start();
      pages.b.start();
      const yielded = new Set();
      const deliver = (who, e) => {
        const p = pages[who];
        if (e === "answer") p.posted(who === "a" ? 5 : 6);
        else p.record(e === "rec5" ? live(5, "a") : live(6, "b", 2), who);
        // what the page does after each event: give way, if it must
        if (p.yields(list(), who)) {
          p.stop();
          presence.delete(who);
          yielded.add(who);
        }
      };
      // the two pages' events interleaved: A's first, then B's (each page's
      // own order is what matters; a yield changes presence for the other)
      for (let i = 0; i < 3; i++) {
        deliver("a", orderA[i]);
        deliver("b", orderB[i]);
      }
      const why = `A saw ${orderA}, B saw ${orderB}`;
      assert.deepEqual([...yielded], ["a"], `only the older claim gives way: ${why}`);
      assert.equal(pages.b.presenting, true, `the newer presents: ${why}`);
      assert.equal(pages.a.presenter(list(), "a")?.from, "b", `the older follows the newer: ${why}`);
      const viewer = new Present();
      viewer.record(live(5, "a"), "v");
      viewer.record(live(6, "b", 2), "v");
      assert.equal(viewer.presenter(list(), "v")?.from, "b", `a viewer follows the newer: ${why}`);
    }
  }
});

test("a page that has started yields to nothing until it knows its own claim", () => {
  const b = new Present();
  b.start();
  b.record(live(5, "a"), "b");
  assert.equal(b.yields(here("a", "b"), "b"), false, "its own claim may be the newer");
  b.posted(6);
  assert.equal(b.yields(here("a", "b"), "b"), false, "and it is");
  const a = new Present();
  a.start();
  a.record(live(5, "a"), "a");
  a.record(live(6, "b"), "a");
  assert.equal(a.yields(here("a", "b"), "a"), true, "its own record told it its claim, the older");
});
