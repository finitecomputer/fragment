// cell/shell/move-page.js, the move's page (FIN-463 F6): a person's old
// agents in the server's words, the one question (which, by what name),
// then for each a new agent made as Settings' Add makes one and its Yes
// posted; a refusal in the server's words; Ask again after a Never; a
// move under way read again. The real `createMovePage`, its page faked;
// each call answers only when the test says.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { createMovePage } from "../shell/move-page.js";

class Node {
  constructor(tag, cls, text) {
    Object.assign(this, { tag, className: cls ?? "", textContent: text ?? "", children: [], dataset: {}, hidden: false, disabled: false, onclick: null, attrs: {} });
  }
  append(...c) {
    this.children.push(...c);
  }
  replaceChildren(...c) {
    this.children = [...c];
  }
  setAttribute(k, v) {
    this.attrs[k] = v;
  }
}
const walk = (n) => [n, ...n.children.flatMap(walk)];
const settle = async () => {
  for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 0));
};

function page(t, own = { agents: [], ids: new Map(), chats: new Map() }) {
  const root = new Node("section");
  const calls = [];
  const made = [];
  const timers = { setTimeout: (fn) => (timers.due = fn, 1), clearTimeout: () => (timers.due = null), due: null };
  const api = (method, path, body) => new Promise((ok, fail) => calls.push({ method, path, body, ok, fail }));
  let guest = true;
  const opened = [];
  const ui = createMovePage(root, {
    api, el: (tag, cls, text) => new Node(tag, cls, text), timers,
    setGuest: (g) => (guest = g),
    addAgent: (spec) => new Promise((ok, fail) => made.push({ spec, ok, fail })),
    reload: () => {},
    ownAgents: () => own.agents, identityOf: (f) => own.ids.get(f) ?? null,
    chatOf: (identity) => own.chats?.get(identity) ?? null,
    openChat: (chat) => (opened.push(chat), ui.close()),
  });
  t.after(() => ui.close());
  const rows = () => walk(root).filter((n) => n.tag === "li").map((li) => ({
    old: li.dataset.old,
    words: walk(li).filter((n) => n.tag === "strong" || n.tag === "p").map((n) => n.textContent),
    pick: walk(li).some((n) => n.tag === "input"),
    buttons: walk(li).filter((n) => n.tag === "button").map((b) => b.textContent),
  }));
  const button = (label) => walk(root).find((n) => n.tag === "button" && n.textContent === label);
  const errors = () => walk(root).filter((n) => n.className === "form-error").map((n) => n.textContent);
  const next = (method, path) => {
    const c = calls.shift();
    assert.deepEqual([c?.method, c?.path], [method, path], "the next call");
    return c;
  };
  return { ui, root, calls, made, timers, rows, button, errors, next, opened, guest: () => guest };
}

const offer = (old, state, said, o = {}) => ({ old, name: `Old ${old}`, state, said, ...o });

test("its agents in the server's words: the askable ones each a choice, a Never'd one Ask again, one under way neither", async (t) => {
  const p = page(t);
  p.ui.open();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "open", "Old a is on finite.computer.", { image: "/api/migrations/offers/a/picture" }), offer("b", "never", "You said never."), offer("c", "staging", "Copying Old c…")] });
  await settle();
  assert.deepEqual(p.rows(), [
    { old: "a", words: ["Old a", "Old a is on finite.computer."], pick: true, buttons: [] },
    { old: "b", words: ["Old b", "You said never."], pick: false, buttons: ["Ask again"] },
    { old: "c", words: ["Old c", "Copying Old c…"], pick: false, buttons: [] },
  ]);
  assert.equal(walk(p.root).find((n) => n.tag === "img")?.dataset.src, "/api/migrations/offers/a/picture");
  assert.ok(p.button("Bring over"), "the one question's answer");
  assert.ok(p.timers.due, "a move under way is read again");
  p.timers.due();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("c", "importing", "Bringing Old c over…")] });
  await settle();
  assert.deepEqual(p.rows()[0].words, ["Old c", "Bringing Old c over…"], "as the server says it now");
});

test("Bring over: the move opened (the comp), a seat waited for, then each a new agent made quietly and its Yes posted with it, the page read again; nothing said ahead of the server", async (t) => {
  const p = page(t);
  p.ui.open();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "open", "Old a is on finite.computer.")] });
  await settle();
  p.button("Bring over").onclick();
  await settle();
  p.next("POST", "/api/migrations/offers").ok({ offers: [] });
  await settle();
  p.next("GET", "/api/ledger").ok({ plan: "seat" });
  await settle();
  assert.equal(p.guest(), false, "a seat, as the platform said");
  assert.deepEqual(p.made.map((m) => m.spec), [{ name: "Old a", role: null, quiet: true, from: "a", agent: null }], "made for its old agent, which its agent.json keeps, under its name");
  assert.deepEqual(p.rows()[0].words, ["Old a", "Making Old a…"], "while it is made");
  p.made[0].ok({ chat: "juniper-chat--k3x9", id: "npub1juniper", computer: "computer:aa" });
  await settle();
  const yes = p.next("POST", "/api/migrations/offers/a/yes");
  assert.deepEqual(yes.body, { agent: "npub1juniper" });
  yes.ok({ state: "asked" });
  await settle();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "asked", "Stopping Old a on finite.computer…")] });
  await settle();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "asked", "Stopping Old a on finite.computer…")] });
  await settle();
  assert.deepEqual(p.rows(), [{ old: "a", words: ["Old a", "Stopping Old a on finite.computer…"], pick: false, buttons: [] }]);
  assert.equal(p.button("Bring over"), undefined, "nothing left to ask");
});

test("a refusal is the server's words, the page as read, to try again", async (t) => {
  const p = page(t);
  p.ui.open();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "open", "Old a is on finite.computer.")] });
  await settle();
  p.button("Bring over").onclick();
  await settle();
  p.next("POST", "/api/migrations/offers").fail(Object.assign(new Error("this person signed in with no WorkOS user"), { status: 409 }));
  await settle();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "open", "Old a is on finite.computer.")] });
  await settle();
  assert.deepEqual(p.errors(), ["this person signed in with no WorkOS user"]);
  assert.equal(p.made.length, 0, "no agent made");
  assert.equal(p.button("Bring over").disabled, false, "to try again");
});

test("Ask again after a Never posts it, then the page as the server says", async (t) => {
  const p = page(t);
  p.ui.open();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("b", "never", "You said never.")] });
  await settle();
  p.button("Ask again").onclick();
  await settle();
  p.next("POST", "/api/migrations/offers/b/ask-again").ok({ state: "open" });
  await settle();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("b", "open", "Old b is on finite.computer.")] });
  await settle();
  assert.deepEqual(p.rows()[0].pick, true);
});

test("one new agent an old one: the agent made for it before (a Yes that never arrived, a making cut short, a reload) is handed back to Add to go on with, never a second", async (t) => {
  for (const placed of [true, false]) {
    const p = page(t, { agents: ["mine--a1b2", "juniper--k3x9"], ids: new Map([["mine--a1b2", "npub1mine"], ...(placed ? [["juniper--k3x9", "npub1juniper"]] : [])]) });
    p.ui.open();
    p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "open", "Old a is on finite.computer.")] });
    await settle();
    p.button("Bring over").onclick();
    await settle();
    p.next("POST", "/api/migrations/offers").ok({ offers: [] });
    await settle();
    p.next("GET", "/api/ledger").ok({ plan: "seat" });
    await settle();
    p.next("GET", "/api/f/mine--a1b2/file?path=agent.json").fail(Object.assign(new Error("not found"), { status: 404 }));
    await settle();
    p.next("GET", "/api/f/juniper--k3x9/file?path=agent.json").ok({ tier: "standard", from: "a" });
    await settle();
    assert.deepEqual(p.made.map((m) => m.spec), [{ name: "Old a", role: null, quiet: true, from: "a", agent: "juniper--k3x9" }], `the one made for it, handed back (${placed ? "on its computer" : "cut short"})`);
    p.made[0].ok({ chat: "juniper-chat--k3x9", id: "npub1juniper", computer: "computer:aa" });
    await settle();
    assert.deepEqual(p.next("POST", "/api/migrations/offers/a/yes").body, { agent: "npub1juniper" });
  }
});

test("a mapping that could not be read is no leave to make one: Bring over stops, saying so, to try again, and makes nothing", async (t) => {
  const p = page(t, { agents: ["juniper--k3x9"], ids: new Map([["juniper--k3x9", "npub1juniper"]]) });
  p.ui.open();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "open", "Old a is on finite.computer.")] });
  await settle();
  p.button("Bring over").onclick();
  await settle();
  p.next("POST", "/api/migrations/offers").ok({ offers: [] });
  await settle();
  p.next("GET", "/api/ledger").ok({ plan: "seat" });
  await settle();
  p.next("GET", "/api/f/juniper--k3x9/file?path=agent.json").fail(Object.assign(new Error("deploying"), { status: 503 }));
  await settle();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "open", "Old a is on finite.computer.")] });
  await settle();
  assert.equal(p.made.length, 0, "no agent made");
  assert.equal(p.calls.filter((c) => c.path.endsWith("/yes")).length, 0, "no Yes");
  assert.deepEqual(p.errors(), ["Couldn't check whether Old a was brought over already, so nothing was made. Try again."]);
  assert.equal(p.button("Bring over").disabled, false, "to try again");
});

test("a choice made again after none was: Bring over is offered again", async (t) => {
  const p = page(t);
  p.ui.open();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "open", "Old a is on finite.computer.")] });
  await settle();
  const box = () => walk(p.root).find((n) => n.tag === "input" && n.type === "checkbox");
  box().checked = false;
  box().onchange();
  assert.equal(p.button("Bring over").disabled, true, "nothing chosen");
  box().checked = true;
  box().onchange();
  assert.equal(p.button("Bring over").disabled, false, "chosen again");
});

// FIN-463: once a move imports, one calm line says where its SimpleX chat
// is kept; before, none
test("a move that imports says, once, that its SimpleX chat is kept in Settings", async (t) => {
  const p = page(t);
  const line = () => walk(p.root).filter((n) => n.className.includes("move-simplex")).map((n) => n.textContent);
  p.ui.open();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "open", "Old a is on finite.computer."), offer("c", "staging", "Copying Old c…")] });
  await settle();
  assert.deepEqual(line(), [], "nothing carried yet");
  p.timers.due();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("c", "importing", "Bringing Old c over…"), offer("d", "moved", "Old d is here.")] });
  await settle();
  assert.deepEqual(line(), ["If you chatted with it on SimpleX, keep that chat in Settings."]);
});

test("a choice keeps the page as it is, the box and its focus with it: only Bring over follows it; the box is the platform's check", async (t) => {
  const p = page(t);
  p.ui.open();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "open", "Old a is on finite.computer.")] });
  await settle();
  const box = walk(p.root).find((n) => n.tag === "input" && n.type === "checkbox");
  assert.ok(box.className.split(" ").includes("check"), "drawn as the shell's check (shell.css `input.check`)");
  box.checked = false;
  box.onchange();
  assert.equal(walk(p.root).find((n) => n.tag === "input" && n.type === "checkbox"), box, "the same box: nothing drawn again under the person's focus");
  assert.equal(p.button("Bring over").disabled, true, "nothing chosen");
  box.checked = true;
  box.onchange();
  assert.equal(walk(p.root).find((n) => n.tag === "input" && n.type === "checkbox"), box);
  assert.equal(p.button("Bring over").disabled, false, "chosen");
});

// FIN-463: the first move on production imported, and its new agent asked
// its person a question in its own chat, while this page said "Bringing
// it over…". Once an import starts, its agent's chat is where its person
// is, or one tap away.
test("an import that starts while the page shows takes the person to its new agent's chat; each row with a new agent opens that agent's chat", async (t) => {
  const own = { agents: ["juniper--k3x9", "fern--p2q8"], ids: new Map(), chats: new Map([["npub1juniper", "juniper-chat--k3x9"], ["npub1fern", "fern-chat--p2q8"]]) };
  const p = page(t, own);
  p.ui.open();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "staging", "Copying Old a…", { agent: "npub1juniper" }), offer("b", "importing", "Bringing Old b over…", { agent: "npub1fern" })] });
  await settle();
  assert.deepEqual(p.opened, [], "one already importing as the page opens is the person's to open: they came here to see it");
  assert.deepEqual(p.rows().map((r) => r.buttons), [["Open chat"], ["Open chat"]], "each a tap from its new agent's chat");
  p.button("Open chat").onclick();
  assert.deepEqual(p.opened, ["juniper-chat--k3x9"], "its own chat");
  p.opened.length = 0;
  p.ui.open();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "staging", "Copying Old a…", { agent: "npub1juniper" }), offer("b", "importing", "Bringing Old b over…", { agent: "npub1fern" })] });
  await settle();
  const due = p.timers.due;
  p.timers.due = null;
  due();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "importing", "Bringing Old a over…", { agent: "npub1juniper" }), offer("b", "importing", "Bringing Old b over…", { agent: "npub1fern" })] });
  await settle();
  assert.deepEqual(p.opened, ["juniper-chat--k3x9"], "its import began as the page showed: to its agent's chat, where it asks and says how it goes");
  assert.equal(p.timers.due, null, "the page read no more");
  assert.equal(p.calls.length, 0);
});

test("Bring over into a move that imports at once (a kept stage's): the person is taken to the new agent's chat once its Yes is posted", async (t) => {
  const own = { agents: [], ids: new Map(), chats: new Map([["npub1juniper", "juniper-chat--k3x9"]]) };
  const p = page(t, own);
  p.ui.open();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "archived", "Old a is kept.")] });
  await settle();
  p.button("Bring over").onclick();
  await settle();
  p.next("POST", "/api/migrations/offers").ok({ offers: [] });
  await settle();
  p.next("GET", "/api/ledger").ok({ plan: "seat" });
  await settle();
  p.made[0].ok({ chat: "juniper-chat--k3x9", id: "npub1juniper", computer: "computer:aa" });
  await settle();
  p.next("POST", "/api/migrations/offers/a/yes").ok({ state: "importing" });
  await settle();
  assert.deepEqual(p.opened, [], "not while Bring over is under way");
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "importing", "Bringing Old a over…", { agent: "npub1juniper" })] });
  await settle();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "importing", "Bringing Old a over…", { agent: "npub1juniper" })] });
  await settle();
  assert.deepEqual(p.opened, ["juniper-chat--k3x9"]);
});
