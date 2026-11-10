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

function page(t, own = { agents: [], ids: new Map() }) {
  const root = new Node("section");
  const calls = [];
  const made = [];
  const timers = { setTimeout: (fn) => (timers.due = fn, 1), clearTimeout: () => (timers.due = null), due: null };
  const api = (method, path, body) => new Promise((ok, fail) => calls.push({ method, path, body, ok, fail }));
  let guest = true;
  const ui = createMovePage(root, {
    api, el: (tag, cls, text) => new Node(tag, cls, text), timers,
    setGuest: (g) => (guest = g),
    addAgent: (spec) => new Promise((ok, fail) => made.push({ spec, ok, fail })),
    reload: () => {},
    ownAgents: () => own.agents, identityOf: (f) => own.ids.get(f) ?? null,
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
  return { ui, root, calls, made, timers, rows, button, errors, next, guest: () => guest };
}

const offer = (old, state, said, o = {}) => ({ old, name: `Old ${old}`, state, said, ...o });

test("its agents in the server's words: the askable ones each a choice and a name, a Never'd one Ask again, one under way neither", async (t) => {
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
  const name = walk(p.root).find((n) => n.className === "move-name");
  name.value = "Juniper";
  name.oninput();
  p.button("Bring over").onclick();
  await settle();
  p.next("POST", "/api/migrations/offers").ok({ offers: [] });
  await settle();
  p.next("GET", "/api/ledger").ok({ plan: "seat" });
  await settle();
  assert.equal(p.guest(), false, "a seat, as the platform said");
  assert.deepEqual(p.made.map((m) => m.spec), [{ name: "Juniper", role: null, quiet: true, from: "a" }], "made for its old agent, which its agent.json keeps");
  assert.deepEqual(p.rows()[0].words, ["Old a", "Making Juniper…"], "while it is made");
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

test("one new agent an old one: an agent made for it before whose Yes never arrived (or a reload) is gone on with, never a second made", async (t) => {
  const p = page(t, { agents: ["mine--a1b2", "juniper--k3x9"], ids: new Map([["mine--a1b2", "npub1mine"], ["juniper--k3x9", "npub1juniper"]]) });
  p.ui.open();
  p.next("GET", "/api/migrations/offers").ok({ offers: [offer("a", "open", "Old a is on finite.computer.")] });
  await settle();
  p.button("Bring over").onclick();
  await settle();
  p.next("POST", "/api/migrations/offers").ok({ offers: [] });
  await settle();
  p.next("GET", "/api/ledger").ok({ plan: "seat" });
  await settle();
  p.next("GET", "/api/f/mine--a1b2/file?path=agent.json").ok({ tier: "standard" });
  await settle();
  p.next("GET", "/api/f/juniper--k3x9/file?path=agent.json").ok({ tier: "standard", from: "a" });
  await settle();
  assert.equal(p.made.length, 0, "no second agent made");
  const yes = p.next("POST", "/api/migrations/offers/a/yes");
  assert.deepEqual(yes.body, { agent: "npub1juniper" }, "the one made for it");
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
