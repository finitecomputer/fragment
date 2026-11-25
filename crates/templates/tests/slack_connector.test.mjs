// The Slack connector (templates/slack/app.mjs): what it takes from Slack
// and publishes, as whose words and in which room, and where the agent's
// replies go. Against an in-memory SQLite (as a Durable Object's), a fake
// of Slack's Web API behind `job.fetch`, as the platform hands a job its
// egress, and a job that keeps what it publishes.
import { test } from "node:test";
import assert from "node:assert/strict";
import { register } from "node:module";
import { DatabaseSync } from "node:sqlite";
import { createHash } from "node:crypto";

const BASE = "export class DurableObject { constructor(ctx, env) { this.ctx = ctx; this.env = env; } }";
const HOOKS = `export async function resolve(specifier, context, next) {
  if (specifier === "cloudflare:workers") return { url: ${JSON.stringify("data:text/javascript," + encodeURIComponent(BASE))}, shortCircuit: true };
  return next(specifier, context);
}`;
register("data:text/javascript," + encodeURIComponent(HOOKS));
const { App, readable, taken, continuations } = await import(new URL("../../../templates/slack/app.mjs", import.meta.url).href);

const TEAM = "T0TEAM";
const BOT = "U0BOT";
const PAUL = "U0PAUL";
const MAPLE = "U0MAPLE";
const AGENT = "juniper--k3x9";
const FRAGMENT = "juniper-slack--m4p7";
const RELAYED = { via: "slack", for: "npub1paul", agent: AGENT };
const PEOPLE = { [PAUL]: "Paul", [MAPLE]: "Maple", U0SAM: "Sam" };

// an app on a fresh store, connected to TEAM; Slack as it answers; and the
// records its jobs publish
function world({ history = [], replies = [], failing = [] } = {}) {
  const db = new DatabaseSync(":memory:");
  const sql = { exec: (q, ...p) => { const st = db.prepare(q); const rows = /^\s*select\b|\breturning\b/i.test(q) ? st.all(...p) : (st.run(...p), []); return { toArray: () => rows }; } };
  const app = new App({ storage: { sql } }, {});
  const owner = { agent: null };
  const calls = [];
  const published = [];
  const slack = (url, init) => {
    const u = new URL(url);
    const method = u.pathname.replace("/api/", "");
    const body = init?.body ? JSON.parse(init.body) : Object.fromEntries(u.searchParams);
    calls.push({ method, body, auth: init?.headers?.authorization });
    const ok = (x) => ({ ok: true, ...x });
    switch (method) {
      case "auth.test": return ok({ team_id: TEAM, team: "Acme", user_id: BOT, user: "juniper" });
      case "users.info": return PEOPLE[body.user] ? ok({ user: { name: body.user.toLowerCase(), profile: { display_name: PEOPLE[body.user] } } }) : { ok: false, error: "user_not_found" };
      case "conversations.info": return ok({ channel: { id: body.channel, name: "deploys" } });
      case "conversations.history": return ok({ messages: history });
      case "conversations.replies": return ok({ messages: replies });
      default: return ok({ ts: "1790000099.000100" });
    }
  };
  const job = (via = "channel", door = { to: "npub1juniper", relayed: RELAYED }) => ({
    via,
    fragment: FRAGMENT,
    door,
    call: async (op, input) => app[op](input, owner),
    fetch: async (url, init) => {
      if (failing.some((m) => url.includes(`/api/${m}`))) throw new Error("fetch failed: its tries ran out");
      const answer = slack(url, init);
      return { ok: true, status: 200, json: () => answer };
    },
    publish: async (channel, body, kind) => {
      published.push({ channel, body, kind });
      return { seq: published.length };
    },
    sleep: async () => {},
  });
  return { app, owner, calls, published, job };
}

const event = (e, team = TEAM) => ({ body: { source: "external", payload: { type: "event_callback", team_id: team, event_id: `Ev${e.ts}`, event: e } } });
const mention = (user, text, ts, extra = {}) => event({ type: "app_mention", user, text, ts, channel: "C0DEPLOY", channel_type: "channel", ...extra });
const dm = (user, text, ts, channel = `D${user}`) => event({ type: "message", user, text, ts, channel, channel_type: "im" });
const turnOf = (seq) => createHash("sha256").update(`${AGENT}|${FRAGMENT}/chat/${seq}`).digest("hex").slice(0, 24);

async function connected(w) {
  await w.app.connect({ api: "https://slack.example" }, w.job("call"));
  return w;
}

async function paired(w) {
  const { code } = w.app.pair({}, w.owner);
  await w.app.intake({ record: dm(PAUL, `pair ${code}`, "1790000001.000100") }, w.job());
  w.app.confirm({ user: PAUL }, w.owner);
  w.calls.length = 0;
  return w;
}

test("Slack's URL check is answered with its challenge; anything else, with the platform's own answer", () => {
  const { app } = world();
  assert.deepEqual(app.handshake({ record: { body: { payload: { type: "url_verification", challenge: "abc123" } } } }), { body: "abc123" });
  assert.equal(app.handshake({ record: mention(MAPLE, "hi", "1.1") }), null);
});

test("connect keeps the workspace and bot auth.test names, the token filled at the egress", async () => {
  const w = await connected(world());
  assert.deepEqual(w.app.state().bot, { name: "juniper", team: "Acme" });
  assert.equal(w.calls[0].auth, "Bearer {{SLACK_BOT_TOKEN}}");
  assert.equal(w.app.route().api, "https://slack.example");
});

test("someone in a channel is heard, in its room, with what was said there just before, less the bot's words and its mentions", async () => {
  const history = [
    { ts: "1790000003.000100", user: "U0SAM", text: "deploy <@U0MAPLE> started &amp; running" },
    { ts: "1790000002.000100", user: BOT, text: "earlier answer" },
    { ts: "1790000001.500100", user: MAPLE, text: "<@U0BOT> old question" },
    { ts: "1790000001.000100", bot_id: "B1", text: "a bot's" },
  ];
  const w = await paired(await connected(world({ history })));
  const out = await w.app.intake({ record: mention(MAPLE, "<@U0BOT> is it green?", "1790000004.000100") }, w.job());
  assert.equal(out.heard, true);
  const [rec] = w.published;
  assert.deepEqual(rec.body, {
    text: "is it green?",
    to: ["npub1juniper"],
    heard: { via: "slack", from: MAPLE, name: "Maple" },
    room: { id: "C0DEPLOY", name: "#deploys", thread: "1790000004.000100" },
    context: [{ name: "Sam", text: "deploy @Maple started & running", at: 1790000003000 }],
  });
  assert.equal(rec.kind, "message");
  assert.deepEqual(w.calls.find((c) => c.method === "reactions.add").body, { channel: "C0DEPLOY", timestamp: "1790000004.000100", name: "eyes" });
  assert.deepEqual(w.app.thread({ turn: turnOf(1) }).to, { channel: "C0DEPLOY", thread: "1790000004.000100", ts: "1790000004.000100" });
});

test("its owner's DM is theirs, in the chat's own conversation; their words in a channel are theirs, in its room", async () => {
  const w = await paired(await connected(world()));
  await w.app.intake({ record: dm(PAUL, "hello", "1790000005.000100") }, w.job());
  assert.deepEqual(w.published.at(-1).body, { text: "hello", to: ["npub1juniper"], relayed: RELAYED });
  await w.app.intake({ record: mention(PAUL, "<@U0BOT> status?", "1790000006.000100", { thread_ts: "1790000000.000100" }) }, w.job());
  const rec = w.published.at(-1).body;
  assert.deepEqual([rec.relayed, rec.heard, rec.room], [RELAYED, undefined, { id: "C0DEPLOY", name: "#deploys", thread: "1790000000.000100" }]);
});

test("someone else's DM is heard in a room of its own, with no context", async () => {
  const w = await connected(world());
  await w.app.intake({ record: dm(MAPLE, "hi there", "1790000007.000100") }, w.job());
  const rec = w.published.at(-1).body;
  assert.deepEqual([rec.heard, rec.room, rec.context], [{ via: "slack", from: MAPLE, name: "Maple" }, { id: `D${MAPLE}`, name: "DM" }, undefined]);
});

test("a message is taken once: Slack's retry publishes nothing more", async () => {
  const w = await connected(world());
  await w.app.intake({ record: dm(MAPLE, "once", "1790000008.000100") }, w.job());
  assert.equal((await w.app.intake({ record: dm(MAPLE, "once", "1790000008.000100") }, w.job())).ignored, "taken already");
  assert.equal(w.published.length, 1);
});

test("what is no message to the agent is taken as nothing", async () => {
  const w = await connected(world());
  const bot = { user: BOT };
  assert.equal(taken({ type: "app_mention", user: MAPLE, ts: "1.2", channel: "DXYZ", channel_type: "im", text: "<@U0BOT>" }, bot), null, "a DM's mention (its message comes too)");
  assert.equal(taken({ type: "message", user: MAPLE, ts: "1.2", channel: "C0DEPLOY", channel_type: "channel", text: "no mention" }, bot), null, "a channel's message that mentions no one");
  assert.equal(taken({ type: "message", subtype: "message_changed", user: MAPLE, ts: "1.2", channel: "DXYZ", channel_type: "im" }, bot), null, "an edit");
  assert.equal(taken({ type: "message", bot_id: "B9", user: "U9", ts: "1.2", channel: "DXYZ", channel_type: "im", text: "x" }, bot), null, "a bot's");
  assert.equal(taken({ type: "message", user: BOT, ts: "1.2", channel: "DXYZ", channel_type: "im", text: "x" }, bot), null, "its own");
  assert.equal((await w.app.intake({ record: mention(MAPLE, "<@U0BOT> hi", "1790000009.000100") }, w.job("call"))).ignored, "only its inbox's trigger");
  assert.equal((await w.app.intake({ record: event({ type: "app_mention", user: MAPLE, text: "hi", ts: "1790000009.1", channel: "C1" }, "T0OTHER") }, w.job())).ignored, "another workspace's");
  assert.equal(w.published.length, 0);
});

test("unlinked, nothing is published, and a DM is told so", async () => {
  const w = await connected(world());
  const out = await w.app.intake({ record: dm(MAPLE, "anyone?", "1790000010.000100") }, w.job("channel", null));
  assert.equal(out.ignored, "not linked");
  assert.equal(w.published.length, 0);
  assert.equal(w.calls.find((c) => c.method === "chat.postMessage").body.markdown_text, "This app isn't linked to an agent.");
});

test("pairing: the account that sends the code is proposed, its owner confirms it; two accounts sending one code propose no one", async () => {
  const w = await connected(world());
  const { code } = w.app.pair({}, w.owner);
  assert.match(code, /^[0-9a-f]{16}$/);
  const out = await w.app.intake({ record: dm(PAUL, `pair ${code.toUpperCase()}`, "1790000011.000100") }, w.job());
  assert.deepEqual(out, { proposed: true, conflict: false });
  assert.equal(w.published.length, 0, "a pairing code is never the agent's");
  assert.deepEqual(w.app.state().pairing.candidate, { user: PAUL, name: "Paul" });
  await assert.rejects(async () => w.app.confirm({ user: PAUL }, { agent: "juniper--k3x9" }), /in person/);
  w.app.confirm({ user: PAUL }, w.owner);
  assert.deepEqual(w.app.route().paired, { user: PAUL, dm: `D${PAUL}` });
  const again = w.app.pair({}, w.owner).code;
  await w.app.intake({ record: dm(PAUL, `pair ${again}`, "1790000012.000100") }, w.job());
  const raced = await w.app.intake({ record: dm(MAPLE, `pair ${again}`, "1790000013.000100") }, w.job());
  assert.deepEqual([raced, w.app.state().pairing], [{ proposed: false, conflict: true }, null]);
});

test("a room takes others' messages up to its cap an hour, then says once that the agent rests; its owner's are never counted", async () => {
  const w = await paired(await connected(world()));
  for (let i = 0; i < 30; i += 1) await w.app.intake({ record: mention(MAPLE, `<@U0BOT> q${i}`, `17900001${String(i).padStart(2, "0")}.000100`) }, w.job());
  assert.equal(w.published.length, 30);
  const over = await w.app.intake({ record: mention(MAPLE, "<@U0BOT> one more", "1790000200.000100") }, w.job());
  const more = await w.app.intake({ record: mention(MAPLE, "<@U0BOT> and more", "1790000201.000100") }, w.job());
  assert.deepEqual([over.ignored, more.ignored], ["resting", "resting"]);
  assert.equal(w.calls.filter((c) => c.method === "chat.postMessage" && /resting/.test(c.body.markdown_text)).length, 1);
  await w.app.intake({ record: mention(PAUL, "<@U0BOT> mine", "1790000202.000100") }, w.job());
  assert.equal(w.published.length, 31);
});

test("a reply goes to the thread its turn answers, 👀 off; one whose turn it doesn't know to its room; one with neither to its owner's DM", async () => {
  const w = await paired(await connected(world()));
  await w.app.intake({ record: mention(MAPLE, "<@U0BOT> is it green?", "1790000300.000100") }, w.job());
  w.calls.length = 0;
  const reply = (body, principal = "npub1juniper") => ({ principal, body: { kind: "message", text: "Green.", ...body } });
  assert.deepEqual(await w.app.mirror({ record: reply({ turn: turnOf(1), room: "C0DEPLOY" }) }, w.job()), { sent: 1 });
  assert.deepEqual(w.calls.map((c) => [c.method, c.body.channel, c.body.thread_ts ?? c.body.timestamp]), [["chat.postMessage", "C0DEPLOY", "1790000300.000100"], ["reactions.remove", "C0DEPLOY", "1790000300.000100"]]);
  w.calls.length = 0;
  await w.app.mirror({ record: reply({ turn: "bg0000000000000000000001", room: "C0OTHER" }) }, w.job());
  assert.deepEqual(w.calls.map((c) => [c.method, c.body.channel, c.body.thread_ts]), [["chat.postMessage", "C0OTHER", undefined]]);
  w.calls.length = 0;
  await w.app.mirror({ record: reply({ turn: "bg0000000000000000000002" }) }, w.job());
  assert.deepEqual(w.calls.map((c) => [c.method, c.body.channel]), [["chat.postMessage", `D${PAUL}`]]);
  w.calls.length = 0;
  for (const [record, why] of [
    [reply({ turn: turnOf(1) }, "npub1other"), "not its bound agent's"],
    [reply({ turn: turnOf(1), interim: true }), "not a reply"],
    [{ principal: "npub1juniper", body: { kind: "turn.start", turn: turnOf(1) } }, "not a reply"],
  ]) assert.equal((await w.app.mirror({ record }, w.job())).why, why);
  assert.equal((await w.app.mirror({ record: reply({ turn: turnOf(1) }) }, w.job("channel", null))).why, "not linked");
  assert.equal(w.calls.length, 0);
});

test("a question that asks its owner goes where their answer counts: its turn's thread, or their DM; one asking anyone else goes nowhere", async () => {
  const w = await paired(await connected(world()));
  await w.app.intake({ record: mention(MAPLE, "<@U0BOT> deploy it?", "1790000400.000100") }, w.job());
  w.calls.length = 0;
  const card = (turn, asks = "npub1paul") => ({ channel: "work", principal: "npub1juniper", body: { kind: "turn.prompt", turn, prompt: "p1", text: "Deploy now?", asks, options: [{ id: "yes", label: "Yes" }, { id: "w", label: "Other", words: true }] } });
  assert.deepEqual(await w.app.mirror({ record: card(turnOf(1)) }, w.job()), { sent: 1 });
  assert.deepEqual(w.calls.map((c) => [c.method, c.body.channel, c.body.thread_ts]), [["chat.postMessage", "C0DEPLOY", "1790000400.000100"]], "in the room's thread, its 👀 kept: no answer yet");
  assert.match(w.calls[0].body.markdown_text, /^Deploy now\?\n\n\(Answer with one of: Yes, or in your own words\.\)\n\n\(Answer here by mentioning @juniper\.\)$/);
  w.calls.length = 0;
  await w.app.mirror({ record: card("cafe00000000000000000001") }, w.job());
  assert.deepEqual(w.calls.map((c) => c.body.channel), [`D${PAUL}`]);
  w.calls.length = 0;
  assert.equal((await w.app.mirror({ record: card(turnOf(1), "slack:U0MAPLE") }, w.job())).why, "it asks someone else");
  assert.equal((await w.app.mirror({ record: { channel: "work", principal: "npub1juniper", body: { kind: "turn.step", turn: turnOf(1) } } }, w.job())).why, "not a question");
  assert.equal(w.calls.length, 0);
});

test("a turn a restart cut, continued, answers where it would have: its thread", async () => {
  const w = await paired(await connected(world()));
  await w.app.intake({ record: mention(MAPLE, "<@U0BOT> long job", "1790000500.000100") }, w.job());
  const [next, after] = await continuations(turnOf(1));
  assert.equal(next, createHash("sha256").update(`continue\0${turnOf(1)}`).digest("hex").slice(0, 24));
  for (const turn of [next, after]) assert.deepEqual(w.app.thread({ turn }).to, { channel: "C0DEPLOY", thread: "1790000500.000100", ts: "1790000500.000100" });
});

test("a question in a channel's thread says to answer by mentioning the bot, and that answer comes in as its words alone", async () => {
  const w = await paired(await connected(world()));
  await w.app.intake({ record: mention(MAPLE, "<@U0BOT> deploy it?", "1790000600.000100") }, w.job());
  w.calls.length = 0;
  await w.app.mirror({ record: { channel: "work", principal: "npub1juniper", body: { kind: "turn.prompt", turn: turnOf(1), prompt: "p", text: "Sure?", asks: "npub1paul", options: [{ id: "w", label: "Other", words: true }] } } }, w.job());
  assert.match(w.calls[0].body.markdown_text, /\(Answer here by mentioning @juniper\.\)$/);
  await w.app.intake({ record: mention(PAUL, "<@U0BOT>: yes, go", "1790000601.000100", { thread_ts: "1790000600.000100" }) }, w.job());
  assert.equal(w.published.at(-1).body.text, "yes, go");
  await w.app.intake({ record: mention(PAUL, "ask <@U0BOT> later", "1790000602.000100") }, w.job());
  assert.equal(w.published.at(-1).body.text, "ask @juniper later", "a mention past its start is its words");
});

test("what a message can do without (👀, its room's context, names) failing outright publishes it all the same", async () => {
  const w = await connected(world({ failing: ["reactions.add", "conversations.history", "conversations.info", "users.info"] }));
  const out = await w.app.intake({ record: mention(MAPLE, "<@U0BOT> still there?", "1790000700.000100") }, w.job());
  assert.equal(out.heard, true);
  assert.deepEqual(w.published.at(-1).body, { text: "still there?", to: ["npub1juniper"], heard: { via: "slack", from: MAPLE, name: "someone" }, room: { id: "C0DEPLOY", name: "#C0DEPLOY", thread: "1790000700.000100" } });
});

test("a thread's context is read up to its mention, its newest lines kept", async () => {
  const replies = Array.from({ length: 30 }, (_, i) => ({ ts: `17900008${String(i).padStart(2, "0")}.000100`, user: "U0SAM", text: `line ${i}` }));
  const w = await connected(world({ replies }));
  await w.app.intake({ record: mention(MAPLE, "<@U0BOT> sum up", "1790000899.000100", { thread_ts: "1790000800.000100" }) }, w.job());
  const read = w.calls.find((c) => c.method === "conversations.replies").body;
  assert.deepEqual([read.latest, read.inclusive], ["1790000899.000100", "false"]);
  const context = w.published.at(-1).body.context;
  assert.deepEqual([context.length, context[0].text, context.at(-1).text], [20, "line 10", "line 29"]);
});

test("a long reply goes in at most 4 messages, the rest in the chat", async () => {
  const w = await paired(await connected(world()));
  const out = await w.app.mirror({ record: { principal: "npub1juniper", body: { kind: "message", turn: "t", text: "x".repeat(60_000) } } }, w.job());
  assert.equal(out.sent, 4);
  const sent = w.calls.filter((c) => c.method === "chat.postMessage").map((c) => c.body.markdown_text);
  assert.ok(sent.every((t) => t.length <= 12_000) && sent[3].endsWith("(The rest is in its owner's chat.)"));
});

test("another workspace's token forgets the account paired and what it remembered", async () => {
  const w = await paired(await connected(world()));
  w.app.known({ bot: { team: "T0NEW", teamName: "New", user: "U0B2", name: "juniper" }, api: "https://slack.example" }, w.owner);
  assert.deepEqual([w.app.state().paired, w.app.state().verified], [null, false]);
});

test("Slack's text, as a person reads it", () => {
  const names = new Map([["U0MAPLE", "Maple"], ["C0DEPLOY", "deploys"]]);
  assert.equal(readable("hi <@U0MAPLE>, see <#C0DEPLOY> and <#C0X|general>", names), "hi @Maple, see #deploys and #general");
  assert.equal(readable("<https://x.example|the doc> or <https://y.example> &lt;b&gt; &amp;", names), "the doc (https://x.example) or https://y.example <b> &");
  assert.equal(readable("<!here> <!subteam^S1|@devs> <@U0NOBODY>", names), "@here @devs @someone");
});
