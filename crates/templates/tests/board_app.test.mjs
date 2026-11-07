// The board's app (templates/board/app.mjs) against an in-memory SQLite
// (as a Durable Object's) and a fake of the files at main. Valid: a frame
// at each named size, a custom one, a picture, a placed one; put again,
// it is set whole at the board's next rev, keeping its place in the
// order; removed, it is gone, and removing it again changes nothing.
// Invalid: what the input schema can't say (an id, a file, a kind's
// fields, half a place), and the 49th frame. Replay-like: a frame removed
// and put again never has a rev it had. Its route says where a frame's
// one file is at main (the platform's file at that version, a blob's
// too), or plainly why there is none, and answers no other path.
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
const { App, FRAMES_MAX, FILE_MAX_BYTES, SIZES } = await import(new URL("../../../templates/board/app.mjs", import.meta.url).href);

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

// main's files: path → size, each last changed by commit `c-<path>`
function board(files = {}) {
  const FILES = {
    async list(prefix) {
      return Object.keys(files).filter((p) => p.startsWith(prefix)).map((path) => ({ path, size: files[path], ...(files[path] >= 1 << 20 ? { blob: true } : {}) }));
    },
    async stat(path) {
      return path in files ? { path, size: 130, sha: "s", commit: `c-${path}` } : null;
    },
  };
  return new App({ storage: { sql: storage() } }, { FILES });
}

const put = (app, input) => app.put_frame(input, CALL);

test("a frame at each named size, a custom one and a picture are listed in the order put", () => {
  const app = board();
  ["desktop", "tablet", "mobile"].forEach((size, i) => assert.deepEqual(put(app, { id: `home-${size}`, kind: "html", label: "Home", size }), { id: `home-${size}`, rev: i + 1, created: true }));
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

test("a frame put again is set whole, at the board's next rev, its place in the order kept", () => {
  const app = board();
  put(app, { id: "a", kind: "html", label: "A", x: 0, y: 0 });
  put(app, { id: "b", kind: "html", label: "B" });
  assert.deepEqual(put(app, { id: "a", kind: "html", label: "  A,   again ", size: "mobile", height: 2000 }), { id: "a", rev: 3, created: false });
  const [a, b] = app.frames().frames;
  assert.deepEqual([a.id, a.label, a.size, a.width, a.height, a.x, a.y, a.rev], ["a", "A, again", "mobile", 390, 2000, null, null, 3]);
  assert.equal(b.rev, 2, "the other frame is untouched");
});

test("a removed frame is gone, and removing it again changes nothing", () => {
  const app = board();
  put(app, { id: "a", kind: "html", label: "A" });
  assert.deepEqual(app.remove_frame({ id: "a" }, CALL), { removed: 1 });
  assert.deepEqual(app.remove_frame({ id: "a" }, CALL), { removed: 0 });
  assert.deepEqual(app.frames().frames, []);
  assert.deepEqual(put(app, { id: "a", kind: "html", label: "A" }), { id: "a", rev: 2, created: true }, "put again, it is new, at a rev it never had");
});

test("a frame removed and put again never repeats a rev, so a board that missed the removal still loads it again", () => {
  const app = board();
  const seen = [];
  for (const file of ["old.html", "old.html", "new.html"]) {
    seen.push(put(app, { id: "home", kind: "html", label: "Home", file }).rev);
    app.remove_frame({ id: "home" }, CALL);
  }
  put(app, { id: "other", kind: "html", label: "Other" });
  app.remove_frame({ id: "other" }, CALL);
  seen.push(put(app, { id: "home", kind: "html", label: "Home", file: "new.html" }).rev);
  assert.equal(new Set(seen).size, seen.length, `each put its own rev: ${seen}`);
  assert.ok(seen.every((r, i) => i === 0 || r > seen[i - 1]), "and they only go up");
});

test("what the input schema can't say is refused, with its reason, and nothing is written", () => {
  const app = board();
  const refused = [
    [{ id: "Home", kind: "html", label: "x" }, /a-z, 0-9 and -/],
    [{ id: "-a", kind: "html", label: "x" }, /a-z, 0-9 and -/],
    [{ id: "a/b", kind: "html", label: "x" }, /a-z, 0-9 and -/],
    [{ id: "a", kind: "html", label: "   " }, /label says what it is/],
    [{ id: "a", kind: "html", label: "x", x: 1 }, /both x and y/],
    [{ id: "a", kind: "html", label: "x", file: "page.txt" }, /one .html file, everything it needs inline/],
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
  assert.equal(put(app, { id: "f3", kind: "html", label: "F3 again" }).rev, FRAMES_MAX + 1);
  app.remove_frame({ id: "f0" }, CALL);
  assert.equal(put(app, { id: "one-more", kind: "html", label: "x" }).created, true);
  assert.equal(app.frames().frames.length, FRAMES_MAX);
});

test("a frame's route says where its file is at main: the platform's file at that version, never this origin's page", async () => {
  const app = board({ "frames/home/index.html": 2 << 20, "frames/shot/a.png": 900 });
  put(app, { id: "home", kind: "html", label: "Home" });
  put(app, { id: "shot", kind: "image", label: "Shot", file: "a.png" });
  for (const [id, path] of [["home", "frames/home/index.html"], ["shot", "frames/shot/a.png"]]) {
    const r = await app.fetch(new Request(`https://b.example/frames/${id}?r=3`));
    assert.equal(r.status, 302, id);
    assert.equal(r.headers.get("location"), `/__file?path=${encodeURIComponent(path)}&v=c-${path}`, "a page past 1 MiB (a blob) goes the same way");
    assert.equal(r.headers.get("cache-control"), "no-store");
  }
});

test("a frame's route says plainly why it has nothing: no frame, no file yet, too large, no such route", async () => {
  const app = board({ "frames/big/index.html": FILE_MAX_BYTES + 1, "frames/ok/index.html": FILE_MAX_BYTES });
  for (const id of ["early", "big", "ok"]) put(app, { id, kind: "html", label: id });
  const asked = async (path, init) => {
    const r = await app.fetch(new Request(`https://b.example${path}`, init));
    return [r.status, r.headers.get("content-type"), (await r.text()).trim()];
  };
  assert.deepEqual(await asked("/frames/none"), [404, "text/plain; charset=utf-8", "No frame none on this board."]);
  assert.deepEqual(await asked("/frames/early"), [404, "text/plain; charset=utf-8", "No frames/early/index.html yet."]);
  assert.deepEqual(await asked("/frames/big"), [413, "text/plain; charset=utf-8", "frames/big/index.html is over 16 MiB, a frame's most."]);
  assert.equal((await asked("/frames/ok"))[0], 302, "at the most, it is shown");
  for (const path of ["/", "/frames/ok/index.html", "/frames/Home", "/frames/ok/", "/notes.md"]) assert.equal((await asked(path))[0], 404, path);
  assert.equal((await asked("/frames/ok", { method: "POST", body: "x" }))[0], 404);
});
