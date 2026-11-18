// An agent's fragment acting in other fragments as its agent
// (`this.otherFragments`): `act` does one act and answers it, or its
// refusal; `relay` posts what its inbox is given to the room it names, at
// the inbox delivery's hop depth.
import { DurableObject } from "cloudflare:workers";

export class App extends DurableObject {
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
}
