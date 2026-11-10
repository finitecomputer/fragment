// cell/shell/computer.js's notices band (FIN-458): every notice the server
// tells (`GET /api/notices`) drawn the same way, whatever its kind: its
// words, its countdown, and its actions, each a route posted with its body
// and then one more read (the newest wins), or a page opened. The real
// `createComputer`, its page faked as far as the band; each call answers
// only when the test says, so answers come in any order.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { createComputer, untilSaid } from "../shell/computer.js";

class Node {
  constructor(tag, cls, text) {
    Object.assign(this, { tag, className: cls ?? "", textContent: text ?? "", children: [], dataset: {}, hidden: false, disabled: false, onclick: null, attrs: {} });
    this.classList = { add: (c) => (this.className += ` ${c}`) };
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
  contains(n) {
    return walk(this).includes(n);
  }
  querySelector(tag) {
    return walk(this).find((n) => n.tag === tag) ?? null;
  }
  querySelectorAll(tag) {
    return walk(this).filter((n) => n.tag === tag);
  }
  focus() {
    globalThis.document.activeElement = this;
    // `focusin`, heard by the dialog it is in (the cover's listener)
    globalThis.document.focused?.(this);
  }
  addEventListener(type, heard) {
    (this.listeners ??= []).push({ type, heard });
  }
}
const walk = (n) => [n, ...n.children.flatMap(walk)];
const settle = () => new Promise((r) => setTimeout(r, 0));

// The page's clock and timers, a test's.
function clock(start = 0) {
  let now = start;
  let next = 1;
  const due = new Map();
  return {
    now: () => now,
    pending: () => [...due.values()].map((t) => t.at),
    setTimeout(fn, ms) {
      due.set(next, { at: now + ms, fn });
      return next++;
    },
    clearTimeout(id) {
      due.delete(id);
    },
    tick(ms) {
      now += ms;
      for (const [id, t] of [...due]) {
        if (t.at <= now) {
          due.delete(id);
          t.fn();
        }
      }
    },
  };
}

const COMPUTER = "computer:aa";
const RESTART = `/api/computers/${COMPUTER}/restart`;
const allow = (cap) => ({ label: "Allow up to $20 an hour", post: `/api/computers/${COMPUTER}/key-caps/elevenlabs`, body: { from: cap } });
const paused = (o = {}) => ({ id: "key_paused:elevenlabs", kind: "key_paused", level: "warn", title: "ElevenLabs calls paused: $5.03 this hour", actions: [allow(5_000_000)], ...o });

// The page's top layer, as the HTML dialog rules keep it: a modal dialog
// shown goes on top, one closed leaves it; what is on top takes the keys.
// Its watchers (`MutationObserver` on `open`) hear each change as a page's
// would, when the test says (a microtask, in a browser).
function topLayer() {
  const layer = [];
  const watchers = [];
  const dialog = (cls) =>
    Object.assign(new Node("dialog", cls), {
      open: false,
      showModal() {
        layer.splice(layer.indexOf(this) >>> 0, layer.includes(this) ? 1 : 0);
        layer.push(this);
        this.open = true;
        changed.push(this);
      },
      close() {
        if (layer.includes(this)) layer.splice(layer.indexOf(this), 1);
        this.open = false;
        changed.push(this);
      },
    });
  const changed = [];
  globalThis.MutationObserver = class {
    constructor(heard) {
      this.heard = heard;
    }
    observe() {
      watchers.push(this);
    }
    disconnect() {
      watchers.splice(watchers.indexOf(this) >>> 0, 1);
    }
  };
  return {
    dialog,
    top: () => layer.at(-1) ?? null,
    // what the page's watchers hear of the dialogs opened and closed since
    heard() {
      const now = changed.splice(0).map((target) => ({ type: "attributes", attributeName: "open", target }));
      if (now.length) for (const w of [...watchers]) w.heard(now);
    },
  };
}

function page(t, first, c = clock(Date.now())) {
  const box = new Node("div", "computer-notices");
  const layer = topLayer();
  // the page's one cover (index.html `#must-answer`), a modal dialog
  const cover = layer.dialog("must-answer");
  globalThis.document = {
    hidden: false, addEventListener() {}, removeEventListener() {}, getElementById: (id) => ({ "computer-notices": box, "must-answer": cover })[id] ?? null,
    focused: (n) => {
      if (cover.contains(n)) for (const l of cover.listeners ?? []) if (l.type === "focusin") l.heard({ target: n });
    },
  };
  const calls = [];
  const api = (method, path, body) => new Promise((ok, fail) => calls.push({ method, path, body, ok: (v) => ok(v), fail: (e) => fail(e) }));
  const reads = { asked: 0, shown: 0, updating: false, listFailed: false, noticesFailed: false };
  const opened = [];
  const shell = { notices: first, reads, computer: { computer: COMPUTER, phase: "awake", generation: 3, notices: [] } };
  let ui = null;
  // shell.js `load`, as far as the notices: an older read's answer that
  // comes after a newer one's is not shown
  const load = async () => {
    const mine = ++reads.asked;
    const answer = await api("GET", "/api/notices");
    if (mine < reads.shown) return;
    reads.shown = mine;
    shell.notices = answer.notices;
    ui.renderComputer();
  };
  const stub = () => new Node("div");
  ui = createComputer({
    api, el: (tag, cls, text) => new Node(tag, cls, text), seg: (s) => s, named: (n) => n, section: stub, line: stub, actionLine: stub,
    computer: () => shell.computer, notices: () => shell.notices, agents: () => new Map(), computerFailed: () => false, setupPending: () => true,
    made: () => {}, computerIs: () => {}, openScreen: () => {}, markOffer: () => {}, reads: () => reads, load, timers: c, now: c.now, openPage: (url) => opened.push(url),
  });
  ui.renderComputer();
  t.after(() => {
    shell.notices = [];
    ui.renderComputer();
    ui.dispose();
  });
  const rows = () => box.children.filter((r) => !r.hidden);
  // a row: its id, its words, and its actions as drawn
  const drawn = (r) => ({
    id: r.dataset.id,
    words: walk(r).filter((n) => n.tag === "strong" || n.tag === "p").map((n) => n.textContent),
    actions: walk(r).filter((n) => n.tag === "button" || n.tag === "a").map((n) => (n.tag === "a" ? { open: n.href } : { label: n.textContent, enabled: !n.disabled })),
  });
  const press = (row, label) => walk(row).find((n) => n.tag === "button" && n.textContent === label).onclick();
  // the cover's notice (its page's own rows, a read that failed, above it)
  const askedRow = () => cover.children.find((r) => r.dataset.id);
  return {
    box, cover, calls, load, clock: c, shell, ui, opened, layer, reads,
    shown: () => rows().map(drawn),
    tap(id, label) {
      press(rows().find((r) => r.dataset.id === id), label);
    },
    // what the cover shows, open; else null
    asked: () => (cover.open ? drawn(askedRow()) : null),
    answer: (label) => press(askedRow(), label),
    // the cover's buttons as drawn, by label
    coverButton: (label) => walk(cover).find((n) => n.tag === "button" && n.textContent === label),
  };
}

test("a notice of a kind the shell has never seen is drawn like any other: its words, its detail, a button for each post and a link for each page", (t) => {
  const p = page(t, [
    { id: "brand_new", kind: "brand_new", level: "warn", title: "Something new", text: "Said by the server.", detail: "its own words", actions: [{ label: "Do it", post: "/api/new/do", body: { a: 1 } }, { label: "Get help", open: "mailto:help@example.com" }] },
  ]);
  assert.deepEqual(p.shown(), [
    { id: "brand_new", words: ["Something new", "Said by the server.", "its own words"], actions: [{ label: "Do it", enabled: true }, { open: "mailto:help@example.com" }] },
  ]);
  assert.ok(p.box.children[0].className.includes("warn"), "its level, drawn");
  assert.equal(p.box.hidden, false);
});

test("a tap posts its body to its route, then reads again; the newest read decides what shows", async (t) => {
  const p = page(t, [paused()]);
  p.tap("key_paused:elevenlabs", "Allow up to $20 an hour");
  assert.deepEqual([p.calls[0].method, p.calls[0].path, p.calls[0].body], ["POST", `/api/computers/${COMPUTER}/key-caps/elevenlabs`, { from: 5_000_000 }]);
  assert.equal(p.shown()[0].actions[0].enabled, false, "pressed once");
  // the page's watch reads meanwhile, before the lift: the pause, as it was
  p.load();
  p.calls[1].ok({ notices: [paused()] });
  await settle();
  // the tap answers, and the read after it is the newest
  p.calls[0].ok({ computer: COMPUTER });
  await settle();
  assert.deepEqual([p.calls[2].method, p.calls[2].path], ["GET", "/api/notices"]);
  p.calls[2].ok({ notices: [] });
  await settle();
  assert.deepEqual(p.shown(), [], "lifted: nothing shown");
  assert.equal(p.box.hidden, true);
});

test("a tap refused: the notice stays as read, its button back with a short line, and tapped again it goes through", async (t) => {
  const p = page(t, [paused()]);
  p.tap("key_paused:elevenlabs", "Allow up to $20 an hour");
  p.calls[0].fail(Object.assign(new Error("That key isn't paused any more."), { status: 409 }));
  await settle();
  assert.equal(p.calls[1].path, "/api/notices", "read again: the read decides");
  p.calls[1].ok({ notices: [paused()] });
  await settle();
  assert.deepEqual(p.shown(), [{ id: "key_paused:elevenlabs", words: ["ElevenLabs calls paused: $5.03 this hour", "That key isn't paused any more."], actions: [{ label: "Allow up to $20 an hour", enabled: true }] }], "the server's own words for its refusal");
  p.tap("key_paused:elevenlabs", "Allow up to $20 an hour");
  p.calls[2].ok({ computer: COMPUTER });
  await settle();
  p.calls[3].ok({ notices: [paused()] });
  await settle();
  assert.deepEqual(p.shown()[0].words, ["ElevenLabs calls paused: $5.03 this hour"], "it went through: no line");
});

test("a tap refused whose read shows the notice gone: nothing left behind", async (t) => {
  const p = page(t, [paused()]);
  p.tap("key_paused:elevenlabs", "Allow up to $20 an hour");
  p.calls[0].fail(new TypeError("Failed to fetch"));
  await settle();
  p.calls[1].ok({ notices: [] });
  await settle();
  assert.deepEqual(p.shown(), []);
  // a new pause of the same key later is a new notice, with no old line
  p.load();
  p.calls[2].ok({ notices: [paused()] });
  await settle();
  assert.deepEqual(p.shown()[0].words, ["ElevenLabs calls paused: $5.03 this hour"]);
});

test("a countdown: said live, each minute, and the notice gone as it ends, with one timer at a time and no call", (t) => {
  const c = clock(0);
  const p = page(t, [paused({ until: { at: 150_000, said: "Calls resume" } }), { id: "update:abc", kind: "update", level: "info", title: "An update is ready", actions: [] }], c);
  assert.deepEqual(p.shown()[0].words, ["ElevenLabs calls paused: $5.03 this hour", "Calls resume in 3 min."]);
  assert.deepEqual(c.pending(), [30_000], "one timer, as its minute changes");
  const button = p.box.children[0].children[1].children[0];
  c.tick(30_000);
  assert.deepEqual(p.shown()[0].words.at(-1), "Calls resume in 2 min.");
  assert.equal(p.box.children[0].children[1].children[0], button, "said again in place: its button stays");
  c.tick(59_999);
  assert.deepEqual(p.shown()[0].words.at(-1), "Calls resume in 2 min.");
  c.tick(1);
  assert.deepEqual(p.shown()[0].words.at(-1), "Calls resume in under a minute.");
  c.tick(59_999);
  assert.equal(p.shown().length, 2);
  c.tick(1);
  assert.deepEqual(p.shown().map((r) => r.id), ["update:abc"], "ended: gone, the rest as they were");
  assert.deepEqual(c.pending(), [], "nothing left to wait for");
  assert.equal(p.calls.length, 0, "no call");
});

test("a countdown in words", () => {
  const u = { at: 0, said: "Calls resume" };
  assert.equal(untilSaid(u, -59_000), "Calls resume in under a minute.");
  assert.equal(untilSaid(u, -60_000), "Calls resume in under a minute.");
  assert.equal(untilSaid(u, -61_000), "Calls resume in 2 min.");
  assert.equal(untilSaid(u, -3_600_000), "Calls resume in 60 min.");
  assert.equal(untilSaid(u, -3_660_000), "Calls resume in 1 h 1 min.");
  assert.equal(untilSaid(u, -7_200_000), "Calls resume in 2 h.");
});

test("the computer's Restart is its restart, followed where the notice was, and the notices it answers wait while it runs", async (t) => {
  const update = { id: "update:abc", kind: "update", level: "info", title: "An update is ready", actions: [{ label: "Restart", post: RESTART, body: { generation: 3 } }, { label: "Later", post: `/api/computers/${COMPUTER}/notices/later`, body: { version: "abc" } }] };
  const disk = { id: "disk_filling", kind: "disk_filling", level: "warn", title: "Your computer is almost full (3.2 GB left)", actions: [] };
  const p = page(t, [update, disk]);
  p.tap("update:abc", "Restart");
  assert.deepEqual([p.calls[0].method, p.calls[0].path, p.calls[0].body], ["POST", RESTART, { generation: 3 }], "the restart's own ask, of the start shown");
  assert.deepEqual(p.shown().map((r) => r.id ?? r.words[0]), ["Restarting your computer", "disk_filling"], "its progress where the update was; the disk's notice stays");
  assert.deepEqual(p.shown()[0].words, ["Restarting your computer"]);
});

test("the notices' read failing: what it last said stays, under the page's own Couldn't refresh line with Try again", async (t) => {
  const update = { id: "update:abc", kind: "update", level: "info", title: "An update is ready", actions: [] };
  const p = page(t, [update]);
  p.ui.renderComputer();
  assert.deepEqual(p.shown().map((r) => r.id), ["update:abc"]);
  // shell.js `load`: its notices read failed (a blip, a 5xx), the others answered
  p.shell.reads.noticesFailed = true;
  p.ui.renderComputer();
  assert.deepEqual(p.shown().map((r) => r.id ?? r.words[0]), ["Couldn't refresh your computer", "update:abc"]);
  assert.deepEqual(p.shown()[0].actions, [{ label: "Try again", enabled: true }]);
});

test("a notice's Restart restarts the start it named, never a newer one: a stale row's press changes nothing (its generation)", (t) => {
  // the notices read older than the computer's: the update was told of start 3, the computer is on 4 now
  const stale = { id: "update:abc", kind: "update", level: "info", title: "An update is ready", actions: [{ label: "Restart", post: RESTART, body: { generation: 3 } }] };
  const p = page(t, [stale]);
  p.shell.computer = { ...p.shell.computer, generation: 4 };
  p.ui.renderComputer();
  p.tap("update:abc", "Restart");
  assert.deepEqual([p.calls[0].method, p.calls[0].path, p.calls[0].body], ["POST", RESTART, { generation: 3 }], "the action's own precondition, as the server wrote it");
});

test("a tap that never reached the server (the network): its notice says so in the page's words, to tap again", async (t) => {
  const p = page(t, [paused()]);
  p.tap("key_paused:elevenlabs", "Allow up to $20 an hour");
  p.calls[0].fail(new TypeError("Failed to fetch"));
  await settle();
  p.calls[1].ok({ notices: [paused()] });
  await settle();
  assert.deepEqual(p.shown()[0].words, ["ElevenLabs calls paused: $5.03 this hour", "Couldn't do that. Try again."]);
});

test("an action's answer that names a page opens it, once the read after it has it (an invite's app); one that names none opens nothing", async (t) => {
  const invite = { id: "invite:garden--k3x9", kind: "invite", level: "info", title: "Garden plans", text: "ada@example.com invited you to edit it.", actions: [{ label: "Accept", post: "/api/invites/garden--k3x9/accept", body: {} }, { label: "Decline", post: "/api/invites/garden--k3x9/decline", body: {} }] };
  const p = page(t, [invite]);
  p.tap("invite:garden--k3x9", "Accept");
  p.calls[0].ok({ fragment: "garden--k3x9", role: "editor", open: "https://garden--k3x9.fragment.test/" });
  await settle();
  assert.deepEqual(p.opened, [], "not before the read: the list may not have it yet");
  p.calls[1].ok({ notices: [] });
  await settle();
  assert.deepEqual(p.opened, ["https://garden--k3x9.fragment.test/"]);
  const q = page(t, [invite]);
  q.tap("invite:garden--k3x9", "Decline");
  q.calls[0].ok({ ok: true, declined: "garden--k3x9" });
  await settle();
  q.calls[1].ok({ notices: [] });
  await settle();
  assert.deepEqual(q.opened, []);
});

test("a notice that must be answered shows alone, over the page and never in the band, the first of them first, until the read after an answer of it has it no more", async (t) => {
  const ask = (id, title) => ({
    id: `asked:${id}`, kind: "asked", level: "info", title, text: "Said by the server.", mustAnswer: true,
    actions: ["Yes", "Not now"].map((answer) => ({ label: answer, post: `/api/notices/asked/${id}`, body: { answer } })),
  });
  const first = ask("move", "Move to finite.chat?"), second = ask("terms", "New terms");
  const p = page(t, [paused(), first, second]);
  assert.deepEqual(p.shown().map((r) => r.id), ["key_paused:elevenlabs"], "the band: none that must be answered");
  assert.deepEqual(p.asked(), { id: "asked:move", words: ["Move to finite.chat?", "Said by the server."], actions: [{ label: "Yes", enabled: true }, { label: "Not now", enabled: true }] }, "the first, alone, over the page");
  // refused: it stays, saying why in the server's words
  p.answer("Not now");
  p.calls[0].fail(Object.assign(new Error("Not yet: answer Yes, or Never."), { status: 409 }));
  await settle();
  p.calls[1].ok({ notices: [paused(), first, second] });
  await settle();
  assert.deepEqual(p.asked().words, ["Move to finite.chat?", "Said by the server.", "Not yet: answer Yes, or Never."]);
  assert.equal(globalThis.document.activeElement?.textContent, "Yes", "drawn again, its first answer has the focus");
  // answered, and the read after it has it no more: the next
  p.answer("Yes");
  assert.deepEqual([p.calls[2].path, p.calls[2].body], ["/api/notices/asked/move", { answer: "Yes" }]);
  p.calls[2].ok({ answered: "move" });
  await settle();
  assert.equal(p.asked().id, "asked:move", "until the read after it: nothing taken away before");
  p.calls[3].ok({ notices: [paused(), second] });
  await settle();
  assert.equal(p.asked().id, "asked:terms");
  p.answer("Yes");
  p.calls[4].ok({ answered: "terms" });
  await settle();
  p.calls[5].ok({ notices: [paused()] });
  await settle();
  assert.equal(p.asked(), null, "none left: the page, as it was");
  assert.deepEqual(p.shown().map((r) => r.id), ["key_paused:elevenlabs"]);
});

// An invite, as the registry tells it, for the tests below.
const invited = () => ({
  id: "invite:garden--k3x9", kind: "invite", level: "info", title: "Garden plans", text: "ada@example.com invited you to edit it.",
  actions: [{ label: "Accept", post: "/api/invites/garden--k3x9/accept", body: {} }, { label: "Decline", post: "/api/invites/garden--k3x9/decline", body: {} }],
});
const gone = "No invite to garden--k3x9 waits on you: it was taken back, answered, or ran out.";

test("a refusal outlives its notice: the read after it no longer has it (an invite taken back), and its words stay, a line of their own, until dismissed", async (t) => {
  const p = page(t, [invited()]);
  p.tap("invite:garden--k3x9", "Accept");
  p.calls[0].fail(Object.assign(new Error(gone), { status: 404 }));
  await settle();
  p.calls[1].ok({ notices: [] });
  await settle();
  assert.deepEqual(p.shown(), [{ id: "invite:garden--k3x9", words: ["Garden plans", gone], actions: [{ label: "OK", enabled: true }] }], "its title and the server's words");
  assert.deepEqual(p.opened, [], "nothing opened");
  // another read meanwhile changes nothing of it
  p.load();
  p.calls[2].ok({ notices: [] });
  await settle();
  assert.deepEqual(p.shown().map((r) => r.words), [["Garden plans", gone]]);
  p.tap("invite:garden--k3x9", "OK");
  assert.deepEqual(p.shown(), [], "dismissed, it goes");
  assert.equal(p.calls.length, 3, "and dismissing asks nothing");
});

test("a refusal whose read fails: the notice as last read, with the server's words under it, to tap again", async (t) => {
  const p = page(t, [invited()]);
  p.tap("invite:garden--k3x9", "Decline");
  p.calls[0].fail(Object.assign(new Error(gone), { status: 404 }));
  await settle();
  p.calls[1].fail(new TypeError("Failed to fetch"));
  await settle();
  assert.deepEqual(p.shown(), [{ id: "invite:garden--k3x9", words: ["Garden plans", "ada@example.com invited you to edit it.", gone], actions: [{ label: "Accept", enabled: true }, { label: "Decline", enabled: true }] }]);
});

for (const [first, second] of [["Accept", "Decline"], ["Decline", "Accept"]]) {
  test(`a notice's actions are one answer: ${first} pending, every action of it is off, through redraws, and ${second} asks nothing, until its read`, async (t) => {
    const p = page(t, [invited()]);
    p.tap("invite:garden--k3x9", first);
    const off = [{ label: "Accept", enabled: false }, { label: "Decline", enabled: false }];
    assert.deepEqual(p.shown()[0].actions, off, "both off at once");
    // the page's watch reads meanwhile: drawn again, still both off
    p.load();
    p.calls[1].ok({ notices: [invited()] });
    await settle();
    assert.deepEqual(p.shown()[0].actions, off, "through a redraw");
    p.tap("invite:garden--k3x9", second);
    assert.equal(p.calls.filter((c) => c.method === "POST").length, 1, `${second} posts nothing while ${first} is out`);
    // its answer, slow, comes; the read after it decides
    p.calls[0].ok(first === "Accept" ? { fragment: "garden--k3x9", role: "editor" } : { ok: true, declined: "garden--k3x9" });
    await settle();
    p.calls[2].ok({ notices: [] });
    await settle();
    assert.deepEqual(p.shown(), [], "answered once");
    assert.deepEqual(p.calls.filter((c) => c.method === "POST").map((c) => c.path), [`/api/invites/garden--k3x9/${first.toLowerCase()}`]);
  });
}


test("a refusal outlives its notice's time too: Allow refused, the pause's countdown ends before another read, and the words stay, a line of their own", async (t) => {
  const c = clock(0);
  const pause = () => paused({ until: { at: 150_000, said: "Calls resume" } });
  const p = page(t, [pause()], c);
  p.tap("key_paused:elevenlabs", "Allow up to $20 an hour");
  p.calls[0].fail(Object.assign(new Error("That key isn't paused any more."), { status: 409 }));
  await settle();
  p.calls[1].ok({ notices: [pause()] });
  await settle();
  assert.deepEqual(p.shown()[0].words.at(-1), "That key isn't paused any more.", "under its notice while it shows");
  c.tick(150_000);
  assert.deepEqual(p.shown(), [{ id: "key_paused:elevenlabs", words: ["ElevenLabs calls paused: $5.03 this hour", "That key isn't paused any more."], actions: [{ label: "OK", enabled: true }] }], "its time passed: a line of its own, with OK");
  p.tap("key_paused:elevenlabs", "OK");
  assert.deepEqual(p.shown(), [], "dismissed, it goes");
});

// A notice that must be answered, as an invite to a `must-answer` fragment
// is on a local fleet, for the tests below.
const mustAnswer = (name, title) => ({
  id: `invite:${name}`, kind: "invite", level: "info", title, text: "ada@example.com invited you to view it.", mustAnswer: true,
  actions: [{ label: "Accept", post: `/api/invites/${name}/accept`, body: {} }, { label: "Decline", post: `/api/invites/${name}/decline`, body: {} }],
});

test("the answer that has the cover's focus keeps it through a redraw of the same notice, so Enter is the one the person chose", async (t) => {
  const asked = mustAnswer("must-answer-1--k3x9", "Garden plans");
  const p = page(t, [asked]);
  // they tab to Decline; the page's watch reads meanwhile, the same notice
  p.coverButton("Decline").focus();
  p.load();
  p.calls[0].ok({ notices: [asked] });
  await settle();
  assert.equal(globalThis.document.activeElement?.textContent, "Decline", "drawn again, Decline still has the focus");
  assert.ok(p.cover.contains(globalThis.document.activeElement), "the new Decline: the one drawn now");
  // their Decline, refused once: under way its buttons are off, and back, the focus is theirs again
  globalThis.document.activeElement.onclick();
  p.calls[1].fail(Object.assign(new Error("the identity registry did not answer; try again shortly"), { status: 503 }));
  await settle();
  p.calls[2].ok({ notices: [asked] });
  await settle();
  assert.equal(globalThis.document.activeElement?.textContent, "Decline", "after the refusal too");
});

test("a dialog the page opens over the cover (a frame's ask, an answer's continuation) waits under it: the cover is shown again on top, and the other is the page's once it is answered", async (t) => {
  const asked = mustAnswer("must-answer-1--k3x9", "Garden plans");
  const p = page(t, [asked]);
  p.layer.heard();
  // they move to Decline; the other opens, and the browser moves the focus into it
  p.coverButton("Decline").focus();
  const other = p.layer.dialog("add-agent");
  other.showModal();
  other.focus();
  p.layer.heard();
  assert.equal(p.layer.top(), p.cover, "the cover, on top again");
  assert.ok(other.open, "the other still open, under it");
  assert.equal(globalThis.document.activeElement?.textContent, "Decline", "the answer they chose has the focus again");
  p.answer("Decline");
  p.calls[0].ok({ ok: true, declined: "must-answer-1--k3x9" });
  await settle();
  p.calls[1].ok({ notices: [] });
  await settle();
  p.layer.heard();
  assert.equal(p.cover.open, false);
  assert.equal(p.layer.top(), other, "answered: the other is the page's");
});

test("a read that fails while a notice must be answered says so in the cover, with Try again: the notice as last read stays under it", async (t) => {
  const asked = mustAnswer("must-answer-1--k3x9", "Garden plans");
  const p = page(t, [asked]);
  // their answer goes through, and the read after it fails
  p.answer("Decline");
  p.calls[0].ok({ ok: true, declined: "must-answer-1--k3x9" });
  await settle();
  p.reads.noticesFailed = true;
  p.calls[1].fail(Object.assign(new Error("the read failed"), { status: 503 }));
  await settle();
  p.ui.renderComputer();
  assert.ok(p.cover.open, "the notice as last read, still over the page");
  const failed = p.cover.children.find((r) => r.dataset.kind === "refresh");
  assert.ok(failed, "the read's failure, in the cover");
  assert.equal(walk(failed).find((n) => n.tag === "strong")?.textContent, "Couldn't refresh your computer");
  p.coverButton("Try again").onclick();
  assert.deepEqual(p.calls.at(-1).path, "/api/notices", "Try again reads again");
  p.reads.noticesFailed = false;
  p.calls.at(-1).ok({ notices: [] });
  await settle();
  assert.equal(p.cover.open, false, "read again without it: the page, as it was");
});
