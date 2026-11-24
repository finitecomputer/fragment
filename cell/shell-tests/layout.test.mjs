// cell/shell/layout.js's desktop frame in a window with no room for the
// sidebar, the chat and the viewer at their least (761–881 px, the viewer
// open as it is by default; FIN-722): the chat list stays and the viewer
// gives way. Show sidebar (⌘B) there shows a sidebar the window squeezed
// out and never saves it closed, so it is back as the window widens; the
// viewer asked for there shows, the list giving way unsaved. A sidebar the
// person hides stays hidden. The e2e's `chat_list_home_ui` presses the same
// in Chrome.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";

// a wide window, motion reduced (no animation to run here)
globalThis.matchMedia = (query) => ({ matches: query.includes("reduce"), addEventListener() {} });
const storage = new Map();
globalThis.localStorage = { getItem: (k) => storage.get(k) ?? null, setItem: (k, v) => storage.set(k, String(v)) };
globalThis.getComputedStyle = (el) => ({ gridTemplateColumns: el.style.gridTemplateColumns ?? "", transform: "none", opacity: "1" });
globalThis.requestAnimationFrame = (fn) => (fn(), 1);
globalThis.cancelAnimationFrame = () => {};
globalThis.addEventListener = () => {};
let measured = null;
globalThis.ResizeObserver = class {
  constructor(fn) { measured = fn; }
  observe() {}
};

const { createLayout } = await import("../shell/layout.js");

class El {
  constructor() {
    const classes = new Set();
    this.style = {};
    this.clientWidth = 0;
    this.classList = {
      add: (...c) => c.forEach((x) => classes.add(x)),
      remove: (...c) => c.forEach((x) => classes.delete(x)),
      toggle: (c, on = !classes.has(c)) => (on ? classes.add(c) : classes.delete(c), on),
      contains: (c) => classes.has(c),
    };
  }
  addEventListener() {}
  querySelector() { return new El(); }
}

// The frame of a window `width` wide, from what the page saved (`saved`).
function frame(width, saved = {}) {
  storage.clear();
  for (const [k, v] of Object.entries(saved)) storage.set(k, JSON.stringify(v));
  const grid = new El();
  grid.clientWidth = width;
  let shown = null;
  const layout = createLayout({ grid, gutters: [new El(), new El()], onChange: (s) => { shown = s; } });
  return {
    layout,
    shown: () => shown,
    columns: () => grid.style.gridTemplateColumns,
    saved: () => JSON.parse(storage.get("finite.layout.v3") ?? "null"),
    resize(w) { grid.clientWidth = w; measured(); },
  };
}

test("a desktop window with room for all three shows the sidebar, the chat and the viewer", () => {
  const f = frame(1280);
  assert.deepEqual(f.shown(), { left: true, right: true });
  assert.equal(f.columns(), "264px 1px 1fr 1px 480px");
});

test("820 px wide with the viewer open, the chat list stays and the viewer gives way, nothing saved", () => {
  const f = frame(820);
  assert.deepEqual(f.shown(), { left: true, right: false });
  assert.equal(f.columns(), "264px 1px 1fr 0px 0px");
  assert.equal(f.saved(), null, "the squeeze saves nothing");
});

test("Show sidebar where the viewer was asked for shows the sidebar, and never saves it closed", () => {
  const f = frame(820);
  f.layout.toggle("right");
  assert.deepEqual(f.shown(), { left: false, right: true }, "the viewer asked for there shows, the list giving way");
  f.layout.toggle("left");
  assert.deepEqual(f.shown(), { left: true, right: false }, "Show sidebar shows it");
  assert.deepEqual([f.saved().leftOpen, f.saved().rightOpen], [true, true], "neither is saved closed");
  f.resize(1280);
  assert.deepEqual(f.shown(), { left: true, right: true }, "widened, both are back");
});

test("a viewer opened in a wide window still gives way as the window narrows", () => {
  const f = frame(1280);
  f.layout.show("right");
  f.resize(820);
  assert.deepEqual(f.shown(), { left: true, right: false });
});

test("a sidebar the person hides stays hidden, saved, until they show it", () => {
  const f = frame(1280);
  f.layout.toggle("left");
  assert.deepEqual(f.shown(), { left: false, right: true });
  assert.equal(f.saved().leftOpen, false);
  f.resize(820);
  assert.deepEqual(f.shown(), { left: false, right: true });
  f.layout.toggle("left");
  assert.deepEqual(f.shown(), { left: true, right: false });
  assert.equal(f.saved().leftOpen, true);
});

test("a layout saved closed before (v2) starts with the sidebar open", () => {
  const f = frame(1280, { "finite.layout.v2": { left: 300, right: 400, leftOpen: false, rightOpen: true } });
  assert.deepEqual(f.shown(), { left: true, right: true });
});
