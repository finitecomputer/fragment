// door-rules.mjs: a SimpleX door's rules. Valid (the newest readable save
// restored, told behind only past an effect it lacks, the newest three
// kept), invalid (an unreadable save never restored again, a stale start's
// save fenced, a door with saves never started as a new one over them),
// and its schema's steps held to their released text.
import { test } from "node:test";
import assert from "node:assert/strict";
import { STEPS, SAVES_KEPT, behind, fenced, nameOk, restorable, saveKey, startable, steps, surplus } from "./door-rules.mjs";

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

test("a door with saves starts only from one of them", () => {
  assert.equal(startable([]), true, "a new door");
  assert.equal(startable([save(1, 1, 0, 1), save(1, 2, 0)]), true);
  assert.equal(startable([save(1, 1, 0, 1), save(1, 2, 0, 1)]), false, "every save unreadable: never a new profile over them");
});

test("a door's name is a fragment's, and its saves are under its own prefix", () => {
  assert.equal(saveKey("pat-simplex--k3x9", 2, 5), "doors/pat-simplex--k3x9/saves/2-5");
  for (const bad of ["", "../x", "A", "a/b", "x".repeat(97), 7]) {
    assert.equal(nameOk(bad), false, String(bad));
  }
  assert.throws(() => saveKey("../x", 1, 1));
});

// Each released step's text, as it was released: a step is never edited or
// dropped (stored rows stay readable across releases), only added after.
const RELEASED = [
  "CREATE TABLE door (id INTEGER PRIMARY KEY CHECK (id = 1), door TEXT NOT NULL, agent TEXT NOT NULL, generation INTEGER NOT NULL, marks INTEGER NOT NULL, stopped TEXT)",
  "CREATE TABLE saves (generation INTEGER NOT NULL, seq INTEGER NOT NULL, mark INTEGER NOT NULL, bytes INTEGER NOT NULL, at INTEGER NOT NULL, unreadable INTEGER NOT NULL, PRIMARY KEY (generation, seq))",
  "CREATE TABLE deliveries (id TEXT PRIMARY KEY, mark INTEGER NOT NULL, body TEXT NOT NULL, at INTEGER NOT NULL)",
];

test("the schema's released steps are as released, and taken in order", () => {
  assert.ok(STEPS.length >= RELEASED.length, "no released step dropped");
  assert.deepEqual(STEPS.slice(0, RELEASED.length), RELEASED, "no released step edited");
  assert.deepEqual(steps(0).map((s) => s.version), STEPS.map((_, i) => i + 1));
  assert.deepEqual(steps(STEPS.length), []);
  assert.throws(() => steps(STEPS.length + 1));
  assert.throws(() => steps(-1));
});
