// cell/shell/connections.js `createAgentHomes`: an agent's home, read from
// its agent.json while unknown. Reads can overlap: a list's refresh reads
// it (`loadHomes`), and Try again under a read that failed reads it afresh
// whatever read is out. Only the newest read's answer applies, success or
// failure, so a slow older read never puts back a home another device has
// since changed, nor its error line (#1121's review). Run by
// `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

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
// `.fail(status)`), then the page's work on that answer let run
function world() {
  const reads = [];
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
    chats: () => [],
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
  });
  const [label, line] = homes.homePicker(AGENT, new Node("span"));
  const pick = label.children[1];
  picks.push(pick);
  const again = line.children[1];
  return { homes, reads, pick, line, again };
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
