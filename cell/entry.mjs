// The platform's hand-written JavaScript (besides platform.mjs, which runs
// inside the app facet). Everything else is Rust. The runtime gives RPC
// only to a class that extends DurableObject, and workers-rs classes do
// not, so these classes do and forward each handler. workers-rs 0.8.5 has
// no Workflows and no Containers, so the job driver and a computer's
// container calls are here too; they only call and call back: every
// decision is Rust's (jobs.rs, computer.rs).
import { DurableObject, WorkerEntrypoint, WorkflowEntrypoint } from "cloudflare:workers";
import { DirectoryBackup, DirectoryBackupGateway as SandboxGateway, SandboxBackupError, SandboxFileError } from "@cloudflare/sandbox";
import { readAgain } from "./backup-reads.mjs";
import { intercept, interceptSwap, settled, startIntercepted } from "./intercepts.mjs";
import { fpacRoute } from "./fpac.mjs";
import { relay } from "./relay.mjs";
import * as rs from "./build/index.js";
// A SimpleX door's object and its container's egress: JavaScript of its
// own (door.mjs), no Rust in the cell's Wasm
import { DoorEgress as DoorCalls } from "./door.mjs";
export { SimplexDoor } from "./door.mjs";

// Every call a door's container makes to `door.fragment.internal` goes to
// its own door (door.mjs `DoorEgress`), but an XFTP relay
// (`GET /relay?host=&port=`, a WebSocket): to one of the deployment's
// connector relay targets alone, as Rust allows (`DoorRelay.target`),
// piped by relay.mjs as a computer's is.
export class DoorEgress extends DoorCalls {
  async fetch(request) {
    const url = new URL(request.url);
    if (url.pathname !== "/relay") return super.fetch(request);
    if (request.method !== "GET" || (request.headers.get("upgrade") ?? "").toLowerCase() !== "websocket") return Response.json({ error: "invalid", message: "a relay is a WebSocket" }, { status: 400 });
    const target = rs.DoorRelay.target(this.env, url.searchParams.get("host") ?? "", url.searchParams.get("port") ?? "");
    if (target.error) {
      console.log(JSON.stringify({ relay: "refused", door: this.ctx.props.door, message: target.message }));
      return Response.json(target, { status: 403 });
    }
    return relay(this.ctx, target, { door: this.ctx.props.door }, async () => {});
  }
}

// The Sandbox SDK's gateway between a computer's container and its saves in
// R2, whose ranged reads (a restore's) that storage failed are asked again
// (backup-reads.mjs); every other request is the SDK's own, once.
export class DirectoryBackupGateway extends SandboxGateway {
  fetch(request) {
    if (request.method !== "GET") return super.fetch(request);
    const again = (tried, status) => console.log(JSON.stringify({ backup: "read again", status, tried, key: this.ctx.props?.key }));
    return readAgain(() => super.fetch(request), again);
  }
}

export default rs.default;

export class Fragment extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    this.rs = new rs.FragmentCell(ctx, env);
  }
  fetch(request) { return this.rs.fetch(request); }
  alarm(info) { return this.rs.alarm(info); }
  webSocketMessage(ws, message) { return this.rs.webSocketMessage(ws, message); }
  webSocketClose(ws, code, reason, clean) { return this.rs.webSocketClose(ws, code, reason, clean); }
  webSocketError(ws, error) { return this.rs.webSocketError(ws, error); }
}

export class Principal extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    this.rs = new rs.PrincipalCell(ctx, env);
  }
  fetch(request) { return this.rs.fetch(request); }
  alarm(info) { return this.rs.alarm(info); }
}

export class Ledger extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    this.rs = new rs.LedgerCell(ctx, env);
  }
  fetch(request) { return this.rs.fetch(request); }
  alarm(info) { return this.rs.alarm(info); }
}

// A fragment life's search index (search_index.rs): its supervisor calls it.
export class SearchIndex extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    this.rs = new rs.SearchIndexCell(ctx, env);
  }
  fetch(request) { return this.rs.fetch(request); }
  alarm(info) { return this.rs.alarm(info); }
}

// A person's Vault (vault.rs): their secrets, sealed; the router and the
// platform's own code call it.
export class Vault extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    this.rs = new rs.VaultCell(ctx, env);
  }
  fetch(request) { return this.rs.fetch(request); }
}

// The finite.computer move (migrations.rs): one object, the router's alone.
export class Migrations extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    this.rs = new rs.MigrationsCell(ctx, env);
  }
  fetch(request) { return this.rs.fetch(request); }
  alarm(info) { return this.rs.alarm(info); }
}

export class Registry extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    this.rs = new rs.RegistryCell(ctx, env);
  }
  fetch(request) { return this.rs.fetch(request); }
  alarm(info) { return this.rs.alarm(info); }
}

// The app's read access to its files (files.rs), handed to its facet as
// `env.FILES` bound to one fragment by `props`: the app cannot name another.
// It, `Search` and the job driver make the fragment's typed calls
// (`call/…`, fragment/calls.rs, which defines each one's body) through
// `rs.InternalRoute.request` (routed.rs), which marks the request as a call
// and stamps it with this release (an object refuses another's:
// docs/MODEL.md, "Calls between releases"): the JavaScript names no header.
export class Files extends WorkerEntrypoint {
  async #ask(op, body) {
    const { fragment } = this.ctx.props;
    return this.env.FRAGMENT.getByName(fragment).fetch(rs.InternalRoute.request(this.env, `call/files/${op}`, JSON.stringify(body)));
  }

  async #answer(op, body) {
    const resp = await this.#ask(op, body);
    if (resp.status === 404 && op === "read") return null;
    if (!resp.ok) {
      const e = await resp.json().catch(() => ({}));
      throw new Error(e.message || `files.${op} answered ${resp.status}`);
    }
    return op === "read" ? new Uint8Array(await resp.arrayBuffer()) : resp.json();
  }

  read(path) { return this.#answer("read", { path: String(path) }); }
  list(prefix = "") { return this.#answer("list", { prefix: String(prefix) }); }
  stat(path) { return this.#answer("stat", { path: String(path) }); }
}

// The app's own search index (searchable.rs, "A fragment's search"), handed
// to its facet as `env.SEARCH` bound to one fragment by `props`, as FILES
// is: the app writes and asks its own fragment's index, never another's. It
// answers `{answer}`, or a refusal as a plain `{error}`: Workers RPC keeps
// none of a thrown Error's own fields, so platform.mjs's `searchIndex`
// throws it on the app's side, its fields whole.
export class Search extends WorkerEntrypoint {
  async #answer(op, body) {
    const { fragment } = this.ctx.props;
    const resp = await this.env.FRAGMENT.getByName(fragment).fetch(rs.InternalRoute.request(this.env, `call/search/${op}`, JSON.stringify(body)));
    const answer = await resp.json().catch(() => ({}));
    if (resp.ok) return { answer };
    return {
      error: {
        message: answer.message || `search.${op} answered ${resp.status}`,
        code: answer.error ?? null,
        status: resp.status,
        // a write's: whether it emptied an index of another embeddings space
        // first, the newer fence that refused it, and the wait a 429 asks for
        cleared: answer.cleared === true,
        fence: answer.fence ?? null,
        retry_after_ms: answer.retry_after_ms ?? null,
      },
    };
  }

  upsert(source) { return this.#answer("upsert", source); }
  // `{source, fence?}`, or a source's name alone
  delete(d) {
    const asked = typeof d === "string" ? { source: d } : { source: String(d?.source), fence: d?.fence };
    return this.#answer("delete", asked);
  }
  query(question) { return this.#answer("query", question); }
  status() { return this.#answer("status", {}); }
}

// One run of an operation (docs/api.md, Jobs and triggers). Each round asks
// the supervisor to advance the job to its next step, then performs that
// step through the supervisor, which keeps the step's answer before it
// replies (jobs.rs, `steps`): an advance names only how many steps are
// done, and a step tried again because its reply was lost is answered from
// what was kept, not performed again. Both calls are durable steps here, so
// a replay after a crash re-sends nothing that already happened. A step
// that fails for now (the supervisor answers 5xx) is retried with backoff;
// when its retries run out, the next advance carries its error, and the job
// sees it and may catch it.
export class Job extends WorkflowEntrypoint {
  async run(event, step) {
    const { fragment, incarnation, run, attempt } = event.payload;
    const delay = Math.max(1, Number(this.env.FRAGMENT_JOB_RETRY_DELAY_S) || 10);
    const retrying = { retries: { limit: 4, delay: `${delay} seconds`, backoff: "exponential" }, timeout: "5 minutes" };
    const post = async (path, body) => {
      const call = JSON.stringify({ incarnation, run, attempt, ...body });
      const resp = await this.env.FRAGMENT.getByName(fragment).fetch(rs.InternalRoute.request(this.env, `call/job/${path}`, call));
      const out = await resp.json().catch(() => ({}));
      if (!resp.ok) throw new Error(out.message || `call/job/${path} answered ${resp.status}`);
      return out;
    };
    // the step before this round, when it ran out of retries
    let failed = null;
    try {
      for (let i = 0; ; i++) {
        const next = await step.do(`advance ${i}`, retrying, () => post("advance", { count: i, failed }));
        if (!next.step) return next; // done, failed, or stop: the supervisor has recorded it
        failed = null;
        const { kind, args } = next.step;
        if (kind === "sleep") {
          // the supervisor kept the sleep's answer as it handed the step out
          await step.sleep(`sleep ${i}`, args.ms);
          continue;
        }
        try {
          // `try`: this step's try, from 1 (a text step's timing says it)
          // a video's call waits while its model makes it (ai.rs `step_video`):
          // its try outlasts a making (Workflows advise 30 minutes at most), so
          // it is not tried again meanwhile
          const tries = kind === "ai.video" ? { ...retrying, timeout: "15 minutes" } : retrying;
          const done = await step.do(`${kind} ${i}`, tries, (ctx) => post("effect", { index: i, kind, args, try: ctx?.attempt }));
          if (done.stop) return done;
        } catch (e) {
          failed = { index: i, kind, error: String((e && e.message) || e) };
        }
      }
    } catch (e) {
      const error = String((e && e.message) || e);
      await step.do("finish", retrying, () => post("finish", { error }));
      return { failed: error };
    }
  }
}

// The last `keep` bytes `stream` carries, read to its end, as text: at
// most those and one chunk are held at once.
async function tail(stream, keep) {
  let last = new Uint8Array(0);
  for await (const chunk of stream) {
    const both = new Uint8Array(last.length + chunk.length);
    both.set(last);
    both.set(chunk, last.length);
    last = both.slice(-keep);
  }
  return new TextDecoder().decode(last);
}

// A container step that answered nothing within its bound: its name
// (`step`) and the bound (`ms`) say which, in its message too, so the
// Rust side's failure (a save's, a start's, a stop's) says it.
class Overdue extends Error {
  constructor(step, ms) {
    super(`${step}: no answer within ${ms} ms`);
    this.name = "Overdue";
    this.step = step;
    this.ms = ms;
  }
}

// `run(signal)`, a step the Computer DO bounds (computer.rs names each
// bound and why): past `ms` its signal aborts and the step fails as
// `Overdue`, whether or not `run` heeds the abort. The Sandbox SDK's calls
// heed it and clean up after it (an upload aborted, the shim's stdin
// closed), but "never time out anything" themselves, and neither the
// runtime's snapshot nor its destroy takes a signal; a sick container
// answers none of them (r4, r6). A late answer, or a late failure, is no
// one's to hear.
async function bounded(step, ms, run) {
  if (!(ms > 0)) throw new Error(`${step}: a step is bounded`);
  const abort = new AbortController();
  const running = Promise.resolve().then(() => run(abort.signal));
  running.catch(() => {});
  let timer;
  const late = new Promise((_, reject) => {
    timer = setTimeout(() => {
      const overdue = new Overdue(step, ms);
      abort.abort(overdue);
      reject(overdue);
    }, ms);
  });
  try {
    return await Promise.race([running, late]);
  } finally {
    clearTimeout(timer);
  }
}

// A computer's container (docs/computers.md), for the Rust `ComputerCell`
// (computer.rs), which reaches it as `ctx.computerHost`. Each method is one
// runtime call and its plumbing. Every call that touches the container
// names the start it is for (its generation): a late call for an earlier
// start (a sleep that finishes after a wake started another) touches
// nothing.
class ContainerHost {
  #ctx;
  #env;
  #report;
  #generation = 0;
  #backups;
  // A message from a tab's page is its person's use (an open tab holds
  // the computer only by its use, IDLE_MS after the last: fragment_core's
  // lifecycle, `Event::TabUsed`). One is told at once, then at most one
  // every `#usedEvery` ms across its tabs (TAB_USED_EVERY_MS, which `port`
  // hands over), so a person at work costs a lifecycle write a minute, not
  // one a keystroke: a use within that window is told at its end
  // (`#usedLater`), or with its tab's close, whichever comes first.
  #usedEvery = 60_000;
  #usedAt = 0;
  #usedLater = null;
  // each open tab's close (`port`), for a sleep to close them (`closeTabs`)
  #tabs = new Set();
  // the sleeps begun (`closeTabs`), and the last one's why: a socket
  // admitted before one, its upgrade still on its way as it began, is
  // closed as the open ones were, never registered (`port`)
  #sleeps = 0;
  #sleptWhy = "asleep";
  // Until when the lever's `slow-snapshot` holds the container, as the
  // runtime's own snapshot does: a signal or a destroy lands once it ends.
  #heldUntil = 0;
  // The destroys begun of whatever runs or of the start that runs: a start
  // that sees one begin while it registers its intercepts starts nothing.
  #destroys = 0;

  constructor(ctx, env, report) {
    this.#ctx = ctx;
    this.#env = env;
    this.#report = report;
    this.#backups = new DirectoryBackup(ctx.container, ctx.exports.DirectoryBackupGateway, {
      binding: "BLOBS",
      prefix: `computers/${ctx.id}/backups/`,
    });
  }

  get #c() {
    return this.#ctx.container;
  }

  // Whether the container runs. A deployment with no computers names no
  // container class for this object (`xtask deploy` writes none, and a
  // local node may keep none), and the runtime then gives it no container
  // at all: nothing runs, so a wipe has nothing to destroy or to wait for
  // (FIN-661), as celld's node with no engine has one that never runs.
  get #runs() {
    return !!this.#c?.running;
  }

  running() {
    return this.#runs;
  }

  // The images the deployment declares (wrangler.jsonc `containers`).
  images() {
    return Object.keys(this.#c?.images || {});
  }

  // What the name `image` stands for in this deployment (its reference: a
  // redeploy that changes the image keeps the name and changes this).
  imageRef(image) {
    const ref = this.#c?.images?.[image];
    return ref === undefined ? null : typeof ref === "string" ? ref : JSON.stringify(ref);
  }

  // A new isolate finds the container of start `generation` (lesson 6):
  // when it runs, this isolate takes it, watching its exit again. Answers
  // whether it runs.
  adopt(generation) {
    if (!this.#runs) return false;
    this.#generation = generation;
    this.#c.monitor().then(
      () => this.#report("computer/exited", { generation }),
      (e) => this.#report("computer/exited", { generation, why: String((e && e.message) || e) }),
    );
    return true;
  }

  // Starts `image` (or a snapshot of it) for `computer` once every egress
  // intercept of its guest is registered (`startIntercepted`), so the
  // guest's first call never races them; then watches it (its exit, for
  // any reason, is reported as `computer/exited` for this generation) and
  // sets the runtime's idle stop. Its call to the runtime's start comes
  // after its registrations, never in the same turn as this call: one
  // overtaken meanwhile (a start after it, or a destroy begun: a sleep's
  // or a wipe's) starts nothing, answering false, and one overtaken by the
  // time its idle stop is set answers false too (that destroy ends it).
  async start(generation, image, snapshot, env, instance, computer, swapHosts, codestorage, idleMs, disk) {
    this.#generation = generation;
    const destroys = this.#destroys;
    // `FRAGMENT_COMPUTER_INTERNET=off` (the e2e's fleet): a run must not
    // depend on public DNS, which local workerd's sidecar asks first for
    // the intercepted hosts while the internet is on (devstack::Fleet)
    const opts = { env, enableInternet: this.#env.FRAGMENT_COMPUTER_INTERNET !== "off" };
    if (instance) opts.instance = instance;
    if (snapshot) opts.containerSnapshot = { id: snapshot };
    else if (this.#c.images && this.#c.images[image]) opts.image = this.#c.images[image];
    const overtaken = () => generation !== this.#generation || destroys !== this.#destroys;
    if (!(await startIntercepted(this.#c, opts, () => this.#intercept(computer, swapHosts, codestorage, disk), overtaken))) return false;
    this.#c.monitor().then(
      () => this.#report("computer/exited", { generation }),
      (e) => this.#report("computer/exited", { generation, why: String((e && e.message) || e) }),
    );
    try {
      await settled(() => this.#c.setInactivityTimeout(idleMs));
    } catch (e) {
      // overtaken meanwhile, its failure is no one's: the destroy ends it
      if (overtaken()) return false;
      throw e;
    }
    return !overtaken();
  }

  // Every egress intercept of `computer`'s guest, all at once (one entry
  // per host: `ComputerEgress` with the route as props; intercepts.mjs).
  // `swapHosts` are the connections' and operator keys' hosts, and on a
  // takeover its agents' Vault secrets' too. Not the backups' host: each
  // save and restore registers its own grant there and puts its refusal
  // back after (the Sandbox SDK's `DirectoryBackup`), and its
  // `intercept()`, a refusal registered again, is for beside a catch-all
  // intercept, which a computer has none of. One registered late (a
  // start's that was overtaken) would replace the grant of a restore under
  // way.
  // A start on a disk (`disk`: its prefix, name, epoch, volume and the
  // head it opens from) also routes fpac.fragment.internal, with the disk
  // in its props (fpac.mjs).
  #intercept(computer, swapHosts, codestorage, disk) {
    return intercept(this.#c, this.#egress(computer), swapHosts, codestorage, disk);
  }

  #egress(computer) {
    return (route, disk) => this.#ctx.exports.ComputerEgress({ props: disk ? { computer, route, disk } : { computer, route } });
  }

  // Its running start `generation`'s intercepts again (a new isolate's),
  // and the runtime's idle stop, the safety net under the Computer DO's
  // own sleep.
  async arm(generation, computer, idleMs, swapHosts, codestorage, disk) {
    if (generation !== this.#generation) return false;
    await this.#intercept(computer, swapHosts, codestorage, disk);
    await settled(() => this.#c.setInactivityTimeout(idleMs));
    return generation === this.#generation;
  }

  // `hosts` more for its running start `generation`'s swap (its agents'
  // Vault secrets'), those alone (`interceptSwap`). False: that start was
  // superseded, before or while they registered.
  async armSwap(generation, computer, hosts) {
    if (generation !== this.#generation) return false;
    await interceptSwap(this.#c, this.#egress(computer), hosts);
    return generation === this.#generation;
  }

  // Waits until the container takes an exec (it is up), for at most `ms`.
  async execReady(generation, ms) {
    return (await this.execUntil(generation, ["true"], ms)).ok;
  }

  // Runs `argv` until it exits 0 (polling every 100 ms), for at most `ms`:
  // `{ok, tries, last}`, whether it did, how many runs it took, and the
  // last run's answer (`{exitCode}`, or `{error}`). The guest's answer to a
  // hold (`held`) is read so.
  async execUntil(generation, argv, ms) {
    const t0 = Date.now();
    let tries = 0;
    let last = null;
    while (generation === this.#generation && Date.now() - t0 < ms) {
      tries++;
      try {
        const out = await this.exec(generation, argv, Math.max(1, ms - (Date.now() - t0)));
        last = { exitCode: out.exitCode, ms: Date.now() - t0 };
        if (out.exitCode === 0) return { ok: true, tries, last };
      } catch (e) {
        last = { error: String((e && e.message) || e).slice(0, 300), ms: Date.now() - t0 };
      }
      await new Promise((r) => setTimeout(r, 100));
    }
    return { ok: false, tries, last };
  }

  // Runs `argv` in the container: its exit code and the last `keep` bytes
  // of its output, stdout and stderr together (4 KiB unless named), all of
  // it held as it runs. Past `ms` (a wait for a container still starting
  // included) the call fails and the process is killed (SIGKILL), started
  // or not; one seen to exit is never signalled (an uncaught error in the
  // DO, the runtime's docs say).
  async exec(generation, argv, ms, keep = 4096) {
    if (generation !== this.#generation) throw new Error(`start ${generation} is not the running one`);
    if (!(ms > 0)) throw new Error(`${argv.join(" ")}: an exec is bounded`);
    const kill = new AbortController();
    let exited = false;
    const run = (async () => {
      const proc = await this.#c.exec(argv, { stderr: "combined", signal: kill.signal });
      proc.exitCode.then(() => (exited = true), () => (exited = true));
      const output = await tail(proc.stdout, keep);
      return { exitCode: await proc.exitCode, output };
    })();
    // a late failure, once the bound answered, is no one's to hear
    run.catch(() => {});
    let timer;
    const late = new Promise((_, reject) => {
      timer = setTimeout(() => {
        if (!exited) kill.abort();
        reject(new Error(`${argv.join(" ")}: no answer within ${ms} ms`));
      }, ms);
    });
    try {
      return await Promise.race([run, late]);
    } finally {
      clearTimeout(timer);
    }
  }

  // Saves `dir`, leaving out what `exclude` names (gitignore patterns,
  // relative to `dir`): its record, within `ms` (`Overdue` past it).
  async backup(generation, dir, exclude, ms) {
    if (generation !== this.#generation) throw new Error(`start ${generation} is not the running one`);
    const options = { dir, name: `generation ${generation}`, exclude: exclude || [] };
    return bounded(`the backup of ${dir}`, ms, (signal) => this.#backups.backup({ ...options, signal }));
  }

  // Restores `record` into its directory within `ms` (`Overdue` past it):
  // null once it is in place, `{unusable}` when the save itself will never
  // restore (its archive gone or altered: the SDK's BACKUP_NOT_FOUND and
  // BACKUP_INTEGRITY), or `{noRoom}` when it found no room on the disk (a
  // restore's SandboxFileError ENOSPC), which the Rust side tells from a
  // failure that may pass (it throws).
  async restore(generation, record, ms) {
    if (generation !== this.#generation) throw new Error(`start ${generation} is not the running one`);
    try {
      await bounded(`the restore of ${record.dir}`, ms, (signal) => this.#backups.restore(record, { signal }));
      return null;
    } catch (e) {
      if (SandboxBackupError.is(e) && (e.code === "BACKUP_NOT_FOUND" || e.code === "BACKUP_INTEGRITY")) {
        return { unusable: `${e.code}: ${e.detail || e.message}` };
      }
      if (SandboxFileError.is(e) && e.operation === "restore" && e.code === "ENOSPC") {
        return { noRoom: `${e.code}: ${e.detail || e.message}` };
      }
      throw e;
    }
  }

  // Deletes `record`'s archive within `ms` (`Overdue` past it).
  async forget(record, ms) {
    await bounded(`the delete of record ${record.id}`, ms, (signal) => this.#backups.delete(record, { signal }));
  }

  // A snapshot of the container within `ms` (`Overdue` past it): its id.
  // `slowMs` (the lever's `slow-snapshot`, where local workerd takes none):
  // one made up that takes that long, holding the container.
  async snapshot(generation, name, ms, slowMs) {
    if (generation !== this.#generation) throw new Error(`start ${generation} is not the running one`);
    if (slowMs > 0) {
      this.#heldUntil = Date.now() + slowMs;
      return bounded("the snapshot", ms, () => this.#unheld().then(() => `lever-${name}`));
    }
    const s = await bounded("the snapshot", ms, () => this.#c.snapshotContainer({ name }));
    return s.id;
  }

  // Sends signal `n` to the guest's PID 1 (from outside its PID namespace,
  // so SIGKILL lands as a crash's would). Answers whether it was sent.
  signal(generation, n) {
    if (generation !== this.#generation || !this.#runs || Date.now() < this.#heldUntil) return false;
    this.#c.signal(n);
    return true;
  }

  // Destroys the container if it is still this generation's (`0`: whatever
  // runs, an earlier start's or one an earlier isolate left), and waits for
  // it to be gone (lesson 6: never start before `running` is false), all
  // within `ms`: whether it is gone by then is the answer.
  async destroy(generation, reason, ms) {
    if (generation !== 0 && generation !== this.#generation) return false;
    if (!(ms > 0)) throw new Error("a destroy is bounded");
    this.#destroys++;
    const t0 = Date.now();
    if (this.#runs) await bounded("the destroy", ms, () => this.#unheld().then(() => this.#c.destroy(reason))).catch(() => {});
    while (this.#runs && Date.now() - t0 < ms) await new Promise((r) => setTimeout(r, 100));
    return !this.#runs;
  }

  // Once the lever's snapshot no longer holds the container (at once if none does).
  #unheld() {
    return new Promise((r) => setTimeout(r, Math.max(0, this.#heldUntil - Date.now())));
  }

  // Waits up to `ms` for the container to exit on its own (after a signal).
  async exited(ms) {
    const t0 = Date.now();
    while (this.#runs && Date.now() - t0 < ms) await new Promise((r) => setTimeout(r, 100));
    return !this.#runs;
  }

  // A request to one of the container's ports. A WebSocket is bridged
  // through this Durable Object (both ends accepted here), its opening,
  // closing and use reported as `computer/tab`: a message from the page's
  // end after its opening was told is its use (`#used`), and a tab whose
  // page says nothing holds the computer no longer than its opening does.
  // The container's end speaking is no use: a screen draws by itself.
  // Each end's listeners are added before it is accepted
  // and before anything is awaited: a message that arrives with no
  // listener is gone, and a container's socket may speak first (an RFB
  // server's version, the screen's control socket's holder), whose first
  // word the report's await used to lose. A close is passed on to the
  // other end with its code where that end may send it: 1005 (a close
  // that named none, as a bridge's plain close is) and 1006 (dropped) are
  // a receiver's to report, which workerd refuses to send, and passing one
  // on left the other end open: a screen whose desktop restarted kept its
  // page's stream, frozen (p5, 2026-10-05). Those go on as 1000. A message
  // that arrives once the bridge is closed is dropped, not sent: an end we
  // closed still delivers until its peer answers the close, and `send()`
  // throws on an end closed for sending. Only `close` closes one
  // (workerd's own answer to a peer's close comes in the step that
  // dispatches the close event `close` hears), so `closed` covers every
  // such send.
  async port(port, request, usedEvery) {
    if (usedEvery > 0) this.#usedEvery = usedEvery;
    const sleeps = this.#sleeps;
    const resp = await this.#c.getTcpPort(port).fetch(request);
    const upstream = resp.webSocket;
    if (!upstream) return resp;
    const [client, server] = Object.values(new WebSocketPair());
    if (this.#sleeps !== sleeps) {
      // a sleep began as its upgrade waited: closed with its why, as the
      // tabs open then were, so nothing its page sends reaches the
      // container that sleep saves (nothing awaited from here to its
      // registering below, so no sleep slips in after this)
      for (const ws of [upstream, server]) {
        try {
          ws.accept();
          ws.close(4000, this.#sleptWhy);
        } catch {}
      }
      return new Response(null, { status: 101, webSocket: client });
    }
    let closed = false;
    // its opening is told (before that, a word from its page is the opening's)
    let told = false;
    const close = (code, reason) => {
      if (closed) return;
      closed = true;
      this.#tabs.delete(close);
      // a use not told yet goes with the close, told first
      const used = this.#usedLater !== null;
      if (used) {
        clearTimeout(this.#usedLater);
        this.#usedLater = null;
        this.#usedAt = Date.now();
      }
      const sendable = code >= 1000 && code < 5000 && ![1004, 1005, 1006, 1015].includes(code);
      for (const ws of [upstream, server]) {
        try {
          ws.close(sendable ? code : 1000, sendable ? reason || "" : "");
        } catch {
          // a reason too long for a close frame: the code alone
          try {
            ws.close(sendable ? code : 1000);
          } catch {}
        }
      }
      this.#report("computer/tab", { open: false, used });
    };
    upstream.addEventListener("message", (e) => {
      if (!closed) server.send(e.data);
    });
    server.addEventListener("message", (e) => {
      if (closed) return;
      upstream.send(e.data);
      if (told) this.#used();
    });
    for (const ws of [upstream, server]) {
      ws.addEventListener("close", (e) => close(e.code, e.reason));
      ws.addEventListener("error", () => close(1011, "the other end failed"));
    }
    server.accept();
    upstream.accept();
    this.#tabs.add(close);
    await this.#report("computer/tab", { open: true });
    told = true;
    return new Response(null, { status: 101, webSocket: client });
  }

  // A sleep (or a restart's) begins, before its hold: each open port tab
  // closes with 4000 and `why` ("asleep" or "restarting"), so nothing its
  // page sends after reaches the container the sleep saves, and its page
  // says why and waits for its person (images/hermes/screen/index.html).
  // Answers how many it closed.
  closeTabs(why) {
    this.#sleeps += 1;
    this.#sleptWhy = why;
    const tabs = [...this.#tabs];
    for (const close of tabs) close(4000, why);
    return tabs.length;
  }

  // A port tab's page spoke: told at once, or at the end of the window of
  // the last told (once however many came), or with its tab's close. A
  // report that fails is logged; the next use tells it.
  #used() {
    if (this.#usedLater !== null) return;
    const wait = this.#usedAt + this.#usedEvery - Date.now();
    if (wait <= 0) return this.#tellUsed();
    this.#usedLater = setTimeout(() => {
      this.#usedLater = null;
      if (this.#tabs.size > 0) this.#tellUsed();
    }, wait);
  }

  #tellUsed() {
    this.#usedAt = Date.now();
    this.#report("computer/tab", { open: true, used: true }).catch((e) => console.error(JSON.stringify({ tab: "used", error: String((e && e.message) || e) })));
  }
}

export class Computer extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    const report = (path, body) => this.rs.fetch(rs.InternalRoute.request(env, path, JSON.stringify(body)));
    Object.defineProperty(ctx, "computerHost", { value: new ContainerHost(ctx, env, report) });
    this.rs = new rs.ComputerCell(ctx, env);
  }
  fetch(request) { return this.rs.fetch(request); }
  alarm(info) { return this.rs.alarm(info); }
  webSocketMessage(ws, message) { return this.rs.webSocketMessage(ws, message); }
  webSocketClose(ws, code, reason, clean) { return this.rs.webSocketClose(ws, code, reason, clean); }
  webSocketError(ws, error) { return this.rs.webSocketError(ws, error); }
}

// Every intercepted request a computer's guest makes (docs/computers.md):
// `props.route` is the host it asked for, and `props.computer` the
// computer, both set by the Computer DO, never by the guest. Rust answers
// it (`ComputerEgress.handle`); an XFTP relay it holds a place for is piped
// here (`relay`).
export class ComputerEgress extends WorkerEntrypoint {
  async fetch(request) {
    const { computer, route } = this.ctx.props;
    // a disk's own route (fpac.mjs): its bytes R2's, streamed through, its
    // head the Computer DO's (Rust carries the post to it)
    if (route === "fpac") {
      const E = rs.ComputerEgress;
      const rules = { nameEpoch: (n) => E.fpac_name_epoch(n) ?? null, objectBytesMax: E.fpac_object_bytes_max(), headLineMax: E.fpac_head_line_max() };
      return fpacRoute(
        request,
        this.env.BLOBS,
        this.ctx.props.disk,
        rules,
        (epoch, line) => E.disk_head(this.env, computer, epoch, line),
        (epoch, head, why, blank, unmountable) => E.disk_unusable(this.env, computer, epoch, head, why, blank, unmountable),
      );
    }
    // a relay only where one was asked for: its route, a GET that upgrades
    // (Rust strips the relay's headers from every other answer besides)
    const asked = route === "api" && request.method === "GET" && new URL(request.url).pathname === "/api/computer/relay" && (request.headers.get("upgrade") ?? "").toLowerCase() === "websocket";
    // a disk start's API names its life (`disk.epoch`): its guest's reads say whose
    const answer = await rs.ComputerEgress.handle(request, this.env, this.ctx, computer, route, this.ctx.props.disk?.epoch ?? null);
    const lease = asked ? answer.headers.get("x-fragment-relay") : null;
    if (!lease) return answer;
    const h = answer.headers;
    const target = { host: h.get("x-fragment-relay-host"), port: Number(h.get("x-fragment-relay-port")), bytesMax: Number(h.get("x-fragment-relay-bytes")), msMax: Number(h.get("x-fragment-relay-ms")) };
    return relay(this.ctx, target, { computer }, (bytes) => rs.ComputerEgress.relay_closed(this.env, computer, lease, bytes));
  }
}

