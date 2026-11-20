// An agent's fragment acting in other fragments as its agent: `act` drafts
// or reads through its capability (`this.otherFragments`), answering it or
// its refusal; `send` posts as a job's step (`job.otherFragments`), and
// sleeps after when asked, so its body runs again; `relay` posts what its
// inbox is given to the room it names, at the inbox delivery's hop depth.
import { DurableObject } from "cloudflare:workers";

export class App extends DurableObject {
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
}
