// A SimpleX door's rules, pure (docs/computers.md, "F: messengers outside
// the computer"): door.mjs is their I/O, and door-rules.test.mjs tries them
// under node's own runner (cargo xtask check runs it).

// A door's saves kept, the newest readable ones (Paul, 2026-11-20): what a
// start falls back on when its newest will not restore.
export const SAVES_KEPT = 3;

// Its owner's messages the door keeps: at most this many its connector's
// inbox has yet to take (past them its runner is refused, and delivers
// again later from its own cursor: none is let go untaken), and the newest
// this many it took (each taken once, by its id).
export const DELIVERIES_KEPT = 256;

// An ask of its runner overtaken by a later mark (a delivery's, numbered
// as it went) is refused, and asked again under a new one this many times
// (images/simplex-door/src/main.rs, "Marks").
export const STALE_ASKS = 3;

// The header its deliveries carry their signature in, by its connector's
// inbox secret: the one templates/simplex/fragment.json's `inbox` names.
export const INBOX_SIGNATURE_HEADER = "x-simplex-door-signature";

// What its connector's jobs ask (cell/src/jobs.rs sends each as
// `/job/<ask>`): nothing of its runner's own or its levers'. `file` is a
// file's, by its SHA-256 (`/job/file/<sha256>`).
export const JOB_ASKS = ["connect", "state", "link", "unlink", "reply", "file", "kept"];

// The files a door holds between its connector and its runner (its
// owner's, on their way to the connector's chat; its agent's, on their way
// to its owner's SimpleX app), each for a moment: at most this many, this
// many bytes between them, every one it holds counted, and each this long
// since it was last given or used (but one a waiting delivery names).
// Past the first two, those nothing still uses make room, the oldest
// first; with none, a new one is refused until older ones go (asked again
// later). SimpleX's own bound on one file is its runner's
// (images/simplex-door).
export const FILES_KEPT = 64;
export const FILES_BYTES_MAX = 1024 * 1024 * 1024;
export const FILE_KEPT_MS = 24 * 60 * 60 * 1000;
// A reply's files, at most: a chat record's (fragment_proto
// `limits::FILES_PER_MESSAGE`, held to it by xtask/src/guards.rs).
export const REPLY_FILES_MAX = 8;
// The header a body names its SHA-256 in, as its producer hashed it: a
// file the door answers, and every body between a door and its runner,
// either way (fragment_core::door::FILE_SHA_HEADER).
export const FILE_SHA_HEADER = "x-door-sha256";

// The door's schema, step by step, from 1: a step released is never edited
// or dropped, only one added after it (as the Rust objects' are,
// crates/core/migrations/schema.lock). door-rules.test.mjs holds each
// released step to its text.
export const STEPS = [
  "CREATE TABLE door (id INTEGER PRIMARY KEY CHECK (id = 1), door TEXT NOT NULL, agent TEXT NOT NULL, started INTEGER NOT NULL, generation INTEGER NOT NULL, marks INTEGER NOT NULL, saved INTEGER NOT NULL, stopped TEXT)",
  "CREATE TABLE saves (generation INTEGER NOT NULL, seq INTEGER NOT NULL, mark INTEGER NOT NULL, bytes INTEGER NOT NULL, at INTEGER NOT NULL, unreadable INTEGER NOT NULL, PRIMARY KEY (generation, seq))",
  "CREATE TABLE deliveries (id TEXT PRIMARY KEY, mark INTEGER NOT NULL, body TEXT NOT NULL, at INTEGER NOT NULL)",
  // its connector (its name, its inbox's), the inbox's secret as its
  // `connect` gave it, and the newest link or unlink it numbered
  "CREATE TABLE inbox (id INTEGER PRIMARY KEY CHECK (id = 1), fragment TEXT NOT NULL, secret TEXT NOT NULL, asked INTEGER NOT NULL DEFAULT 0)",
  // whether its connector's inbox took it
  "ALTER TABLE deliveries ADD COLUMN sent INTEGER NOT NULL DEFAULT 0",
  // the files it holds (`FILES_KEPT`): each by its SHA-256, its bytes, its
  // type as its giver said, and when it was last given or used
  "CREATE TABLE files (sha TEXT PRIMARY KEY, bytes INTEGER NOT NULL, type TEXT NOT NULL, at INTEGER NOT NULL)",
  // who still uses each: a delivery (`d:<its id>`) until its connector
  // says it kept the file, a reply (`r:<its key>`) until it is done, and
  // its runner's give (`given`) until the delivery naming it is made; each
  // once, so an answer said again releases nothing more
  "CREATE TABLE uses (sha TEXT NOT NULL, owner TEXT NOT NULL, PRIMARY KEY (sha, owner))",
  // the save it was given to begin from (`/door/import`), by its SHA-256:
  // at most one, ever
  "CREATE TABLE imported (sha TEXT NOT NULL, at INTEGER NOT NULL, staged INTEGER NOT NULL DEFAULT 0)",
  // each save's SHA-256, as its maker hashed it (its runner, or the
  // computer that handed it over), which every copy of it given carries
  // (none for a save from before it, hashed when it is first given)
  "ALTER TABLE saves ADD COLUMN sha TEXT",
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

// Whether `door` is the door of the connector named `connector`: its name
// and its life's digits (fragment_core::door::door_id).
export function doorOf(door, connector) {
  return nameOk(door) && nameOk(connector) && door.startsWith(`${connector}--`) && /^[0-9a-f]{12}$/.test(door.slice(connector.length + 2));
}

// Its connector's inbox: on the platform's own origin, at the connector's
// name (as the platform named it, never a job).
export function inboxUrl(platform, door) {
  if (!nameOk(door)) throw new Error(`no door is named ${JSON.stringify(door)}`);
  return `${new URL(platform).origin}/api/f/${door}/inbox`;
}

// An inbox secret as a header carries it: visible ASCII, none empty.
export function secretOk(secret) {
  return typeof secret === "string" && /^[\x21-\x7e]+$/.test(secret);
}

// Whether a runner's refusal is a stale mark's: asked again under a new one.
export function stale(status, body) {
  return status === 409 && typeof body?.message === "string" && body.message.startsWith("a stale mark");
}

// Every file a door keeps in the bucket is under this prefix of its own,
// which its end deletes whole.
export function doorPrefix(door) {
  if (!nameOk(door)) throw new Error(`no door is named ${JSON.stringify(door)}`);
  return `doors/${door}/`;
}

// A file it holds, in the bucket under the door's own prefix.
export function fileKey(door, sha) {
  if (!shaOk(sha)) throw new Error(`no file is named ${JSON.stringify(sha)}`);
  return `${doorPrefix(door)}files/${sha}`;
}

// A file's name: its SHA-256, in lowercase hex.
export function shaOk(sha) {
  return typeof sha === "string" && /^[0-9a-f]{64}$/.test(sha);
}

// A file's type as its giver said it: a media type's shape, else none.
export function typeOf(type) {
  return typeof type === "string" && type.length <= 128 && /^[a-z0-9][a-z0-9!#$&^_.+-]*\/[a-z0-9][a-z0-9!#$&^_.+-]*$/.test(type.toLowerCase()) ? type.toLowerCase() : "application/octet-stream";
}

// Whether a file of `bytes` may be held beside `files` (each {bytes}):
// every one it holds counts, an old one a waiting delivery keeps included
// (those past `FILE_KEPT_MS` that none needs are let go before it asks).
export function fileRoom(files, bytes) {
  const held = files.reduce((n, f) => n + f.bytes, 0);
  return files.length < FILES_KEPT && held + bytes <= FILES_BYTES_MAX;
}

// A reply's files as its connector names them: each {sha256, name, type,
// size}, at most `REPLY_FILES_MAX`, else null.
export function replyFiles(files) {
  if (files === undefined) return [];
  if (!Array.isArray(files) || files.length > REPLY_FILES_MAX) return null;
  const out = [];
  for (const f of files) {
    if (!shaOk(f?.sha256) || typeof f.name !== "string" || f.name.length > 255 || !Number.isInteger(f.size) || f.size < 0) return null;
    out.push({ sha256: f.sha256, name: f.name, type: typeOf(f.type), size: f.size });
  }
  return out;
}

// A save's file in the bucket, under the door's own prefix.
export function saveKey(door, generation, seq) {
  return `${doorPrefix(door)}saves/${generation}-${seq}`;
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

// What a start restores: none (a new door: it never saved), its newest
// readable save, or nothing left (it saved, and none restores: it never
// starts as a new door over them, a new profile, its link gone). `saved`
// is how many saves it ever took, so the answer outlives pruning.
export function restoreOf(saves, saved) {
  const s = restorable(saves);
  if (s) return { kind: "save", save: s };
  return saved === 0 && saves.length === 0 ? { kind: "none" } : { kind: "exhausted" };
}

// Whether a door may start: unless it saved and nothing restores.
export function startable(saves, saved) {
  return restoreOf(saves, saved).kind !== "exhausted";
}

// A save's id, as a start is told it and names it back ("<generation>-<seq>").
export const saveId = (s) => `${s.generation}-${s.seq}`;

export function parseSaveId(id) {
  const m = typeof id === "string" ? /^([0-9]{1,15})-([0-9]{1,15})$/.exec(id) : null;
  return m ? { generation: Number(m[1]), seq: Number(m[2]) } : null;
}

const hexOf = (digest) => [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");

// Bytes' SHA-256, in lowercase hex.
export async function sha256Hex(bytes) {
  return hexOf(await crypto.subtle.digest("SHA-256", bytes));
}

// A delivery's signature for its connector's inbox (`INBOX_SIGNATURE_HEADER`,
// its inbox's `hmac`: templates/simplex fragment.json): HMAC-SHA256 of its
// body by the inbox's secret, so a body changed on its way is refused.
export async function inboxSignature(secret, body) {
  const key = await crypto.subtle.importKey("raw", new TextEncoder().encode(secret), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  return `sha256=${hexOf(await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(body)))}`;
}

// A transfer as the e2e's levers damage one on its way (`damage`, as
// FIN-775's were between a container and its Worker): one byte flipped,
// its length kept.
export function flipped(body) {
  let done = false;
  return body.pipeThrough(
    new TransformStream({
      transform(chunk, c) {
        if (!done && chunk.byteLength > 64) {
          chunk = chunk.slice();
          chunk[64] ^= 1;
          done = true;
        }
        c.enqueue(chunk);
      },
    }),
  );
}

// A stream passed through as it comes, its SHA-256 taken on the way (hex,
// `digest` once it ends), one chunk at a time with its backpressure, never
// the whole held at once (a save may be 64 MiB).
export function hashing() {
  const digest = new crypto.DigestStream("SHA-256");
  const w = digest.getWriter();
  const stream = new TransformStream({
    async transform(chunk, c) {
      await w.write(chunk.slice());
      c.enqueue(chunk);
    },
    async flush() {
      await w.close();
    },
  });
  return { stream, digest: digest.digest.then(hexOf) };
}
