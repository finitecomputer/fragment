// The Telegram connector's connect (templates/telegram/app.mjs), each
// answer told with the attempt its page named, so a page waits for its own
// (#1092's review: a reconnect form left open while another dialog's
// reconnect answered took that answer as its own). Against an in-memory
// SQLite (as a Durable Object's) and a fake of Telegram's Bot API behind
// `job.fetch`, as the platform hands a job its egress.
import { test } from "node:test";
import assert from "node:assert/strict";
import { register } from "node:module";
import { DatabaseSync } from "node:sqlite";

const BASE = "export class DurableObject { constructor(ctx, env) { this.ctx = ctx; this.env = env; } }";
const HOOKS = `export async function resolve(specifier, context, next) {
  if (specifier === "cloudflare:workers") return { url: ${JSON.stringify("data:text/javascript," + encodeURIComponent(BASE))}, shortCircuit: true };
  return next(specifier, context);
}`;
register("data:text/javascript," + encodeURIComponent(HOOKS));
const { App } = await import(new URL("../../../templates/telegram/app.mjs", import.meta.url).href);

// an app on a fresh store, and Telegram as `bots` say: the token each takes
function world(bots) {
  const db = new DatabaseSync(":memory:");
  // each statement run as it is made, as a Durable Object's `exec` runs it
  const sql = { exec: (q, ...p) => { const st = db.prepare(q); const rows = /^\s*select\b|\breturning\b/i.test(q) ? st.all(...p) : (st.run(...p), []); return { toArray: () => rows }; } };
  const app = new App({ storage: { sql } }, {});
  const owner = { agent: null };
  let token = null;
  const job = {
    fragment: "telegram--k3x9",
    call: async (op, input) => app[op](input, owner),
    fetch: async (url) => {
      const method = new URL(url.replace("{{TELEGRAM_BOT_TOKEN}}", token)).pathname.split("/").pop();
      const bot = bots[token];
      const body = !bot ? { ok: false, description: "Unauthorized" } : method === "getMe" ? { ok: true, result: bot } : { ok: true, result: true };
      return { ok: !!bot, status: bot ? 200 : 401, json: () => body };
    },
  };
  const connect = (t, attempt) => {
    token = t;
    return app.connect({ platform: "https://finite.example", ...(attempt ? { attempt } : {}) }, job);
  };
  return { app, connect };
}

const BOTS = { good: { id: 7, username: "maple_bot" }, other: { id: 8, username: "wren_bot" } };

test("a connector that never connected on this code has answered no attempt (an older one's state reads as before)", () => {
  const { app } = world(BOTS);
  assert.deepEqual(app.state().answered, []);
  assert.equal(app.state().bot, null);
});

test("each answer is kept with its attempt: its bot, or why not, the bot before kept and its account still paired", async () => {
  const { app, connect } = world(BOTS);
  await connect("good", "a1");
  assert.deepEqual([app.state().answered, app.state().bot, app.state().error], [["a1"], { username: "maple_bot" }, null]);
  // paired, then another form's token Telegram refuses
  app.put("paired", { user: 1, chat: 1, name: "Robin", bot: 7 });
  await connect("nope", "b2");
  assert.deepEqual([app.state().answered, app.state().bot, app.state().paired], [["a1", "b2"], { username: "maple_bot" }, { name: "Robin" }]);
  assert.match(app.state().error, /didn't take that bot token/);
  // the same bot's token again: its own answer, the error gone, still paired
  await connect("good", "c3");
  assert.deepEqual([app.state().answered.at(-1), app.state().error, app.state().paired], ["c3", null, { name: "Robin" }]);
});

test("two answers before a page looks: each page still finds its own, never another's in its place", async () => {
  const { app, connect } = world(BOTS);
  // this dialog's reconnect, then another's, both answered before its first poll
  await connect("good", "mine");
  await connect("good", "theirs");
  const answered = app.state().answered;
  assert.ok(answered.includes("mine") && answered.includes("theirs"));
  assert.ok(!answered.includes("never"), "a page whose attempt had no answer keeps waiting");
});

test("the attempts kept are the latest few, and a connect naming none (a call of the API's own) adds none", async () => {
  const { app, connect } = world(BOTS);
  for (let i = 0; i < 20; i++) await connect("good", `a${i}`);
  await connect("good");
  const answered = app.state().answered;
  assert.equal(answered.length, 16);
  assert.deepEqual([answered[0], answered.at(-1)], ["a4", "a19"]);
});
