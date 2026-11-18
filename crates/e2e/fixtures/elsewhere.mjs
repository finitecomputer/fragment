// An agent's fragment acting in other fragments as its agent
// (`this.otherFragments`): `act` does one act and answers it, or its
// refusal; `relay` posts what its inbox is given to the room it names, at
// the inbox delivery's hop depth; `seen`, its followed trigger, keeps each
// record of a channel it follows, one row a run.
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

  async relay({ record }) {
    const { name, id, body } = record.body.payload;
    return await this.otherFragments.post(name, "chat", { id, body });
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
