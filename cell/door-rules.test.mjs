// door-rules.mjs: a SimpleX door's rules. Valid (the newest readable save
// restored, told behind only past an effect it lacks, the newest three
// kept), invalid (an unreadable save never restored again, a stale start's
// save fenced, a door with saves never started as a new one over them),
// and its schema's steps held to their released text.
import { test } from "node:test";
import assert from "node:assert/strict";
import { FILES_BYTES_MAX, FILES_KEPT, FILE_KEPT_MS, REPLY_FILES_MAX, STEPS, SAVES_KEPT, behind, fileKey, fileRoom, replyFiles, shaOk, typeOf, doorOf, doorPrefix, fenced, inboxUrl, nameOk, parseSaveId, restorable, restoreOf, saveId, saveKey, secretOk, stale, startable, steps, surplus } from "./door-rules.mjs";

const save = (generation, seq, mark, unreadable = 0) => ({ generation, seq, mark, unreadable });

test("a start restores the newest save that is not unreadable", () => {
  assert.equal(restorable([]), null);
  const saves = [save(1, 3, 4), save(2, 1, 7), save(1, 9, 5)];
  assert.deepEqual(restorable(saves), save(2, 1, 7), "a later start's first save is newer than an earlier start's last");
  assert.deepEqual(restorable([...saves, save(2, 2, 8, 1)]), save(2, 1, 7), "one found unreadable is passed over");
  assert.equal(restorable([save(1, 1, 0, 1)]), null);
});

test("a save is behind only when the door numbered an effect past it", () => {
  assert.equal(behind(save(1, 1, 7), 7), false);
  assert.equal(behind(save(1, 1, 7), 8), true);
});

test("only the running start's saves are taken", () => {
  assert.equal(fenced(3, 3), false);
  assert.equal(fenced(2, 3), true, "a start superseded");
  assert.equal(fenced(4, 3), true, "a start the door never made");
});

test("the newest readable saves are kept, the rest let go", () => {
  const saves = [save(1, 1, 0), save(1, 2, 1), save(2, 1, 2, 1), save(2, 2, 3), save(2, 3, 4)];
  assert.deepEqual(surplus(saves), [save(1, 1, 0), save(2, 1, 2, 1)], `the newest ${SAVES_KEPT} readable kept`);
  assert.deepEqual(surplus(saves.slice(0, 2)), []);
});

test("a door that ever saved starts only from a save, never as a new one", () => {
  assert.deepEqual(restoreOf([], 0), { kind: "none" }, "a new door");
  assert.deepEqual(restoreOf([save(1, 1, 0, 1), save(1, 2, 0)], 2), { kind: "save", save: save(1, 2, 0) });
  assert.deepEqual(restoreOf([save(1, 1, 0, 1), save(1, 2, 0, 1)], 2), { kind: "exhausted" }, "every save unreadable");
  assert.deepEqual(restoreOf([], 5), { kind: "exhausted" }, "its saves pruned or gone: it saved, so never a new profile");
  assert.equal(startable([], 0), true);
  assert.equal(startable([], 5), false);
});

test("a save is named back exactly", () => {
  assert.equal(saveId(save(12, 3, 0)), "12-3");
  assert.deepEqual(parseSaveId("12-3"), { generation: 12, seq: 3 });
  for (const bad of ["12", "a-1", "1-2-3", "", null, 7, "1-" + "9".repeat(16)]) assert.equal(parseSaveId(bad), null, String(bad));
});

test("a door's name is a fragment's, and its saves are under its own prefix", () => {
  assert.equal(saveKey("pat-simplex--k3x9", 2, 5), "doors/pat-simplex--k3x9/saves/2-5");
  for (const bad of ["", "../x", "A", "a/b", "x".repeat(97), 7]) {
    assert.equal(nameOk(bad), false, String(bad));
  }
  assert.throws(() => saveKey("../x", 1, 1));
  // its end deletes its prefix whole: every save's key is under it, and
  // no other door's (a name that begins another's) is
  assert.equal(doorPrefix("pat-simplex--k3x9"), "doors/pat-simplex--k3x9/");
  assert.ok(saveKey("pat-simplex--k3x9", 2, 5).startsWith(doorPrefix("pat-simplex--k3x9")));
  assert.ok(!saveKey("pat-simplex--k3x9a", 1, 1).startsWith(doorPrefix("pat-simplex--k3x9")), "a longer name is another prefix");
  assert.throws(() => doorPrefix("../x"));
});

// Each released step's text, as it was released: a step is never edited or
// dropped (stored rows stay readable across releases), only added after.
const RELEASED = [
  "CREATE TABLE door (id INTEGER PRIMARY KEY CHECK (id = 1), door TEXT NOT NULL, agent TEXT NOT NULL, started INTEGER NOT NULL, generation INTEGER NOT NULL, marks INTEGER NOT NULL, saved INTEGER NOT NULL, stopped TEXT)",
  "CREATE TABLE saves (generation INTEGER NOT NULL, seq INTEGER NOT NULL, mark INTEGER NOT NULL, bytes INTEGER NOT NULL, at INTEGER NOT NULL, unreadable INTEGER NOT NULL, PRIMARY KEY (generation, seq))",
  "CREATE TABLE deliveries (id TEXT PRIMARY KEY, mark INTEGER NOT NULL, body TEXT NOT NULL, at INTEGER NOT NULL)",
  "CREATE TABLE inbox (id INTEGER PRIMARY KEY CHECK (id = 1), fragment TEXT NOT NULL, secret TEXT NOT NULL, asked INTEGER NOT NULL DEFAULT 0)",
  "ALTER TABLE deliveries ADD COLUMN sent INTEGER NOT NULL DEFAULT 0",
  "CREATE TABLE files (sha TEXT PRIMARY KEY, bytes INTEGER NOT NULL, type TEXT NOT NULL, at INTEGER NOT NULL)",
];

test("the schema's released steps are as released, and taken in order", () => {
  assert.ok(STEPS.length >= RELEASED.length, "no released step dropped");
  assert.deepEqual(STEPS.slice(0, RELEASED.length), RELEASED, "no released step edited");
  assert.deepEqual(steps(0).map((s) => s.version), STEPS.map((_, i) => i + 1));
  assert.deepEqual(steps(STEPS.length), []);
  assert.throws(() => steps(STEPS.length + 1));
  assert.throws(() => steps(-1));
});

test("a connector's door is its name and its life's digits", () => {
  assert.equal(doorOf("pat-sx--k3x9--0123456789ab", "pat-sx--k3x9"), true);
  for (const [door, connector] of [["pat-sx--k3x9--0123456789ab", "pat-sx--k3x8"], ["pat-sx--k3x9", "pat-sx--k3x9"], ["pat-sx--k3x9--0123456789AB", "pat-sx--k3x9"], ["pat-sx--k3x9--0123", "pat-sx--k3x9"], ["x--0123456789ab", "../x"]]) {
    assert.equal(doorOf(door, connector), false, `${door} of ${connector}`);
  }
});

test("its connector's inbox is on the platform's origin, at the connector's name", () => {
  assert.equal(inboxUrl("https://finite.example/some/path", "pat-sx--k3x9"), "https://finite.example/api/f/pat-sx--k3x9/inbox");
  assert.equal(inboxUrl("http://127.0.0.1:8790", "pat-sx--k3x9"), "http://127.0.0.1:8790/api/f/pat-sx--k3x9/inbox");
  for (const bad of ["../x", "a/b", "x?y", ""]) assert.throws(() => inboxUrl("https://finite.example", bad), String(bad));
});

test("an inbox secret is a header's value; only a stale mark's refusal is asked again", () => {
  assert.equal(secretOk("e2e5ec4e7a9f0b1c"), true);
  for (const bad of ["", "two words", "line\nbreak", "naïve", null, 7]) assert.equal(secretOk(bad), false, JSON.stringify(bad));
  assert.equal(stale(409, { message: "a stale mark: ask again under a new one" }), true);
  assert.equal(stale(409, { message: "not linked" }), false);
  assert.equal(stale(503, { message: "a stale mark: ask again under a new one" }), false);
  assert.equal(stale(409, null), false);
});

test("a file it holds is named by its SHA-256, under its own prefix, and typed as a media type or not at all", () => {
  const sha = "ab".repeat(32);
  assert.equal(fileKey("pat-simplex--k3x9", sha), `doors/pat-simplex--k3x9/files/${sha}`);
  for (const bad of ["AB".repeat(32), "ab".repeat(31), `${"ab".repeat(31)}/x`, "../x", null]) {
    assert.equal(shaOk(bad), false, String(bad));
    assert.throws(() => fileKey("pat-simplex--k3x9", bad));
  }
  assert.equal(typeOf("Audio/MP4"), "audio/mp4");
  for (const bad of ["audio", "audio/mp4; codecs=x", "text/html\r\nx: y", "", null, "a/".repeat(80)]) assert.equal(typeOf(bad), "application/octet-stream", String(bad));
});

test("it holds files while there is room: at most so many, so many bytes between them, each for a day", () => {
  const now = 1_790_000_000_000;
  const f = (bytes, age = 0) => ({ bytes, at: now - age });
  assert.equal(fileRoom([], 1, now), true);
  assert.equal(fileRoom(Array.from({ length: FILES_KEPT - 1 }, () => f(1)), 1, now), true);
  assert.equal(fileRoom(Array.from({ length: FILES_KEPT }, () => f(1)), 1, now), false, "as many as it may");
  assert.equal(fileRoom([f(FILES_BYTES_MAX - 1)], 1, now), true);
  assert.equal(fileRoom([f(FILES_BYTES_MAX - 1)], 2, now), false, "past its bytes");
  assert.equal(fileRoom(Array.from({ length: FILES_KEPT }, () => f(FILES_BYTES_MAX, FILE_KEPT_MS)), FILES_BYTES_MAX, now), true, "those past a day count for nothing");
});

test("a reply's files are a message's at most, each named by its SHA-256", () => {
  const one = { sha256: "cd".repeat(32), name: "a.mp3", type: "Audio/MPEG", size: 9 };
  assert.deepEqual(replyFiles(undefined), []);
  assert.deepEqual(replyFiles([one]), [{ ...one, type: "audio/mpeg" }]);
  assert.equal(replyFiles(Array.from({ length: REPLY_FILES_MAX + 1 }, () => one)), null);
  for (const bad of [{ ...one, sha256: "x" }, { ...one, name: 7 }, { ...one, size: -1 }, { ...one, size: 1.5 }, null]) assert.equal(replyFiles([bad]), null, JSON.stringify(bad));
  assert.equal(replyFiles("nope"), null);
});
