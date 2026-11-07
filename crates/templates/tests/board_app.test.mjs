// The board's app (templates/board/app.mjs) against an in-memory SQLite
// (as a Durable Object's) and a fake of the files at main. Valid: a frame
// at each named size, a custom one, a picture, a placed one; put again,
// it is set whole and its rev goes up, keeping its place in the order;
// removed, it is gone, and removing it again changes nothing. Invalid:
// what the input schema can't say (an id, a file, a kind's fields, half
// a place), and the 49th frame. Its route serves a frame's file from
// main, sandboxed, a folder's index.html, a blob through the platform,
// and no path outside frames/.
import { test } from "node:test";
import assert from "node:assert/strict";
import { register } from "node:module";
import { DatabaseSync } from "node:sqlite";

const BASE = `export class DurableObject {
  constructor(ctx, env) { this.ctx = ctx; this.env = env; }
  get files() { return this.env.FILES; }
}`;
const HOOKS = `export async function resolve(specifier, context, next) {
  if (specifier === "cloudflare:workers") return { url: ${JSON.stringify("data:text/javascript," + encodeURIComponent(BASE))}, shortCircuit: true };
  return next(specifier, context);
}`;
register("data:text/javascript," + encodeURIComponent(HOOKS));
const { App, FRAMES_MAX, SIZES } = await import(new URL("../../../templates/board/app.mjs", import.meta.url).href);

const CALL = { principal: "npub1owner", agent: "npub1agent", role: "editor" };

function storage() {
  const db = new DatabaseSync(":memory:");
  return {
    exec(q, ...args) {
      const st = db.prepare(q);
      const rows = st.columns().length ? st.all(...args) : (st.run(...args), []);
      return { toArray: () => rows, one: () => rows[0] };
    },
  };
}

// main's files: path → bytes, and the paths that are blobs
function board(files = {}, blobs = []) {
  const FILES = {
    async list(prefix) {
      return Object.keys(files).filter((p) => p.startsWith(prefix)).map((path) => (blobs.includes(path) ? { path, size: 2 << 20, blob: true } : { path, size: files[path].length }));
    },
    async readBytes(path) {
      if (blobs.includes(path)) throw new Error(`${path} (2097152 bytes; serve larger files from the site)`);
      return path in files ? new TextEncoder().encode(files[path]) : null;
    },
  };
  return new App({ storage: { sql: storage() } }, { FILES });
}

const put = (app, input) => app.put_frame(input, CALL);

test("a frame at each named size, a custom one and a picture are listed in the order put", () => {
  const app = board();
  for (const size of ["desktop", "tablet", "mobile"]) assert.deepEqual(put(app, { id: `home-${size}`, kind: "html", label: "Home", size }), { id: `home-${size}`, rev: 1, created: true });
  put(app, { id: "wide", kind: "html", label: "Wide", width: 1920 });
  put(app, { id: "shot", kind: "image", label: "Shot", file: "shot.png", width: 1440, x: -10, y: 20 });
  const { frames, max } = app.frames();
  assert.equal(max, FRAMES_MAX);
  assert.deepEqual(frames.map((f) => [f.id, f.size, f.width, f.height, f.file]), [
    ["home-desktop", "desktop", SIZES.desktop.width, SIZES.desktop.height, "index.html"],
    ["home-tablet", "tablet", 768, 1024, "index.html"],
    ["home-mobile", "mobile", 390, 844, "index.html"],
    ["wide", "custom", 1920, 900, "index.html"],
    ["shot", null, 1440, null, "shot.png"],
  ]);
  assert.deepEqual([frames[4].x, frames[4].y, frames[0].x], [-10, 20, null]);
  assert.deepEqual([frames[0].by, frames[0].agent], ["npub1owner", "npub1agent"]);
});

test("a frame put again is set whole, its rev up and its place in the order kept", () => {
  const app = board();
  put(app, { id: "a", kind: "html", label: "A", x: 0, y: 0 });
  put(app, { id: "b", kind: "html", label: "B" });
  assert.deepEqual(put(app, { id: "a", kind: "html", label: "  A,   again ", size: "mobile", height: 2000 }), { id: "a", rev: 2, created: false });
  const [a, b] = app.frames().frames;
  assert.deepEqual([a.id, a.label, a.size, a.width, a.height, a.x, a.y, a.rev], ["a", "A, again", "mobile", 390, 2000, null, null, 2]);
  assert.equal(b.rev, 1, "the other frame is untouched");
});

test("a removed frame is gone, and removing it again changes nothing", () => {
  const app = board();
  put(app, { id: "a", kind: "html", label: "A" });
  assert.deepEqual(app.remove_frame({ id: "a" }, CALL), { removed: 1 });
  assert.deepEqual(app.remove_frame({ id: "a" }, CALL), { removed: 0 });
  assert.deepEqual(app.frames().frames, []);
  assert.deepEqual(put(app, { id: "a", kind: "html", label: "A" }), { id: "a", rev: 1, created: true }, "put again, it starts over");
});

test("what the input schema can't say is refused, with its reason, and nothing is written", () => {
  const app = board();
  const refused = [
    [{ id: "Home", kind: "html", label: "x" }, /a-z, 0-9 and -/],
    [{ id: "-a", kind: "html", label: "x" }, /a-z, 0-9 and -/],
    [{ id: "a/b", kind: "html", label: "x" }, /a-z, 0-9 and -/],
    [{ id: "a", kind: "html", label: "   " }, /label says what it is/],
    [{ id: "a", kind: "html", label: "x", x: 1 }, /both x and y/],
    [{ id: "a", kind: "html", label: "x", file: "page.txt" }, /a .html file/],
    [{ id: "a", kind: "html", label: "x", file: "../secret.html" }, /none starting with a dot/],
    [{ id: "a", kind: "html", label: "x", file: "a//b.html" }, /none starting with a dot/],
    [{ id: "a", kind: "html", label: "x", size: "custom" }, /names its width/],
    [{ id: "a", kind: "html", label: "x", size: "tablet", width: 800 }, /768 wide/],
    [{ id: "a", kind: "image", label: "x" }, /names its file/],
    [{ id: "a", kind: "image", label: "x", file: "shot.html" }, /\.png/],
    [{ id: "a", kind: "image", label: "x", file: "a.png", size: "mobile" }, /no named size/],
    [{ id: "a", kind: "image", label: "x", file: "a.png", height: 300 }, /own shape/],
  ];
  for (const [input, why] of refused) assert.throws(() => put(app, input), why, JSON.stringify(input));
  assert.deepEqual(app.frames().frames, []);
});

test(`a board holds ${FRAMES_MAX} frames; one more is refused, and putting one there again is not`, () => {
  const app = board();
  for (let i = 0; i < FRAMES_MAX; i++) put(app, { id: `f${i}`, kind: "html", label: `F${i}` });
  assert.throws(() => put(app, { id: "one-more", kind: "html", label: "x" }), new RegExp(`holds ${FRAMES_MAX} frames`));
  assert.equal(put(app, { id: "f3", kind: "html", label: "F3 again" }).rev, 2);
  app.remove_frame({ id: "f0" }, CALL);
  assert.equal(put(app, { id: "one-more", kind: "html", label: "x" }).created, true);
  assert.equal(app.frames().frames.length, FRAMES_MAX);
});

test("a frame's file is served from main, sandboxed; a folder is its index.html", async () => {
  const app = board({ "frames/home/index.html": "<h1>Home</h1>", "frames/home/css/a.css": "h1{}", "notes.md": "private" });
  for (const path of ["/frames/home/index.html", "/frames/home/"]) {
    const r = await app.fetch(new Request(`https://b.example${path}?v=3`));
    assert.equal(r.status, 200, path);
    assert.equal(await r.text(), "<h1>Home</h1>");
    assert.equal(r.headers.get("content-type"), "text/html; charset=utf-8");
    assert.equal(r.headers.get("content-security-policy"), "sandbox allow-scripts allow-forms");
    assert.equal(r.headers.get("cache-control"), "no-store");
  }
  const css = await app.fetch(new Request("https://b.example/frames/home/css/a.css"));
  assert.equal(css.headers.get("content-type"), "text/css; charset=utf-8");
  for (const path of ["/notes.md", "/frames/home", "/frames/home/missing.html", "/frames/home/%2E%2E/x", "/frames/Home/index.html", "/frames/home/%ZZ", "/frames/../notes.md"]) {
    const r = await app.fetch(new Request(`https://b.example${path}`));
    assert.equal(r.status, 404, path);
    assert.equal(r.headers.get("content-security-policy"), "sandbox allow-scripts allow-forms", path);
  }
  const post = await app.fetch(new Request("https://b.example/frames/home/index.html", { method: "POST", body: "x" }));
  assert.equal(post.status, 404);
});

test("a frame's file of 1 MiB or more goes on to the platform's, which serves blobs", async () => {
  const app = board({ "frames/home/big.js": "x" }, ["frames/home/big.js"]);
  const r = await app.fetch(new Request("https://b.example/frames/home/big.js"));
  assert.equal(r.status, 302);
  assert.equal(r.headers.get("location"), "/__file?path=frames%2Fhome%2Fbig.js");
});
