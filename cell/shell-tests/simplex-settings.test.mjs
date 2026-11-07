// cell/shell/connections.js `createSimplex`: Settings' SimpleX row, one per
// agent, as Telegram's is (FIN-393): Set up opens its computer's link page
// in the connector setup dialog (connector-setup.js), at its taller height
// on a wide screen, the frame the page's messages are taken from; Open,
// once the list holds its SimpleX chat, goes to that chat. A Set up whose
// ticket comes late never closes the dialog of a Set up pressed after it
// (#1104's review). Run by `cargo xtask check` on the pinned Node
// (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { createSimplex } from "../shell/connections.js";
import { createConnectorSetup } from "../shell/connector-setup.js";

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
  setAttribute() {}
  addEventListener(type, fn) {
    if (type === "close") this.onclose = fn;
  }
  // a dialog, as the connector setup opens and closes one
  showModal() {
    this.open = true;
  }
  close() {
    this.open = false;
    this.onclose?.();
  }
  remove() {
    this.removed = true;
  }
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

// `deferred`: each port ticket answered only when the test says (`answer`)
function world({ deferred = false } = {}) {
  const asked = [];
  const went = [];
  const notices = [];
  const dialogs = [];
  const waiting = new Map();
  const api = (method, path, body) => {
    asked.push([method, path, body]);
    if (method !== "POST" || path !== "/api/computers/c-robin/ports/6080/ticket") throw new Error(`unexpected ${method} ${path}`);
    const ticket = { url: `https://c-robin-6080.example/p/6080${body.path}&t=1` };
    if (!deferred) return Promise.resolve(ticket);
    return new Promise((done) => waiting.set(body.path.split("agent=")[1], () => done(ticket)));
  };
  const answer = async (agent) => {
    waiting.get(agent)();
    await new Promise((r) => setTimeout(r, 0));
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
  const el = (tag, cls, text) => new Node(tag, cls, text);
  const named = (node, f) => Object.assign(node, { textContent: node.textContent || f });
  const frameOf = (src, title) => Object.assign(new Node("iframe"), { src, title, dataset: { src } });
  globalThis.addEventListener = () => {};
  globalThis.removeEventListener = () => {};
  globalThis.document = { querySelector: () => null, querySelectorAll: () => [], body: { append: (d) => dialogs.push(d) } };
  const setup = createConnectorSetup({ el, named, frameOf });
  const simplex = createSimplex({
    api, el, section, actionLine, named, seg: (s) => s,
    computer: () => ({ computer: "c-robin", simplex: true, alwaysOn: true, agents: AGENTS }), getSeat: () => null, titleOf: (a) => a.split("--")[0],
    frameOf, pressSetup: setup.pressSetup, openSetup: setup.openSetup, simplexChatOf: (a) => CHATS[a] ?? null, openChat: (name) => went.push(name),
    notice: (...n) => notices.push(n), setHome: async () => null,
  });
  // the dialogs open now: their title, the agent they name, their frame
  const shown = () => dialogs.filter((d) => d.open).map((d) => [d.children[0].textContent, d.children[1].textContent, d.children[2].className, d.children[2].dataset.simplex ?? d.children[2].src]);
  const rows = () => {
    const s = simplex.simplexSection();
    return s.children.filter((c) => c.className.includes("settings-line"));
  };
  return { simplex, setup, asked, went, notices, answer, shown, rows };
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
  await buttons(w.rows()[1])[0].onclick();
  assert.deepEqual(w.asked, [["POST", "/api/computers/c-robin/ports/6080/ticket", { path: "/simplex/?agent=wren--c3d4" }]]);
  assert.deepEqual(w.shown(), [["SimpleX", "wren--c3d4", "connector-setup-frame tall", "wren--c3d4"]]);
  assert.deepEqual(w.notices, []);
});

test("a Set up whose ticket comes after a later Set up's never closes that one's dialog", async () => {
  const w = world({ deferred: true });
  const [maple, wren] = w.rows();
  const first = buttons(maple)[1].onclick();
  const second = buttons(wren)[0].onclick();
  await w.answer("wren--c3d4");
  await w.answer("maple--a1b2");
  await Promise.all([first, second]);
  assert.deepEqual(w.shown(), [["SimpleX", "wren--c3d4", "connector-setup-frame tall", "wren--c3d4"]], "the later press's dialog, its invitation kept");
});

test("nor does a SimpleX ticket that comes after a Telegram Set up pressed since", async () => {
  const w = world({ deferred: true });
  const simplex = buttons(w.rows()[0])[1].onclick();
  // Telegram's Set up, as createTelegram's presses and then opens
  const pressed = w.setup.pressSetup();
  w.setup.openSetup({ pressed, src: "/auth/frame?name=maple-telegram--g7h8&return=/setup.html", name: "maple-telegram--g7h8", agent: "maple--a1b2", title: "Telegram" });
  await w.answer("maple--a1b2");
  await simplex;
  assert.deepEqual(w.shown(), [["Telegram", "maple--a1b2", "connector-setup-frame", "/auth/frame?name=maple-telegram--g7h8&return=/setup.html"]]);
});
