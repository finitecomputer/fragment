// cell/shell/connections.js `createSimplex`: Settings' SimpleX row, one per
// agent, as Telegram's is (FIN-393): Set up opens its computer's link page
// in the connector setup dialog (connector-setup.js), at its taller height
// on a wide screen, the frame the page's messages are taken from; Open,
// once the list holds its SimpleX chat, goes to that chat. Run by `cargo
// xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { createSimplex } from "../shell/connections.js";

// a node as the section draws it: its tag, text, children and data
class Node {
  constructor(tag, cls, text) {
    Object.assign(this, { tag, className: cls ?? "", textContent: text ?? "", children: [], dataset: {}, hidden: false });
  }
  append(...c) {
    this.children.push(...c);
  }
  replaceChildren(...c) {
    this.children = [...c];
  }
  addEventListener() {}
  querySelectorAll(sel) {
    return sel === ":scope > h2" ? this.children.filter((c) => c.tag === "h2") : [];
  }
}
const all = (n) => [n, ...n.children.flatMap(all)];
const buttons = (row) => all(row).filter((n) => n.tag === "button");

// Robin's always-on computer, which does SimpleX: two agents of theirs,
// Maple's SimpleX chat in their list
const AGENTS = [{ fragment: "maple--a1b2", identity: "npub1maple" }, { fragment: "wren--c3d4", identity: "npub1wren" }];
const CHATS = { "maple--a1b2": { name: "maple-simplex--e5f6" } };

function world() {
  const asked = [];
  const opened = [];
  const went = [];
  const notices = [];
  const api = async (method, path, body) => {
    asked.push([method, path, body]);
    if (method === "POST" && path === "/api/computers/c-robin/ports/6080/ticket") return { url: `https://c-robin-6080.example/p/6080${body.path}&t=1` };
    throw new Error(`unexpected ${method} ${path}`);
  };
  const section = (title) => {
    const s = new Node("section", "settings-section");
    s.append(new Node("h2", null, title));
    return s;
  };
  const actionLine = (label, ...actions) => {
    const row = new Node("div", "settings-line settings-action-line");
    row.append(label, ...actions);
    return row;
  };
  const frameOf = (src, title) => Object.assign(new Node("iframe"), { src, title, dataset: { src } });
  globalThis.addEventListener = () => {};
  globalThis.removeEventListener = () => {};
  globalThis.document = { querySelector: () => null, querySelectorAll: () => [] };
  const simplex = createSimplex({
    api, el: (tag, cls, text) => new Node(tag, cls, text), section, actionLine, named: (node, f) => Object.assign(node, { textContent: node.textContent || f }), seg: (s) => s,
    computer: () => ({ computer: "c-robin", simplex: true, alwaysOn: true, agents: AGENTS }), getSeat: () => null, titleOf: (a) => a.split("--")[0],
    frameOf, openSetup: (o) => opened.push(o), simplexChatOf: (a) => CHATS[a] ?? null, openChat: (name) => went.push(name),
    notice: (...n) => notices.push(n), setHome: async () => null,
  });
  return { simplex, asked, opened, went, notices };
}

test("each agent's row is Set up, and Open once the list holds its SimpleX chat, which Open goes to", () => {
  const w = world();
  const s = w.simplex.simplexSection();
  const rows = s.children.filter((c) => c.className.includes("settings-line"));
  assert.deepEqual(rows.map((r) => buttons(r).map((b) => b.textContent)), [["Open", "Set up"], ["Set up"]]);
  assert.equal(s.children.find((c) => c.tag === "p").textContent, "Message your agents from SimpleX Chat.");
  buttons(rows[0])[0].onclick();
  assert.deepEqual(w.went, ["maple-simplex--e5f6"]);
});

test("Set up opens its computer's link page, through a port ticket, in the connector setup dialog at its taller height", async () => {
  const w = world();
  const s = w.simplex.simplexSection();
  const wren = s.children.filter((c) => c.className.includes("settings-line"))[1];
  await buttons(wren)[0].onclick();
  assert.deepEqual(w.asked, [["POST", "/api/computers/c-robin/ports/6080/ticket", { path: "/simplex/?agent=wren--c3d4" }]]);
  assert.equal(w.opened.length, 1);
  const [o] = w.opened;
  assert.deepEqual([o.title, o.agent, o.tall, o.frame.dataset.simplex, o.frame.src], ["SimpleX", "wren--c3d4", true, "wren--c3d4", "https://c-robin-6080.example/p/6080/simplex/?agent=wren--c3d4&t=1"]);
  assert.deepEqual(w.notices, []);
});
