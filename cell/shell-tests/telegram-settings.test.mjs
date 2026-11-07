// cell/shell/connections.js `createTelegram`: Settings' Telegram row, one
// per agent. Which chat is whose is read, never guessed: each of the
// person's own chats with reach, its door as its owner reads it; a read
// that fails, or a list not read, shows no Set up (which would make a
// second chat beside one not read). Set up opens a bound chat's setup page;
// with none, it makes one, binds it as the agent's door, and opens its; a
// binding refused deletes the one it made. Run by `cargo xtask check` on
// the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { createTelegram } from "../shell/connections.js";

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
  querySelectorAll(sel) {
    return sel === ":scope > h2" ? this.children.filter((c) => c.tag === "h2") : [];
  }
}
const all = (n) => [n, ...n.children.flatMap(all)];
const buttons = (row) => all(row).filter((n) => n.tag === "button");

// Robin's computer: two agents of theirs; their chats as the list says
const AGENTS = [{ fragment: "maple--a1b2", identity: "npub1maple" }, { fragment: "wren--c3d4", identity: "npub1wren" }];
const LIST = [
  { name: "maple-telegram--e5f6", kind: "chat", role: "owner", sharing: { reach: true } },
  { name: "maple-chat--g7h8", kind: "chat", role: "owner" },
  { name: "keyed-chat--j9k2", kind: "chat", role: "owner", sharing: { reach: true } },
  { name: "sams-telegram--m3n4", kind: "chat", role: "viewer", sharing: { reach: true } },
];
const DOORS = { "maple-telegram--e5f6": { door: "telegram", agent: "maple--a1b2" }, "keyed-chat--j9k2": null };

function world({ doors = { ...DOORS }, refuse = null, listed = true, link = (name, body) => (doors[name] = { door: "telegram", agent: body.agent }) } = {}) {
  const asked = [];
  const opened = [];
  const notices = [];
  let made = 0;
  let loads = 0;
  const api = async (method, path, body) => {
    asked.push(`${method} ${path}`);
    const door = path.match(/^\/api\/f\/([^/]+)\/door$/)?.[1];
    if (method === "GET" && door) {
      if (refuse === door) throw Object.assign(new Error("refused"), { status: 500 });
      if (!(door in doors)) throw Object.assign(new Error("not found"), { status: 404 });
      return { bound: doors[door] };
    }
    if (method === "POST" && path === "/api/fragments") return { name: `${body.label}--n${++made}p4` };
    if (method === "POST" && door) return link(door, body);
    if (method === "DELETE") return {};
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
  const telegram = createTelegram({
    api, el: (tag, cls, text) => new Node(tag, cls, text), section, actionLine, named: (node, f) => Object.assign(node, { textContent: f }), seg: (s) => s,
    computer: () => ({ agents: AGENTS }), fragments: () => LIST, listed: () => listed, own: (f) => f.role === "owner",
    connectorLabel: (agent, door) => `${agent.split("--")[0]}-${door}`, markOffer: (node, agent, what) => Object.assign(node, { offer: [agent, what] }),
    framed: (name, path) => `/auth/frame?name=${name}&return=${path}`, openSetup: (o) => opened.push(o), openChat: () => {},
    notice: (...n) => notices.push(n), load: async () => { loads++; },
  });
  let drawn = null;
  globalThis.document = { getElementById: (id) => (id === "settings-telegram" ? drawn : null), querySelector: () => null };
  const draw = () => (drawn = telegram.telegramSection());
  const rows = () => drawn.children.filter((c) => c.className.includes("settings-line"));
  const said = () => drawn.children.find((c) => c.tag === "p")?.textContent;
  return { telegram, asked, opened, notices, draw, rows, said, loads: () => loads };
}

test("its own chats with reach are read, and an agent's bound one is Open and Set up; another's Set up alone", async () => {
  const w = world();
  await w.telegram.readTelegram();
  w.draw();
  assert.deepEqual(w.asked.sort(), ["GET /api/f/keyed-chat--j9k2/door", "GET /api/f/maple-telegram--e5f6/door"], "no other's, and none without reach");
  const [maple, wren] = w.rows();
  assert.deepEqual(buttons(maple).map((b) => b.textContent), ["Open", "Set up"]);
  assert.equal(buttons(maple)[0].dataset.telegramOpen, "maple-telegram--e5f6");
  assert.deepEqual(buttons(wren).map((b) => b.textContent), ["Set up"]);
  assert.deepEqual(maple.offer, ["maple--a1b2", "doors"], "shown only for a runtime with doors");
  assert.equal(w.said(), "Message your agents from Telegram.");
});

test("a door read that fails, or a list not read, shows no Set up: Try again", async () => {
  for (const w of [world({ refuse: "keyed-chat--j9k2" }), world({ listed: false })]) {
    await w.telegram.readTelegram();
    w.draw();
    assert.deepEqual(w.rows().map((r) => buttons(r).map((b) => b.textContent)), [["Try again"], ["Try again"]]);
    assert.equal(w.said(), "Couldn't read your Telegram chats.");
  }
});

test("Set up opens a bound chat's setup page, making nothing", async () => {
  const w = world();
  await w.telegram.readTelegram();
  w.draw();
  await buttons(w.rows()[0]).at(-1).onclick();
  assert.equal(w.opened.length, 1);
  assert.deepEqual({ ...w.opened[0], onClose: undefined }, { src: "/auth/frame?name=maple-telegram--e5f6&return=/setup.html", name: "maple-telegram--e5f6", agent: "maple--a1b2", title: "Telegram", onClose: undefined });
  assert.ok(!w.asked.some((a) => a.startsWith("POST")));
});

test("with none, Set up makes one, binds it as the agent's door, and opens its setup page, the list read first", async () => {
  const w = world();
  await w.telegram.readTelegram();
  w.draw();
  await buttons(w.rows()[1]).at(-1).onclick();
  assert.deepEqual(w.asked.filter((a) => !a.startsWith("GET")), ["POST /api/fragments", "POST /api/f/wren-telegram--n1p4/door"]);
  assert.equal(w.loads(), 1);
  assert.deepEqual(w.opened.map((o) => [o.name, o.agent]), [["wren-telegram--n1p4", "wren--c3d4"]]);
  assert.deepEqual(buttons(w.rows()[1]).map((b) => b.textContent), ["Open", "Set up"], "drawn bound at once");
  // read again, the list not yet knowing it has reach: read all the same
  await w.telegram.readTelegram();
  w.draw();
  assert.ok(w.asked.includes("GET /api/f/wren-telegram--n1p4/door"));
  assert.deepEqual(buttons(w.rows()[1]).map((b) => b.textContent), ["Open", "Set up"]);
});

test("a binding refused deletes the chat it made, and says so; nothing opens", async () => {
  const w = world({ link: () => { throw Object.assign(new Error("telegram is not allowed"), { status: 403 }); } });
  await w.telegram.readTelegram();
  w.draw();
  await buttons(w.rows()[1]).at(-1).onclick();
  assert.deepEqual(w.asked.filter((a) => !a.startsWith("GET")), ["POST /api/fragments", "POST /api/f/wren-telegram--n1p4/door", "DELETE /api/f/wren-telegram--n1p4"]);
  assert.deepEqual(w.notices, [["Telegram was not set up", "telegram is not allowed"]]);
  assert.equal(w.opened.length, 0);
  assert.deepEqual(buttons(w.rows()[1]).map((b) => b.textContent), ["Set up"]);
});
