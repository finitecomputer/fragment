// cell/shell/connections.js `createAgentHomes`: an agent's home, read from
// its agent.json while unknown. Reads can overlap: a list's refresh reads
// it (`loadHomes`), and Try again under a read that failed reads it afresh
// whatever read is out. Only the newest read's answer applies, success or
// failure, so a slow older read never puts back a home another device has
// since changed, nor its error line (#1121's review). Its SimpleX home is
// its computer's SimpleX chat, else the chat bound as its SimpleX door, as
// its bridge picks it. Run by `cargo xtask check` on the pinned Node
// (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { createAgentHomes } from "../shell/connections.js";

// a node as the pick and its line draw it
class Node {
  constructor(tag, cls, text) {
    Object.assign(this, { tag, className: cls ?? "", textContent: text ?? "", children: [], dataset: {}, hidden: false, value: "" });
  }
  append(...c) {
    this.children.push(...c);
  }
  replaceChildren(...c) {
    this.children = [...c];
  }
  setAttribute() {}
}

const AGENT = { fragment: "maple--a1b2", identity: "npub1maple" };

// Robin's one agent, its home pick drawn in Settings; each read of its
// agent.json answered only when the test says (`reads[i].ok(json)` or
// `.fail(status)`), then the page's work on that answer let run;
// `chats`, the person's list; `bound`, each connector's binding by its
// name; `failing`, chats whose binding's read fails; `clock`, the time
function world({ chats = [], bound = {}, failing = new Set(), clock = { t: 1_000_000 } } = {}) {
  const reads = [];
  const doorReads = [];
  const picks = [];
  globalThis.document = { querySelectorAll: (sel) => (sel === "select[data-home-for]" ? picks : []) };
  const homes = createAgentHomes({
    el: (tag, cls, text) => new Node(tag, cls, text),
    icon: () => new Node("svg"),
    named: (n) => n,
    actionLine: (label, ...actions) => {
      const row = new Node("div", "settings-line");
      row.append(label, ...actions);
      return row;
    },
    settingLabel: (label, control) => {
      const l = new Node("label", "agent-model-label");
      l.append(new Node("span", "settings-key", label), control);
      return l;
    },
    modelOption: (value, text) => Object.assign(new Node("option", null, text), { value }),
    savePick: async () => {},
    agents: () => new Map([[AGENT.identity, AGENT]]),
    chats: () => chats,
    own: () => true,
    labelOf: (name) => name.split("--")[0],
    byName: () => ({}),
    titleOf: () => "Maple",
    agentsOwnChat: () => ({ name: "maple-chat--g7h8" }),
    agentJson: () =>
      new Promise((done, refuse) => {
        const settle = async (f) => {
          f();
          await new Promise((r) => setTimeout(r, 0));
        };
        reads.push({ ok: (j) => settle(() => done(j)), fail: (status) => settle(() => refuse(Object.assign(new Error("refused"), { status }))) });
      }),
    writeAgentJson: async () => {},
    renderChats: () => {},
    readDoor: async (name) => {
      doorReads.push(name);
      if (failing.has(name)) throw Object.assign(new Error("refused"), { status: 503 });
      return bound[name] ?? null;
    },
    now: () => clock.t,
  });
  const [label, line] = homes.homePicker(AGENT, new Node("span"));
  const pick = label.children[1];
  picks.push(pick);
  const again = line.children[1];
  return { homes, reads, pick, line, again, doorReads };
}

test("a refresh's read that answers after Try again's newer one is let go: the newer home stays", async () => {
  const { homes, reads, pick, line, again } = world();
  await reads[0].fail(503);
  assert.equal(line.hidden, false, "its first read failed: said, with Try again");
  // a list's refresh reads it (its old home, its own chat), slow
  homes.loadHomes();
  assert.equal(reads.length, 2);
  assert.equal(line.hidden, false, "the line stays while that read is out");
  // another device picks SimpleX; Try again reads afresh and answers first
  again.onclick();
  assert.equal(reads.length, 3, "Try again reads afresh, a read already out");
  await reads[2].ok({ home: "simplex" });
  assert.deepEqual([pick.value, pick.disabled, line.hidden], ["simplex", false, true]);
  // the refresh's old answer, then
  await reads[1].ok({});
  assert.deepEqual([pick.value, line.hidden], ["simplex", true], "an older read's answer never undoes a newer one's");
});

test("an older read's failure after a newer read's answer never brings the line back", async () => {
  const { homes, reads, pick, line, again } = world();
  await reads[0].fail(503);
  homes.loadHomes();
  again.onclick();
  await reads[2].ok({});
  await reads[1].fail(503);
  assert.deepEqual([pick.value, pick.disabled, line.hidden], ["", false, true]);
});

test("an older read's answer after a newer read's failure is let go too: the newest says", async () => {
  const { homes, reads, pick, line, again } = world();
  await reads[0].fail(503);
  homes.loadHomes();
  again.onclick();
  await reads[2].fail(503);
  assert.equal(line.hidden, false);
  await reads[1].ok({ home: "simplex" });
  assert.deepEqual([pick.disabled, line.hidden], [true, false], "the newest read failed: still unknown, said, with Try again");
  // and Try again again reads it
  again.onclick();
  await reads[3].ok({ home: "simplex" });
  assert.deepEqual([pick.value, line.hidden], ["simplex", true]);
});

// its SimpleX a door (a connector set up from Settings): found as its
// bridge finds it (images/bridge api.rs `door_candidates`): its owner's
// chats it is in labelled as SimpleX chats, its own label's first, then by
// name, at most 8, the first bound as its SimpleX door
const settle = async () => {
  for (let i = 0; i < 6; i += 1) await new Promise((r) => setTimeout(r, 0));
};
const chat = (name) => ({ name, kind: "chat", owner: "npub1robin", agents: [AGENT.identity] });
const ITS = { door: "simplex", agent: AGENT.fragment };

test("an agent whose SimpleX is a door is offered SimpleX, and its bell is on that door's chat", async () => {
  const door = chat("maple-simplex-2--q4w5");
  const w = world({ chats: [door], bound: { [door.name]: ITS } });
  await w.reads[0].ok({ home: "simplex" });
  await settle();
  assert.deepEqual([w.pick.children.map((o) => o.value), w.pick.value], [["", "simplex"], "simplex"]);
  assert.deepEqual([w.homes.homeMark(door).length, w.homes.homeMark({ name: "maple-chat--g7h8" }).length], [1, 0], "its bell on the door's chat");
  // its home its own chat still: SimpleX offered to pick
  const offered = world({ chats: [door], bound: { [door.name]: ITS } });
  await offered.reads[0].ok({});
  await settle();
  assert.deepEqual([offered.pick.children.map((o) => o.value), offered.pick.value], [["", "simplex"], ""]);
});

test("its door over a SimpleX chat of its label kept from before; its own label's first, then by name", async () => {
  const kept = chat("maple-simplex--r1r1");
  const door = chat("maple-simplex-2--q4w5");
  const w = world({ chats: [door, kept], bound: { [door.name]: ITS } });
  await w.reads[0].ok({ home: "simplex" });
  await settle();
  assert.deepEqual([w.homes.homeMark(door).length, w.homes.homeMark(kept).length], [1, 0]);
  assert.deepEqual(w.doorReads, ["maple-simplex--r1r1", "maple-simplex-2--q4w5"], "read in its bridge's order, until one is its door");
  // two doors bound: its own label's first, as its bridge picks
  const alpha = chat("alpha-simplex--g7g7");
  const mine = chat("maple-simplex-3--h8h8");
  const two = world({ chats: [alpha, mine], bound: { [alpha.name]: ITS, [mine.name]: ITS } });
  await two.reads[0].ok({ home: "simplex" });
  await settle();
  assert.deepEqual([two.homes.homeMark(mine).length, two.homes.homeMark(alpha).length], [1, 0]);
});

test("a door its bridge reads no further than finds none: not offered", async () => {
  // labelled as no SimpleX chat
  const odd = chat("maple-phone--s2s2");
  const w = world({ chats: [odd], bound: { [odd.name]: ITS } });
  await w.reads[0].ok({});
  await settle();
  assert.deepEqual(w.pick.children.map((o) => o.value), [""]);
  assert.deepEqual(w.doorReads, [], "none read");
  // past the first 8 of its SimpleX chats
  const many = Array.from({ length: 9 }, (_, i) => chat(`old-${i}-simplex--a${i}a${i}`));
  const far = world({ chats: many, bound: { [many[8].name]: ITS } });
  await far.reads[0].ok({});
  await settle();
  assert.deepEqual([far.pick.children.map((o) => o.value), far.doorReads.length], [[""], 8]);
});

test("a binding's read that fails is passed over for the next; with none found, looked for again a while after", async () => {
  const kept = chat("maple-simplex--r1r1");
  const door = chat("maple-simplex-2--q4w5");
  const w = world({ chats: [kept, door], bound: { [door.name]: ITS }, failing: new Set([kept.name]) });
  await w.reads[0].ok({ home: "simplex" });
  await settle();
  assert.equal(w.homes.homeMark(door).length, 1, "the door found past a read that failed");
  // its only candidate failing: not found, and not looked for again at once
  const clock = { t: 1_000_000 };
  const failing = new Set([door.name]);
  const v = world({ chats: [door], bound: { [door.name]: ITS }, failing, clock });
  await v.reads[0].ok({ home: "simplex" });
  await settle();
  assert.deepEqual([v.homes.homeMark(door).length, v.doorReads.length], [0, 1]);
  v.homes.homeMark(door);
  await settle();
  assert.equal(v.doorReads.length, 1, "not again at once");
  failing.clear();
  clock.t += 30_000;
  v.homes.homeMark(door);
  await settle();
  assert.deepEqual([v.homes.homeMark(door).length, v.doorReads.length], [1, 2], "a while after, as the list draws again");
});
