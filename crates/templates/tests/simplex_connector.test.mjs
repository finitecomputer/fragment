// The SimpleX connector (templates/simplex/app.mjs), against an in-memory
// SQLite (as a Durable Object's) and a fake of its door behind `job.fetch`,
// as the platform hands a job its door. Valid (its owner's message
// published to the bound agent as theirs; a reply asked until sent, by
// one key), invalid (unbound, a member's call, another's reply, a kind not
// text), replay (a message delivered again taken once), and the
// invitation kept for its owner alone, never in a run's output.
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
const { App } = await import(new URL("../../../templates/simplex/app.mjs", import.meta.url).href);

const AGENT = "npub1juniper";
const BOUND = { relayed: { via: "simplex", for: "npub1paul", agent: "juniper--k3x9" }, to: AGENT };

// an app on a fresh store, and its door as `answers` say: each ask's
// (status, body) in turn by its name, else 200 {}
function world(answers = {}) {
  const db = new DatabaseSync(":memory:");
  const sql = { exec: (q, ...p) => { const st = db.prepare(q); const rows = /^\s*select\b|\breturning\b/i.test(q) ? st.all(...p) : (st.run(...p), []); return { toArray: () => rows }; } };
  const app = new App({ storage: { sql } }, {});
  const w = { app, asked: [], published: [], slept: [] };
  w.job = (via, door = BOUND) => ({
    via,
    door,
    call: async (op, input) => app[op](input, { agent: null }),
    publish: async (channel, body, kind) => void w.published.push({ channel, body, kind }),
    sleep: async (ms) => void w.slept.push(ms),
    fetch: async (url, init) => {
      const ask = new URL(url).pathname.slice(1);
      w.asked.push({ url, ask, headers: init.headers, body: JSON.parse(init.body) });
      const [status, body] = answers[ask]?.shift() ?? [200, {}];
      return { ok: status < 300, status, json: () => body };
    },
  });
  w.delivered = (payload) => ({ record: { body: { source: "simplex", payload } } });
  w.reply = (seq, body) => ({ record: { seq, principal: AGENT, body: { text: "It'll be sunny.", turn: "t1", ...body } } });
  return w;
}

const message = (id, text = `text of ${id}`, kind = "text") => ({ id, kind, text, at: "2026-11-21T00:00:00Z", file: null });

test("its owner's message is published to its bound agent as theirs, once however often the door delivers it", async () => {
  const w = world();
  assert.deepEqual(await w.app.intake(w.delivered(message("m1", "  hello from simplex ")), w.job("channel")), { relayed: true });
  assert.deepEqual(w.published, [{ channel: "chat", body: { text: "hello from simplex", to: [AGENT], relayed: BOUND.relayed }, kind: "message" }]);
  assert.deepEqual(await w.app.intake(w.delivered(message("m1")), w.job("channel")), { ignored: "taken already" });
  assert.equal(w.published.length, 1);
  assert.deepEqual(w.asked, [], "nothing asked of the door");
});

test("nothing is relayed but its inbox's own message, while bound, of text", async () => {
  const w = world();
  assert.deepEqual(await w.app.intake(w.delivered(message("m1")), w.job("call")), { ignored: "only its inbox's trigger" });
  assert.deepEqual(await w.app.intake(w.delivered({ text: "no id" }), w.job("channel")), { ignored: "not a message" });
  assert.deepEqual(await w.app.intake(w.delivered(message("m2")), w.job("channel", null)), { ignored: "not linked" });
  assert.deepEqual(await w.app.intake(w.delivered(message("m3", "", "voice")), w.job("channel")), { ignored: "not text" });
  assert.deepEqual(w.published, []);
  assert.deepEqual(w.asked.map((a) => [a.ask, a.body]), [["reply", { key: "unlinked:m2", text: "This chat isn't linked to your agent." }], ["reply", { key: "only-text:m3", text: "Only text reaches your agent from SimpleX for now." }]], "its owner's app told why, once a message");
  // a door that fetches nothing (shut) fails no intake over a word unsent
  const shut = world();
  const job = shut.job("channel", null);
  job.fetch = async () => {
    throw new Error("this connector is bound as a simplex door that is shut");
  };
  assert.deepEqual(await shut.app.intake(shut.delivered(message("m4")), job), { ignored: "not linked" });
});

test("its bound agent's reply goes through the door by its record's key, asked again until sent", async () => {
  const w = world({ reply: [[202, { done: false }], [202, { done: false }], [200, { done: true }]] });
  assert.deepEqual(await w.app.mirror(w.reply(41), w.job("channel")), { sent: 1 });
  assert.deepEqual(w.asked.map((a) => a.body), Array(3).fill({ key: "chat:41", text: "It'll be sunny." }), "one key on every ask");
  assert.deepEqual(w.slept, [1000, 2000], "waiting longer each time");
  for (const [status, why] of [[409, "not linked"], [410, "its SimpleX link ended"]]) {
    const v = world({ reply: [[status, {}]] });
    assert.deepEqual(await v.app.mirror(v.reply(42), v.job("channel")), { sent: 0, why });
  }
  const held = world({ reply: Array(40).fill([202, { done: false }]) });
  assert.deepEqual(await held.app.mirror(held.reply(43), held.job("channel")), { sent: 0, why: "the door still holds it unsent" });
  assert.equal(held.asked.length, 30, "bounded");
  assert.ok(held.slept.every((ms) => ms <= 30_000));
});

test("only its bound agent's replies go, never interim words, a member's call or another's", async () => {
  const w = world();
  assert.deepEqual(await w.app.mirror(w.reply(1), w.job("call")), { sent: 0, why: "only the chat's trigger sends" });
  assert.deepEqual(await w.app.mirror(w.reply(2), w.job("channel", null)), { sent: 0, why: "not linked" });
  assert.deepEqual(await w.app.mirror({ record: { seq: 3, principal: "npub1other", body: { text: "x", turn: "t" } } }, w.job("channel")), { sent: 0, why: "not its bound agent's" });
  assert.deepEqual(await w.app.mirror(w.reply(4, { interim: true }), w.job("channel")), { sent: 0, why: "not a reply" });
  assert.deepEqual(await w.app.mirror(w.reply(5, { text: " " }), w.job("channel")), { sent: 0, why: "nothing said" });
  assert.deepEqual(w.asked, []);
});

test("connect gives the door the inbox's secret as a placeholder the platform fills, never its value", async () => {
  const w = world({ connect: [[200, { started: 1 }]] });
  assert.deepEqual(await w.app.connect({ name: "Juniper" }, w.job("call")), { connected: true, started: 1 });
  assert.equal(w.asked[0].url, "https://door.fragment.internal/connect");
  assert.equal(w.asked[0].headers["x-door-secret"], "{{SIMPLEX_DOOR_SECRET}}");
  assert.deepEqual(w.asked[0].body, { name: "Juniper" });
  const none = world({ connect: [[409, { error: "no_doors", message: "this deployment runs no doors (no container for them)" }]] });
  assert.deepEqual(await none.app.connect({ name: "Juniper" }, none.job("call")), { connected: false, error: "the door answered 409: this deployment runs no doors (no container for them)" });
});

test("the invitation for its owner's app is kept for its owner's query alone, never in a run's output", async () => {
  const link = "https://simplex.chat/invitation#/?v=2-7&smp=a-secret-invitation";
  const w = world({ link: [[202, {}]], unlink: [[202, {}]], state: [[200, { runner: { phase: "wanting" } }], [200, { runner: { link, short: "https://smp.example/i#short" } }]] });
  const out = await w.app.link({}, w.job("call"));
  assert.deepEqual(out, { invited: true });
  assert.ok(!JSON.stringify(out).includes("secret-invitation"), "a run's output, which its agent reads, holds none");
  assert.equal(w.app.state().invitation.link, link);
  assert.deepEqual(await w.app.unlink({}, w.job("call")), { unlinked: true });
  assert.equal(w.app.state().invitation, null, "unlinked, none kept");
});

test("an invitation found after an unlink was asked is kept by no one", async () => {
  const link = "https://simplex.chat/invitation#/?v=2-7&smp=late";
  const w = world({ link: [[202, {}]], unlink: [[202, {}]] });
  // an unlink asked while the link looks for its invitation
  const answers = [async () => (await w.app.unlink({}, w.job("call")), [200, { runner: { phase: "wanting" } }]), [200, { runner: { link } }]];
  const job = w.job("call");
  const fetch = job.fetch;
  job.fetch = async (url, init) => {
    if (!url.endsWith("/state")) return fetch(url, init);
    const [status, body] = await (typeof answers[0] === "function" ? answers.shift()() : answers.shift());
    return { ok: true, status, json: () => body };
  };
  assert.deepEqual(await w.app.link({}, job), { invited: false, why: "unlinked since" });
  assert.equal(w.app.state().invitation, null);
});
