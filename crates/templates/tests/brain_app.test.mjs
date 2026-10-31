// The brain's index upkeep (templates/brain/app.mjs) against an in-memory
// SQLite (as a Durable Object's) and fakes of its two capabilities: the
// files at main and the fragment's search index, the index's fences and
// tombstones (cell/src/search_index.rs) and its `cleared` answer included,
// the index answering as the `Search` capability does across Workers RPC
// (`{answer}` or a plain `{error}`, thrown on the app's side as
// cell/platform.mjs throws it). Each test sets up an interleaving the
// platform allows (Alex's reviews of #869): batches side by side, a claim
// that lapses, a page deleted or made while read, an index emptied by an
// embeddings change, a batch that fails.
import { test } from "node:test";
import assert from "node:assert/strict";
import { register } from "node:module";
import { DatabaseSync } from "node:sqlite";

// the brain's base class as the platform's wrapper gives it: its env's
// capabilities behind `files` and `searchIndex`, a refusal thrown on the
// app's side with its fields (cell/platform.mjs, `searchIndex`)
const BASE = `export class DurableObject {
  constructor(ctx, env) { this.ctx = ctx; this.env = env; }
  get files() { return this.env.FILES; }
  get searchIndex() {
    const cap = this.env.SEARCH;
    const answered = async (pending) => { const { answer, error } = await pending; if (error) throw Object.assign(new Error(error.message), error); return answer; };
    return { upsert: (s) => answered(cap.upsert(s)), delete: (d) => answered(cap.delete(d)), query: (q) => answered(cap.query(q)), status: () => answered(cap.status()) };
  }
}`;
const HOOKS = `export async function resolve(specifier, context, next) {
  if (specifier === "cloudflare:workers") return { url: ${JSON.stringify("data:text/javascript," + encodeURIComponent(BASE))}, shortCircuit: true };
  return next(specifier, context);
}`;
register("data:text/javascript," + encodeURIComponent(HOOKS));
const { App } = await import(new URL("../../../templates/brain/app.mjs", import.meta.url).href);

const CLAIM_MS = 2 * 60 * 1000;
// each test's own bound: a batch waiting on a gate that never opens (a
// second batch reading a page another holds) fails, never hangs the run
const it = (name, fn) => test(name, { timeout: 10_000 }, fn);
const EDITOR = { role: "editor" };

function storage() {
  const db = new DatabaseSync(":memory:");
  const sql = {
    exec(q, ...args) {
      const st = db.prepare(q);
      const rows = st.columns().length ? st.all(...args) : (st.run(...args), []);
      return { toArray: () => rows, one: () => rows[0] };
    },
  };
  const transactionSync = (fn) => {
    db.exec("BEGIN");
    try {
      const out = fn();
      db.exec("COMMIT");
      return out;
    } catch (e) {
      db.exec("ROLLBACK");
      throw e;
    }
  };
  return { sql, transactionSync };
}

// The files at main: a read or a listing takes them as they are when it
// starts, and waits on the path's gate (`read:<path>`, `list:<path>`), if
// one is set, before it answers.
function files(initial) {
  const at = new Map(Object.entries(initial));
  const gates = new Map();
  const wait = async (key) => {
    const gate = gates.get(key);
    if (gate) await gate;
  };
  return {
    at,
    gate(key) {
      let open;
      gates.set(key, new Promise((resolve) => (open = resolve)));
      return () => {
        gates.delete(key);
        open();
      };
    },
    async list(prefix) {
      const listed = [...at].filter(([p]) => p.startsWith(prefix)).map(([path, text]) => ({ path, size: Buffer.byteLength(text) }));
      await wait(`list:${prefix}`);
      return listed;
    },
    async readBytes(path) {
      const text = at.get(path);
      await wait(`read:${path}`);
      return text === undefined ? null : new TextEncoder().encode(text);
    },
  };
}

// The fragment's index as the Search capability answers it: each source's
// revision and text, each source's fence (kept past a delete: a
// tombstone), a fence below the stored one refused (409 naming it);
// `clearNext` makes the next write empty it first, as a write in a new
// embeddings space does, its fences kept, the clear said on whatever comes
// of the write. `forget` is the platform's own delete, which sends no fence;
// `holdDelete` holds the next delete until it is let go.
function index() {
  const sources = new Map();
  const fences = new Map();
  const refused = (code, status, extra) => ({ error: { message: code, code, status, cleared: false, fence: null, ...extra } });
  const ix = {
    sources,
    fences,
    writes: 0,
    fail: null,
    clearNext: false,
    text: (source) => sources.get(source)?.text ?? null,
    forget: (source) => sources.delete(source),
    held: null,
    holdDelete() {
      let go;
      ix.held = new Promise((resolve) => (go = resolve));
      return go;
    },
    async upsert(src) {
      if (ix.fail?.(src)) return refused("upstream_failed", 502);
      const stored = fences.get(src.source);
      if (src.fence !== undefined && stored !== undefined && src.fence < stored) return refused("conflict", 409, { fence: stored });
      let cleared = false;
      if (ix.clearNext) {
        sources.clear();
        ix.clearNext = false;
        cleared = true;
      }
      const written = sources.get(src.source)?.revision !== src.revision;
      if (src.fence !== undefined) fences.set(src.source, Math.max(stored ?? 0, src.fence));
      ix.writes++;
      sources.set(src.source, { revision: src.revision, text: src.sections.map((s) => s.text).join("\n") });
      return { answer: { source: src.source, revision: src.revision, written, chunks: src.sections.length, cleared } };
    },
    async delete({ source, fence }) {
      const held = ix.held;
      ix.held = null;
      if (held) await held;
      const stored = fences.get(source);
      if (fence !== undefined && stored !== undefined && fence < stored) return refused("conflict", 409, { fence: stored });
      if (fence !== undefined) fences.set(source, Math.max(stored ?? 0, fence));
      return { answer: { source, deleted: sources.delete(source) } };
    },
    async query() {
      return { answer: { results: [], semantic: "off" } };
    },
    async status() {
      return { answer: { chunks: sources.size, chunks_split: 25000, chunks_max: 50000, semantic: "off" } };
    },
  };
  return ix;
}

// A brain over `pages`, its first full pass done.
async function brain(pages) {
  const fs = files(pages);
  const ix = index();
  const ctx = { storage: storage() };
  const app = new App(ctx, { FILES: fs, SEARCH: ix });
  await settle(app);
  return { app, fs, ix, ctx };
}

// Batches until nothing is pending.
async function settle(app) {
  for (let i = 0; i < 20; i++) {
    const r = await app.reindex({}, EDITOR);
    if (r.pending === 0) return r;
  }
  throw new Error("the index never settled");
}

const mark = (app, ...paths) => app.mark({ paths, full: false }, EDITOR);
const claims = (ctx) => ctx.storage.sql.exec("SELECT path, by FROM dirty ORDER BY path").toArray();

// A clock that tests move: Date.now, for the claims.
function clock(t) {
  const real = Date.now;
  let now = real();
  Date.now = () => now;
  t.after(() => (Date.now = real));
  return (ms) => (now += ms);
}

it("a batch that read a page before a newer change never writes the older over it", async () => {
  const { app, fs, ix } = await brain({ "w/a.md": "# A\n\nfirst" });
  fs.at.set("w/a.md", "# A\n\nsecond");
  mark(app, "w/a.md");
  const open = fs.gate("read:w/a.md");
  const a = app.reindex({}, EDITOR); // reads "second", and waits
  await new Promise((r) => setImmediate(r));
  fs.at.set("w/a.md", "# A\n\nthird");
  mark(app, "w/a.md");
  // a new mark keeps the claim: no other batch reads the page meanwhile
  assert.equal((await app.reindex({}, EDITOR)).claimed, 0);
  open();
  await a;
  await settle(app);
  assert.equal(ix.text("w/a.md"), "third");
});

it("a batch whose claim lapsed loses to the one that took the page, whichever writes first", async (t) => {
  const tick = clock(t);
  for (const firstWriter of ["late", "early"]) {
    const { app, fs, ix } = await brain({ "w/a.md": "# A\n\nfirst" });
    fs.at.set("w/a.md", "# A\n\nsecond");
    mark(app, "w/a.md");
    const openA = fs.gate("read:w/a.md");
    const a = app.reindex({}, EDITOR); // reads "second", and waits past its claim
    await new Promise((r) => setImmediate(r));
    fs.at.set("w/a.md", "# A\n\nthird");
    mark(app, "w/a.md");
    tick(CLAIM_MS + 1);
    const openB = fs.gate("read:w/a.md");
    const b = app.reindex({}, EDITOR); // takes the lapsed claim, reads "third", and waits
    await new Promise((r) => setImmediate(r));
    if (firstWriter === "late") {
      openB();
      await b;
      openA();
      await a;
    } else {
      openA();
      await a;
      openB();
      await b;
    }
    const after = await settle(app);
    assert.equal(ix.text("w/a.md"), "third", `the stale batch wrote ${firstWriter}`);
    assert.equal(after.pending, 0);
  }
});

it("a page deleted while it is read is not left in the index", async () => {
  for (const fresh of [false, true]) {
    const { app, fs, ix } = await brain(fresh ? {} : { "w/a.md": "# A\n\nfirst" });
    fs.at.set("w/a.md", "# A\n\nsecond");
    mark(app, "w/a.md");
    const open = fs.gate("read:w/a.md");
    const a = app.reindex({}, EDITOR);
    await new Promise((r) => setImmediate(r));
    // main deletes it: the platform forgets its source, and the trigger marks it
    fs.at.delete("w/a.md");
    ix.forget("w/a.md");
    mark(app, "w/a.md");
    open();
    await a;
    await settle(app);
    assert.equal(ix.text("w/a.md"), null, fresh ? "a page made and deleted while read" : "a page changed and deleted while read");
  }
});

it("an index emptied by an embeddings change is written whole again", async () => {
  const { app, fs, ix } = await brain({ "w/a.md": "# A\n\none", "w/b.md": "# B\n\ntwo", "w/c.md": "# C\n\nthree" });
  ix.clearNext = true;
  fs.at.set("w/a.md", "# A\n\nuno");
  mark(app, "w/a.md");
  await settle(app);
  assert.deepEqual([ix.text("w/a.md"), ix.text("w/b.md"), ix.text("w/c.md")], ["uno", "two", "three"]);
});

it("an unchanged sync writes nothing; an editor's full pass writes back what the index lost", async () => {
  const { app, ix } = await brain({ "w/a.md": "# A\n\none", "w/b.md": "# B\n\ntwo" });
  const before = ix.writes;
  mark(app, "w/a.md", "w/b.md");
  await settle(app);
  assert.equal(ix.writes, before, "the same bytes again write nothing");
  ix.sources.clear();
  await app.reindex({ full: true }, EDITOR);
  await settle(app);
  assert.deepEqual([ix.text("w/a.md"), ix.text("w/b.md")], ["one", "two"]);
  // a viewer's full pass is no rebuild
  ix.sources.clear();
  await app.reindex({ full: true }, { role: "viewer" });
  await settle(app);
  assert.equal(ix.text("w/a.md"), null);
});

it("a batch that fails lets go of its claims, and a retry at once does the work", async () => {
  const { app, fs, ix, ctx } = await brain({});
  fs.at.set("w/a.md", "# A\n\none");
  mark(app, "w/a.md");
  ix.fail = () => true;
  await assert.rejects(app.reindex({}, EDITOR));
  ix.fail = null;
  const r = await app.reindex({}, EDITOR);
  assert.deepEqual([r.claimed, r.indexed, r.pending], [1, 1, 0]);
  assert.equal(ix.text("w/a.md"), "one");
  // a failed batch lets go of its own claims only
  fs.at.set("w/b.md", "# B\n\ntwo");
  fs.at.set("w/c.md", "# C\n\nthree");
  mark(app, "w/b.md");
  const open = fs.gate("read:w/b.md");
  const holder = app.reindex({}, EDITOR); // holds b
  await new Promise((r) => setImmediate(r));
  mark(app, "w/c.md");
  ix.fail = (src) => src.source === "w/c.md";
  await assert.rejects(app.reindex({}, EDITOR));
  const held = claims(ctx).find((c) => c.path === "w/b.md");
  assert.ok(held?.by, "the other batch's claim stays");
  ix.fail = null;
  open();
  await holder;
  await settle(app);
  assert.deepEqual([ix.text("w/b.md"), ix.text("w/c.md")], ["two", "three"]);
});

it("a run that finds the rest held waits out one claim, then takes what a batch that died left", async (t) => {
  const tick = clock(t);
  const { app, fs, ix, ctx } = await brain({});
  fs.at.set("w/a.md", "# A\n\none");
  mark(app, "w/a.md");
  // a batch that claimed it and died
  ctx.storage.sql.exec("UPDATE dirty SET claimed = ?, by = 'dead'", Date.now());
  let slept = 0;
  const job = {
    run: 1,
    call: (op, input) => app[op](input, EDITOR),
    sleep: async (ms) => {
      slept++;
      tick(ms);
    },
  };
  const out = await app.changed({}, job);
  assert.deepEqual([slept, out.pending], [1, 0]);
  assert.equal(ix.text("w/a.md"), "one");
  // with nothing to do: two steps, no wait
  const idle = await app.changed({}, job);
  assert.deepEqual([slept, idle.steps], [1, 1]);
});

it("a write whose claim lapsed never brings back a page a newer batch deleted", async (t) => {
  const tick = clock(t);
  const { app, fs, ix } = await brain({});
  fs.at.set("w/a.md", "# A\n\nnew");
  mark(app, "w/a.md");
  const open = fs.gate("read:w/a.md");
  const a = app.reindex({}, EDITOR); // reads the new page, and waits past its claim
  await new Promise((r) => setImmediate(r));
  fs.at.delete("w/a.md");
  ix.forget("w/a.md");
  mark(app, "w/a.md");
  tick(CLAIM_MS + 1);
  await settle(app); // a newer batch takes the deletion to its end
  open();
  await a; // the stale write lands now, at its older fence
  const after = await settle(app);
  assert.equal(ix.text("w/a.md"), null, "the deleted page stays out");
  assert.equal(after.pending, 0);
});

it("a delete whose claim lapsed never takes out the page a newer batch wrote", async (t) => {
  const tick = clock(t);
  const { app, fs, ix } = await brain({ "w/a.md": "# A\n\nfirst" });
  fs.at.delete("w/a.md");
  ix.forget("w/a.md");
  mark(app, "w/a.md");
  const open = fs.gate("list:w/a.md");
  const a = app.reindex({}, EDITOR); // finds it gone, and waits past its claim
  await new Promise((r) => setImmediate(r));
  fs.at.set("w/a.md", "# A\n\nagain");
  mark(app, "w/a.md");
  tick(CLAIM_MS + 1);
  const letGo = ix.holdDelete();
  open();
  await new Promise((r) => setImmediate(r)); // its delete is on its way
  await settle(app); // a newer batch writes the page made again
  letGo();
  await a; // the stale delete lands now, at its older fence
  const after = await settle(app);
  assert.equal(ix.text("w/a.md"), "again", "the page made again stays");
  assert.equal(after.pending, 0);
});
