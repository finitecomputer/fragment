// door.mjs: a SimpleX door's object, against stand-ins for its storage
// (node's own SQLite), its bucket and its container (workerd's alone has
// `cloudflare:workers`: a hook gives it the two classes door.mjs extends).
// Valid (a new door starts with none; a save stored, named and restored),
// invalid (a door that saved never starts as a new one: its last file lost,
// it stops), replay (an unreadable report retires its own save once, from
// its own life alone), restart (each runner life a generation of its own:
// no save's key reused, a stale life's upload never touching a committed
// save), and its starts (a stop asked while one registers its egress
// starts nothing; a first start that fails is tried again by its watch);
// and its connector (D2): its jobs ask only their own door, and only
// their asks; its owner's messages reach the connector's inbox in order,
// each until taken, again at its watch; a deployment with no doors keeps
// nothing; an ask a delivery's mark overtook is asked again.
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
  const st = { sql, transactionSync, alarms, setAlarm: async (t) => void alarms.push(t) };
  st.deleteAlarm = async () => void (alarms.length = 0);
  // its storage gone whole: every table, as a Durable Object's SQLite's
  st.deleteAll = async () => {
    for (const { name } of db.prepare("SELECT name FROM sqlite_master WHERE type = 'table'").all()) db.exec(`DROP TABLE ${name}`);
  };
  st.tables = () => db.prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name").all().map((t) => t.name);
  return st;
}

// The bucket: `uploading`, when set, is awaited in the middle of each put.
function bucket() {
  const b = { files: new Map(), uploading: null };
  b.get = async (k) => (b.files.has(k) ? { body: new Blob([b.files.get(k)]).stream() } : null);
  // R2's put: a SHA-256 asked of it is checked as it stores (else it throws)
  b.put = async (k, v, opts = {}) => {
    const bytes = typeof v === "string" ? v : await new Response(v).text();
    if (b.uploading) await b.uploading();
    if (opts.sha256) {
      const raw = typeof v === "string" ? new TextEncoder().encode(v) : new TextEncoder().encode(bytes);
      const got = [...new Uint8Array(await crypto.subtle.digest("SHA-256", raw))].map((x) => x.toString(16).padStart(2, "0")).join("");
      if (got !== opts.sha256) throw new Error(`put: the SHA-256 asked is not its bytes' (${got})`);
    }
    b.files.set(k, bytes);
    return { size: bytes.length };
  };
  b.delete = async (ks) => [].concat(ks).forEach((k) => b.files.delete(k));
  // a page of the keys under `prefix` (R2's list, at most `limit`)
  b.list = async ({ prefix, limit = 1000 }) => {
    const keys = [...b.files.keys()].filter((k) => k.startsWith(prefix)).sort();
    return { objects: keys.slice(0, limit).map((key) => ({ key })), truncated: keys.length > limit };
  };
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
  c.destroyed = 0;
  c.destroy = async () => {
    c.running = false;
    c.destroyed += 1;
  };
  // its runner: `answers`, each ask's (status, body) in turn, then 202
  c.asked = [];
  c.answers = [];
  c.getTcpPort = () => ({
    fetch: async (url, init) => {
      c.asked.push({ method: init.method, path: new URL(url).pathname, mark: Number(init.headers["x-door-mark"]), body: init.body ? JSON.parse(init.body) : null });
      const next = c.answers.shift() ?? [202, { done: false }];
      const [status, body] = typeof next === "function" ? await next() : next;
      return new Response(JSON.stringify(body), { status });
    },
  });
  return c;
}

const PLATFORM = "https://platform.example";
// a connector's door: its name (NAME) and its life's digits
const DOOR = `${NAME}--0123456789ab`;
const INBOX = `${PLATFORM}/api/f/${NAME}/inbox`;

// `noDoors`: a deployment that runs none (no container for them)
function door({ noDoors = false } = {}) {
  const st = storage();
  const c = noDoors ? null : container();
  const blobs = bucket();
  // the platform's front, as its inbox answers: `inbox.status`, each post kept
  const inbox = { status: 200, posts: [] };
  const front = { fetch: async (url, init) => (inbox.posts.push({ url, secret: init.headers["x-simplex-door-secret"], body: JSON.parse(init.body) }), new Response("{}", { status: inbox.status })) };
  const ctx = { storage: st, container: c, exports: { DoorEgress: () => ({}), default: front }, blockConcurrencyWhile: (f) => f() };
  const o = new SimplexDoor(ctx, { BLOBS: blobs, FRAGMENT_DOOR_INTERNET: "off", FRAGMENT_PLATFORM_URL: PLATFORM });
  const call = (method, path, headers = {}, body) => o.fetch(new Request(`http://door${path}`, { method, headers: { "x-fragment-release": "dev", ...headers }, body, duplex: "half" }));
  const d = { o, st, c, blobs, call, inbox };
  // its connector's job, as the platform sends it (the door and the connector named)
  d.job = (ask, body = {}, headers = {}) => call("POST", `/job/${ask}`, { "x-fragment-door": DOOR, "x-fragment-connector": NAME, ...headers }, JSON.stringify(body));
  d.connect = (secret = "s3cret-of-the-inbox") => d.job("connect", { name: "Juniper" }, { "x-door-secret": secret });
  // its runner's delivery of one of its owner's messages
  d.deliver = (id, text = `text of ${id}`) => call("POST", "/egress/deliver", {}, JSON.stringify({ id, kind: "text", text, at: "2026-11-21T00:00:00Z", file: null }));
  d.posted = () => d.inbox.posts.map((p) => p.body.payload.id);
  d.lever = (op, more = {}) => call("POST", "/door/lever", {}, JSON.stringify({ door: NAME, op, ...more }));
  d.start = () => d.lever("start", { agent: "Juniper" });
  d.row = () => st.sql.exec("SELECT started, generation, saved, stopped FROM door").toArray()[0];
  d.saves = () => st.sql.exec("SELECT generation || '-' || seq AS id, unreadable FROM saves ORDER BY generation, seq").toArray();
  // a runner's calls: a life begins (its start's generation), a save (its life's)
  d.restore = (started) => call("GET", "/egress/save", { "x-door-generation": String(started) });
  d.save = (life, seq, bytes = `save ${life}-${seq}`) => call("PUT", `/egress/save?seq=${seq}`, { "x-door-generation": String(life), "x-door-mark": "0", "content-length": String(bytes.length) }, bytes);
  d.unreadable = (life, save) => call("POST", "/egress/unreadable", { "x-door-generation": String(life) }, JSON.stringify({ save, why: "a test's" }));
  // its end, as the platform asks it: its connector unbound, or (`last`)
  // its connector's life over (deleted, wiped, other code)
  d.end = (name = DOOR, last = false) => call("POST", "/door/end", {}, JSON.stringify({ door: name, final: last }));
  // the same object started again over its storage (an eviction)
  d.again = () => new SimplexDoor(ctx, { BLOBS: blobs, FRAGMENT_DOOR_INTERNET: "off", FRAGMENT_PLATFORM_URL: PLATFORM });
  d.prefixed = (name = DOOR) => [...blobs.files.keys()].filter((k) => k.startsWith(`doors/${name}/`));
  // a file its runner gives (`life`'s), and one its connector gives or reads
  d.give = (life, sha, bytes, type = "audio/mp4") => call("PUT", `/egress/file/${sha}`, { "x-door-generation": String(life), "content-type": type, "content-length": String(bytes.length) }, bytes);
  d.jobFile = (method, sha, bytes) => call(method, `/job/file/${sha}`, { "x-fragment-door": DOOR, "x-fragment-connector": NAME, ...(bytes ? { "content-type": "application/pdf", "content-length": String(bytes.length) } : {}) }, bytes);
  return d;
}

const sha = (n) => String(n).padStart(2, "0").repeat(32);

// A file its runner gives (`life`'s) and a delivery names (`id`), which
// its connector keeps and says so: one nothing uses any more.
async function used(d, life, n, id = `u${n}`) {
  await d.give(life, sha(n), "x");
  await d.call("POST", "/egress/deliver", {}, JSON.stringify({ id, kind: "file", text: "", at: "2026-11-24T00:00:00Z", file: { sha256: sha(n), name: "x", type: "application/octet-stream", size: 1 } }));
  await d.jobFile("GET", sha(n));
  await d.job("kept", { id });
}
const uses = (d, n) => d.st.sql.exec("SELECT owner FROM uses WHERE sha = ? ORDER BY owner", sha(n)).toArray().map((u) => u.owner);

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

test("its connector's jobs ask only their own door, and only their asks", async () => {
  const d = door();
  assert.equal((await d.job("link")).status, 409, "nothing asked of a door its connector never connected");
  assert.equal((await d.call("POST", "/job/connect", { "x-door-secret": "s" }, JSON.stringify({ name: "Juniper" }))).status, 400, "no door named: refused");
  assert.equal((await d.job("connect", { name: "Juniper" }, { "x-door-secret": "s", "x-fragment-connector": "other--k3x9" })).status, 400, "a door not its connector's: refused");
  assert.equal((await d.job("connect", { name: "Juniper" })).status, 400, "no inbox secret given: refused");
  assert.equal(d.row(), undefined, "nothing kept from either");
  assert.equal((await d.connect()).status, 200);
  assert.deepEqual(d.c.starts.map((o) => o.env.DOOR_NAME), ["Juniper"], "started as its connector named it");
  assert.equal((await d.job("state", {}, { "x-fragment-door": `${NAME}--fedcba987654` })).status, 409, "another life's door: refused (its object, hosted, is another)");
  for (const ask of ["door/lever", "egress/save", "lever", "deliver"]) assert.equal((await d.job(ask)).status, 404, `${ask}: none of its jobs' asks`);
  const state = await (await d.job("state")).json();
  assert.equal(state.door.door, DOOR);
  assert.equal(state.inbox, true);
  assert.ok(!JSON.stringify(state).includes("s3cret-of-the-inbox"), "its secret never told back");
  // link, unlink and a reply go to its runner, each under a mark of its own
  assert.equal((await d.job("link", { n: 1 })).status, 202);
  assert.equal((await d.job("link")).status, 400, "a link names its connector's number");
  assert.equal((await d.job("reply", { key: "chat:7", text: "hi" })).status, 202);
  const asks = d.c.asked.filter((a) => a.method === "POST");
  assert.deepEqual(asks.map((a) => [a.path, a.body]), [["/link", null], ["/reply", { key: "chat:7", text: "hi" }]]);
  assert.ok(asks[1].mark > asks[0].mark);
});

test("a deployment that runs no doors refuses its connector's connect and keeps nothing", async () => {
  const d = door({ noDoors: true });
  const r = await d.connect();
  assert.equal(r.status, 409);
  assert.equal((await r.json()).error, "no_doors");
  assert.equal(d.row(), undefined);
  assert.deepEqual(d.st.sql.exec("SELECT * FROM inbox").toArray(), [], "no secret kept");
  assert.deepEqual(d.st.alarms, [], "no watch set, to wake in vain");
});

test("its owner's messages reach its connector's inbox in order, each until taken, again at its watch", async () => {
  const d = door();
  // a door started before its connector connected (D1's lever): its
  // owner's message kept, then sent once it does
  await d.lever("start", { door: DOOR, agent: "Juniper" });
  assert.equal((await d.deliver("m1")).status, 200);
  assert.deepEqual(d.posted(), [], "no inbox to send it to yet");
  await d.connect();
  await d.o.alarm();
  assert.deepEqual(d.posted(), ["m1"]);
  assert.deepEqual(d.inbox.posts[0], { url: INBOX, secret: "s3cret-of-the-inbox", body: { source: "simplex", payload: { id: "m1", kind: "text", text: "text of m1", at: "2026-11-21T00:00:00Z", file: null, door: DOOR } } }, "to its own inbox, with the secret, as a webhook's delivery, naming its door");
  // the inbox refusing (a deploy, full): kept, and the next waits behind it
  d.inbox.status = 503;
  await d.deliver("m2");
  await d.deliver("m3");
  await d.o.alarm();
  const refused = d.posted().slice(1);
  assert.ok(refused.length >= 2 && refused.every((id) => id === "m2"), `m2 asked again at its watch, m3 never before it: ${refused}`);
  d.inbox.status = 200;
  const before = d.posted().length;
  await d.o.alarm();
  assert.deepEqual(d.posted().slice(before), ["m2", "m3"], "in order, at its watch");
  const kept = await (await d.lever("deliveries")).json();
  assert.deepEqual(kept.map((x) => [x.id, x.sent]), [["m1", 1], ["m2", 1], ["m3", 1]]);
  // its runner delivering one again (a start from an older save): taken once
  assert.equal((await (await d.deliver("m2")).json()).again, true);
  await d.o.alarm();
  assert.equal(d.posted().length, before + 2, "nothing sent again");
});

test("an ask a delivery's mark overtook is asked again under a new one; its runner's other refusals are its asker's", async () => {
  const d = door();
  await d.connect();
  const STALE = [409, { error: "door", message: "a stale mark: ask again under a new one" }];
  d.c.answers = [STALE, [200, { done: true }]];
  const r = await d.job("reply", { key: "chat:9", text: "hi" });
  assert.equal(r.status, 200, "asked again, and done");
  assert.deepEqual(d.c.asked.map((a) => a.path), ["/reply", "/reply"]);
  assert.ok(d.c.asked[1].mark > d.c.asked[0].mark, "under a newer mark");
  d.c.answers = [[409, { error: "door", message: "not linked" }]];
  const unlinked = await d.job("reply", { key: "chat:10", text: "hi" });
  assert.equal(unlinked.status, 409);
  assert.equal((await unlinked.json()).message, "not linked", "passed on as it said it");
  d.c.answers = [STALE, STALE, STALE];
  assert.equal((await d.job("reply", { key: "chat:11", text: "hi" })).status, 503, "overtaken every time: its asker tries again later");
});

test("its inbox behind by as many as it keeps, its runner is refused, and none it took is lost", async () => {
  const d = door();
  await d.connect();
  d.inbox.status = 503;
  for (let n = 0; n < 256; n += 1) assert.equal((await d.deliver(`m${n}`)).status, 200);
  const refused = await d.deliver("m256");
  assert.equal(refused.status, 503, "refused: its runner keeps its cursor on it");
  assert.equal((await refused.json()).error, "inbox_behind");
  d.inbox.status = 200;
  const before = d.posted().length;
  await d.o.alarm();
  assert.deepEqual(d.posted().slice(before), Array.from({ length: 256 }, (_, n) => `m${n}`), "every one it took, in order");
  assert.equal((await d.deliver("m256")).status, 200, "taken once its inbox caught up");
});

test("a link a delivery's mark overtook is not asked again once an unlink came after it", async () => {
  const d = door();
  await d.connect();
  const STALE = [409, { error: "door", message: "a stale mark: ask again under a new one" }];
  let unlinked;
  // the link's answer comes after an unlink asked meanwhile was taken
  d.c.answers = [async () => ((unlinked = await d.job("unlink", { n: 2 })), STALE)];
  const linked = await d.job("link", { n: 1 });
  assert.equal(unlinked.status, 202);
  assert.equal(linked.status, 409);
  assert.equal((await linked.json()).error, "overtaken");
  assert.deepEqual(d.c.asked.filter((a) => a.method === "POST").map((a) => a.path), ["/link", "/unlink"], "never a link after the unlink");
});

test("a link asked again after its unlink (a job's retry, a request of its own) is refused, its number older", async () => {
  const d = door();
  await d.connect();
  d.c.answers = [[503, { error: "door", message: "the door is starting or stopping" }]];
  assert.equal((await d.job("link", { n: 1 })).status, 503, "its runner starting: its job tries again later");
  assert.equal((await d.job("unlink", { n: 2 })).status, 202);
  const again = await d.job("link", { n: 1 });
  assert.equal(again.status, 409);
  assert.equal((await again.json()).error, "overtaken");
  assert.deepEqual(d.c.asked.filter((a) => a.method === "POST").map((a) => a.path), ["/link", "/unlink"], "never a link after the unlink");
  assert.equal((await d.job("link", { n: 3 })).status, 202, "a newer link goes");
});

// ---- its end (its connector deleted, unbound, or running other code) ----

// a connected door that saved: its connector's door, one save and a file
// of its own beside (the end deletes its whole prefix)
async function saved() {
  const d = door();
  assert.equal((await d.connect()).status, 200);
  const l = life(await d.restore(1));
  assert.equal((await d.save(l, 1)).status, 200);
  d.blobs.files.set(`doors/${DOOR}/saves/stray`, "a file a crash left");
  d.blobs.files.set(`doors/${NAME}--fedcba987654/saves/1-1`, "another life's door");
  return { d, l };
}

test("its end ends it: no last save, every file under its prefix gone, its storage whole gone, its watch let go; asked again, the same", async () => {
  const { d } = await saved();
  assert.equal(d.prefixed().length, 2);
  const r = await d.end();
  assert.equal(r.status, 200);
  assert.deepEqual(await r.json(), { ended: DOOR, files: 2 });
  assert.equal(d.c.destroyed, 1, "its container ended at once");
  assert.deepEqual(d.c.signals, [], "never asked to save last");
  assert.deepEqual(d.prefixed(), [], "every file of its own gone");
  assert.ok(d.blobs.files.has(`doors/${NAME}--fedcba987654/saves/1-1`), "another life's door's files untouched");
  assert.deepEqual(d.st.tables(), [], "its storage gone whole");
  assert.deepEqual(d.st.alarms, [], "its watch let go");
  assert.equal((await d.job("state")).status, 410, "ended, it answers its connector's asks no more");
  assert.deepEqual(d.st.tables(), [], "and keeps nothing for them");
  const again = await d.end();
  assert.deepEqual(await again.json(), { ended: DOOR, files: 0 }, "asked again, the same");
});

test("an upload under way across its end lets go of its own file, which nothing names", async () => {
  const d = door();
  await d.connect();
  const l = life(await d.restore(1));
  let holding, release;
  const held = new Promise((r) => (holding = r));
  const gate = new Promise((r) => (release = r));
  d.blobs.uploading = async () => {
    d.blobs.uploading = null;
    holding();
    await gate;
  };
  const upload = d.save(l, 1);
  await held;
  assert.equal((await d.end()).status, 200, "ended while its save uploads");
  release();
  assert.equal((await upload).status, 410, "the save is refused: its door ended");
  assert.deepEqual(d.prefixed(), [], "its file, landed after the end's deletion, gone too");
  assert.deepEqual(d.st.tables(), []);
});

test("a door already stopped ends the same: its saves and storage gone", async () => {
  const { d } = await saved();
  assert.equal((await d.lever("stop")).status, 200);
  assert.ok(d.row().stopped);
  assert.equal((await d.end()).status, 200);
  assert.deepEqual(d.prefixed(), []);
  assert.deepEqual(d.st.tables(), []);
});

test("a door never started keeps nothing at its end (a connector that never connected)", async () => {
  const d = door();
  assert.equal((await d.end()).status, 200);
  assert.deepEqual(d.st.tables(), [], "its schema, made as it began, gone too");
  assert.deepEqual(d.prefixed(), []);
});

test("ended, its connector's connect starts a new door, nothing of the old", async () => {
  const { d } = await saved();
  await d.end();
  const r = await d.connect();
  assert.equal(r.status, 200, "a new door, never refused for the old one's saves");
  assert.equal(d.row().saved, 0);
  assert.equal(d.row().stopped, null);
  assert.equal(life(await d.restore(d.row().started)), d.row().started + 1);
  assert.equal((await d.restore(d.row().started)).status, 204, "none to restore: a new door");
});

test("its last end (its connector's life over) is for good: a connect delivered late starts nothing, then or after a restart", async () => {
  const { d } = await saved();
  assert.equal((await d.end(DOOR, true)).status, 200);
  assert.deepEqual(d.prefixed(), []);
  const late = await d.connect();
  assert.equal(late.status, 410, "a connect sent before its end, come after it");
  assert.equal((await late.json()).error, "over");
  assert.equal((await d.start()).status, 410, "nor the lever's start");
  assert.equal(d.c.starts.length, 1, "no container started again");
  assert.deepEqual(d.st.tables(), ["over"], "its one row, all it keeps");
  // started again over its storage, as an eviction does
  const o = d.again();
  const call = (path, headers, body) => o.fetch(new Request(`http://door${path}`, { method: "POST", headers: { "x-fragment-release": "dev", ...headers }, body, duplex: "half" }));
  const r = await call("/job/connect", { "x-fragment-door": DOOR, "x-fragment-connector": NAME, "x-door-secret": "s3cret-of-the-inbox" }, JSON.stringify({ name: "Juniper" }));
  assert.equal(r.status, 410);
  assert.equal((await call("/door/end", {}, JSON.stringify({ door: DOOR, final: true }))).status, 200, "ended again, the same");
  assert.deepEqual(d.st.tables(), ["over"]);
  await o.alarm();
  assert.equal(d.c.starts.length, 1, "its alarm starts nothing");
  // an unbound connector's end is not its last: bound again, it connects
  const { d: e } = await saved();
  await e.end();
  assert.equal((await e.connect()).status, 200);
});

test("its end names its own door alone", async () => {
  const { d } = await saved();
  assert.equal((await d.end(`${NAME}--fedcba987654`)).status, 409, "another life's door is not this one");
  assert.equal((await d.end("not a door name!")).status, 400);
  assert.equal(d.prefixed().length, 2, "nothing ended");
  assert.ok(d.st.tables().includes("door"));
});

test("a file its runner received is held for its connector: once, from its running life alone, while there is room", async () => {
  const d = door();
  await d.connect();
  const life1 = life(await d.restore(d.row().started));
  assert.equal((await d.give(life1, sha(1), "voice bytes")).status, 200);
  assert.deepEqual(await (await d.give(life1, sha(1), "voice bytes")).json(), { kept: sha(1), again: true }, "given again: held once");
  assert.equal((await d.give(life1 + 1, sha(2), "x")).status, 409, "another life's: refused");
  assert.equal((await d.give(life1, "nope", "x")).status, 400, "a file named by no SHA-256");
  assert.equal((await d.call("PUT", `/egress/file/${sha(3)}`, { "x-door-generation": String(life1) })).status, 400, "no bytes, no length: refused");
  // its connector keeps it as one of its blobs: its bytes, its type, named
  const r = await d.jobFile("GET", sha(1));
  assert.equal(r.status, 200);
  assert.deepEqual([r.headers.get("x-door-sha256"), r.headers.get("content-type"), r.headers.get("content-length"), await r.text()], [sha(1), "audio/mp4", "11", "voice bytes"]);
  assert.equal((await d.jobFile("GET", sha(9))).status, 404, "one it does not hold");
  assert.equal((await d.call("GET", `/job/file/${sha(1)}`, { "x-fragment-door": `${NAME}--fedcba987654`, "x-fragment-connector": NAME })).status, 409, "another life's connector: refused");
  // as many as it may hold: a file nothing uses any more (its delivery
  // kept) makes room; one still used never does; with none unused, a new
  // one is refused until older ones go
  assert.deepEqual(uses(d, 1), ["given"], "given: its runner's use, until a delivery names it");
  for (let n = 10; n < 10 + 62; n += 1) assert.equal((await d.give(life1, sha(n), "x")).status, 200, `file ${n}`);
  await used(d, life1, 80);
  assert.equal((await d.give(life1, sha(98), "x")).status, 200, "the one nothing uses made room");
  assert.deepEqual(d.st.sql.exec("SELECT sha FROM files WHERE sha = ?", sha(80)).toArray(), [], "it was let go");
  assert.ok(!d.prefixed().includes(`doors/${DOOR}/files/${sha(80)}`), "its file too");
  assert.ok(d.st.sql.exec("SELECT 1 FROM files WHERE sha = ?", sha(1)).toArray().length, "read by its connector, never said kept: still used, held");
  const full = await d.give(life1, sha(99), "x");
  assert.deepEqual([full.status, (await full.json()).error], [503, "files_full"], "every one still used: refused");
  // a day on, its watch lets them go, rows and files
  d.st.sql.exec("UPDATE files SET at = at - ?", 24 * 60 * 60 * 1000);
  await d.o.alarm();
  assert.equal(d.st.sql.exec("SELECT count(*) AS n FROM files").one().n, 0);
  assert.deepEqual(d.prefixed().filter((k) => k.includes("/files/")), [], "their files gone too");
  assert.equal((await d.give(life1, sha(99), "x")).status, 200, "room again");
});

test("files on their way in hold their room: two given at once past it, one is refused", async () => {
  const d = door();
  await d.connect();
  const life1 = life(await d.restore(d.row().started));
  for (let n = 10; n < 10 + 63; n += 1) await d.give(life1, sha(n), "x");
  // the first held mid-upload while the second is given
  let release;
  const held = new Promise((r) => (release = r));
  d.blobs.uploading = () => held;
  const first = d.give(life1, sha(1), "one");
  await new Promise((r) => setTimeout(r, 10));
  d.blobs.uploading = null;
  const second = await d.give(life1, sha(2), "two");
  release();
  assert.equal((await first).status, 200);
  assert.deepEqual([second.status, (await second.json()).error], [503, "files_full"], "the second refused: the first held its room");
  assert.equal(d.st.sql.exec("SELECT count(*) AS n FROM files").one().n, 64, "never past its bound");
  // the same file given twice at once (room for one, a file none uses):
  // the second asked again later, "uploading"
  await d.call("POST", "/egress/deliver", {}, JSON.stringify({ id: "u10", kind: "file", text: "", at: "2026-11-24T00:00:00Z", file: { sha256: sha(10), name: "x", type: "application/octet-stream", size: 1 } }));
  await d.job("kept", { id: "u10" });
  d.blobs.uploading = () => new Promise((r) => setTimeout(r, 20));
  const [a, b] = await Promise.all([d.give(life1, sha(3), "three"), d.give(life1, sha(3), "three")]);
  assert.deepEqual([a.status, b.status, (await b.json()).error], [200, 503, "uploading"]);
});

test("every delivery its inbox has yet to take keeps its file past a day", async () => {
  const d = door();
  await d.connect();
  const life1 = life(await d.restore(d.row().started));
  d.inbox.status = 503;
  for (const n of [1, 2]) {
    await d.give(life1, sha(n), `voice ${n}`);
    await d.call("POST", "/egress/deliver", {}, JSON.stringify({ id: `v${n}`, kind: "voice", text: "", at: "2026-11-24T00:00:00Z", file: { sha256: sha(n), name: "memo.m4a", type: "audio/mp4", size: 7 } }));
  }
  await d.give(life1, sha(3), "named by no delivery");
  d.st.sql.exec("UPDATE files SET at = at - ?", 24 * 60 * 60 * 1000);
  await d.o.alarm();
  assert.deepEqual(d.st.sql.exec("SELECT sha FROM files ORDER BY sha").toArray().map((f) => f.sha), [sha(1), sha(2)], "both waiting deliveries kept theirs, the oldest and the one behind it; the other went");
  d.inbox.status = 200;
  await d.o.alarm();
  for (const n of [1, 2]) assert.equal((await d.jobFile("GET", sha(n))).status, 200, `its connector keeps file ${n} once its inbox takes the delivery`);
});

test("a reply's file is used until its reply is done, whatever its runner reads; said done again, another reply's use stays", async () => {
  const d = door();
  await d.connect();
  const give = (key) => d.call("PUT", `/job/file/${sha(5)}`, { "x-fragment-door": DOOR, "x-fragment-connector": NAME, "content-type": "application/pdf", "content-length": "4", "x-door-for": key }, "%PDF");
  await give("chat:9");
  assert.deepEqual(uses(d, 5), ["r:chat:9"], "given for its reply");
  await d.call("GET", `/egress/file/${sha(5)}`);
  assert.deepEqual(uses(d, 5), ["r:chat:9"], "its runner's read ends nothing (a reply read again after a sync finds it)");
  await give("chat:10");
  assert.deepEqual(uses(d, 5), ["r:chat:10", "r:chat:9"], "another reply's too");
  const files = [{ sha256: sha(5), name: "plan.pdf", type: "application/pdf", size: 4 }];
  d.c.answers.push([200, { files: true }], [202, { done: false }]);
  await d.job("reply", { key: "chat:9", text: "", files });
  assert.deepEqual(uses(d, 5), ["R:chat:9", "r:chat:10"], "not yet done: taken by its runner, its use its runner's");
  for (let n = 0; n < 2; n += 1) {
    d.c.answers.push([200, { files: true }], [200, { done: true }]);
    await d.job("reply", { key: "chat:9", text: "", files });
  }
  assert.deepEqual(uses(d, 5), ["r:chat:10"], "done, said twice: its own use ended, the other reply's stays");
  // a full door: a file still used is never let go for room
  const life1 = life(await d.restore(d.row().started));
  for (let n = 10; n < 10 + 63; n += 1) await d.give(life1, sha(n), "x");
  const full = await d.give(life1, sha(99), "x");
  assert.deepEqual([full.status, (await full.json()).error], [503, "files_full"]);
});

const replyFileOf = (d, n, key) => d.call("PUT", `/job/file/${sha(n)}`, { "x-fragment-door": DOOR, "x-fragment-connector": NAME, "content-type": "application/pdf", "content-length": "4", "x-door-for": key }, "%PDF");
const replyFiles1 = (n) => [{ sha256: sha(n), name: "plan.pdf", type: "application/pdf", size: 4 }];

test("a reply sent as its text alone (a runner that carries no files) lets go of its files once done: a full door takes a new file after (the review of #1538)", async () => {
  const d = door();
  await d.connect();
  // as many replies' files as it holds, each refused by a runner that
  // carries none, then sent as its text alone under the same key, done
  for (let n = 1; n <= 64; n += 1) {
    assert.equal((await replyFileOf(d, n, `chat:${n}`)).status, 200, `file ${n}`);
    d.c.answers.push([200, { phase: "linked" }]);
    assert.equal((await d.job("reply", { key: `chat:${n}`, text: "here", files: replyFiles1(n) })).status, 409, "its runner carries no files");
    d.c.answers.push([200, { done: true }]);
    assert.equal((await d.job("reply", { key: `chat:${n}`, text: "here", files: [] })).status, 200);
  }
  assert.equal(d.st.sql.exec("SELECT count(*) AS n FROM uses").one().n, 0, "no done reply's use left");
  const taken = await replyFileOf(d, 99, "chat:99");
  assert.equal(taken.status, 200, `a full door makes room of files nothing uses: ${await taken.clone().text()}`);
});

test("a reply its runner says ended (410: its link ended first) lets go of its files, as its text alone or with them; one pending, or another reply's, keeps its own (the review of #1538)", async () => {
  const d = door();
  await d.connect();
  await replyFileOf(d, 1, "chat:1");
  d.c.answers.push([200, { phase: "linked" }]);
  await d.job("reply", { key: "chat:1", text: "here", files: replyFiles1(1) });
  d.c.answers.push([410, { done: false, ended: true }]);
  assert.equal((await d.job("reply", { key: "chat:1", text: "here", files: [] })).status, 410);
  assert.deepEqual(uses(d, 1), [], "ended as its text alone: let go");
  await replyFileOf(d, 2, "chat:2");
  await replyFileOf(d, 2, "chat:3");
  d.c.answers.push([200, { files: true }], [202, { done: false }]);
  await d.job("reply", { key: "chat:2", text: "", files: replyFiles1(2) });
  assert.deepEqual(uses(d, 2), ["R:chat:2", "r:chat:3"], "pending: kept, its runner's");
  d.c.answers.push([200, { files: true }], [410, { done: false, ended: true }]);
  assert.equal((await d.job("reply", { key: "chat:2", text: "", files: replyFiles1(2) })).status, 410);
  assert.deepEqual(uses(d, 2), ["r:chat:3"], "ended with its files: its own use let go, the other reply's kept");
});

test("a reply its runner refuses for good (not linked, or not there to ask) lets go of its files: its connector asks no more, and a full door takes a new file after (the fix-check of #1538)", async () => {
  const d = door();
  await d.connect();
  // as many replies' files as it holds, each refused: its link unlinked
  // (409 not linked), or its runner not up to say (its state unread)
  for (let n = 1; n <= 64; n += 1) {
    assert.equal((await replyFileOf(d, n, `chat:${n}`)).status, 200, `file ${n}`);
    if (n % 2) d.c.answers.push([200, { phase: "off", files: true }], [409, { error: "not_linked" }]);
    else d.c.answers.push([503, { error: "starting" }]);
    const r = await d.job("reply", { key: `chat:${n}`, text: "here", files: replyFiles1(n) });
    assert.ok([409, 503].includes(r.status), `refused: ${r.status}`);
  }
  assert.equal(d.st.sql.exec("SELECT count(*) AS n FROM uses").one().n, 0, "no refused reply's use left");
  const taken = await replyFileOf(d, 99, "chat:99");
  assert.equal(taken.status, 200, `a full door makes room of files nothing uses: ${await taken.clone().text()}`);
});

test("a reply its runner took (202) keeps its files across an answer that says nothing (its state unread, 503), however full the door, until its runner says it is done (the review of #1538)", async () => {
  const d = door();
  await d.connect();
  assert.equal((await replyFileOf(d, 1, "chat:1")).status, 200);
  d.c.answers.push([200, { phase: "linked", files: true }], [202, { done: false }]);
  assert.equal((await d.job("reply", { key: "chat:1", text: "", files: replyFiles1(1) })).status, 202, "taken by its runner");
  // its runner restarting: its state unread on the next poll
  d.c.answers.push([503, { error: "starting" }]);
  assert.equal((await d.job("reply", { key: "chat:1", text: "", files: replyFiles1(1) })).status, 503);
  // a full door makes room of files nothing uses: never that one
  for (let n = 2; n <= 70; n += 1) {
    await replyFileOf(d, n, `chat:${n}`);
    d.c.answers.push([200, { phase: "off", files: true }], [409, { error: "not_linked" }]);
    await d.job("reply", { key: `chat:${n}`, text: "", files: replyFiles1(n) });
  }
  const read = await d.call("GET", `/egress/file/${sha(1)}`);
  assert.deepEqual([read.status, await read.text()], [200, "%PDF"], "its runner, started again, reads its bytes");
  d.c.answers.push([200, { phase: "linked", files: true }], [200, { done: true }]);
  assert.equal((await d.job("reply", { key: "chat:1", text: "", files: replyFiles1(1) })).status, 200);
  assert.deepEqual(uses(d, 1), [], "done: let go");
});

test("an old file a waiting delivery keeps counts toward the door's bounds (the review of #1538)", async () => {
  const d = door();
  await d.connect();
  const life1 = life(await d.restore(d.row().started));
  d.inbox.status = 503;
  await d.give(life1, sha(1), "voice");
  await d.call("POST", "/egress/deliver", {}, JSON.stringify({ id: "v1", kind: "voice", text: "", at: "2026-11-24T00:00:00Z", file: { sha256: sha(1), name: "memo.m4a", type: "audio/mp4", size: 5 } }));
  d.st.sql.exec("UPDATE files SET at = at - ?", 24 * 60 * 60 * 1000);
  await d.o.alarm();
  for (let n = 10; n < 10 + 63; n += 1) assert.equal((await d.give(life1, sha(n), "x")).status, 200, `file ${n}`);
  const past = await d.give(life1, sha(99), "x");
  assert.deepEqual([past.status, (await past.json()).error], [503, "files_full"], "64 held, the old one among them: no 65th");
  assert.equal(d.st.sql.exec("SELECT count(*) AS n FROM files").one().n, 64);
});

test("one file two messages name: said kept for the first, twice, still there for the second however full the door (the review of #1538)", async () => {
  const d = door();
  await d.connect();
  const life1 = life(await d.restore(d.row().started));
  d.inbox.status = 503;
  const file = { sha256: sha(1), name: "memo.m4a", type: "audio/mp4", size: 5 };
  for (const id of ["v1", "v2"]) {
    await d.give(life1, sha(1), "voice");
    await d.call("POST", "/egress/deliver", {}, JSON.stringify({ id, kind: "voice", text: "", at: "2026-11-24T00:00:00Z", file }));
  }
  d.inbox.status = 200;
  await d.o.alarm();
  assert.deepEqual(d.st.sql.exec("SELECT id, sent FROM deliveries ORDER BY mark").toArray(), [{ id: "v1", sent: 1 }, { id: "v2", sent: 1 }], "both taken by its inbox, neither kept yet");
  assert.deepEqual(uses(d, 1), ["d:v1", "d:v2"], "each delivery's use, its runner's gives become theirs");
  assert.equal((await d.jobFile("GET", sha(1))).status, 200, "the first kept");
  for (let n = 0; n < 2; n += 1) await d.job("kept", { id: "v1" });
  assert.deepEqual(uses(d, 1), ["d:v2"], "said kept twice for the first: the second's use stays");
  // full of files nothing uses, newer than it
  for (let n = 10; n < 10 + 63; n += 1) await used(d, life1, n);
  for (let n = 90; n < 95; n += 1) assert.equal((await d.give(life1, sha(n), "x")).status, 200, `room made for ${n}`);
  assert.equal((await d.jobFile("GET", sha(1))).status, 200, "the second still kept: never let go while a message uses it");
});

test("a keep cut off before its connector has the file: never let go for room, read again whole (the review of #1538)", async () => {
  const d = door();
  await d.connect();
  const life1 = life(await d.restore(d.row().started));
  await d.give(life1, sha(1), "voice bytes");
  await d.call("POST", "/egress/deliver", {}, JSON.stringify({ id: "v1", kind: "voice", text: "", at: "2026-11-24T00:00:00Z", file: { sha256: sha(1), name: "memo.m4a", type: "audio/mp4", size: 11 } }));
  await d.o.alarm();
  // its connector's read, cut off: its answer opened and dropped, no kept said
  const cut = await d.jobFile("GET", sha(1));
  await cut.body.cancel();
  for (let n = 10; n < 10 + 63; n += 1) await used(d, life1, n);
  for (let n = 90; n < 95; n += 1) assert.equal((await d.give(life1, sha(n), "x")).status, 200, `room made for ${n}`);
  const again = await d.jobFile("GET", sha(1));
  assert.deepEqual([again.status, await again.text()], [200, "voice bytes"], "read again, whole");
  await d.job("kept", { id: "v1" });
  assert.deepEqual(uses(d, 1), [], "kept: nothing uses it");
  d.st.sql.exec("UPDATE files SET at = (SELECT min(at) FROM files) - 1 WHERE sha = ?", sha(1));
  await d.give(life1, sha(99), "x");
  assert.deepEqual(d.st.sql.exec("SELECT sha FROM files WHERE sha = ?", sha(1)).toArray(), [], "then it makes room, the oldest none uses");
});

test("a delivery names its file by its SHA-256; a file named otherwise (an older runner's) goes as none", async () => {
  const d = door();
  await d.connect();
  const life1 = life(await d.restore(d.row().started));
  await d.give(life1, sha(1), "voice bytes");
  const file = { sha256: sha(1), name: "memo.m4a", type: "audio/mp4", size: 11 };
  await d.call("POST", "/egress/deliver", {}, JSON.stringify({ id: "v1", kind: "voice", text: "", at: "2026-11-24T00:00:00Z", file }));
  await d.call("POST", "/egress/deliver", {}, JSON.stringify({ id: "v2", kind: "voice", text: "", at: "2026-11-24T00:00:00Z", file: { name: "memo.m4a", size: 11 } }));
  await d.o.alarm();
  assert.deepEqual(d.inbox.posts.map((p) => [p.body.payload.id, p.body.payload.file]), [["v1", file], ["v2", null]]);
});

test("a reply's files: given by its connector, then sent to a runner that carries files; one it does not hold, or a runner that carries none, refused", async () => {
  const d = door();
  await d.connect();
  assert.equal((await d.jobFile("PUT", sha(5), "%PDF-1.7")).status, 200);
  const files = [{ sha256: sha(5), name: "plan.pdf", type: "application/pdf", size: 8 }];
  const no = await d.job("reply", { key: "chat:8", text: "here", files: [{ ...files[0], sha256: sha(6) }] });
  assert.deepEqual([no.status, (await no.json()).error], [409, "no_file"], "a file it does not hold");
  d.c.answers.push([200, { phase: "linked" }]);
  const old = await d.job("reply", { key: "chat:8", text: "here", files });
  assert.deepEqual([old.status, (await old.json()).error], [409, "no_files"], "a runner that carries no files: its connector sends the text alone");
  d.c.answers.push([200, { phase: "linked", files: true }]);
  assert.equal((await d.job("reply", { key: "chat:8", text: "here", files })).status, 202);
  const asked = d.c.asked.filter((a) => a.path === "/reply");
  assert.deepEqual(asked.map((a) => a.body), [{ key: "chat:8", text: "here", files }], "the runner asked once, with its files");
  assert.equal((await d.job("reply", { key: "chat:8", text: "here", files: Array.from({ length: 9 }, () => files[0]) })).status, 400, "past a message's files");
  // the file it holds is its runner's to read
  const r = await d.call("GET", `/egress/file/${sha(5)}`);
  assert.deepEqual([r.status, await r.text()], [200, "%PDF-1.7"]);
});

test("its end lets go of the files it holds with the rest", async () => {
  const d = door();
  await d.connect();
  const life1 = life(await d.restore(d.row().started));
  await d.give(life1, sha(1), "voice bytes");
  await d.jobFile("PUT", sha(2), "%PDF");
  assert.equal(d.prefixed().filter((k) => k.includes("/files/")).length, 2);
  await d.end();
  assert.deepEqual(d.prefixed(), [], "every file under its prefix gone");
});

test("the e2e's lever gives a file as its runner does, and hashes again what it holds", async () => {
  const d = door();
  await d.connect();
  const given = await d.lever("file", { door: DOOR, bytes: btoa("voice bytes"), type: "audio/mp4" });
  assert.equal(given.status, 200, await given.clone().text());
  const { sha256, size } = await given.json();
  assert.equal(size, 11);
  assert.deepEqual(d.st.sql.exec("SELECT owner FROM uses WHERE sha = ?", sha256).toArray().map((u) => u.owner), ["given"], "its runner's use, until the delivery naming it is made");
  const looked = await (await d.lever("file", { door: DOOR, sha: sha256 })).json();
  assert.deepEqual(looked, { sha256, size: 11, type: "audio/mp4" });
});

test("a door that never saved begins from a save it is given, and starts only when its connector connects", async () => {
  const d = door();
  const given = await d.lever("import", { door: DOOR, agent: "Juniper", bytes: btoa("a packed save") });
  assert.equal(given.status, 200, await given.clone().text());
  const { imported } = await given.json();
  assert.match(imported, /^[0-9a-f]{64}$/);
  assert.deepEqual(d.row(), { started: 0, generation: 0, saved: 1, stopped: null }, "its row, as though it had saved");
  assert.deepEqual(d.saves(), [{ id: "0-1", unreadable: 0 }]);
  assert.deepEqual(d.prefixed(), [`doors/${DOOR}/saves/0-1`]);
  assert.deepEqual(d.c.starts, [], "not started: its connector's connect starts it");
  await d.connect();
  assert.equal(d.c.starts.length, 1, "its connector's connect starts it");
  const r = await d.restore(d.row().started);
  assert.deepEqual([r.status, r.headers.get("x-door-save"), r.headers.get("x-door-behind"), await r.text()], [200, "0-1", "0", "a packed save"], "its runner restores the save it was given, in step");
});

test("a save given again is answered the same; another, or one to a door that saved, is refused; a door over takes none", async () => {
  const d = door();
  const give = (bytes, name = DOOR) => d.lever("import", { door: name, agent: "Juniper", bytes: btoa(bytes) });
  assert.equal((await give("first")).status, 200);
  assert.equal((await (await give("first")).json()).again, true, "given again: the same");
  const other = await give("second");
  assert.deepEqual([other.status, (await other.json()).error], [409, "imported"], "another save: refused, the first kept");
  assert.deepEqual(d.saves(), [{ id: "0-1", unreadable: 0 }]);
  // a door that started and saved of its own
  const e = door();
  await e.connect();
  const life1 = life(await e.restore(e.row().started));
  await e.save(life1, 1);
  const late = await e.lever("import", { door: DOOR, agent: "Juniper", bytes: btoa("late") });
  assert.deepEqual([late.status, (await late.json()).error], [409, "has_saves"], "a door that saved takes none");
  // a door whose connector's life is over
  const f = door();
  await f.connect();
  await f.end(DOOR, true);
  assert.equal((await f.lever("import", { door: DOOR, agent: "Juniper", bytes: btoa("x") })).status, 410);
});

test("a save given whose bytes are not the SHA-256 it names is never kept", async () => {
  const d = door();
  const bytes = "a packed save";
  const wrong = "ab".repeat(32);
  const r = await d.call("PUT", "/door/import", { "x-fragment-door": DOOR, "x-door-agent": "Juniper", "x-door-sha256": wrong, "x-door-length": String(bytes.length) }, bytes);
  assert.deepEqual([r.status, (await r.json()).error], [422, "not_its_sha"]);
  assert.deepEqual([d.saves(), d.prefixed(), d.row()], [[], [], undefined], "no row, no save, no file");
  // the same bytes under their own SHA-256: taken
  assert.equal((await d.lever("import", { door: DOOR, agent: "Juniper", bytes: btoa(bytes) })).status, 200);
});

test("the way back: a planned stop, its runner's last save, and that save handed back; again the same", async () => {
  const d = door();
  const back = (name = DOOR) => d.call("POST", "/door/handback", {}, JSON.stringify({ door: name }));
  await d.connect();
  const life1 = life(await d.restore(d.row().started));
  await d.save(life1, 1, "its first");
  // its runner's planned stop: it saves last, then ends
  d.c.signal = async (n) => {
    d.c.signals.push(n);
    await d.save(life1, 2, "its last");
    d.c.running = false;
  };
  assert.deepEqual([(await back("juniper-simplex--0000--0123456789ab")).status], [409], "another door's");
  const r = await back();
  assert.deepEqual([r.status, r.headers.get("x-door-save"), r.headers.get("x-door-behind"), await r.text()], [200, `${life1}-2`, "0", "its last"]);
  assert.deepEqual(d.c.signals, [15], "a planned stop");
  assert.equal(d.row().stopped, "handed back", "and not started again");
  const again = await back();
  assert.deepEqual([again.status, await again.text()], [200, "its last"], "asked again: the same");
  assert.deepEqual(d.c.signals, [15], "stopped once");
  assert.equal(d.c.starts.length, 1, "never started again");
  // its state, as the operators' move reads it
  const s = await (await d.call("POST", "/door/state", {}, "{}")).json();
  assert.equal(s.door.stopped, "handed back");
  assert.equal(r.headers.get("x-door-started"), "1", "a door that ran");
  // never started again, whatever comes late: its computer runs that link now
  const late = await d.connect();
  assert.deepEqual([late.status, (await late.json()).error], [409, "handed_back"], "a connect after it");
  assert.equal((await d.start()).status, 409, "nor the lever's start");
  assert.equal(d.c.starts.length, 1, "never started again");
});

test("the way back waits while its owner's messages still go to its connector, never a save past a message no one has", async () => {
  const d = door();
  await d.connect();
  const life1 = life(await d.restore(d.row().started));
  d.inbox.status = 503;
  await d.deliver("m1");
  await d.save(life1, 1, "its save, past m1");
  d.c.signal = async () => void (d.c.running = false);
  const busy = await d.call("POST", "/door/handback", {}, JSON.stringify({ door: DOOR }));
  assert.deepEqual([busy.status, (await busy.json()).error], [409, "busy"], "m1 not yet its connector's");
  assert.equal(d.row().stopped, "handed back", "stopped all the same: nothing new comes");
  d.inbox.status = 200;
  const r = await d.call("POST", "/door/handback", {}, JSON.stringify({ door: DOOR }));
  assert.deepEqual([r.status, await r.text()], [200, "its save, past m1"], "asked again, m1 sent on first");
  assert.equal(d.st.sql.exec("SELECT sent FROM deliveries WHERE id = 'm1'").one().sent, 1, "m1 its connector's");
  // a door given a save and never started: its save is that one, and says so
  const e = door();
  await e.lever("import", { door: DOOR, agent: "Juniper", bytes: btoa("a link given") });
  const given = await e.call("POST", "/door/handback", {}, JSON.stringify({ door: DOOR }));
  assert.deepEqual([given.status, given.headers.get("x-door-save"), given.headers.get("x-door-started")], [200, "0-1", "0"]);
  // fenced first, a staged chat's let-go included: no new chat begins
  const late = await e.job("connect", { name: "Juniper", keep: false }, { "x-door-secret": "s3cret-of-the-inbox" });
  assert.deepEqual([late.status, (await late.json()).error], [409, "handed_back"]);
  assert.deepEqual([e.saves().length, e.c.starts.length], [1, 0], "its save kept, nothing started");
  // one never begun (its save not yet given, a move racing its way back):
  // fenced too, taking no save and starting nothing after
  const f = door();
  assert.equal((await f.call("POST", "/door/handback", {}, JSON.stringify({ door: DOOR }))).status, 204);
  const after = await f.lever("import", { door: DOOR, agent: "Juniper", bytes: btoa("too late") });
  assert.deepEqual([after.status, (await after.json()).error], [409, "handed_back"]);
  assert.equal((await f.connect()).status, 409);
  assert.deepEqual([f.saves(), f.c.starts.length], [[], 0]);
});

test("a door given its owner's chat with their old agent starts only at their word: kept, it restores it; let go, it begins anew", async () => {
  const d = door();
  assert.equal((await d.lever("import", { door: DOOR, agent: "Juniper", bytes: btoa("their old chat"), staged: true })).status, 200);
  assert.equal((await (await d.call("POST", "/door/state", {}, "{}")).json()).staged, true, "its state says it waits");
  const asked = await d.connect();
  assert.deepEqual([asked.status, (await asked.json()).error], [409, "staged"], "a connect that says nothing of it");
  assert.deepEqual([d.c.starts.length, d.st.sql.exec("SELECT 1 FROM inbox").toArray().length], [0, 0], "nothing started, nothing kept");
  const kept = await d.job("connect", { name: "Juniper", keep: true }, { "x-door-secret": "s3cret-of-the-inbox" });
  assert.equal(kept.status, 200, await kept.clone().text());
  assert.equal(d.c.starts.length, 1, "kept: started");
  const r = await d.restore(d.row().started);
  assert.deepEqual([r.status, r.headers.get("x-door-save"), await r.text()], [200, "0-1", "their old chat"], "from the chat it was given (its runner adopts it)");
  assert.equal((await (await d.call("POST", "/door/state", {}, "{}")).json()).staged, false);
  // let go: a new chat instead
  const e = door();
  await e.lever("import", { door: DOOR, agent: "Juniper", bytes: btoa("their old chat"), staged: true });
  const fresh = await e.job("connect", { name: "Juniper", keep: false }, { "x-door-secret": "s3cret-of-the-inbox" });
  assert.equal(fresh.status, 200, await fresh.clone().text());
  assert.deepEqual([e.saves(), e.prefixed(), e.row().saved], [[], [], 0], "its chat let go, rows and file");
  assert.equal((await e.restore(e.row().started)).status, 204, "a new door: a new profile");
  // a moved link (not staged) starts at its connect, as any door
  const f = door();
  await f.lever("import", { door: DOOR, agent: "Juniper", bytes: btoa("a linked link") });
  assert.equal((await f.connect()).status, 200);
});
