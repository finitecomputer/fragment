// An agent's fragment acting in other fragments as its agent: `act` drafts
// or reads through its capability (`this.otherFragments`), answering it or
// its refusal; `send` posts as a job's step (`job.otherFragments`), and
// sleeps after when asked, so its body runs again; `relay` posts what its
// inbox is given to the room it names, at the inbox delivery's hop depth;
// `follow` follows a channel there, as a step, and `seen`, its followed
// trigger, keeps each record it hears, one row a run.
import { DurableObject } from "cloudflare:workers";

export class App extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    ctx.storage.sql.exec(
      "CREATE TABLE IF NOT EXISTS seen (n INTEGER PRIMARY KEY AUTOINCREMENT, fragment TEXT NOT NULL, channel TEXT NOT NULL, seq INTEGER NOT NULL, principal TEXT NOT NULL, text TEXT)",
    );
  }

  async act({ act, name, channel, arg }) {
    try {
      return { answer: await this.otherFragments[act](name, channel, arg) };
    } catch (e) {
      return { error: { message: e.message, code: e.code ?? null, status: e.status ?? null } };
    }
  }

  async send({ name, channel, record, sleep }, job) {
    try {
      const answer = await job.otherFragments.post(name, channel, record);
      if (sleep) await job.sleep("1 seconds");
      return { answer };
    } catch (e) {
      return { error: { message: e.message } };
    }
  }

  async relay({ record }, job) {
    const { name, id, body } = record.body.payload;
    return await job.otherFragments.post(name, "chat", { id, body });
  }

  async follow({ name, channel }, job) {
    try {
      return { answer: await job.otherFragments.follow(name, channel) };
    } catch (e) {
      return { error: { message: e.message } };
    }
  }

  seen({ fragment, channel, record }) {
    this.ctx.storage.sql.exec(
      "INSERT INTO seen (fragment, channel, seq, principal, text) VALUES (?, ?, ?, ?, ?)",
      fragment,
      channel,
      record.seq,
      record.principal,
      record.body?.text ?? null,
    );
    return { seq: record.seq };
  }

  seen_log() {
    return this.ctx.storage.sql.exec("SELECT fragment, channel, seq, principal, text FROM seen ORDER BY n").toArray();
  }
}
