// The files viewer, served by the platform on every fragment as __files
// (with ./__files.js and ./__files.css): the fragment's content files, on
// live and main, as a tree beside a reader, the way the notes template's
// vault viewer shows them. Markdown renders through the platform's renderer
// (./__markdown.js), and its [[wikilinks]] and relative links open here; any
// other text shows with line numbers; pictures, video, and sound play
// inline; anything else is a download. A file's text becomes DOM through
// the DOM's constructors only, never as markup, as the chat's page renders
// an agent's: a file can never add markup or script to this origin, and
// HTML in one shows as text (xtask's check refuses an HTML sink here).
//
// A file opens by the hash (`#/notes/a.md`, or `#/notes/a.md#a-heading`);
// when none is named, the first page: the one the fragment's manifest names
// (`files.home`), else README.md at the root if it says something, else
// index.md at the root, else the first markdown file by path. The list is
// `__files` asked for JSON and the bytes `__file?path=`, as anyone who may
// see the fragment reads them; `__watch` (viewers and up) says when main
// moves, and the tree and the open file follow. Framed (a pane of a page),
// the reader's bar also offers to open a file as a pane of its own: it asks
// the page around it, `{fragment: "open", url, title}`.
//
// It is also a fragment's own page when its manifest says so (`files.page`:
// a brain's, a notes vault's), and its layer comes from there too (FIN-372):
// a search box when the fragment has an index (`__search`, by words and
// meaning, as the asker), and a status line its own query answers
// (`files.status`: `{line, busy}`), asked again while busy and as main
// moves. Narrow, the page comes first: the search sits in the bar, and the
// tree waits behind its button.
//
// Beside the reader, a graph (`#graph`, `#graph/<path>` with a page chosen):
// the markdown pages and the links between them, as the reader resolves
// them, coloured by top folder. A page chosen there is previewed beside it,
// with what it links to and what links to it; a page read says what links
// to it below. The fragment opens on it when its manifest says so
// (`files.graph`: a brain's). Its links come from each page's bytes read
// at their version (`__file?path=&v=`, kept for good by the browser's
// cache), so a visit again reads only the pages that changed. Its layout is
// a small force simulation of our own: pages are first-party source, served
// as git has them, and a fragment's few thousand pages need no library.

import { markdown as render, resolvePath, scheme, slug, withoutFrontMatter } from "./__markdown.js";

// Each icon's shapes, on a 24-unit grid (files.css strokes them).
const ICON = {
  menu: [["path", { d: "M4 7h16M4 12h16M4 17h16" }]],
  filter: [["path", { d: "M4 5h16l-6 7v6l-4 2v-8z" }]],
  search: [["circle", { cx: 11, cy: 11, r: 6.5 }], ["path", { d: "m20 20-4.2-4.2" }]],
  chevron: [["path", { d: "m9 6 6 6-6 6" }]],
  folder: [["path", { d: "M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" }]],
  file: [["path", { d: "M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" }], ["path", { d: "M14 3v5h5" }]],
  text: [["path", { d: "M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" }], ["path", { d: "M14 3v5h5M9 13h6M9 17h4" }]],
  image: [["rect", { x: 3, y: 4, width: 18, height: 16, rx: 2 }], ["circle", { cx: 9, cy: 10, r: 1.6 }], ["path", { d: "m21 16-5-5-9 9" }]],
  media: [["rect", { x: 3, y: 4, width: 18, height: 16, rx: 2 }], ["path", { d: "m10 9 5 3-5 3z" }]],
  raw: [["path", { d: "M14 4h6v6M20 4l-9 9" }], ["path", { d: "M18 14v4a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h4" }]],
  pane: [["rect", { x: 3, y: 4, width: 18, height: 16, rx: 2 }], ["path", { d: "M14 4v16" }]],
  download: [["path", { d: "M12 4v11M7 10l5 5 5-5M5 20h14" }]],
  back: [["path", { d: "m15 6-6 6 6 6" }]],
  forward: [["path", { d: "m9 6 6 6-6 6" }]],
  graph: [["circle", { cx: 6, cy: 7, r: 2.5 }], ["circle", { cx: 18, cy: 8, r: 2.5 }], ["circle", { cx: 10, cy: 18, r: 2.5 }], ["path", { d: "M8.4 7.3l7.1.6M7 9.3l2.1 6.3M16.4 10.1l-4.8 6" }]],
  page: [["path", { d: "M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" }], ["path", { d: "M14 3v5h5M9 13h6M9 17h4" }]],
  plus: [["path", { d: "M5 12h14M12 5v14" }]],
  minus: [["path", { d: "M5 12h14" }]],
  fit: [["path", { d: "M8 4H5a1 1 0 0 0-1 1v3M16 4h3a1 1 0 0 1 1 1v3M8 20H5a1 1 0 0 1-1-1v-3M16 20h3a1 1 0 0 0 1-1v-3" }]],
  close: [["path", { d: "M18 6 6 18M6 6l12 12" }]],
  focus: [["circle", { cx: 12, cy: 12, r: 8 }], ["circle", { cx: 12, cy: 12, r: 3 }]],
};
const SVG_NS = "http://www.w3.org/2000/svg";
// An icon as DOM: an <svg> of its shapes, hidden from assistive technology.
function icon(name) {
  const shapes = ICON[name];
  if (!shapes) throw new Error(`files: no icon ${name}`);
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("aria-hidden", "true");
  for (const [tag, attrs] of shapes) {
    const shape = document.createElementNS(SVG_NS, tag);
    for (const [key, value] of Object.entries(attrs)) shape.setAttribute(key, String(value));
    svg.append(shape);
  }
  return svg;
}

// What a file is, by its extension; "text" is a guess the bytes confirm.
const KINDS = {
  markdown: "md markdown",
  image: "png jpg jpeg gif webp avif svg ico bmp",
  video: "mp4 webm mov m4v",
  audio: "mp3 wav ogg oga m4a flac aac",
  binary: "pdf zip gz tgz tar wasm woff woff2 ttf otf sqlite db bin exe dmg psd",
};
const KIND = Object.fromEntries(Object.entries(KINDS).flatMap(([kind, exts]) => exts.split(" ").map((e) => [e, kind])));
const kind = (path) => KIND[(path.match(/\.([^./]+)$/) || [])[1]?.toLowerCase()] || "text";
const ICON_OF = { markdown: "text", image: "image", video: "media", audio: "media", binary: "file", text: "text" };

// A text file past this is a download; one up to the second is read on hover.
const TEXT_MAX = 1024 * 1024;
const PREFETCH_MAX = 256 * 1024;
// Past this many files, folders start closed.
const OPEN_FOLDERS_MAX = 100;
// hits a search shows, and how soon a busy status line is asked again
const HITS_MAX = 20;
const TYPING_MS = 250;
const STATUS_BUSY_MS = 5000;
// The graph: the pages it reads at most (by path; past it, it says so), the
// reads it keeps out at once, the links it takes from one page, the pages
// it names (by links) when it cannot name them all, and the most it shows
// a preview's text.
const GRAPH_PAGES_MAX = 2000;
const GRAPH_READS = 6;
const GRAPH_LINKS_MAX = 500;
const GRAPH_NAMED = 48;
const GRAPH_ALL_NAMED = 80;
const PEEK_CHARS = 700;

const $ = (id) => document.getElementById(id);
function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}
// An element with its attributes and children: the page is built of these.
function h(tag, attrs, ...children) {
  const e = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (key === "class") e.className = value;
    else if (key === "hidden") e.hidden = value;
    else e.setAttribute(key, value);
  }
  e.append(...children);
  return e;
}
const decode = (s) => {
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
};
const fileUrl = (path) => `__file?path=${encodeURIComponent(path)}`;
const route = (path, anchor) => `#/${path.split("/").map(encodeURIComponent).join("/")}${anchor ? `#${encodeURIComponent(anchor)}` : ""}`;
const folder = (path) => path.slice(0, Math.max(path.lastIndexOf("/"), 0));
const base = (path) => path.slice(path.lastIndexOf("/") + 1);
const bytes = (n) => (n < 1024 ? `${n} B` : n < 1048576 ? `${(n / 1024).toFixed(1)} KB` : `${(n / 1048576).toFixed(1)} MB`);

const fragmentName = document.body.dataset.fragment || "";
let title = document.body.dataset.title || fragmentName;
const framed = parent !== window;

let files = [];                // [{path, size}], by path
let index = new Map();         // path -> its row
let lower = new Map();         // lowercased path -> path
let byName = new Map();        // lowercased file name, and without .md -> [path]
const opened = new Set();      // the folders shown open
let current = null;            // the file in the reader
let nav = 0;                   // the view asked for last (a file, a search, the landing): an earlier one's answer is dropped
const texts = new Map();       // a hovered file's read, taken by its open
let home = null;               // the manifest's first page (`files.home`)
let statusOp = null;           // the manifest's status query (`files.status`)
let searching = false;         // a search's hits are in the reader
let typing = null;             // a search waiting for the typing to stop
let inflight = false;          // a search asked and not answered yet
let drawn = null;              // the open note as drawn ({path, text}), to draw again
let ready = null;              // the file whose view is in the reader, done (not still being read)
let graphFirst = false;        // the manifest opens it on its graph (`files.graph`)
let mode = "page";             // what the main pane shows: "page" (the reader) or "graph"

// ---- the page ----

document.body.replaceChildren(
  h("div", { class: "files", id: "files" },
    h("aside", { class: "side" },
      h("div", { class: "side-top", id: "side-top" },
        h("div", { class: "brand", id: "brand" }),
        h("label", { class: "filter search", id: "search", hidden: true }, icon("search"), h("input", { id: "q", type: "search", placeholder: `Search ${title}`, autocomplete: "off", "aria-label": `Search ${title}` })),
        h("label", { class: "filter", id: "filter-box" }, icon("filter"), h("input", { id: "filter", type: "search", placeholder: "Filter files", autocomplete: "off", spellcheck: "false", "aria-label": "Filter files" })),
      ),
      h("nav", { class: "tree", id: "tree", "aria-label": "Files" }),
      h("div", { class: "side-foot", id: "foot" },
        h("div", { class: "status", id: "status", hidden: true, role: "status" }, h("span", { class: "dot" }), h("span", { id: "status-line" })),
        h("div", { id: "count" }),
      ),
    ),
    h("div", { class: "scrim", id: "scrim" }),
    h("main", { class: "main" },
      h("header", { class: "bar", id: "bar" },
        h("button", { class: "icon menu", id: "menu", type: "button", title: "Files", "aria-label": "Show or hide the files" }, icon("menu")),
        h("button", { class: "icon", id: "back", type: "button", title: "Back", "aria-label": "Back", disabled: "" }, icon("back")),
        h("button", { class: "icon", id: "forward", type: "button", title: "Forward", "aria-label": "Forward", disabled: "" }, icon("forward")),
        h("div", { class: "crumbs", id: "crumbs" }),
        h("span", { class: "meta", id: "meta" }),
        h("a", { class: "icon", id: "raw", target: "_blank", rel: "noopener", title: "The file itself", "aria-label": "The file itself", hidden: true }, icon("raw")),
        h("button", { class: "icon", id: "pop", type: "button", title: "Open in a pane of its own", "aria-label": "Open in a pane of its own", hidden: true }, icon("pane")),
        h("div", { class: "views", role: "group", "aria-label": "View" },
          h("button", { class: "view", id: "to-graph", type: "button", title: "The graph of its pages", "aria-pressed": "false" }, icon("graph"), el("span", null, "Graph")),
          h("button", { class: "view", id: "to-page", type: "button", title: "The page", "aria-pressed": "true" }, icon("page"), el("span", null, "Page")),
        ),
      ),
      h("div", { class: "stage", id: "stage" },
        h("div", { class: "reader", id: "reader" }),
        h("section", { class: "graph", id: "graph", "aria-label": "Graph of the pages", hidden: true },
          h("div", { class: "gbar", id: "gbar" },
            h("div", { class: "chips", id: "folders" }),
            h("button", { class: "chip toggle", id: "orphans", type: "button", "aria-pressed": "true", title: "Pages that link nowhere and that nothing links to" }, "Unlinked"),
            h("button", { class: "chip toggle", id: "local", type: "button", "aria-pressed": "false", disabled: "", title: "Only the chosen page and its links" }, icon("focus"), "Its links only"),
            h("span", { class: "ghint", id: "ghint" }),
          ),
          h("div", { class: "gcanvas", id: "gcanvas" }),
          h("div", { class: "gsay", id: "gsay", role: "status", hidden: true }),
          h("div", { class: "zoom" },
            h("button", { class: "icon", id: "zoom-out", type: "button", title: "Zoom out", "aria-label": "Zoom out" }, icon("minus")),
            h("button", { class: "icon", id: "zoom-in", type: "button", title: "Zoom in", "aria-label": "Zoom in" }, icon("plus")),
            h("button", { class: "icon", id: "zoom-fit", type: "button", title: "Fit to the window", "aria-label": "Fit to the window" }, icon("fit")),
          ),
        ),
        h("aside", { class: "peek", id: "peek", "aria-label": "The chosen page", hidden: true }),
      ),
    ),
  ),
);
$("brand").textContent = title;
const drawer = (open) => $("files").classList.toggle("nav", open);
const narrowing = matchMedia("(max-width: 520px)");
const narrow = () => narrowing.matches;
// Narrow, the search sits in the bar and a busy status under it; wide, both
// in the sidebar (one element each, moved).
function place() {
  if (narrow()) {
    $("bar").insertBefore($("search"), $("crumbs"));
    $("bar").after($("status"));
  } else {
    $("side-top").insertBefore($("search"), $("filter-box"));
    $("foot").prepend($("status"));
  }
  $("status").classList.toggle("strip", narrow());
  // the count gives way to a status line in the sidebar, not to one in the bar
  $("count").hidden = !$("status").hidden && !narrow();
}
narrowing.addEventListener("change", place);
place();

// ---- the list and the tree ----

async function load() {
  const r = await fetch("__files", { headers: { accept: "application/json" } });
  const body = await r.json().catch(() => null);
  if (!r.ok) throw new Error(body?.message || `http ${r.status}`);
  files = body.files;
  // its title as it is now: a rename while it is open is followed
  if (typeof body.title === "string" && body.title.trim() && body.title !== title) {
    title = body.title;
    $("brand").textContent = title;
    $("q").placeholder = `Search ${title}`;
    $("q").setAttribute("aria-label", `Search ${title}`);
    document.title = current ? `${base(current)} · ${title}` : title;
  }
  home = typeof body.home === "string" ? body.home : null;
  statusOp = typeof body.status === "string" ? body.status : null;
  graphFirst = body.graph === true;
  $("search").hidden = body.searchable !== true;
  $("files").classList.toggle("searchable", body.searchable === true);
  index = new Map(files.map((f) => [f.path, f]));
  lower = new Map(files.map((f) => [f.path.toLowerCase(), f.path]));
  byName = new Map();
  for (const { path } of files) {
    const name = base(path).toLowerCase();
    for (const key of new Set([name, name.replace(/\.md$/, "")])) byName.set(key, [...(byName.get(key) || []), path]);
  }
  $("count").textContent = `${files.length} ${files.length === 1 ? "file" : "files"}`;
  tree();
}

function tree() {
  const q = $("filter").value.trim().toLowerCase();
  const shown = q ? files.filter((f) => f.path.toLowerCase().includes(q)) : files;
  const root = { dirs: new Map(), files: [] };
  for (const f of shown) {
    let node = root;
    for (const part of f.path.split("/").slice(0, -1)) {
      if (!node.dirs.has(part)) node.dirs.set(part, { dirs: new Map(), files: [] });
      node = node.dirs.get(part);
    }
    node.files.push(f);
  }
  const rows = [];
  const walk = (node, prefix, depth) => {
    for (const [name, child] of [...node.dirs].sort((a, b) => a[0].localeCompare(b[0], undefined, { numeric: true }))) {
      const path = prefix + name;
      const open = !!q || opened.has(path);
      const row = el("button", "row dir");
      row.type = "button";
      row.dataset.dir = path;
      row.setAttribute("aria-expanded", String(open));
      row.append(h("span", { class: "chev" }, icon("chevron")), icon("folder"));
      row.append(el("span", "name", name));
      row.style.setProperty("--depth", depth);
      rows.push(row);
      if (open) walk(child, `${path}/`, depth + 1);
    }
    const active = activePath();
    for (const f of node.files) {
      const row = el("a", `row file${f.path === active ? " active" : ""}`);
      row.href = route(f.path);
      row.dataset.path = f.path;
      row.title = `${f.path} · ${bytes(f.size)}`;
      row.append(icon(ICON_OF[kind(f.path)]));
      row.append(el("span", "name", base(f.path)));
      row.style.setProperty("--depth", depth);
      rows.push(row);
    }
  };
  walk(root, "", 0);
  $("tree").replaceChildren(...(rows.length ? rows : [el("div", "none", q ? "No file matches" : "No files yet")]));
}

// The file the tree marks: the page chosen in the graph, else the one read.
const activePath = () => (mode === "graph" ? G.sel?.path ?? null : current);

// The open file's row, its folders opened, in view.
function mark() {
  const active = activePath();
  if (active) {
    const parts = active.split("/");
    for (let n = 1; n < parts.length; n++) opened.add(parts.slice(0, n).join("/"));
  }
  tree();
  document.querySelector("#tree .row.active")?.scrollIntoView({ block: "nearest" });
}

$("tree").addEventListener("click", (e) => {
  const dir = e.target.closest(".row.dir");
  if (dir) {
    const path = dir.dataset.dir;
    if (!opened.delete(path)) opened.add(path);
    tree();
  } else if (e.target.closest(".row.file")) {
    drawer(false);
  }
});
$("filter").addEventListener("input", tree);
$("filter").addEventListener("keydown", (e) => {
  if (e.key === "Enter") {
    const first = document.querySelector("#tree .row.file");
    if (first) location.hash = first.getAttribute("href");
  } else if (e.key === "Escape") {
    e.target.value = "";
    tree();
  }
});
addEventListener("keydown", (e) => {
  if (e.key === "/" && !e.target.closest?.("input, textarea")) {
    e.preventDefault();
    drawer(true);
    $("filter").focus();
  }
});
// Narrow, the tree slides in over the page; wide, it folds away and back.
$("menu").onclick = () => (narrow() ? drawer(!$("files").classList.contains("nav")) : $("files").classList.toggle("fold"));
$("scrim").onclick = () => drawer(false);

// ---- reading ----

// A file's text, or null when its bytes are not text (a NUL early on, or not UTF-8).
async function read(path) {
  const r = await fetch(fileUrl(path));
  if (!r.ok) throw new Error((await r.json().catch(() => null))?.message || `http ${r.status}`);
  const data = new Uint8Array(await r.arrayBuffer());
  if (data.subarray(0, 8000).includes(0)) return null;
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(data);
  } catch {
    return null;
  }
}

// A hovered file's read, taken once (a later open reads again), or a fresh one.
function take(path) {
  const pending = texts.get(path) || read(path);
  texts.delete(path);
  return pending;
}

document.addEventListener("pointerover", (e) => {
  const f = index.get(e.target.closest?.("a[data-path]")?.dataset.path);
  if (!f || f.path === current || texts.has(f.path) || f.size > PREFETCH_MAX || !["markdown", "text"].includes(kind(f.path))) return;
  if (texts.size >= 32) texts.delete(texts.keys().next().value);
  const pending = read(f.path);
  pending.catch(() => {});
  texts.set(f.path, pending);
}, { passive: true });

function bar(path, f) {
  const crumbs = $("crumbs");
  crumbs.replaceChildren();
  if (path) {
    // each folder above the file opens its first page (`inFolder`)
    path.split("/").forEach((part, n, all) => {
      if (n) crumbs.append(el("span", "sep", "/"));
      if (n === all.length - 1) return crumbs.append(el("strong", null, part));
      const a = el("a", "crumb", part);
      a.href = route(`${all.slice(0, n + 1).join("/")}/`);
      crumbs.append(a);
    });
  }
  $("meta").textContent = f ? bytes(f.size) : "";
  $("raw").hidden = !f;
  if (f) {
    $("raw").href = fileUrl(path);
    $("raw").toggleAttribute("download", kind(path) === "binary");
  }
  $("pop").hidden = !(f && framed);
  document.title = path ? `${base(path)} · ${title}` : title;
}

function say(title, text) {
  const box = el("div", "say");
  box.append(el("h2", null, title));
  if (text) box.append(el("p", null, text));
  return box;
}

function download(f, why) {
  const box = say(base(f.path), [why, `${bytes(f.size)}.`].filter(Boolean).join(" "));
  const a = el("a", "button");
  a.href = fileUrl(f.path);
  a.download = base(f.path);
  a.append(icon("download"), "Download");
  box.append(a);
  return box;
}

// Opens `path` in the reader; `keep` holds its place (a live refresh).
async function open(path, anchor, keep) {
  const n = ++nav;
  setView("page");
  const reader = $("reader");
  const top = keep ? reader.scrollTop : 0;
  current = path;
  drawn = null;
  ready = null;
  const f = index.get(path);
  bar(path, f);
  mark();
  drawer(false);
  let view;
  if (!f) {
    // a folder asked that holds no file (`inFolder` found none): git keeps
    // no empty folder, so it is one with nothing in it yet
    view = path.endsWith("/") ? say("Nothing here yet", `${path} has no files yet.`) : say("No such file", `${path} is not in ${fragmentName}.`);
  } else if (kind(path) === "image") {
    view = el("div", "media");
    const img = el("img");
    img.src = fileUrl(path);
    img.alt = path;
    img.onload = () => {
      if (current === path) $("meta").textContent = `${img.naturalWidth} × ${img.naturalHeight} · ${bytes(f.size)}`;
    };
    view.append(img);
  } else if (kind(path) === "video" || kind(path) === "audio") {
    view = el("div", "media");
    const player = el(kind(path));
    player.src = fileUrl(path);
    player.controls = true;
    player.preload = "metadata";
    view.append(player);
  } else if (kind(path) === "binary") {
    view = download(f);
  } else if (f.size > TEXT_MAX) {
    view = download(f, "Too large to show here.");
  } else {
    const text = await take(path).catch((e) => e);
    if (n !== nav) return;
    if (text instanceof Error) view = say("This file could not be read", text.message);
    else if (text === null) view = download(f);
    else if (kind(path) === "markdown") {
      view = page(text, path);
      drawn = { path, text };
    } else view = code(text);
  }
  reader.replaceChildren(view);
  reader.scrollTop = top;
  ready = path;
  if (anchor) reach(anchor);
}

// A markdown page as the reader shows it: the page, and below it the pages
// that link to it, once the graph knows them.
function page(text, path) {
  const doc = markdown(text, path);
  const from = [...(G.nodes.get(path)?.in ?? [])].sort((a, b) => titleOf(a).localeCompare(titleOf(b), undefined, { numeric: true }));
  if (!from.length) return doc;
  const box = h("nav", { class: "linked", "aria-label": "Linked from" }, el("h2", null, "Linked from"));
  for (const p of from) {
    const a = h("a", { class: "linked-row", href: route(p), "data-path": p }, h("span", { class: `dot ${G.nodes.get(p)?.tone ?? ""}` }), el("span", "name", titleOf(p)), el("span", "where", folder(p)));
    box.append(a);
  }
  doc.append(box);
  return doc;
}

function reach(anchor) {
  document.querySelector(`#reader [data-anchor="${CSS.escape(slug(anchor))}"]`)?.scrollIntoView({ block: "start" });
}

function landing() {
  setView("page");
  current = null;
  bar(null);
  mark();
  const n = files.length;
  $("reader").replaceChildren(
    n ? say("Pick a file", `${n} ${n === 1 ? "file" : "files"} in ${title}.`) : say("No files yet", `Nothing is in ${title} yet.`),
  );
}

// Whether a README says something: more than headings once its front
// matter is gone (a new repo's is often its title alone).
const says = (text) => typeof text === "string" && withoutFrontMatter(text).split("\n").some((line) => line.trim() && !/^\s*#/.test(line));

// The page shown when the hash names none (the header's rule).
async function first() {
  if (home && index.has(home)) return home;
  const readme = lower.get("readme.md");
  if (readme && (index.get(readme)?.size ?? 0) <= PREFETCH_MAX) {
    const pending = read(readme);
    pending.catch(() => {});
    texts.set(readme, pending);
    if (says(await pending.catch(() => null))) return readme;
  }
  return lower.get("index.md") || files.find((f) => kind(f.path) === "markdown")?.path || null;
}

// A folder the hash names (`#/skills/`, or one named without its slash):
// its first file, its README or index first; its folders open above it
// (`mark`). None when it names a file, or no folder holding one.
function inFolder(path) {
  const dir = path.replace(/\/+$/, "");
  if (!dir || index.has(dir)) return null;
  const under = files.map((f) => f.path).filter((p) => p.startsWith(`${dir}/`)).sort((a, b) => a.localeCompare(b, undefined, { numeric: true }));
  return under.find((p) => /^(readme|index)\.md$/i.test(p.slice(dir.length + 1))) ?? under[0] ?? null;
}

// The view the hash names. Any view asked for after this one wins: a
// search still out, a search still waiting for the typing to stop, and a
// file still being read are all dropped.
async function go() {
  const n = ++nav;
  clearTimeout(typing);
  typing = null;
  searching = inflight = false;
  stamp();
  const chosen = graphRoute(location.hash);
  if (chosen !== undefined) return showGraph(chosen);
  const [p, anchor] = location.hash.replace(/^#\/?/, "").split("#");
  const asked = decode(p);
  if (!asked && graphFirst) return showGraph(null);
  const path = (asked && inFolder(asked)) || asked || (await first());
  if (n !== nav) return;
  if (!path) return landing();
  // the file in the reader already, drawn: only its heading to reach. One
  // still being read is read again for this view (its read is dropped)
  if (path === ready && path === current && anchor && mode === "page") return reach(decode(anchor));
  open(path, anchor && decode(anchor));
}

$("reader").addEventListener("click", (e) => {
  const a = e.target.closest("a[href^='#/']");
  if (a && a.getAttribute("href") === location.hash) {
    // the hash is already this: no hashchange comes
    e.preventDefault();
    go();
  }
});
$("pop").onclick = () => {
  if (current) parent.postMessage({ fragment: "open", url: new URL(fileUrl(current), location.href).href, title: current }, "*");
};

// A text file, a line to a row, numbered by CSS (a copy takes the text alone).
function code(text) {
  const pre = el("pre", "code");
  const lines = text.replace(/\r?\n$/, "").split(/\r?\n/);
  pre.style.setProperty("--digits", String(lines.length).length);
  for (const line of lines) pre.append(el("span", "line", line));
  return pre;
}

// ---- following main ----

// Main's moves, for viewers and up. A feed that never opened was refused
// (a visitor below viewer), and the page keeps what it loaded; one that
// closes after opening comes back, and catches up on what it missed.
function watch(wait = 0) {
  const url = new URL("__watch", location.href);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  const ws = new WebSocket(url);
  let heard = false;
  ws.onmessage = (e) => {
    const frame = JSON.parse(e.data);
    if (frame.type === "hello" && wait) changed(null);
    if (frame.type === "changed") changed(frame.paths);
    heard = true;
  };
  ws.onclose = (e) => {
    if (heard && e.code !== 4003 && e.code !== 4004) setTimeout(() => watch(Math.min((wait || 500) * 2, 30000)), wait || 1000);
  };
}

// Changed paths (null: any), one refresh for a burst.
let stale = new Set();
let every = false;
let refreshing = null;
function changed(paths) {
  if (paths) paths.forEach((p) => stale.add(p));
  else every = true;
  for (const p of paths || [...texts.keys()]) texts.delete(p);
  clearTimeout(refreshing);
  refreshing = setTimeout(refresh, 150);
}

async function refresh() {
  const named = stale;
  const all = every;
  stale = new Set();
  every = false;
  const before = new Set(index.keys());
  await load().catch(() => {});
  status();
  // the graph, when drawn, reads again the pages whose version moved
  if (G.wanted) graphNow();
  // a search asked, out or shown keeps the reader: only the tree follows
  if (searching || inflight || typing) return;
  // the graph keeps its place: the reader follows when it is shown again
  if (mode === "graph") return;
  // the file open now, a file opened while the list was read included: read
  // again when main changed it, or when it was not listed and is now (opened
  // against the list before, it said there was no such file)
  const again = current && (all || named.has(current) || (!before.has(current) && index.has(current)));
  // a file came or went: the open note, unchanged, is drawn again from what
  // was read (a picture or a note it names may be here now, or gone)
  const moved = files.length !== before.size || files.some((f) => !before.has(f.path));
  if (again) open(current, null, true);
  else if (moved && current && drawn?.path === current) redraw();
  else if (!current) go();
}

function redraw() {
  const reader = $("reader");
  const top = reader.scrollTop;
  reader.replaceChildren(page(drawn.text, drawn.path));
  reader.scrollTop = top;
}

// ---- search: the fragment's own index, by words and meaning, as the
// asker (`__search`); a hit opens its file at its heading ----

async function search(q) {
  typing = null;
  const n = ++nav;
  inflight = true;
  const r = await fetch("__search", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ q, limit: HITS_MAX }) }).catch((e) => e);
  const body = r instanceof Error ? null : await r.json().catch(() => null);
  if (n !== nav) return;
  inflight = false;
  searching = true;
  setView("page");
  current = null;
  drawn = null;
  bar(null);
  mark();
  $("crumbs").replaceChildren(el("strong", null, "Search"));
  if (r instanceof Error || !r.ok) {
    $("reader").replaceChildren(say("Search did not answer", body?.message || (r instanceof Error ? r.message : `http ${r.status}`)));
    return;
  }
  const hits = body.results || [];
  const box = el("div", "hits");
  box.append(el("p", "said", hits.length ? `${hits.length} ${hits.length === 1 ? "section" : "sections"}, best first.` : "Nothing here matches that."));
  for (const hit of hits) {
    // a hit whose source is a file opens it; another's (a conversation) is shown
    const path = index.has(hit.source) ? hit.source : null;
    const row = el(path ? "a" : "div", "hit");
    if (path) row.href = route(path, hit.heading || undefined);
    row.append(el("div", "hit-title", hit.title || base(hit.source)));
    const where = [...(hit.context || []), hit.heading].filter((part) => part && part !== hit.title).join(" › ");
    if (where) row.append(el("div", "hit-where", where));
    if (hit.snippet) row.append(el("div", "hit-snippet", hit.snippet));
    row.append(el("div", "hit-path", hit.source));
    box.append(row);
  }
  $("reader").replaceChildren(box);
  $("reader").scrollTop = 0;
}

$("q").addEventListener("input", () => {
  clearTimeout(typing);
  drawer(false);
  const q = $("q").value.trim();
  if (!q) {
    typing = null;
    // the page the hash names again; a search still out is dropped
    if (searching || inflight) go();
    return;
  }
  typing = setTimeout(() => search(q), TYPING_MS);
});
$("q").addEventListener("keydown", (e) => {
  if (e.key === "Enter" && $("q").value.trim()) {
    clearTimeout(typing);
    search($("q").value.trim());
  } else if (e.key === "Escape") {
    $("q").value = "";
    $("q").dispatchEvent(new Event("input"));
  }
});

// ---- its status line: its own query's `{line, busy}`, asked on load, as
// main moves, and again while busy ----

let statusTimer = null;
async function status() {
  if (!statusOp) return;
  clearTimeout(statusTimer);
  const r = await fetch(`__op/${encodeURIComponent(statusOp)}`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ id: crypto.randomUUID(), input: {} }) }).catch(() => null);
  const said = r?.ok ? (await r.json().catch(() => null))?.result : null;
  const line = typeof said?.line === "string" ? said.line.slice(0, 120) : "";
  const busy = said?.busy === true;
  $("status-line").textContent = line;
  $("status").hidden = !line;
  $("status").classList.toggle("busy", busy);
  $("count").hidden = !!line && !narrow();
  if (busy) statusTimer = setTimeout(status, STATUS_BUSY_MS);
}

// ---- markdown, through the platform's renderer: its links and pictures
// are this fragment's files (from the linking file's folder, else the
// root; opened here) or the web's (http(s) and mailto), and nothing else.
// A picture on the web is fetched by each reader's browser, so its host
// learns who opened the file and when (a note's author can watch its
// readers this way); keeping pictures to the fragment's own files is
// Paul's call, not made here ----

// The file `self` as the viewer draws it, its links resolved from it (its
// own headings are its own, whichever page the reader last opened).
function markdown(text, self) {
  const at = folder(self);
  const doc = el("article", "doc");
  doc.append(
    render(withoutFrontMatter(text), {
      resolve: (target, how) => (how === "wiki" ? wikiTarget(target, at, self) : destination(target, at, self)),
      image: (src, how) => picture(src, how, at, self),
    }),
  );
  return doc;
}

// Where a link in the file `self` (in the folder `at`) goes: one of this
// fragment's files, a heading of this one, or an http(s) or mailto URL;
// anything else (javascript:, data:, a path that names no file here) is no
// link: null.
function destination(href, at, self) {
  // the scheme as a browser reads it (the renderer's one reading)
  const s = scheme(href);
  if (s !== null) return ["http", "https", "mailto"].includes(s) ? { href } : null;
  const [p, hash] = href.split("#");
  const anchor = hash ? decode(hash) : undefined;
  if (!p) return anchor && self ? internal(self, anchor) : null;
  const path = [resolvePath(at, decode(p)), resolvePath("", decode(p))]
    .flatMap((t) => [t.toLowerCase(), `${t.toLowerCase()}.md`])
    .map((t) => lower.get(t))
    .find(Boolean);
  return path ? internal(path, anchor) : null;
}

// A link the viewer opens: a file here, and a heading in it.
const internal = (path, anchor) => ({ href: route(path, anchor), path });

// A [[wikilink]]'s file: its path from the root or from this file's folder,
// with or without .md, else a file of that name (this folder's first).
function wiki(target, at) {
  const t = target.trim().replace(/^\.?\//, "").toLowerCase();
  for (const p of [t, resolvePath(at, t).toLowerCase()]) {
    const path = lower.get(p) ?? lower.get(`${p}.md`);
    if (path) return path;
  }
  const hits = (byName.get(base(t)) || []).filter((p) => !t.includes("/") || p.toLowerCase().replace(/\.md$/, "").endsWith(t.replace(/\.md$/, "")));
  return hits.find((p) => folder(p) === at) ?? hits[0] ?? null;
}

// [[name#heading]]: the file it names (`self`, with no name) and the heading.
function wikiTarget(ref, at, self) {
  const [name, anchor] = ref.split("#");
  const path = name.trim() ? wiki(name, at) : self;
  return path ? internal(path, anchor?.trim() || undefined) : null;
}

// A picture's URL: one of this fragment's pictures, or one on the web.
function picture(src, how, at, self) {
  if (how === "wiki") {
    const path = wiki(src, at);
    return path && kind(path) === "image" ? { src: fileUrl(path) } : null;
  }
  const to = destination(src, at, self);
  if (to?.path) return { src: fileUrl(to.path) };
  return to && ["http", "https"].includes(scheme(to.href)) ? { src: to.href } : null;
}

// ---- back and forward: this page's own steps through its history, so the
// buttons never leave it (framed, `history.back()` from its first step
// would move the page around it) ----

let step = 0;        // this history entry's place among the page's own
let steps = 0;       // the furthest one forward still there
let stamped = false;
function stamp() {
  const s = history.state;
  if (s && Number.isInteger(s.files)) {
    step = s.files;
    if (!stamped) steps = step;
  } else {
    // a new entry (or the first): the ones forward of it are gone
    step = stamped ? step + 1 : 0;
    steps = step;
    history.replaceState({ files: step }, "");
  }
  stamped = true;
  $("back").disabled = step <= 0;
  $("forward").disabled = step >= steps;
}
$("back").onclick = () => step > 0 && history.back();
$("forward").onclick = () => step < steps && history.forward();

// ---- the graph: the pages and the links between them (`#graph`, and
// `#graph/<path>` with one chosen) ----

const svgEl = (tag, attrs = {}) => {
  const e = document.createElementNS(SVG_NS, tag);
  for (const [key, value] of Object.entries(attrs)) e.setAttribute(key, String(value));
  return e;
};
// A page's name: its title (its first heading, once the graph read it),
// else its file's; an index or README is its folder's.
const titleOf = (path) => G.asks.get(path)?.title || nameOf(path);
function nameOf(path) {
  const b = base(path);
  if (/^(index|readme)\.md$/i.test(b) && path.includes("/")) return base(folder(path));
  return b.replace(/\.(md|markdown)$/i, "");
}
const topOf = (path) => (path.includes("/") ? path.slice(0, path.indexOf("/")) : "");
const clamp = (x, lo, hi) => Math.min(hi, Math.max(lo, x));
// A path's place, the same on every visit (FNV-1a).
function hash(s) {
  let x = 2166136261;
  for (let i = 0; i < s.length; i++) x = Math.imul(x ^ s.charCodeAt(i), 16777619);
  return x >>> 0;
}

// The colours a top folder takes, in turn (files.css's tone-0 to tone-7).
const TONES = 8;
// The simulation, in pixels at zoom 1: repulsion within a grid cell's
// reach, links as springs, each page drawn to its top folder's place.
const CELL = 180;
const REPEL = 2600;
const LINK = 64;
const SPRING = 0.035;
const PULL = 0.01;
const DECAY = 0.985;
const ALPHA_MIN = 0.02;
const FRAME_MS = 10;
const ZOOM_MIN = 0.1;
const ZOOM_MAX = 5;
const NAMES_ZOOM = 1.6;

const G = {
  wanted: false,       // asked for: shown, or the fragment opens on it
  asks: new Map(),     // path -> {v, asks: [[target, how]]}: its links as written, at its version
  nodes: new Map(),    // path -> {path, top, x, y, vx, vy, g, dot, label, out, in, deg, r, tone, anchor, off}
  edges: [],           // [{a, b, line}]: one per linked pair
  tops: [],            // the top folders ("" the files at the top)
  named: new Set(),    // the pages named when not all are
  building: null,
  again: false,
  built: false,
  left: 0,             // markdown pages not drawn (past its cap, or too large)
  alpha: 0,
  frame: null,
  k: 1,
  tx: 0,
  ty: 0,
  auto: true,          // fitted to the window until someone pans or zooms
  sel: null,
  hover: null,
  hidden: new Set(),   // top folders hidden
  orphans: true,
  local: false,
};

const svg = svgEl("svg", { class: "gsvg", role: "img", "aria-label": "The pages and the links between them" });
const viewG = svgEl("g");
const edgesG = svgEl("g", { class: "edges" });
const nodesG = svgEl("g", { class: "nodes" });
viewG.append(edgesG, nodesG);
svg.append(viewG);
$("gcanvas").append(svg);

function setView(m) {
  mode = m;
  $("files").dataset.view = m;
  $("reader").hidden = m !== "page";
  $("graph").hidden = m !== "graph";
  $("peek").hidden = m !== "graph" || !G.sel;
  $("to-graph").setAttribute("aria-pressed", String(m === "graph"));
  $("to-page").setAttribute("aria-pressed", String(m === "page"));
  if (m === "graph") kick(0);
  else if (G.frame) {
    cancelAnimationFrame(G.frame);
    G.frame = null;
  }
}

// `#graph` (null), `#graph/<path>` (that path), or not the graph (undefined).
function graphRoute(hash) {
  if (hash === "#graph") return null;
  if (!hash.startsWith("#graph/")) return undefined;
  return hash.slice("#graph/".length).split("/").map(decode).join("/") || null;
}
const graphHash = (path) => (path ? `#graph/${path.split("/").map(encodeURIComponent).join("/")}` : "#graph");

async function showGraph(chosen) {
  const n = nav;
  setView("graph");
  G.wanted = true;
  graphBar();
  await graphNow();
  if (n !== nav || mode !== "graph") return;
  choose(chosen && G.nodes.has(chosen) ? chosen : null, !G.auto);
}

function graphBar() {
  $("crumbs").replaceChildren(el("strong", null, "Graph"));
  const pages = G.nodes.size;
  const links = G.edges.length;
  $("meta").textContent = G.built ? `${pages} ${pages === 1 ? "page" : "pages"} · ${links} ${links === 1 ? "link" : "links"}` : "";
  $("raw").hidden = true;
  $("pop").hidden = true;
  document.title = `Graph · ${title}`;
  mark();
}

const sayGraph = (text) => {
  $("gsay").textContent = text;
  $("gsay").hidden = !text;
};

// The graph as the files are now: one build at a time, and one more after
// it when the files moved meanwhile.
function graphNow() {
  if (G.building) {
    G.again = true;
    return G.building;
  }
  G.building = build()
    .catch((e) => sayGraph(`The graph could not be drawn: ${e.message}`))
    .finally(() => {
      G.building = null;
      if (G.again) {
        G.again = false;
        graphNow();
      }
    });
  return G.building;
}

// Reads the pages whose version it has not read (a few at a time), then
// wires what they link to.
async function build() {
  const pages = files.filter((f) => kind(f.path) === "markdown");
  const shown = pages.filter((f) => f.size <= PREFETCH_MAX).slice(0, GRAPH_PAGES_MAX);
  G.left = pages.length - shown.length;
  // the release's own pages have no version: read once a visit
  const version = (f) => f.commit ?? "release";
  const todo = shown.filter((f) => G.asks.get(f.path)?.v !== version(f));
  let done = shown.length - todo.length;
  const progress = () => !G.built && sayGraph(`Reading the pages: ${done} of ${shown.length}`);
  if (todo.length) progress();
  let next = 0;
  const reader = async () => {
    while (next < todo.length) {
      const f = todo[next++];
      const url = f.commit ? `${fileUrl(f.path)}&v=${encodeURIComponent(f.commit)}` : fileUrl(f.path);
      const text = await fetch(url).then((r) => (r.ok ? r.text() : null)).catch(() => null);
      // a page that could not be read is read again next time
      G.asks.set(f.path, text === null ? { v: null, asks: [], title: "" } : { v: version(f), ...asksIn(text, f.path) });
      if (++done % 25 === 0) progress();
    }
  };
  await Promise.all(Array.from({ length: Math.min(GRAPH_READS, todo.length) }, reader));
  const keep = new Set(shown.map((f) => f.path));
  for (const p of G.asks.keys()) if (!keep.has(p)) G.asks.delete(p);
  wire(keep);
}

// A page's links as it writes them, through the reader's own renderer (a
// link in code is none; a picture shown is none), and its title.
function asksIn(text, path) {
  const asks = [];
  const doc = render(withoutFrontMatter(text), {
    resolve: (target, how) => {
      if (asks.length < GRAPH_LINKS_MAX) asks.push([target, how]);
      return null;
    },
    image: (src, how) => picture(src, how, folder(path), path),
  });
  const title = doc.querySelector("h1")?.textContent.trim().slice(0, 120) ?? "";
  // an index's heading is its folder's name: the folder says more
  return { asks, title: /^(index|readme)\.md$/i.test(base(path)) && path.includes("/") ? "" : title };
}

// The pages `keep`, and the files they link to, as nodes; their links
// resolved as the reader resolves them, against the files as they are now.
function wire(keep) {
  const links = new Map();
  for (const p of keep) {
    const at = folder(p);
    const out = new Set();
    for (const [target, how] of G.asks.get(p)?.asks ?? []) {
      const to = how === "wiki" ? wikiTarget(target, at, p) : destination(target, at, p);
      if (to?.path && to.path !== p && index.has(to.path)) out.add(to.path);
    }
    links.set(p, out);
  }
  const want = new Set(keep);
  for (const out of links.values()) for (const q of out) want.add(q);
  G.tops = [...new Set([...want].map(topOf))].sort((a, b) => (a !== "") - (b !== "") || a.localeCompare(b, undefined, { numeric: true }));
  for (const [p, node] of G.nodes) {
    if (want.has(p)) continue;
    node.g.remove();
    G.nodes.delete(p);
    if (G.sel === node) G.sel = null;
    if (G.hover === node) G.hover = null;
  }
  const fresh = [];
  for (const p of want) {
    if (G.nodes.has(p)) continue;
    const node = makeNode(p);
    G.nodes.set(p, node);
    fresh.push(node);
  }
  for (const node of G.nodes.values()) {
    node.out = links.get(node.path) ?? new Set();
    node.in = new Set();
  }
  for (const [p, out] of links) for (const q of out) G.nodes.get(q).in.add(p);
  // one line per linked pair, a link each way included
  const before = new Set(G.edges.map((e) => pair(e.a.path, e.b.path)));
  const pairs = new Map();
  for (const [p, out] of links) for (const q of out) pairs.set(pair(p, q), [p, q]);
  edgesG.replaceChildren();
  G.edges = [...pairs.values()].map(([p, q]) => {
    const line = svgEl("line", { class: "edge" });
    edgesG.append(line);
    return { a: G.nodes.get(p), b: G.nodes.get(q), line };
  });
  const moved = fresh.length > 0 || pairs.size !== before.size || [...pairs.keys()].some((k) => !before.has(k));
  const reach = 60 + 26 * Math.sqrt(G.nodes.size);
  const ring = G.tops.filter((t) => t !== "");
  for (const node of G.nodes.values()) {
    node.tone = node.top === "" ? "tone-root" : `tone-${ring.indexOf(node.top) % TONES}`;
    node.g.setAttribute("class", `node ${node.tone}${/^(index|readme)\.md$/i.test(base(node.path)) ? " hub" : ""}`);
    node.deg = node.in.size + node.out.size;
    node.label.textContent = titleOf(node.path);
    node.r = Math.min(4 + Math.sqrt(node.deg) * 2, 13);
    node.dot.setAttribute("r", node.r.toFixed(1));
    node.label.setAttribute("y", (node.r + 3).toFixed(1));
    const a = ring.length > 1 && node.top !== "" ? (2 * Math.PI * ring.indexOf(node.top)) / ring.length - Math.PI / 2 : null;
    node.anchor = a === null ? { x: 0, y: 0 } : { x: Math.cos(a) * reach, y: Math.sin(a) * reach };
  }
  for (const node of fresh) seed(node, reach);
  G.named = new Set([...G.nodes.values()].sort((a, b) => b.deg - a.deg || a.path.localeCompare(b.path)).slice(0, GRAPH_NAMED));
  G.built = true;
  chips();
  filter();
  focus();
  names();
  draw();
  if (moved) kick(fresh.length === G.nodes.size ? 1 : 0.4);
  sayGraph(G.nodes.size ? "" : "No pages yet: each markdown page is drawn here, with lines for its links.");
  $("ghint").textContent = G.left ? `${G.left} ${G.left === 1 ? "page" : "pages"} not drawn (past ${GRAPH_PAGES_MAX}, or over ${bytes(PREFETCH_MAX)})` : "";
  if (mode === "graph") {
    graphBar();
    if (G.sel) peek(G.sel);
  } else if (moved && drawn && drawn.path === current && !searching) redraw();
}
const pair = (p, q) => (p < q ? `${p}\n${q}` : `${q}\n${p}`);

function makeNode(path) {
  const g = svgEl("g", { class: "node", "data-path": path });
  const dot = svgEl("circle", { r: 5 });
  const label = svgEl("text", { class: "label", "text-anchor": "middle", "dominant-baseline": "hanging" });
  label.textContent = titleOf(path);
  const tip = svgEl("title");
  tip.textContent = path;
  g.append(dot, label, tip);
  nodesG.append(g);
  return { path, top: topOf(path), x: 0, y: 0, vx: 0, vy: 0, g, dot, label, out: new Set(), in: new Set(), deg: 0, r: 5, tone: "", anchor: { x: 0, y: 0 }, off: false, placed: false };
}

// A new page's first place: beside a page it links with, else about its
// top folder's place, where its path puts it.
function seed(node, reach) {
  const u = (hash(node.path) & 0xffff) / 65536;
  const v = (hash(node.path) >>> 16) / 65536;
  const near = [...node.out, ...node.in].map((p) => G.nodes.get(p)).find((n) => n?.placed);
  const at = near ?? node.anchor;
  const spread = near ? 40 : reach * 0.7;
  node.x = at.x + (u - 0.5) * spread;
  node.y = at.y + (v - 0.5) * spread;
  node.vx = node.vy = 0;
  node.placed = true;
}

function tick() {
  const a = G.alpha;
  const nodes = [...G.nodes.values()];
  const grid = new Map();
  const cellOf = (x, y) => `${Math.floor(x / CELL)},${Math.floor(y / CELL)}`;
  for (const n of nodes) {
    const key = cellOf(n.x, n.y);
    if (!grid.has(key)) grid.set(key, []);
    grid.get(key).push(n);
  }
  for (const n of nodes) {
    const cx = Math.floor(n.x / CELL);
    const cy = Math.floor(n.y / CELL);
    for (let i = -1; i <= 1; i++) {
      for (let j = -1; j <= 1; j++) {
        for (const m of grid.get(`${cx + i},${cy + j}`) ?? []) {
          if (m === n) continue;
          let dx = n.x - m.x;
          let dy = n.y - m.y;
          let d2 = dx * dx + dy * dy;
          if (d2 > CELL * CELL) continue;
          if (d2 < 1) {
            // two in one place: apart along their paths' order
            dx = n.path < m.path ? -1 : 1;
            dy = 0.5;
            d2 = 1.25;
          }
          const d = Math.sqrt(d2);
          const f = (REPEL * a) / d2;
          n.vx += (dx / d) * f;
          n.vy += (dy / d) * f;
        }
      }
    }
  }
  for (const { a: p, b: q } of G.edges) {
    const dx = q.x - p.x;
    const dy = q.y - p.y;
    const d = Math.hypot(dx, dy) || 1;
    // a link across folders pulls less: each folder keeps its place
    const f = ((d - LINK) * SPRING * a * (p.top === q.top ? 1 : 0.5)) / d;
    p.vx += dx * f;
    p.vy += dy * f;
    q.vx -= dx * f;
    q.vy -= dy * f;
  }
  const held = drag?.moved ? drag.node : null;
  for (const n of nodes) {
    if (n === held) {
      n.vx = n.vy = 0;
      continue;
    }
    n.vx = (n.vx + (n.anchor.x - n.x) * PULL * a) * 0.6;
    n.vy = (n.vy + (n.anchor.y - n.y) * PULL * a) * 0.6;
    n.x += n.vx;
    n.y += n.vy;
  }
  G.alpha *= DECAY;
}

function loop() {
  G.frame = null;
  const t = performance.now();
  do tick();
  while (G.alpha > ALPHA_MIN && performance.now() - t < FRAME_MS);
  draw();
  if (G.auto) fit();
  if (G.alpha > ALPHA_MIN && mode === "graph") G.frame = requestAnimationFrame(loop);
}

// Sets the layout moving again (by `alpha`, or as it was), while shown.
function kick(alpha) {
  if (alpha) G.alpha = Math.max(G.alpha, alpha);
  if (!G.frame && G.alpha > ALPHA_MIN && mode === "graph" && G.nodes.size) G.frame = requestAnimationFrame(loop);
}

function draw() {
  for (const n of G.nodes.values()) n.g.setAttribute("transform", `translate(${n.x.toFixed(1)} ${n.y.toFixed(1)})`);
  for (const { a, b, line } of G.edges) {
    line.setAttribute("x1", a.x.toFixed(1));
    line.setAttribute("y1", a.y.toFixed(1));
    line.setAttribute("x2", b.x.toFixed(1));
    line.setAttribute("y2", b.y.toFixed(1));
  }
}

function apply() {
  viewG.setAttribute("transform", `translate(${G.tx.toFixed(1)} ${G.ty.toFixed(1)}) scale(${G.k.toFixed(4)})`);
  svg.style.setProperty("--k", G.k.toFixed(4));
  names();
}

// The pages shown, all in the window.
function fit() {
  const shown = [...G.nodes.values()].filter((n) => !n.off);
  const w = svg.clientWidth;
  const ht = svg.clientHeight;
  if (!shown.length || !w || !ht) return;
  let x0 = Infinity;
  let x1 = -Infinity;
  let y0 = Infinity;
  let y1 = -Infinity;
  for (const n of shown) {
    x0 = Math.min(x0, n.x - n.r);
    x1 = Math.max(x1, n.x + n.r);
    y0 = Math.min(y0, n.y - n.r);
    y1 = Math.max(y1, n.y + n.r + 14);
  }
  const pad = 48;
  G.k = clamp(Math.min((w - 2 * pad) / Math.max(x1 - x0, 1), (ht - 2 * pad) / Math.max(y1 - y0, 1)), ZOOM_MIN, 2);
  G.tx = w / 2 - (G.k * (x0 + x1)) / 2;
  G.ty = ht / 2 - (G.k * (y0 + y1)) / 2;
  apply();
}

function zoomAt(cx, cy, by) {
  const k = clamp(G.k * by, ZOOM_MIN, ZOOM_MAX);
  G.tx = cx - (cx - G.tx) * (k / G.k);
  G.ty = cy - (cy - G.ty) * (k / G.k);
  G.k = k;
  G.auto = false;
  apply();
}

// A page in the middle of what the graph shows: beside the chosen page's
// card, or above its sheet, when it lies over the graph (narrower windows).
function centerOn(node) {
  G.auto = false;
  const box = svg.getBoundingClientRect();
  let w = box.width;
  let ht = box.height;
  const card = $("peek");
  if (!card.hidden && getComputedStyle(card).position === "absolute") {
    const over = card.getBoundingClientRect();
    if (over.left - box.left > box.width / 3) w = over.left - box.left;
    else ht = Math.max(over.top - box.top, 80);
  }
  G.tx = w / 2 - G.k * node.x;
  G.ty = ht / 2 - G.k * node.y;
  apply();
}
const overlaid = () => !$("peek").hidden && getComputedStyle($("peek")).position === "absolute";

// Names: every page's when few or close; else the most linked.
function names() {
  const all = G.nodes.size <= GRAPH_ALL_NAMED || G.k >= NAMES_ZOOM;
  for (const n of G.nodes.values()) n.g.classList.toggle("named", all || G.named.has(n));
}

// The page pointed at (else the one chosen), its links and theirs lit.
function focus() {
  const f = G.hover ?? G.sel;
  svg.classList.toggle("focused", !!f);
  for (const n of G.nodes.values()) {
    n.g.classList.toggle("near", !!f && (n === f || f.out.has(n.path) || f.in.has(n.path)));
    n.g.classList.toggle("chosen", n === G.sel);
  }
  for (const e of G.edges) e.line.classList.toggle("near", !!f && (e.a === f || e.b === f));
}

// What is shown: its folders, unlinked pages or not, and the chosen page's links alone.
function filter() {
  const s = G.sel;
  for (const n of G.nodes.values()) {
    n.off = G.hidden.has(n.top) || (!G.orphans && n.deg === 0) || (G.local && !!s && n !== s && !s.out.has(n.path) && !s.in.has(n.path));
    n.g.classList.toggle("off", n.off);
  }
  for (const e of G.edges) e.line.classList.toggle("off", e.a.off || e.b.off);
  $("orphans").setAttribute("aria-pressed", String(G.orphans));
  $("local").setAttribute("aria-pressed", String(G.local && !!s));
  $("local").disabled = !s;
  document.getElementById("peek-local")?.setAttribute("aria-pressed", String(G.local && !!s));
}

function chips() {
  const ring = G.tops.filter((t) => t !== "");
  $("folders").replaceChildren(
    ...G.tops.map((t) =>
      h("button", { class: "chip", type: "button", "data-top": t, "aria-pressed": String(!G.hidden.has(t)), title: t ? `Show or hide ${t}/` : "Show or hide the files at the top" },
        h("span", { class: `dot ${t === "" ? "tone-root" : `tone-${ring.indexOf(t) % TONES}`}` }),
        el("span", null, t || "top"),
      ),
    ),
  );
}

// Chooses a page (null: none): lit, marked in the tree, previewed.
function choose(path, center) {
  const node = path ? G.nodes.get(path) ?? null : null;
  G.sel = node;
  if (!node) G.local = false;
  if (mode === "graph" && graphRoute(location.hash) !== (node?.path ?? null)) history.replaceState(history.state, "", graphHash(node?.path));
  filter();
  focus();
  mark();
  peek(node);
  if (G.local) fit();
  else if (node && (center || overlaid())) centerOn(node);
}

function toggleLocal() {
  if (!G.sel) return;
  G.local = !G.local;
  filter();
  if (G.local) fit();
}

// The chosen page beside the graph: its first lines, as the reader draws
// them, what it links to and what links to it.
let peeking = 0;
async function peek(node) {
  const n = ++peeking;
  const box = $("peek");
  box.hidden = !node || mode !== "graph";
  if (!node) return box.replaceChildren();
  const path = node.path;
  const heading = el("h2", "peek-title", titleOf(path));
  const text = el("div", "doc peek-text");
  const list = (label, paths) => {
    const sorted = [...paths].sort((a, b) => titleOf(a).localeCompare(titleOf(b), undefined, { numeric: true }));
    const box = h("section", { class: "peek-list" }, h("h3", {}, el("span", null, label), el("span", "count", String(sorted.length))));
    if (!sorted.length) box.append(el("p", "none", "None"));
    for (const p of sorted) {
      box.append(h("button", { class: "peek-row", type: "button", "data-path": p, title: p }, h("span", { class: `dot ${G.nodes.get(p)?.tone ?? ""}` }), el("span", "name", titleOf(p)), el("span", "where", folder(p) || "/")));
    }
    return box;
  };
  const only = h("button", { class: "chip toggle", id: "peek-local", type: "button", "aria-pressed": String(G.local) }, icon("focus"), "Its links only");
  only.onclick = toggleLocal;
  const close = h("button", { class: "icon", type: "button", title: "Close", "aria-label": "Close" }, icon("close"));
  close.onclick = () => choose(null);
  box.replaceChildren(
    h("div", { class: "peek-head" }, el("span", "peek-path", path), close),
    heading,
    text,
    h("div", { class: "peek-acts" }, h("a", { class: "button", href: route(path) }, icon("page"), "Open"), only),
    list("Links to", node.out),
    list("Linked from", node.in),
  );
  if (kind(path) !== "markdown") return text.append(el("p", null, bytes(index.get(path)?.size ?? 0)));
  const read = await take(path).catch(() => null);
  if (n !== peeking || typeof read !== "string") return;
  const doc = markdown(read, path);
  const h1 = doc.querySelector("h1");
  if (h1?.textContent.trim()) heading.textContent = h1.textContent.trim();
  let chars = 0;
  for (const block of [...doc.children]) {
    if (chars >= PEEK_CHARS) break;
    if (!["P", "UL", "OL", "BLOCKQUOTE"].includes(block.tagName)) continue;
    chars += block.textContent.length;
    text.append(block);
  }
}

// pointing, choosing, dragging, panning and zooming
let drag = null;
const nodeAt = (target) => G.nodes.get(target.closest?.(".node")?.dataset.path) ?? null;
svg.addEventListener("pointerdown", (e) => {
  if (e.button !== 0) return;
  drag = { id: e.pointerId, x: e.clientX, y: e.clientY, tx: G.tx, ty: G.ty, node: nodeAt(e.target), moved: false, box: svg.getBoundingClientRect() };
  svg.setPointerCapture(e.pointerId);
});
svg.addEventListener("pointermove", (e) => {
  if (!drag) {
    const node = nodeAt(e.target);
    if (node !== G.hover) {
      G.hover = node;
      focus();
    }
    return;
  }
  if (e.pointerId !== drag.id) return;
  const dx = e.clientX - drag.x;
  const dy = e.clientY - drag.y;
  if (!drag.moved && Math.hypot(dx, dy) < 4) return;
  drag.moved = true;
  G.auto = false;
  if (drag.node) {
    drag.node.x = (e.clientX - drag.box.left - G.tx) / G.k;
    drag.node.y = (e.clientY - drag.box.top - G.ty) / G.k;
    kick(0.25);
    draw();
  } else {
    G.tx = drag.tx + dx;
    G.ty = drag.ty + dy;
    apply();
  }
});
svg.addEventListener("pointerup", (e) => {
  if (!drag || e.pointerId !== drag.id) return;
  const { node, moved } = drag;
  drag = null;
  if (!moved) choose(node?.path ?? null);
});
svg.addEventListener("pointercancel", () => {
  drag = null;
});
svg.addEventListener("pointerleave", () => {
  if (!drag && G.hover) {
    G.hover = null;
    focus();
  }
});
svg.addEventListener("dblclick", (e) => {
  const node = nodeAt(e.target);
  if (node) location.hash = route(node.path);
});
svg.addEventListener("wheel", (e) => {
  e.preventDefault();
  const box = svg.getBoundingClientRect();
  zoomAt(e.clientX - box.left, e.clientY - box.top, Math.exp(-e.deltaY * (e.ctrlKey ? 0.01 : 0.0015)));
}, { passive: false });
new ResizeObserver(() => G.auto && mode === "graph" && fit()).observe($("gcanvas"));

$("zoom-in").onclick = () => zoomAt(svg.clientWidth / 2, svg.clientHeight / 2, 1.3);
$("zoom-out").onclick = () => zoomAt(svg.clientWidth / 2, svg.clientHeight / 2, 1 / 1.3);
$("zoom-fit").onclick = () => {
  G.auto = true;
  fit();
};
$("orphans").onclick = () => {
  G.orphans = !G.orphans;
  filter();
};
$("local").onclick = toggleLocal;
$("folders").addEventListener("click", (e) => {
  const chip = e.target.closest(".chip");
  if (!chip) return;
  const t = chip.dataset.top;
  if (!G.hidden.delete(t)) G.hidden.add(t);
  chip.setAttribute("aria-pressed", String(!G.hidden.has(t)));
  filter();
});
$("peek").addEventListener("click", (e) => {
  const row = e.target.closest(".peek-row");
  if (row) choose(row.dataset.path, true);
});
$("peek").addEventListener("dblclick", (e) => {
  const row = e.target.closest(".peek-row");
  if (row) location.hash = route(row.dataset.path);
});
addEventListener("keydown", (e) => {
  if (e.key === "Escape" && mode === "graph" && G.sel && !e.target.closest?.("input, textarea")) choose(null);
});
// the graph with the page read chosen; the page chosen (else the one read last, else the first)
$("to-graph").onclick = () => {
  if (mode !== "graph") location.hash = graphHash(current);
};
$("to-page").onclick = async () => {
  if (mode === "page") return;
  const path = G.sel?.path ?? current ?? (await first());
  if (path) location.hash = route(path);
};

// ---- start ----

try {
  await load();
  // a small fragment's top folders start open
  if (files.length <= OPEN_FOLDERS_MAX) for (const { path } of files) if (path.includes("/")) opened.add(path.slice(0, path.indexOf("/")));
} catch (e) {
  $("tree").replaceChildren(el("div", "none", `The files could not be listed: ${e.message}`));
}
addEventListener("hashchange", go);
go();
status();
watch();
// a fragment that opens on its graph draws it now, whatever page is asked
// for first: the reader says what links to a page from it
if (graphFirst) {
  G.wanted = true;
  graphNow();
}
