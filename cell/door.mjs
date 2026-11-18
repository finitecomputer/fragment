// SPIKE (branch spike/simplex-door, never merged): the SimpleX door, a
// Durable Object per SimpleX connector fragment (templates/simplex), named
// by that fragment, holding a `lite` container that runs SimpleX's core and
// the door's runner (images/simplex-door), always on. Its state:
//
// - the runner's saves: both databases after every change, gzipped, kept
//   here in parts (a stored value is at most 2 MB), the newest
//   `SAVES_KEPT` of them (older ones for the spike's rollback test);
// - its config: where the connector's inbox is and the secret its
//   deliveries carry (from the connector's `connect`, the secret filled at
//   the platform's egress, never the app's), and the XFTP relay's targets;
// - what it sent for the connector (`sent`, by the job step's key), so a
//   step asked again sends once;
// - the spike's measurements (`stats`).
//
// Who reaches it: the connector's jobs (`https://door.fragment.internal/…`,
// routed here by cell/src/jobs.rs for a `simplex` connector alone), its own
// container (`http://door.fragment.internal/…`, intercepted: DoorEgress),
// and the spike's driver (`/api/sxdoor/<connector>/admin/…` on the platform
// host, with the test levers' secret).
import { DurableObject, WorkerEntrypoint } from "cloudflare:workers";
import { connect } from "cloudflare:sockets";

const PART_MAX = 1_900_000;
const SAVES_KEPT = 300;
const STATS_KEPT = 200_000;
const WATCH_MS = 30_000;
const HEALTH_EVERY_MS = 10 * 60_000;
const PORT = 8080;
const RELAY_HOSTS = ["xftp1.simplex.im", "xftp2.simplex.im", "xftp3.simplex.im", "xftp4.simplex.im", "xftp5.simplex.im", "xftp6.simplex.im", "xftp1.simplexonflux.com", "xftp2.simplexonflux.com", "xftp3.simplexonflux.com", "xftp4.simplexonflux.com", "xftp5.simplexonflux.com", "xftp6.simplexonflux.com"];

const json = (v, status = 200, headers = {}) => new Response(JSON.stringify(v), { status, headers: { "content-type": "application/json", ...headers } });

export class SimplexDoor extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    const sql = ctx.storage.sql;
    sql.exec("CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT NOT NULL)");
    sql.exec("CREATE TABLE IF NOT EXISTS saves (id INTEGER PRIMARY KEY AUTOINCREMENT, generation INTEGER NOT NULL, seq INTEGER NOT NULL, at INTEGER NOT NULL, bytes INTEGER NOT NULL, parts INTEGER NOT NULL, sha TEXT, why TEXT)");
    sql.exec("CREATE TABLE IF NOT EXISTS parts (save INTEGER NOT NULL, idx INTEGER NOT NULL, data BLOB NOT NULL, PRIMARY KEY (save, idx))");
    sql.exec("CREATE TABLE IF NOT EXISTS stats (id INTEGER PRIMARY KEY AUTOINCREMENT, at INTEGER NOT NULL, generation INTEGER, kind TEXT NOT NULL, data TEXT)");
    sql.exec("CREATE TABLE IF NOT EXISTS sent (key TEXT PRIMARY KEY, at INTEGER NOT NULL, answer TEXT NOT NULL)");
    this.watching = 0;
  }

  get(k) {
    const r = this.ctx.storage.sql.exec("SELECT v FROM kv WHERE k = ?", k).toArray()[0];
    return r ? JSON.parse(r.v) : null;
  }

  put(k, v) {
    if (v === null || v === undefined) this.ctx.storage.sql.exec("DELETE FROM kv WHERE k = ?", k);
    else this.ctx.storage.sql.exec("INSERT INTO kv (k, v) VALUES (?, ?) ON CONFLICT (k) DO UPDATE SET v = excluded.v", k, JSON.stringify(v));
  }

  stat(kind, data, generation = this.get("generation")) {
    this.ctx.storage.sql.exec("INSERT INTO stats (at, generation, kind, data) VALUES (?, ?, ?, ?)", Date.now(), generation, kind, JSON.stringify(data ?? null));
  }

  get running() {
    return this.ctx.container.running;
  }

  // The container, started when it is not running: a new generation,
  // which its saves are fenced by, its egress intercepted, its exit watched.
  async ensure(why) {
    const c = this.ctx.container;
    if (!c.running) {
      const generation = (this.get("generation") ?? 0) + 1;
      this.put("generation", generation);
      const opts = { env: { DOOR_GENERATION: String(generation) }, enableInternet: true, instance: "lite" };
      const images = c.images || {};
      if (images.door) opts.image = images.door;
      const t0 = Date.now();
      c.start(opts);
      this.put("startedAt", t0);
      this.stat("container_start", { why, generation }, generation);
      const name = this.get("name");
      await this.settled(() => c.interceptOutboundHttp("door.fragment.internal", this.ctx.exports.DoorEgress({ props: { door: name } })));
      this.stat("intercepted", { ms: Date.now() - t0 }, generation);
    }
    this.watch();
    const alarm = await this.ctx.storage.getAlarm();
    if (alarm === null) await this.ctx.storage.setAlarm(Date.now() + WATCH_MS);
  }

  watch() {
    const c = this.ctx.container;
    if (!c.running || this.watching === this.get("generation")) return;
    const generation = this.get("generation");
    this.watching = generation;
    c.monitor().then(
      () => this.exited(generation, null),
      (e) => this.exited(generation, String((e && e.message) || e)),
    );
  }

  async exited(generation, why) {
    this.stat("container_exit", { why }, generation);
    if (this.watching === generation) this.watching = 0;
    if (this.get("paused")) return;
    try {
      await this.ensure("exited");
    } catch (e) {
      this.stat("start_failed", { why: String(e) });
    }
  }

  // A start answers "temporarily unavailable" for a while: asked again.
  async settled(f) {
    const t0 = Date.now();
    for (;;) {
      try {
        return await f();
      } catch (e) {
        if (!/temporarily unavailable|not running|starting/i.test(String(e)) || Date.now() - t0 > 120_000) throw e;
        await new Promise((r) => setTimeout(r, 100));
      }
    }
  }

  async alarm() {
    try {
      if (!this.get("paused")) await this.ensure("alarm");
      const last = this.get("healthAt") ?? 0;
      if (Date.now() - last > HEALTH_EVERY_MS && this.running) {
        this.put("healthAt", Date.now());
        const h = await this.runner("GET", "/health").catch((e) => ({ error: String(e) }));
        this.stat("health", h);
      }
    } finally {
      await this.ctx.storage.setAlarm(Date.now() + WATCH_MS);
    }
  }

  // A call to the runner's HTTP API in the container.
  async runner(method, path, body) {
    const init = { method, headers: { "content-type": "application/json" } };
    if (body !== undefined) init.body = typeof body === "string" ? body : JSON.stringify(body);
    const r = await this.ctx.container.getTcpPort(PORT).fetch(`http://door${path}`, init);
    const text = await r.text();
    let v;
    try {
      v = JSON.parse(text);
    } catch {
      v = { text };
    }
    if (!r.ok) throw Object.assign(new Error(`runner ${path}: ${r.status} ${text.slice(0, 300)}`), { status: r.status, answer: v });
    return v;
  }

  config() {
    const c = this.get("config") ?? {};
    return { ...c, relay: { hosts: RELAY_HOSTS, port: 443 }, name: c.name ?? "Agent", ttl: c.ttl ?? "day" };
  }

  // ---- saves ----

  newest() {
    const pinned = this.get("restoreFrom");
    if (pinned !== null) {
      const r = this.ctx.storage.sql.exec("SELECT * FROM saves WHERE id = ?", pinned).toArray()[0];
      if (r) return r;
    }
    return this.ctx.storage.sql.exec("SELECT * FROM saves ORDER BY id DESC LIMIT 1").toArray()[0] ?? null;
  }

  blob(save) {
    const parts = this.ctx.storage.sql.exec("SELECT data FROM parts WHERE save = ? ORDER BY idx", save.id).toArray();
    const out = new Uint8Array(save.bytes);
    let at = 0;
    for (const p of parts) {
      const b = new Uint8Array(p.data);
      out.set(b, at);
      at += b.byteLength;
    }
    if (at !== save.bytes) throw new Error(`save ${save.id}: ${at} bytes of ${save.bytes}`);
    return out;
  }

  store(generation, seq, bytes, sha, why) {
    const t0 = Date.now();
    const parts = Math.max(1, Math.ceil(bytes.byteLength / PART_MAX));
    let id;
    this.ctx.storage.transactionSync(() => {
      id = this.ctx.storage.sql.exec("INSERT INTO saves (generation, seq, at, bytes, parts, sha, why) VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id", generation, seq, Date.now(), bytes.byteLength, parts, sha, why).one().id;
      for (let i = 0; i < parts; i++) {
        this.ctx.storage.sql.exec("INSERT INTO parts (save, idx, data) VALUES (?, ?, ?)", id, i, bytes.subarray(i * PART_MAX, (i + 1) * PART_MAX));
      }
      const old = this.ctx.storage.sql.exec("SELECT id FROM saves ORDER BY id DESC LIMIT 1 OFFSET ?", SAVES_KEPT).toArray()[0];
      if (old) {
        this.ctx.storage.sql.exec("DELETE FROM parts WHERE save <= ?", old.id);
        this.ctx.storage.sql.exec("DELETE FROM saves WHERE id <= ?", old.id);
      }
    });
    return { id, parts, storeMs: Date.now() - t0 };
  }

  // ---- routes ----

  async fetch(request) {
    const url = new URL(request.url);
    const [, area, ...rest] = url.pathname.split("/");
    const path = rest.join("/");
    const name = request.headers.get("x-door-name");
    if (name && !this.get("name")) this.put("name", name);
    try {
      if (area === "egress") return await this.egress(request, path, url);
      if (area === "job") return await this.job(request, path);
      if (area === "admin") return await this.admin(request, path, url);
      return json({ error: "no such route" }, 404);
    } catch (e) {
      return json({ error: String((e && e.message) || e) }, e.status && e.status < 600 ? (e.status >= 500 ? e.status : 502) : 500);
    }
  }

  // The container's own calls.
  async egress(request, path, url) {
    const generation = Number(request.headers.get("x-door-generation"));
    const current = this.get("generation");
    if (path === "save" && request.method === "GET") {
      const s = this.newest();
      this.put("restoreFrom", null);
      if (!s) return new Response(null, { status: 204 });
      this.stat("restore_served", { save: s.id, bytes: s.bytes, generationAsked: generation });
      return new Response(this.blob(s), { headers: { "content-type": "application/octet-stream", "x-save-seq": String(s.id), "x-save-generation": String(s.generation) } });
    }
    if (path === "save" && request.method === "PUT") {
      if (generation !== current) {
        this.stat("save_fenced", { generation, current });
        return json({ error: `generation ${generation} is not the running one (${current})` }, 409);
      }
      const bytes = new Uint8Array(await request.arrayBuffer());
      const seq = Number(url.searchParams.get("seq"));
      const kept = this.store(generation, seq, bytes, request.headers.get("x-save-sha256"), "change");
      return json({ ok: true, ...kept });
    }
    if (path === "config") return json(this.config());
    if (path === "stat" && request.method === "POST") {
      const v = await request.json();
      this.stat(String(v.kind).slice(0, 40), v.data, generation);
      const n = this.ctx.storage.sql.exec("SELECT max(id) - min(id) AS n FROM stats").one().n;
      if (n > STATS_KEPT) this.ctx.storage.sql.exec("DELETE FROM stats WHERE id < (SELECT max(id) FROM stats) - ?", STATS_KEPT);
      return json({ ok: true });
    }
    return json({ error: "no such egress route" }, 404);
  }

  // The connector's jobs (cell/src/jobs.rs routes `door.fragment.internal`
  // here for its own door alone).
  async job(request, path) {
    if (path === "connect") {
      const v = await request.json();
      const secret = request.headers.get("x-door-secret");
      if (!secret || !v.inbox) return json({ error: "connect names its inbox and carries its secret" }, 400);
      this.put("config", { inbox: v.inbox, secret, header: "x-simplex-door-secret", name: v.name ?? null, ttl: v.ttl ?? "day" });
      this.put("paused", null);
      await this.ensure("connect");
      if (this.running) await this.runner("POST", "/config", this.config()).catch(() => {});
      this.stat("connect", { inbox: v.inbox });
      return json({ ok: true, generation: this.get("generation") });
    }
    await this.ensure("job");
    if (path === "link") return json(await this.runner("POST", "/link", {}));
    if (path === "send") {
      const v = await request.json();
      const key = String(v.key ?? request.headers.get("idempotency-key") ?? "");
      const had = key ? this.ctx.storage.sql.exec("SELECT answer FROM sent WHERE key = ?", key).toArray()[0] : null;
      if (had) return json({ ...JSON.parse(had.answer), replayed: true });
      const t0 = Date.now();
      const answer = await this.runner("POST", "/send", { text: v.text });
      if (key) this.ctx.storage.sql.exec("INSERT INTO sent (key, at, answer) VALUES (?, ?, ?) ON CONFLICT DO NOTHING", key, Date.now(), JSON.stringify(answer));
      this.stat("sent", { key, ms: Date.now() - t0, items: answer.items });
      return json(answer);
    }
    if (path === "status") return json({ generation: this.get("generation"), running: this.running, runner: await this.runner("GET", "/health").catch((e) => ({ error: String(e) })) });
    return json({ error: "no such door call" }, 404);
  }

  // The spike's driver.
  async admin(request, path, url) {
    const c = this.ctx.container;
    switch (path) {
      case "status": {
        const saves = this.ctx.storage.sql.exec("SELECT count(*) AS n, sum(bytes) AS bytes, max(id) AS newest FROM saves").one();
        const runner = c.running ? await this.runner("GET", "/health").catch((e) => ({ error: String(e) })) : null;
        return json({ name: this.get("name"), generation: this.get("generation"), running: c.running, startedAt: this.get("startedAt"), paused: this.get("paused"), config: { ...this.config(), secret: this.get("config")?.secret ? "(set)" : null }, saves, runner });
      }
      case "start":
        this.put("paused", null);
        await this.ensure("admin");
        return json({ generation: this.get("generation"), running: c.running });
      case "stats": {
        const since = Number(url.searchParams.get("since") ?? 0);
        const kind = url.searchParams.get("kind");
        const limit = Math.min(Number(url.searchParams.get("limit") ?? 5000), 20000);
        const rows = kind
          ? this.ctx.storage.sql.exec("SELECT * FROM stats WHERE id > ? AND kind = ? ORDER BY id LIMIT ?", since, kind, limit).toArray()
          : this.ctx.storage.sql.exec("SELECT * FROM stats WHERE id > ? ORDER BY id LIMIT ?", since, limit).toArray();
        return json({ stats: rows.map((r) => ({ ...r, data: JSON.parse(r.data) })) });
      }
      case "saves":
        return json({ saves: this.ctx.storage.sql.exec("SELECT id, generation, seq, at, bytes, parts, sha FROM saves ORDER BY id DESC LIMIT ?", Number(url.searchParams.get("limit") ?? 50)).toArray() });
      case "save": {
        const s = this.ctx.storage.sql.exec("SELECT * FROM saves WHERE id = ?", Number(url.searchParams.get("id"))).toArray()[0];
        if (!s) return json({ error: "no such save" }, 404);
        return new Response(this.blob(s), { headers: { "content-type": "application/octet-stream", "x-save-seq": String(s.id) } });
      }
      case "kill": {
        // SIGKILL to the runner (PID 1): a crash, not a stop; or SIGTERM
        const sig = Number(url.searchParams.get("signal") ?? 9);
        const at = Date.now();
        this.put("killedAt", at);
        if (c.running) c.signal(sig);
        this.stat("killed", { signal: sig, at });
        return json({ killed: true, signal: sig, at, generation: this.get("generation") });
      }
      case "rollback": {
        // the next start restores save `to` (older than the newest), once
        const to = Number(url.searchParams.get("to"));
        const s = this.ctx.storage.sql.exec("SELECT id, generation, seq, at FROM saves WHERE id = ?", to).toArray()[0];
        if (!s) return json({ error: "no such save" }, 404);
        this.put("restoreFrom", to);
        const at = Date.now();
        if (c.running) c.signal(9);
        this.stat("rollback", { to: s, at });
        return json({ rollback: s, at });
      }
      case "pause": {
        // stopped and not started again (the move's "the old core never runs again")
        this.put("paused", true);
        if (c.running) c.signal(15);
        return json({ paused: true });
      }
      case "adopt": {
        // a link staged from a computer (#1207's files, packed as a save):
        // the door's next start runs on it
        const bytes = new Uint8Array(await request.arrayBuffer());
        const generation = (this.get("generation") ?? 0) + 1;
        const kept = this.store(generation, 0, bytes, request.headers.get("x-save-sha256"), "adopt");
        this.put("restoreFrom", kept.id);
        this.stat("adopt", { ...kept, bytes: bytes.byteLength });
        if (c.running) c.signal(9);
        else await this.ensure("adopt");
        return json({ adopted: kept });
      }
      case "runner": {
        const v = await request.json();
        return json(await this.runner(v.method ?? "GET", v.path, v.body));
      }
      default:
        return json({ error: "no such admin route" }, 404);
    }
  }
}

// Every intercepted request a door's container makes: to its own door
// alone (`props.door`, set by the door as it started it), and its XFTP
// relay, piped as a computer's is (entry.mjs `relay`), to SimpleX's file
// servers on 443 alone.
export class DoorEgress extends WorkerEntrypoint {
  async fetch(request) {
    const door = this.ctx.props.door;
    const url = new URL(request.url);
    if (url.pathname === "/api/computer/relay") return relay(this.ctx, url, request, door);
    const stub = this.env.SIMPLEX_DOOR.get(this.env.SIMPLEX_DOOR.idFromName(door));
    const headers = new Headers(request.headers);
    headers.set("x-door-name", door);
    return stub.fetch(new Request(`http://door/egress${url.pathname}${url.search}`, { method: request.method, headers, body: request.body }));
  }
}

const RELAY_FRAME_MAX = 256 * 1024;
const RELAY_QUEUE_MAX = 1024 * 1024;
const RELAY_BYTES_MAX = 64 * 1024 * 1024;
const RELAY_MS_MAX = 30 * 60_000;
function relay(ctx, url, request, door) {
  const host = url.searchParams.get("host");
  const port = Number(url.searchParams.get("port"));
  if ((request.headers.get("upgrade") ?? "").toLowerCase() !== "websocket") return json({ error: "a relay is a WebSocket" }, 400);
  if (!RELAY_HOSTS.includes(host) || port !== 443) return json({ error: `the relay carries SimpleX's file servers on 443, not ${host}:${port}` }, 403);
  const [client, server] = Object.values(new WebSocketPair());
  server.accept();
  const t0 = Date.now();
  let bytes = 0;
  let queued = 0;
  let ended = false;
  const socket = connect({ hostname: host, port }, { secureTransport: "off", allowHalfOpen: false });
  const writer = socket.writable.getWriter();
  const end = (why, code) => {
    if (ended) return;
    ended = true;
    clearTimeout(timer);
    try {
      server.close(code, why);
    } catch {}
    socket.close().catch(() => {});
    console.log(JSON.stringify({ doorRelay: "close", door, host, bytes, ms: Date.now() - t0, why }));
  };
  const timer = setTimeout(() => end("its time is up", 4008), RELAY_MS_MAX);
  const counted = (n) => {
    bytes += n;
    if (bytes <= RELAY_BYTES_MAX) return true;
    end("its bytes are spent", 4008);
    return false;
  };
  server.addEventListener("message", (e) => {
    if (ended) return;
    const data = typeof e.data === "string" ? new TextEncoder().encode(e.data) : new Uint8Array(e.data);
    if (data.byteLength === 0) return;
    if (data.byteLength > RELAY_FRAME_MAX) return end("a frame past its bound", 4008);
    if (!counted(data.byteLength)) return;
    queued += data.byteLength;
    if (queued > RELAY_QUEUE_MAX) return end("the server is not taking what it is sent", 4008);
    writer.write(data).then(() => {
      queued -= data.byteLength;
    }, () => end("the server's side failed", 1011));
  });
  server.addEventListener("close", () => end("the door closed it", 1000));
  server.addEventListener("error", () => end("the door's side failed", 1011));
  socket.opened.then(() => console.log(JSON.stringify({ doorRelay: "open", door, host })), () => end("the server did not answer", 1011));
  ctx.waitUntil(
    (async () => {
      const reader = socket.readable.getReader();
      try {
        for (;;) {
          const { value, done } = await reader.read();
          if (done || ended) break;
          if (!counted(value.byteLength)) break;
          server.send(value);
        }
        end("the server closed it", 1000);
      } catch {
        end("the server's side failed", 1011);
      }
    })(),
  );
  return new Response(null, { status: 101, webSocket: client });
}

// The spike's admin routes on the platform host: `/api/sxdoor/<connector>/admin/<op>`,
// with the test levers' secret (`x-fragment-test-secret`).
export async function sxdoorAdmin(request, env) {
  const url = new URL(request.url);
  const m = /^\/api\/sxdoor\/([a-z0-9-]{1,80})\/admin\/([a-z]+)$/.exec(url.pathname);
  if (!m) return null;
  const secret = env.FRAGMENT_TEST_SECRET ? String(env.FRAGMENT_TEST_SECRET).trim() : "";
  if (!secret || request.headers.get("x-fragment-test-secret") !== secret) return json({ error: "not found" }, 404);
  const stub = env.SIMPLEX_DOOR.get(env.SIMPLEX_DOOR.idFromName(m[1]));
  const headers = new Headers(request.headers);
  headers.set("x-door-name", m[1]);
  headers.delete("x-fragment-test-secret");
  return stub.fetch(new Request(`http://door/admin/${m[2]}${url.search}`, { method: request.method, headers, body: request.body }));
}
