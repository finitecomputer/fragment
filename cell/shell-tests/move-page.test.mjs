// cell/shell/move-page.js, the move's page (FIN-463 F6): a person's old
// agents in the server's words, the one question (which, by what name),
// then for each a new agent made as Settings' Add makes one and its Yes
// posted; for someone with agents here already, a new one or one of
// theirs, asked; a refusal in the server's words; Ask again after a Never;
// a move under way read again. The real `createMovePage`, its page faked;
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

function page(t, own = { agents: [], ids: new Map(), chats: new Map(), names: new Map() }) {
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
    named: (node, fragment, make) => ((node.textContent = make(own.names?.get(fragment) ?? fragment)), node),
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
  // a row's question, as drawn: its words, each answer and whether it is
  // the one chosen, and whether it shows
  const question = (old) => {
    const li = walk(root).find((n) => n.tag === "li" && n.dataset.old === old);
    const q = li && walk(li).find((n) => n.className === "move-into");
    if (!q) return null;
    const answers = walk(q).filter((n) => n.tag === "label").map((l) => ({ said: l.children[1].textContent, chosen: l.children[0].checked, value: l.children[0].value, type: l.children[0].type }));
    return { said: walk(q).find((n) => n.tag === "p").textContent, answers, hidden: q.hidden, role: q.attrs.role };
  };
  // the answer `said` for `old`'s question, chosen as a person does
  const choose = (old, said) => {
    const li = walk(root).find((n) => n.tag === "li" && n.dataset.old === old);
    const r = walk(li).find((n) => n.tag === "label" && n.children[1].textContent === said).children[0];
    for (const x of walk(li).filter((n) => n.tag === "input" && n.type === "radio")) x.checked = x === r;
    r.onchange();
  };
  return { ui, root, calls, made, timers, rows, button, errors, next, opened, guest: () => guest, question, choose };
}

const offer = (old, state, said, o = {}) => ({ old, name: `Old ${old}`, state, said, ...o });

test("its agents in the server's words: the askable ones each a choice, a Never'd one Ask again, one under way neither", async (t) => {
  const p = page(t);
  p.ui.open();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "open", "Old a\'s upgrade is ready.", { image: "/api/migrations/offers/a/picture" }), offer("b", "never", "You said never."), offer("c", "staging", "Packing up Old c…")] });
  await settle();
  assert.deepEqual(p.rows(), [
    { old: "a", words: ["Old a", "Old a\'s upgrade is ready."], pick: true, buttons: [] },
    { old: "b", words: ["Old b", "You said never."], pick: false, buttons: ["Ask again"] },
    { old: "c", words: ["Old c", "Packing up Old c…"], pick: false, buttons: [] },
  ]);
  assert.equal(walk(p.root).find((n) => n.tag === "img")?.dataset.src, "/api/migrations/offers/a/picture");
  assert.ok(p.button("Upgrade"), "the one question's answer");
  assert.ok(p.timers.due, "a move under way is read again");
  p.timers.due();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("c", "importing", "Upgrading Old c…")] });
  await settle();
  assert.deepEqual(p.rows()[0].words, ["Old c", "Upgrading Old c…"], "as the server says it now");
});

test("Upgrade: the move opened (the comp), a seat waited for, then each a new agent made quietly and its Yes posted with it, the page read again; nothing said ahead of the server", async (t) => {
  const p = page(t);
  p.ui.open();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "open", "Old a\'s upgrade is ready.")] });
  await settle();
  p.button("Upgrade").onclick();
  await settle();
  p.next("POST", "/api/migrations/offers").ok({ offers: [] });
  await settle();
  p.next("GET", "/api/ledger").ok({ plan: "seat" });
  await settle();
  assert.equal(p.guest(), false, "a seat, as the platform said");
  assert.deepEqual(p.made.map((m) => m.spec), [{ name: "Old a", role: null, quiet: true, from: "a", agent: null }], "made for its old agent, which its agent.json keeps, under its name");
  assert.deepEqual(p.rows()[0].words, ["Old a", "Preparing Old a's upgrade…"], "while it is made");
  p.made[0].ok({ chat: "juniper-chat--k3x9", id: "npub1juniper", computer: "computer:aa" });
  await settle();
  const yes = p.next("POST", "/api/migrations/offers/a/yes");
  assert.deepEqual(yes.body, { agent: "npub1juniper" });
  yes.ok({ state: "asked" });
  await settle();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "asked", "Pausing Old a for its upgrade…")] });
  await settle();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "asked", "Pausing Old a for its upgrade…")] });
  await settle();
  assert.deepEqual(p.rows(), [{ old: "a", words: ["Old a", "Pausing Old a for its upgrade…"], pick: false, buttons: [] }]);
  assert.equal(p.button("Upgrade"), undefined, "nothing left to ask");
});

test("a refusal is the server's words, the page as read, to try again", async (t) => {
  const p = page(t);
  p.ui.open();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "open", "Old a\'s upgrade is ready.")] });
  await settle();
  p.button("Upgrade").onclick();
  await settle();
  p.next("POST", "/api/migrations/offers").fail(Object.assign(new Error("this person signed in with no WorkOS user"), { status: 409 }));
  await settle();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "open", "Old a\'s upgrade is ready.")] });
  await settle();
  assert.deepEqual(p.errors(), ["this person signed in with no WorkOS user"]);
  assert.equal(p.made.length, 0, "no agent made");
  assert.equal(p.button("Upgrade").disabled, false, "to try again");
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
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("b", "open", "Old b\'s upgrade is ready.")] });
  await settle();
  assert.deepEqual(p.rows()[0].pick, true);
});

test("one new agent an old one: the agent made for it before (a Yes that never arrived, a making cut short, a reload) is handed back to Add to go on with, never a second", async (t) => {
  for (const placed of [true, false]) {
    const p = page(t, { agents: ["mine--a1b2", "juniper--k3x9"], ids: new Map([["mine--a1b2", "npub1mine"], ...(placed ? [["juniper--k3x9", "npub1juniper"]] : [])]) });
    const reads = () => {
      p.next("GET", "/api/f/mine--a1b2/file?path=agent.json").fail(Object.assign(new Error("not found"), { status: 404 }));
      p.next("GET", "/api/f/juniper--k3x9/file?path=agent.json").ok({ tier: "standard", from: "a" });
    };
    p.ui.open();
    p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "open", "Old a\'s upgrade is ready.")] });
    await settle();
    reads();
    await settle();
    assert.equal(p.question("a"), null, "an agent was made for it: that is its answer, and nothing is asked");
    p.button("Upgrade").onclick();
    await settle();
    reads();
    await settle();
    p.next("POST", "/api/migrations/offers").ok({ offers: [] });
    await settle();
    p.next("GET", "/api/ledger").ok({ plan: "seat" });
    await settle();
    assert.deepEqual(p.made.map((m) => m.spec), [{ name: "Old a", role: null, quiet: true, from: "a", agent: "juniper--k3x9" }], `the one made for it, handed back (${placed ? "on its computer" : "cut short"})`);
    p.made[0].ok({ chat: "juniper-chat--k3x9", id: "npub1juniper", computer: "computer:aa" });
    await settle();
    assert.deepEqual(p.next("POST", "/api/migrations/offers/a/yes").body, { agent: "npub1juniper" });
  }
});

test("a mapping that could not be read is no leave to make one: Upgrade stops, saying so, to try again, and makes nothing", async (t) => {
  const p = page(t, { agents: ["juniper--k3x9"], ids: new Map([["juniper--k3x9", "npub1juniper"]]) });
  const unread = () => p.next("GET", "/api/f/juniper--k3x9/file?path=agent.json").fail(Object.assign(new Error("deploying"), { status: 503 }));
  p.ui.open();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "open", "Old a\'s upgrade is ready.")] });
  await settle();
  unread();
  await settle();
  assert.equal(p.question("a"), null, "nothing read to ask about");
  p.button("Upgrade").onclick();
  await settle();
  unread();
  await settle();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "open", "Old a\'s upgrade is ready.")] });
  await settle();
  unread();
  await settle();
  assert.equal(p.made.length, 0, "no agent made");
  assert.equal(p.calls.filter((c) => c.path.endsWith("/yes") || c.path === "/api/migrations/offers").length, 0, "no Yes, nor the move opened");
  assert.deepEqual(p.errors(), ["Couldn't check your agents, so nothing changed. Try again."]);
  assert.equal(p.button("Upgrade").disabled, false, "to try again");
});

test("a choice made again after none was: Upgrade is offered again", async (t) => {
  const p = page(t);
  p.ui.open();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "open", "Old a\'s upgrade is ready.")] });
  await settle();
  const box = () => walk(p.root).find((n) => n.tag === "input" && n.type === "checkbox");
  box().checked = false;
  box().onchange();
  assert.equal(p.button("Upgrade").disabled, true, "nothing chosen");
  box().checked = true;
  box().onchange();
  assert.equal(p.button("Upgrade").disabled, false, "chosen again");
});

// FIN-463: once a move imports, one calm line says where its SimpleX chat
// is kept; before, none
test("a move that imports says, once, that its SimpleX chat is kept in Settings", async (t) => {
  const p = page(t);
  const line = () => walk(p.root).filter((n) => n.className.includes("move-simplex")).map((n) => n.textContent);
  p.ui.open();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "open", "Old a\'s upgrade is ready."), offer("c", "staging", "Packing up Old c…")] });
  await settle();
  assert.deepEqual(line(), [], "nothing carried yet");
  p.timers.due();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("c", "importing", "Upgrading Old c…"), offer("d", "moved", "Upgraded.")] });
  await settle();
  assert.deepEqual(line(), ["If you chatted with it on SimpleX, keep that chat in Settings."]);
});

test("a choice keeps the page as it is, the box and its focus with it: only Upgrade follows it; the box is the platform's check", async (t) => {
  const p = page(t);
  p.ui.open();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "open", "Old a\'s upgrade is ready.")] });
  await settle();
  const box = walk(p.root).find((n) => n.tag === "input" && n.type === "checkbox");
  assert.ok(box.className.split(" ").includes("check"), "drawn as the shell's check (shell.css `input.check`)");
  box.checked = false;
  box.onchange();
  assert.equal(walk(p.root).find((n) => n.tag === "input" && n.type === "checkbox"), box, "the same box: nothing drawn again under the person's focus");
  assert.equal(p.button("Upgrade").disabled, true, "nothing chosen");
  box.checked = true;
  box.onchange();
  assert.equal(walk(p.root).find((n) => n.tag === "input" && n.type === "checkbox"), box);
  assert.equal(p.button("Upgrade").disabled, false, "chosen");
});

// FIN-463: the first move on production imported, and its new agent asked
// its person a question in its own chat, while this page said "Bringing
// it over…". Once an import starts, its agent's chat is where its person
// is, or one tap away.
test("an import that starts while the page shows takes the person to its new agent's chat; each row with a new agent opens that agent's chat", async (t) => {
  const own = { agents: ["juniper--k3x9", "fern--p2q8"], ids: new Map(), chats: new Map([["npub1juniper", "juniper-chat--k3x9"], ["npub1fern", "fern-chat--p2q8"]]) };
  const p = page(t, own);
  p.ui.open();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "staging", "Packing up Old a…", { agent: "npub1juniper" }), offer("b", "importing", "Upgrading Old b…", { agent: "npub1fern" })] });
  await settle();
  assert.deepEqual(p.opened, [], "one already importing as the page opens is the person's to open: they came here to see it");
  assert.deepEqual(p.rows().map((r) => r.buttons), [["Open chat"], ["Open chat"]], "each a tap from its new agent's chat");
  p.button("Open chat").onclick();
  assert.deepEqual(p.opened, ["juniper-chat--k3x9"], "its own chat");
  p.opened.length = 0;
  p.ui.open();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "staging", "Packing up Old a…", { agent: "npub1juniper" }), offer("b", "importing", "Upgrading Old b…", { agent: "npub1fern" })] });
  await settle();
  const due = p.timers.due;
  p.timers.due = null;
  due();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "importing", "Upgrading Old a…", { agent: "npub1juniper" }), offer("b", "importing", "Upgrading Old b…", { agent: "npub1fern" })] });
  await settle();
  assert.deepEqual(p.opened, ["juniper-chat--k3x9"], "its import began as the page showed: to its agent's chat, where it asks and says how it goes");
  assert.equal(p.timers.due, null, "the page read no more");
  assert.equal(p.calls.length, 0);
});

test("Upgrade into a move that imports at once (a kept stage's): the person is taken to the new agent's chat once its Yes is posted", async (t) => {
  const own = { agents: [], ids: new Map(), chats: new Map([["npub1juniper", "juniper-chat--k3x9"]]) };
  const p = page(t, own);
  p.ui.open();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "archived", "Old a is kept.")] });
  await settle();
  p.button("Upgrade").onclick();
  await settle();
  p.next("POST", "/api/migrations/offers").ok({ offers: [] });
  await settle();
  p.next("GET", "/api/ledger").ok({ plan: "seat" });
  await settle();
  p.made[0].ok({ chat: "juniper-chat--k3x9", id: "npub1juniper", computer: "computer:aa" });
  await settle();
  p.next("POST", "/api/migrations/offers/a/yes").ok({ state: "importing" });
  await settle();
  assert.deepEqual(p.opened, [], "not while Upgrade is under way");
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "importing", "Upgrading Old a…", { agent: "npub1juniper" })] });
  await settle();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "importing", "Upgrading Old a…", { agent: "npub1juniper" })] });
  await settle();
  assert.deepEqual(p.opened, ["juniper-chat--k3x9"]);
});

test("a read the person left before it answered is not this opening's: an import they come back to is theirs to open, never a jump", async (t) => {
  const own = { agents: [], ids: new Map(), chats: new Map([["npub1juniper", "juniper-chat--k3x9"]]) };
  const p = page(t, own);
  p.ui.open();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "staging", "Packing up Old a…", { agent: "npub1juniper" })] });
  await settle();
  p.timers.due();
  const stale = p.next("GET", "/api/migrations/offers");
  p.ui.close();
  p.ui.open();
  const fresh = p.next("GET", "/api/migrations/offers");
  stale.ok({ offers: [offer("a", "staging", "Packing up Old a…", { agent: "npub1juniper" })] });
  await settle();
  fresh.ok({ offers: [offer("a", "importing", "Upgrading Old a…", { agent: "npub1juniper" })] });
  await settle();
  p.timers.due();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "importing", "Upgrading Old a…", { agent: "npub1juniper" })] });
  await settle();
  assert.deepEqual(p.opened, [], "importing as they came back: the page stays");
  assert.deepEqual(p.rows()[0].buttons, ["Open chat"]);
});

test("reads that fail as the person comes back are no news: a move importing before is never taken for one just begun", async (t) => {
  const own = { agents: [], ids: new Map(), chats: new Map([["npub1juniper", "juniper-chat--k3x9"]]) };
  const p = page(t, own);
  p.ui.open();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "importing", "Upgrading Old a…", { agent: "npub1juniper" })] });
  await settle();
  p.ui.close();
  p.ui.open();
  p.next("GET", "/api/migrations/offers").fail(Object.assign(new Error("offline"), { status: 503 }));
  await settle();
  p.timers.due();
  p.next("GET", "/api/migrations/offers").fail(Object.assign(new Error("offline"), { status: 503 }));
  await settle();
  assert.deepEqual(p.opened, [], "no read of this opening answered: nothing is new");
  p.timers.due();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "importing", "Upgrading Old a…", { agent: "npub1juniper" })] });
  await settle();
  assert.deepEqual(p.opened, [], "importing at this opening's first answer: the page stays");
  assert.deepEqual(p.rows()[0].buttons, ["Open chat"]);
});

test("an opening the person left before its first read answered goes no further: no read again on its behalf", async (t) => {
  const p = page(t);
  p.ui.open();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "staging", "Packing up Old a…", { agent: "npub1juniper" })] });
  await settle();
  p.ui.close();
  p.timers.due = null;
  p.ui.open();
  const stale = p.next("GET", "/api/migrations/offers");
  p.ui.close();
  p.ui.open();
  const fresh = p.next("GET", "/api/migrations/offers");
  stale.ok({ offers: [offer("a", "staging", "Packing up Old a…", { agent: "npub1juniper" })] });
  await settle();
  assert.equal(p.timers.due, null, "nothing read again on the old opening's word");
  fresh.ok({ offers: [offer("a", "staging", "Packing up Old a…", { agent: "npub1juniper" })] });
  await settle();
  assert.ok(p.timers.due, "the opening's own read follows the move");
});

// Paul 11-19: someone who moved by hand has an agent here already, and
// Upgrade made them a second. Now they are asked, an old agent at a time:
// a new one, or one of theirs, nothing chosen for them.
const pip = () => ({ agents: ["pip--a1b2"], ids: new Map([["pip--a1b2", "npub1pip"]]), chats: new Map(), names: new Map([["pip--a1b2", "Pip"]]) });
const READY = "Old a's upgrade is ready.";
const NEW = "Make a new one";
const ASKED = "You already have an agent here. Make a new one for this upgrade, or use Pip?";
const notFound = () => Object.assign(new Error("not found"), { status: 404 });
// an agent.json that could not be read (not missing)
const UNREAD = "unread";
const answerRead = (c, json) => (json === UNREAD ? c.fail(Object.assign(new Error("deploying"), { status: 503 })) : json ? c.ok(json) : c.fail(notFound()));
// the page opened on old agent a: its offer, then their agents read
async function opened(p, agents, offers = [offer("a", "open", READY)]) {
  p.ui.open();
  p.next("GET", "/api/migrations/offers").ok({ offers });
  await settle();
  for (const [fragment, json] of agents) {
    answerRead(p.next("GET", `/api/f/${fragment}/file?path=agent.json`), json);
  }
  await settle();
}
// Upgrade pressed: their agents read again, whole, then the move opened
// and a seat waited for
async function upgraded(p, agents) {
  p.button("Upgrade").onclick();
  await settle();
  for (const [fragment, json] of agents) {
    answerRead(p.next("GET", `/api/f/${fragment}/file?path=agent.json`), json);
  }
  await settle();
  p.next("POST", "/api/migrations/offers").ok({ offers: [] });
  await settle();
  p.next("GET", "/api/ledger").ok({ plan: "seat" });
  await settle();
}

test("someone with an agent here is asked, an old agent at a time: a new one, or theirs by its name; nothing chosen for them, and Upgrade waits for the answer", async (t) => {
  const p = page(t, pip());
  await opened(p, [["pip--a1b2", { tier: "standard" }]]);
  assert.deepEqual(p.question("a"), {
    said: ASKED,
    answers: [{ said: NEW, chosen: false, value: "", type: "radio" }, { said: "Use Pip", chosen: false, value: "pip--a1b2", type: "radio" }],
    hidden: false,
    role: "radiogroup",
  });
  assert.equal(p.button("Upgrade").disabled, true, "not answered yet");
  p.choose("a", "Use Pip");
  assert.equal(p.button("Upgrade").disabled, false, "answered");
  const box = walk(p.root).find((n) => n.tag === "input" && n.type === "checkbox");
  box.checked = false;
  box.onchange();
  assert.equal(p.question("a").hidden, true, "not to be upgraded: nothing to ask");
  box.checked = true;
  box.onchange();
  assert.equal(p.question("a").hidden, false);
  assert.equal(p.question("a").answers[1].chosen, true, "the answer kept");
  assert.equal(p.made.length + p.calls.length, 0, "nothing made or asked of the server while they choose");
});

test("one of theirs chosen: the move opened, a seat, then its Yes with that agent, as it is: nothing made, nothing written to it", async (t) => {
  const p = page(t, pip());
  const agents = [["pip--a1b2", { tier: "standard" }]];
  await opened(p, agents);
  p.choose("a", "Use Pip");
  await upgraded(p, agents);
  assert.deepEqual(p.rows()[0].words.slice(0, 2), ["Old a", "Starting Old a's upgrade…"]);
  const yes = p.next("POST", "/api/migrations/offers/a/yes");
  assert.deepEqual(yes.body, { agent: "npub1pip" });
  yes.ok({ state: "asked" });
  await settle();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "asked", "Pausing Old a for its upgrade…", { agent: "npub1pip" })] });
  await settle();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "asked", "Pausing Old a for its upgrade…", { agent: "npub1pip" })] });
  await settle();
  assert.equal(p.made.length, 0, "no agent made");
  assert.equal(p.calls.length, 0, "and nothing else asked: no write to Pip, no other agent's Yes");
  assert.equal(p.question("a"), null, "bound: nothing left to ask");
  assert.equal(p.button("Upgrade"), undefined);
});

test("Make a new one: as without an agent here, a new one made for it and its Yes posted with that one", async (t) => {
  const p = page(t, pip());
  const agents = [["pip--a1b2", { tier: "standard" }]];
  await opened(p, agents);
  p.choose("a", NEW);
  await upgraded(p, agents);
  assert.deepEqual(p.made.map((m) => m.spec), [{ name: "Old a", role: null, quiet: true, from: "a", agent: null }], "a new one, made for its old agent");
  p.made[0].ok({ chat: "old-a-chat--k3x9", id: "npub1olda", computer: "computer:aa" });
  await settle();
  assert.deepEqual(p.next("POST", "/api/migrations/offers/a/yes").body, { agent: "npub1olda" });
});

test("no agent here: no question, and Upgrade goes on as it always has", async (t) => {
  const p = page(t);
  await opened(p, []);
  assert.equal(p.question("a"), null);
  assert.equal(p.button("Upgrade").disabled, false);
});

test("a Yes into theirs that failed: tried again into the same one; a reload asks again, nothing chosen; never an agent made unasked", async (t) => {
  const p = page(t, pip());
  const agents = [["pip--a1b2", { tier: "standard" }]];
  await opened(p, agents);
  p.choose("a", "Use Pip");
  await upgraded(p, agents);
  p.next("POST", "/api/migrations/offers/a/yes").fail(Object.assign(new Error("Couldn't reach the server."), { status: 0 }));
  await settle();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "open", READY)] });
  await settle();
  p.next("GET", "/api/f/pip--a1b2/file?path=agent.json").ok({ tier: "standard" });
  await settle();
  assert.deepEqual(p.errors(), ["Couldn't reach the server."]);
  assert.equal(p.question("a").answers[1].chosen, true, "the answer kept, to try again");
  await upgraded(p, agents);
  assert.deepEqual(p.next("POST", "/api/migrations/offers/a/yes").body, { agent: "npub1pip" }, "again, into Pip");
  assert.equal(p.made.length, 0, "no agent made");
  // a reload (the page made again) before any Yes landed: asked again
  const again = page(t, pip());
  await opened(again, agents);
  assert.deepEqual(again.question("a").answers.map((a) => a.chosen), [false, false], "nothing decided until a Yes lands: asked again");
  assert.equal(again.button("Upgrade").disabled, true);
});

test("once a new one was made for it (its from), a reload asks nothing: Upgrade goes on with that agent, never a second", async (t) => {
  const own = { ...pip(), agents: ["pip--a1b2", "old-a--k3x9"], ids: new Map([["pip--a1b2", "npub1pip"], ["old-a--k3x9", "npub1olda"]]) };
  const p = page(t, own);
  const agents = [["pip--a1b2", { tier: "standard" }], ["old-a--k3x9", { tier: "standard", from: "a" }]];
  await opened(p, agents);
  assert.equal(p.question("a"), null);
  await upgraded(p, agents);
  assert.deepEqual(p.made.map((m) => m.spec.agent), ["old-a--k3x9"], "the one made for it, gone on with");
});

test("an agent a move may still import into, one made for another old agent still to answer, and one not on Hermes are no one's choice", async (t) => {
  const own = {
    agents: ["pip--a1b2", "busy--c3d4", "old-b--e5f6", "claude--g7h8", "unplaced--j9k2"],
    ids: new Map([["pip--a1b2", "npub1pip"], ["busy--c3d4", "npub1busy"], ["old-b--e5f6", "npub1oldb"], ["claude--g7h8", "npub1claude"]]),
    names: new Map([["pip--a1b2", "Pip"]]),
  };
  const p = page(t, own);
  await opened(p, [["pip--a1b2", { tier: "standard" }], ["busy--c3d4", { tier: "standard" }], ["old-b--e5f6", { tier: "standard", from: "b" }], ["claude--g7h8", { runtime: "claude" }], ["unplaced--j9k2", null]], [
    offer("a", "open", READY),
    offer("b", "open", "Old b's upgrade is ready."),
    offer("c", "importing", "Upgrading Old c…", { agent: "npub1busy" }),
  ]);
  assert.deepEqual(p.question("a").answers.map((a) => a.said), [NEW, "Use Pip"], "Pip alone");
  assert.equal(p.question("b"), null, "b has its new one already");
});

test("several of theirs: one question, each by name; one agent takes one upgrade at a time, so choosing it for another old agent takes it from the first", async (t) => {
  const own = { agents: ["pip--a1b2", "rook--c3d4"], ids: new Map([["pip--a1b2", "npub1pip"], ["rook--c3d4", "npub1rook"]]), names: new Map([["pip--a1b2", "Pip"], ["rook--c3d4", "Rook"]]) };
  const p = page(t, own);
  await opened(p, [["pip--a1b2", { tier: "standard" }], ["rook--c3d4", { tier: "standard" }]], [offer("a", "open", READY), offer("b", "open", "Old b's upgrade is ready.")]);
  assert.equal(p.question("a").said, "You already have agents here. Make a new one for this upgrade, or use one of yours?");
  assert.deepEqual(p.question("a").answers.map((a) => a.said), [NEW, "Use Pip", "Use Rook"]);
  p.choose("a", "Use Pip");
  p.choose("b", "Use Pip");
  assert.deepEqual(p.question("a").answers.map((a) => a.chosen), [false, false, false], "taken from a");
  assert.equal(p.button("Upgrade").disabled, true, "a to answer again");
  p.choose("a", "Use Rook");
  assert.equal(p.button("Upgrade").disabled, false);
  // both Make a new one: nothing taken
  p.choose("a", NEW);
  p.choose("b", NEW);
  assert.deepEqual([p.question("a").answers[0].chosen, p.question("b").answers[0].chosen], [true, true]);
});

test("an agent of theirs the page could not read as it opened, read at Upgrade, is asked about first: nothing made unasked", async (t) => {
  const p = page(t, pip());
  // its read as the page opened failed (not missing): left out of the question
  await opened(p, [["pip--a1b2", UNREAD]]);
  assert.equal(p.question("a"), null, "nothing read to ask about");
  p.button("Upgrade").onclick();
  await settle();
  p.next("GET", "/api/f/pip--a1b2/file?path=agent.json").ok({ tier: "standard" });
  await settle();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "open", READY)] });
  await settle();
  p.next("GET", "/api/f/pip--a1b2/file?path=agent.json").ok({ tier: "standard" });
  await settle();
  assert.equal(p.made.length, 0, "no agent made");
  assert.equal(p.calls.length, 0, "no Yes, nor the move opened");
  assert.deepEqual(p.errors(), []);
  assert.equal(p.question("a").said, ASKED, "asked now");
  assert.equal(p.button("Upgrade").disabled, true, "to answer first");
});
