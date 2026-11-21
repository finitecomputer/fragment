// cell/shell/computer.js's Storage (docs/computers.md, "Room to restore"):
// what its owner's view says of its disk, as Settings → Computer → Storage
// draws it: its sizes as its notices say them, each kind's share, Other for
// what the kinds leave out, its biggest files, and nothing before a read;
// "Full" at no room; drawn again as a view that says otherwise lands (the
// real `createComputer`, its page faked as far as Settings → Computer).
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { createComputer, storageParts, storageSize } from "../shell/computer.js";

test("sizes as its notices say them: GB to a tenth, under one whole MB, at least 1", () => {
  assert.deepEqual([2_389_566_592, 2_349_999_999, 1_000_000_000, 999_499_999, 400_000, 0].map(storageSize), ["2.4 GB", "2.3 GB", "1.0 GB", "999 MB", "1 MB", "1 MB"]);
});

test("before its first read after a save: nothing to draw", () => {
  assert.equal(storageParts(null), null);
  assert.equal(storageParts({}), null);
  assert.equal(storageParts({ disk: { total: 20e9, free: 9e9, room: 8e9, at: 1 } }), null, "a start's read alone: no /data size yet");
});

test("its kinds, Other for the rest, the room left, its biggest files and when", () => {
  const c = {
    disk: { total: 20_000_002_048, free: 9e9, data: 14e9, room: 2.6e9, filling: true, at: 10 },
    sizes: {
      at: 12,
      folders: [],
      files: [{ path: "work/juniper/downloads/talk.mp4", bytes: 3e9 }, { path: "big.bin", bytes: 2e9 }],
      kinds: [{ kind: "projects", bytes: 8e9 }, { kind: "downloads", bytes: 3e9 }, { kind: "agents", bytes: 1e9 }, { kind: "nonsense", bytes: 5e9 }, { kind: "caches", bytes: 0 }],
    },
  };
  const p = storageParts(c);
  assert.deepEqual(p.kinds.map((k) => [k.kind, k.label, k.bytes]), [["projects", "Projects", 8e9], ["downloads", "Downloads", 3e9], ["agents", "Agents", 1e9], ["other", "Other", 2e9]], "a kind it does not know, or empty, is left out; the rest is Other");
  assert.deepEqual([p.used, p.left, p.filling, p.at], [14e9, 2.6e9, true, 12]);
  assert.deepEqual(p.files, [{ path: "work/juniper/downloads/talk.mp4", name: "talk.mp4", folder: "work/juniper/downloads", bytes: 3e9 }, { path: "big.bin", name: "big.bin", folder: "", bytes: 2e9 }]);
});

test("no summary yet: its size alone, all of it Other; kinds past its size widen it", () => {
  const bare = storageParts({ disk: { total: 20e9, free: 9e9, data: 4e9, room: 12e9, at: 5 } });
  assert.deepEqual([bare.kinds, bare.files, bare.at, bare.filling], [[{ kind: "other", label: "Other", bytes: 4e9 }], [], 5, false]);
  const past = storageParts({ disk: { total: 20e9, free: 9e9, data: 4e9, room: 12e9, at: 5 }, sizes: { at: 6, folders: [], files: [], kinds: [{ kind: "projects", bytes: 5e9 }] } });
  assert.deepEqual([past.used, past.kinds.length], [5e9, 1], "never a negative Other");
  const files = Array.from({ length: 9 }, (_, i) => ({ path: `f${i}`, bytes: 9 - i }));
  assert.equal(storageParts({ disk: { data: 1, room: 1 }, sizes: { at: 1, folders: [], files, kinds: [] } }).files.length, 5, "the five biggest");
});

test("all of /data, its total known: what is used, Other what its kinds leave out; no room: Full", () => {
  const c = { disk: { total: 20e9, free: 2e8, data: 15e9, room: 0, filling: true, at: 3 }, sizes: { at: 4, total: 17e9, folders: [], files: [], kinds: [{ kind: "caches", bytes: 2e9 }, { kind: "projects", bytes: 13e9 }] } };
  const p = storageParts(c);
  assert.deepEqual([p.used, p.left, p.full, p.filling], [17e9, 0, true, true], "its caches too, which its save leaves out");
  assert.deepEqual(p.kinds.map((k) => [k.kind, k.bytes]), [["caches", 2e9], ["projects", 13e9], ["other", 2e9]]);
  assert.equal(storageParts({ disk: { data: 1, room: 1 } }).full, false);
});

test("what a view carries that is not as its wire says is left out, never thrown", () => {
  const d = { total: 20e9, free: 9e9, data: 4e9, room: 12e9, at: 5 };
  for (const sizes of [null, 7, "x", { kinds: "projects", files: { path: "a" } }, { kinds: [null, { kind: "toString", bytes: 9 }, { kind: "projects", bytes: -1 }, { kind: "projects", bytes: "9" }], files: [null, { path: 7, bytes: 1 }, { path: "", bytes: 1 }, { path: "a", bytes: -1 }, { path: "b" }] }]) {
    const p = storageParts({ disk: d, sizes });
    assert.deepEqual([p.kinds, p.files, p.used, p.at], [[{ kind: "other", label: "Other", bytes: 4e9 }], [], 4e9, 5], JSON.stringify(sizes));
  }
  for (const disk of [{ ...d, data: -1 }, { ...d, room: NaN }, { ...d, data: "4" }]) assert.equal(storageParts({ disk }), null, JSON.stringify(disk));
});

// Settings → Computer, as far as Storage: nodes that know their parent, so
// a block replaced is gone from the page, and one in it `isConnected`.
class Node {
  constructor(tag, cls, text) {
    Object.assign(this, { tag, className: cls ?? "", textContent: text ?? "", children: [], parent: null, dataset: {}, style: {}, hidden: false, attrs: {} });
  }
  get isConnected() {
    return this.root || !!this.parent?.isConnected;
  }
  append(...c) {
    for (const n of c) if (n instanceof Node) n.parent = this;
    this.children.push(...c.filter((n) => n instanceof Node));
  }
  replaceChildren(...c) {
    this.children = [];
    this.append(...c);
  }
  replaceWith(n) {
    const at = this.parent.children.indexOf(this);
    this.parent.children[at] = n;
    n.parent = this.parent;
    this.parent = null;
  }
  setAttribute(k, v) {
    this.attrs[k] = v;
  }
  querySelector(sel) {
    return walk(this).find((n) => n !== this && n.className.split(" ").includes(sel.replace(/^\./, ""))) ?? null;
  }
  querySelectorAll() {
    return [];
  }
  addEventListener() {}
}
const walk = (n) => [n, ...n.children.flatMap(walk)];

test("a view that lands while Settings shows draws its Storage again; the same view draws nothing new", () => {
  const box = new Node("div", "computer-notices");
  const cover = Object.assign(new Node("dialog", "must-answer"), { open: false, showModal() {}, close() {} });
  globalThis.document = { hidden: false, addEventListener() {}, removeEventListener() {}, getElementById: (id) => ({ "computer-notices": box, "must-answer": cover })[id] ?? null, createElementNS: (_, tag) => new Node(tag) };
  globalThis.MutationObserver ??= class {
    observe() {}
    disconnect() {}
  };
  const view = (room, filling) => ({ computer: "computer:aa", phase: "awake", generation: 3, notices: [], disk: { total: 20e9, free: 9e9, data: 14e9, room, filling, at: 10 }, sizes: { at: 12, total: 14e9, folders: [], files: [{ path: "work/a/talk.mp4", bytes: 3e9 }], kinds: [{ kind: "downloads", bytes: 3e9 }] } });
  let c = view(5e9, false);
  const el = (tag, cls, text) => new Node(tag, cls, text);
  const line = (key, value) => {
    const l = el("div", "settings-line");
    l.append(el("span", "settings-key", key), el("span", "settings-value", value));
    return l;
  };
  const section = (title, ...rows) => {
    const s = el("section", "settings-section");
    s.append(el("h2", "", title), ...rows);
    return s;
  };
  const reads = { updating: false, listFailed: false, noticesFailed: false };
  const ui = createComputer({
    api: () => new Promise(() => {}), el, seg: (s) => s, named: (n) => n, section, line, actionLine: line,
    computer: () => c, notices: () => [], agents: () => new Map(), computerFailed: () => false, setupPending: () => false,
    made: () => {}, computerIs: () => {}, openScreen: () => {}, markOffer: (n) => n, reads: () => reads, load: async () => {}, openPage: () => {},
  });
  const page = Object.assign(new Node("main"), { root: true });
  page.append(ui.computerSection(c));
  const storage = () => walk(page).find((n) => n.dataset.storage === "");
  const left = () => storage().querySelector(".storage-left");
  assert.deepEqual([left().textContent, left().className.includes("storage-low")], ["5.0 GB left", false]);
  const first = storage();
  ui.renderComputer();
  assert.equal(storage(), first, "the same view: the same block");
  c = view(0, true);
  ui.renderComputer();
  assert.notEqual(storage(), first, "drawn again");
  assert.deepEqual([left().textContent, left().className.includes("storage-low"), storage().querySelector(".storage-bar").attrs["aria-label"]], ["Full", true, "14.0 GB used, full"]);
  assert.equal(walk(page).filter((n) => n.dataset.storage === "").length, 1, "once");
  ui.dispose();
});

test("Delete: never on a path with a ?, asked once; a view that changes nothing says Deleted", async () => {
  const box = new Node("div", "computer-notices");
  const cover = Object.assign(new Node("dialog", "must-answer"), { open: false, showModal() {}, close() {} });
  globalThis.document = { hidden: false, addEventListener() {}, removeEventListener() {}, getElementById: (id) => ({ "computer-notices": box, "must-answer": cover })[id] ?? null, createElementNS: (_, tag) => new Node(tag) };
  globalThis.MutationObserver ??= class {
    observe() {}
    disconnect() {}
  };
  const files = [{ path: "work/a/talk.mp4", bytes: 3e9 }, { path: "work/a/odd?name", bytes: 2e9 }];
  const c = { computer: "computer:aa", phase: "awake", generation: 3, notices: [], disk: { total: 20e9, free: 9e9, data: 14e9, room: 5e9, at: 10 }, sizes: { at: 12, total: 14e9, folders: [], files, kinds: [] } };
  const el = (tag, cls, text) => new Node(tag, cls, text);
  const line = (key, value) => {
    const l = el("div", "settings-line");
    l.append(el("span", "settings-key", key), el("span", "settings-value", value));
    return l;
  };
  const section = (title, ...rows) => {
    const s = el("section", "settings-section");
    s.append(el("h2", "", title), ...rows);
    return s;
  };
  const asked = [];
  // its delete answers the view as it was (its read after it did not answer in time)
  const api = (method, path, body) => (asked.push({ method, path, body }), path.endsWith("/files/delete") ? Promise.resolve(c) : new Promise(() => {}));
  const reads = { updating: false, listFailed: false, noticesFailed: false };
  const ui = createComputer({
    api, el, seg: (s) => s, named: (n) => n, section, line, actionLine: line,
    computer: () => c, notices: () => [], agents: () => new Map(), computerFailed: () => false, setupPending: () => false,
    made: () => {}, computerIs: () => ui.renderComputer(), openScreen: () => {}, markOffer: (n) => n, reads: () => reads, load: async () => {}, openPage: () => {},
  });
  const page = Object.assign(new Node("main"), { root: true });
  page.append(ui.computerSection(c));
  const row = (path) => walk(page).find((n) => n.dataset.path === path);
  const button = (path, action) => walk(row(path)).find((n) => n.dataset.action === action);
  assert.ok(button("work/a/talk.mp4", "storage-delete"), "a file of its work");
  assert.equal(button("work/a/odd?name", "storage-delete"), undefined, "never a path with a ? (a newline, as GNU lists it)");
  button("work/a/talk.mp4", "storage-delete").onclick();
  assert.equal(asked.length, 0, "asked first");
  button("work/a/talk.mp4", "storage-delete-sure").onclick();
  assert.deepEqual(asked.map((a) => [a.method, a.path, a.body]), [["POST", "/api/computers/computer:aa/files/delete", { path: "work/a/talk.mp4" }]]);
  await new Promise((r) => setTimeout(r, 0));
  assert.deepEqual(walk(row("work/a/talk.mp4")).filter((n) => n.className === "muted").map((n) => n.textContent), ["Deleted"], "said, never left at Deleting…");
  ui.dispose();
});
