// The board's page: every frame of the app's manifest (`frames`) on one
// canvas, moved with a drag, a wheel or a trackpad, zoomed with a pinch
// or ctrl/⌘ and the wheel. The manifest is a live query: when an agent's
// put_frame commits it answers again, and each frame whose `rev` moved
// loads again in place, the others untouched. The camera is this page's
// own and nothing the manifest says moves it.
//
// A frame is one file. Its bytes are read from where the app says the
// file is at main (`/frames/<id>`, app.mjs), behind the frame shown,
// which stays until they have loaded; a load that fails leaves it there,
// says why, and is tried again. A page is shown with `srcdoc` in an
// iframe sandboxed without allow-same-origin: an opaque origin, so its
// scripts reach neither this page, nor the viewer's session, nor this
// origin's cookies or storage. It may run scripts and send its own forms,
// and nothing else (no popups, modals, downloads or top navigation).
// The platform serves the file itself as inert as ever; this page reads
// its bytes, of any size a frame may be.
import * as fragment from "./__fragment.js";

const SANDBOX = "allow-scripts allow-forms";
// The layout the board makes for frames the agent placed nowhere, in
// board pixels: left to right in the order they were first put, a new
// row past ROW_MAX, below every placed frame.
const GAP = 160;
const ROW_MAX = 6000;
// A picture's size before it has loaded.
const PICTURE_GUESS = { width: 800, height: 600 };
// How far a person may zoom out: this, or as far as Fit went to show
// every frame, whichever is further.
const Z_MIN = 0.02;
const Z_MAX = 4;
const STEP = 1.5;
// Screen pixels kept clear around the frames when fitting them, and above
// and below for the title and the controls.
const FIT_PAD = 40;
const FIT_TOP = 56;
const FIT_BOTTOM = 64;
// How far a press may move and still be a click (into a frame, or out).
const CLICK_SLOP = 4;
// A failed load is tried again after this, doubling up to the most.
const RETRY_MS = 1000;
const RETRY_MAX_MS = 30_000;
// The most of a refusal's text the board shows.
const NOTE_MAX_BYTES = 4096;

const $ = (id) => document.getElementById(id);
const viewport = $("viewport");
const world = $("world");
const camera = { x: 0, y: 0, z: 1 };
let zFloor = Z_MIN;
// A frame's file at most, as the manifest says (app.mjs FILE_MAX_BYTES);
// nothing loads before the manifest has said it.
let fileMax = 0;
// id → { frame, el, body, label, note, content, url, next, ticket, tries, retry, problem, natural, order, rect }
const shown = new Map();
// whether this person has moved the camera: until then it fits the
// frames as their pictures' sizes come in
let moved = false;
let fitted = false;

const og = document.querySelector('meta[property="og:title"]')?.content;
if (og) {
  $("title").textContent = og;
  document.title = og;
}

function apply() {
  world.style.transform = `translate(${camera.x}px, ${camera.y}px) scale(${camera.z})`;
  world.style.setProperty("--z", String(camera.z));
  $("actual").textContent = `${Math.round(camera.z * 100)}%`;
}

function zoomAt(sx, sy, factor) {
  const z = Math.min(Z_MAX, Math.max(zFloor, camera.z * factor));
  // the board's point under (sx, sy) stays there
  camera.x = sx - ((sx - camera.x) / camera.z) * z;
  camera.y = sy - ((sy - camera.y) / camera.z) * z;
  camera.z = z;
  apply();
}

function panBy(dx, dy) {
  camera.x += dx;
  camera.y += dy;
  apply();
}

// Every frame in view, as large as fits and never past actual size, however
// far apart the agent put them.
function fit() {
  const rects = [...shown.values()].map((s) => s.rect);
  if (rects.length === 0) return;
  const left = Math.min(...rects.map((r) => r.x));
  const top = Math.min(...rects.map((r) => r.y));
  const right = Math.max(...rects.map((r) => r.x + r.w));
  const bottom = Math.max(...rects.map((r) => r.y + r.h));
  const w = Math.max(1, innerWidth - 2 * FIT_PAD);
  const h = Math.max(1, innerHeight - FIT_TOP - FIT_BOTTOM - 2 * FIT_PAD);
  const z = Math.min(1, w / (right - left), h / (bottom - top));
  zFloor = Math.min(Z_MIN, z);
  camera.z = z;
  camera.x = FIT_PAD + (w - (right - left) * z) / 2 - left * z;
  camera.y = FIT_TOP + FIT_PAD + (h - (bottom - top) * z) / 2 - top * z;
  apply();
}

function sizeOf(s) {
  const f = s.frame;
  if (f.kind === "html") return { w: f.width, h: f.height };
  const n = s.natural ?? PICTURE_GUESS;
  if (f.width === null) return { w: n.width, h: n.height };
  return { w: f.width, h: Math.round((f.width * n.height) / n.width) };
}

function layout() {
  const all = [...shown.values()].sort((a, b) => a.order - b.order);
  for (const s of all) s.rect = { x: s.frame.x, y: s.frame.y, ...sizeOf(s) };
  const placed = all.filter((s) => s.frame.x !== null);
  let x = 0;
  let y = placed.length ? Math.max(...placed.map((s) => s.rect.y + s.rect.h)) + 2 * GAP : 0;
  let row = 0;
  for (const s of all.filter((s) => s.frame.x === null)) {
    if (x > 0 && x + s.rect.w > ROW_MAX) {
      x = 0;
      y += row + GAP;
      row = 0;
    }
    s.rect.x = x;
    s.rect.y = y;
    x += s.rect.w + GAP;
    row = Math.max(row, s.rect.h);
  }
  for (const s of all) {
    Object.assign(s.el.style, { left: `${s.rect.x}px`, top: `${s.rect.y}px`, width: `${s.rect.w}px`, height: `${s.rect.h}px` });
  }
}

function sizeLabel(f) {
  if (f.kind === "image") return "Image";
  if (f.size === "custom") return `${f.width} wide`;
  return `${f.size[0].toUpperCase()}${f.size.slice(1)} ${f.width}`;
}

// What a load brought, made ready behind the content shown: a page, or a
// picture (its bytes as an object URL, given back when it goes).
function made(s, f, body) {
  if (f.kind === "html") {
    const frame = document.createElement("iframe");
    frame.setAttribute("sandbox", SANDBOX);
    frame.setAttribute("referrerpolicy", "no-referrer");
    frame.title = f.label;
    frame.srcdoc = body;
    return { el: frame, url: null };
  }
  const img = document.createElement("img");
  img.alt = f.label;
  img.draggable = false;
  const url = URL.createObjectURL(body);
  img.src = url;
  return { el: img, url };
}

function drop(part) {
  if (!part) return;
  part.el.remove();
  if (part.url) URL.revokeObjectURL(part.url);
}

// Loads frame `s` at its rev, behind what it shows. Only the newest load
// of a frame still on the board counts (`ticket`).
function load(s) {
  const f = s.frame;
  const ticket = {};
  s.ticket = ticket;
  clearTimeout(s.retry);
  drop(s.next);
  s.next = null;
  s.el.dataset.rev = String(f.rev);
  const current = () => s.ticket === ticket && shown.get(f.id) === s;
  const failed = (why) => {
    if (!current()) return;
    drop(s.next);
    s.next = null;
    s.problem = why;
    s.el.dataset.failed = String(f.rev);
    noted(s);
    s.tries += 1;
    s.retry = setTimeout(() => current() && load(s), Math.min(RETRY_MAX_MS, RETRY_MS * 2 ** (s.tries - 1)));
  };
  const ready = (part) => {
    if (!current()) return drop(part);
    drop(s.content);
    part.el.classList.remove("next");
    Object.assign(s, { content: part, next: null, tries: 0, problem: null });
    delete s.el.dataset.failed;
    s.el.dataset.loaded = String(f.rev);
    noted(s);
    if (f.kind === "image") {
      s.natural = { width: part.el.naturalWidth || PICTURE_GUESS.width, height: part.el.naturalHeight || PICTURE_GUESS.height };
      layout();
      if (!moved) fit();
    }
  };
  // the whole of it, the body's bytes included: a connection that drops
  // after the headers fails here like one that drops before them
  (async () => {
    try {
      const r = await fetch(`/frames/${f.id}?r=${f.rev}`, { cache: "no-store" });
      if (!current()) return void r.body?.cancel().catch(() => {});
      if (!r.ok) {
        // the app's refusals are plain text saying why (app.mjs `said`)
        const plain = r.headers.get("content-type")?.startsWith("text/plain");
        const said = plain ? new TextDecoder().decode(await bytesOf(r, NOTE_MAX_BYTES)).trim() : "";
        return failed(said || `It didn't load (${r.status}).`);
      }
      const bytes = await bytesOf(r, fileMax);
      if (!current()) return;
      const body = f.kind === "html" ? new TextDecoder().decode(bytes) : new Blob([bytes], { type: r.headers.get("content-type") ?? "" });
      const part = made(s, f, body);
      part.el.classList.add("next");
      part.el.addEventListener("load", () => ready(part), { once: true });
      if (f.kind === "image") part.el.addEventListener("error", () => (current() ? (drop(part), failed("This picture didn't load.")) : drop(part)), { once: true });
      s.next = part;
      s.body.append(part.el);
    } catch (e) {
      failed(e instanceof TooLarge ? e.message : "The board couldn't reach it.");
    }
  })();
}

// A body read whole, but never past `max` bytes: past it, the read stops
// there and the load fails saying so, whatever the file's size said.
class TooLarge extends Error {}
async function bytesOf(r, max) {
  const reader = r.body.getReader();
  const parts = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > max) {
      reader.cancel().catch(() => {});
      throw new TooLarge(`It is over ${Math.round(max / 1024 / 1024)} MiB, a frame's most.`);
    }
    parts.push(value);
  }
  const bytes = new Uint8Array(size);
  let at = 0;
  for (const part of parts) {
    bytes.set(part, at);
    at += part.byteLength;
  }
  return bytes;
}

function make(f, order) {
  const el = document.createElement("div");
  el.className = "bd-frame";
  el.dataset.id = f.id;
  el.dataset.kind = f.kind;
  const label = document.createElement("div");
  label.className = "bd-label";
  const body = document.createElement("div");
  body.className = "bd-body";
  const note = document.createElement("p");
  note.className = "bd-note";
  note.hidden = true;
  body.append(note);
  el.append(label, body);
  if (f.kind === "html") {
    const shield = document.createElement("div");
    shield.className = "bd-shield";
    el.append(shield);
  }
  world.append(el);
  return { frame: f, el, body, label, note, content: null, next: null, ticket: null, tries: 0, retry: 0, problem: null, natural: null, order, rect: null };
}

function labelled(s) {
  const name = document.createElement("b");
  name.textContent = s.frame.label;
  const size = document.createElement("span");
  size.textContent = sizeLabel(s.frame);
  s.label.replaceChildren(name, size);
  if (s.content?.el.tagName === "IFRAME") s.content.el.title = s.frame.label;
  else if (s.content) s.content.el.alt = s.frame.label;
}

// Why a frame shows nothing, or not its newest: in its place when it has
// nothing to show, else over the top of what it shows.
function noted(s) {
  s.note.hidden = s.problem === null;
  s.note.textContent = s.problem ?? "";
  s.note.classList.toggle("over", s.content !== null);
}

function draw(frames) {
  const ids = new Set(frames.map((f) => f.id));
  for (const [id, s] of shown) {
    if (!ids.has(id)) gone(id, s);
  }
  frames.forEach((f, order) => {
    let s = shown.get(f.id);
    if (s && s.frame.kind !== f.kind) {
      gone(f.id, s);
      s = null;
    }
    if (!s) {
      s = make(f, order);
      shown.set(f.id, s);
      s.frame = f;
      load(s);
    } else {
      const again = s.frame.rev !== f.rev;
      s.frame = f;
      s.order = order;
      // a new rev, or one that failed: the board changed, so try it now
      if (again || s.problem !== null) load(s);
    }
    labelled(s);
  });
  layout();
  const n = frames.length;
  $("count").textContent = n ? `${n} frame${n === 1 ? "" : "s"}` : "";
  $("empty").hidden = n > 0;
  if (n > 0 && !fitted) {
    fitted = true;
    fit();
  }
}

function gone(id, s) {
  clearTimeout(s.retry);
  drop(s.next);
  drop(s.content);
  s.el.remove();
  shown.delete(id);
  if (!document.querySelector(".bd-frame.on")) $("done").hidden = true;
}

function status(text) {
  $("status").hidden = !text;
  $("status").textContent = text ?? "";
}

fragment.live(
  "frames",
  {},
  ({ frames, file_max_bytes }) => {
    fileMax = file_max_bytes;
    status(null);
    draw(frames);
  },
  (e) => status(`The board didn't load: ${e.message}`),
);

fragment.closed(({ code }) => status(code === 4004 ? "This board was deleted." : "Your access to this board changed: reload to see it."));

// Clicking into a frame's page lets it take the pointer, the wheel and the
// keys; Done (or the board, or Escape while the board has the keys) gives
// them back. Done is there because a frame's keys never reach this page.
function into(el) {
  for (const s of shown.values()) s.el.classList.toggle("on", s.el === el);
  $("done").hidden = el === null;
}
$("done").onclick = () => into(null);

// A drag moves the board, and two fingers pinch it.
const pointers = new Map();
let press = null;

viewport.addEventListener("pointerdown", (e) => {
  if (e.pointerType === "mouse" && e.button !== 0 && e.button !== 1) return;
  viewport.setPointerCapture(e.pointerId);
  pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
  if (pointers.size === 1) press = { x: e.clientX, y: e.clientY, far: false, frame: e.target.closest(".bd-shield")?.parentElement ?? null };
  else if (press) press.far = true;
});

viewport.addEventListener("pointermove", (e) => {
  const was = pointers.get(e.pointerId);
  if (!was) return;
  const now = { x: e.clientX, y: e.clientY };
  if (pointers.size === 1) {
    panBy(now.x - was.x, now.y - was.y);
    if (press && Math.hypot(now.x - press.x, now.y - press.y) > CLICK_SLOP) press.far = true;
  } else if (pointers.size === 2) {
    const [other] = [...pointers].filter(([id]) => id !== e.pointerId).map(([, p]) => p);
    const before = Math.hypot(was.x - other.x, was.y - other.y);
    const after = Math.hypot(now.x - other.x, now.y - other.y);
    panBy((now.x - was.x) / 2, (now.y - was.y) / 2);
    if (before > 0) zoomAt((now.x + other.x) / 2, (now.y + other.y) / 2, after / before);
  }
  pointers.set(e.pointerId, now);
  if (press?.far) {
    moved = true;
    viewport.classList.add("panning");
  }
});

function lift(e) {
  if (!pointers.delete(e.pointerId)) return;
  if (pointers.size > 0) return;
  viewport.classList.remove("panning");
  if (press && !press.far && e.type === "pointerup") into(press.frame);
  press = null;
}
viewport.addEventListener("pointerup", lift);
viewport.addEventListener("pointercancel", lift);

viewport.addEventListener(
  "wheel",
  (e) => {
    e.preventDefault();
    moved = true;
    const unit = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? innerHeight : 1;
    // a trackpad's pinch is a wheel with ctrl held (Chrome, Firefox, Edge)
    if (e.ctrlKey || e.metaKey) zoomAt(e.clientX, e.clientY, Math.exp(-e.deltaY * unit * 0.01));
    else panBy(-e.deltaX * unit, -e.deltaY * unit);
  },
  { passive: false },
);

// Safari's trackpad pinch (and the page's own zoom on iOS, held off)
let gesture = null;
document.addEventListener("gesturestart", (e) => {
  e.preventDefault();
  gesture = pointers.size < 2 ? { z: camera.z } : null;
});
document.addEventListener("gesturechange", (e) => {
  e.preventDefault();
  if (!gesture || pointers.size >= 2) return;
  moved = true;
  zoomAt(e.clientX, e.clientY, (gesture.z * e.scale) / camera.z);
});
document.addEventListener("gestureend", () => {
  gesture = null;
});

addEventListener("keydown", (e) => {
  if (e.key === "Escape") into(null);
});

const centre = () => [innerWidth / 2, innerHeight / 2];
$("in").onclick = () => {
  moved = true;
  zoomAt(...centre(), STEP);
};
$("out").onclick = () => {
  moved = true;
  zoomAt(...centre(), 1 / STEP);
};
$("actual").onclick = () => {
  moved = true;
  zoomAt(...centre(), 1 / camera.z);
};
$("fit").onclick = () => {
  moved = true;
  fit();
};

apply();
