// cell/shell/threads.js against a world of its own: two agents of one
// person, the chat the first asks from and their pair chat, each row's
// listed records published only when the test says, as the list coalesces
// them (docs/api.md, Listed records: each poster's newest record, sent at
// most once in 3 s), and the public API's newest records read from it.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test, mock } from "node:test";
import assert from "node:assert/strict";
import { createThreads, pairLabel } from "../shell/threads.js";

const ANN = { identity: "npub1ann", fragment: "ann--abcd" };
const BOB = { identity: "npub1bob", fragment: "bob--efgh" };
const ASKING = "ann-chat--wxyz";
const PAIR = "ann-bob--k2m3";

function world() {
  const records = new Map();
  const rows = new Map([
    [ASKING, { name: ASKING, role: "owner", kind: "chat", agents: [ANN.identity] }],
    [PAIR, { name: PAIR, role: "owner", kind: "chat", agents: [ANN.identity, BOB.identity] }],
  ]);
  let now = 1_000;
  const post = (name, channel, principal, body) => {
    const key = `${name}/${channel}`;
    const list = records.get(key) ?? [];
    records.set(key, list);
    const r = { seq: list.length + 1, at: (now += 1_000), principal, body };
    list.push(r);
    return r;
  };
  // A row as the list sends it now: each member's newest `work` record,
  // and when its newest message was said.
  const publish = (name) => {
    const work = records.get(`${name}/work`) ?? [];
    const newest = new Map(work.map((r) => [r.principal, r]));
    const said = records.get(`${name}/chat`) ?? [];
    Object.assign(rows.get(name), {
      listed: [...newest.values()].reverse().map((r) => ({ channel: "work", principal: r.principal, kind: r.body.kind, at: r.at })),
      previewAt: said.at(-1)?.at,
    });
  };
  const api = async (method, path) => {
    const m = path.match(/^\/api\/f\/([^/]+)\/channels\/([a-z]+)\?before=\d+&limit=(\d+)$/);
    assert.ok(method === "GET" && m, `${method} ${path}`);
    return { records: [...(records.get(`${m[1]}/${m[2]}`) ?? [])].reverse().slice(0, Number(m[3])) };
  };
  const el = (tag, cls = "", text = "") => ({ tag, className: cls, textContent: text, dataset: {}, children: [], append(...c) { this.children.push(...c); } });
  const strip = el("div");
  const threads = createThreads({
    $: (id) => (assert.equal(id, "threads"), strip),
    el,
    patch: (box, lines) => { box.children = lines; },
    api,
    seg: (s) => s,
    avatar: () => el("span"),
    data: {
      byName: (name) => rows.get(name),
      own: (f) => f.role === "owner",
      chats: () => [...rows.values()],
      titleOf: (name) => name,
      labelOf: (name) => name.replace(/--[a-km-np-z2-9]{4}$/, ""),
      agentFace: () => ({}),
      ownAgents: () => [ANN, BOB],
      current: () => ASKING,
      page: () => null,
    },
    navigation: { openChat() {}, leaveSidebar() {} },
    rendered: () => {},
  });
  // The asking chat's lines, as their states.
  const lines = async () => {
    await threads.refresh();
    threads.render();
    return strip.hidden ? [] : strip.children.map((line) => line.dataset.state);
  };
  // The first agent asks the second in their pair chat; the second's turn
  // of it; the first's answer turn in the asking chat.
  const ask = (text) => post(PAIR, "chat", ANN.identity, { text, to: [BOB.identity], return_to: ASKING });
  const work = (name, who, kind, turn, more = {}) => post(name, "work", who.identity, { kind, turn, ...more });
  const answer = (turn) => {
    const a = post(ANN.fragment, "tasks", ANN.identity, { kind: "answer", turn, text: "found it" });
    work(ASKING, ANN, "turn.start", `answer-${turn}`, { cause: { fragment: ANN.fragment, channel: "tasks", seq: a.seq } });
    work(ASKING, ANN, "turn.end", `answer-${turn}`, { outcome: "idle" });
  };
  return { publish, lines, ask, work, answer };
}

test("the pair chat's label is the CLI's (cli/src/ask.rs pair_label)", () => {
  assert.equal(pairLabel("bob", "ann"), "ann-bob");
  assert.equal(pairLabel("a".repeat(30), "b".repeat(30)), `${"a".repeat(19)}-${"b".repeat(19)}`);
});

// Alex's first case (#779): the question read before its turn started,
// and the row's next send already past the start.
for (const [kind, state] of [["turn.step", "working"], ["turn.prompt", "you"]]) {
  test(`a turn that started unseen, its row already at ${kind}, says ${state}`, async () => {
    mock.timers.enable({ apis: ["setTimeout"] });
    try {
      const w = world();
      const q = w.ask("look it up");
      w.publish(PAIR);
      w.publish(ASKING);
      assert.deepEqual(await w.lines(), ["asked"]);
      w.work(PAIR, BOB, "turn.start", "t1", { cause: { channel: "chat", seq: q.seq } });
      w.work(PAIR, BOB, kind, "t1");
      w.publish(PAIR);
      assert.deepEqual(await w.lines(), [state]);
    } finally {
      mock.timers.reset();
    }
  });
}

// Alex's second case (#779): two asks returning to one chat, the asking
// chat's row sent with both answers in before the pair's row says the
// second ended.
test("an answer that came before its thread's end was read still takes its line away", async () => {
  mock.timers.enable({ apis: ["setTimeout"] });
  try {
    const w = world();
    const q1 = w.ask("first");
    w.work(PAIR, BOB, "turn.start", "t1", { cause: { channel: "chat", seq: q1.seq } });
    w.work(PAIR, BOB, "turn.end", "t1", { outcome: "idle" });
    const q2 = w.ask("second");
    w.work(PAIR, BOB, "turn.start", "t2", { cause: { channel: "chat", seq: q2.seq } });
    w.publish(PAIR);
    w.publish(ASKING);
    assert.deepEqual(await w.lines(), ["done", "working"]);
    w.answer("t1");
    w.work(PAIR, BOB, "turn.end", "t2", { outcome: "idle" });
    w.answer("t2");
    w.publish(ASKING);
    assert.deepEqual(await w.lines(), ["working"], "the first's answer is in; the pair's row has not said the second ended");
    w.publish(PAIR);
    assert.deepEqual(await w.lines(), [], "the second's end read, its answer is found");
    assert.deepEqual(await w.lines(), [], "and stays found");
  } finally {
    mock.timers.reset();
  }
});
