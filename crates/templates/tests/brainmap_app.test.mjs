// The brain map's app (templates/brainmap/app.mjs) against an in-memory
// SQLite and a fake of the files at main. Valid: the template's own sample
// map.json answers whole. Invalid: no map, one too large, one that is not
// JSON, one of another version or shape: each answers `map: null` and says
// why. Replay: `touched` counts each move of map.json, again after a
// restart (a new object over the same storage).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { register } from "node:module";
import { DatabaseSync } from "node:sqlite";

const BASE = `export class DurableObject {
  constructor(ctx, env) { this.ctx = ctx; this.env = env; }
  get files() { return this.env.FILES; }
}`;
const HOOKS = `export async function resolve(specifier, context, next) {
  if (specifier === "cloudflare:workers") return { url: ${JSON.stringify("data:text/javascript," + encodeURIComponent(BASE))}, shortCircuit: true };
  return next(specifier, context);
}`;
register("data:text/javascript," + encodeURIComponent(HOOKS));
const TEMPLATE = new URL("../../../templates/brainmap/", import.meta.url);
const { App, MAP_MAX_BYTES, problemOf } = await import(new URL("app.mjs", TEMPLATE).href);
const SAMPLE = readFileSync(new URL("map.json", TEMPLATE), "utf8");

function storage(db = new DatabaseSync(":memory:")) {
  return {
    db,
    exec(q, ...args) {
      const st = db.prepare(q);
      const rows = st.columns().length ? st.all(...args) : (st.run(...args), []);
      return { toArray: () => rows, one: () => rows[0] };
    },
  };
}

// an app over main's files (path → text), its SQL `sql` (kept across a restart)
function app(files = {}, sql = storage()) {
  const FILES = {
    async stat(path) {
      return path in files ? { path, size: Buffer.byteLength(files[path]), sha: "s", commit: "c" } : null;
    },
    async read(path) {
      return files[path] ?? null;
    },
  };
  return new App({ storage: { sql } }, { FILES });
}

test("the template's own sample map answers whole", async () => {
  const r = await app({ "map.json": SAMPLE }).map();
  assert.equal(r.problem, undefined);
  assert.deepEqual(r.map, JSON.parse(SAMPLE));
  assert.equal(r.map.sample, true, "the page says it is sample data");
  assert.equal(r.rev, 0);
});

test("a map that is missing, too large, not JSON or not this version answers none, and says why", async () => {
  const cases = [
    [{}, /no map\.json/],
    [{ "map.json": " ".repeat(MAP_MAX_BYTES + 1) }, /the most this page reads/],
    [{ "map.json": "{ not json" }, /not JSON/],
    [{ "map.json": JSON.stringify({ version: 2, brain: {} }) }, /version 2/],
    [{ "map.json": JSON.stringify({ version: 1 }) }, /names no brain/],
    [{ "map.json": JSON.stringify({ version: 1, brain: {}, sources: {} }) }, /sources is not a list/],
    [{ "map.json": "[]" }, /not an object/],
  ];
  for (const [files, why] of cases) {
    const r = await app(files).map();
    assert.equal(r.map, null, JSON.stringify(files).slice(0, 80));
    assert.match(r.problem, why);
  }
  assert.equal(problemOf({ version: 1, brain: {} }), null, "the least map is a brain");
});

test("touched counts each move of map.json, and the count survives a restart", async () => {
  const sql = storage();
  const first = app({ "map.json": SAMPLE }, sql);
  assert.deepEqual(first.touched({ ref: "main", commit: "abc", paths: ["map.json"], more: 0 }), { rev: 1 });
  assert.deepEqual(first.touched({ commit: "def" }), { rev: 2 });
  const again = app({ "map.json": SAMPLE }, storage(sql.db));
  assert.equal((await again.map()).rev, 2, "a new object over the same storage");
  assert.deepEqual(again.touched({}), { rev: 3 }, "a trigger with no commit still counts");
});
