// A SimpleX door's rules, pure (docs/computers.md, "F: messengers outside
// the computer"): door.mjs is their I/O, and door-rules.test.mjs tries them
// under node's own runner (cargo xtask check runs it).

// A door's saves kept, the newest readable ones (Paul, 2026-11-20): what a
// start falls back on when its newest will not restore.
export const SAVES_KEPT = 3;

// Its owner's messages the door keeps, the newest, until its connector
// takes them on (the next step: D2).
export const DELIVERIES_KEPT = 256;

// The door's schema, step by step, from 1: a step released is never edited
// or dropped, only one added after it (as the Rust objects' are,
// crates/core/migrations/schema.lock). door-rules.test.mjs holds each
// released step to its text.
export const STEPS = [
  "CREATE TABLE door (id INTEGER PRIMARY KEY CHECK (id = 1), door TEXT NOT NULL, agent TEXT NOT NULL, generation INTEGER NOT NULL, marks INTEGER NOT NULL, stopped TEXT)",
  "CREATE TABLE saves (generation INTEGER NOT NULL, seq INTEGER NOT NULL, mark INTEGER NOT NULL, bytes INTEGER NOT NULL, at INTEGER NOT NULL, unreadable INTEGER NOT NULL, PRIMARY KEY (generation, seq))",
  "CREATE TABLE deliveries (id TEXT PRIMARY KEY, mark INTEGER NOT NULL, body TEXT NOT NULL, at INTEGER NOT NULL)",
];

// The steps a schema at `version` has yet to take, in order.
export function steps(version) {
  if (!(Number.isInteger(version) && version >= 0 && version <= STEPS.length)) throw new Error(`a door's schema at ${version}: not one of its ${STEPS.length} steps`);
  return STEPS.slice(version).map((sql, i) => ({ version: version + i + 1, sql }));
}

// A door's name: its fragment's (a label, `--`, a suffix), as a key in the
// bucket and in its object's name.
export function nameOk(name) {
  return typeof name === "string" && name.length > 0 && name.length <= 96 && /^[a-z0-9-]+$/.test(name);
}

// A save's file in the bucket, under the door's own prefix.
export function saveKey(door, generation, seq) {
  if (!nameOk(door)) throw new Error(`no door is named ${JSON.stringify(door)}`);
  return `doors/${door}/saves/${generation}-${seq}`;
}

const newestFirst = (a, b) => b.generation - a.generation || b.seq - a.seq;

// The save a start restores: the newest one not found unreadable.
export function restorable(saves) {
  return [...saves].sort(newestFirst).find((s) => !s.unreadable) ?? null;
}

// What a start is told of the save it restores: behind when the door has
// numbered an effect (a send it asked for, a message it took) past what
// the save holds (images/simplex-door/src/door.rs, "an effect").
export function behind(save, marks) {
  return save.mark < marks;
}

// Whether a save of start `asked` is refused: any start but the one running
// is fenced off (its runner ends at a refusal).
export function fenced(asked, running) {
  return asked !== running;
}

// The saves past the newest `SAVES_KEPT` readable ones: let go, their files
// with them (an unreadable one is never restored again).
export function surplus(saves) {
  const keep = new Set([...saves].sort(newestFirst).filter((s) => !s.unreadable).slice(0, SAVES_KEPT).map((s) => `${s.generation}-${s.seq}`));
  return saves.filter((s) => !keep.has(`${s.generation}-${s.seq}`));
}

// Whether a door may start: one with saves starts only from one of them,
// never as a new door over them (a new profile, its link gone).
export function startable(saves) {
  return saves.length === 0 || restorable(saves) !== null;
}
