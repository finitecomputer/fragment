// cell/shell/frame-standin.js (Paul, 2026-11-01: no blank page while a
// woken chat loads): a middle-column frame's stand-in shows from its
// making until its page has loaded (never on the empty page a frame holds
// before its `src`) or spoken to the shell, whichever comes first; it
// follows the frame's hiding, and comes back when the frame is sent
// elsewhere, until that page comes.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { standIn } from "../shell/frame-standin.js";

// a column of nodes (#frames) and the elements the stand-in makes and moves
function column() {
  const nodes = [];
  const node = (cls) => {
    const n = {
      className: cls ?? "", children: [], hidden: false, attrs: {},
      get isConnected() { return nodes.includes(n); },
      setAttribute(k, v) { n.attrs[k] = v; },
      getAttribute(k) { return n.attrs[k] ?? null; },
      append(...c) { n.children.push(...c); },
      remove() { const i = nodes.indexOf(n); if (i >= 0) nodes.splice(i, 1); },
      after(x) { if (!nodes.includes(x)) nodes.splice(nodes.indexOf(n) + 1, 0, x); },
      listeners: new Map(),
      addEventListener(type, f) { (n.listeners.get(type) ?? n.listeners.set(type, new Set()).get(type)).add(f); },
      removeEventListener(type, f) { n.listeners.get(type)?.delete(f); },
      dispatch(type) { for (const f of [...(n.listeners.get(type) ?? [])]) f(); },
    };
    return n;
  };
  const el = (tag, cls) => node(cls);
  const frame = node("frame");
  nodes.push(frame);
  return { nodes, el, frame };
}

test("shown from the frame's making; the empty page before its src loading is not its page", () => {
  const { el, frame } = column();
  const s = standIn(frame, el);
  frame.after(s.box);
  assert.equal(s.shown(), true);
  assert.equal(s.box.getAttribute("aria-hidden"), "true");
  frame.dispatch("load");
  assert.equal(s.shown(), true, "an about:blank load leaves it");
});

test("its page loaded: it goes, and a later load changes nothing", () => {
  const { el, frame } = column();
  const s = standIn(frame, el);
  frame.after(s.box);
  frame.setAttribute("src", "/auth/frame?name=talk--k3x9&return=%2F");
  frame.dispatch("load");
  assert.equal(s.shown(), false);
  frame.dispatch("load");
  assert.equal(s.shown(), false);
});

test("its page spoke to the shell first: it goes then, before its load", () => {
  const { el, frame } = column();
  const s = standIn(frame, el);
  frame.after(s.box);
  frame.setAttribute("src", "/auth/frame?name=talk--k3x9&return=%2F");
  s.settled();
  assert.equal(s.shown(), false);
});

test("the frame sent elsewhere: shown again, hidden as the frame is, until that page loads", () => {
  const { el, frame } = column();
  const s = standIn(frame, el);
  frame.after(s.box);
  frame.setAttribute("src", "/a");
  frame.dispatch("load");
  frame.hidden = true;
  s.again();
  assert.equal(s.shown(), true);
  assert.equal(s.box.hidden, true);
  s.again();
  assert.equal(frame.listeners.get("load").size, 1, "armed once however often it is shown");
  frame.dispatch("load");
  assert.equal(s.shown(), false);
});
