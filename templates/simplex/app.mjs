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
//   never in a run's output, which its agent, a viewer here, reads.
//   `unlink` ends that link.
// - `intake` (the inbox's trigger alone) takes each message the door
//   delivers, once by its SimpleX id (`seen`), and publishes it here as
//   its owner's words to their agent (`job.door`'s `relayed`, `to` that
//   agent), which the platform takes only while its owner has bound this
//   chat as that agent's `simplex` door. The agent's computer wakes for it
//   as for any message: nothing here needs it awake.
// - `mirror` (each reply its bound agent posts here) sends it through the
//   door, once by its record, asking again until the door's runner says
//   it is sent (or its link ended, or there is none).
//
// No one but its owner edits this chat: its secrets, its code and the
// operations here that are an owner's are theirs alone, the platform's
// rule for a connector (docs/api.md, "An agent's doors"). Text only, for
// now: another kind of message is answered so.
import { DurableObject } from "cloudflare:workers";

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

// An ask of its door, as its job's fetch: its status, and what it said.
async function door(job, ask, body = {}, headers = {}) {
  const resp = await job.fetch(`${DOOR}/${ask}`, { method: "POST", headers: { "content-type": "application/json", ...headers }, body: JSON.stringify(body) });
  let said = {};
  try {
    said = resp.json() ?? {};
  } catch {}
  return { status: resp.status, said };
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

  // its owner's alone: the invitation waiting for their SimpleX app
  state() {
    return { invitation: this.get("invitation") };
  }

  async connect({ name } = {}, job) {
    const { status, said } = await door(job, "connect", { name }, { "x-door-secret": "{{SIMPLEX_DOOR_SECRET}}" });
    if (status !== 200) return { connected: false, error: `the door answered ${status}: ${String(said.message ?? "").slice(0, 200)}` };
    return { connected: true, started: said.started };
  }

  async link(_, job) {
    // numbered: an unlink after it means its invitation is kept by no one
    const { n } = await job.call("asking", {});
    const asked = await door(job, "link");
    if (asked.status !== 202) return { invited: false, why: `the door answered ${asked.status}: ${String(asked.said.message ?? "").slice(0, 200)}` };
    for (let look = 0; look < INVITE_LOOKS_MAX; look += 1) {
      await job.sleep(1000);
      const { said } = await door(job, "state");
      const runner = said.runner ?? {};
      if (typeof runner.link === "string" && runner.link) {
        const { kept } = await job.call("invited", { n, link: runner.link, ...(typeof runner.short === "string" && runner.short ? { short: runner.short } : {}) });
        return kept ? { invited: true } : { invited: false, why: "unlinked since" };
      }
    }
    return { invited: false, why: "no invitation yet" };
  }

  // its owner's (its link and unlink jobs'), never its agent's, a viewer
  // here: each link or unlink numbered, its invitation let go
  asking() {
    const n = (this.get("asked") ?? 0) + 1;
    this.put("asked", n);
    this.put("invitation", null);
    return { n };
  }

  // the invitation its link `n` found, kept only while no link or unlink
  // was asked after it
  invited({ n, link, short }) {
    if (this.get("asked") !== n) return { kept: false };
    this.put("invitation", { link: String(link).slice(0, 2000), short: short ? String(short).slice(0, 2000) : null, at: Date.now() });
    return { kept: true };
  }

  async unlink(_, job) {
    await job.call("asking", {});
    const { status } = await door(job, "unlink");
    return { unlinked: status === 202 };
  }

  // a SimpleX message taken once: the door may deliver one again (an
  // answer from the inbox lost; its runner restored from an older save)
  seen({ id }) {
    const fresh = this.ctx.storage.sql.exec("INSERT INTO seen (id, n) VALUES (?, (SELECT coalesce(max(n), 0) + 1 FROM seen)) ON CONFLICT DO NOTHING RETURNING n", id).toArray().length === 1;
    this.ctx.storage.sql.exec("DELETE FROM seen WHERE n < (SELECT max(n) FROM seen) - ?", SEEN_MAX);
    return { fresh };
  }

  async intake({ record } = {}, job) {
    // its inbox's trigger alone: a member who calls it relays nothing
    if (job.via !== "channel") return { ignored: "only its inbox's trigger" };
    const m = record?.body?.payload;
    if (typeof m?.id !== "string" || !m.id || m.id.length > 64 || typeof m.text !== "string") return { ignored: "not a message" };
    if (!(await job.call("seen", { id: m.id })).fresh) return { ignored: "taken already" };
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
    // its trigger's runs alone: each reply an agent posts here
    if (job.via !== "channel") return { sent: 0, why: "only the chat's trigger sends" };
    // unlinked, or its door shut: the agent's words stay in the chat; and
    // only the bound agent's replies reach SimpleX, never another's here
    if (!job.door) return { sent: 0, why: "not linked" };
    if (record?.principal !== job.door.to) return { sent: 0, why: "not its bound agent's" };
    const b = record?.body;
    if (!b || typeof b.turn !== "string" || b.interim === true || (b.kind !== undefined && b.kind !== "message")) return { sent: 0, why: "not a reply" };
    const said = typeof b.text === "string" ? b.text.trim() : "";
    const files = Array.isArray(b.attachments) ? b.attachments.length : 0;
    const text = said || (files ? "(Sent a file: open the chat to see it.)" : "");
    if (!text) return { sent: 0, why: "nothing said" };
    return reply(job, `chat:${record.seq}`, text);
  }
}
