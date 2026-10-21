// Execute buildSection itself, including its assigned onclick handler.
// The recorded elements do not model browser permissions or selection UI.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { runInNewContext } from "node:vm";

const shell = readFileSync(new URL("../../../cell/shell/shell.js", import.meta.url), "utf8");
const section = shell.match(/^function buildSection\(\) \{[\s\S]*?^\}/m)?.[0];
assert.ok(section, "the shell declares buildSection; update the test if it moves");

function setup(clipboard) {
  const made = [], notices = [], timers = [];
  function el(tag, classes, text) {
    const node = {
      tag, classes, textContent: text ?? "", children: [], dataset: {}, disabled: false,
      append(...children) { this.children.push(...children); },
      setAttribute(name, value) { this[name] = value; },
      focus() { this.focused = true; },
      select() { this.selected = this.value; },
    };
    made.push(node);
    return node;
  }
  runInNewContext(`${section}\nbuildSection();`, {
    el, navigator: { clipboard }, location: { origin: "https://platform.example" },
    INSTALL: "install fixture", SKILL: "skill fixture",
    say: (text) => el("p", null, text),
    section: (title, ...children) => ({ title, children }),
    notice: (...args) => notices.push(args),
    setTimeout: (callback, delay) => timers.push({ callback, delay }),
  });
  return {
    prompt: made.find((n) => n.id === "build-prompt"),
    copy: made.find((n) => n.dataset.action === "copy-prompt"), notices, timers,
  };
}

function manual(h, text) {
  assert.equal(h.copy.textContent, "Copy");
  assert.equal(h.copy.disabled, false, "the person can retry");
  assert.equal(h.prompt.value, text, "manual fallback preserves the person's prompt");
  assert.equal(h.prompt.focused, true);
  assert.equal(h.prompt.selected, text);
  assert.deepEqual(h.notices.at(-1), ["The prompt was not copied", "Copy the selected prompt manually, then paste it into your coding agent."]);
}

for (const [name, clipboard] of [
  ["clipboard API absent", undefined],
  ["writeText absent", {}],
  ["synchronous exception", { writeText() { throw new Error("unavailable"); } }],
  ["permission denied", { writeText() { return Promise.reject(new Error("NotAllowedError")); } }],
  ["rejection without an Error", { writeText() { return Promise.reject(null); } }],
]) {
  test(`${name}: select the prompt for manual copying without claiming success`, async () => {
    const h = setup(clipboard);
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
  const h = setup(clipboard);
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
  assert.deepEqual(h.notices, []);
  assert.equal(h.timers.length, 1);
  assert.equal(h.timers[0].delay, 1500);
  h.timers[0].callback();
  assert.equal(h.copy.textContent, "Copy");
});

test("a later refusal clears prior success, preserves edited text, and permits an explicit retry", async () => {
  const writes = [];
  let reject = false;
  const h = setup({ async writeText(text) { writes.push(text); if (reject) throw new Error("denied"); } });
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
});
