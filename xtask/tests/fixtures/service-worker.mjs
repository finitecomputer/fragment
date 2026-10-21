// Execute the whole served script, not a reimplementation of its handlers.
// These APIs record requests; they do not implement browser permissions,
// notification UI, service-worker installation or real push delivery.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { runInNewContext } from "node:vm";

const script = readFileSync(new URL("../../../cell/sw.js", import.meta.url), "utf8");
const origin = "https://chat.fragment.test";
const matchOptions = { type: "window", includeUncontrolled: true };
const defaults = { body: "", tag: undefined, data: undefined, icon: undefined };

function worker({ windows = [], show = async () => {}, match, open = async () => ({}) } = {}, source = script) {
  const listeners = new Map(), calls = [];
  const clients = windows.map((w, i) => ({
    url: w.url,
    focus() { calls.push(["focus", i]); return w.focus ? w.focus() : Promise.resolve(w); },
  }));
  const self = {
    location: new URL(`${origin}/__sw.js`),
    addEventListener(type, listener) {
      assert.equal(listeners.has(type), false, `one ${type} listener`);
      listeners.set(type, listener);
    },
    registration: {
      showNotification(title, options) { calls.push(["show", title, { ...options }]); return show(); },
    },
    clients: {
      matchAll(options) { calls.push(["match", { ...options }]); return match ? match() : Promise.resolve(clients); },
      openWindow(url) { calls.push(["open", url]); return open(); },
    },
  };
  runInNewContext(source, { self, URL }, { filename: "cell/sw.js", timeout: 1000 });
  function fire(type, event) {
    const waits = [];
    listeners.get(type)({ ...event, waitUntil: (promise) => waits.push(promise) });
    assert.equal(waits.length, 1, `${type} extends its lifetime exactly once`);
    return Promise.all(waits);
  }
  return {
    calls, listeners,
    push: (data) => fire("push", { data }),
    click: (data) => fire("notificationclick", { notification: { data, close() { calls.push(["close"]); } } }),
  };
}

test("the worker registers only push and notificationclick, never fetch", () => {
  assert.deepEqual([...worker().listeners.keys()], ["push", "notificationclick"]);
});

async function displayLifetime(source = script) {
  let resolve;
  const h = worker({ show: () => new Promise((r) => { resolve = r; }) }, source);
  const payload = { title: "Agent replied", body: "Ready ✓", tag: "chat-7", url: "./", icon: "https://images.example/agent.png" };
  let settled = false;
  const pending = h.push({ json: () => payload }).then(() => { settled = true; });
  try {
    assert.equal(typeof resolve, "function", "display started");
    // Drain promise reactions before checking: a single microtask can miss a
    // detached waitUntil promise settling through Promise.all and .then.
    await new Promise(setImmediate);
    assert.equal(settled, false, "the event waits for display, not just the handler's return");
    assert.deepEqual(h.calls, [["show", payload.title, { body: payload.body, tag: payload.tag, data: payload.url, icon: payload.icon }]]);
  } finally {
    resolve?.();
    await pending;
  }
  assert.equal(settled, true);
}

test("valid payload fields reach the notification and waitUntil tracks display completion", { timeout: 1000 }, async () => {
  await displayLifetime();
});

test("missing, invalid JSON and non-object payloads display safe defaults", async () => {
  const malformed = [undefined, null, { json() { throw new SyntaxError("invalid JSON"); } }];
  for (const value of [null, false, 42, "text", [], {}]) malformed.push({ json: () => value });
  for (const data of malformed) {
    const h = worker();
    await h.push(data);
    assert.deepEqual(h.calls, [["show", "fragment", defaults]]);
  }
});

test("notification fields are typed and icons accept only absolute http or https addresses", async () => {
  for (const icon of [undefined, 7, "/avatar.png", "javascript:alert(1)", "data:image/png;base64,AA", "file:///avatar.png"]) {
    const h = worker();
    await h.push({ json: () => ({ title: "", body: [], tag: 7, url: {}, icon }) });
    assert.deepEqual(h.calls, [["show", "fragment", defaults]]);
  }
  for (const icon of ["http://images.example/agent.png", "https://images.example/agent.png"]) {
    const h = worker();
    await h.push({ json: () => ({ title: 7, icon }) });
    assert.deepEqual(h.calls, [["show", "fragment", { ...defaults, icon }]]);
  }
});

test("a rejected notification display is contained by the event lifetime", async () => {
  const h = worker({ show: () => Promise.reject(new Error("notification permission denied")) });
  await assert.doesNotReject(h.push({ json: () => ({ title: "Reply" }) }));
  assert.deepEqual(h.calls, [["show", "Reply", defaults]]);
});

async function focusLifetime(source = script) {
  let resolve, started;
  const focusing = new Promise((r) => { started = r; });
  const h = worker({ windows: [
    { url: `${origin}/other` },
    { url: `${origin}/`, focus: () => new Promise((r) => { resolve = r; started(); }) },
  ] }, source);
  let settled = false;
  const pending = h.click("./").then(() => { settled = true; });
  try {
    assert.deepEqual(h.calls[0], ["close"]);
    await focusing;
    await new Promise(setImmediate);
    assert.equal(settled, false, "the event waits for focus completion");
    assert.deepEqual(h.calls, [["close"], ["match", matchOptions], ["focus", 1]]);
  } finally {
    resolve?.();
    await pending;
  }
  assert.equal(settled, true);
}

test("a click closes immediately and waits for the matching existing window's focus", { timeout: 1000 }, async () => {
  await focusLifetime();
});

// Negative controls keep the lifetime assertions honest. Mutate only the copy
// evaluated by this harness; the served worker remains unchanged.
function replaceOnce(before, after) {
  assert.equal(script.split(before).length, 2, "negative control matches exactly one source statement");
  return script.replace(before, after);
}

test("the display lifetime assertion rejects a detached notification promise", { timeout: 1000 }, async () => {
  const detached = replaceOnce(
    "event.waitUntil(self.registration.showNotification(title, opts).catch(() => {}));",
    "self.registration.showNotification(title, opts).catch(() => {}); event.waitUntil(Promise.resolve());",
  );
  await assert.rejects(displayLifetime(detached), {
    code: "ERR_ASSERTION", actual: true, expected: false,
    message: /^the event waits for display, not just the handler's return/,
  });
});

test("the focus lifetime assertion rejects a detached matching-window promise", { timeout: 1000 }, async () => {
  const detached = replaceOnce(
    "if (url && c.url === url) return await c.focus();",
    "if (url && c.url === url) { c.focus().catch(() => {}); return; }",
  );
  await assert.rejects(focusLifetime(detached), {
    code: "ERR_ASSERTION", actual: true, expected: false,
    message: /^the event waits for focus completion/,
  });
});

test("relative and same-origin absolute targets open their resolved URL when no window matches", async () => {
  for (const [target, resolved] of [
    ["./", `${origin}/`],
    ["/thread?from=push#reply", `${origin}/thread?from=push#reply`],
    [`${origin}:443/thread`, `${origin}/thread`],
  ]) {
    const h = worker({ windows: [{ url: `${origin}/unrelated` }] });
    await h.click(target);
    assert.deepEqual(h.calls, [["close"], ["match", matchOptions], ["open", resolved]]);
  }
});

test("invalid and foreign targets never open a URL, with or without an existing window", async () => {
  for (const target of [
    undefined, null, 7, {}, "http://[", "https://other.example/", "//other.example/",
    "https://chat.fragment.test.other.example/", "https://chat.fragment.test@other.example/",
    "http://chat.fragment.test/", "https://chat.fragment.test:444/", "javascript:alert(1)", "data:text/html,hello",
  ]) {
    for (const windows of [[], [{ url: `${origin}/first` }, { url: `${origin}/second` }]]) {
      const h = worker({ windows });
      await h.click(target);
      const expected = [["close"], ["match", matchOptions]];
      if (windows.length) expected.push(["focus", 0]);
      assert.deepEqual(h.calls, expected, `target ${JSON.stringify(target)}`);
    }
  }
});

test("openWindow returning null falls back to an existing window, or ends if none exists", async () => {
  for (const windows of [[], [{ url: `${origin}/first` }, { url: `${origin}/second` }]]) {
    const h = worker({ windows, open: async () => null });
    await h.click("./");
    const expected = [["close"], ["match", matchOptions], ["open", `${origin}/`]];
    if (windows.length) expected.push(["focus", 0]);
    assert.deepEqual(h.calls, expected);
  }
});

test("rejected lookup, matching focus, fallback focus and open promises settle without retry", async () => {
  const reject = () => Promise.reject(new Error("client unavailable"));
  for (const [options, target, attempted] of [
    [{ match: reject }, "./", []],
    [{ windows: [{ url: `${origin}/`, focus: reject }] }, "./", [["focus", 0]]],
    [{ windows: [{ url: `${origin}/`, focus: reject }] }, undefined, [["focus", 0]]],
    [{ windows: [{ url: `${origin}/other` }], open: reject }, "./", [["open", `${origin}/`]]],
  ]) {
    const h = worker(options);
    await assert.doesNotReject(h.click(target));
    assert.deepEqual(h.calls, [["close"], ["match", matchOptions], ...attempted]);
  }
});
