// The SimpleX connector (docs/api.md, "An agent's doors": a hosted door,
// as Telegram's is). A chat that is also its owner's conversation with
// their SimpleX app, through the platform's SimpleX door: a small
// container of its own running SimpleX's core (cell/door.mjs,
// images/simplex-door), which this fragment's jobs alone reach, at
// `door.fragment.internal` (the platform sends each to the door of this
// fragment's life, never out to the internet).
//
// - Its owner, in person, sets the inbox's secret (`SIMPLEX_DOOR_SECRET`,
//   toward door.fragment.internal), then `connect` starts the door and
//   gives it that secret, filled at the platform's egress (never this
//   code's); the door's deliveries carry it to this chat's inbox, as a
//   webhook's do (fragment.json `inbox`).
// - `link` asks the door for a one-time invitation for its owner's
//   SimpleX app, kept for its owner alone (`state`, an owner's query):
//   never in a run's output, which its agent, a viewer here, reads. It
//   then watches for their app to answer it, for as long as a person takes
//   to scan its code, and keeps whom it linked; `check` looks once, later.
//   `unlink` ends that link. A step that fails says why in `state`.
//   Its owner's setup page (site/setup.html, opened from Settings) runs
//   them: the inbox's secret and `connect` first, then `link`.
// - `intake` (the inbox's trigger alone) takes each message the door
//   delivers, once by its SimpleX id (`seen`), and publishes it here as
//   its owner's words to their agent (`job.door`'s `relayed`, `to` that
//   agent), which the platform takes only while its owner has bound this
//   chat as that agent's `simplex` door. The agent's computer wakes for it
//   as for any message: nothing here needs it awake.
// - `mirror` (each reply its bound agent posts here, and each question it
//   asks its owner on `work`, as applib/question.mjs words it) sends it
//   through the door, once by its record, asking again until the door's
//   runner says it is sent (or its link ended, or there is none). Their
//   next message from SimpleX answers a question that takes words.
//
// No one but its owner edits this chat: its secrets, its code and the
// operations here that are an owner's are theirs alone, the platform's
// rule for a connector (docs/api.md, "An agent's doors"). Text only, for
// now: another kind of message is answered so.
import { DurableObject } from "cloudflare:workers";
import { asked } from "./applib/question.mjs";

const DOOR = "https://door.fragment.internal";
// A message's text, as the platform's chat takes one (a person's post).
const TEXT_MAX = 32 * 1024;
// Messages remembered, so one the door delivers again is taken once.
const SEEN_MAX = 2000;
// A reply asked again while the door's runner holds it unsent: after
// 1 s, 2 s, 4 s … at most REPLY_WAIT_MAX_MS apart, REPLY_ASKS_MAX times
// (two steps a round, within a job's JOB_STEPS_MAX of 256).
const REPLY_ASKS_MAX = 30;
const REPLY_WAIT_MAX_MS = 30_000;
// The door's invitation, looked for this many times a second apart once
// asked (two steps a round).
const INVITE_LOOKS_MAX = 30;
// Its owner's app answering that invitation, looked for after each of
// these waits: about 8 minutes, as long as a person takes to scan a code
// (two steps a look: with the invitation's, 212 of a job's 256).
const LINK_WAITS_MS = [...Array(30).fill(2_000), ...Array(24).fill(5_000), ...Array(20).fill(15_000)];
// A QR code's side, in modules, at most (version 40's).
const QR_SIZE_MAX = 177;
// The steps a failure names, the one its setup page tries again.
const STEPS = ["connect", "link", "unlink"];

// An ask of its door, as its job's fetch: its status, and what it said.
async function door(job, ask, body = {}, headers = {}) {
  const resp = await job.fetch(`${DOOR}/${ask}`, { method: "POST", headers: { "content-type": "application/json", ...headers }, body: JSON.stringify(body) });
  let said = {};
  try {
    said = resp.json() ?? {};
  } catch {}
  return { status: resp.status, said };
}

// The door's runner's own state (images/simplex-door `View`), or none.
async function runnerOf(job) {
  const { said } = await door(job, "state");
  return said.runner ?? {};
}

// A failure, for its owner's setup page to say: its words, and the
// door's code for it (`no_doors`: this deployment runs none).
const answered = (status, said) => `the door answered ${status}: ${String(said.message ?? "").slice(0, 200)}`;
const codeOf = (said) => (typeof said.error === "string" ? said.error.slice(0, 40) : null);

// An invitation's QR code as the door drew it, kept only whole: a square
// of modules, each a `1` or a `0` (images/simplex-door `Qr`).
function qrOf(qr) {
  const size = qr?.size;
  if (!Number.isInteger(size) || size < 21 || size > QR_SIZE_MAX || typeof qr.dark !== "string" || qr.dark.length !== size * size || /[^01]/.test(qr.dark)) return null;
  return { size, dark: qr.dark };
}

// A reply through the door, by `key`, asked until its runner says it is
// sent (200), its link ended first (410), or it has no link (409).
async function reply(job, key, text) {
  for (let ask = 0; ask < REPLY_ASKS_MAX; ask += 1) {
    const { status, said } = await door(job, "reply", { key, text });
    if (status === 200) return { sent: 1 };
    if (status === 410) return { sent: 0, why: "its SimpleX link ended" };
    if (status === 409) return { sent: 0, why: "not linked" };
    if (status !== 202) return { sent: 0, why: `the door answered ${status}: ${String(said.message ?? "").slice(0, 200)}` };
    await job.sleep(Math.min(1000 * 2 ** ask, REPLY_WAIT_MAX_MS));
  }
  return { sent: 0, why: "the door still holds it unsent" };
}

// A word to its owner's app about a message not carried: tried, never
// failing what it is about (a door shut sends nothing).
async function tell(job, key, text) {
  try {
    await reply(job, key, text);
  } catch {}
}

export class App extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT NOT NULL)");
    ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS seen (id TEXT PRIMARY KEY, n INTEGER NOT NULL)");
  }

  get(k) {
    const row = this.ctx.storage.sql.exec("SELECT v FROM kv WHERE k = ?", k).toArray()[0];
    return row ? JSON.parse(row.v) : null;
  }

  put(k, v) {
    if (v === null) this.ctx.storage.sql.exec("DELETE FROM kv WHERE k = ?", k);
    else this.ctx.storage.sql.exec("INSERT INTO kv (k, v) VALUES (?, ?) ON CONFLICT (k) DO UPDATE SET v = excluded.v", k, JSON.stringify(v));
  }

  // its owner's alone: whether its door was started, the invitation
  // waiting for their SimpleX app, whom it linked, and why its last step
  // failed
  state() {
    return { connected: this.get("door") !== null, invitation: this.get("invitation"), linked: this.get("linked"), why: this.get("why") };
  }

  async connect({ name } = {}, job) {
    const { status, said } = await door(job, "connect", { name }, { "x-door-secret": "{{SIMPLEX_DOOR_SECRET}}" });
    if (status !== 200) {
      const error = answered(status, said);
      await job.call("failed", { why: error, code: codeOf(said), step: "connect" });
      return { connected: false, error };
    }
    // its door, its life's: a delivery from any other is no one's here
    await job.call("connected", { door: said.door });
    return { connected: true, started: said.started };
  }

  // its owner's (its connect job's), never its agent's, a viewer here
  connected({ door }) {
    this.put("door", String(door));
    this.put("why", null);
    return { ok: true };
  }

  // its owner's (its jobs'): why a step failed, and which (`connect`,
  // `link` or `unlink`, the one a setup page tries again), until another
  // is asked (`at`: which failure it is, one said again included)
  failed({ n, why, code, step }) {
    if (n !== undefined && this.get("asked") !== n) return { kept: false };
    this.put("why", { text: String(why).slice(0, 300), code: typeof code === "string" ? code.slice(0, 40) : null, at: Date.now(), step: STEPS.includes(step) ? step : null });
    return { kept: true };
  }

  async link(_, job) {
    // numbered: an unlink after it means its invitation is kept by no one
    const { n } = await job.call("asking", {});
    const asked = await door(job, "link", { n });
    if (asked.status !== 202) {
      const why = answered(asked.status, asked.said);
      await job.call("failed", { n, why, code: codeOf(asked.said), step: "link" });
      return { invited: false, why };
    }
    let invitation = null;
    for (let look = 0; look < INVITE_LOOKS_MAX && !invitation; look += 1) {
      await job.sleep(1000);
      const runner = await runnerOf(job);
      // linked already: its door asks no invitation of a link it holds
      if (runner.phase === "linked") return this.#linked(job, n, runner);
      if (typeof runner.link === "string" && runner.link) invitation = runner;
    }
    if (!invitation) {
      await job.call("failed", { n, why: "no invitation yet", step: "link" });
      return { invited: false, why: "no invitation yet" };
    }
    const short = typeof invitation.short === "string" && invitation.short ? { short: invitation.short } : {};
    const { kept } = await job.call("invited", { n, link: invitation.link, ...short, qr: invitation.qr ?? null });
    if (!kept) return { invited: false, why: "unlinked since" };
    // its owner's app answering it; another link or an unlink asked since
    // (a phase or an invitation not this one's) ends the watch
    for (const ms of LINK_WAITS_MS) {
      await job.sleep(ms);
      const runner = await runnerOf(job);
      if (runner.phase === "linked") return this.#linked(job, n, runner);
      if (runner.phase !== "inviting" || runner.link !== invitation.link) return { invited: true, linked: false };
    }
    return { invited: true, linked: false };
  }

  // whom its door says it is linked to, kept as link `n`'s
  async #linked(job, n, runner) {
    const { kept } = await job.call("linked", { n, name: typeof runner.name === "string" ? runner.name : "" });
    return kept ? { linked: true } : { linked: false, why: "unlinked since" };
  }

  // a look at whom its door is linked to now, for an invitation answered
  // after its link stopped watching: kept while no link or unlink is
  // asked meanwhile
  async check(_, job) {
    // the link it looks for, read as a step (a job reads nothing else)
    const { n } = await job.call("current", {});
    const runner = await runnerOf(job);
    if (runner.phase !== "linked") return { linked: false };
    return this.#linked(job, n, runner);
  }

  // its owner's (its link and unlink jobs'), never its agent's, a viewer
  // here: each link or unlink numbered, its invitation, its link and why
  // a step failed let go; an unlink's number is one no link is kept under
  asking({ unlink } = {}) {
    const n = (this.get("asked") ?? 0) + 1;
    this.put("asked", n);
    this.put("unlinking", unlink === true ? n : null);
    this.put("invitation", null);
    this.put("linked", null);
    this.put("why", null);
    return { n };
  }

  // the invitation its link `n` found, kept only while no link or unlink
  // was asked after it, with its QR code where the door drew it whole
  invited({ n, link, short, qr }) {
    if (this.get("asked") !== n) return { kept: false };
    this.put("invitation", { link: String(link).slice(0, 2000), short: short ? String(short).slice(0, 2000) : null, qr: qrOf(qr), at: Date.now() });
    return { kept: true };
  }

  // the newest link or unlink asked (`check`'s)
  current() {
    return { n: this.get("asked") ?? 0 };
  }

  // whom link `n`'s invitation linked (their SimpleX name), kept only
  // while no link or unlink was asked after it, and never under an
  // unlink's number (a `check` that read it while the door had yet to
  // unlink); its invitation is used
  linked({ n, name }) {
    if ((this.get("asked") ?? 0) !== n || this.get("unlinking") === n) return { kept: false };
    this.put("linked", { name: String(name ?? "").slice(0, 200), at: Date.now() });
    this.put("invitation", null);
    return { kept: true };
  }

  async unlink(_, job) {
    const { n } = await job.call("asking", { unlink: true });
    const { status, said } = await door(job, "unlink", { n });
    if (status !== 202) await job.call("failed", { n, why: answered(status, said), code: codeOf(said), step: "unlink" });
    return { unlinked: status === 202 };
  }

  // a SimpleX message taken once, from its own door alone (a door an
  // earlier life of its name made is another's): the door may deliver one
  // again (an answer from the inbox lost; its runner restored from an
  // older save)
  seen({ id, door }) {
    if (door !== this.get("door")) return { fresh: false, other: true };
    const fresh = this.ctx.storage.sql.exec("INSERT INTO seen (id, n) VALUES (?, (SELECT coalesce(max(n), 0) + 1 FROM seen)) ON CONFLICT DO NOTHING RETURNING n", id).toArray().length === 1;
    this.ctx.storage.sql.exec("DELETE FROM seen WHERE n < (SELECT max(n) FROM seen) - ?", SEEN_MAX);
    return { fresh };
  }

  async intake({ record } = {}, job) {
    // its inbox's trigger alone: a member who calls it relays nothing
    if (job.via !== "channel") return { ignored: "only its inbox's trigger" };
    const m = record?.body?.payload;
    if (typeof m?.id !== "string" || !m.id || m.id.length > 64 || typeof m.text !== "string" || typeof m.door !== "string") return { ignored: "not a message" };
    const seen = await job.call("seen", { id: m.id, door: m.door });
    if (seen.other) return { ignored: "not its door's" };
    if (!seen.fresh) return { ignored: "taken already" };
    // its binding, as the platform tells it: no guess at which agent
    if (!job.door) {
      await tell(job, `unlinked:${m.id}`, "This chat isn't linked to your agent.");
      return { ignored: "not linked" };
    }
    const text = m.text.trim();
    if (m.kind !== "text" || !text) {
      await tell(job, `only-text:${m.id}`, "Only text reaches your agent from SimpleX for now.");
      return { ignored: "not text" };
    }
    await job.publish("chat", { text: [...text].slice(0, TEXT_MAX).join(""), to: [job.door.to], relayed: job.door.relayed }, "message");
    return { relayed: true };
  }

  async mirror({ record } = {}, job) {
    // its triggers' runs alone: each reply an agent posts here, each
    // question it asks
    if (job.via !== "channel") return { sent: 0, why: "only its triggers send" };
    // unlinked, or its door shut: the agent's words stay in the chat; and
    // only the bound agent's replies reach SimpleX, never another's here
    if (!job.door) return { sent: 0, why: "not linked" };
    if (record?.principal !== job.door.to) return { sent: 0, why: "not its bound agent's" };
    const b = record?.body;
    // a question, only one that asks its owner (whom the door is theirs)
    if (record.channel === "work") {
      if (b?.asks !== job.door.relayed?.for) return { sent: 0, why: "it asks someone else" };
      const question = asked(b);
      return question ? reply(job, `work:${record.seq}`, question) : { sent: 0, why: "not a question" };
    }
    if (!b || typeof b.turn !== "string" || b.interim === true || (b.kind !== undefined && b.kind !== "message")) return { sent: 0, why: "not a reply" };
    const said = typeof b.text === "string" ? b.text.trim() : "";
    const files = Array.isArray(b.attachments) ? b.attachments.length : 0;
    const text = said || (files ? "(Sent a file: open the chat to see it.)" : "");
    if (!text) return { sent: 0, why: "nothing said" };
    return reply(job, `chat:${record.seq}`, text);
  }
}
