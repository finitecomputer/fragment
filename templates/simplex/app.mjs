// SPIKE (branch spike/simplex-door, never merged): the SimpleX connector, a
// hosted door like Telegram's (templates/telegram), whose "Telegram" is the
// platform's SimpleX door: a small container of its own running SimpleX's
// core (cell/door.mjs, images/simplex-door).
//
// - Its owner, in person, sets the inbox's secret (`SIMPLEX_DOOR_SECRET`,
//   toward door.fragment.internal), then `connect` starts the door and
//   tells it where this chat's inbox is, the secret filled at the
//   platform's egress (never this code's).
// - `link` asks the door for a one-time invitation, which its owner's
//   SimpleX app scans.
// - `intake` (the inbox's trigger alone) takes each message the door
//   delivers, once by its SimpleX message id (`seen`), and publishes it as
//   its owner's words to the bound agent (`job.door`).
// - `mirror` (each reply the bound agent posts here) sends it through the
//   door, once by its record (the door keeps what it sent by key).
// - `copy` (the spike's history question): its owner copies their old
//   SimpleX chat here before the door is bound, each record once, marked
//   copied with who said it and when.
import { DurableObject } from "cloudflare:workers";

const DOOR = "https://door.fragment.internal";
const TEXT_MAX = 32 * 1024;
const SEEN_MAX = 20000;

async function door(job, path, body, headers = {}) {
  const resp = await job.fetch(`${DOOR}/${path}`, { method: "POST", headers: { "content-type": "application/json", ...headers }, body: JSON.stringify(body ?? {}) });
  let answer = {};
  try {
    answer = resp.json();
  } catch {}
  if (resp.status >= 300) throw new Error(`the door's ${path}: ${resp.status} ${JSON.stringify(answer).slice(0, 300)}`);
  return answer;
}

export class App extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT NOT NULL)");
    ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS seen (shared TEXT PRIMARY KEY, n INTEGER NOT NULL)");
    ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS copied (src TEXT NOT NULL, seq INTEGER NOT NULL, PRIMARY KEY (src, seq))");
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
    const copied = this.ctx.storage.sql.exec("SELECT count(*) AS n FROM copied").one().n;
    return { connected: this.get("connected"), error: this.get("error"), invitation: this.get("invitation"), copied };
  }

  async connect({ platform, name } = {}, job) {
    const inbox = `${new URL(platform).origin}/api/f/${job.fragment}/inbox`;
    try {
      const a = await door(job, "connect", { inbox, name: name ?? null }, { "x-door-secret": "{{SIMPLEX_DOOR_SECRET}}" });
      await job.call("known", { connected: { inbox, generation: a.generation } });
      return { connected: true, generation: a.generation };
    } catch (e) {
      await job.call("known", { error: String(e.message ?? e) });
      return { connected: false, error: String(e.message ?? e) };
    }
  }

  known({ connected, error }, call) {
    if (call.agent) throw new Error("its owner connects it, in person");
    this.put("error", error ? String(error).slice(0, 300) : null);
    if (connected) this.put("connected", connected);
    return { ok: true };
  }

  async link(_, job) {
    const a = await door(job, "link", {});
    await job.call("invited", { link: a.link, short: a.short ?? null });
    return { link: a.link, short: a.short ?? null };
  }

  invited({ link, short }, call) {
    if (call.agent) throw new Error("its owner links it, in person");
    this.put("invitation", { link, short, at: Date.now() });
    return { ok: true };
  }

  async status(_, job) {
    return door(job, "status", {});
  }

  // a SimpleX message taken once: the door delivers again after a crash
  // or a rollback (its cursor rolls back with its databases), by the
  // message's id, which both sides keep through either
  seen({ shared }) {
    const fresh = this.ctx.storage.sql.exec("INSERT INTO seen (shared, n) VALUES (?, (SELECT coalesce(max(n), 0) + 1 FROM seen)) ON CONFLICT DO NOTHING RETURNING n", shared).toArray().length === 1;
    this.ctx.storage.sql.exec("DELETE FROM seen WHERE n < (SELECT max(n) FROM seen) - ?", SEEN_MAX);
    return { fresh };
  }

  async intake({ record } = {}, job) {
    if (job.via !== "channel") return { ignored: "only its inbox's trigger" };
    const p = record?.body?.payload;
    if (p?.kind !== "message" || typeof p.shared !== "string" || !p.shared) return { ignored: "not a message" };
    if (!(await job.call("seen", { shared: p.shared })).fresh) return { ignored: "taken already", shared: p.shared };
    if (!job.door) return { ignored: "not linked" };
    const text = typeof p.text === "string" ? [...p.text].slice(0, TEXT_MAX).join("") : "";
    if (!text) return { ignored: "no text" };
    const published = await job.publish("chat", { text, to: [job.door.to], relayed: job.door.relayed, simplex: { item: p.item, shared: p.shared, at: p.atMs ?? null } }, "message");
    return { relayed: true, seq: published?.seq ?? null };
  }

  async mirror({ record } = {}, job) {
    if (job.via !== "channel") return { sent: 0, why: "only the chat's trigger sends" };
    if (!job.door) return { sent: 0, why: "not linked" };
    if (record?.principal !== job.door.to) return { sent: 0, why: "not its bound agent's" };
    const b = record?.body;
    if (!b || typeof b.turn !== "string" || b.interim === true || (b.kind !== undefined && b.kind !== "message")) return { sent: 0, why: "not a reply" };
    const said = typeof b.text === "string" ? b.text.trim() : "";
    const files = Array.isArray(b.attachments) ? b.attachments.length : 0;
    const text = said || (files ? "(Sent a file: open the chat to see it.)" : "");
    if (!text) return { sent: 0, why: "nothing said" };
    const a = await door(job, "send", { key: `chat:${record.seq}`, text });
    return { sent: 1, items: a.items ?? [], replayed: a.replayed === true };
  }

  // The spike's history copy: records of the owner's old SimpleX chat
  // (`from`), in order, each once, before the door is bound (so the
  // agent, which joins at the binding, never takes one as new).
  copy({ from, records }, call) {
    if (call.agent) throw new Error("its owner copies it, in person");
    // (a mutation is not told its binding: the driver copies before it binds)
    let copied = 0;
    let skipped = 0;
    for (const r of records) {
      if (!Number.isInteger(r?.seq) || typeof r?.principal !== "string" || !Number.isFinite(r?.at)) throw new Error("each record is {seq, at, principal, body}");
      const fresh = this.ctx.storage.sql.exec("INSERT INTO copied (src, seq) VALUES (?, ?) ON CONFLICT DO NOTHING RETURNING seq", from, r.seq).toArray().length === 1;
      if (!fresh) {
        skipped += 1;
        continue;
      }
      const b = r.body ?? {};
      const relay = b.relayed && typeof b.relayed === "object" ? b.relayed : null;
      // a relayed message was its owner's words; anything else, its poster's
      const by = relay && typeof relay.for === "string" ? relay.for : r.principal;
      const text = typeof b === "string" ? b : typeof b.text === "string" ? b.text : "";
      call.publish("chat", { text: [...text].slice(0, TEXT_MAX).join(""), copied: { from, seq: r.seq, at: r.at, by, via: relay ? String(relay.via ?? "") : null, files: Array.isArray(b.attachments) ? b.attachments.length : 0 } }, "message");
      copied += 1;
    }
    return { copied, skipped };
  }
}
