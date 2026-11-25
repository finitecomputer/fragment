// cell/shell/theme.js (FIN-652): the shell's light or dark is the system's
// while the person keeps System, the default, and the one they pick in
// Settings otherwise, on this device. The root's data-theme says it, the
// bars' theme-color follows it, and the shell hears each change. The real
// module, its page faked: the system's media, storage, the root, the metas.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";

const KEY = "finite.theme.v1";
const LIGHT_BAR = "#ebe9e5";
const DARK_BAR = "#181818";

// the system's prefers-color-scheme: dark, which a test flips
const media = { matches: false, listeners: [], addEventListener(_, f) { this.listeners.push(f); } };
function systemIs(mode) {
  media.matches = mode === "dark";
  for (const f of media.listeners) f();
}
// storage as the browser's: strings, and one that throws when asked to
let broken = false;
const kept = new Map();
globalThis.localStorage = {
  getItem: (k) => { if (broken) throw new Error("denied"); return kept.has(k) ? kept.get(k) : null; },
  setItem: (k, v) => { if (broken) throw new Error("denied"); kept.set(k, String(v)); },
};
const bars = [
  { content: LIGHT_BAR, media: "(prefers-color-scheme: light)" },
  { content: DARK_BAR, media: "(prefers-color-scheme: dark)" },
];
const root = { dataset: {} };
globalThis.document = { documentElement: root, querySelectorAll: () => bars };
globalThis.matchMedia = () => media;
const windowListeners = {};
globalThis.addEventListener = (type, f) => { windowListeners[type] = f; };

const theme = await import("../shell/theme.js");
const barColours = () => bars.map((b) => b.content);

test("with nothing chosen it is the system's, and follows it", () => {
  assert.equal(theme.themeChoice(), "system");
  assert.equal(root.dataset.theme, "light");
  systemIs("dark");
  assert.equal(theme.themeMode(), "dark");
  assert.equal(root.dataset.theme, "dark");
  assert.deepEqual(barColours(), [LIGHT_BAR, DARK_BAR], "each bar keeps its own colour under System");
});

test("a chosen light holds against a dark system, bars and all, and is told", () => {
  systemIs("dark");
  const heard = [];
  const stop = theme.onThemeChange((mode) => heard.push(mode));
  theme.chooseTheme("light");
  assert.equal(kept.get(KEY), JSON.stringify("light"));
  assert.equal(root.dataset.theme, "light");
  assert.deepEqual(barColours(), [LIGHT_BAR, LIGHT_BAR]);
  systemIs("light");
  systemIs("dark");
  assert.equal(root.dataset.theme, "light", "the system's change is not the person's");
  assert.deepEqual(heard, ["light"], "only the choice was told");
  stop();
});

test("a chosen dark holds against a light system", () => {
  systemIs("light");
  theme.chooseTheme("dark");
  assert.equal(root.dataset.theme, "dark");
  assert.deepEqual(barColours(), [DARK_BAR, DARK_BAR]);
});

test("System again follows the system, and gives each bar back its own", () => {
  systemIs("light");
  theme.chooseTheme("dark");
  theme.chooseTheme("system");
  assert.equal(root.dataset.theme, "light");
  assert.deepEqual(barColours(), [LIGHT_BAR, DARK_BAR]);
  systemIs("dark");
  assert.equal(root.dataset.theme, "dark");
});

test("another tab's choice is taken; another key's change is not", () => {
  systemIs("light");
  theme.chooseTheme("system");
  kept.set(KEY, JSON.stringify("dark"));
  windowListeners.storage({ key: "finite.layout.v2" });
  assert.equal(root.dataset.theme, "light");
  windowListeners.storage({ key: KEY });
  assert.equal(root.dataset.theme, "dark");
  theme.chooseTheme("system");
});

test("a stored value that is no theme is System", () => {
  systemIs("dark");
  for (const stored of [JSON.stringify("purple"), JSON.stringify(3), "{not json"]) {
    kept.set(KEY, stored);
    assert.equal(theme.themeChoice(), "system", stored);
    assert.equal(theme.themeMode(), "dark", stored);
  }
  theme.chooseTheme("system");
});

test("no theme but the three is chosen, and nothing is kept for it", () => {
  theme.chooseTheme("light");
  assert.throws(() => theme.chooseTheme("purple"), /No such theme/);
  assert.throws(() => theme.chooseTheme(undefined), /No such theme/);
  assert.equal(kept.get(KEY), JSON.stringify("light"));
  theme.chooseTheme("system");
});

test("where storage throws it stays the system's, and nothing breaks", () => {
  systemIs("dark");
  broken = true;
  try {
    assert.equal(theme.themeChoice(), "system");
    theme.chooseTheme("light");
    // not kept, so the system's: a page with no storage cannot hold a choice
    assert.equal(root.dataset.theme, "dark");
  } finally {
    broken = false;
  }
});
