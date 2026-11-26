// cell/shell/connections.js `createConnectors`: Settings' Telegram row, one
// per agent, and SimpleX's, as its connector's is the same (its own at the
// end). Which chat is whose is read, never guessed: each of the
// person's own chats with reach, its door as its owner reads it; a read
// that fails, or a list not read, shows no Set up (which would make a
// second chat beside one not read). Set up opens a bound chat's setup page;
// with none, it makes one, binds it as the agent's door, and opens its; a
// binding refused deletes the one it made. Run by `cargo xtask check` on
// the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { createConnectors } from "../shell/connections.js";

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
// `vault`: the person's Vault as its list answers it (none: its read fails)
// `computerRow`: SimpleX's row on an agent's computer, where it keeps one
function world({ refuse = new Set(), listed = true, link = (name, body, doors) => (doors[name] = { door: name.includes("-simplex--") ? "simplex" : "telegram", agent: body.agent }), vault, computerRow } = {}) {
  const doors = { ...DOORS };
  // the person's list as the platform holds it, and as this page last read it
  const server = listOf();
  const list = listOf();
  const asked = [];
  // each body of the API's writes, in order
  const bodies = [];
  const opened = [];
  const notices = [];
  const at = { listed, loads: 0, made: 0 };
  const api = async (method, path, body) => {
    asked.push(`${method} ${path}`);
    if (method !== "GET") bodies.push(body);
    const door = path.match(/^\/api\/f\/([^/]+)\/door$/)?.[1];
    if (method === "GET" && door) {
      if (refuse.has(door)) throw Object.assign(new Error("refused"), { status: 500 });
      if (!(door in doors)) throw Object.assign(new Error("not found"), { status: 404 });
      return { bound: doors[door] };
    }
    if (method === "GET" && path === "/api/vault") {
      if (!vault) throw Object.assign(new Error("refused"), { status: 500 });
      return { secrets: vault };
    }
    if (method === "POST" && path === "/api/fragments") {
      const name = `${body.label}--n${++at.made}p4`;
      doors[name] = null;
      server.push({ name, kind: "chat", role: "owner" });
      return { name };
    }
    if (method === "POST" && door) {
      const answer = link(door, body, doors);
      if (doors[door]) server.find((f) => f.name === door).sharing = { reach: true };
      return answer;
    }
    const lent = path.match(/^\/api\/vault\/([a-z0-9-]+)$/);
    if (method === "PUT" && lent) {
      if (at.lendRefused) throw Object.assign(new Error("a secret's value: it is no secret of yours yet"), { status: 400 });
      return { name: lent[1], ...body, setAt: 1 };
    }
    if (method === "DELETE" && /^\/api\/f\/[^/]+\/secrets\/[A-Z_]+$/.test(path)) return { ok: true, removed: false };
    if (method === "DELETE") {
      const name = path.split("/")[3];
      delete doors[name];
      server.splice(server.findIndex((f) => f.name === name), 1);
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
  // the page's listeners (`message`), and each frame it made, as `frameOf` makes one
  const listeners = [];
  globalThis.addEventListener = (type, fn) => type === "message" && listeners.push(fn);
  const frames = [];
  const frameOf = (src, title, name) => {
    const posted = [];
    const frame = { dataset: { src, fragment: name }, title, contentWindow: { postMessage: (m, origin) => posted.push([m, origin]) }, posted };
    frames.push(frame);
    return frame;
  };
  const originAt = (name) => `http://${name}.fragment.localhost`;
  // a message to the page, as the browser hands it to its listeners
  const message = async (e) => {
    for (const fn of listeners) await fn(e);
  };
  const telegram = createConnectors({
    api, el: (tag, cls, text) => new Node(tag, cls, text), section, actionLine, named: (node, f) => Object.assign(node, { textContent: f }), seg: (s) => s,
    computer: () => ({ agents: AGENTS }), fragments: () => list, listed: () => at.listed, own: (f) => f.role === "owner",
    connectorLabel: (agent, door) => `${agent.split("--")[0]}-${door}`, markOffer: (node, agent, what) => Object.assign(node, { offer: [agent, what] }),
    framed: (name, path) => `/auth/frame?name=${name}&return=${path}`, frameOf, originAt, pressSetup: () => 1, openSetup: (o) => opened.push(o), openChat: () => {},
    titleOf: (agent) => `${agent[0].toUpperCase()}${agent.slice(1).split("--")[0]}`, computerRow, computerKey: () => "",
    notice: (...n) => notices.push(n), load: async () => {
      at.loads++;
      if (at.listFails) {
        at.listed = false;
        throw new Error("the list did not come");
      }
      list.splice(0, list.length, ...structuredClone(server));
      at.listed = true;
    },
  });
  let drawn = null;
  let sections = {};
  globalThis.document = { getElementById: (id) => sections[id] ?? null, querySelector: () => null };
  // `kind`: which connector's section (Telegram's, unless SimpleX's)
  const draw = (kind = "telegram") => {
    drawn = kind === "simplex" ? telegram.simplexSection() : telegram.telegramSection();
    sections = { [`settings-${kind}`]: drawn };
    return drawn;
  };
  const rows = () => drawn.children.filter((c) => c.className.includes("settings-line"));
  const said = () => drawn.children.find((c) => c.tag === "p")?.textContent;
  const press = (i, text) => buttons(rows()[i]).find((b) => b.textContent === text).onclick();
  const shown = () => rows().map((r) => buttons(r).map((b) => b.textContent));
  // another tab's Set up: its connector made and bound, which this page's list does not show yet
  const elsewhere = (name, agent) => {
    server.push({ name, kind: "chat", role: "owner", sharing: { reach: true } });
    doors[name] = { door: "telegram", agent };
  };
  return { telegram, asked, bodies, frames, message, originAt, opened, notices, draw, rows, said, press, shown, doors, list, server, at, refuse, elsewhere };
}

test("its own chats with reach are read, and an agent's bound one is Open and Set up; another's Set up alone", async () => {
  const w = world();
  await w.telegram.readConnectors();
  w.draw();
  assert.deepEqual(w.asked.sort(), ["GET /api/f/keyed-chat--j9k2/door", "GET /api/f/maple-telegram--e5f6/door"], "no other's, and none without reach");
  assert.deepEqual(w.shown(), [["Open", "Set up"], ["Set up"]]);
  assert.equal(buttons(w.rows()[0])[0].dataset.telegramOpen, "maple-telegram--e5f6");
  assert.deepEqual(w.rows()[0].offer, ["maple--a1b2", "doors"], "shown only for a runtime with doors");
  assert.equal(w.said(), "Message your agents from Telegram.");
});

test("a door read that fails, or a list not read, shows no Set up: Try again", async () => {
  for (const w of [world({ refuse: new Set(["keyed-chat--j9k2"]) }), world({ listed: false })]) {
    await w.telegram.readConnectors();
    w.draw();
    assert.deepEqual(w.shown(), [["Try again"], ["Try again"]]);
    assert.equal(w.said(), "Couldn't read your Telegram chats.");
  }
});

test("a read that fails after one that answered shows no Set up either: its doors are the latest read's", async () => {
  const w = world();
  await w.telegram.readConnectors();
  w.draw();
  w.refuse.add("keyed-chat--j9k2");
  await w.telegram.refreshConnectors(true);
  assert.deepEqual(w.shown(), [["Try again"], ["Try again"]]);
});

test("Try again with the list not read reads the list, then its doors", async () => {
  const w = world({ listed: false });
  await w.telegram.readConnectors();
  w.draw();
  await w.press(1, "Try again");
  assert.equal(w.at.loads, 1);
  assert.deepEqual(w.shown(), [["Open", "Set up"], ["Set up"]]);
});

test("Set up opens a bound chat's setup page, making nothing", async () => {
  const w = world();
  await w.telegram.readConnectors();
  w.draw();
  await w.press(0, "Set up");
  assert.equal(w.opened.length, 1);
  assert.deepEqual({ ...w.opened[0], onClose: undefined, frame: undefined }, { pressed: 1, src: "/auth/frame?name=maple-telegram--e5f6&return=/setup.html", name: "maple-telegram--e5f6", agent: "maple--a1b2", title: "Telegram", tall: false, onClose: undefined, frame: undefined });
  // its frame, Settings' own, sent to its setup page
  assert.equal(w.opened[0].frame, w.frames[0]);
  assert.equal(w.frames[0].dataset.src, w.opened[0].src);
  assert.ok(!w.asked.some((a) => a.startsWith("POST")));
});

test("Set up names the bot its person's Vault holds for that agent, toward api.telegram.org alone, to its setup page (FIN-463 F11); none else", async () => {
  const vault = [
    { name: "telegram-wren--c3d4", hosts: ["api.telegram.org"], agents: ["wren--c3d4"] },
    { name: "two-hosts", hosts: ["api.telegram.org", "example.com"], agents: ["maple--a1b2"] },
    { name: "linear", hosts: ["api.linear.app"], agents: ["maple--a1b2"] },
    { name: "telegram-maple--a1b2", hosts: ["api.telegram.org"], agents: ["maple--a1b2"] },
  ];
  const w = world({ vault });
  await w.telegram.readConnectors();
  w.draw();
  await w.press(0, "Set up");
  assert.equal(w.opened[0].src, "/auth/frame?name=maple-telegram--e5f6&return=/setup.html?vault=telegram-maple--a1b2");
  // none of its: the page as before; nor with a Vault not read
  const none = world({ vault: vault.slice(0, 3) });
  await none.telegram.readConnectors();
  none.draw();
  await none.press(0, "Set up");
  assert.equal(none.opened[0].src, "/auth/frame?name=maple-telegram--e5f6&return=/setup.html");
});

test("Set up reads the list and its doors anew first: a chat another tab made since, which this page's list does not show yet, is opened, and none made", async () => {
  const w = world();
  await w.telegram.readConnectors();
  w.draw();
  w.elsewhere("wren-telegram--x1y2", "wren--c3d4");
  assert.ok(!w.list.some((f) => f.name === "wren-telegram--x1y2"), "this page's list is behind");
  await w.press(1, "Set up");
  assert.deepEqual(w.opened.map((o) => o.name), ["wren-telegram--x1y2"]);
  assert.ok(!w.asked.some((a) => a.startsWith("POST")), "none made");
  // and a list that comes showing it reads it, no press
  const v = world();
  await v.telegram.readConnectors();
  v.draw();
  v.elsewhere("wren-telegram--x1y2", "wren--c3d4");
  v.list.push(structuredClone(v.server.at(-1)));
  await v.telegram.refreshConnectors();
  assert.deepEqual(v.shown(), [["Open", "Set up"], ["Open", "Set up"]]);
});

test("Set up whose list or doors read fails makes nothing, and says so", async () => {
  const w = world();
  await w.telegram.readConnectors();
  w.draw();
  w.refuse.add("keyed-chat--j9k2");
  await w.press(1, "Set up");
  const v = world();
  await v.telegram.readConnectors();
  v.draw();
  v.at.listFails = true;
  await v.press(1, "Set up");
  for (const x of [w, v]) {
    assert.ok(!x.asked.some((a) => a.startsWith("POST")));
    assert.deepEqual(x.notices, [["Telegram was not set up", "Couldn't read your Telegram chats. Try again."]]);
  }
});

test("with none, Set up makes one, binds it as the agent's door, and opens its setup page, the list read first", async () => {
  const w = world();
  await w.telegram.readConnectors();
  w.draw();
  await w.press(1, "Set up");
  assert.deepEqual(w.asked.filter((a) => !a.startsWith("GET")), ["POST /api/fragments", "POST /api/f/wren-telegram--n1p4/door"]);
  assert.equal(w.at.loads, 2, "the list read before it, and after");
  assert.deepEqual(w.opened.map((o) => [o.name, o.agent]), [["wren-telegram--n1p4", "wren--c3d4"]]);
  assert.deepEqual(w.shown()[1], ["Open", "Set up"], "drawn bound at once");
  // read again, the list not yet knowing it has reach: read all the same
  await w.telegram.readConnectors();
  w.draw();
  assert.ok(w.asked.includes("GET /api/f/wren-telegram--n1p4/door"));
  assert.deepEqual(w.shown()[1], ["Open", "Set up"]);
});

test("a binding refused deletes the chat it made, and says so; nothing opens", async () => {
  const w = world({ link: () => { throw Object.assign(new Error("telegram is not allowed"), { status: 403 }); } });
  await w.telegram.readConnectors();
  w.draw();
  await w.press(1, "Set up");
  assert.deepEqual(w.asked.filter((a) => !a.startsWith("GET")), ["POST /api/fragments", "POST /api/f/wren-telegram--n1p4/door", "DELETE /api/f/wren-telegram--n1p4"]);
  assert.deepEqual(w.notices, [["Telegram was not set up", "telegram is not allowed"]]);
  assert.equal(w.opened.length, 0);
  assert.deepEqual(w.shown()[1], ["Set up"]);
});

test("a binding whose answer was lost is read: bound to its agent, it opens; to another meanwhile, it is theirs; not bound, it goes; unreadable, it is kept", async () => {
  const lost = (commit) => (name, body, doors) => {
    if (commit) doors[name] = { door: "telegram", agent: body.agent };
    throw Object.assign(new Error("Failed to fetch"), { status: commit === null ? 502 : undefined });
  };
  // bound, its answer lost on the way
  const w = world({ link: lost(true) });
  await w.telegram.readConnectors();
  w.draw();
  await w.press(1, "Set up");
  assert.deepEqual(w.opened.map((o) => o.name), ["wren-telegram--n1p4"]);
  assert.ok(!w.asked.some((a) => a.startsWith("DELETE")), "kept");
  // not bound (a 502): read, and deleted
  const v = world({ link: lost(null) });
  await v.telegram.readConnectors();
  v.draw();
  await v.press(1, "Set up");
  assert.ok(v.asked.includes("DELETE /api/f/wren-telegram--n1p4"));
  assert.equal(v.opened.length, 0);
  // its door unreadable: kept, and said
  const u = world({ link: (name, body, doors) => { u.refuse.add(name); throw Object.assign(new Error("Failed to fetch"), {}); } });
  await u.telegram.readConnectors();
  u.draw();
  await u.press(1, "Set up");
  assert.ok(!u.asked.some((a) => a.startsWith("DELETE")), "kept");
  assert.deepEqual(u.notices, [["Telegram was not set up", "Couldn't tell whether it was set up. Try again."]]);
  // bound, its answer lost, then bound again elsewhere to another agent before it was read: theirs, kept
  const t = world({ link: (name, body, doors) => {
    doors[name] = { door: "telegram", agent: "maple--a1b2" };
    throw Object.assign(new Error("Failed to fetch"), {});
  } });
  await t.telegram.readConnectors();
  t.draw();
  await t.press(1, "Set up");
  assert.ok(!t.asked.some((a) => a.startsWith("DELETE")), "kept: another agent's now");
  assert.equal(t.opened.length, 0);
  assert.deepEqual(t.notices, [["Telegram was not set up", "It was linked to another agent meanwhile."]]);
});

// Use it: the page asks Settings to lend the Vault's bot (`vault-lend`),
// since a page may not write a Vault (FIN-615, PR 12). Settings lends only
// what it offered that frame, to that connector, the rest of the secret as
// it was, and answers that frame alone.
const PLACE = { header: "authorization", format: "Bearer {}" };
const OFFERED = [{ name: "telegram-maple--a1b2", hosts: ["api.telegram.org"], place: PLACE, agents: ["maple--a1b2"], apps: [{ app: "old--x1y2", key: "BOT" }] }];
async function offered(opts = {}) {
  const w = world({ vault: OFFERED, ...opts });
  await w.telegram.readConnectors();
  w.draw();
  await w.press(0, "Set up");
  const frame = w.frames[0];
  const from = (data, over = {}) => w.message({ data, source: frame.contentWindow, origin: w.originAt("maple-telegram--e5f6"), ...over });
  return { w, frame, from };
}

test("Use it: Settings lends the bot it offered that frame to its connector, the Vault keeping it as it was, and answers it", async () => {
  const { w, frame, from } = await offered();
  await from({ fragment: "vault-lend", nonce: "n1", vault: "another-secret", hosts: ["evil.example.com"], name: "elsewhere--x1y2" });
  assert.deepEqual(w.asked.filter((a) => !a.startsWith("GET")).slice(-2), ["PUT /api/vault/telegram-maple--a1b2", "DELETE /api/f/maple-telegram--e5f6/secrets/TELEGRAM_BOT_TOKEN"]);
  assert.deepEqual(
    w.bodies.at(-2),
    { hosts: ["api.telegram.org"], place: PLACE, agents: ["maple--a1b2"], apps: [{ app: "old--x1y2", key: "BOT" }, { app: "maple-telegram--e5f6", key: "TELEGRAM_BOT_TOKEN" }] },
    "what it offered, no value, its own apps kept beside: never what the page names",
  );
  assert.deepEqual(frame.posted, [[{ fragment: "vault-lend", nonce: "n1", ok: true }, "http://maple-telegram--e5f6.fragment.localhost"]]);
  // again (its answer lost): lent as before, once
  await from({ fragment: "vault-lend", nonce: "n1b" });
  assert.deepEqual(w.bodies.at(-2).apps, [{ app: "old--x1y2", key: "BOT" }, { app: "maple-telegram--e5f6", key: "TELEGRAM_BOT_TOKEN" }]);
});

test("a lend refused is said to that frame with its status, and the connector's own token is left be", async () => {
  const { w, frame, from } = await offered();
  w.at.lendRefused = true;
  await from({ fragment: "vault-lend", nonce: "n2" });
  assert.deepEqual(frame.posted, [[{ fragment: "vault-lend", nonce: "n2", ok: false, status: 400, error: "a secret's value: it is no secret of yours yet" }, "http://maple-telegram--e5f6.fragment.localhost"]]);
  assert.ok(!w.asked.some((a) => a.startsWith("DELETE")));
});

test("no lend for a frame at another origin, another window, a frame offered nothing, or once its dialog closed", async () => {
  const { w, frame, from } = await offered();
  // its frame sent elsewhere, at another origin
  await from({ fragment: "vault-lend", nonce: "a" }, { origin: "https://evil.example.com" });
  // a window of no frame of Settings'
  await from({ fragment: "vault-lend", nonce: "b" }, { source: { postMessage: () => {} } });
  assert.ok(!w.asked.some((a) => a.startsWith("PUT")));
  assert.deepEqual(frame.posted, []);
  // with no bot in the Vault for its agent, its frame is offered nothing
  const none = world({ vault: [] });
  await none.telegram.readConnectors();
  none.draw();
  await none.press(0, "Set up");
  await none.message({ data: { fragment: "vault-lend", nonce: "c" }, source: none.frames[0].contentWindow, origin: none.originAt("maple-telegram--e5f6") });
  assert.ok(!none.asked.some((a) => a.startsWith("PUT")));
  // its dialog closed: its offer is gone
  w.opened[0].onClose();
  await from({ fragment: "vault-lend", nonce: "d" });
  assert.ok(!w.asked.some((a) => a.startsWith("PUT")));
});

// SimpleX's section is its connector's, as Telegram's is, on any seat:
// its kind's words, its setup page named for its agent at its QR code's
// height; and its computer's row instead where its computer keeps it
test("SimpleX's row makes its agent's connector, bound as its door, and opens its setup page named for that agent", async () => {
  const w = world();
  await w.telegram.readConnectors();
  const s = w.draw("simplex");
  assert.equal(s.id, "settings-simplex");
  assert.equal(w.said(), "Message your agents from SimpleX Chat.");
  const tags = s.children.find((c) => c.className === "connector-tags").children.map((t) => [t.textContent, t.title ?? ""]);
  assert.deepEqual(tags, [["Experimental", ""], ["Secure", "SimpleX's servers can't read these messages"]]);
  assert.deepEqual(w.shown(), [["Set up"], ["Set up"]], "Maple's Telegram is no SimpleX");
  assert.deepEqual(w.rows().map((r) => [buttons(r)[0].dataset.simplexFor, r.offer]), [["maple--a1b2", ["maple--a1b2", "doors"]], ["wren--c3d4", ["wren--c3d4", "doors"]]]);
  await w.press(1, "Set up");
  const made = w.bodies.find((b) => b?.template);
  assert.deepEqual(made, { label: "wren-simplex", template: "simplex" });
  const name = Object.keys(w.doors).find((n) => n.startsWith("wren-simplex--"));
  assert.deepEqual(w.doors[name], { door: "simplex", agent: "wren--c3d4" }, "bound as Wren's SimpleX door");
  assert.equal(w.opened.length, 1);
  assert.deepEqual([w.opened[0].title, w.opened[0].tall, w.opened[0].src], ["SimpleX", true, `/auth/frame?name=${name}&return=/setup.html?name=Wren`]);
  assert.ok(!w.asked.includes("GET /api/vault"), "no Vault offer: SimpleX has no bot to lend");
  assert.deepEqual(w.notices, []);
  w.draw("simplex");
  assert.deepEqual(w.shown(), [["Set up"], ["Open", "Set up"]], "its chat, to open");
});

test("where its computer keeps an agent's SimpleX, its row is its computer's; Telegram's never is", async () => {
  const kept = { tag: "div", className: "settings-line its-computer", textContent: "", children: [], dataset: {}, hidden: false };
  const w = world({ computerRow: (a) => (a.fragment === "maple--a1b2" ? kept : null) });
  await w.telegram.readConnectors();
  w.draw("simplex");
  assert.equal(w.rows()[0], kept);
  assert.deepEqual(buttons(w.rows()[1]).map((b) => b.textContent), ["Set up"]);
  w.draw("telegram");
  assert.ok(!w.rows().includes(kept));
});
