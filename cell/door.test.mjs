// door.mjs: a SimpleX door's object, against stand-ins for its storage
// (node's own SQLite), its bucket and its container (workerd's alone has
// `cloudflare:workers`: a hook gives it the two classes door.mjs extends).
// Valid (a new door starts with none; a save stored, named and restored),
// invalid (a door that saved never starts as a new one: its last file lost,
// it stops), replay (an unreadable report retires its own save once, from
// its own life alone), restart (each runner life a generation of its own:
// no save's key reused, a stale life's upload never touching a committed
// save), and its starts (a stop asked while one registers its egress
// starts nothing; a first start that fails is tried again by its watch).
import { test } from "node:test";
import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { DatabaseSync } from "node:sqlite";

registerHooks({
  resolve(specifier, context, next) {
    if (specifier !== "cloudflare:workers") return next(specifier, context);
    return { url: "data:text/javascript,export class DurableObject { constructor(ctx, env) { this.ctx = ctx; this.env = env; } } export class WorkerEntrypoint {}", shortCircuit: true };
  },
});
// workerd's: a stream whose length is said first (the bucket's upload)
globalThis.FixedLengthStream ??= class {
  constructor() {
    const t = new TransformStream();
    this.readable = t.readable;
    this.writable = t.writable;
  }
};
const logged = [];
console.log = (line) => logged.push(JSON.parse(line));
const { SimplexDoor } = await import("./door.mjs");

const NAME = "pat-simplex--k3x9";
const key = (save) => `doors/${NAME}/saves/${save}`;

function storage() {
  const db = new DatabaseSync(":memory:");
  const alarms = [];
  const sql = {
    exec(q, ...binds) {
      const rows = db.prepare(q).all(...binds).map((r) => ({ ...r }));
      return { toArray: () => rows, one: () => (assert.equal(rows.length, 1, q), rows[0]) };
    },
  };
  const transactionSync = (f) => {
    db.exec("BEGIN");
    try {
      const r = f();
      db.exec("COMMIT");
      return r;
    } catch (e) {
      db.exec("ROLLBACK");
      throw e;
    }
  };
  return { sql, transactionSync, alarms, setAlarm: async (t) => void alarms.push(t) };
}

// The bucket: `uploading`, when set, is awaited in the middle of each put.
function bucket() {
  const b = { files: new Map(), uploading: null };
  b.get = async (k) => (b.files.has(k) ? { body: new Blob([b.files.get(k)]).stream() } : null);
  b.put = async (k, v) => {
    const bytes = typeof v === "string" ? v : await new Response(v).text();
    if (b.uploading) await b.uploading();
    b.files.set(k, bytes);
    return { size: bytes.length };
  };
  b.delete = async (ks) => [].concat(ks).forEach((k) => b.files.delete(k));
  return b;
}

// A container: `registering`, when set, is awaited as its egress registers.
function container() {
  const c = { running: false, starts: [], signals: [], registering: null };
  c.interceptOutboundHttp = async () => c.registering && (await c.registering());
  c.start = (opts) => {
    c.running = true;
    c.starts.push(opts);
  };
  c.monitor = () => new Promise(() => {});
  c.signal = async (n) => void c.signals.push(n);
  return c;
}

function door() {
  const st = storage();
  const c = container();
  const blobs = bucket();
  const ctx = { storage: st, container: c, exports: { DoorEgress: () => ({}) }, blockConcurrencyWhile: (f) => f() };
  const o = new SimplexDoor(ctx, { BLOBS: blobs, FRAGMENT_COMPUTER_INTERNET: "off" });
  const call = (method, path, headers = {}, body) => o.fetch(new Request(`http://door${path}`, { method, headers: { "x-fragment-release": "dev", ...headers }, body, duplex: "half" }));
  const d = { o, st, c, blobs, call };
  d.lever = (op, more = {}) => call("POST", "/door/lever", {}, JSON.stringify({ door: NAME, op, ...more }));
  d.start = () => d.lever("start", { agent: "Juniper" });
  d.row = () => st.sql.exec("SELECT started, generation, saved, stopped FROM door").toArray()[0];
  d.saves = () => st.sql.exec("SELECT generation || '-' || seq AS id, unreadable FROM saves ORDER BY generation, seq").toArray();
  // a runner's calls: a life begins (its start's generation), a save (its life's)
  d.restore = (started) => call("GET", "/egress/save", { "x-door-generation": String(started) });
  d.save = (life, seq, bytes = `save ${life}-${seq}`) => call("PUT", `/egress/save?seq=${seq}`, { "x-door-generation": String(life), "x-door-mark": "0", "content-length": String(bytes.length) }, bytes);
  d.unreadable = (life, save) => call("POST", "/egress/unreadable", { "x-door-generation": String(life) }, JSON.stringify({ save, why: "a test's" }));
  return d;
}

const life = (r) => Number(r.headers.get("x-door-life"));

test("a new door starts with none; its save is stored, named and restored by its next life", async () => {
  const d = door();
  assert.equal((await d.start()).status, 200);
  assert.deepEqual(d.c.starts.map((o) => o.env.DOOR_GENERATION), ["1"]);
  assert.equal(d.st.alarms.length, 1, "its watch set");
  assert.equal((await d.call("GET", "/__probe", { "x-fragment-release": "another", "x-fragment-probe": "1" })).headers.get("x-fragment-release-refused"), "dev", "another release's call refused first");
  const first = await d.restore(1);
  assert.equal(first.status, 204, "none: a new door");
  assert.equal(life(first), 2);
  assert.equal((await d.save(2, 1)).status, 200);
  assert.equal(d.blobs.files.get(key("2-1")), "save 2-1", "its file under its door's own prefix");
  const again = await d.restore(1);
  assert.equal(again.status, 200);
  assert.equal(again.headers.get("x-door-save"), "2-1", "the save named, to be named back exactly");
  assert.equal(await again.text(), "save 2-1");
  assert.equal((await d.restore(2)).status, 409, "a start the door never made is refused");
});

test("each runner life is a generation of its own: no save's key reused, a stale life's upload refused and its file alone let go", async () => {
  const d = door();
  await d.start();
  assert.equal(life(await d.restore(1)), 2);
  assert.equal((await d.save(2, 1)).status, 200);
  // its runner restarted in place (local Docker): the same start, a new life
  assert.equal(life(await d.restore(1)), 3);
  assert.equal((await d.save(2, 2)).status, 409, "the life before it is fenced off");
  assert.equal((await d.save(3, 1)).status, 200, "its seq from 1 again, under its own life: a key of its own");
  assert.equal((await d.save(3, 1, "other bytes")).status, 409, "a key taken is never written again");
  assert.equal(d.blobs.files.get(key("3-1")), "save 3-1");
  // a life begun while another's upload is under way
  d.blobs.uploading = async () => {
    d.blobs.uploading = null;
    await d.restore(1);
  };
  assert.equal((await d.save(3, 2)).status, 409, "superseded during its upload");
  assert.equal(d.blobs.files.has(key("3-2")), false, "its own file let go");
  assert.deepEqual([...d.blobs.files.keys()].sort(), [key("2-1"), key("3-1")], "every committed save's file kept");
  assert.deepEqual(d.saves().map((s) => s.id), ["2-1", "3-1"]);
});

test("an upload under way holds its key: a replay of it is refused, and superseded it lets go of its own file alone", async () => {
  const d = door();
  await d.start();
  const l = life(await d.restore(1));
  let holding, release;
  const held = new Promise((r) => (holding = r));
  const gate = new Promise((r) => (release = r));
  d.blobs.uploading = async () => {
    d.blobs.uploading = null;
    holding();
    await gate;
  };
  const first = d.save(l, 1);
  await held;
  assert.equal((await d.save(l, 1)).status, 409, "its replay, while it uploads, refused");
  assert.equal(life(await d.restore(1)), l + 1, "its runner restarted meanwhile");
  release();
  assert.equal((await first).status, 409, "superseded during its upload");
  for (const s of d.saves()) assert.ok(d.blobs.files.has(key(s.id)), `every save named has its file: ${s.id}`);
  assert.deepEqual(d.saves(), [], "none of the superseded life's");
});

test("a door that saved never starts as a new one: its last file lost, it stops", async () => {
  const d = door();
  await d.start();
  await d.restore(1);
  await d.save(2, 1);
  d.blobs.files.clear();
  for (const asked of ["first", "again"]) {
    assert.equal((await d.restore(1)).status, 410, `none left, asked ${asked}: never a new profile`);
  }
  assert.equal(d.row().stopped, "no save of this door restores");
  assert.deepEqual(d.saves(), [{ id: "2-1", unreadable: 1 }]);
  d.c.running = false;
  await d.start();
  assert.equal(d.c.starts.length, 1, "asked to start again, it starts nothing");
  assert.equal(d.row().stopped, "no save of this door restores");
});

test("an unreadable report retires its own save once, from its own life alone", async () => {
  const d = door();
  await d.start();
  await d.restore(1);
  await d.save(2, 1);
  await d.save(2, 2);
  const given = await d.restore(1);
  assert.equal(given.headers.get("x-door-save"), "2-2");
  assert.equal((await d.unreadable(2, "2-2")).status, 409, "a superseded life's report changes nothing");
  assert.deepEqual(d.saves().map((s) => s.unreadable), [0, 0]);
  for (const asked of ["first", "again"]) {
    assert.equal((await d.unreadable(life(given), "2-2")).status, 200, asked);
    assert.deepEqual(d.saves(), [{ id: "2-1", unreadable: 0 }, { id: "2-2", unreadable: 1 }], `its own save alone, asked ${asked}`);
  }
  assert.equal((await d.unreadable(life(given), "newest")).status, 400, "a save named exactly or not at all");
  assert.equal((await d.restore(1)).headers.get("x-door-save"), "2-1", "the one before it given");
});

test("a stop asked while a start registers its egress starts nothing", async () => {
  const d = door();
  let registered;
  const registering = new Promise((r) => (registered = r));
  let release;
  d.c.registering = () => (registered(), new Promise((r) => (release = r)));
  const starting = d.start();
  await registering;
  const stopping = d.lever("stop");
  while (!d.row().stopped) await new Promise((r) => setImmediate(r));
  release();
  assert.equal((await starting).status, 200);
  assert.equal((await stopping).status, 200);
  assert.equal(d.c.starts.length, 0, "nothing started after the stop");
  assert.deepEqual(d.c.signals, [], "and nothing to stop");
  assert.equal(d.row().stopped, "asked");
  // one asked once its container runs: told to stop (it saves last)
  d.c.registering = null;
  await d.start();
  assert.equal((await d.lever("stop")).status, 200);
  assert.deepEqual(d.c.signals, [15]);
});

test("a first start whose egress fails to register is tried again by its watch", async () => {
  const d = door();
  d.c.registering = async () => {
    d.c.registering = null;
    throw new Error("the runtime refused it");
  };
  assert.equal((await d.start()).status, 500);
  assert.equal(d.c.starts.length, 0);
  assert.equal(d.st.alarms.length, 1, "its watch set before the start was tried");
  await d.o.alarm();
  assert.deepEqual(d.c.starts.map((o) => o.env.DOOR_GENERATION), ["2"], "started by its watch");
});
