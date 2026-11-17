// cell/shell/apps-page.js's cards (FIN-279, #782): every card a read sets is
// drawn, though a newer batch of reads began meanwhile, which may find the
// card the same (its read carries the new card's etag) and draw nothing.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { createCards } from "../shell/apps-page.js";

// A server whose answers the test gives, one request at a time.
function server() {
  const asked = [];
  const fetch = (url, options) => new Promise((answer) => asked.push({ url, etag: options.headers["if-none-match"] ?? null, answer }));
  // the newest read of `url` waiting (`oldest`: the first)
  const answer = (url, status, etag = null, oldest = false) => {
    const matching = asked.map((a, i) => [a, i]).filter(([a]) => a.url === url).map(([, i]) => i);
    const i = oldest ? matching[0] : matching.at(-1);
    assert.ok(i !== undefined, `a read of ${url} is waiting`);
    const [read] = asked.splice(i, 1);
    read.answer({ status, ok: status >= 200 && status < 300, headers: { get: () => etag }, blob: async () => `bytes ${etag}` });
    return read;
  };
  return { fetch, answer, waiting: () => asked.map((a) => a.url) };
}

// every promise the answers released, run
const settle = () => new Promise((done) => setImmediate(done));

const GROCERIES = "/api/f/groceries--q84t/card";
const POTLUCK = "/api/f/potluck--57sb/card";

test("a card a superseded batch's read set is drawn, though the newer batch found it the same", async () => {
  const s = server();
  let rows = [{ name: "groceries--q84t", kind: "app", sharing: "link" }, { name: "potluck--57sb", kind: "app", sharing: "members" }];
  let made = 0;
  const shown = [];
  const cards = createCards({
    rows: () => rows,
    visit: () => 1,
    shown: () => true,
    draw: () => shown.push(cards.get("groceries--q84t")?.url ?? null),
    fetch: s.fetch,
    urls: { createObjectURL: () => `blob:${++made}`, revokeObjectURL: () => {} },
    timers: { setTimeout: () => 0, clearTimeout: () => {} },
  });

  // the visit's open: both cards asked; groceries' changed card comes back,
  // potluck's (it has none yet) is slow
  const opened = cards.load(true);
  assert.deepEqual(s.waiting(), [GROCERIES, POTLUCK]);
  s.answer(GROCERIES, 200, '"new"');
  await settle();
  assert.equal(cards.get("groceries--q84t")?.etag, '"new"', "its read set the new card");

  // the list changed meanwhile (its sharing): a newer batch asks again, with
  // the new card's etag, and hears it is the same
  rows = [{ ...rows[0], sharing: "public" }, rows[1]];
  const listed = cards.load();
  const again = s.answer(GROCERIES, 304);
  assert.equal(again.etag, '"new"', "the newer read asks with the new card's etag");
  s.answer(POTLUCK, 404);
  await listed;
  // then the first batch's slow read ends
  s.answer(POTLUCK, 404, null, true);
  await opened;

  assert.ok(shown.includes(cards.get("groceries--q84t").url), `the new card was drawn: draws saw ${JSON.stringify(shown)}`);
});

test("a read of an earlier visit, or one a newer read of its row superseded, sets nothing", async () => {
  const s = server();
  let visit = 1;
  const cards = createCards({
    rows: () => [{ name: "groceries--q84t", kind: "app" }],
    visit: () => visit,
    shown: () => true,
    draw: () => {},
    fetch: s.fetch,
    urls: { createObjectURL: (b) => `blob:${b}`, revokeObjectURL: () => {} },
    timers: { setTimeout: () => 0, clearTimeout: () => {} },
  });
  const earlier = cards.read("groceries--q84t", 1);
  visit = 2;
  s.answer(GROCERIES, 200, '"old visit"');
  assert.equal(await earlier, "stale");
  assert.equal(cards.get("groceries--q84t"), undefined, "an earlier visit's read sets nothing");

  const first = cards.read("groceries--q84t", 2);
  const second = cards.read("groceries--q84t", 2);
  s.answer(GROCERIES, 200, '"first"', true);
  assert.equal(await first, "stale", "superseded by the newer read of its row");
  s.answer(GROCERIES, 200, '"second"');
  assert.equal(await second, "new");
  assert.equal(cards.get("groceries--q84t").etag, '"second"');
});
