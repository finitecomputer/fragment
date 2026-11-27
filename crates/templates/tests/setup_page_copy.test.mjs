// A connector setup page's Copy (templates/telegram/site/setup-page.js
// `copyButton`, which Slack's pairing step uses), as the shell's: a copy
// icon and its word, a check and "Copied" for 1.5 s once copied, then Copy
// again; a newer press's answer is the one shown; a clipboard that
// refuses (as a frame's whose policy names another origin) copies by the
// browser's older way, as the chat page does; when that refuses too, it
// selects what it copies and says so, and Copy stays Copy.
import { test, mock } from "node:test";
import assert from "node:assert/strict";
import { register } from "node:module";

register("data:text/javascript," + encodeURIComponent(`export async function resolve(s, c, next) {
  if (s.endsWith("/__fragment.js")) return { url: "data:text/javascript,export const me = () => ({});", shortCircuit: true };
  return next(s, c);
}`));

class El {
  constructor(tag) {
    Object.assign(this, { tag, className: "", textContent: "", children: [], attrs: {}, listeners: {}, hidden: false, style: {} });
  }
  select() {}
  remove() {
    removed.push(this);
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
  addEventListener(type, f) {
    this.listeners[type] = f;
  }
}
globalThis.window = { parent: {} };
let selected = null;
let byHand = false;
const removed = [];
const execs = [];
globalThis.document = {
  body: { append() {} },
  execCommand: (c) => (execs.push(c), byHand),
  createElement: (t) => new El(t),
  createElementNS: (_, t) => new El(t),
  documentElement: { dataset: {} },
  createRange: () => ({ selectNodeContents(n) { this.node = n; } }),
};
globalThis.getSelection = () => ({ removeAllRanges: () => (selected = null), addRange: (r) => (selected = r.node) });

const { copyButton, el } = await import(new URL("../../../templates/telegram/site/setup-page.js", import.meta.url).href);

// what the button shows: its icon's shapes' first path, and its word
const shown = (b) => ({ word: b.children.find((c) => typeof c === "string"), check: b.children[0].children.some((s) => s.attrs.d === "M20 6 9 17l-5-5"), label: b.attrs["aria-label"] });
const clipboard = (writeText) => Object.defineProperty(globalThis, "navigator", { value: { clipboard: { writeText } }, configurable: true });

test("Copy copies, shows a check and Copied for 1.5 s, then Copy again", async () => {
  mock.timers.enable({ apis: ["setTimeout"] });
  const wrote = [];
  clipboard(async (t) => wrote.push(t));
  const code = el("code", null, "pair 975fb7d36e5c93db");
  const { button, said } = copyButton("pair 975fb7d36e5c93db", code, "the pairing message");
  assert.deepEqual(shown(button), { word: "Copy", check: false, label: "Copy the pairing message" });
  assert.equal(button.className, "quiet setup-copy");
  await button.listeners.click();
  assert.deepEqual(wrote, ["pair 975fb7d36e5c93db"]);
  assert.deepEqual(shown(button), { word: "Copied", check: true, label: "Copied" });
  assert.equal(said.hidden, true);
  mock.timers.tick(1499);
  assert.equal(shown(button).word, "Copied");
  mock.timers.tick(1);
  assert.deepEqual(shown(button), { word: "Copy", check: false, label: "Copy the pairing message" });
  mock.timers.reset();
});

test("a second press keeps Copied its full 1.5 s: the first press's timer changes nothing", async () => {
  mock.timers.enable({ apis: ["setTimeout"] });
  clipboard(async () => {});
  const { button } = copyButton("x", el("code"), "it");
  await button.listeners.click();
  mock.timers.tick(1000);
  await button.listeners.click();
  mock.timers.tick(600);
  assert.equal(shown(button).word, "Copied", "the first press's timer is spent, and shows nothing");
  mock.timers.tick(900);
  assert.equal(shown(button).word, "Copy");
  mock.timers.reset();
});

test("a clipboard that refuses (a frame's, FIN-508's live test) copies by the browser's older way, and shows Copied", async () => {
  mock.timers.enable({ apis: ["setTimeout"] });
  Object.defineProperty(globalThis, "navigator", { value: { clipboard: { writeText: async () => { throw new Error("NotAllowedError"); } } }, configurable: true });
  byHand = true;
  execs.length = 0;
  removed.length = 0;
  selected = null;
  const { button, said } = copyButton("pair 975fb7d36e5c93db", el("code"), "the pairing message");
  await button.listeners.click();
  assert.deepEqual(execs, ["copy"]);
  assert.equal(removed.length, 1, "its hidden field is gone");
  assert.equal(removed[0].value, "pair 975fb7d36e5c93db");
  assert.deepEqual(shown(button), { word: "Copied", check: true, label: "Copied" });
  assert.equal(said.hidden, true);
  assert.equal(selected, null);
  mock.timers.tick(1500);
  assert.equal(shown(button).word, "Copy");
  byHand = false;
  mock.timers.reset();
});

test("a clipboard that refuses, or none at all, with the older way refused too, selects the code to copy by hand, says so, and Copy stays Copy", async () => {
  byHand = false;
  for (const nav of [{ clipboard: { writeText: async () => { throw new Error("NotAllowedError"); } } }, {}]) {
    Object.defineProperty(globalThis, "navigator", { value: nav, configurable: true });
    selected = null;
    const code = el("code", null, "pair 975fb7d36e5c93db");
    const { button, said } = copyButton("pair 975fb7d36e5c93db", code, "the pairing message");
    await button.listeners.click();
    assert.equal(selected, code, "the code is selected");
    assert.equal(said.hidden, false);
    assert.match(said.textContent, /copy it yourself/);
    assert.equal(said.attrs.role, "status");
    assert.equal(shown(button).word, "Copy");
  }
});
