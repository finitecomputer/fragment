// A computer's container on a sandcastle node (docs/self-host.md, seam 2).
// `NodeContainer` is Cloudflare's container API, `ctx.container`, as far as
// `ContainerHost` (entry.mjs) and the Sandbox SDK's `DirectoryBackup` call
// it, over the node's API (sandcastle's docs/node.md): neither, nor
// computer.rs, nor the image, knows where its container runs. Every call is
// signed with the node's secret. The node hands each intercepted request to
// the platform's `/api/nodes/egress`, signed; the router passes it to the
// computer's object, which checks it here again (`nodeEgress`) and gives it
// to the binding its container set for that intercept.
//
// The deployment's nodes are `FRAGMENT_NODES`, which Rust reads and checks
// (fragment_core::placement); computer.rs places a computer, then pins its
// host to the node (`pin`), handing over what this needs of it. A node that
// does not answer is `NodeDown`, which the cell answers as an upstream's
// failure, typed, within a bound: a call never hangs on it.

const AUTH = "x-sandcastle-auth";
// a signature's timestamp, at most this far from our clock (the node's)
const WINDOW_S = 60;
const SECRET_BYTES_MIN = 32;
// an exec frame's payload, at most (the engine's)
const FRAME_PAYLOAD_MAX = 1 << 20;
// an intercepted request's body, at most (the node's, the engine's)
export const EGRESS_BODY_MAX = 32 << 20;
// a node's health answers within this, or it is down
const HEALTH_MS = 6000;
// any other call but a start, a `wait`, an exec's stream and a port's
const CALL_MS = 30_000;
// a start: the engine boots the guest before it answers (`wait=ready`)
const START_MS = 120_000;
// an exec's socket, closed from this end as its process exits, has closed
// within this, or its exit is answered all the same
const CLOSE_WAIT_MS = 2000;
// a `wait` cut short sooner than this after it was asked is a quick one; at
// most WAIT_QUICK_MAX of those in a row are asked again (`#waited`)
const WAIT_QUICK_MS = 5000;
const WAIT_QUICK_MAX = 3;
// A node silent this long under a running container (its `wait` cut, and
// every look since unanswered) is said to be down (docs/self-host.md, found
// 39). Longer than every outage a node heals by itself: its process or its
// engine restarting (seconds; the engine keeps its VMs). Short enough that
// the owner hears within two minutes, not after an awake computer that
// answers nothing. Saying so changes what the computer says, never what
// runs: the container is still waited on, and goes on when its node answers.
const SILENT_MS = 120_000;
// the pause between looks at a silent node: doubling from the first to the
// most, which is how late a node that answers again is heard
const LOOK_PAUSE_MS = 1000;
const LOOK_PAUSE_MAX_MS = 30_000;
const enc = new TextEncoder();

// A sandcastle node that did not answer, or answered as a gateway that it
// could not be reached: the cell's `upstream_failed` (js.rs reads the name).
export class NodeDown extends Error {
  constructor(node, why) {
    super(`its node ${node} does not answer: ${why}`);
    this.name = "NodeDown";
    this.node = node;
    this.why = why;
  }
}

// `promise`, or NodeDown for `node` after `ms`.
function within(node, ms, promise) {
  let timer;
  const late = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new NodeDown(node, `no answer within ${ms / 1000} s`)), ms);
  });
  return Promise.race([promise, late]).finally(() => clearTimeout(timer));
}

const hex = (buf) => [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, "0")).join("");
const sha256Hex = async (bytes) => hex(await crypto.subtle.digest("SHA-256", bytes));

function unhex(s) {
  const out = new Uint8Array(s.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(s.slice(2 * i, 2 * i + 2), 16);
  return out;
}

const callString = (method, path, t, body) => `sandcastle-node-v1\n${method}\n${path}\n${t}\n${body}`;
const egressString = (e, t, body) =>
  `sandcastle-egress-v1\n${e.method}\n${e.container}\n${e.intercept}\n${e.scheme}\n${e.host}\n${e.path}\n${t}\n${body}`;

// One node, as Rust named it (fragment_core::placement `Pin` and `Asked`:
// its id, origin and the binding of its secret): `key`, its secret's HMAC
// key, read once from that binding (a store's `get()`, which cell/secrets.mjs
// stands in for where the runtime has none). A node whose secret is missing
// or short fails every call, saying so.
export function nodeOf(env, n) {
  const key = (async () => {
    const binding = env[n.secret];
    const value = typeof binding?.get === "function" ? await binding.get() : binding;
    const secret = String(value || "").trim();
    if (enc.encode(secret).length < SECRET_BYTES_MIN) throw new Error(`${n.secret}, the node ${n.id}'s secret, is at least ${SECRET_BYTES_MIN} bytes`);
    return crypto.subtle.importKey("raw", enc.encode(secret), { name: "HMAC", hash: "SHA-256" }, false, ["sign", "verify"]);
  })();
  // a key never asked for leaves no rejection unheard; one asked for throws there
  key.catch(() => {});
  return { id: n.id, url: n.url, key, fetch: (path, init) => fetch(n.url + path, init) };
}

// What placing a computer needs of each node (computer.rs `place`): its
// health within HEALTH_MS, the architecture it reports, and its engine's
// VMs, now and at most. One answer each, as `placement::Probe` reads it.
export async function probe(env, asked) {
  return Promise.all(
    asked.map(async (n) => {
      const node = nodeOf(env, n);
      try {
        const r = await within(node.id, HEALTH_MS, signedFetch(node, "GET", "/v1/health"));
        const h = await r.json().catch(() => ({}));
        if (!r.ok) return { id: node.id, down: `its health answered ${r.status} ${h.error || ""}`.trim() };
        return { id: node.id, arch: h?.node?.arch ?? null, vms: h?.vms ?? null, vmsMax: h?.vms_max ?? null };
      } catch (e) {
        return { id: node.id, down: String((e && e.message) || e) };
      }
    }),
  );
}

// A signed call to `node` with no body, its transport's failure NodeDown.
async function signedFetch(node, method, path) {
  const headers = { [AUTH]: await signed(node, method, path, await sha256Hex(new Uint8Array())) };
  try {
    return await node.fetch(path, { method, headers });
  } catch (e) {
    throw e instanceof NodeDown ? e : new NodeDown(node.id, String((e && e.message) || e));
  }
}

async function authHeader(node, t, canonical) {
  return `t=${t},sig=${hex(await crypto.subtle.sign("HMAC", await node.key, enc.encode(canonical)))}`;
}

function signed(node, method, path, bodyHash) {
  const t = Math.floor(Date.now() / 1000);
  return authHeader(node, t, callString(method, path, t, bodyHash));
}

// A signature header's time and signature, when it is well formed and its
// time inside the window; else null. Checked before anything else is read.
function inWindow(header) {
  const m = /^t=(\d{1,12}),sig=([0-9a-f]{64})$/.exec(header || "");
  if (!m) return null;
  const t = Number(m[1]);
  return Math.abs(Math.floor(Date.now() / 1000) - t) > WINDOW_S ? null : { t, sig: m[2] };
}

// Whether `signed` (`inWindow`'s) is the node's over the string
// `canonicalOf(t)` makes.
async function verified(node, signed, canonicalOf) {
  return crypto.subtle.verify("HMAC", await node.key, unhex(signed.sig), enc.encode(canonicalOf(signed.t)));
}

// A request's body, read to its end, or null once it passes `max` bytes:
// what it holds is never more than that and one chunk, and the rest is
// cancelled unread (a body with no declared length streams on).
async function bounded(request, max) {
  if (!request.body) return new Uint8Array();
  const reader = request.body.getReader();
  const chunks = [];
  let size = 0;
  // bounded by `max`: each pass reads one chunk, and past `max` it stops
  for (;;) {
    const { value, done } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > max) {
      await reader.cancel().catch(() => {});
      return null;
    }
    chunks.push(value);
  }
  const body = new Uint8Array(size);
  let at = 0;
  for (const c of chunks) {
    body.set(c, at);
    at += c.byteLength;
  }
  return body;
}

// One exec frame: the engine's 8-byte header (a stream byte, three zeroes,
// a big-endian length) and its payload.
function frame(stream, payload) {
  const out = new Uint8Array(8 + payload.length);
  out[0] = stream;
  new DataView(out.buffer).setUint32(4, payload.length);
  out.set(payload, 8);
  return out;
}

// A process exec started on the node, as Cloudflare's: stdin a
// WritableStream, stdout and stderr ReadableStreams (when piped), its exit
// code a promise, `kill`, and `output()`.
class NodeProcess {
  stdin = null;
  stdout = null;
  stderr = null;
  pid = null;
  exitCode;
  #ws;
  #gone;

  constructor(ws, spec, opts) {
    this.#ws = ws;
    let out = null;
    let err = null;
    if (spec.stdout === "pipe") this.stdout = new ReadableStream({ start: (c) => { out = c; } });
    if (spec.stderr === "pipe") this.stderr = new ReadableStream({ start: (c) => { err = c; } });
    let settle;
    this.exitCode = new Promise((resolve, reject) => { settle = { resolve, reject }; });
    let done = false;
    // the socket is closed (or closing) from this end: nothing more is sent
    let gone = false;
    this.#gone = () => gone;
    const end = (e) => {
      if (done) return;
      done = true;
      for (const c of [out, err]) {
        try { e ? c?.error(e) : c?.close(); } catch {}
      }
    };
    // The process is over: its socket is closed from this end too, and its
    // exit is answered once the socket has closed (within CLOSE_WAIT_MS): a
    // socket still open as the request that made it ends holds that request
    // on celld (docs/self-host.md, found 13).
    let finished = null;
    const answer = () => {
      if (!finished || finished.answered) return;
      finished.answered = true;
      if (finished.error) settle.reject(finished.error);
      else settle.resolve(finished.code);
    };
    const finish = (outcome) => {
      if (finished) return;
      finished = outcome;
      gone = true;
      try { ws.close(1000, "exited"); } catch {}
      setTimeout(answer, CLOSE_WAIT_MS);
    };
    ws.addEventListener("message", (m) => {
      if (typeof m.data === "string") return;
      const b = new Uint8Array(m.data);
      if (b.length < 8) return;
      const len = new DataView(b.buffer, b.byteOffset, b.byteLength).getUint32(4);
      const payload = b.slice(8, 8 + len);
      const json = () => JSON.parse(new TextDecoder().decode(payload));
      switch (b[0]) {
        case 1: out?.enqueue(payload); break;
        case 2: err?.enqueue(payload); break;
        case 4: this.pid = json().pid; break;
        case 3: {
          const x = json();
          end(null);
          finish({ code: x.code ?? 128 + (x.signal ?? 0) });
          break;
        }
        case 5: {
          const e = new Error(`exec: ${json().error}`);
          end(e);
          finish({ error: e });
          break;
        }
      }
    });
    const closed = () => {
      gone = true;
      if (finished) return answer();
      const e = new Error("exec: the node's socket closed before the process exited");
      end(e);
      finished = { error: e };
      answer();
    };
    ws.addEventListener("close", closed);
    ws.addEventListener("error", closed);
    // the node takes an exec's spec as its first message, before any frame
    // (its stdin, a kill): sent once its answers are listened for
    ws.send(JSON.stringify(spec));
    if (spec.stdin) {
      const write = (chunk) => {
        if (gone) throw new Error("exec: the process has exited");
        const bytes = typeof chunk === "string" ? enc.encode(chunk) : new Uint8Array(chunk);
        for (let at = 0; at < bytes.length; at += FRAME_PAYLOAD_MAX) ws.send(frame(0, bytes.subarray(at, at + FRAME_PAYLOAD_MAX)));
      };
      const eof = () => {
        if (gone) return;
        try { ws.send(frame(0, new Uint8Array())); } catch {}
      };
      if (opts.stdin === "pipe") {
        this.stdin = new WritableStream({ write, close: eof, abort: eof });
      } else if (typeof opts.stdin === "string") {
        write(opts.stdin);
        eof();
      } else {
        (async () => {
          for await (const chunk of opts.stdin) write(chunk);
          eof();
        })().catch(eof);
      }
    }
    if (opts.signal) {
      const abort = () => this.kill(9);
      if (opts.signal.aborted) abort();
      else opts.signal.addEventListener("abort", abort, { once: true });
    }
  }

  kill(signal = 15) {
    if (this.#gone()) return;
    try { this.#ws.send(frame(7, enc.encode(JSON.stringify({ signal })))); } catch {}
  }

  async output() {
    const all = async (s) => (s ? new Uint8Array(await new Response(s).arrayBuffer()) : new Uint8Array());
    const [stdout, stderr] = await Promise.all([all(this.stdout), all(this.stderr)]);
    return { exitCode: await this.exitCode, stdout, stderr };
  }
}

// One computer's container on its node: `ctx.container`'s face.
export class NodeContainer {
  #node;
  #name;
  #images;
  #running = false;
  // whether `#running` is the node's word (false: it did not answer)
  #known = false;
  // the start in flight: every later call waits for it
  #starting = null;
  // each start's own number: what a call about an earlier container (its
  // `wait` answered, its destroy done) may change of `running` is nothing,
  // once another started under the same name
  #life = 0;
  // the intercepts this start set, in order: the node names one by its index
  #intercepts = [];
  // inside `intercepting`: the list is sent once, at its end
  #batched = false;

  constructor(node, name, images) {
    this.#node = node;
    this.#name = name;
    this.#images = images;
  }

  get running() {
    return this.#running;
  }

  // The images its node runs, by name (its architecture's).
  get images() {
    return this.#images;
  }

  // The container's name on the node: the computer's id.
  get name() {
    return this.#name;
  }

  // The node it is on.
  get node() {
    return this.#node.id;
  }

  // What the node says of this container: an isolate's first look, before
  // `running` is read, bounded as a health is. A node that does not answer
  // has said nothing (`#known` false): asked again by `known`.
  async refresh() {
    try {
      const r = await this.#call("GET", `/v1/containers/${this.#name}`, undefined, [404], HEALTH_MS);
      this.#running = r.status === 404 ? false : (await r.json()).running === true;
      this.#known = true;
    } catch (e) {
      console.log(JSON.stringify({ node: this.#node.id, container: this.#name, refresh: String((e && e.message) || e) }));
      this.#running = false;
      this.#known = false;
    }
  }

  // What the node says of this container, asked again while it has said
  // nothing: a container still running there is taken over, not started
  // again. A node still silent leaves it not running.
  async known() {
    if (!this.#known) await this.refresh();
  }

  // A container whose node could not be asked (`known` found it silent),
  // taken as running from now on: its watch (`monitor`) learns what became
  // of it, since silence is no exit (docs/self-host.md, found 39). False
  // when the node has said.
  presume() {
    if (this.#known) return false;
    this.#running = true;
    this.#known = true;
    return true;
  }

  // The binding the container's intercept `index` names, if this isolate set it.
  binding(index) {
    return this.#intercepts[index]?.fetcher || null;
  }

  // A signed call, answered within `ms` (none: a `wait`, as long as the
  // container lives). A transport that fails, or a gateway's 502, 503 or
  // 504 (a proxy in front of the node), is NodeDown; any other refusal is
  // the node's own, its status kept.
  async #call(method, path, body, allowed = [], ms = CALL_MS) {
    const node = this.#node.id;
    const bytes = body === undefined ? new Uint8Array() : enc.encode(JSON.stringify(body));
    const headers = { [AUTH]: await signed(this.#node, method, path, await sha256Hex(bytes)) };
    if (body !== undefined) headers["content-type"] = "application/json";
    const sent = this.#node.fetch(path, { method, headers, body: body === undefined ? undefined : bytes }).catch((e) => {
      throw e instanceof NodeDown ? e : new NodeDown(node, String((e && e.message) || e));
    });
    const resp = await (ms ? within(node, ms, sent) : sent);
    if (!resp.ok && !allowed.includes(resp.status)) {
      const e = await resp.json().catch(() => ({}));
      if ([502, 503, 504].includes(resp.status)) throw new NodeDown(node, `${method} ${path}: ${resp.status} ${e.error || ""}`.trim());
      throw Object.assign(new Error(`the node ${node}: ${method} ${path}: ${resp.status} ${e.error || ""} ${e.message || ""}`.trim()), { status: resp.status });
    }
    return resp;
  }

  async #json(method, path, body, ms = CALL_MS) {
    const text = await (await this.#call(method, path, body, [], ms)).text();
    return text ? JSON.parse(text) : null;
  }

  async #started() {
    if (this.#starting) await this.#starting;
  }

  // Cloudflare's start returns at once, the container running from then;
  // the node's start is in flight behind it, and every later call waits
  // for it (`#started`). Its health first: a node that is down says so
  // within HEALTH_MS, where a start's own bound is the boot's. Answered once
  // its guest is up (`wait=ready`): the engine refuses an exec before its
  // guest is up (409 "still starting"), which an immediate sleep's hold met
  // once a start's intercepts took one call (docs/self-host.md, seam 2).
  start(opts = {}) {
    const body = { enableInternet: opts.enableInternet === true, env: opts.env || {} };
    for (const k of ["image", "containerSnapshot", "entrypoint", "instance", "labels"]) if (opts[k] !== undefined) body[k] = opts[k];
    const life = ++this.#life;
    this.#running = true;
    this.#intercepts = [];
    const path = `/v1/containers/${this.#name}/start?wait=ready`;
    const begin = () => this.#json("POST", path, body, START_MS);
    this.#known = true;
    this.#starting = this.#call("GET", "/v1/health", undefined, [], HEALTH_MS)
      .then(begin)
      .catch(async (e) => {
        if (e.status !== 409) throw e;
        // one of its name runs there, which this isolate could not see
        // (lesson 6: never reused half-known): it goes, and this one starts
        await this.#json("POST", `/v1/containers/${this.#name}/destroy`, { error: "a new start" });
        return begin();
      })
      .then((started) => {
        // the engine's own account of the start (prepared, jailed, its
        // guest's hello, ready), for "Where the time goes"
        console.log(JSON.stringify({ node: this.#node.id, container: this.#name, started: started?.timings ?? null }));
      })
      .catch((e) => {
        if (this.#life === life) this.#running = false;
        throw e;
      });
    this.#starting.catch(() => {});
  }

  // Resolves when the container exits cleanly or is destroyed; rejects on a
  // failed start, a nonzero exit, a signal, or a destroy with an error.
  // `said` hears why its node has been silent past SILENT_MS, and null once
  // it answers again (`#look`).
  async monitor(said = async () => {}) {
    const life = this.#life;
    try {
      await this.#started();
    } catch (e) {
      // a start that failed is the start's to report (ContainerHost's
      // `arm` throws it), never an exit of a container that ran
      throw Object.assign(new Error(String((e && e.message) || e)), { startFailed: true });
    }
    const exit = await this.#waited(life, said);
    // the container it watched is gone; a start since is another's
    if (this.#life === life) this.#running = false;
    if (exit.error) throw new Error(exit.error);
    if (exit.destroyed || exit.code === 0) return;
    throw new Error(exit.signal != null ? `the container was killed by signal ${exit.signal}` : `the container exited with code ${exit.code}`);
  }

  // The container's exit, from the node's `wait`: a long poll for the
  // container's life. A transport may end it first (celld bounds an
  // outbound fetch, CELLD_FETCH_TIMEOUT_S; a proxy may close a quiet
  // connection), and that is no exit: the node is asked whether the
  // container still runs, and one that does is waited on again (found 22);
  // a node that does not answer is asked until it does (`#look`, found 39).
  // Thrown: a refusal of the node's own; a cut poll when the container no
  // longer runs there, or another start took its name; and the fourth quick
  // cut in a row the node answered at once (a node that answers its look
  // but drops its wait).
  async #waited(life, said) {
    let quick = 0;
    // bounded: each pass is a poll the container outlived, the quick ones
    // (under WAIT_QUICK_MS, the node answering the look at once) at most
    // WAIT_QUICK_MAX in a row
    for (;;) {
      const asked = Date.now();
      try {
        return await this.#json("GET", `/v1/containers/${this.#name}/wait`, undefined, null);
      } catch (e) {
        if (e.status !== undefined) throw e;
        const cutMs = Date.now() - asked;
        const look = await this.#look(life, said);
        quick = cutMs < WAIT_QUICK_MS && !look.silent ? quick + 1 : 0;
        console.log(JSON.stringify({ node: this.#node.id, container: this.#name, waitCut: String((e && e.message) || e), afterMs: cutMs, silentMs: look.silentMs, stillRunning: look.running }));
        if (!look.running || quick > WAIT_QUICK_MAX) throw e;
      }
    }
  }

  // After a cut `wait`: whether the container still runs, as the node says
  // (404: it is gone). A node that does not answer (NodeDown) has said
  // nothing, and is asked again, the pause doubling from LOOK_PAUSE_MS to
  // LOOK_PAUSE_MAX_MS, for as long as it is silent; past SILENT_MS `said`
  // hears why (the computer says its node is down), and null once it
  // answers. Another start under its name ends the look: what runs is no
  // longer this start's. `{running, silent, silentMs}`.
  async #look(life, said) {
    const since = Date.now();
    let pause = LOOK_PAUSE_MS;
    let told = false;
    // Unbounded by design, as the `wait` it stands in for: each look is
    // bounded by HEALTH_MS and each pause by LOOK_PAUSE_MAX_MS, for as long
    // as the node is silent and this start is the latest
    for (let silent = false; ; silent = true) {
      let r;
      try {
        r = await this.#call("GET", `/v1/containers/${this.#name}`, undefined, [404], HEALTH_MS);
      } catch (e) {
        if (!(e instanceof NodeDown)) throw e;
        if (this.#life !== life) return { running: false, silent, silentMs: Date.now() - since };
        if (!told && Date.now() - since >= SILENT_MS) {
          told = true;
          await said(`its node ${this.#node.id} has not answered for ${Math.floor((Date.now() - since) / 60_000)} minutes (${e.why}): it is kept as it was, and goes on when its node answers`);
        }
        await new Promise((resolve) => setTimeout(resolve, pause));
        pause = Math.min(2 * pause, LOOK_PAUSE_MAX_MS);
        continue;
      }
      const running = r.status === 200 && (await r.json().catch(() => ({}))).running === true;
      if (told) await said(null);
      return { running, silent, silentMs: Date.now() - since };
    }
  }

  async destroy(error) {
    const life = this.#life;
    await this.#started().catch(() => {});
    await this.#json("POST", `/v1/containers/${this.#name}/destroy`, error ? { error: String(error) } : {});
    if (this.#life === life) this.#running = false;
  }

  signal(signal) {
    this.#started()
      .then(() => this.#json("POST", `/v1/containers/${this.#name}/signal`, { signal }))
      .catch(() => {});
  }

  // The Computer object's own sleep stops an idle computer; a node keeps no
  // timer of its own.
  async setInactivityTimeout() {}

  async snapshotContainer(opts = {}) {
    await this.#started();
    const s = await this.#json("POST", `/v1/containers/${this.#name}/snapshots`, opts.name ? { name: opts.name } : {});
    return { id: s.id, size: s.size };
  }

  interceptOutboundHttp(host, fetcher) {
    return this.#intercept("http", host, fetcher);
  }

  interceptOutboundHttps(host, fetcher) {
    return this.#intercept("https", host, fetcher);
  }

  // Appended, never reordered while the container runs: the node names an
  // intercept by its index. Setting one again replaces its binding.
  async #intercept(scheme, target, fetcher) {
    await this.#started();
    const i = this.#intercepts.findIndex((x) => x.scheme === scheme && x.target === target);
    if (i >= 0) this.#intercepts[i].fetcher = fetcher;
    else this.#intercepts.push({ scheme, target, fetcher });
    if (!this.#batched) await this.#sendIntercepts();
  }

  // Every intercept `set` makes, sent to the node as one list at its end:
  // each alone is a PUT of the whole list, and a start sets one for each
  // host the swap catches, on http and https (dozens of calls through the
  // node to its engine; docs/self-host.md, seam 2).
  async intercepting(set) {
    await this.#started();
    this.#batched = true;
    try {
      await set();
    } finally {
      this.#batched = false;
    }
    await this.#sendIntercepts();
  }

  async #sendIntercepts() {
    const intercepts = this.#intercepts.map((x) => ({ scheme: x.scheme, target: x.target, action: { kind: "handler" } }));
    await this.#json("PUT", `/v1/containers/${this.#name}/intercepts`, { intercepts });
  }

  async exec(cmd, opts = {}) {
    await this.#started();
    const path = `/v1/containers/${this.#name}/exec`;
    const headers = { [AUTH]: await signed(this.#node, "GET", path, await sha256Hex(new Uint8Array())), upgrade: "websocket" };
    const node = this.#node.id;
    const sent = this.#node.fetch(path, { headers }).catch((e) => {
      throw e instanceof NodeDown ? e : new NodeDown(node, String((e && e.message) || e));
    });
    const resp = await within(node, CALL_MS, sent);
    const ws = resp.webSocket;
    if (!ws) throw new Error(`the node ${node}: exec: ${resp.status} ${await resp.text()}`);
    ws.accept();
    const stdin = opts.stdin !== undefined && opts.stdin !== "ignore";
    const spec = {
      cmd,
      env: opts.env || {},
      stdin,
      stdout: opts.stdout === "ignore" ? "ignore" : "pipe",
      stderr: opts.stderr === "combined" || opts.stderr === "ignore" ? opts.stderr : "pipe",
    };
    if (opts.cwd) spec.cwd = opts.cwd;
    if (opts.user) spec.user = opts.user;
    return new NodeProcess(ws, spec, opts);
  }

  // A guest port, HTTP and WebSocket alike: its body streams, signed as
  // unsigned (docs/node.md, Auth); the URL asked for rides along so the
  // guest sees its own host.
  getTcpPort(port) {
    return {
      fetch: async (request) => {
        await this.#started();
        const u = new URL(request.url);
        const path = `/v1/containers/${this.#name}/ports/${port}${u.pathname}${u.search}`;
        const headers = new Headers(request.headers);
        headers.set(AUTH, await signed(this.#node, request.method, path, "UNSIGNED-PAYLOAD"));
        headers.set("x-sandcastle-url", request.url);
        // a node that does not answer says so, typed, as every other call's does
        return this.#node.fetch(path, { method: request.method, headers, body: request.body, redirect: "manual" }).catch((e) => {
          throw e instanceof NodeDown ? e : new NodeDown(this.#node.id, String((e && e.message) || e));
        });
      },
    };
  }
}

const EGRESS_HEADERS = ["x-sandcastle-container", "x-sandcastle-intercept", "x-sandcastle-host", "x-sandcastle-scheme", "x-sandcastle-path"];

// An intercepted request from the node (`/api/nodes/egress`, which the
// router sent to the computer its container names): checked against its
// node's signature, rebuilt as the guest sent it, and given to its binding
// (computer.rs took the container over first, so this isolate armed it). A
// refusal is one log line (lesson 14): a guest's request answered here
// reaches no request log.
export async function nodeEgress(request, node, nodeContainer) {
  const h = request.headers;
  const [container, intercept, host, scheme, path] = EGRESS_HEADERS.map((k) => h.get(k) || "");
  const refuse = (status, error) => {
    console.log(JSON.stringify({ nodeEgress: "refused", node: node?.id ?? null, container, intercept, host, path, status, error }));
    return Response.json({ error }, { status });
  };
  if (!node || !nodeContainer) return refuse(404, "this computer is not on a node");
  if (container !== nodeContainer.name) return refuse(403, "another computer's intercept");
  // the route is public: what reaches its body first carries a signature in
  // its window, and no more of it is read than an intercept may hold
  const signed = inWindow(h.get(AUTH));
  if (!signed) return refuse(401, "no signature in its window");
  const declared = Number(h.get("content-length") || 0);
  if (!(declared <= EGRESS_BODY_MAX)) return refuse(413, `a body of at most ${EGRESS_BODY_MAX} bytes`);
  const body = await bounded(request, EGRESS_BODY_MAX);
  if (!body) return refuse(413, `a body of at most ${EGRESS_BODY_MAX} bytes`);
  const e = { method: request.method, container, intercept, scheme, host, path };
  const bodyHash = await sha256Hex(body);
  if (!(await verified(node, signed, (t) => egressString(e, t, bodyHash)))) return refuse(401, "a bad signature");
  if (!/^\d{1,3}$/.test(intercept)) return refuse(400, "an intercept's index");
  const index = Number(intercept);
  const fetcher = nodeContainer.binding(index);
  if (!fetcher) return refuse(503, `no binding for intercept ${intercept}`);
  const headers = new Headers(h);
  // the node's own, and what the platform set on its way here (computer.rs
  // `node_egress_route`, routed.rs's release): the guest sent none of them
  for (const k of [...EGRESS_HEADERS, AUTH, "host", "x-fragment-computer", "x-fragment-release", "x-fragment-release-deployed"]) headers.delete(k);
  const method = request.method;
  const hasBody = method !== "GET" && method !== "HEAD";
  // a redirect is the guest's to follow, as the runtime's own intercepts hand it back
  return fetcher.fetch(new Request(`${scheme}://${host}${path}`, { method, headers, body: hasBody ? body : undefined, redirect: "manual" }));
}
