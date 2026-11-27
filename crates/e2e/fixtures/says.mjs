// An app whose code says what its caller gives it in its own chat, where
// its agent hears it (docs/chat-records.md, "Who wrote a record"): app
// code's words, no one's.
import { DurableObject } from "cloudflare:workers";

export class App extends DurableObject {
  tell({ text }, call) {
    call.publish("chat", { text }, "message");
    return { ok: true };
  }

  whoami(_input, call) {
    return { principal: call.principal, role: call.role };
  }
}
