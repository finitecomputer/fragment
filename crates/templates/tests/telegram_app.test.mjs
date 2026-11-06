import { test } from "node:test";
import assert from "node:assert/strict";
import { register } from "node:module";
import { DatabaseSync } from "node:sqlite";

const BASE = `export class DurableObject {
  constructor(ctx, env) { this.ctx = ctx; this.env = env; }
}`;
const HOOKS = `export async function resolve(specifier, context, next) {
  if (specifier === "cloudflare:workers") return { url: ${JSON.stringify("data:text/javascript," + encodeURIComponent(BASE))}, shortCircuit: true };
  return next(specifier, context);
}`;
register("data:text/javascript," + encodeURIComponent(HOOKS));
const { App } = await import(new URL("../../../templates/telegram/app.mjs", import.meta.url).href);

const OWNER = { agent: false };
const AGENT = { agent: true };
const PAIRING_MS = 10 * 60_000;

function storage() {
  const db = new DatabaseSync(":memory:");
  return {
    sql: {
      exec(q, ...args) {
        const st = db.prepare(q);
        const rows = st.columns().length ? st.all(...args) : (st.run(...args), []);
        return { toArray: () => rows, one: () => rows[0] };
      },
    },
  };
}

function connector() {
  const ctx = { storage: storage() };
  return new App(ctx, {});
}

function known(app, id = 1) {
  return app.known({ bot: { id, username: `bot${id}` }, api: "https://api.telegram.org" }, OWNER);
}

function pair(app) {
  app.pair({}, OWNER);
  return app.get("pairing").nonce;
}

function propose(app, nonce, user = 10, chat = 100) {
  return app.propose({ nonce, user, chat, name: `person ${user}` });
}

test("the lifecycle keeps its bot, pairing, paired account, and error as separate facts", () => {
  const app = connector();
  assert.deepEqual(app.state(), { lifecycle: "disconnected", bot: null, error: null, paired: null, pairing: null });

  app.known({ error: "bad token" }, OWNER);
  assert.deepEqual([app.state().lifecycle, app.state().error], ["disconnected", "bad token"]);

  known(app);
  assert.deepEqual([app.state().lifecycle, app.state().bot, app.state().error], ["connected", { username: "bot1" }, null]);
  const nonce = pair(app);
  assert.equal(app.state().lifecycle, "pairing");
  assert.equal(app.state().pairing.candidate, null);
  propose(app, nonce);
  assert.equal(app.state().lifecycle, "pairing");
  assert.deepEqual(app.state().pairing.candidate, { user: 10, name: "person 10" });
  app.confirm({ user: 10 }, OWNER);
  assert.deepEqual([app.state().lifecycle, app.state().paired, app.state().pairing], ["paired", { name: "person 10" }, null]);

  app.known({ error: "webhook failed" }, OWNER);
  assert.deepEqual([app.state().lifecycle, app.state().error, app.state().paired], ["paired", "webhook failed", { name: "person 10" }]);
  app.unpair({}, OWNER);
  assert.equal(app.state().lifecycle, "connected");
});

test("a second distinct candidate cancels pairing and a same-candidate replay changes nothing", () => {
  const app = connector();
  known(app);
  const nonce = pair(app);
  assert.deepEqual(propose(app, nonce), { proposed: true });
  const proposed = app.get("pairing");
  assert.deepEqual(propose(app, nonce), { proposed: true });
  assert.deepEqual(app.get("pairing"), proposed);

  assert.deepEqual(propose(app, nonce, 11, 101), { proposed: false, conflict: true });
  assert.equal(app.state().lifecycle, "connected");
  assert.equal(app.get("pairing"), null);
  assert.equal(app.get("paired"), null);
  assert.deepEqual(propose(app, nonce), { proposed: false });
});

test("pairing, confirmation, and unpairing stay owner-only", () => {
  const app = connector();
  known(app);
  assert.throws(() => app.pair({}, AGENT), /its owner pairs it/);
  const nonce = pair(app);
  propose(app, nonce);
  assert.throws(() => app.confirm({ user: 10 }, AGENT), /its owner confirms it/);
  assert.equal(app.state().lifecycle, "pairing");
  assert.throws(() => app.confirm({ user: 11 }, OWNER), /no account is waiting/);
  assert.equal(app.state().lifecycle, "pairing");
  assert.throws(() => app.unpair({}, AGENT), /its owner unpairs it/);
  assert.equal(app.state().lifecycle, "pairing");
  app.confirm({ user: 10 }, OWNER);
  assert.equal(app.state().lifecycle, "paired");
});

test("an expired pairing is neither shown, proposed, nor confirmed", (t) => {
  const real = Date.now;
  let now = real();
  Date.now = () => now;
  t.after(() => (Date.now = real));
  const app = connector();
  known(app);
  const nonce = pair(app);
  propose(app, nonce);
  now += PAIRING_MS + 1;
  assert.deepEqual([app.state().lifecycle, app.state().pairing], ["connected", null]);
  assert.deepEqual(propose(app, nonce), { proposed: false });
  assert.throws(() => app.confirm({ user: 10 }, OWNER), /no account is waiting/);
  assert.equal(app.get("paired"), null);
});

test("changing bots clears the account, pending pairing, and seen updates", () => {
  const app = connector();
  known(app);
  const nonce = pair(app);
  propose(app, nonce);
  app.confirm({ user: 10 }, OWNER);
  assert.equal(app.seen({ update: 1 }).fresh, true);
  known(app);
  assert.equal(app.state().lifecycle, "paired", "reconnecting the same bot keeps its account");

  pair(app);
  assert.equal(app.seen({ update: 2 }).fresh, true);
  known(app, 2);
  assert.deepEqual([app.state().lifecycle, app.state().paired, app.state().pairing, app.route().paired], ["connected", null, null, null]);
  assert.equal(app.get("pairing"), null);
  assert.deepEqual([app.seen({ update: 1 }).fresh, app.seen({ update: 2 }).fresh], [true, true]);
});

test("existing kv pairing rows need no migration", () => {
  const app = connector();
  app.put("bot", { id: 1, username: "oldbot" });
  app.put("pairing", { nonce: "a".repeat(32), expires: Date.now() + PAIRING_MS, link: "https://t.me/oldbot", candidate: { user: 10, chat: 100, name: "old account" } });
  assert.equal(app.state().lifecycle, "pairing");
  assert.deepEqual(app.state().pairing.candidate, { user: 10, name: "old account" });
  app.confirm({ user: 10 }, OWNER);
  assert.equal(app.state().lifecycle, "paired");
  assert.deepEqual(app.route().paired, { user: 10, chat: 100 });
});
