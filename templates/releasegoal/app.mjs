// One sandbox release note, not a goal engine. Never runs Hermes in a job.
import { DurableObject } from "cloudflare:workers";

export class App extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS goal (id INTEGER PRIMARY KEY CHECK(id = 1), body TEXT NOT NULL)");
  }

  state() {
    const row = this.ctx.storage.sql.exec("SELECT body FROM goal WHERE id = 1").toArray()[0];
    return row ? JSON.parse(row.body) : null;
  }

  #save(g) {
    this.ctx.storage.sql.exec("INSERT OR REPLACE INTO goal VALUES (1, ?)", JSON.stringify(g));
    return g;
  }

  draft({ text, vendor, key }, call) {
    // Deliberately local-only: this demonstration must never publish for real.
    const url = new URL(vendor);
    if (url.protocol !== "http:" || url.hostname !== "127.0.0.1" || url.pathname !== "/release" || url.search || url.hash || url.username || url.password)
      throw new Error("sandbox vendor must be http://127.0.0.1:<port>/release");
    const old = this.state();
    if (old) {
      if (old.text !== text || old.vendor !== vendor || old.effect.key !== key) throw new Error("this fragment already has a different release goal");
      return old;
    }
    call.files.write("artifacts/release.md", text);
    return this.#save({ desiredOutcome: "Publish this release note to the sandbox after tomorrow's review and owner approval", status: "scheduled", nextAction: "review", wakeCondition: "one scheduled follow-up", artifacts: ["artifacts/release.md"], text, vendor, effect: { key, receipt: null }, approval: null });
  }

  review() {
    const g = this.state();
    if (!g) throw new Error("draft first");
    if (g.status !== "scheduled") return g;
    return this.#save({ ...g, status: "waiting_owner", nextAction: "ask owner to publish", wakeCondition: "owner answer to the original approval card" });
  }

  approve({ answer, source }) {
    const g = this.state();
    if (!g || !["waiting_owner", "approved", "done", "canceled"].includes(g.status)) throw new Error("review before approval");
    if (g.approval) {
      if (g.approval.answer !== answer || g.approval.source !== source) throw new Error("approval is immutable");
      return g;
    }
    return this.#save({ ...g, approval: { answer, source }, status: answer === "Publish" ? "approved" : "canceled", nextAction: answer === "Publish" ? "deliver" : null, wakeCondition: answer === "Publish" ? "fresh continuation turn" : null });
  }

  receipt({ key, receipt }) {
    const g = this.state();
    if (!g || !["approved", "done"].includes(g.status) || g.effect.key !== key || receipt.key !== key || receipt.id !== "sandbox-release") throw new Error("receipt does not match the approved effect");
    if (g.effect.receipt && JSON.stringify(g.effect.receipt) !== JSON.stringify(receipt)) throw new Error("conflicting receipt");
    return this.#save({ ...g, status: "done", nextAction: null, wakeCondition: null, effect: { key, receipt } });
  }

  async deliver(_, job) {
    const g = await job.call("state", {});
    if (g?.status === "done") return g;
    if (g?.status !== "approved") throw new Error("owner approval required");
    const response = await job.fetch(g.vendor, { method: "POST", headers: { "Content-Type": "application/json", "Idempotency-Key": g.effect.key }, body: JSON.stringify({ text: g.text }) });
    if (!response.ok) throw new Error(`sandbox refused: ${response.status}`);
    return await job.call("receipt", { key: g.effect.key, receipt: response.json() });
  }
}
