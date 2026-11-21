// A SimpleX door (docs/computers.md, "F: messengers outside the computer"):
// one Durable Object per door, holding a `lite` container that runs
// SimpleX's core and the door's runner (images/simplex-door), always on.
// JavaScript in the cell, no Rust in its Wasm, so only doors carry its
// weight: it moves to a Worker of its own once two Workers deploy in
// lockstep (docs/technical-debt-ledger.md). Its rules are door-rules.mjs;
// this is their I/O.
//
// What it keeps:
// - its saves: each a file in the bucket under `doors/<door>/saves/`, and
//   a row here naming it (its start, its seq, its mark), the newest three
//   readable ones kept (Paul, 2026-11-20);
// - its door row: its name, its agent's, the start running (generation),
//   the newest effect it numbered (marks), and whether it is stopped, why;
// - its owner's messages, the newest `DELIVERIES_KEPT`, until its
//   connector takes them on (D2).
//
// Who reaches it: the platform, as any object, through `routed::send`
// (cell/src/levers.rs, the e2e's lever: `door/lever`; a deploy's probe),
// each call's release checked first; and its own container, whose calls to
// `door.fragment.internal` its egress (`DoorEgress`) brings here
// (`/egress/…`). Its runner's protocol is images/simplex-door/src/door.rs.
import { DurableObject, WorkerEntrypoint } from "cloudflare:workers";
import { settled, startIntercepted } from "./intercepts.mjs";
import { DELIVERIES_KEPT, behind, fenced, nameOk, parseSaveId, restoreOf, saveId, saveKey, startable, steps, surplus } from "./door-rules.mjs";

const HOST = "door.fragment.internal";
const PORT = 8080;
// Its container watched this often (its exit is told at once; this is the
// net under that, and a door's restart after its object was evicted).
const WATCH_MS = 30_000;
// What a delivery's body, or a lever's, may be (a message's text is at
// most 32 KiB: images/bridge's wire::TEXT_MAX_BYTES, JSON-escaped).
const BODY_MAX = 256 * 1024;
// The release headers (fragment_core::release): a call from another
// release is refused before anything is done.
const RELEASE = "x-fragment-release";
const REFUSED = "x-fragment-release-refused";

const json = (v, status = 200, headers = {}) => new Response(JSON.stringify(v), { status, headers: { "content-type": "application/json", ...headers } });
const refusal = (status, error, message) => json({ error, message }, status);
const own = (env) => env.FRAGMENT_DEPLOY_ID || "dev";

async function bounded(request) {
  const text = await request.text();
  if (text.length > BODY_MAX) throw Object.assign(new Error("a body past its bound"), { status: 413 });
  return text ? JSON.parse(text) : {};
}

export class SimplexDoor extends DurableObject {
  #watching = 0;
  #starting = null;
  // the save keys an upload holds, one upload each
  #uploading = new Set();

  constructor(ctx, env) {
    super(ctx, env);
    ctx.blockConcurrencyWhile(async () => this.#migrate());
  }

  #migrate() {
    const sql = this.ctx.storage.sql;
    sql.exec("CREATE TABLE IF NOT EXISTS door_schema (version INTEGER NOT NULL)");
    const row = sql.exec("SELECT version FROM door_schema").toArray()[0];
    if (!row) sql.exec("INSERT INTO door_schema (version) VALUES (0)");
    this.ctx.storage.transactionSync(() => {
      for (const step of steps(row ? row.version : 0)) {
        sql.exec(step.sql);
        sql.exec("UPDATE door_schema SET version = ?", step.version);
      }
    });
  }

  #door() {
    return this.ctx.storage.sql.exec("SELECT door, agent, started, generation, marks, saved, stopped FROM door WHERE id = 1").toArray()[0] ?? null;
  }

  #saves() {
    return this.ctx.storage.sql.exec("SELECT generation, seq, mark, bytes, at, unreadable FROM saves").toArray();
  }

  // An effect numbered: what the door asks of its runner, or takes from it.
  #mark() {
    this.ctx.storage.sql.exec("UPDATE door SET marks = marks + 1 WHERE id = 1");
    return this.#door().marks;
  }

  #log(event, fields = {}) {
    console.log(JSON.stringify({ event, door: this.#door()?.door, ...fields }));
  }

  async fetch(request) {
    // another release's call is refused, before anything is read or done
    if (request.headers.get(RELEASE) !== own(this.env)) {
      const r = refusal(503, "deploying", `a call from release ${request.headers.get(RELEASE)} into ${own(this.env)}`);
      r.headers.set(REFUSED, own(this.env));
      return r;
    }
    const url = new URL(request.url);
    try {
      // a deploy's probe: the release checked, the schema taken (in the
      // constructor), nothing else (cell/src/routed.rs `PROBED`)
      if (url.pathname === "/__probe" && request.headers.has("x-fragment-probe")) return json({ ready: true });
      if (url.pathname === "/door/lever" && request.method === "POST") return await this.#lever(await bounded(request));
      if (url.pathname === "/egress/save" && request.method === "GET") return await this.#restore(request);
      if (url.pathname === "/egress/save" && request.method === "PUT") return await this.#save(request, url);
      if (url.pathname === "/egress/deliver" && request.method === "POST") return this.#deliver(await bounded(request));
      if (url.pathname === "/egress/unreadable" && request.method === "POST") return this.#unreadable(request, await bounded(request));
      return refusal(404, "not_found", `no door route ${request.method} ${url.pathname}`);
    } catch (e) {
      return refusal(e.status ?? 500, e.status ? "invalid" : "internal", String((e && e.message) || e).slice(0, 300));
    }
  }

  async alarm() {
    try {
      await this.#ensure("alarm");
    } catch (e) {
      this.#log("door.start_failed", { why: String((e && e.message) || e).slice(0, 300) });
    }
    if (this.#door() && !this.#door().stopped) await this.ctx.storage.setAlarm(Date.now() + WATCH_MS);
  }

  // ---- its container ----

  // Its container running, one start at a time: a new start's generation,
  // its egress caught before it starts, its exit watched.
  async #ensure(why) {
    while (this.#starting) await this.#starting;
    this.#starting = this.#start(why).finally(() => (this.#starting = null));
    return this.#starting;
  }

  // Each start takes a generation of its own (`started`), and each runner
  // life within it another when it restores (`generation`: a process
  // Docker restarts in place is a new life), so no save's key is ever
  // reused and a life superseded is fenced off.
  async #start(why) {
    const c = this.ctx.container;
    if (!c) throw new Error("this deployment runs no doors (no container for them)");
    const d = this.#door();
    if (!d || d.stopped) return;
    if (c.running) return this.#watch(d.started);
    // a door that saved starts only from a save, never as a new one
    if (!startable(this.#saves(), d.saved)) return this.#stop("no save of this door restores");
    const started = d.generation + 1;
    this.ctx.storage.sql.exec("UPDATE door SET started = ?, generation = ? WHERE id = 1", started, started);
    const opts = { env: { DOOR_GENERATION: String(started), DOOR_NAME: d.agent }, enableInternet: this.env.FRAGMENT_COMPUTER_INTERNET !== "off", instance: "lite" };
    if (c.images && c.images.door) opts.image = c.images.door;
    const egress = this.ctx.exports.DoorEgress({ props: { door: d.door } });
    // a stop asked while its egress registered starts nothing
    const overtaken = () => this.#door()?.started !== started || !!this.#door()?.stopped;
    if (await startIntercepted(c, opts, () => settled(() => c.interceptOutboundHttp(HOST, egress)), overtaken)) {
      this.#log("door.started", { generation: started, why });
      this.#watch(started);
    }
  }

  #watch(generation) {
    if (this.#watching === generation) return;
    this.#watching = generation;
    const exited = (why) => {
      if (this.#watching !== generation) return;
      this.#watching = 0;
      this.#log("door.exited", { generation, why });
      // started again, unless it was stopped (a lever, or no save left)
      this.#ensure("exited").catch((e) => this.#log("door.start_failed", { why: String((e && e.message) || e).slice(0, 300) }));
    };
    this.ctx.container.monitor().then(() => exited(null), (e) => exited(String((e && e.message) || e).slice(0, 300)));
  }

  #stop(why) {
    this.ctx.storage.sql.exec("UPDATE door SET stopped = ? WHERE id = 1", why);
    this.#log("door.stopped", { why });
  }

  // A call to its runner (`PORT`, its container's own).
  async #runner(method, path, mark, body) {
    if (!this.ctx.container?.running) return refusal(503, "starting", "the door's runner is not up");
    const headers = { "content-type": "application/json" };
    if (mark !== undefined) headers["x-door-mark"] = String(mark);
    const init = { method, headers };
    if (body !== undefined) init.body = JSON.stringify(body);
    let r;
    try {
      r = await this.ctx.container.getTcpPort(PORT).fetch(`http://door${path}`, init);
    } catch (e) {
      // its container started, its runner not yet listening
      return refusal(503, "starting", String((e && e.message) || e).slice(0, 300));
    }
    return new Response(r.body, { status: r.status, headers: { "content-type": "application/json" } });
  }

  // ---- what its runner asks (images/simplex-door/src/door.rs) ----

  // A runner life begins: its generation (`x-door-life`), and the save it
  // restores (`x-door-save`, named back exactly), whether the door
  // numbered an effect past it; none, a new door (204); or none left
  // though it saved (410: the door stops, never a new profile).
  async #restore(request) {
    const started = Number(request.headers.get("x-door-generation"));
    let d = this.#door();
    if (!d || started !== d.started) return refusal(409, "superseded", `start ${started} is not the door's running one`);
    const life = d.generation + 1;
    this.ctx.storage.sql.exec("UPDATE door SET generation = ? WHERE id = 1", life);
    // bounded: each pass marks one save unreadable, of at most the kept few
    for (;;) {
      const r = restoreOf(this.#saves(), d.saved);
      if (r.kind === "none") return new Response(null, { status: 204, headers: { "x-door-mark": String(d.marks), "x-door-life": String(life) } });
      if (r.kind === "exhausted") {
        this.#stop("no save of this door restores");
        // its container ended by the door, a moment after this answer: a
        // runtime that starts an exited process again in place (local
        // Docker) would ask on forever
        setTimeout(() => this.ctx.container?.running && this.ctx.container.destroy().catch(() => {}), 1000);
        return refusal(410, "exhausted", "this door saved, and no save of it restores");
      }
      const s = r.save;
      const obj = await this.env.BLOBS.get(saveKey(d.door, s.generation, s.seq));
      d = this.#door();
      if (d.generation !== life) return refusal(409, "superseded", `life ${life} was superseded as it restored`);
      if (obj) {
        this.#log("door.restoring", { save: saveId(s), mark: s.mark, marks: d.marks, life });
        return new Response(obj.body, { status: 200, headers: { "x-door-mark": String(d.marks), "x-door-behind": behind(s, d.marks) ? "1" : "0", "x-door-save": saveId(s), "x-door-life": String(life), "content-type": "application/octet-stream" } });
      }
      // its file gone: never given again; the one before it, if any
      this.ctx.storage.sql.exec("UPDATE saves SET unreadable = 1 WHERE generation = ? AND seq = ?", s.generation, s.seq);
    }
  }

  // A save of the running start: its file, then the row that names it, the
  // start fenced again after the upload (one may have begun meanwhile).
  async #save(request, url) {
    const generation = Number(request.headers.get("x-door-generation"));
    const mark = Number(request.headers.get("x-door-mark"));
    const seq = Number(url.searchParams.get("seq"));
    const length = Number(request.headers.get("content-length"));
    let d = this.#door();
    if (!d || fenced(generation, d.generation)) return refusal(409, "superseded", `start ${generation} is not the door's running one`);
    if (!(Number.isInteger(seq) && seq > 0 && Number.isInteger(mark) && mark >= 0 && mark <= d.marks && Number.isInteger(length) && length > 0)) {
      return refusal(400, "invalid", "a save names its seq, its mark (one the door numbered) and its length");
    }
    // its key its life's own (a life's seqs never repeat), held by one
    // upload at a time (taken before anything awaits), so no committed
    // save's file is ever written over or deleted by another's upload
    const key = saveKey(d.door, generation, seq);
    if (this.#uploading.has(key) || this.ctx.storage.sql.exec("SELECT 1 FROM saves WHERE generation = ? AND seq = ?", generation, seq).toArray().length) return refusal(409, "taken", `save ${generation}-${seq} is taken`);
    this.#uploading.add(key);
    try {
      const { readable, writable } = new FixedLengthStream(length);
      const [, put] = await Promise.all([request.body.pipeTo(writable), this.env.BLOBS.put(key, readable)]);
      d = this.#door();
      if (fenced(generation, d.generation)) {
        await this.env.BLOBS.delete(key);
        return refusal(409, "superseded", `life ${generation} was superseded during its save`);
      }
      this.ctx.storage.sql.exec("INSERT INTO saves (generation, seq, mark, bytes, at, unreadable) VALUES (?, ?, ?, ?, ?, 0)", generation, seq, mark, put.size, Date.now());
      this.ctx.storage.sql.exec("UPDATE door SET saved = saved + 1 WHERE id = 1");
    } finally {
      this.#uploading.delete(key);
    }
    // the rows let go first, so no restore picks one whose file goes
    const gone = surplus(this.#saves());
    for (const s of gone) this.ctx.storage.sql.exec("DELETE FROM saves WHERE generation = ? AND seq = ?", s.generation, s.seq);
    if (gone.length) await this.env.BLOBS.delete(gone.map((s) => saveKey(d.door, s.generation, s.seq)));
    return json({ saved: `${generation}-${seq}` });
  }

  // One of its owner's messages, kept once by its sender's id and numbered.
  #deliver(m) {
    if (!(typeof m.id === "string" && m.id.length > 0 && m.id.length <= 64 && typeof m.text === "string")) throw Object.assign(new Error("a delivery is {id, kind, text, at, file}"), { status: 400 });
    const had = this.ctx.storage.sql.exec("SELECT mark FROM deliveries WHERE id = ?", m.id).toArray()[0];
    if (had) return json({ again: true }, 200, { "x-door-mark": String(had.mark) });
    const mark = this.#mark();
    this.ctx.storage.sql.exec("INSERT INTO deliveries (id, mark, body, at) VALUES (?, ?, ?, ?)", m.id, mark, JSON.stringify(m), Date.now());
    this.ctx.storage.sql.exec("DELETE FROM deliveries WHERE mark <= (SELECT mark FROM deliveries ORDER BY mark DESC LIMIT 1 OFFSET ?)", DELIVERIES_KEPT);
    return json({ again: false }, 200, { "x-door-mark": String(mark) });
  }

  // The save a life was given would not restore: that save, named
  // exactly, never given again (asked again, the same; from a life
  // superseded, refused).
  #unreadable(request, body) {
    const life = Number(request.headers.get("x-door-generation"));
    const d = this.#door();
    if (!d || fenced(life, d.generation)) return refusal(409, "superseded", `life ${life} is not the door's running one`);
    const s = parseSaveId(body.save);
    if (!s) throw Object.assign(new Error("an unreadable save is named {save: <generation>-<seq>, why}"), { status: 400 });
    this.ctx.storage.sql.exec("UPDATE saves SET unreadable = 1 WHERE generation = ? AND seq = ?", s.generation, s.seq);
    this.#log("door.save_unreadable", { save: body.save, why: String(body.why ?? "").slice(0, 300) });
    return json({});
  }

  // ---- the e2e's lever (cell/src/levers.rs) ----

  async #lever(a) {
    const d = this.#door();
    switch (a.op) {
      case "start": {
        if (!nameOk(a.door) || typeof a.agent !== "string" || !a.agent) throw Object.assign(new Error("start names its door and its agent"), { status: 400 });
        if (!d) this.ctx.storage.sql.exec("INSERT INTO door (id, door, agent, started, generation, marks, saved, stopped) VALUES (1, ?, ?, 0, 0, 0, 0, NULL)", a.door, a.agent);
        else this.ctx.storage.sql.exec("UPDATE door SET stopped = NULL WHERE id = 1");
        // its watch first: a start that fails is tried again by it
        await this.ctx.storage.setAlarm(Date.now() + WATCH_MS);
        await this.#ensure("asked");
        return json({ started: this.#door().started });
      }
      case "state": {
        let runner = null;
        if (this.ctx.container?.running) {
          const r = await this.#runner("GET", "/state");
          runner = r.ok ? await r.json().catch(() => null) : null;
        }
        const deliveries = this.ctx.storage.sql.exec("SELECT count(*) AS n FROM deliveries").one().n;
        return json({ door: d, saves: this.#saves(), deliveries, running: !!this.ctx.container?.running, runner });
      }
      case "link":
      case "unlink":
        return this.#runner("POST", `/${a.op}`, this.#mark());
      case "reply":
        return this.#runner("POST", "/reply", this.#mark(), { key: a.key, text: a.text });
      case "deliveries": {
        const rows = this.ctx.storage.sql.exec("SELECT id, mark, body FROM deliveries ORDER BY mark").toArray();
        return json(rows.map((r) => ({ id: r.id, mark: r.mark, ...JSON.parse(r.body) })));
      }
      // a planned stop: its runner saves last and ends, and is not started again
      case "stop":
        this.#stop("asked");
        // a start under way sees the stop before it launches, or launched
        if (this.#starting) await this.#starting.catch(() => {});
        if (this.ctx.container?.running) await this.ctx.container.signal(15);
        return json({});
      // a crash: started again, as any exit is
      case "kill":
        if (this.ctx.container?.running) await this.ctx.container.destroy();
        return json({});
      // a rollback: its newest `n` readable saves are taken as unreadable,
      // and it crashes, so its next start restores an older one
      case "rollback": {
        const n = Number(a.n);
        const readable = this.#saves().filter((s) => !s.unreadable).sort((x, y) => y.generation - x.generation || y.seq - x.seq);
        for (const s of readable.slice(0, n)) this.ctx.storage.sql.exec("UPDATE saves SET unreadable = 1 WHERE generation = ? AND seq = ?", s.generation, s.seq);
        if (this.ctx.container?.running) await this.ctx.container.destroy();
        return json({ unread: Math.min(n, readable.length) });
      }
      // a save its next start cannot read: the newest readable one's file
      // replaced by bytes that are no save, then a crash
      case "corrupt": {
        const r = restoreOf(this.#saves(), d.saved);
        const s = r.kind === "save" ? r.save : null;
        if (!s) throw Object.assign(new Error("no save to corrupt"), { status: 409 });
        await this.env.BLOBS.put(saveKey(d.door, s.generation, s.seq), "not a save");
        if (this.ctx.container?.running) await this.ctx.container.destroy();
        return json({ corrupted: `${s.generation}-${s.seq}` });
      }
      // every save's file lost, then a crash: a door that saved finds none
      // to restore, and stops (never a new profile over them)
      case "lose": {
        const keys = this.#saves().map((s) => saveKey(d.door, s.generation, s.seq));
        if (keys.length) await this.env.BLOBS.delete(keys);
        if (this.ctx.container?.running) await this.ctx.container.destroy();
        return json({ lost: keys.length });
      }
      default:
        throw Object.assign(new Error(`no door lever ${a.op}`), { status: 400 });
    }
  }
}

// Every call a door's container makes to `door.fragment.internal`
// (images/simplex-door/src/door.rs): to its own door alone, `props.door`
// set by the door, never by the container. A GET carries no body (the
// spike's first hosted door sat 510 s restoring: a `Request` with a body
// on a GET throws).
export class DoorEgress extends WorkerEntrypoint {
  async fetch(request) {
    const ns = this.env.SIMPLEX_DOOR;
    const stub = ns.get(ns.idFromName(this.ctx.props.door));
    const url = new URL(request.url);
    const headers = new Headers(request.headers);
    headers.set(RELEASE, own(this.env));
    const init = { method: request.method, headers };
    if (request.method !== "GET" && request.method !== "HEAD") init.body = request.body;
    return stub.fetch(`http://door/egress${url.pathname}${url.search}`, init);
  }
}
