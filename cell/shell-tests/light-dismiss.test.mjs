// cell/shell/dialog.js's press outside a modal card, in an app's icon
// sheet (app-icon.js): a press in the card's own padding, and words
// selected from its field past its edge, leave it open with what was
// typed; a press outside closes it, on its click. The e2e's
// `light_dismiss_ui` presses the same in Chrome.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { createIconEditor } from "../shell/app-icon.js";

// the sheet's card, as the page lays it out
const CARD = { left: 430, right: 850, top: 200, bottom: 600 };

class Node {
  constructor(tag) {
    Object.assign(this, { tag, className: "", textContent: "", children: [], dataset: {}, attrs: {}, hidden: false, listeners: [] });
    this.classList = { add() {}, remove() {}, toggle() {} };
    this.style = { setProperty() {} };
  }
  append(...c) {
    this.children.push(...c);
  }
  replaceChildren(...c) {
    this.children = [...c];
  }
  replaceWith() {}
  setAttribute(k, v) {
    this.attrs[k] = v;
  }
  removeAttribute(k) {
    delete this.attrs[k];
  }
  addEventListener(type, fn, capture = false) {
    this.listeners.push({ type, fn, capture });
  }
  remove() {
    this.removed = true;
  }
}

class Dialog extends Node {
  constructor() {
    super("dialog");
    this.open = true;
  }
  getBoundingClientRect() {
    return { ...CARD, x: CARD.left, y: CARD.top, width: CARD.right - CARD.left, height: CARD.bottom - CARD.top };
  }
  close() {
    if (!this.open) return;
    this.open = false;
    this.onclose?.();
  }
}

// An icon sheet, open.
function sheet() {
  globalThis.document = {
    createElement: (tag) => (tag === "dialog" ? new Dialog() : new Node(tag)),
    createElementNS: (_, tag) => new Node(tag),
  };
  const h = { api: async () => ({}), reload: async () => {} };
  const { element } = createIconEditor(h, "groceries--a1b2", "Groceries", { name: "groceries--a1b2", kind: "app" });
  const find = (n, tag, name) => (n.tag === tag && (!name || n.name === name) ? n : n.children?.map((c) => find(c, tag, name)).find(Boolean));
  return { dialog: element, hint: find(element, "input", "hint") };
}

// An event reaching the dialog: its capture listeners, its `on…`, then
// its bubbling ones, as the browser hands it down and up again.
function reach(dialog, type, target, [clientX, clientY]) {
  const e = { type, target, clientX, clientY };
  for (const l of dialog.listeners) if (l.type === type && l.capture) l.fn(e);
  dialog[`on${type}`]?.(e);
  for (const l of dialog.listeners) if (l.type === type && !l.capture) l.fn(e);
}
// A press at `down` let go at `up`: the click goes to what holds both.
function press(dialog, downOn, down, upOn, up) {
  reach(dialog, "pointerdown", downOn, down);
  reach(dialog, "click", downOn === upOn ? upOn : dialog, up);
}

const padding = [CARD.left + 8, 400];
const field = [640, 300];
const backdrop = [200, 400];

test("a press in the card's own padding leaves the sheet open", () => {
  const { dialog } = sheet();
  press(dialog, dialog, padding, dialog, padding);
  assert.equal(dialog.open, true);
});

test("words selected from its field past the card onto the backdrop leave it open, what was typed kept", () => {
  const { dialog, hint } = sheet();
  hint.value = "a lighthouse at dusk";
  press(dialog, hint, field, dialog, backdrop);
  assert.equal(dialog.open, true);
  assert.equal(hint.value, "a lighthouse at dusk");
});

test("pressed on the backdrop and let go in the card: it stays open", () => {
  const { dialog } = sheet();
  press(dialog, dialog, backdrop, dialog, padding);
  assert.equal(dialog.open, true);
});

test("a press on the backdrop closes it, on its click, and it is taken from the page", () => {
  const { dialog } = sheet();
  reach(dialog, "pointerdown", dialog, backdrop);
  assert.equal(dialog.open, true);
  reach(dialog, "click", dialog, backdrop);
  assert.equal(dialog.open, false);
  assert.equal(dialog.removed, true);
});
