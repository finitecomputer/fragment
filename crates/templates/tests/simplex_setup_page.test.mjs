// The SimpleX connector's setup page (templates/simplex/site/setup.js),
// against a stand-in for the page's DOM and for its platform module
// (`__fragment.js`: its owner, its calls, its live `state`), the clock
// mocked. Valid (new, it starts its door, asks a link, shows its link and
// code, then whom it linked), invalid (a deployment with no doors, a
// viewer), and its waits: a step whose job never answers is said failed
// with Try again, which asks that same step again; a second tab's unlink
// starts no link here; an invitation answered after its link stopped
// watching is looked at once, as its watch ends.
import { test, mock } from "node:test";
import assert from "node:assert/strict";
import { register } from "node:module";

const FRAGMENT = "export const me = () => globalThis.__fragment.me(); export const call = (...a) => globalThis.__fragment.call(...a); export const live = (...a) => globalThis.__fragment.live(...a);";
const HOOKS = `export async function resolve(specifier, context, next) {
  if (specifier.endsWith("/__fragment.js")) return { url: ${JSON.stringify("data:text/javascript," + encodeURIComponent(FRAGMENT))}, shortCircuit: true };
  return next(specifier, context);
}`;
register("data:text/javascript," + encodeURIComponent(HOOKS));

// a node as the page draws one: its tag, text, children and attributes
class El {
  constructor(tag) {
    Object.assign(this, { tag, className: "", textContent: "", children: [], attrs: {}, dataset: {}, listeners: {} });
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
const all = (n) => [n, ...n.children.flatMap(all)];

// framed in Settings' dialog, as Set up opens it
globalThis.window = { parent: {} };
globalThis.document = { createElement: (t) => new El(t), createElementNS: (_, t) => new El(t), documentElement: { dataset: {} } };
globalThis.location = { search: "?name=Juniper", href: "https://juniper-sx--k3x9.fragment.localhost/setup.html?name=Juniper" };
globalThis.addEventListener = () => {};

const { setup } = await import(new URL("../../../templates/simplex/site/setup.js", import.meta.url).href);

// a page opened as `role`, its calls and fetches kept; `push(state)`, its
// live `state` answering
async function opened(role = "owner") {
  const w = { calls: [], fetched: [], push: null, root: new El("main") };
  globalThis.fetch = async (url, init) => {
    w.fetched.push(`${init?.method ?? "GET"} ${new URL(url).pathname}`);
    return new Response(JSON.stringify(String(url).includes("__door") ? { bound: { door: "simplex", agent: "juniper--k3x9" } } : { ok: true }), { status: 200 });
  };
  globalThis.__fragment = {
    me: async () => ({ role, platform: "https://platform.example" }),
    call: async (op) => (w.calls.push(op), { run: w.calls.length, status: "queued" }),
    live: (_op, _input, onResult, onError) => {
      w.push = async (s) => {
        onResult(s);
        await settled();
      };
      w.lose = async () => {
        onError(new Error("its socket closed"));
        await settled();
      };
    },
  };
  await setup(w.root);
  await settled();
  w.said = () => {
    const nodes = all(w.root);
    return { line: nodes.find((n) => n.tag === "p")?.textContent ?? null, buttons: nodes.filter((n) => n.tag === "button" || n.tag === "a").map((n) => n.textContent), qr: nodes.some((n) => n.tag === "svg") };
  };
  w.press = async (text) => {
    const b = all(w.root).find((n) => n.tag === "button" && n.textContent === text);
    assert.ok(b, `a ${text} button: ${JSON.stringify(w.said())}`);
    await b.listeners.click();
    await settled();
  };
  w.secrets = () => w.fetched.filter((f) => f === "PUT /__secrets/SIMPLEX_DOOR_SECRET").length;
  return w;
}

// the page's promises run (the mocked clock leaves setImmediate be)
const settled = async () => {
  for (let i = 0; i < 5; i += 1) await new Promise((r) => setImmediate(r));
};

const NEW = { connected: false, invitation: null, linked: null, why: null };
const CONNECTED = { ...NEW, connected: true };
const invitation = (at) => ({ ...CONNECTED, invitation: { link: "simplex:/invitation#/?v=2-7&smp=s", short: "https://smp.example/i#short", qr: { size: 21, dark: "10".repeat(220) + "1" }, at } });
const LINKED = { ...CONNECTED, linked: { name: "Robin", at: 1 } };
const failed = (at, code = "starting", step = null) => ({ text: `the door answered 503: ${code}`, code, at, step });

test("opened new, it starts its door, asks a link, shows its link and code, then whom it linked; unlinked, it waits for Link", async () => {
  mock.timers.enable({ apis: ["setTimeout", "Date"], now: 1_000_000 });
  try {
    const w = await opened();
    await w.push(NEW);
    assert.deepEqual([w.calls, w.secrets(), w.said().line], [["connect"], 1, "Starting…"], "its inbox's secret, then its connect");
    await w.push(CONNECTED);
    assert.deepEqual([w.calls, w.said().line], [["connect", "link"], "Making a link…"]);
    await w.push(invitation(Date.now()));
    assert.deepEqual(w.said(), { line: "Scan with SimpleX, or open the link.", buttons: ["Open in SimpleX", "Copy link"], qr: true });
    assert.equal(all(w.root).find((n) => n.tag === "a").href, "https://smp.example/i#short", "its short link");
    await w.push(LINKED);
    assert.deepEqual(w.said(), { line: "Linked with Robin.", buttons: ["Unlink"], qr: false });
    await w.press("Unlink");
    assert.equal(w.calls.at(-1), "unlink");
    await w.push(CONNECTED);
    assert.deepEqual(w.said(), { line: "Not linked.", buttons: ["Link"], qr: false });
    assert.deepEqual(w.calls, ["connect", "link", "unlink"], "no link of its own after an unlink");
  } finally {
    mock.timers.reset();
  }
});

test("a step whose job never answers is said failed after its wait, with Try again, which asks that step again", async () => {
  mock.timers.enable({ apis: ["setTimeout", "Date"], now: 1_000_000 });
  try {
    const w = await opened();
    await w.push(NEW);
    assert.equal(w.said().line, "Starting…");
    // its connect's job failed outright: nothing written to its state
    mock.timers.tick(5 * 60_000);
    await settled();
    assert.deepEqual(w.said(), { line: "Couldn't reach SimpleX. Try again.", buttons: ["Try again"], qr: false });
    await w.press("Try again");
    assert.deepEqual([w.calls, w.secrets(), w.said().line], [["connect", "connect"], 2, "Starting…"]);
  } finally {
    mock.timers.reset();
  }
});

test("a step answered after its wait still counts: the page goes on as it would have", async () => {
  mock.timers.enable({ apis: ["setTimeout", "Date"], now: 1_000_000 });
  try {
    const w = await opened();
    await w.push(NEW);
    mock.timers.tick(3 * 60_000);
    await settled();
    assert.equal(w.said().line, "Couldn't reach SimpleX. Try again.");
    // a door's slow first start: its connect answers late
    await w.push(CONNECTED);
    assert.deepEqual([w.calls, w.said().line], [["connect", "link"], "Making a link…"]);
  } finally {
    mock.timers.reset();
  }
});

test("Try again after a failed unlink unlinks again, once, never asking a link", async () => {
  mock.timers.enable({ apis: ["setTimeout", "Date"], now: 1_000_000 });
  try {
    const w = await opened();
    await w.push(LINKED);
    await w.press("Unlink");
    // its unlink asked (its link let go in its state), its door refusing it
    await w.push({ ...CONNECTED, why: failed(Date.now(), "starting", "unlink") });
    assert.deepEqual(w.said(), { line: "Couldn't reach SimpleX. Try again.", buttons: ["Try again"], qr: false });
    await w.press("Try again");
    assert.deepEqual(w.calls, ["unlink", "unlink"]);
    // the failure it tries again stays in its state until the job asks:
    // no answer to this try, so no Try again to press twice
    assert.deepEqual(w.said(), { line: "Unlinking…", buttons: [], qr: false });
  } finally {
    mock.timers.reset();
  }
});

test("Try again repeats the step its failure names, whichever tab asked it", async () => {
  mock.timers.enable({ apis: ["setTimeout", "Date"], now: 1_000_000 });
  try {
    const w = await opened();
    await w.push(NEW);
    await w.push(CONNECTED);
    await w.push(invitation(Date.now()));
    assert.deepEqual(w.calls, ["connect", "link"]);
    // another tab's unlink, refused by its door
    await w.push({ ...CONNECTED, why: failed(Date.now(), "starting", "unlink") });
    await w.press("Try again");
    assert.deepEqual(w.calls, ["connect", "link", "unlink"], "its failed unlink again, not this tab's link");
  } finally {
    mock.timers.reset();
  }
});

test("opened linked, another tab's unlink starts no link here", async () => {
  mock.timers.enable({ apis: ["setTimeout", "Date"], now: 1_000_000 });
  try {
    const w = await opened();
    await w.push(LINKED);
    await w.push(CONNECTED);
    assert.deepEqual(w.said(), { line: "Not linked.", buttons: ["Link"], qr: false });
    assert.deepEqual(w.calls, []);
    // opened on an invitation, the same
    const v = await opened();
    await v.push(invitation(Date.now()));
    await v.push(CONNECTED);
    assert.deepEqual(v.calls, []);
  } finally {
    mock.timers.reset();
  }
});

test("an invitation left open is looked at once as its link's watch ends, and an old one at once", async () => {
  mock.timers.enable({ apis: ["setTimeout", "Date"], now: 1_000_000 });
  try {
    const w = await opened();
    await w.push(invitation(Date.now()));
    assert.deepEqual(w.calls, []);
    mock.timers.tick(9 * 60_000);
    await settled();
    assert.deepEqual(w.calls, ["check"], "as its watch ends, with no change to its state");
    mock.timers.tick(30 * 60_000);
    await settled();
    assert.deepEqual(w.calls, ["check"], "once");
    const v = await opened();
    await v.push(invitation(Date.now() - 20 * 60_000));
    assert.deepEqual(v.calls, ["check"], "older than its watch: at once");
  } finally {
    mock.timers.reset();
  }
});

test("its live state lost is said, with nothing to press, until it answers again", async () => {
  const w = await opened();
  await w.push(LINKED);
  await w.lose();
  assert.deepEqual(w.said(), { line: "Couldn't reach SimpleX.", buttons: [], qr: false });
  await w.push(LINKED);
  assert.deepEqual(w.said(), { line: "Linked with Robin.", buttons: ["Unlink"], qr: false });
  assert.deepEqual(w.calls, [], "nothing asked of it meanwhile");
});

test("a deployment that runs no doors says so, with nothing to try; a viewer is told it is its owner's", async () => {
  const w = await opened();
  await w.push({ ...NEW, why: failed(1, "no_doors") });
  assert.deepEqual(w.said(), { line: "SimpleX isn't available here yet.", buttons: [], qr: false });
  assert.deepEqual(w.calls, []);
  const v = await opened("viewer");
  assert.equal(v.push, null, "its state never read");
  assert.equal(v.said().line, "Only its owner sets this up.");
});
