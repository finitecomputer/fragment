// A board: an agent's designs side by side on one canvas. A frame is one
// file at main, frames/<id>/<file>: an HTML page with everything it needs
// inline, or a picture. The app holds the board's manifest, one row per
// frame (what it is, its label, its size, where it sits, and its `rev`),
// so an agent changes a frame with no deploy: it writes the file
// (`fragment write`, `fragment sync`), then calls `put_frame`, whose
// commit re-runs every open board's `frames` query; each board loads
// again the frames whose `rev` moved, and only those (site/board.js).
//
// A mutation cannot read files (it is synchronous), so `put_frame` is the
// signal that a frame's file changed: a files trigger would say so by
// itself, but triggered runs pause themselves past
// TRIGGERED_RUNS_PER_HOUR, which an agent iterating on a design reaches,
// and a paused trigger stops every board without a word.
//
//   frames       (query)    the manifest
//   put_frame    (mutation) add a frame, or set one again whole; redraws it
//   remove_frame (mutation) take a frame off; its file stays
//
// fragment.json's input schemas hold each field's own bounds (the cell
// enforces them on every call, and they are what an MCP client reads);
// what a schema cannot say is checked here.
import { DurableObject } from "cloudflare:workers";

// Each HTML frame is a live page in every viewer's browser, and a phone
// holds a few dozen before it struggles.
export const FRAMES_MAX = 48;
// A frame's file, which every viewer's browser downloads whole each time
// it changes: a page with its pictures inline fits with room. The route
// refuses a larger one by its size at main, and the board, told this by
// `frames`, stops reading any body past it.
export const FILE_MAX_BYTES = 16 * 1024 * 1024;
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
    // the board's last rev: every put takes the next, so a rev is never
    // another put's, a frame's put again after its removal included (a
    // board that missed the removal still sees a new rev)
    ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS counters (name TEXT PRIMARY KEY, value INTEGER NOT NULL)");
  }

  frames() {
    const frames = this.ctx.storage.sql
      .exec("SELECT id, kind, label, file, size, width, height, x, y, rev, at, by, agent FROM frames ORDER BY seq LIMIT ?", FRAMES_MAX)
      .toArray();
    return { frames, max: FRAMES_MAX, file_max_bytes: FILE_MAX_BYTES };
  }

  put_frame(input, call) {
    checkCaller(call);
    const frame = shaped(input);
    const sql = this.ctx.storage.sql;
    const was = sql.exec("SELECT seq FROM frames WHERE id = ?", frame.id).toArray()[0];
    if (!was) {
      const { n } = sql.exec("SELECT COUNT(*) AS n FROM frames").one();
      if (n >= FRAMES_MAX) throw new Error(`A board holds ${FRAMES_MAX} frames: remove one first.`);
    }
    const seq = was ? was.seq : sql.exec("SELECT COALESCE(MAX(seq), 0) + 1 AS seq FROM frames").one().seq;
    const { rev } = sql.exec("INSERT INTO counters (name, value) VALUES ('rev', 1) ON CONFLICT (name) DO UPDATE SET value = value + 1 RETURNING value AS rev").one();
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

  // `/frames/<id>`: where frame <id>'s file is at main, as the platform
  // serves a file at its version (`__file?path=&v=<commit>`, which serves
  // a blob's bytes too). Its version is main's, so a frame shows what the
  // agent wrote last, whatever a deploy put live. The board reads a page's
  // bytes from there and shows them in its sandboxed iframe (board.js);
  // nothing serves a frame as a page of this origin.
  async fetch(request) {
    if (request.method !== "GET" && request.method !== "HEAD") return said(404, "No such route.");
    const id = /^\/frames\/([a-z0-9][a-z0-9-]{0,47})$/.exec(new URL(request.url).pathname)?.[1];
    if (!id) return said(404, "No such route.");
    const row = this.ctx.storage.sql.exec("SELECT file FROM frames WHERE id = ?", id).toArray()[0];
    if (!row) return said(404, `No frame ${id} on this board.`);
    const path = `frames/${id}/${row.file}`;
    const [listed] = (await this.files.list(path)).filter((f) => f.path === path);
    if (!listed) return said(404, `No ${path} yet.`);
    if (listed.size > FILE_MAX_BYTES) return said(413, `${path} is over ${FILE_MAX_BYTES / 1024 / 1024} MiB, a frame's most.`);
    const stat = await this.files.stat(path);
    if (!stat) return said(404, `No ${path} yet.`);
    return new Response(null, { status: 302, headers: { location: `/__file?path=${encodeURIComponent(path)}&v=${stat.commit}`, "cache-control": "no-store" } });
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
    checkFile(file, PAGE, "An HTML frame is one .html file, everything it needs inline.");
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

// A refusal the board shows beside the frame, as plain text that no
// browser runs as a page.
function said(status, text) {
  return new Response(`${text}\n`, { status, headers: { "content-type": "text/plain; charset=utf-8", "cache-control": "no-store" } });
}

const encoder = new TextEncoder();

// The platform names every caller; what is stored of them fits the row
// FRAME_MAX_BYTES counts on.
function checkCaller(call) {
  if (typeof call.principal !== "string" || call.principal.length === 0 || call.principal.length > PRINCIPAL_MAX_CHARS) {
    throw new Error("board: a caller is named by a principal");
  }
  if (call.agent != null && (typeof call.agent !== "string" || call.agent.length > PRINCIPAL_MAX_CHARS)) throw new Error("board: an agent is named by its npub");
}
