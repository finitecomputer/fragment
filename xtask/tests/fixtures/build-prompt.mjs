// Execute the build handler, Settings visibility setup/renderer and notice.
// The recorded tree's relevant nesting is checked against index.html; it
// models hidden ancestors, not browser layout, permissions or selection UI.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { createContext, runInContext } from "node:vm";

const shell = readFileSync(new URL("../../../cell/shell/shell.js", import.meta.url), "utf8");
const functions = ["buildSection", "section", "openSettings", "renderSettingsPage", "notice"].map((name) => {
  const source = shell.match(new RegExp(`^(?:async )?function ${name}\\([^]*?^\\}`, "m"))?.[0];
  assert.ok(source, `the shell declares ${name}; update the test if it moves`);
  return source;
}).join("\n");
const html = readFileSync(new URL("../../../cell/shell/index.html", import.meta.url), "utf8");
const css = readFileSync(new URL("../../../cell/shell/shell.css", import.meta.url), "utf8");
assert.match(html, /<div\b[^>]*\bid="frames"[^>]*>\s*<div\b[^>]*\bid="notice"[^>]*>[\s\S]*?<\/div>\s*<\/div>\s*<section\b[^>]*\bid="settings-page"\s+hidden><\/section>/,
  "the recorded tree follows the actual notice-in-frames and sibling Settings structure");
assert.match(css, /^\[hidden\]\s*\{\s*display:\s*none\s*!important;/m);

async function setup(clipboard) {
  const made = [], timers = [];
  function el(tag, classes, text) {
    const node = {
      tag, classes, textContent: text ?? "", children: [], dataset: {}, disabled: false,
      classList: { add() {} },
      append(...children) {
        for (const child of children) {
          this.children.push(child);
          if (typeof child === "object") child.parent = this;
        }
      },
      replaceChildren(...children) {
        for (const child of this.children) if (typeof child === "object") child.parent = null;
        this.children = [];
        this.append(...children);
      },
      setAttribute(name, value) { this[name] = value; },
      removeAttribute(name) { delete this[name]; },
      querySelector(selector) {
        assert.equal(selector, ":scope > h2");
        return this.children.find((n) => n.tag === "h2") ?? null;
      },
      getBoundingClientRect() { return { top: 0 }; },
      focus() { this.focused = true; },
      select() { this.selected = this.value; },
    };
    made.push(node);
    return node;
  }
  const layout = el("div"), frames = el("div"), notice = el("div"), page = el("section");
  layout.append(frames, page);
  frames.append(notice);
  page.hidden = true;
  const ids = { layout, frames, notice, "settings-page": page };
  const visible = (node) => {
    for (let n = node; n; n = n.parent) {
      if (n.hidden) return false;
      if (n === layout) return true;
    }
    return false;
  };
  let waiting = false, openingError;
  const unused = () => { throw new Error("unexpected Settings data call"); };
  const context = createContext({
    el, navigator: { clipboard }, location: { origin: "https://platform.example" },
    $: (id) => ids[id], state: { guest: false },
    INSTALL: "install fixture", SKILL: "skill fixture",
    say: (text) => el("p", null, text),
    renderHeading() {}, renderChats() {}, leaveSidebar() {},
    api: unused, line: unused, actionLine: unused, usd: unused, noSeatSaid: unused, switchForm: unused,
    // Run openSettings through its real visibility changes, then park its
    // unrelated billing/data work at its first await. Render the real build
    // section with the real Settings renderer below.
    returning: () => { waiting = true; return new Promise(() => {}); },
    setTimeout: (callback, delay) => timers.push({ callback, delay }),
  });
  runInContext(`${functions}\nopenSettings(false);`, context).catch((error) => { openingError = error; });
  await Promise.resolve();
  assert.ifError(openingError);
  assert.equal(waiting, true, "openSettings reached its data boundary");
  assert.equal(frames.hidden, true);
  assert.equal(page.hidden, false);
  runInContext('renderSettingsPage($("settings-page"), [buildSection()]);', context);
  return {
    prompt: made.find((n) => n.id === "build-prompt"),
    copy: made.find((n) => n.dataset.action === "copy-prompt"),
    feedback: made.find((n) => n.role === "status"), timers, visible, notice,
    globalNotice: () => runInContext('notice("The prompt was not copied", "Copy manually");', context),
  };
}

test("Settings hides the global notice's ancestor while the build prompt remains visible", async () => {
  const h = await setup(undefined);
  h.globalNotice();
  assert.equal(h.notice.hidden, false, "notice unhides itself");
  assert.equal(h.visible(h.notice), false, "its frames ancestor remains hidden");
  assert.equal(h.visible(h.prompt), true);
  assert.equal(h.visible(h.copy), true);
});

function manual(h, text) {
  assert.equal(h.copy.textContent, "Copy");
  assert.equal(h.copy.disabled, false, "the person can retry");
  assert.equal(h.prompt.value, text, "manual fallback preserves the person's prompt");
  assert.equal(h.prompt.focused, true);
  assert.equal(h.prompt.selected, text);
  assert.ok(h.feedback, "failure feedback lives in the Settings section");
  assert.equal(h.feedback.parent, h.prompt.parent, "feedback is beside the prompt");
  assert.equal(h.visible(h.feedback), true, "no hidden ancestor conceals the fallback instruction");
  assert.equal(h.feedback.textContent, "The prompt was not copied. Copy the selected prompt manually, then paste it into your coding agent.");
}

for (const [name, clipboard] of [
  ["clipboard API absent", undefined],
  ["writeText absent", {}],
  ["synchronous exception", { writeText() { throw new Error("unavailable"); } }],
  ["permission denied", { writeText() { return Promise.reject(new Error("NotAllowedError")); } }],
  ["rejection without an Error", { writeText() { return Promise.reject(null); } }],
]) {
  test(`${name}: select the prompt for manual copying without claiming success`, async () => {
    const h = await setup(clipboard);
    h.prompt.value += "a shared calendar 🗓";
    const text = h.prompt.value;
    await h.copy.onclick();
    manual(h, text);
    assert.equal(h.timers.length, 0, "failure schedules no success label");
  });
}

test("Copied appears only after the current write fulfills, then resets after 1500 ms", async () => {
  const writes = [];
  let resolve;
  const clipboard = { writeText(text) { writes.push(text); return new Promise((r) => { resolve = r; }); } };
  const h = await setup(clipboard);
  assert.equal(h.prompt.value, "Read https://platform.example/llms.txt and help me build: ");
  h.prompt.value += "my plan";
  const pending = h.copy.onclick();
  assert.deepEqual(writes, [h.prompt.value]);
  assert.equal(h.copy.textContent, "Copy");
  assert.equal(h.copy.disabled, true);
  assert.equal(h.timers.length, 0);
  resolve();
  await pending;
  assert.equal(h.copy.textContent, "Copied");
  assert.equal(h.copy.disabled, false);
  assert.equal(h.visible(h.feedback), false);
  assert.equal(h.timers.length, 1);
  assert.equal(h.timers[0].delay, 1500);
  h.timers[0].callback();
  assert.equal(h.copy.textContent, "Copy");
});

test("a later refusal clears prior success, preserves edited text, and permits an explicit retry", async () => {
  const writes = [];
  let reject = false;
  const h = await setup({ async writeText(text) { writes.push(text); if (reject) throw new Error("denied"); } });
  await h.copy.onclick();
  assert.equal(h.copy.textContent, "Copied");
  h.prompt.value += "edited request";
  reject = true;
  const pending = h.copy.onclick();
  assert.equal(h.copy.textContent, "Copy", "clear the previous success before awaiting this attempt");
  await pending;
  manual(h, h.prompt.value);
  assert.equal(writes.length, 2, "no automatic retry");
  reject = false;
  await h.copy.onclick();
  assert.equal(h.copy.textContent, "Copied");
  assert.equal(writes.length, 3);
  assert.equal(writes[2], h.prompt.value);
  assert.equal(h.visible(h.feedback), false, "successful retry clears stale failure feedback");
});
