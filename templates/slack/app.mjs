// The Slack connector (docs/api.md, "An agent's doors": a hosted door). A
// chat of its owner's that is also their agent's Slack app, one app an
// agent (FIN-508, Paul's decisions of 2026-11-16):
//
// - Its owner, in person, makes the app from a link its setup page fills
//   in, then gives its bot token and signing secret from the page
//   (`__secrets`, never this code's); `connect` checks the token
//   (`auth.test`) and keeps its workspace and bot. The inbox takes a
//   delivery only signed by that secret (fragment.json `inbox`), and
//   answers Slack's URL check (`handshake`).
// - `pair` makes a single-use code (ten minutes); the account that sends
//   it to the bot in a DM is proposed, and its owner confirms it on the
//   page (`confirm`). From then on that account's words are its owner's.
// - `intake` (the inbox's trigger alone) takes a message that mentions
//   the bot in a channel, or a DM, and publishes it here to the agent:
//   its owner's as theirs (`job.door`'s `relayed`), anyone else's as
//   someone heard (`heard`); said in a channel, or in someone else's DM,
//   it names its room (`room`) and, in a channel, what was said there just
//   before (`context`). docs/chat-records.md, "A message heard in a room".
//   The platform stamps what it publishes the door's only while its owner
//   has bound this chat as that agent's `slack` door.
// - `mirror` (each reply the agent posts here, and each question it asks
//   its owner on `work`, as applib/question.mjs words it) sends it to
//   Slack: in the thread of the message its turn answers, or, with none
//   known, in its room or its owner's DM. A question goes where its
//   owner's answer counts: the room its turn was asked in, or their DM.
//
// Anyone in a room the bot was added to, and anyone who DMs it, may ask
// (decision 5), on its owner's computer and credits: a room takes at most
// ROOM_TURNS_PER_HOUR of others' messages an hour. Past them it says once
// that the agent is resting, and takes none until the hour turns.
//
// No one but its owner edits this chat: its secrets, its code, its pages
// and the operations here that are an editor's are theirs alone, the
// platform's rule for a connector (docs/api.md, "An agent's doors").
//
// Slack carries it all as plain text to and from its servers, and every
// member of a room reads what the agent says there. Text only, for now.
import { DurableObject } from "cloudflare:workers";
import { asked } from "./applib/question.mjs";

const SLACK = "https://slack.com";
const PAIRING_MS = 10 * 60_000;
// A pairing code: what its owner sends the bot, alone, in a DM.
const PAIR = /^pair\s+([0-9a-f]{16})$/i;
// What one message's text is cut to: in, 4,000 characters (at most 16 KB
// of UTF-8, with its context's CONTEXT_MAX_BYTES well within a record's
// RECORD_BODY_MAX_BYTES, 64 KiB, and the bridge's MESSAGE_TEXT_MAX_BYTES,
// 32 KiB); out, Slack's own limit on a message's `markdown_text`.
const TEXT_IN_MAX = 4000;
const TEXT_OUT_MAX = 12_000;
// A reply goes in at most this many messages; the rest is the chat's.
const PARTS_MAX = 4;
// What it remembers, each bounded, the oldest let go first.
const SEEN_MAX = 2000;
const NAMES_MAX = 2000;
const TURNS_MAX = 1000;
// A name looked up again after a day (a person renames themselves).
const NAME_FRESH_MS = 24 * 60 * 60_000;
// Names one message looks up from Slack, at most: its speaker, its room and
// the context's speakers and mentions. Past them a name goes as "someone".
const LOOKUPS_MAX = 12;
// Lines of a room's context, and characters of each (the bridge reads the
// newest 20, each cut to 640 bytes: docs/chat-records.md).
const CONTEXT_LINES_MAX = 20;
const CONTEXT_TEXT_MAX = 600;
// A room's context in all, at most (the oldest lines let go first).
const CONTEXT_MAX_BYTES = 16 * 1024;
// A thread's replies read for its context, at most: Slack answers a
// thread's oldest first, so a thread longer than this up to its mention
// is told the newest 20 of its first 100 (at most 100 of Slack's
// messages, a few KiB each at most, within a fetch's answer,
// FETCH_RESPONSE_MAX_BYTES, 1 MiB).
const THREAD_READ_MAX = 100;
// Names, one line, as the bridge cuts them.
const NAME_MAX = 60;
// Connect attempts whose answer is kept, the latest last.
const ANSWERED_MAX = 16;
// A room's cap on others' messages an hour (decision 5), and how long a
// reply waits for its turn's thread to be known before it goes to its room.
const ROOM_TURNS_PER_HOUR = 30;
const HOUR_MS = 60 * 60_000;
const THREAD_WAIT_MS = 2000;

const hex = (bytes) => [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
// A room's id or a person's, as the record's rules read them
// (docs/chat-records.md: `^[A-Za-z0-9._-]{1,64}$`, a speaker's to 128).
const token = (s, max) => typeof s === "string" && s.length >= 1 && s.length <= max && /^[A-Za-z0-9._-]+$/.test(s);

/// Someone's words from Slack, as one line: no control or format
/// characters, at most NAME_MAX characters.
export function oneLine(s) {
  if (typeof s !== "string") return "";
  const flat = s.replace(/[\p{C}\p{Z}]+/gu, " ").trim();
  return [...flat].slice(0, NAME_MAX).join("").trim();
}

const cut = (s, max) => [...s].slice(0, max).join("");

/// Slack's message text, as a person reads it: its mentions by name (as
/// `names` has them: `U…` people, `C…` rooms), its links' labels and its
/// three escapes undone (Slack escapes `&`, `<` and `>`).
export function readable(text, names = new Map()) {
  if (typeof text !== "string") return "";
  const said = text.replace(/<([^<>]*)>/g, (_, inner) => {
    const [ref, label] = inner.split("|", 2);
    if (ref.startsWith("@")) return `@${names.get(ref.slice(1)) ?? label ?? "someone"}`;
    if (ref.startsWith("#")) return `#${label ?? names.get(ref.slice(1)) ?? "a channel"}`;
    if (ref.startsWith("!")) return label ?? `@${ref.slice(1).split("^")[0]}`;
    return label && label !== ref ? `${label} (${ref})` : ref;
  });
  return said.replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&amp;/g, "&");
}

/// The people and rooms `text` mentions (`<@U…>`, `<#C…>`), each once.
export function mentioned(text) {
  if (typeof text !== "string") return [];
  return [...new Set([...text.matchAll(/<([@#])([A-Z0-9]{2,32})(?:\|[^<>]*)?>/g)].map((m) => m[2]))];
}

/// What an event is to the connector: a message it takes, or null. A
/// mention of the bot in a channel (`app_mention`), or a message in a DM
/// with it (`message`, `channel_type: im`); never a bot's (its own
/// included), an edit, a deletion or any other subtype, nor a DM's
/// mention, which Slack sends as both.
export function taken(event, bot) {
  const e = event;
  if (!e || typeof e !== "object" || e.bot_id || e.subtype !== undefined) return null;
  if (!token(e.user, 128) || e.user === bot.user || !token(e.channel, 64) || typeof e.ts !== "string") return null;
  const dm = e.channel_type === "im" || e.channel.startsWith("D");
  if (e.type === "app_mention" && dm) return null;
  if (e.type !== "app_mention" && !(e.type === "message" && dm)) return null;
  const thread = typeof e.thread_ts === "string" && e.thread_ts !== e.ts ? e.thread_ts : null;
  return { user: e.user, channel: e.channel, ts: e.ts, thread, dm, text: typeof e.text === "string" ? e.text : "" };
}

/// A context line's or a record's time, from Slack's `ts` (Unix seconds).
const atOf = (ts) => {
  const s = Number.parseFloat(ts);
  return Number.isFinite(s) ? Math.round(s * 1000) : undefined;
};

const sha24 = async (s) => hex(new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(s)))).slice(0, 24);

/// A turn's id, as docs/chat-records.md defines it: the agent's, of the
/// record that started it.
export const turnOf = (agent, fragment, seq) => sha24(`${agent}|${fragment}/chat/${seq}`);

/// The turn a restart's cut makes of `turn` (docs/chat-records.md,
/// "Turns"; the bridge continues a turn once): its replies go where its do.
const CONTINUATIONS = 1;
export async function continuations(turn) {
  const out = [];
  for (let t = turn; out.length < CONTINUATIONS; ) out.push((t = await sha24(`continue\0${t}`)));
  return out;
}

/// A Web API call, the bot's token filled at the platform's egress point,
/// toward Slack's host alone (its secret's): a read as GET with its query,
/// a write as POST with its JSON body.
async function slack(job, api, method, { query, body } = {}) {
  const headers = { authorization: "Bearer {{SLACK_BOT_TOKEN}}" };
  const qs = query ? `?${new URLSearchParams(query)}` : "";
  const init = body ? { method: "POST", headers: { ...headers, "content-type": "application/json; charset=utf-8" }, body: JSON.stringify(body) } : { headers };
  const resp = await job.fetch(`${api}/api/${method}${qs}`, init);
  let answer = {};
  try {
    answer = resp.json();
  } catch {}
  return { ok: resp.ok && answer.ok === true, answer, error: typeof answer.error === "string" ? answer.error : `Slack answered ${resp.status}` };
}

/// What a message wants of Slack but can do without (👀, a room's context,
/// a name): asked, and its failure, however it fails, none of the
/// message's.
async function maybe(fn, otherwise) {
  try {
    return await fn();
  } catch {
    return otherwise;
  }
}

async function post(job, api, channel, thread, text) {
  const sent = await slack(job, api, "chat.postMessage", { body: { channel, markdown_text: text, ...(thread ? { thread_ts: thread } : {}) } });
  if (!sent.ok) throw new Error(`chat.postMessage: ${sent.error}`);
}

/// What a run says to Slack when it may: a connector whose door is shut
/// sends nothing, and that is no failure of the run.
async function tell(job, api, channel, thread, text) {
  try {
    await post(job, api, channel, thread, text);
  } catch {}
}

/// The names `ids` (people, `U…`/`W…`; rooms, `C…`/`G…`) as Slack has
/// them: those remembered, then at most LOOKUPS_MAX more asked of Slack,
/// remembered in their turn. One asked that fails is no one's name.
async function names(job, api, ids) {
  const known = new Map(Object.entries((await job.call("named", { ids })).names));
  const asked = ids.filter((id) => !known.has(id)).slice(0, LOOKUPS_MAX);
  const learned = {};
  for (const id of asked) {
    const room = /^[CG]/.test(id);
    const r = await maybe(() => (room ? slack(job, api, "conversations.info", { query: { channel: id } }) : slack(job, api, "users.info", { query: { user: id } })), { ok: false, answer: {} });
    const p = r.answer.user?.profile;
    const name = oneLine(room ? r.answer.channel?.name : p?.display_name || p?.real_name || r.answer.user?.real_name || r.answer.user?.name);
    if (r.ok && name) learned[id] = name;
  }
  if (Object.keys(learned).length) await job.call("remember", { names: learned });
  return new Map([...known, ...Object.entries(learned)]);
}

/// What was said in a channel just before `m`, oldest first: in a thread,
/// its earlier replies; otherwise its channel's latest messages. Its
/// bot's own words and every message that mentions the bot are left out:
/// the agent's conversation in that room has them already. Its lines'
/// speakers named as `named` has them.
async function contextOf(job, api, m, bot) {
  const r = m.thread
    ? await slack(job, api, "conversations.replies", { query: { channel: m.channel, ts: m.thread, latest: m.ts, inclusive: "false", limit: String(THREAD_READ_MAX) } })
    : await slack(job, api, "conversations.history", { query: { channel: m.channel, latest: m.ts, inclusive: "false", limit: String(CONTEXT_LINES_MAX) } });
  if (!r.ok || !Array.isArray(r.answer.messages)) return [];
  const before = r.answer.messages
    .filter((x) => typeof x?.ts === "string" && atOf(x.ts) < atOf(m.ts) && typeof x.text === "string" && x.text.trim() && !x.bot_id && x.user !== bot.user && !x.text.includes(`<@${bot.user}>`) && x.subtype === undefined && token(x.user, 128))
    .sort((a, b) => atOf(a.ts) - atOf(b.ts));
  return before.slice(-CONTEXT_LINES_MAX);
}

export class App extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    const sql = ctx.storage.sql;
    sql.exec("CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT NOT NULL)");
    sql.exec("CREATE TABLE IF NOT EXISTS seen (key TEXT PRIMARY KEY, at INTEGER NOT NULL)");
    sql.exec("CREATE TABLE IF NOT EXISTS names (id TEXT PRIMARY KEY, name TEXT NOT NULL, at INTEGER NOT NULL)");
    sql.exec("CREATE TABLE IF NOT EXISTS turns (turn TEXT PRIMARY KEY, channel TEXT NOT NULL, thread TEXT, ts TEXT NOT NULL, at INTEGER NOT NULL)");
    sql.exec("CREATE TABLE IF NOT EXISTS rooms (room TEXT PRIMARY KEY, hour INTEGER NOT NULL, turns INTEGER NOT NULL, rested INTEGER NOT NULL)");
  }

  get(k) {
    const row = this.ctx.storage.sql.exec("SELECT v FROM kv WHERE k = ?", k).toArray()[0];
    return row ? JSON.parse(row.v) : null;
  }

  put(k, v) {
    if (v === null) this.ctx.storage.sql.exec("DELETE FROM kv WHERE k = ?", k);
    else this.ctx.storage.sql.exec("INSERT INTO kv (k, v) VALUES (?, ?) ON CONFLICT (k) DO UPDATE SET v = excluded.v", k, JSON.stringify(v));
  }

  state() {
    const bot = this.get("bot");
    const paired = this.get("paired");
    const p = this.get("pairing");
    const pairing = p && Date.now() <= p.expires ? { code: p.code, expires: p.expires, candidate: p.candidate ? { user: p.candidate.user, name: p.candidate.name } : null } : null;
    // `verified`: a delivery signed by its secret came (Slack's URL check,
    // or any event since), so Slack sends here
    return { bot: bot ? { name: bot.name, team: bot.teamName } : null, error: this.get("error"), verified: this.get("verified") === true, paired: paired ? { name: paired.name } : null, pairing, answered: this.get("answered") ?? [] };
  }

  async connect({ api = SLACK, attempt } = {}, job) {
    const base = new URL(api).origin;
    const said = typeof attempt === "string" ? { attempt } : {};
    const me = await slack(job, base, "auth.test", { body: {} });
    const a = me.answer;
    if (!me.ok || !token(a.team_id, 64) || !token(a.user_id, 128)) {
      await job.call("known", { error: `Slack didn't take that bot token (${me.error}).`, ...said });
      return { connected: false };
    }
    await job.call("known", { bot: { team: a.team_id, teamName: oneLine(a.team), user: a.user_id, name: oneLine(a.user) || "your agent" }, api: base, ...said });
    return { connected: true, team: a.team_id };
  }

  known({ bot, api, error, attempt = null }, call) {
    if (call.agent) throw new Error("its owner connects it, in person");
    if (typeof attempt === "string") this.put("answered", [...(this.get("answered") ?? []), attempt.slice(0, 64)].slice(-ANSWERED_MAX));
    if (error) {
      this.put("error", String(error).slice(0, 300));
      return { ok: false };
    }
    this.put("error", null);
    // another workspace, or another bot: what it knew is another's (a URL
    // check before its first connect is this app's own)
    const was = this.get("bot");
    if (was && (was.team !== bot.team || was.user !== bot.user)) {
      this.put("paired", null);
      this.put("pairing", null);
      this.put("verified", null);
      for (const t of ["seen", "names", "turns", "rooms"]) this.ctx.storage.sql.exec(`DELETE FROM ${t}`);
    }
    this.put("bot", { team: bot.team, teamName: oneLine(bot.teamName), user: bot.user, name: oneLine(bot.name) });
    this.put("api", api);
    return { ok: true };
  }

  pair(_, call) {
    if (call.agent) throw new Error("its owner pairs it, in person");
    if (!this.get("bot")) throw new Error("connect the app first");
    const code = hex(crypto.getRandomValues(new Uint8Array(8)));
    const expires = Date.now() + PAIRING_MS;
    this.put("pairing", { code, expires, candidate: null });
    return { code, expires };
  }

  // the inbox's run, with the code a DM carried: the account that sends it,
  // while it holds, is proposed, never bound. Its inbox's records are its
  // members' to read, so one who saw the code could propose an account of
  // their choosing: its owner's own DM comes too, and two accounts sending
  // one code propose no one.
  propose({ code, user, dm, name }) {
    const p = this.get("pairing");
    if (!p || p.code !== String(code).toLowerCase() || Date.now() > p.expires) return { proposed: false };
    if (p.candidate) {
      if (p.candidate.user === user && p.candidate.dm === dm) return { proposed: true };
      this.put("pairing", null);
      return { proposed: false, conflict: true };
    }
    this.put("pairing", { ...p, candidate: { user, dm, name: oneLine(name) || "someone" } });
    return { proposed: true };
  }

  confirm({ user }, call) {
    if (call.agent) throw new Error("its owner confirms it, in person");
    const p = this.get("pairing");
    if (!p?.candidate || p.candidate.user !== user || Date.now() > p.expires) throw new Error("no account is waiting to be confirmed: pair again");
    this.put("paired", { ...p.candidate, team: this.get("bot")?.team });
    this.put("pairing", null);
    return { paired: { name: p.candidate.name } };
  }

  unpair(_, call) {
    if (call.agent) throw new Error("its owner unpairs it, in person");
    this.put("paired", null);
    this.put("pairing", null);
    return { ok: true };
  }

  // a message taken once (`<channel>:<ts>`): Slack sends one again until it
  // hears a 2xx, and a DM's mention as two events
  seen({ key }) {
    const fresh = this.ctx.storage.sql.exec("INSERT INTO seen (key, at) VALUES (?, ?) ON CONFLICT DO NOTHING RETURNING key", String(key).slice(0, 200), Date.now()).toArray().length === 1;
    this.ctx.storage.sql.exec("DELETE FROM seen WHERE key NOT IN (SELECT key FROM seen ORDER BY at DESC, key LIMIT ?)", SEEN_MAX);
    return { fresh };
  }

  verified() {
    this.put("verified", true);
    return { ok: true };
  }

  named({ ids }) {
    const fresh = Date.now() - NAME_FRESH_MS;
    const out = {};
    for (const id of (Array.isArray(ids) ? ids : []).slice(0, 64)) {
      const row = this.ctx.storage.sql.exec("SELECT name FROM names WHERE id = ? AND at >= ?", String(id), fresh).toArray()[0];
      if (row) out[id] = row.name;
    }
    return { names: out };
  }

  remember({ names }) {
    for (const [id, name] of Object.entries(names ?? {}).slice(0, 64)) {
      if (token(id, 128) && oneLine(name)) this.ctx.storage.sql.exec("INSERT INTO names (id, name, at) VALUES (?, ?, ?) ON CONFLICT (id) DO UPDATE SET name = excluded.name, at = excluded.at", id, oneLine(name), Date.now());
    }
    this.ctx.storage.sql.exec("DELETE FROM names WHERE id NOT IN (SELECT id FROM names ORDER BY at DESC, id LIMIT ?)", NAMES_MAX);
    return { ok: true };
  }

  // others' messages in `room` this hour: taken while under the cap; past
  // it, the first one refused says so (`rest`)
  room({ room }) {
    const hour = Math.floor(Date.now() / HOUR_MS);
    this.ctx.storage.sql.exec("DELETE FROM rooms WHERE hour < ?", hour);
    const row = this.ctx.storage.sql.exec("SELECT turns, rested FROM rooms WHERE room = ?", String(room)).toArray()[0] ?? { turns: 0, rested: 0 };
    if (row.turns < ROOM_TURNS_PER_HOUR) {
      this.ctx.storage.sql.exec("INSERT INTO rooms (room, hour, turns, rested) VALUES (?, ?, ?, 0) ON CONFLICT (room) DO UPDATE SET turns = excluded.turns", String(room), hour, row.turns + 1);
      return { ok: true };
    }
    this.ctx.storage.sql.exec("UPDATE rooms SET rested = 1 WHERE room = ?", String(room));
    return { ok: false, rest: row.rested === 0 };
  }

  // where a turn's replies go: the thread of the message that started it
  took({ turn, channel, thread, ts }) {
    this.ctx.storage.sql.exec("INSERT INTO turns (turn, channel, thread, ts, at) VALUES (?, ?, ?, ?, ?) ON CONFLICT (turn) DO NOTHING", String(turn), String(channel), thread ?? null, String(ts), Date.now());
    this.ctx.storage.sql.exec("DELETE FROM turns WHERE turn NOT IN (SELECT turn FROM turns ORDER BY at DESC, turn LIMIT ?)", TURNS_MAX);
    return { ok: true };
  }

  thread({ turn }) {
    const row = this.ctx.storage.sql.exec("SELECT channel, thread, ts FROM turns WHERE turn = ?", String(turn)).toArray()[0];
    return { to: row ? { channel: row.channel, thread: row.thread ?? null, ts: row.ts } : null };
  }

  // where its runs talk to Slack, as whom, and with whom
  route() {
    const paired = this.get("paired");
    return { api: this.get("api") ?? SLACK, bot: this.get("bot"), paired: paired ? { user: paired.user, dm: paired.dm } : null, verified: this.get("verified") === true };
  }

  // Slack's URL check (its `challenge`, said back); anything else, the
  // platform's own answer
  handshake({ record } = {}) {
    const p = record?.body?.payload;
    return p?.type === "url_verification" && typeof p.challenge === "string" ? { body: p.challenge } : null;
  }

  async intake({ record } = {}, job) {
    // its inbox's trigger alone: a member who calls it relays nothing
    if (job.via !== "channel") return { ignored: "only its inbox's trigger" };
    const p = record?.body?.payload;
    const { api, bot, paired, verified } = await job.call("route", {});
    // signed by its secret (the inbox checked it): Slack sends here
    if (!verified) await job.call("verified", {});
    if (p?.type !== "event_callback") return { ignored: "not an event" };
    if (!bot) return { ignored: "not connected" };
    if (p.team_id !== bot.team) return { ignored: "another workspace's" };
    const m = taken(p.event, bot);
    if (!m) return { ignored: "not a message to the agent" };
    if (!(await job.call("seen", { key: `${m.channel}:${m.ts}` })).fresh) return { ignored: "taken already" };
    // a reply in the thread of a channel's message; in a DM, in its thread
    // if it has one
    const thread = m.dm ? m.thread : (m.thread ?? m.ts);
    // pasted into Slack, the code keeps the page's formatting (inline
    // code, `pair …`): its marks around the line are not the code's
    const pairing = m.dm ? PAIR.exec(m.text.trim().replace(/^[`*_~\s]+|[`*_~\s]+$/g, "")) : null;
    if (pairing) {
      const who = await names(job, api, [m.user]);
      const { proposed, conflict } = await job.call("propose", { code: pairing[1], user: m.user, dm: m.channel, name: who.get(m.user) ?? "someone" });
      if (proposed) await post(job, api, m.channel, null, "Almost there: confirm it's you on the page where you started.");
      if (conflict) await post(job, api, m.channel, null, "Two accounts sent that code, so it was cancelled. Start again from the page.");
      return { proposed, conflict: conflict === true };
    }
    // its binding, as the platform tells it: no guess at which agent
    if (!job.door) {
      if (m.dm) await tell(job, api, m.channel, null, "This app isn't linked to an agent.");
      return { ignored: "not linked" };
    }
    const owner = paired?.user === m.user;
    if (!owner) {
      const room = await job.call("room", { room: m.channel });
      if (!room.ok) {
        if (room.rest) await tell(job, api, m.channel, thread, `${bot.name} has answered a lot here this hour, and is resting. Try again later.`);
        return { ignored: "resting" };
      }
    }
    // 👀 on it while the agent works (its reply takes it off); a room that
    // refuses it is no failure
    await maybe(() => slack(job, api, "reactions.add", { body: { channel: m.channel, timestamp: m.ts, name: "eyes" } }));
    // its owner's DM is the chat's own conversation; any other place, a room
    const inRoom = !(owner && m.dm);
    const lines = inRoom && !m.dm ? await maybe(() => contextOf(job, api, m, bot), []) : [];
    const ids = [m.user, ...(m.dm ? [] : [m.channel]), ...mentioned(m.text), ...lines.flatMap((x) => [x.user, ...mentioned(x.text)])];
    const known = await names(job, api, [...new Set(ids)].filter((id) => id !== bot.user));
    known.set(bot.user, bot.name);
    // a mention of the bot that leads a channel's message only routes it
    // here (Slack sends nothing else of a channel): its words are the rest
    const said = m.dm ? m.text : m.text.replace(new RegExp(`^\\s*<@${bot.user}(?:\\|[^>]*)?>[\\s,:]*`), "");
    // a mention alone is still something said to it
    const text = cut(readable(said.trim() ? said : m.text, known).trim(), TEXT_IN_MAX);
    if (!text) return { ignored: "no text" };
    const body = { text, to: [job.door.to] };
    if (owner) body.relayed = job.door.relayed;
    else body.heard = { via: "slack", from: m.user, name: known.get(m.user) ?? "someone" };
    if (inRoom) {
      body.room = { id: m.channel, name: m.dm ? "DM" : `#${known.get(m.channel) ?? m.channel}`, ...(thread ? { thread } : {}) };
      const context = lines.map((x) => ({ name: known.get(x.user) ?? "someone", text: cut(readable(x.text, known).trim(), CONTEXT_TEXT_MAX), at: atOf(x.ts) }));
      while (context.length && new TextEncoder().encode(JSON.stringify(context)).length > CONTEXT_MAX_BYTES) context.shift();
      if (context.length) body.context = context;
    }
    const { seq } = await job.publish("chat", body, "message");
    // where its turn's replies go, known as soon as its record is
    const turn = await turnOf(job.door.relayed.agent, job.fragment, seq);
    for (const t of [turn, ...(await continuations(turn))]) await job.call("took", { turn: t, channel: m.channel, thread, ts: m.ts });
    return { relayed: owner, heard: !owner, room: inRoom ? m.channel : null, turn };
  }

  async mirror({ record } = {}, job) {
    // its triggers' runs alone: each reply an agent posts here, each
    // question it asks
    if (job.via !== "channel") return { sent: 0, why: "only its triggers send" };
    // unlinked, or its door shut: the agent's words stay in the chat; and
    // only the bound agent's replies reach Slack, never another's here
    if (!job.door) return { sent: 0, why: "not linked" };
    if (record?.principal !== job.door.to) return { sent: 0, why: "not its bound agent's" };
    const b = record?.body;
    let text;
    if (record.channel === "work") {
      // a question, only one that asks its owner (whom the door is theirs)
      text = asked(b);
      if (!text) return { sent: 0, why: "not a question" };
      if (b.asks !== job.door.relayed?.for) return { sent: 0, why: "it asks someone else" };
      if (typeof b.turn !== "string") return { sent: 0, why: "not a turn's" };
    } else {
      if (!b || typeof b.turn !== "string" || b.interim === true || (b.kind !== undefined && b.kind !== "message")) return { sent: 0, why: "not a reply" };
      const said = typeof b.text === "string" ? b.text.trim() : "";
      // its files stay in the chat (Slack gets words alone, for now): said so
      const files = Array.isArray(b.attachments) ? b.attachments.length : 0;
      text = [said, files ? "(It sent a file, which stays in its owner's chat.)" : ""].filter(Boolean).join("\n\n");
      if (!text) return { sent: 0, why: "nothing said" };
    }
    const { api, paired, bot } = await job.call("route", {});
    // the thread its turn answers; a reply in a room whose thread is not
    // known yet waits a moment for it (its intake keeps it as it
    // publishes), then goes to its room; any other, its owner's DM
    const room = token(b.room, 64) ? b.room : null;
    let to = (await job.call("thread", { turn: b.turn })).to;
    if (!to && room) {
      await job.sleep(THREAD_WAIT_MS);
      to = (await job.call("thread", { turn: b.turn })).to ?? { channel: room, thread: null, ts: null };
    }
    if (!to && paired) to = { channel: paired.dm, thread: null, ts: null };
    if (!to) return { sent: 0, why: "nowhere to send it" };
    // a channel sends the bot only what mentions it: an answer there must
    const words = Array.isArray(b.options) && b.options.some((o) => o?.words === true) && !b.options.some((o) => o?.computer === true);
    if (record.channel === "work" && words && !to.channel.startsWith("D")) text = `${text}\n\n(Answer here by mentioning @${bot?.name ?? "me"}.)`;
    const chars = [...text];
    const size = TEXT_OUT_MAX - 64;
    let parts = 0;
    // bounded: PARTS_MAX messages; what is past them stays in the chat
    for (let at = 0; at < chars.length && parts < PARTS_MAX; at += size) {
      const last = parts === PARTS_MAX - 1 && at + size < chars.length;
      await post(job, api, to.channel, to.thread, chars.slice(at, at + size).join("") + (last ? "\n\n(The rest is in its owner's chat.)" : ""));
      parts += 1;
    }
    // its answer (a question is none): 👀 off
    if (to.ts && record.channel !== "work") await maybe(() => slack(job, api, "reactions.remove", { body: { channel: to.channel, timestamp: to.ts, name: "eyes" } }));
    return { sent: parts };
  }
}
