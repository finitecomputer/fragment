// A board: an agent's designs side by side on one canvas. The app holds
// the board's manifest, one row per frame (what it is, its label, its
// size, where it sits, and its `rev`), and serves the frames' files from
// `main`, so an agent changes a frame with no deploy: it writes the
// frame's files (`fragment write`, `fragment sync`), then calls
// `put_frame`, whose commit re-runs every open board's `frames` query;
// each board redraws the frames whose `rev` moved, and only those
// (site/board.js). A mutation cannot read files (it is synchronous), so
// `put_frame` is the signal that a frame's files changed: a files
// trigger would say so by itself, but triggered runs pause themselves
// past TRIGGERED_RUNS_PER_HOUR, which an agent iterating on a design
// reaches, and a paused trigger stops every board without a word.
//
//   frames       (query)    the manifest
//   put_frame    (mutation) add a frame, or set one again whole; redraws it
//   remove_frame (mutation) take a frame off; its files stay
//
// fragment.json's input schemas hold each field's own bounds (the cell
// enforces them on every call, and they are what an MCP client reads);
// what a schema cannot say is checked here.
import { DurableObject } from "cloudflare:workers";

// Each HTML frame is a live page in every viewer's browser, and a phone
// holds a few dozen before it struggles.
export const FRAMES_MAX = 48;
// One frame's row as `frames` answers it, at most: a 80-character label
// escaped at worst, a 200-character file, the id, two principals and the
// numbers fit in it with room. With every frame at its largest the
// manifest still fits one answer (asserted below).
export const FRAME_MAX_BYTES = 2048;
// The named sizes an HTML frame is drawn at: blobby's artboards
// (github.com/Ravindrabisht12/blobby, MIT) and a common viewport height.
export const SIZES = {
  desktop: { width: 1440, height: 900 },
  tablet: { width: 768, height: 1024 },
  mobile: { width: 390, height: 844 },
};
// A custom size's height when none is given.
const CUSTOM_HEIGHT = 900;
// An id is its folder's name and part of a URL; 48 characters leave a
// note's anchor (PR 3: `<id>_<x>_<y>`) inside the notes' 64.
const ID = /^[a-z0-9][a-z0-9-]{0,47}$/;
// A file in its frame's folder: names of letters, digits, `.`, `_`, `-`,
// none starting with a dot (so no `..`, and no hidden file).
const SEGMENT = /^[A-Za-z0-9_-][A-Za-z0-9._-]*$/;
const PAGE = /\.html?$/i;
const PICTURE = /\.(png|jpe?g|webp|gif|avif|svg)$/i;
const PRINCIPAL_MAX_CHARS = 64;
const RESULT_MAX_BYTES = 1024 * 1024;
if (FRAMES_MAX * FRAME_MAX_BYTES + 64 > RESULT_MAX_BYTES) {
  throw new Error("app.mjs: the frames' limits outgrow their answer");
}

// What a frame's file is served as (`fetch`); anything else is bytes.
const TYPES = {
  html: "text/html; charset=utf-8",
  htm: "text/html; charset=utf-8",
  css: "text/css; charset=utf-8",
  js: "text/javascript; charset=utf-8",
  mjs: "text/javascript; charset=utf-8",
  json: "application/json",
  map: "application/json",
  txt: "text/plain; charset=utf-8",
  svg: "image/svg+xml",
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  webp: "image/webp",
  gif: "image/gif",
  avif: "image/avif",
  ico: "image/x-icon",
  woff2: "font/woff2",
  woff: "font/woff",
  ttf: "font/ttf",
  wasm: "application/wasm",
};
// Every answer of a frame's file runs sandboxed, as the board's iframe
// does (site/board.js `SANDBOX`), so a frame opened on its own is no
// more than it is on the board: an opaque origin, its scripts on.
const FRAME_POLICY = "sandbox allow-scripts allow-forms";
const encoder = new TextEncoder();

export class App extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    // `seq`: the order a frame was first put, which the board lays out by
    ctx.storage.sql.exec(`CREATE TABLE IF NOT EXISTS frames (
      id TEXT PRIMARY KEY,
      seq INTEGER NOT NULL,
      kind TEXT NOT NULL,
      label TEXT NOT NULL,
      file TEXT NOT NULL,
      size TEXT,
      width INTEGER,
      height INTEGER,
      x INTEGER,
      y INTEGER,
      rev INTEGER NOT NULL,
      at INTEGER NOT NULL,
      by TEXT NOT NULL,
      agent TEXT
    )`);
  }

  frames() {
    const frames = this.ctx.storage.sql
      .exec("SELECT id, kind, label, file, size, width, height, x, y, rev, at, by, agent FROM frames ORDER BY seq LIMIT ?", FRAMES_MAX)
      .toArray();
    return { frames, max: FRAMES_MAX };
  }

  put_frame(input, call) {
    checkCaller(call);
    const frame = shaped(input);
    const sql = this.ctx.storage.sql;
    const was = sql.exec("SELECT seq, rev FROM frames WHERE id = ?", frame.id).toArray()[0];
    if (!was) {
      const { n } = sql.exec("SELECT COUNT(*) AS n FROM frames").one();
      if (n >= FRAMES_MAX) throw new Error(`A board holds ${FRAMES_MAX} frames: remove one first.`);
    }
    const seq = was ? was.seq : sql.exec("SELECT COALESCE(MAX(seq), 0) + 1 AS seq FROM frames").one().seq;
    const rev = (was?.rev ?? 0) + 1;
    const row = { ...frame, rev, at: Date.now(), by: call.principal, agent: call.agent ?? null };
    if (encoder.encode(JSON.stringify(row)).length > FRAME_MAX_BYTES) throw new Error(`A frame is at most ${FRAME_MAX_BYTES} bytes as the board lists it: shorten its label or file.`);
    sql.exec(
      `INSERT INTO frames (id, seq, kind, label, file, size, width, height, x, y, rev, at, by, agent) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
       ON CONFLICT (id) DO UPDATE SET kind = excluded.kind, label = excluded.label, file = excluded.file, size = excluded.size,
         width = excluded.width, height = excluded.height, x = excluded.x, y = excluded.y, rev = excluded.rev, at = excluded.at,
         by = excluded.by, agent = excluded.agent`,
      row.id, seq, row.kind, row.label, row.file, row.size, row.width, row.height, row.x, row.y, rev, row.at, row.by, row.agent,
    );
    return { id: frame.id, rev, created: !was };
  }

  remove_frame({ id }, call) {
    checkCaller(call);
    const removed = this.ctx.storage.sql.exec("DELETE FROM frames WHERE id = ? RETURNING id", id).toArray().length;
    return { removed };
  }

  // `/frames/<id>/<file>`: a frame's file at main, for its iframe (and
  // what the page it is asks for beside it). Another path is no route.
  async fetch(request) {
    if (request.method !== "GET" && request.method !== "HEAD") return missing();
    const path = framePath(new URL(request.url).pathname);
    if (path === null) return missing();
    const [found] = (await this.files.list(path)).filter((f) => f.path === path);
    if (!found) return missing();
    // a file of 1 MiB or more is a blob, whose bytes the platform serves
    if (found.blob) return new Response(null, { status: 302, headers: { location: `/__file?path=${encodeURIComponent(path)}`, "cache-control": "no-store" } });
    const bytes = await this.files.readBytes(path);
    if (bytes === null) return missing();
    const ext = path.slice(path.lastIndexOf(".") + 1).toLowerCase();
    const headers = {
      "content-type": TYPES[ext] ?? "application/octet-stream",
      "content-security-policy": FRAME_POLICY,
      // the board names a frame's version in its iframe's address; what
      // the page loads beside it carries none, so nothing is kept
      "cache-control": "no-store",
    };
    return new Response(request.method === "HEAD" ? null : bytes, { headers });
  }
}

// A put_frame's input as its row holds it, or a refusal saying why.
function shaped({ id, kind, label, file, size, width, height, x, y }) {
  if (!ID.test(id)) throw new Error("A frame's id is a-z, 0-9 and -, starting with a letter or digit, at most 48: it names its folder, frames/<id>/.");
  label = label.replace(/\s+/g, " ").trim();
  if (label.length === 0) throw new Error("A frame's label says what it is.");
  if ((x === undefined) !== (y === undefined)) throw new Error("A frame's place is both x and y, or neither.");
  const place = { x: x ?? null, y: y ?? null };
  if (kind === "html") {
    file ??= "index.html";
    checkFile(file, PAGE, "An HTML frame's file is its page: a .html file.");
    size ??= width === undefined ? "desktop" : "custom";
    if (size === "custom") {
      if (width === undefined) throw new Error("A custom size names its width.");
      return { id, kind, label, file, size, width, height: height ?? CUSTOM_HEIGHT, ...place };
    }
    if (width !== undefined) throw new Error(`A ${size} frame is ${SIZES[size].width} wide: for another width, use size custom.`);
    return { id, kind, label, file, size, width: SIZES[size].width, height: height ?? SIZES[size].height, ...place };
  }
  if (file === undefined) throw new Error("An image frame names its file: frames/<id>/<file>.");
  checkFile(file, PICTURE, "An image frame's file is a .png, .jpg, .webp, .gif, .avif or .svg.");
  if (size !== undefined) throw new Error("An image frame has no named size: give a width to scale it.");
  if (height !== undefined) throw new Error("An image frame keeps its own shape: give a width alone.");
  return { id, kind, label, file, size: null, width: width ?? null, height: null, ...place };
}

function checkFile(file, kind, refusal) {
  if (!file.split("/").every((s) => SEGMENT.test(s))) throw new Error("A frame's file is a path in its folder: names of A-Z, a-z, 0-9, ., _ and -, none starting with a dot.");
  if (!kind.test(file)) throw new Error(refusal);
}

// The repo path a `/frames/<id>/<file>` URL names (a folder's,
// `index.html` in it), or null.
function framePath(pathname) {
  const parts = pathname.split("/").slice(1);
  if (parts.length < 3 || parts[0] !== "frames") return null;
  if (parts.at(-1) === "") parts[parts.length - 1] = "index.html";
  let named;
  try {
    named = parts.map(decodeURIComponent);
  } catch {
    return null;
  }
  if (!ID.test(named[1]) || !named.slice(2).every((s) => SEGMENT.test(s))) return null;
  return named.join("/");
}

function missing() {
  return new Response("No such file in this frame.\n", { status: 404, headers: { "content-type": "text/plain; charset=utf-8", "content-security-policy": FRAME_POLICY } });
}

// The platform names every caller; what is stored of them fits the row
// FRAME_MAX_BYTES counts on.
function checkCaller(call) {
  if (typeof call.principal !== "string" || call.principal.length === 0 || call.principal.length > PRINCIPAL_MAX_CHARS) {
    throw new Error("board: a caller is named by a principal");
  }
  if (call.agent != null && (typeof call.agent !== "string" || call.agent.length > PRINCIPAL_MAX_CHARS)) throw new Error("board: an agent is named by its npub");
}
