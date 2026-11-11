// The Telegram connector (docs/api.md, "An agent's doors": a hosted door).
// A chat that is also its owner's direct messages with their own bot:
//
// - Its owner, in person, gives the bot's token and a webhook secret from
//   the page (`__secrets`, never this code's), then `connect` checks the
//   token (getMe) and points the bot's webhook at this chat's inbox, which
//   takes a delivery only with that secret in Telegram's header
//   (fragment.json `inbox`).
// - `pair` makes a single-use link (`/start <nonce>`, ten minutes); the
//   Telegram account that answers it is proposed, and its owner confirms
//   it on the page (`confirm`): its numeric user and chat ids are bound,
//   its name only shown.
// - `intake` (the inbox's trigger alone) takes a direct message from that
//   account and publishes it here as its owner's words to their agent
//   (`job.door`'s `relayed`, `to` that agent), which the platform takes
//   only while its owner has bound this chat as that agent's `telegram`
//   door.
// - `mirror` (each reply an agent posts here) sends it to that account,
//   while the door is open.
// - `typing`, started by `intake` as it relays a message, shows that
//   account the bot typing until `mirror` sends the agent's answer, or
//   for at most TYPING_ROUNDS_MAX rounds.
//
// No one but its owner edits this chat: its secrets, its code, its pages
// and the operations here that are an editor's are theirs alone, the
// platform's rule for a connector (docs/api.md, "An agent's doors").
//
// Telegram carries it all as plain text to and from its servers: it is not
// end-to-end encrypted. Direct messages only, text only, for now.
import { DurableObject } from "cloudflare:workers";

const TELEGRAM = "https://api.telegram.org";
const PAIRING_MS = 10 * 60_000;
// Telegram's limit on one message's text.
const TEXT_MAX = 4096;
// A reply goes in at most this many messages; the rest is the chat's.
const PARTS_MAX = 4;
// Updates remembered, so a delivery Telegram sends again is taken once.
const SEEN_MAX = 2000;
// Connect attempts whose answer is kept, the latest last, so a page sees its
// own however many answered between its polls.
const ANSWERED_MAX = 16;
// Telegram shows a chat action for 5 s at most, or until the bot's next
// message, so `typing` says it again this often while the agent works.
const TYPING_EVERY_MS = 4000;
// Its hard cap: about five minutes of "typing", three steps a round (a
// look, the action, a sleep) within a job's JOB_STEPS_MAX of 256. A turn
// that ends with no answer here (an error, a card, silence) stops it.
const TYPING_ROUNDS_MAX = 60;

const hex = (bytes) => [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
const nameOf = (from) => [from?.first_name, from?.last_name].filter((s) => typeof s === "string" && s).join(" ").slice(0, 64) || (from?.username ? `@${from.username}` : "someone");

/// A Bot API call, the bot's token filled at the platform's egress point,
/// toward Telegram's host alone (its secret's).
async function telegram(job, api, method, { query = "", body } = {}) {
  const url = `${api}/bot{{TELEGRAM_BOT_TOKEN}}/${method}${query ? `?${query}` : ""}`;
  const init = body ? { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) } : {};
  const resp = await job.fetch(url, init);
  let answer = {};
  try {
    answer = resp.json();
  } catch {}
  return { ok: resp.ok && answer.ok === true, result: answer.result, description: typeof answer.description === "string" ? answer.description : `Telegram answered ${resp.status}` };
}

async function send(job, api, chat, text) {
  const sent = await telegram(job, api, "sendMessage", { body: { chat_id: chat, text } });
  if (!sent.ok) throw new Error(`sendMessage: ${sent.description}`);
}

/// What a run says to Telegram when it may: a bound connector whose door
/// is shut (its deployment turned it off) sends nothing, and that is no
/// failure of the run.
async function tell(job, api, chat, text) {
  try {
    await send(job, api, chat, text);
  } catch {}
}

export class App extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT NOT NULL)");
    ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS seen (update_id INTEGER PRIMARY KEY)");
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
    const pairing = p && Date.now() <= p.expires ? { link: p.link, expires: p.expires, candidate: p.candidate ? { user: p.candidate.user, name: p.candidate.name } : null } : null;
    // `answered`: the attempts `connect` answered lately (each a page's own
    // word), so a page that asked sees its own answer come, the same bot's
    // token again included, whatever other connects answered meanwhile
    // `held`: its owner took the bot their Vault held for this chat
    // (`hold`), and it has not connected since
    return { bot: bot ? { username: bot.username } : null, error: this.get("error"), paired: paired ? { name: paired.name } : null, pairing, answered: this.get("answered") ?? [], held: this.get("held") === true };
  }

  async connect({ platform, api = TELEGRAM, attempt } = {}, job) {
    const base = new URL(api).origin;
    // the attempt its page named, if it named one, with each answer
    const said = typeof attempt === "string" ? { attempt } : {};
    const inbox = `${new URL(platform).origin}/api/f/${job.fragment}/inbox`;
    const me = await telegram(job, base, "getMe");
    if (!me.ok || typeof me.result?.username !== "string") {
      await job.call("known", { error: `Telegram didn't take that bot token (${me.description}).`, ...said });
      return { connected: false };
    }
    const query = new URLSearchParams({ url: inbox, allowed_updates: JSON.stringify(["message"]) }).toString();
    const hook = await telegram(job, base, "setWebhook", { query: `${query}&secret_token={{TELEGRAM_WEBHOOK_SECRET}}` });
    if (!hook.ok) {
      await job.call("known", { error: `Telegram didn't take its webhook (${hook.description}).`, ...said });
      return { connected: false };
    }
    await job.call("known", { bot: { id: me.result.id, username: me.result.username }, api: base, ...said });
    return { connected: true, bot: me.result.username };
  }

  known({ bot, api, error, attempt = null }, call) {
    if (call.agent) throw new Error("its owner connects it, in person");
    if (typeof attempt === "string") this.put("answered", [...(this.get("answered") ?? []), attempt.slice(0, 64)].slice(-ANSWERED_MAX));
    if (error) {
      this.put("error", String(error).slice(0, 300));
      return { ok: false };
    }
    this.put("error", null);
    // another bot: the account paired with the one before is no one here,
    // and its update ids are another bot's
    if (this.get("bot")?.id !== bot.id) {
      this.put("paired", null);
      this.put("pairing", null);
      this.ctx.storage.sql.exec("DELETE FROM seen");
    }
    this.put("bot", { id: bot.id, username: String(bot.username).slice(0, 64) });
    this.put("api", api);
    this.put("held", null);
    return { ok: true };
  }

  // its owner takes the bot their Vault holds for this chat (setup.js),
  // said before the token moves here: a page that lost the move's answer,
  // or one opened again, offers that bot again until it connects
  hold(_, call) {
    if (call.agent) throw new Error("its owner sets it up, in person");
    this.put("held", true);
    return { ok: true };
  }

  pair(_, call) {
    if (call.agent) throw new Error("its owner pairs it, in person");
    const bot = this.get("bot");
    if (!bot) throw new Error("connect the bot first");
    const nonce = hex(crypto.getRandomValues(new Uint8Array(16)));
    const expires = Date.now() + PAIRING_MS;
    const link = `https://t.me/${encodeURIComponent(bot.username)}?start=${nonce}`;
    this.put("pairing", { nonce, expires, link, candidate: null });
    return { link, expires };
  }

  // the inbox's run, with the nonce a `/start` carried: the account that
  // answers the pairing link, while it holds, is proposed, never bound. Its
  // inbox's records are its members' to read, so a member who saw the
  // nonce could call this with an account of its own choosing: the owner's
  // own `/start` always comes too, so two accounts answering one link
  // propose no one, and its owner pairs again.
  propose({ nonce, user, chat, name }) {
    const p = this.get("pairing");
    if (!p || p.nonce !== nonce || Date.now() > p.expires) return { proposed: false };
    if (p.candidate) {
      if (p.candidate.user === user && p.candidate.chat === chat) return { proposed: true };
      this.put("pairing", null);
      return { proposed: false, conflict: true };
    }
    this.put("pairing", { ...p, candidate: { user, chat, name: String(name).slice(0, 64) } });
    return { proposed: true };
  }

  confirm({ user }, call) {
    if (call.agent) throw new Error("its owner confirms it, in person");
    const p = this.get("pairing");
    if (!p?.candidate || p.candidate.user !== user || Date.now() > p.expires) throw new Error("no account is waiting to be confirmed: pair again");
    this.put("paired", { ...p.candidate, bot: this.get("bot")?.id });
    this.put("pairing", null);
    return { paired: { name: p.candidate.name } };
  }

  unpair(_, call) {
    if (call.agent) throw new Error("its owner unpairs it, in person");
    this.put("paired", null);
    this.put("pairing", null);
    return { ok: true };
  }

  // an update taken once: Telegram sends one again until it hears a 2xx
  seen({ update }) {
    const fresh = this.ctx.storage.sql.exec("INSERT INTO seen (update_id) VALUES (?) ON CONFLICT DO NOTHING RETURNING update_id", update).toArray().length === 1;
    this.ctx.storage.sql.exec("DELETE FROM seen WHERE update_id NOT IN (SELECT update_id FROM seen ORDER BY update_id DESC LIMIT ?)", SEEN_MAX);
    return { fresh };
  }

  // a relayed message the agent owes an answer: `typing` shows it working
  // while this one is the latest owed and unanswered (a newer message
  // takes over; the next answer `mirror` sends ends it)
  owe() {
    const id = (this.get("typing")?.id ?? 0) + 1;
    this.put("typing", { id, answered: false });
    return { id };
  }

  owed({ id }) {
    const t = this.get("typing");
    return { typing: t?.id === id && !t.answered };
  }

  answered() {
    const t = this.get("typing");
    if (t) this.put("typing", { ...t, answered: true });
    return { ok: true };
  }

  // where its runs talk to Telegram, and with whom
  route() {
    const paired = this.get("paired");
    return { api: this.get("api") ?? TELEGRAM, paired: paired ? { user: paired.user, chat: paired.chat } : null };
  }

  async intake({ record } = {}, job) {
    // its inbox's trigger alone: a member who calls it relays nothing
    if (job.via !== "channel") return { ignored: "only its inbox's trigger" };
    const update = record?.body?.payload;
    const m = update?.message;
    if (!Number.isInteger(update?.update_id) || m?.chat?.type !== "private" || !Number.isInteger(m?.from?.id) || m.from.is_bot) return { ignored: "not a direct message" };
    if (!(await job.call("seen", { update: update.update_id })).fresh) return { ignored: "taken already" };
    const { api, paired } = await job.call("route", {});
    const text = typeof m.text === "string" ? m.text.trim() : "";
    const start = /^\/start(?:@\w+)?\s+([0-9a-f]{32})$/.exec(text);
    if (start) {
      const { proposed, conflict } = await job.call("propose", { nonce: start[1], user: m.from.id, chat: m.chat.id, name: nameOf(m.from) });
      if (proposed) await send(job, api, m.chat.id, "Almost there: confirm it's you on the page where you started.");
      if (conflict) await send(job, api, m.chat.id, "Two accounts answered that link, so it was cancelled. Start again from the page.");
      return { proposed, conflict: conflict === true };
    }
    // anyone but the account its owner confirmed is no one here
    if (!paired || paired.user !== m.from.id || paired.chat !== m.chat.id) return { ignored: "not its owner's" };
    if (!text) {
      await send(job, api, m.chat.id, "Only text reaches your agent from Telegram for now.");
      return { ignored: "not text" };
    }
    // its binding, as the platform tells it: no guess at which agent
    if (!job.door) {
      await tell(job, api, m.chat.id, "This chat isn't linked to your agent.");
      return { ignored: "not linked" };
    }
    // owed before it is published, so an answer however quick ends it
    const { id } = await job.call("owe", {});
    await job.publish("chat", { text: [...text].slice(0, TEXT_MAX).join(""), to: [job.door.to], relayed: job.door.relayed }, "message");
    const typing = await job.call("typing", { chat: m.chat.id, id });
    return { relayed: true, typing: typing.run };
  }

  // "typing…" in the paired chat while the agent owes message `id` its
  // answer: a look, the action, a sleep, a round; bounded by
  // TYPING_ROUNDS_MAX, and by Telegram refusing it (a shut door fetches
  // nothing). A look then the action, so an answer sent leaves at most one
  // action already on its way.
  async typing({ chat, id } = {}, job) {
    if (job.via !== "job") return { rounds: 0, why: "only intake starts it" };
    const { api } = await job.call("route", {});
    for (let rounds = 0; rounds < TYPING_ROUNDS_MAX; rounds += 1) {
      if (!(await job.call("owed", { id })).typing) return { rounds, why: "answered" };
      let said;
      try {
        said = await telegram(job, api, "sendChatAction", { body: { chat_id: chat, action: "typing" } });
      } catch {
        return { rounds, why: "not sent" };
      }
      if (!said.ok) return { rounds, why: "not sent" };
      await job.sleep(TYPING_EVERY_MS);
    }
    return { rounds: TYPING_ROUNDS_MAX, why: "cap" };
  }

  async mirror({ record } = {}, job) {
    // its trigger's runs alone: each reply an agent posts here
    if (job.via !== "channel") return { sent: 0, why: "only the chat's trigger sends" };
    // unlinked, or its door shut: the agent's words stay in the chat; and
    // only the bound agent's replies reach Telegram, never another's here
    if (!job.door) return { sent: 0, why: "not linked" };
    if (record?.principal !== job.door.to) return { sent: 0, why: "not its bound agent's" };
    const b = record?.body;
    if (!b || typeof b.turn !== "string" || b.interim === true || (b.kind !== undefined && b.kind !== "message")) return { sent: 0, why: "not a reply" };
    const said = typeof b.text === "string" ? b.text.trim() : "";
    const files = Array.isArray(b.attachments) ? b.attachments.length : 0;
    const text = said || (files ? "(Sent a file: open the chat to see it.)" : "");
    if (!text) return { sent: 0, why: "nothing said" };
    const { api, paired } = await job.call("route", {});
    if (!paired) return { sent: 0, why: "no account paired" };
    // its answer: "typing…" stops, before the answer goes
    await job.call("answered", {});
    const chars = [...text];
    let parts = 0;
    // bounded: PARTS_MAX messages; what is past them stays in the chat
    for (let at = 0; at < chars.length && parts < PARTS_MAX; at += TEXT_MAX - 64) {
      const last = parts === PARTS_MAX - 1 && at + TEXT_MAX - 64 < chars.length;
      await send(job, api, paired.chat, chars.slice(at, at + TEXT_MAX - 64).join("") + (last ? "\n\n(The rest is in the chat.)" : ""));
      parts += 1;
    }
    return { sent: parts };
  }
}
