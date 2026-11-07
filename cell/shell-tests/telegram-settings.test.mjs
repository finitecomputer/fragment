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
const listOf = () => [
  { name: "maple-telegram--e5f6", kind: "chat", role: "owner", sharing: { reach: true } },
  { name: "maple-chat--g7h8", kind: "chat", role: "owner" },
  { name: "keyed-chat--j9k2", kind: "chat", role: "owner", sharing: { reach: true } },
  { name: "sams-telegram--m3n4", kind: "chat", role: "viewer", sharing: { reach: true } },
];
const DOORS = { "maple-telegram--e5f6": { door: "telegram", agent: "maple--a1b2" }, "keyed-chat--j9k2": null };

// `link(name, body, doors)`: the binding's answer (its default binds it)
function world({ refuse = new Set(), listed = true, link = (name, body, doors) => (doors[name] = { door: "telegram", agent: body.agent }) } = {}) {
  const doors = { ...DOORS };
  const list = listOf();
  const asked = [];
  const opened = [];
  const notices = [];
  const at = { listed, loads: 0, made: 0 };
  const api = async (method, path, body) => {
    asked.push(`${method} ${path}`);
    const door = path.match(/^\/api\/f\/([^/]+)\/door$/)?.[1];
    if (method === "GET" && door) {
      if (refuse.has(door)) throw Object.assign(new Error("refused"), { status: 500 });
      if (!(door in doors)) throw Object.assign(new Error("not found"), { status: 404 });
      return { bound: doors[door] };
    }
    if (method === "POST" && path === "/api/fragments") {
      const name = `${body.label}--n${++at.made}p4`;
      doors[name] = null;
      return { name };
    }
    if (method === "POST" && door) return link(door, body, doors);
    if (method === "DELETE") {
      delete doors[path.split("/")[3]];
      return {};
    }
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
    computer: () => ({ agents: AGENTS }), fragments: () => list, listed: () => at.listed, own: (f) => f.role === "owner",
    connectorLabel: (agent, door) => `${agent.split("--")[0]}-${door}`, markOffer: (node, agent, what) => Object.assign(node, { offer: [agent, what] }),
    framed: (name, path) => `/auth/frame?name=${name}&return=${path}`, openSetup: (o) => opened.push(o), openChat: () => {},
    notice: (...n) => notices.push(n), load: async () => { at.loads++; at.listed = true; },
  });
  let drawn = null;
  globalThis.document = { getElementById: (id) => (id === "settings-telegram" ? drawn : null), querySelector: () => null };
  const draw = () => (drawn = telegram.telegramSection());
  const rows = () => drawn.children.filter((c) => c.className.includes("settings-line"));
  const said = () => drawn.children.find((c) => c.tag === "p")?.textContent;
  const press = (i, text) => buttons(rows()[i]).find((b) => b.textContent === text).onclick();
  const shown = () => rows().map((r) => buttons(r).map((b) => b.textContent));
  return { telegram, asked, opened, notices, draw, rows, said, press, shown, doors, list, at, refuse };
}

test("its own chats with reach are read, and an agent's bound one is Open and Set up; another's Set up alone", async () => {
  const w = world();
  await w.telegram.readTelegram();
  w.draw();
  assert.deepEqual(w.asked.sort(), ["GET /api/f/keyed-chat--j9k2/door", "GET /api/f/maple-telegram--e5f6/door"], "no other's, and none without reach");
  assert.deepEqual(w.shown(), [["Open", "Set up"], ["Set up"]]);
  assert.equal(buttons(w.rows()[0])[0].dataset.telegramOpen, "maple-telegram--e5f6");
  assert.deepEqual(w.rows()[0].offer, ["maple--a1b2", "doors"], "shown only for a runtime with doors");
  assert.equal(w.said(), "Message your agents from Telegram.");
});

test("a door read that fails, or a list not read, shows no Set up: Try again", async () => {
  for (const w of [world({ refuse: new Set(["keyed-chat--j9k2"]) }), world({ listed: false })]) {
    await w.telegram.readTelegram();
    w.draw();
    assert.deepEqual(w.shown(), [["Try again"], ["Try again"]]);
    assert.equal(w.said(), "Couldn't read your Telegram chats.");
  }
});

test("a read that fails after one that answered shows no Set up either: its doors are the latest read's", async () => {
  const w = world();
  await w.telegram.readTelegram();
  w.draw();
  w.refuse.add("keyed-chat--j9k2");
  await w.telegram.refreshTelegram(true);
  assert.deepEqual(w.shown(), [["Try again"], ["Try again"]]);
});

test("Try again with the list not read reads the list, then its doors", async () => {
  const w = world({ listed: false });
  await w.telegram.readTelegram();
  w.draw();
  await w.press(1, "Try again");
  assert.equal(w.at.loads, 1);
  assert.deepEqual(w.shown(), [["Open", "Set up"], ["Set up"]]);
});

test("Set up opens a bound chat's setup page, making nothing", async () => {
  const w = world();
  await w.telegram.readTelegram();
  w.draw();
  await w.press(0, "Set up");
  assert.equal(w.opened.length, 1);
  assert.deepEqual({ ...w.opened[0], onClose: undefined }, { src: "/auth/frame?name=maple-telegram--e5f6&return=/setup.html", name: "maple-telegram--e5f6", agent: "maple--a1b2", title: "Telegram", onClose: undefined });
  assert.ok(!w.asked.some((a) => a.startsWith("POST")));
});

test("Set up reads again first: a chat another tab made and bound since is opened, and none made", async () => {
  const w = world();
  await w.telegram.readTelegram();
  w.draw();
  // another tab's Set up, which this list has not shown yet
  w.list.push({ name: "wren-telegram--x1y2", kind: "chat", role: "owner", sharing: { reach: true } });
  w.doors["wren-telegram--x1y2"] = { door: "telegram", agent: "wren--c3d4" };
  await w.press(1, "Set up");
  assert.deepEqual(w.opened.map((o) => o.name), ["wren-telegram--x1y2"]);
  assert.ok(!w.asked.some((a) => a.startsWith("POST")), "none made");
  // and a list that shows it reads it with no press
  const v = world();
  await v.telegram.readTelegram();
  v.draw();
  v.list.push({ name: "wren-telegram--x1y2", kind: "chat", role: "owner", sharing: { reach: true } });
  v.doors["wren-telegram--x1y2"] = { door: "telegram", agent: "wren--c3d4" };
  await v.telegram.refreshTelegram();
  assert.deepEqual(v.shown(), [["Open", "Set up"], ["Open", "Set up"]]);
});

test("Set up whose read fails makes nothing, and says so", async () => {
  const w = world();
  await w.telegram.readTelegram();
  w.draw();
  w.refuse.add("keyed-chat--j9k2");
  await w.press(1, "Set up");
  assert.ok(!w.asked.some((a) => a.startsWith("POST")));
  assert.deepEqual(w.notices, [["Telegram was not set up", "Couldn't read your Telegram chats. Try again."]]);
});

test("with none, Set up makes one, binds it as the agent's door, and opens its setup page, the list read first", async () => {
  const w = world();
  await w.telegram.readTelegram();
  w.draw();
  await w.press(1, "Set up");
  assert.deepEqual(w.asked.filter((a) => !a.startsWith("GET")), ["POST /api/fragments", "POST /api/f/wren-telegram--n1p4/door"]);
  assert.equal(w.at.loads, 1);
  assert.deepEqual(w.opened.map((o) => [o.name, o.agent]), [["wren-telegram--n1p4", "wren--c3d4"]]);
  assert.deepEqual(w.shown()[1], ["Open", "Set up"], "drawn bound at once");
  // read again, the list not yet knowing it has reach: read all the same
  await w.telegram.readTelegram();
  w.draw();
  assert.ok(w.asked.includes("GET /api/f/wren-telegram--n1p4/door"));
  assert.deepEqual(w.shown()[1], ["Open", "Set up"]);
});

test("a binding refused deletes the chat it made, and says so; nothing opens", async () => {
  const w = world({ link: () => { throw Object.assign(new Error("telegram is not allowed"), { status: 403 }); } });
  await w.telegram.readTelegram();
  w.draw();
  await w.press(1, "Set up");
  assert.deepEqual(w.asked.filter((a) => !a.startsWith("GET")), ["POST /api/fragments", "POST /api/f/wren-telegram--n1p4/door", "DELETE /api/f/wren-telegram--n1p4"]);
  assert.deepEqual(w.notices, [["Telegram was not set up", "telegram is not allowed"]]);
  assert.equal(w.opened.length, 0);
  assert.deepEqual(w.shown()[1], ["Set up"]);
});

test("a binding whose answer was lost is read: bound, it opens; not bound, it goes; unreadable, it is kept", async () => {
  const lost = (commit) => (name, body, doors) => {
    if (commit) doors[name] = { door: "telegram", agent: body.agent };
    throw Object.assign(new Error("Failed to fetch"), { status: commit === null ? 502 : undefined });
  };
  // bound, its answer lost on the way
  const w = world({ link: lost(true) });
  await w.telegram.readTelegram();
  w.draw();
  await w.press(1, "Set up");
  assert.deepEqual(w.opened.map((o) => o.name), ["wren-telegram--n1p4"]);
  assert.ok(!w.asked.some((a) => a.startsWith("DELETE")), "kept");
  // not bound (a 502): read, and deleted
  const v = world({ link: lost(null) });
  await v.telegram.readTelegram();
  v.draw();
  await v.press(1, "Set up");
  assert.ok(v.asked.includes("DELETE /api/f/wren-telegram--n1p4"));
  assert.equal(v.opened.length, 0);
  // its door unreadable: kept, and said
  const u = world({ link: (name, body, doors) => { u.refuse.add(name); throw Object.assign(new Error("Failed to fetch"), {}); } });
  await u.telegram.readTelegram();
  u.draw();
  await u.press(1, "Set up");
  assert.ok(!u.asked.some((a) => a.startsWith("DELETE")), "kept");
  assert.deepEqual(u.notices, [["Telegram was not set up", "Couldn't tell whether it was set up. Try again."]]);
});
