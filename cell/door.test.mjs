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
  b.put = async (k, v) => {
    const bytes = typeof v === "string" ? v : await new Response(v).text();
    if (b.uploading) await b.uploading();
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
  // its end, as the platform asks it (its connector deleted or unbound)
  d.end = (name = DOOR) => call("POST", "/door/end", {}, JSON.stringify({ door: name }));
  d.prefixed = (name = DOOR) => [...blobs.files.keys()].filter((k) => k.startsWith(`doors/${name}/`));
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

test("its end names its own door alone", async () => {
  const { d } = await saved();
  assert.equal((await d.end(`${NAME}--fedcba987654`)).status, 409, "another life's door is not this one");
  assert.equal((await d.end("not a door name!")).status, 400);
  assert.equal(d.prefixed().length, 2, "nothing ended");
  assert.ok(d.st.tables().includes("door"));
});
