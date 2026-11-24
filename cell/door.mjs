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
// - its owner's messages, the newest `DELIVERIES_KEPT`, each until its
//   connector's inbox takes it;
// - its connector's inbox secret, as the connector's `connect` gave it;
// - the files on their way between its connector's chat and its owner's
//   SimpleX app (door-rules `FILES_KEPT`): each a file in the bucket under
//   `doors/<door>/files/<sha256>` and a row here, for a moment.
//
// It ends with its connector (`/door/end`: the connector deleted, its
// owner's unlink, or other code installed in it; cell/src/ended.rs and
// doors.rs): its container ended with no last save, every file under its
// prefix deleted, and its storage gone whole, so nothing of it outlives its
// connector, and a connect after it starts a new door.
//
// Who reaches it: the platform, as any object, through `routed::send`
// (cell/src/levers.rs, the e2e's lever: `door/lever`; a deploy's probe;
// its connector's jobs, `job/…`: cell/src/jobs.rs, templates/simplex),
// each call's release checked first; and its own container, whose calls to
// `door.fragment.internal` its egress (`DoorEgress`) brings here
// (`/egress/…`). Its runner's protocol is images/simplex-door/src/door.rs.
//
// Its owner's messages go on to its connector's inbox as any webhook's
// deliveries do (`/api/f/<door>/inbox`, the inbox's secret in its header),
// through the platform's own front (`ctx.exports.default`), in order; the
// connector's trigger publishes each to its bound agent, whose computer
// wakes for it as for any message. The door's watch drains what the inbox
// did not take at once.
import { DurableObject, WorkerEntrypoint } from "cloudflare:workers";
import { settled, startIntercepted } from "./intercepts.mjs";
import { DELIVERIES_KEPT, FILE_KEPT_MS, FILE_SHA_HEADER, INBOX_HEADER, JOB_ASKS, STALE_ASKS, behind, doorOf, doorPrefix, fenced, fileKey, fileRoom, inboxUrl, nameOk, parseSaveId, replyFiles, restoreOf, saveId, saveKey, secretOk, shaOk, stale, startable, steps, surplus, typeOf } from "./door-rules.mjs";

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
// Which door is asked (its connector's name and life's digits), and the
// connector that asks: the platform's to set alone
// (fragment_core::door::DOOR_NAME_HEADER, CONNECTOR_HEADER).
const DOOR_NAME_HEADER = "x-fragment-door";
const CONNECTOR_HEADER = "x-fragment-connector";
// The inbox secret its connector's `connect` gives, filled at the
// platform's egress (never the connector's code's to read).
const SECRET_HEADER = "x-door-secret";
// How long a way back waits for its runner's planned stop: its link quiet
// (images/simplex-door `STOP_MS_MAX`, 8 s) and its last save taken (twice
// that), with room.
const HANDBACK_MS = 30_000;
// Its stop's word once it handed its link back: no connect, lever or watch
// starts it again (its computer runs that link now), until its end.
const HANDED_BACK = "handed back";
// Pages of its files (the bucket's, up to 1000 keys each) one end deletes;
// asked again, it deletes the rest.
const END_PAGES_MAX = 10;

// The header a reply's file comes with its reply's key in: its use's owner.
const FOR_HEADER = "x-door-for";

// The files deliveries its inbox has yet to take name: never let go.
const WAITING_FILES = "SELECT json_extract(body, '$.file.sha256') FROM deliveries WHERE sent = 0 AND json_extract(body, '$.file.sha256') IS NOT NULL";

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
  // the files on their way in: each key's length, held against the room
  // its files have (`fileRoom`) until its upload ends
  #incoming = new Map();
  // its deliveries going on to its connector's inbox, one drain at a time
  #draining = null;
  #again = false;
  // its storage gone whole by its end, until a connect starts it again;
  // `#ends` counts its ends, so an upload under way across one lets go of
  // its own file
  #ended = false;
  #ends = 0;
  // its connector's life over (deleted, wiped, or other code: its end
  // `final`): it never starts again, whatever comes late (a connect sent
  // before that end and delivered after it). One row says so (`over`),
  // all its storage keeps once it is released
  #over = false;

  constructor(ctx, env) {
    super(ctx, env);
    ctx.blockConcurrencyWhile(async () => {
      this.#over = ctx.storage.sql.exec("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'over'").toArray().length > 0;
      if (!this.#over) this.#migrate();
    });
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
    if (this.#ended || this.#over) return null;
    return this.ctx.storage.sql.exec("SELECT door, agent, started, generation, marks, saved, stopped FROM door WHERE id = 1").toArray()[0] ?? null;
  }

  #saves() {
    if (this.#ended || this.#over) return [];
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
      if (url.pathname === "/door/end" && request.method === "POST") return await this.#end(await bounded(request));
      // the operators' move (docs/bridge.md, "SimpleX"): what it is, and
      // its way back, whatever it is
      if (url.pathname === "/door/state" && request.method === "POST") return await this.#state();
      if (url.pathname === "/door/handback" && request.method === "POST") return await this.#handback(await bounded(request));
      const lever = url.pathname === "/door/lever" && request.method === "POST" ? await bounded(request) : null;
      // the e2e's look at its files, whatever it is
      if (lever?.op === "files") return json(await this.#files(lever.door));
      if (this.#over) return refusal(410, "over", "this door's connector is gone: it never starts again");
      // ended, only a start makes it again (its connector's connect, the
      // lever's start); anything else is refused, its storage left empty
      // a save given (`/door/import`) begins it too, as a start would
      const imports = url.pathname === "/door/import" && request.method === "PUT";
      const starts = (url.pathname === "/job/connect" && request.method === "POST") || lever?.op === "start" || lever?.op === "import" || imports;
      if (this.#ended && !starts) return refusal(410, "ended", "this door has ended: its connector's connect starts it again");
      if (this.#ended) {
        this.#migrate();
        this.#ended = false;
      }
      if (lever) return await this.#lever(lever);
      if (imports) return await this.#saveGiven(request);
      if (url.pathname === "/egress/save" && request.method === "GET") return await this.#restore(request);
      if (url.pathname === "/egress/save" && request.method === "PUT") return await this.#save(request, url);
      if (url.pathname === "/egress/deliver" && request.method === "POST") return this.#deliver(await bounded(request));
      if (url.pathname === "/egress/unreadable" && request.method === "POST") return this.#unreadable(request, await bounded(request));
      // a file its runner received, or one it sends (`/egress/file/<sha256>`)
      if (url.pathname.startsWith("/egress/file/")) {
        const sha = url.pathname.slice("/egress/file/".length);
        if (request.method === "GET") return await this.#giveFile(sha);
        if (request.method === "PUT") return await this.#runnerFile(request, sha);
      }
      const [ask, sha, ...more] = url.pathname.startsWith("/job/") ? url.pathname.slice("/job/".length).split("/") : [];
      const method = ask === "file" ? (sha !== undefined && ["GET", "PUT"].includes(request.method)) : (sha === undefined && request.method === "POST");
      if (JOB_ASKS.includes(ask) && !more.length && method) return await this.#job(ask, request, sha);
      return refusal(404, "not_found", `no door route ${request.method} ${url.pathname}`);
    } catch (e) {
      return refusal(e.status ?? 500, e.status ? "invalid" : "internal", String((e && e.message) || e).slice(0, 300));
    }
  }

  async alarm() {
    if (this.#over) return;
    try {
      await this.#ensure("alarm");
    } catch (e) {
      this.#log("door.start_failed", { why: String((e && e.message) || e).slice(0, 300) });
    }
    await this.#drain();
    await this.#staleFiles();
    if (this.#door() && !this.#door().stopped) await this.ctx.storage.setAlarm(Date.now() + WATCH_MS);
  }

  // Its end: its container ended with no last save (a start under way
  // finds its row gone and starts nothing), its watch let go, its storage
  // gone whole and every file under its prefix deleted. Asked again, the
  // same, the files' rest deleted; asked of a door never started, nothing
  // is kept. `final` (its connector's life over): it never starts again,
  // its one row (`over`) saying so; else (its connector unbound, which may
  // be bound again) its connector's next connect starts a new door.
  async #end(body) {
    const door = body.door;
    if (!nameOk(door)) throw Object.assign(new Error("an end names its door"), { status: 400 });
    const d = this.#door();
    if (d && d.door !== door) return refusal(409, "another_door", `this door is ${d.door}`);
    this.#ends += 1;
    if (!this.#ended && !this.#over) {
      this.#ended = true;
      await this.ctx.storage.deleteAlarm();
      await this.ctx.storage.deleteAll();
    }
    if (body.final === true && !this.#over) {
      this.ctx.storage.sql.exec("CREATE TABLE over (at INTEGER NOT NULL)");
      this.ctx.storage.sql.exec("INSERT INTO over (at) VALUES (?)", Date.now());
      this.#over = true;
    }
    if (this.ctx.container?.running) await this.ctx.container.destroy();
    let files = 0;
    for (let page = 0; page < END_PAGES_MAX; page += 1) {
      const listed = await this.env.BLOBS.list({ prefix: doorPrefix(door) });
      if (!listed.objects.length) break;
      await this.env.BLOBS.delete(listed.objects.map((o) => o.key));
      files += listed.objects.length;
    }
    console.log(JSON.stringify({ event: "door.ended", door, files }));
    return json({ ended: door, files });
  }

  // The keys of its files in the bucket (at most a page): the e2e's look.
  async #files(door) {
    if (!nameOk(door)) throw Object.assign(new Error("a look names its door"), { status: 400 });
    const listed = await this.env.BLOBS.list({ prefix: doorPrefix(door) });
    return { files: listed.objects.map((o) => o.key) };
  }

  // Started, or started again (unstopped): its row made the first time,
  // its watch set before the start is tried. A deployment that runs no
  // doors refuses it before anything is kept, so nothing watches in vain.
  async #begin(door, agent) {
    if (!this.ctx.container) return refusal(409, "no_doors", "this deployment runs no doors (no container for them)");
    if (!nameOk(door) || typeof agent !== "string" || !agent || agent.length > 64) throw Object.assign(new Error("a start names its door and its agent (at most 64 characters)"), { status: 400 });
    const d = this.#door();
    if (!d) this.ctx.storage.sql.exec("INSERT INTO door (id, door, agent, started, generation, marks, saved, stopped) VALUES (1, ?, ?, 0, 0, 0, 0, NULL)", door, agent);
    else if (d.door !== door) return refusal(409, "another_door", `this door is ${d.door}`);
    else if (d.stopped === HANDED_BACK) return refusal(409, "handed_back", "this door handed its link back to its computer: it never starts again");
    else this.ctx.storage.sql.exec("UPDATE door SET stopped = NULL WHERE id = 1");
    // its watch first: a start that fails is tried again by it
    await this.ctx.storage.setAlarm(Date.now() + WATCH_MS);
    await this.#ensure("asked");
    return json({ started: this.#door().started, door });
  }

  // A door that never saved begins from a save it is given (a computer's
  // link handed over: the platform's, `/door/import`, its door, its
  // agent's name and its length in the platform's headers: a stream's
  // length is its own header, never one a runtime may drop): its file under its prefix and
  // its row, as though it had saved it, its mark none (its link in step: the
  // core that wrote it ended whole). It does not start: its connector's
  // connect starts it, restoring this save. Given again (its SHA-256 the
  // same), the same answer; a door that saved, or was given another, never
  // takes one (409), so no link it holds is written over.
  async #saveGiven(request) {
    const door = request.headers.get(DOOR_NAME_HEADER);
    const agent = request.headers.get("x-door-agent");
    const sha = request.headers.get(FILE_SHA_HEADER);
    const length = Number(request.headers.get("x-door-length"));
    // its owner's chat with their old agent, staged on their computer, never kept there
    const staged = request.headers.get("x-door-staged") === "1" ? 1 : 0;
    const refuse = async (status, error, message) => (await request.body?.cancel(), refusal(status, error, message));
    if (!nameOk(door) || typeof agent !== "string" || !agent || agent.length > 64 || !shaOk(sha) || !(Number.isInteger(length) && length > 0) || !request.body) {
      return refuse(400, "invalid", "an import names its door, its agent and its save's SHA-256, and says its length");
    }
    const d = this.#door();
    if (d && d.door !== door) return refuse(409, "another_door", `this door is ${d.door}`);
    if (d?.stopped === HANDED_BACK) return refuse(409, "handed_back", "this door handed its link back to its computer: it takes no save");
    const had = this.ctx.storage.sql.exec("SELECT sha FROM imported").toArray()[0];
    if (had?.sha === sha) return (await request.body.cancel(), json({ imported: sha, again: true }));
    if (had) return refuse(409, "imported", "this door was given another save");
    if (d && (d.saved > 0 || d.started > 0)) return refuse(409, "has_saves", "this door has started, or saved: it takes no save given");
    const key = saveKey(door, 0, 1);
    if (this.#uploading.has(key)) return refuse(409, "taken", "a save is being given");
    this.#uploading.add(key);
    const ends = this.#ends;
    try {
      const { readable, writable } = new FixedLengthStream(length);
      // the bucket checks its SHA-256 as it stores it: other bytes are never kept
      let put;
      try {
        [, put] = await Promise.all([request.body.pipeTo(writable), this.env.BLOBS.put(key, readable, { sha256: sha })]);
      } catch (e) {
        await this.env.BLOBS.delete(key).catch(() => {});
        return refusal(422, "not_its_sha", `its bytes are not the save its SHA-256 names: ${String((e && e.message) || e).slice(0, 200)}`);
      }
      if (this.#ends !== ends) {
        await this.env.BLOBS.delete(key);
        return refusal(410, "ended", "this door ended as its save came");
      }
      // generation 0: no life of its own is numbered so, nor will be
      this.ctx.storage.transactionSync(() => {
        if (!this.#door()) this.ctx.storage.sql.exec("INSERT INTO door (id, door, agent, started, generation, marks, saved, stopped) VALUES (1, ?, ?, 0, 0, 0, 0, NULL)", door, agent);
        this.ctx.storage.sql.exec("INSERT INTO saves (generation, seq, mark, bytes, at, unreadable) VALUES (0, 1, 0, ?, ?, 0)", put.size, Date.now());
        this.ctx.storage.sql.exec("UPDATE door SET saved = 1 WHERE id = 1");
        this.ctx.storage.sql.exec("INSERT INTO imported (sha, at, staged) VALUES (?, ?, ?)", sha, Date.now(), staged);
      });
    } finally {
      this.#uploading.delete(key);
    }
    this.#log("door.imported", { sha, bytes: length });
    return json({ imported: sha });
  }

  // The way back of the operators' move (docs/bridge.md, "SimpleX"): a
  // planned stop (its runner stops its core and saves last), not started
  // again but by its connector's connect, then its newest save that
  // restores, as a start would restore it, and whether it is behind its
  // newest effects (`x-door-behind`): the computer the link came from
  // places it. None (204) for a door that never held one; 410 for one none
  // of whose saves restores. Asked again, the same. A runner not ended
  // within `HANDBACK_MS` is ended, its last save not taken (its newest is
  // then behind, and says so). Stopped, it never starts again (no connect
  // nor lever does: `HANDED_BACK`), so no core of it runs beside its
  // computer's. Its owner's messages it took and its connector's inbox has
  // not are sent on first: while one waits, 409 `busy` (asked again, it
  // tries again), never a save whose cursor is past a message no one has.
  // `x-door-started`: whether it ever started (a save of a door that never
  // ran is the one it was given).
  async #handback(body) {
    if (!nameOk(body.door)) throw Object.assign(new Error("a way back names its door"), { status: 400 });
    let d = this.#door();
    // one never begun is fenced too: no save given after, nor start
    if (!d) {
      this.ctx.storage.sql.exec("INSERT INTO door (id, door, agent, started, generation, marks, saved, stopped) VALUES (1, ?, '', 0, 0, 0, 0, ?)", body.door, HANDED_BACK);
      return new Response(null, { status: 204 });
    }
    if (d.door !== body.door) return refusal(409, "another_door", `this door is ${d.door}`);
    if (d.stopped !== HANDED_BACK) this.#stop(HANDED_BACK);
    // a start under way sees the stop before it launches, or launched
    if (this.#starting) await this.#starting.catch(() => {});
    const c = this.ctx.container;
    if (c?.running) {
      await c.signal(15);
      const until = Date.now() + HANDBACK_MS;
      // bounded by HANDBACK_MS
      while (c.running && Date.now() < until) await new Promise((r) => setTimeout(r, 250));
      if (c.running) await c.destroy();
    }
    await this.#drain();
    const unsent = this.ctx.storage.sql.exec("SELECT count(*) AS n FROM deliveries WHERE sent = 0").one().n;
    if (unsent > 0) return refusal(409, "busy", `${unsent} of its owner's messages still go to its connector: ask again in a moment`);
    // bounded: each pass marks one save unreadable, of at most the kept few
    for (;;) {
      d = this.#door();
      const r = restoreOf(this.#saves(), d.saved);
      if (r.kind === "none") return new Response(null, { status: 204 });
      if (r.kind === "exhausted") return refusal(410, "exhausted", "this door saved, and no save of it restores");
      const obj = await this.env.BLOBS.get(saveKey(d.door, r.save.generation, r.save.seq));
      if (obj) {
        this.#log("door.handed_back", { save: saveId(r.save), behind: behind(r.save, d.marks) });
        return new Response(obj.body, { status: 200, headers: { "x-door-behind": behind(r.save, d.marks) ? "1" : "0", "x-door-save": saveId(r.save), "x-door-started": d.started > 0 ? "1" : "0", "content-type": "application/octet-stream" } });
      }
      this.ctx.storage.sql.exec("UPDATE saves SET unreadable = 1 WHERE generation = ? AND seq = ?", r.save.generation, r.save.seq);
    }
  }

  // What it is: its row, its saves, its deliveries (and how many its
  // inbox has yet to take), and its runner's own state while it runs.
  async #state() {
    let runner = null;
    if (this.ctx.container?.running) {
      const r = await this.#runner("GET", "/state");
      runner = r.ok ? await r.json().catch(() => null) : null;
    }
    const n = this.ctx.storage.sql.exec("SELECT count(*) AS n, coalesce(sum(sent = 0), 0) AS unsent FROM deliveries").one();
    const inbox = this.ctx.storage.sql.exec("SELECT 1 FROM inbox WHERE id = 1").toArray().length === 1;
    // `doors`: whether this deployment runs doors at all (a container for
    // them); `staged`: a chat it was given waits for its owner's word
    return json({ door: this.#door(), saves: this.#saves(), deliveries: n.n, unsent: n.unsent, inbox, running: !!this.ctx.container?.running, runner, doors: !!this.ctx.container, staged: this.#staged() });
  }

  // Whether it holds its owner's staged chat with their old agent, given
  // (`/door/import`) and not yet kept or let go: it never started.
  #staged() {
    const i = this.ctx.storage.sql.exec("SELECT staged FROM imported").toArray()[0];
    return !!(i?.staged && this.#door()?.started === 0);
  }

  // That chat let go (its owner starts a new one): its rows first, then its
  // file, so no row names a file gone; its connect then begins a new door.
  async #letGo() {
    const d = this.#door();
    this.ctx.storage.transactionSync(() => {
      for (const t of ["saves", "imported", "door"]) this.ctx.storage.sql.exec(`DELETE FROM ${t}`);
    });
    await this.env.BLOBS.delete(saveKey(d.door, 0, 1));
    this.#log("door.staged_let_go", {});
  }

  // An ask of its runner under a new mark, asked again under another while
  // a later mark overtook it (a delivery's), at most `STALE_ASKS` times,
  // and only while it is `current` (a link or unlink none newer followed);
  // still overtaken, its asker tries again later (503).
  async #ask(path, body, current = () => true) {
    for (let n = 0; n < STALE_ASKS; n += 1) {
      const r = await this.#runner("POST", path, this.#mark(), body);
      if (r.status !== 409) return r;
      const text = await r.text();
      let said = null;
      try {
        said = JSON.parse(text);
      } catch {}
      if (!stale(r.status, said)) return new Response(text, { status: 409, headers: { "content-type": "application/json" } });
      if (!current()) return refusal(409, "overtaken", "a newer link or unlink was asked");
    }
    return refusal(503, "overtaken", `asked under ${STALE_ASKS} marks, each overtaken: ask again`);
  }

  // The newest link or unlink its connector numbered.
  #asked() {
    return this.ctx.storage.sql.exec("SELECT asked FROM inbox WHERE id = 1").one().asked;
  }

  // ---- what its connector's jobs ask (templates/simplex/app.mjs) ----

  // Its connector asks (the platform names both: this door, its life's,
  // and the connector): its start with the inbox's secret (`connect`), its
  // state, its SimpleX app's link and unlink, and a reply.
  async #job(ask, request, sha) {
    const door = request.headers.get(DOOR_NAME_HEADER);
    const connector = request.headers.get(CONNECTOR_HEADER);
    if (!doorOf(door, connector)) return refusal(400, "invalid", "a job's ask names its door and its connector (the platform's headers)");
    const d = this.#door();
    if (d && d.door !== door) return refusal(409, "another_door", `this door is ${d.door}`);
    const inbox = this.ctx.storage.sql.exec("SELECT fragment FROM inbox WHERE id = 1").toArray()[0];
    if (inbox && inbox.fragment !== connector) return refusal(409, "another_connector", `this door is ${inbox.fragment}'s`);
    if (ask === "connect") {
      const secret = request.headers.get(SECRET_HEADER);
      const body = await bounded(request);
      if (!secretOk(secret)) throw Object.assign(new Error(`connect gives its inbox's secret (${SECRET_HEADER})`), { status: 400 });
      if (!this.ctx.container) return refusal(409, "no_doors", "this deployment runs no doors (no container for them)");
      // fenced before anything (a new chat's let-go included): its computer runs its link now
      if (d?.stopped === HANDED_BACK) return refusal(409, "handed_back", "this door handed its link back to its computer: it never starts again");
      // its owner's SimpleX chat with their old agent, handed over staged
      // (never kept on their computer), starts only at their word: kept
      // (`keep`: its runner adopts it as it restores), or let go for a new chat
      if (this.#staged()) {
        if (body.keep === true) this.ctx.storage.sql.exec("UPDATE imported SET staged = 0");
        else if (body.keep === false) await this.#letGo();
        else return refusal(409, "staged", "its owner's SimpleX chat with their old agent waits: keep it, or start a new chat");
      }
      this.ctx.storage.sql.exec("INSERT INTO inbox (id, fragment, secret) VALUES (1, ?, ?) ON CONFLICT (id) DO UPDATE SET secret = excluded.secret", connector, secret);
      const started = await this.#begin(door, body.name);
      this.#drain().catch(() => {});
      return started;
    }
    if (!d || !inbox) return refusal(409, "not_connected", "its connector's connect starts it first");
    if (ask === "state") return this.#state();
    // a file its agent sends (`PUT`, one of the connector's blobs, its
    // reply's to use), or one its owner sent (`GET`, kept as one of the
    // connector's blobs); `kept`, once its connector has it, by the
    // delivery that named it
    if (ask === "file") return request.method === "PUT" ? await this.#keepFile(sha, request, `r:${String(request.headers.get(FOR_HEADER) ?? "").slice(0, 200)}`) : await this.#giveFile(sha);
    if (ask === "kept") return this.#kept(await bounded(request));
    if (ask === "reply") return await this.#reply(await bounded(request));
    // a link or unlink, numbered by its connector: one older than the
    // newest it numbered (a retry of a link after its unlink) is refused,
    // here and while it is asked, so none acts after a newer one
    const { n } = await bounded(request);
    if (!Number.isInteger(n) || n < 1) throw Object.assign(new Error(`${ask} names its number (n), its connector's`), { status: 400 });
    if (n < this.#asked()) return refusal(409, "overtaken", "a newer link or unlink was asked");
    this.ctx.storage.sql.exec("UPDATE inbox SET asked = ? WHERE id = 1", n);
    return this.#ask(`/${ask}`, undefined, () => this.#asked() === n);
  }

  // A reply its connector asks sent: its key, its text, and its files, each
  // one the door holds (its connector gave it first, `PUT /file/<sha256>`),
  // to a runner that carries files (a container started before it did
  // runs on until its next start: 409 `no_files`, its connector sends the
  // text alone).
  async #reply(body) {
    const files = replyFiles(body.files);
    if (!files) throw Object.assign(new Error("a reply's files are [{sha256, name, type, size}], at most a message's"), { status: 400 });
    // its key's uses of the files its connector gave for it: once its
    // runner has taken the reply (202), they are its runner's (`R:<key>`),
    // kept until it says the reply is done (200) or ended (410: never
    // sent), whatever a poll between says (its state unread as it starts
    // again: it may hold the reply still); before that (`r:<key>`), any
    // answer but 202 ends them, its connector asking no more for these
    // files (not linked, its runner not there, or one that cannot send
    // them: its text alone next, under the same key). A day unused, a file
    // goes all the same (`#staleFiles`). Another key's uses are its own.
    const asked = await this.#replyAsked(body, files);
    const [mine, taken] = [`r:${body.key}`, `R:${body.key}`];
    if (asked.status === 202) {
      this.ctx.storage.sql.exec("INSERT INTO uses (sha, owner) SELECT sha, ? FROM uses WHERE owner = ? ON CONFLICT DO NOTHING", taken, mine);
      this.ctx.storage.sql.exec("DELETE FROM uses WHERE owner = ?", mine);
    } else if (asked.status === 200 || asked.status === 410) {
      this.ctx.storage.sql.exec("DELETE FROM uses WHERE owner = ? OR owner = ?", mine, taken);
    } else {
      this.ctx.storage.sql.exec("DELETE FROM uses WHERE owner = ?", mine);
    }
    return asked;
  }

  async #replyAsked(body, files) {
    if (!files.length) return this.#ask("/reply", { key: body.key, text: body.text });
    const held = new Set(this.ctx.storage.sql.exec("SELECT sha FROM files").toArray().map((r) => r.sha));
    const missing = files.find((f) => !held.has(f.sha256));
    if (missing) return refusal(409, "no_file", `the door holds no file ${missing.sha256}: give it first`);
    const r = await this.#runner("GET", "/state");
    if (!r.ok) return r;
    const runner = await r.json().catch(() => null);
    if (runner?.files !== true) return refusal(409, "no_files", "this door's runner carries no files: its text alone");
    return this.#ask("/reply", { key: body.key, text: body.text, files });
  }

  // ---- the files it holds, on their way ----

  // A file its runner received for its connector (`PUT /egress/file`), from
  // its running life alone (a superseded one's is refused, as its saves).
  async #runnerFile(request, sha) {
    const life = Number(request.headers.get("x-door-generation"));
    const d = this.#door();
    if (!d || fenced(life, d.generation)) {
      await request.body?.cancel();
      return refusal(409, "superseded", `life ${life} is not the door's running one`);
    }
    // its use until the delivery naming it is made (`#deliver`)
    return this.#keepFile(sha, request, "given");
  }

  // A file kept under its name, its bytes streamed in (`content-length` of
  // them), its type as its giver said: once (one kept already is kept
  // again, its bytes unread), and only while there is room
  // (`fileRoom`; else 503, asked again later); its giver's use (`owner`)
  // recorded, once. The door ending during its upload lets go of its
  // file, as of a save's.
  async #keepFile(sha, request, owner) {
    const length = Number(request.headers.get("content-length"));
    if (!shaOk(sha) || !(Number.isInteger(length) && length > 0) || !request.body) {
      await request.body?.cancel();
      throw Object.assign(new Error("a file is named by its SHA-256 (lowercase hex) and says its length"), { status: 400 });
    }
    const d = this.#door();
    const now = Date.now();
    // given again: its giver's use, once
    if (this.ctx.storage.sql.exec("UPDATE files SET at = ? WHERE sha = ? RETURNING sha", now, sha).toArray().length) {
      this.ctx.storage.sql.exec("INSERT INTO uses (sha, owner) VALUES (?, ?) ON CONFLICT DO NOTHING", sha, owner);
      await request.body.cancel();
      return json({ kept: sha, again: true });
    }
    const key = fileKey(d.door, sha);
    if (this.#incoming.has(key)) {
      await request.body.cancel();
      return refusal(503, "uploading", "that file is on its way in: give it again later");
    }
    // its room held before anything awaits, so two at once never pass it
    this.#incoming.set(key, length);
    const ends = this.#ends;
    try {
      await this.#staleFiles();
      if (!(await this.#room(key, length))) {
        await request.body.cancel();
        return refusal(503, "files_full", "the door holds as many files as it may: give it again later");
      }
      const { readable, writable } = new FixedLengthStream(length);
      await Promise.all([request.body.pipeTo(writable), this.env.BLOBS.put(key, readable)]);
      if (this.#ends !== ends) {
        await this.env.BLOBS.delete(key);
        return refusal(410, "ended", "this door ended as its file came");
      }
      this.ctx.storage.sql.exec("INSERT INTO files (sha, bytes, type, at) VALUES (?, ?, ?, ?) ON CONFLICT (sha) DO UPDATE SET at = excluded.at", sha, length, typeOf(request.headers.get("content-type")), now);
      this.ctx.storage.sql.exec("INSERT INTO uses (sha, owner) VALUES (?, ?) ON CONFLICT DO NOTHING", sha, owner);
    } finally {
      this.#incoming.delete(key);
    }
    return json({ kept: sha });
  }

  // Room for a file of `length` beside every one it holds and those on
  // their way in: made, when there is none, by letting go of files nothing
  // still uses (`uses`), the oldest first; never one still used, nor one a
  // waiting delivery names.
  async #room(key, length) {
    // read again at each look, both at once: an upload ending meanwhile is
    // a row or on its way, never both
    const held = () => [...this.ctx.storage.sql.exec("SELECT bytes FROM files").toArray(), ...[...this.#incoming].filter(([k]) => k !== key).map(([, bytes]) => ({ bytes }))];
    const d = this.#door();
    // bounded: one file let go a pass, of at most `FILES_KEPT`
    while (!fileRoom(held(), length)) {
      // the oldest none uses now (one given again since is used again)
      const f = this.ctx.storage.sql.exec(`DELETE FROM files WHERE sha = (SELECT sha FROM files WHERE sha NOT IN (SELECT sha FROM uses) AND sha NOT IN (${WAITING_FILES}) ORDER BY at LIMIT 1) RETURNING sha`).toArray()[0];
      if (!f) return false;
      await this.env.BLOBS.delete(fileKey(d.door, f.sha));
    }
    return true;
  }

  // A file it holds, its bytes and its type, its name in `FILE_SHA_HEADER`.
  // A read releases no use: its reader may be cut off and read it again
  // (its connector says `kept` once it has it; a reply's file is used until
  // its reply is done).
  async #giveFile(sha) {
    const row = shaOk(sha) ? this.ctx.storage.sql.exec("UPDATE files SET at = ? WHERE sha = ? RETURNING bytes, type", Date.now(), sha).toArray()[0] : null;
    if (!row) return refusal(404, "no_file", `the door holds no file ${String(sha).slice(0, 64)}`);
    const obj = await this.env.BLOBS.get(fileKey(this.#door().door, sha));
    if (!obj) {
      this.ctx.storage.sql.exec("DELETE FROM files WHERE sha = ?", sha);
      return refusal(404, "no_file", `the door's file ${sha} is gone`);
    }
    return new Response(obj.body, { headers: { "content-type": row.type, "content-length": String(row.bytes), [FILE_SHA_HEADER]: sha } });
  }

  // The files held past `FILE_KEPT_MS`, but those a delivery its inbox has
  // yet to take names (an inbox down past a day): their rows and uses
  // first (a use whose answer never came ends with them), then their files.
  async #staleFiles() {
    const d = this.#door();
    if (!d) return;
    const old = this.ctx.storage.sql.exec(`DELETE FROM files WHERE at <= ? AND sha NOT IN (${WAITING_FILES}) RETURNING sha`, Date.now() - FILE_KEPT_MS).toArray();
    for (const f of old) this.ctx.storage.sql.exec("DELETE FROM uses WHERE sha = ?", f.sha);
    if (old.length) await this.env.BLOBS.delete(old.map((f) => fileKey(d.door, f.sha)));
  }

  // Its connector kept the file delivery `id` named: that delivery's use
  // ends, and no other's (said again, nothing more).
  #kept(body) {
    if (!(typeof body.id === "string" && body.id.length > 0 && body.id.length <= 64)) throw Object.assign(new Error("kept names its delivery (id)"), { status: 400 });
    const ended = this.ctx.storage.sql.exec("DELETE FROM uses WHERE owner = ? RETURNING sha", `d:${body.id}`).toArray().length;
    return json({ ended });
  }

  // ---- its owner's messages, on to its connector's inbox ----

  // One drain at a time: the oldest message its inbox has yet to take,
  // then the next, each sent until the inbox takes it (2xx); one refused
  // (a deploy, its inbox full, a secret it does not take, the platform
  // unreachable) ends the drain, tried again at its next watch, so the
  // inbox takes them in order. Bounded by what it keeps.
  #drain() {
    // asked again while one runs: that one goes on once more, so a message
    // kept as it ended is not left for the watch
    this.#again = true;
    this.#draining ??= (async () => {
      while (this.#again) {
        this.#again = false;
        await this.#forward();
      }
    })().finally(() => (this.#draining = null));
    return this.#draining;
  }

  async #forward() {
    const d = this.#door();
    if (!d) return;
    const inbox = this.ctx.storage.sql.exec("SELECT fragment, secret FROM inbox WHERE id = 1").toArray()[0];
    if (!inbox) return;
    const to = inboxUrl(this.env.FRAGMENT_PLATFORM_URL, inbox.fragment);
    for (let n = 0; n <= DELIVERIES_KEPT; n += 1) {
      const next = this.ctx.storage.sql.exec("SELECT id, body FROM deliveries WHERE sent = 0 ORDER BY mark LIMIT 1").toArray()[0];
      if (!next) return;
      let r;
      try {
        r = await this.ctx.exports.default.fetch(to, { method: "POST", headers: { "content-type": "application/json", [INBOX_HEADER]: inbox.secret }, body: JSON.stringify({ source: "simplex", payload: { ...JSON.parse(next.body), door: d.door } }) });
      } catch (e) {
        return this.#log("door.inbox_failed", { id: next.id, why: String((e && e.message) || e).slice(0, 300) });
      }
      const said = (await r.text()).slice(0, 300);
      if (!r.ok) return this.#log("door.inbox_refused", { id: next.id, status: r.status, why: said });
      this.ctx.storage.sql.exec("UPDATE deliveries SET sent = 1 WHERE id = ?", next.id);
      // its file, a day from now: its connector keeps it next
      const file = JSON.parse(next.body).file;
      if (file) this.ctx.storage.sql.exec("UPDATE files SET at = ? WHERE sha = ?", Date.now(), file.sha256);
    }
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
    const opts = { env: { DOOR_GENERATION: String(started), DOOR_NAME: d.agent }, enableInternet: this.env.FRAGMENT_DOOR_INTERNET !== "off", instance: "lite" };
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
    const ends = this.#ends;
    try {
      const { readable, writable } = new FixedLengthStream(length);
      const [, put] = await Promise.all([request.body.pipeTo(writable), this.env.BLOBS.put(key, readable)]);
      // the door ended during its upload: its file, which that end's
      // deletion may have missed, goes too, and nothing names it
      if (this.#ends !== ends) {
        await this.env.BLOBS.delete(key);
        return refusal(410, "ended", "this door ended during its save");
      }
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
    // its file, which its runner gave the door first, named by its SHA-256;
    // one named otherwise (a runner that carried no files, whose delivery
    // named its file alone) is no file here: its message goes as its text
    if (m.file != null && !shaOk(m.file?.sha256)) m = { ...m, file: null };
    const had = this.ctx.storage.sql.exec("SELECT mark FROM deliveries WHERE id = ?", m.id).toArray()[0];
    if (had) return json({ again: true }, 200, { "x-door-mark": String(had.mark) });
    // its inbox behind by as many as it keeps: refused, so its runner
    // keeps its cursor on this one and delivers it again later, none lost
    const unsent = this.ctx.storage.sql.exec("SELECT count(*) AS n FROM deliveries WHERE sent = 0").one().n;
    if (unsent >= DELIVERIES_KEPT) return refusal(503, "inbox_behind", `its inbox has yet to take ${unsent}: deliver it again later`);
    const mark = this.#mark();
    this.ctx.storage.sql.exec("INSERT INTO deliveries (id, mark, body, at) VALUES (?, ?, ?, ?)", m.id, mark, JSON.stringify(m), Date.now());
    // its file's use is this delivery's now, until its connector keeps it
    if (m.file != null) {
      this.ctx.storage.sql.exec("DELETE FROM uses WHERE sha = ? AND owner = 'given'", m.file.sha256);
      this.ctx.storage.sql.exec("INSERT INTO uses (sha, owner) VALUES (?, ?) ON CONFLICT DO NOTHING", m.file.sha256, `d:${m.id}`);
    }
    // the ones its inbox took, but the newest few (each taken once by its id)
    this.ctx.storage.sql.exec("DELETE FROM deliveries WHERE sent = 1 AND mark <= (SELECT mark FROM deliveries WHERE sent = 1 ORDER BY mark DESC LIMIT 1 OFFSET ?)", DELIVERIES_KEPT);
    // on to its inbox once the runner has its answer; its watch is the net
    this.#drain().catch(() => {});
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
      case "start":
        return this.#begin(a.door, a.agent);
      case "state":
        return this.#state();
      case "link":
      case "unlink":
        return this.#ask(`/${a.op}`);
      case "reply":
        return await this.#reply({ key: a.key, text: a.text, files: a.files });
      // a save to begin from, as a computer's link handed over gives one
      // (`bytes`, base64, a door's packed save; `staged`, its owner's chat
      // with their old agent): its SHA-256
      case "import": {
        const bytes = Uint8Array.from(atob(String(a.bytes ?? "")), (c) => c.charCodeAt(0));
        const sha = [...new Uint8Array(await crypto.subtle.digest("SHA-256", bytes))].map((b) => b.toString(16).padStart(2, "0")).join("");
        return this.#saveGiven(new Request("http://door/door/import", { method: "PUT", headers: { [DOOR_NAME_HEADER]: a.door, "x-door-agent": String(a.agent ?? ""), [FILE_SHA_HEADER]: sha, "x-door-length": String(bytes.length), "x-door-staged": a.staged ? "1" : "0" }, body: bytes }));
      }
      // a file as its runner gives one (`bytes`, base64), for a delivery
      // or a reply to name: its SHA-256; or (`sha`) what the door holds
      // under that name, hashed again: the e2e's look at its bytes
      case "file": {
        const hex = async (bytes) => [...new Uint8Array(await crypto.subtle.digest("SHA-256", bytes))].map((b) => b.toString(16).padStart(2, "0")).join("");
        if (a.sha !== undefined) {
          const held = await this.#giveFile(a.sha);
          if (!held.ok) return held;
          const bytes = new Uint8Array(await held.arrayBuffer());
          return json({ sha256: await hex(bytes), size: bytes.length, type: held.headers.get("content-type") });
        }
        const bytes = Uint8Array.from(atob(String(a.bytes ?? "")), (c) => c.charCodeAt(0));
        const sha = await hex(bytes);
        const r = await this.#keepFile(sha, new Request("http://door/", { method: "PUT", headers: { "content-type": typeOf(a.type), "content-length": String(bytes.length) }, body: bytes }), "given");
        return r.ok ? json({ sha256: sha, size: bytes.length }) : r;
      }
      case "deliveries": {
        const rows = this.ctx.storage.sql.exec("SELECT id, mark, body, sent FROM deliveries ORDER BY mark").toArray();
        return json(rows.map((r) => ({ id: r.id, mark: r.mark, sent: r.sent, ...JSON.parse(r.body) })));
      }
      // a message from its owner's SimpleX app, as its runner delivers one
      // (no SimpleX server reaches a local e2e's door), on to its inbox
      case "deliver": {
        const r = this.#deliver(a.message ?? {});
        await this.#drain();
        return r;
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
