// The board's page: every frame of the app's manifest (`frames`) on one
// canvas, moved with a drag, a wheel or a trackpad, zoomed with a pinch
// or ctrl/⌘ and the wheel. The manifest is a live query: when an agent's
// put_frame commits it answers again, and each frame whose `rev` moved
// loads again in place (a page behind the one shown, swapped in once it
// has loaded), the others untouched. The camera is this page's own and
// nothing the manifest says moves it.
//
// A frame's page runs in an iframe sandboxed without allow-same-origin:
// an opaque origin, so its scripts reach neither this page, nor the
// viewer's session, nor this origin's cookies or storage. It may run
// scripts and send its own forms, and nothing else (no popups, modals,
// downloads or top navigation). app.mjs serves its files with the same
// sandbox as a policy (`FRAME_POLICY`), so the two say the same.
import * as fragment from "./__fragment.js";

const SANDBOX = "allow-scripts allow-forms";
// The layout the board makes for frames the agent placed nowhere, in
// board pixels: left to right in the order they were first put, a new
// row past ROW_MAX, below every placed frame.
const GAP = 160;
const ROW_MAX = 6000;
// A picture's size before it has loaded.
const PICTURE_GUESS = { width: 800, height: 600 };
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

const $ = (id) => document.getElementById(id);
const viewport = $("viewport");
const world = $("world");
const camera = { x: 0, y: 0, z: 1 };
// id → { frame, el, body, content, next, natural }
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
  const z = Math.min(Z_MAX, Math.max(Z_MIN, camera.z * factor));
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

// Every frame in view, as large as fits and never past actual size.
function fit() {
  const rects = [...shown.values()].map((s) => s.rect);
  if (rects.length === 0) return;
  const left = Math.min(...rects.map((r) => r.x));
  const top = Math.min(...rects.map((r) => r.y));
  const right = Math.max(...rects.map((r) => r.x + r.w));
  const bottom = Math.max(...rects.map((r) => r.y + r.h));
  const w = Math.max(1, innerWidth - 2 * FIT_PAD);
  const h = Math.max(1, innerHeight - FIT_TOP - FIT_BOTTOM - 2 * FIT_PAD);
  const z = Math.min(1, Math.max(Z_MIN, Math.min(w / (right - left), h / (bottom - top))));
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

function source(f) {
  const path = `frames/${f.id}/${f.file}`;
  if (f.kind === "html") return `/${path.split("/").map(encodeURIComponent).join("/")}?v=${f.rev}`;
  // a picture comes as the platform serves a file, a blob's too
  return `/__file?path=${encodeURIComponent(path)}&r=${f.rev}`;
}

function page(s) {
  const f = s.frame;
  const frame = document.createElement("iframe");
  frame.setAttribute("sandbox", SANDBOX);
  frame.setAttribute("referrerpolicy", "no-referrer");
  frame.title = f.label;
  frame.src = source(f);
  const rev = f.rev;
  frame.addEventListener("load", () => {
    if (s.next === frame) {
      s.content.remove();
      frame.classList.remove("next");
      s.content = frame;
      s.next = null;
    }
    if (s.content === frame) s.el.dataset.loaded = String(rev);
  });
  return frame;
}

function picture(s) {
  const img = document.createElement("img");
  img.alt = s.frame.label;
  img.draggable = false;
  img.addEventListener("load", () => {
    s.natural = { width: img.naturalWidth || PICTURE_GUESS.width, height: img.naturalHeight || PICTURE_GUESS.height };
    s.el.dataset.loaded = img.dataset.rev;
    layout();
    if (!moved) fit();
  });
  return img;
}

function load(s) {
  const f = s.frame;
  s.el.dataset.rev = String(f.rev);
  if (f.kind === "image") {
    s.content.dataset.rev = String(f.rev);
    s.content.src = source(f);
    return;
  }
  // the page shown stays until the next one has loaded behind it
  s.next?.remove();
  s.next = page(s);
  s.next.classList.add("next");
  s.body.append(s.next);
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
  el.append(label, body);
  const s = { frame: f, el, body, label, content: null, next: null, natural: null, order, rect: null };
  if (f.kind === "html") {
    s.content = page(s);
    const shield = document.createElement("div");
    shield.className = "bd-shield";
    el.append(shield);
  } else {
    s.content = picture(s);
  }
  body.append(s.content);
  el.dataset.rev = String(f.rev);
  if (f.kind === "image") {
    s.content.dataset.rev = String(f.rev);
    s.content.src = source(f);
  }
  world.append(el);
  return s;
}

function labelled(s) {
  const name = document.createElement("b");
  name.textContent = s.frame.label;
  const size = document.createElement("span");
  size.textContent = sizeLabel(s.frame);
  s.label.replaceChildren(name, size);
  if (s.content.tagName === "IFRAME") s.content.title = s.frame.label;
  else s.content.alt = s.frame.label;
}

function draw(frames) {
  const ids = new Set(frames.map((f) => f.id));
  for (const [id, s] of shown) {
    if (!ids.has(id)) {
      s.el.remove();
      shown.delete(id);
    }
  }
  frames.forEach((f, order) => {
    let s = shown.get(f.id);
    if (s && s.frame.kind !== f.kind) {
      s.el.remove();
      s = null;
    }
    if (!s) {
      s = make(f, order);
      shown.set(f.id, s);
    } else {
      const again = s.frame.rev !== f.rev;
      s.frame = f;
      s.order = order;
      if (again) load(s);
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

function status(text) {
  $("status").hidden = !text;
  $("status").textContent = text ?? "";
}

fragment.live(
  "frames",
  {},
  ({ frames }) => {
    status(null);
    draw(frames);
  },
  (e) => status(`The board didn't load: ${e.message}`),
);

fragment.closed(({ code }) => status(code === 4004 ? "This board was deleted." : "Your access to this board changed: reload to see it."));

// Clicking into a frame's page lets it take the pointer and the wheel;
// clicking the board, or Escape, gives them back.
function into(el) {
  for (const s of shown.values()) s.el.classList.toggle("on", s.el === el);
}

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
