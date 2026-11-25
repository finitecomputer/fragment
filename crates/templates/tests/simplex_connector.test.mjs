// The SimpleX connector (templates/simplex/app.mjs), against an in-memory
// SQLite (as a Durable Object's) and a fake of its door behind `job.fetch`,
// as the platform hands a job its door. Valid (its owner's message
// published to the bound agent as theirs; a reply asked until sent, by
// one key), invalid (unbound, a member's call, another's reply, a kind not
// text), replay (a message delivered again taken once), and the
// invitation kept for its owner alone, never in a run's output, with its
// QR code; then whom it linked, watched for and checked, and why a step
// failed, for its setup page.
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
// its door, its life's, as its connect was told it
const DOOR = "juniper-sx--k3x9--0123456789ab";
const BOUND = { relayed: { via: "simplex", for: "npub1paul", agent: "juniper--k3x9" }, to: AGENT };

// an app on a fresh store, and its door as `answers` say: each ask's
// (status, body) in turn by its name, else 200 {}
function world(answers = {}) {
  const db = new DatabaseSync(":memory:");
  const sql = { exec: (q, ...p) => { const st = db.prepare(q); const rows = /^\s*select\b|\breturning\b/i.test(q) ? st.all(...p) : (st.run(...p), []); return { toArray: () => rows }; } };
  const app = new App({ storage: { sql } }, {});
  app.connected({ door: DOOR });
  const w = { app, asked: [], published: [], slept: [], calls: [] };
  w.job = (via, door = BOUND) => ({
    via,
    door,
    call: async (op, input) => (w.calls.push(op), app[op](input, { agent: null })),
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

const message = (id, text = `text of ${id}`, kind = "text", door = DOOR) => ({ id, kind, text, at: "2026-11-21T00:00:00Z", file: null, door });

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
  // a door an earlier life of its name made (the review of #1450), its secret the same
  assert.deepEqual(await w.app.intake(w.delivered(message("m0", "from before", "text", "juniper-sx--k3x9--fedcba987654")), w.job("channel")), { ignored: "not its door's" });
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
  assert.deepEqual(await w.app.mirror(w.reply(1), w.job("call")), { sent: 0, why: "only its triggers send" });
  assert.deepEqual(await w.app.mirror(w.reply(2), w.job("channel", null)), { sent: 0, why: "not linked" });
  assert.deepEqual(await w.app.mirror({ record: { seq: 3, principal: "npub1other", body: { text: "x", turn: "t" } } }, w.job("channel")), { sent: 0, why: "not its bound agent's" });
  assert.deepEqual(await w.app.mirror(w.reply(4, { interim: true }), w.job("channel")), { sent: 0, why: "not a reply" });
  assert.deepEqual(await w.app.mirror(w.reply(5, { text: " " }), w.job("channel")), { sent: 0, why: "nothing said" });
  assert.deepEqual(w.asked, []);
});

// a card its bound agent posts on `work`, asking its owner
const card = (seq, body) => ({
  record: {
    channel: "work",
    seq,
    principal: AGENT,
    body: { kind: "turn.prompt", turn: "t1", prompt: "c-1", text: "What should I plant?", asks: "npub1paul", expiresAt: 1, options: [{ id: "c0", label: "Basil" }, { id: "c1", label: "Mint" }, { id: "other", label: "Something else", words: true }], ...body },
  },
});

test("its bound agent's question to its owner goes through the door, worded as every door words one, by its record's key (FIN-747)", async () => {
  const w = world({ reply: [[202, { done: false }], [200, { done: true }]] });
  assert.deepEqual(await w.app.mirror(card(7), w.job("channel")), { sent: 1 });
  assert.deepEqual(w.asked.map((a) => a.body), Array(2).fill({ key: "work:7", text: "What should I plant?\n\n(Answer with one of: Basil, Mint, or in your own words.)" }), "one key on every ask");
  // a code answered on its computer (Claude's sign-in) is never asked for here
  const code = world();
  await code.app.mirror(card(8, { options: [{ id: "code", label: "Paste the code", words: true, computer: true }] }), code.job("channel"));
  assert.equal(code.asked[0].body.text, "I need something from you on a card in our chat. Type it there, not here: from there it goes to your computer alone.", "never its words, which say to paste it here");
});

test("only a question its bound agent asks its owner goes, and nothing else of its turns' work", async () => {
  const w = world();
  assert.deepEqual(await w.app.mirror(card(1, { asks: "npub1someone" }), w.job("channel")), { sent: 0, why: "it asks someone else" });
  assert.deepEqual(await w.app.mirror({ record: { ...card(2).record, principal: "npub1other" } }, w.job("channel")), { sent: 0, why: "not its bound agent's" });
  assert.deepEqual(await w.app.mirror({ record: { channel: "work", seq: 3, principal: AGENT, body: { kind: "turn.start", turn: "t1" } } }, w.job("channel")), { sent: 0, why: "not a question" });
  assert.deepEqual(await w.app.mirror(card(4), w.job("channel", null)), { sent: 0, why: "not linked" });
  assert.deepEqual(await w.app.mirror(card(5), w.job("call")), { sent: 0, why: "only its triggers send" });
  assert.deepEqual(w.asked, []);
});

test("connect gives the door the inbox's secret as a placeholder the platform fills, never its value", async () => {
  const w = world({ connect: [[200, { started: 1, door: "juniper-sx--k3x9--00112233aabb" }]] });
  assert.deepEqual(await w.app.connect({ name: "Juniper" }, w.job("call")), { connected: true, started: 1 });
  assert.equal(w.app.get("door"), "juniper-sx--k3x9--00112233aabb", "its own door kept, as the door named it");
  assert.equal(w.asked[0].url, "https://door.fragment.internal/connect");
  assert.equal(w.asked[0].headers["x-door-secret"], "{{SIMPLEX_DOOR_SECRET}}");
  assert.deepEqual(w.asked[0].body, { name: "Juniper" });
  const none = world({ connect: [[409, { error: "no_doors", message: "this deployment runs no doors (no container for them)" }]] });
  assert.deepEqual(await none.app.connect({ name: "Juniper" }, none.job("call")), { connected: false, error: "the door answered 409: this deployment runs no doors (no container for them)" });
  assert.deepEqual([none.app.state().why.text, none.app.state().why.code, none.app.state().why.step], ["the door answered 409: this deployment runs no doors (no container for them)", "no_doors", "connect"], "said to its setup page, with the door's code and its step");
});

test("the invitation for its owner's app is kept for its owner's query alone, never in a run's output", async () => {
  const link = "https://simplex.chat/invitation#/?v=2-7&smp=a-secret-invitation";
  const w = world({ link: [[202, {}]], unlink: [[202, {}]], state: [[200, { runner: { phase: "wanting" } }], [200, { runner: { link, short: "https://smp.example/i#short" } }]] });
  const out = await w.app.link({}, w.job("call"));
  assert.deepEqual(out, { invited: true, linked: false });
  assert.deepEqual(w.asked[0].body, { n: 1 }, "its link numbered to the door");
  assert.ok(!JSON.stringify(out).includes("secret-invitation"), "a run's output, which its agent reads, holds none");
  assert.equal(w.app.state().invitation.link, link);
  assert.deepEqual(await w.app.unlink({}, w.job("call")), { unlinked: true });
  assert.equal(w.app.state().invitation, null, "unlinked, none kept");
});

// the door's runner's state, as its door answers `state`
const runner = (r) => [200, { runner: r }];
const QR = { size: 21, dark: "10".repeat(220) + "1" };

test("a link watches for its owner's app to answer, keeps whom it linked, and lets its invitation go", async () => {
  const link = "simplex:/invitation#/?v=2-7&smp=a-secret-invitation";
  const inviting = runner({ phase: "inviting", link, short: "https://smp.example/i#short", qr: QR });
  const w = world({ link: [[202, {}]], state: [runner({ phase: "asking" }), inviting, inviting, inviting, runner({ phase: "linked", name: "Robin" })] });
  const out = await w.app.link({}, w.job("call"));
  assert.deepEqual(out, { linked: true });
  assert.ok(!JSON.stringify(out).includes("secret-invitation") && !JSON.stringify(out).includes("Robin"), "a run's output holds neither");
  const s = w.app.state();
  assert.deepEqual([s.connected, s.invitation, s.linked.name, s.why], [true, null, "Robin", null], "linked: its invitation used");
  assert.deepEqual(w.slept, [1000, 1000, 2000, 2000, 2000], "a second apart for its invitation, then the watch's waits");

  // its invitation, as kept before it was answered: its QR code whole
  const v = world({ link: [[202, {}]], state: [inviting, runner({ phase: "off" })] });
  assert.deepEqual(await v.app.link({}, v.job("call")), { invited: true, linked: false }, "unlinked meanwhile: the watch ends");
  assert.deepEqual(v.app.state().invitation.qr, QR);
  assert.equal(v.app.state().invitation.short, "https://smp.example/i#short");
  // another link asked meanwhile shows another invitation: this watch ends
  const other = world({ link: [[202, {}]], state: [inviting, runner({ phase: "inviting", link: "simplex:/invitation#/?v=2-7&smp=another" })] });
  assert.deepEqual(await other.app.link({}, other.job("call")), { invited: true, linked: false });
});

test("a link's watch is bounded, within a job's steps, and a QR code is kept only whole", async () => {
  const link = "simplex:/invitation#/?v=2-7&smp=s";
  const w = world({ link: [[202, {}]], state: Array(200).fill(runner({ phase: "inviting", link, qr: { size: 21, dark: "1".repeat(440) } })) });
  assert.deepEqual(await w.app.link({}, w.job("call")), { invited: true, linked: false });
  const steps = w.asked.length + w.slept.length + w.calls.length;
  assert.ok(steps <= 256, `a job's steps: ${steps}`);
  assert.ok(w.slept.reduce((a, b) => a + b, 0) >= 8 * 60_000, "about 8 minutes, as long as a person takes to scan");
  assert.equal(w.app.state().invitation.qr, null, "a QR code short of its square: none");
  for (const qr of [{ size: 21, dark: "2".repeat(441) }, { size: 20, dark: "1".repeat(400) }, { size: 178, dark: "1".repeat(178 * 178) }, { size: "21", dark: "1".repeat(441) }, null]) {
    const v = world({ link: [[202, {}]], state: [runner({ phase: "inviting", link, qr }), runner({ phase: "off" })] });
    await v.app.link({}, v.job("call"));
    assert.equal(v.app.state().invitation.qr, null, JSON.stringify(qr)?.slice(0, 40));
  }
});

test("a door linked already keeps whom it linked; check looks once, for its own link alone", async () => {
  const w = world({ link: [[202, {}]], state: [runner({ phase: "linked", name: "Robin" })] });
  assert.deepEqual(await w.app.link({}, w.job("call")), { linked: true });
  assert.equal(w.app.state().linked.name, "Robin");
  const c = world({ state: [runner({ phase: "inviting", link: "x" }), runner({ phase: "linked", name: "Robin" })] });
  assert.deepEqual(await c.app.check({}, c.job("call")), { linked: false });
  assert.deepEqual(await c.app.check({}, c.job("call")), { linked: true });
  assert.equal(c.app.state().linked.name, "Robin");
  // an unlink asked while check looked: what it saw is no one's
  const late = world({ unlink: [[202, {}]] });
  const job = late.job("call");
  job.fetch = async (url) => {
    assert.ok(url.endsWith("/state"));
    await late.app.asking();
    return { ok: true, status: 200, json: () => ({ runner: { phase: "linked", name: "Robin" } }) };
  };
  assert.deepEqual(await late.app.check({}, job), { linked: false, why: "unlinked since" });
  assert.equal(late.app.state().linked, null);
});

test("a check that reads an unlink's number keeps no link, the door not yet unlinked", async () => {
  const c = world({ state: [runner({ phase: "linked", name: "Robin" })] });
  c.app.asking({ unlink: true });
  assert.deepEqual(await c.app.check({}, c.job("call")), { linked: false, why: "unlinked since" });
  assert.equal(c.app.state().linked, null);
  // a link asked after it is kept again
  const { n } = c.app.asking({});
  assert.deepEqual(c.app.linked({ n, name: "Robin" }), { kept: true });
});

test("a step that fails says why to its setup page, until another is asked", async () => {
  const w = world({ link: [[409, { message: "not running" }], [202, {}]], state: Array(30).fill(runner({ phase: "asking" })) });
  assert.deepEqual(await w.app.link({}, w.job("call")), { invited: false, why: "the door answered 409: not running" });
  assert.equal(w.app.state().why.text, "the door answered 409: not running");
  assert.deepEqual(await w.app.link({}, w.job("call")), { invited: false, why: "no invitation yet" });
  assert.equal(w.app.state().why.text, "no invitation yet", "the newer step's");
  const u = world({ unlink: [[503, { error: "starting", message: "starting" }]] });
  assert.deepEqual(await u.app.unlink({}, u.job("call")), { unlinked: false });
  assert.deepEqual([u.app.state().why.text, u.app.state().why.code, u.app.state().why.step], ["the door answered 503: starting", "starting", "unlink"], "and which step failed, for its page to try again");
  assert.equal(w.app.state().why.step, "link");
  u.app.asking();
  assert.equal(u.app.state().why, null, "let go as another step is asked");
  // a failure of a link asked before the newest is no one's
  assert.deepEqual(u.app.failed({ n: 1, why: "late" }), { kept: false });
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
