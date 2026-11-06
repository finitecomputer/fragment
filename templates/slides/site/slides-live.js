// The deck's live layer, beside open-slide's own page (scripts/build.mjs
// adds it): who is watching, presenting to everyone, and notes on each page.
//
// The page open-slide shows is its URL's `?p=` (1 for the first; present
// mode follows it too), so presenting is posting that page to the `present`
// channel, which editors and up may post to (fragment.json), and following
// is setting it. A presenter is live while their page's presence says
// `presenting`: a closed tab ends it. The newest live presenter is the one
// followed; a viewer may stop following, and follows again when someone
// else starts. Notes are applib/notes.mjs's operations, each on its page
// (`p3`), followed with `fragment.live`.
import * as fragment from "./__fragment.js";

const NOTE_MAX_CHARS = 2000;
const PROFILES_PER_ASK = 64;

// ---- small DOM helper: text always goes in as text
function h(tag, attrs = {}, ...kids) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v === false || v == null) continue;
    if (k === "class") el.className = v;
    else if (k.startsWith("on")) el.addEventListener(k.slice(2), v);
    else el.setAttribute(k, v === true ? "" : v);
  }
  for (const kid of kids.flat()) if (kid != null && kid !== false) el.append(kid);
  return el;
}

// ---- the page open-slide shows
function page() {
  const p = Number(new URLSearchParams(location.search).get("p") ?? "1");
  return Number.isInteger(p) && p >= 1 ? p : 1;
}
function goTo(p) {
  const url = new URL(location.href);
  url.searchParams.set("p", String(p));
  if (url.href === location.href) return;
  history.replaceState(history.state, "", url);
  // open-slide's router reads the URL again on popstate
  dispatchEvent(new PopStateEvent("popstate", { state: history.state }));
}

// ---- who: names from the fragment's `__people`, in batches
const profiles = new Map();
const asking = new Set();
let askTimer = 0;
function want(principal) {
  if (typeof principal !== "string" || !principal.startsWith("npub1") || profiles.has(principal)) return;
  profiles.set(principal, null);
  asking.add(principal);
  if (!askTimer) askTimer = setTimeout(ask, 20);
}
async function ask() {
  askTimer = 0;
  const ids = [...asking];
  asking.clear();
  for (let i = 0; i < ids.length; i += PROFILES_PER_ASK) {
    const batch = ids.slice(i, i + PROFILES_PER_ASK);
    let found = {};
    try {
      const r = await fetch(`/__people?${batch.map((id) => `id=${encodeURIComponent(id)}`).join("&")}`, { credentials: "same-origin" });
      if (r.ok) found = (await r.json()).profiles ?? {};
    } catch {
      // shown by their npub's end until the page loads again
    }
    for (const id of batch) profiles.set(id, found[id] ?? {});
  }
  render();
}
function nameOf(principal) {
  if (principal && principal === me?.principal) return "You";
  if (typeof principal !== "string" || !principal.startsWith("npub1")) return "A visitor";
  want(principal);
  const p = profiles.get(principal);
  return p?.title ?? p?.name ?? p?.email ?? `npub…${principal.slice(-6)}`;
}

// ---- state
let me = null; // { id, principal, role } once the socket says hello
let people = []; // presence: [{id, principal, data}]
let latest = null; // the newest `present` record
let presenting = false;
let following = true;
let followedFrom = null; // the presenter's socket this page last followed
let shown = page();
let notes = [];
let notesError = null;
let open = false; // the notes panel
let replyingTo = null;
let draft = "";
let failure = null; // what the last action could not do

const editor = () => me?.role === "editor" || me?.role === "owner";
const signedIn = () => typeof me?.principal === "string" && me.principal.startsWith("npub1");
const mayChange = (note) => me !== null && (me.role === "owner" || note.author === me.principal);

// The presenter this page would follow: the newest live record, from a page
// that is still here and presenting, and not this one.
function presenter() {
  const r = latest;
  if (!r?.body?.live || r.body.from === me?.id) return null;
  const there = people.find((p) => p.id === r.body.from);
  return there?.data?.presenting ? r : null;
}

async function share(live) {
  const at = await fragment.me();
  try {
    await fragment.post("present", { page: page(), live, from: at.id });
    failure = null;
  } catch {
    failure = live ? "Your page didn't reach everyone. It will on the next page." : "Couldn't stop presenting. Try again.";
  }
  render();
}

function startPresenting() {
  presenting = true;
  fragment.presence.set({ presenting: true });
  share(true);
}
function stopPresenting() {
  presenting = false;
  fragment.presence.set({});
  share(false);
}

function follow() {
  const r = presenter();
  if (r && r.body.from !== followedFrom) {
    // someone started presenting: follow them, and stop presenting here
    followedFrom = r.body.from;
    following = true;
    if (presenting) {
      presenting = false;
      fragment.presence.set({});
    }
  }
  if (r && following && !presenting && Number.isInteger(r.body.page) && r.body.page !== page()) goTo(r.body.page);
}

function onPage() {
  const p = page();
  if (p === shown) return;
  shown = p;
  replyingTo = null;
  if (presenting) share(true);
  render();
}

// ---- what this layer draws: a bar, and the notes panel above it
const bar = h("div", { class: "sl-bar", role: "region", "aria-label": "Live" });
const panel = h("aside", { class: "sl-panel", hidden: true, "aria-label": "Notes" });

function button(label, onclick, cls = "") {
  return h("button", { type: "button", class: `sl-btn ${cls}`.trim(), onclick }, label);
}

function renderBar() {
  const here = new Set(people.map((p) => p.principal)).size;
  const others = people.filter((p) => p.id !== me?.id).length;
  const r = presenter();
  const parts = [];
  if (presenting) {
    parts.push(h("span", { class: "sl-live" }, "Live"), h("span", { class: "sl-text" }, `${others} watching`), button("Stop", stopPresenting));
  } else {
    if (here > 0) parts.push(h("span", { class: "sl-text", title: [...new Set(people.map((p) => nameOf(p.principal)))].join(", ") }, `${here} here`));
    if (r && following) parts.push(h("span", { class: "sl-text" }, `Following ${nameOf(r.principal)}`), button("Stop following", () => { following = false; render(); }));
    else if (r) parts.push(button(`Follow ${nameOf(r.principal)}`, () => { following = true; follow(); render(); }));
    else if (editor()) parts.push(button("Present live", startPresenting));
  }
  const open_ = roots().filter((n) => !n.resolved).length;
  parts.push(button(open_ ? `Notes · ${open_}` : "Notes", () => { open = !open; render(); }, open ? "sl-on" : ""));
  bar.replaceChildren(...parts);
}

function roots() {
  return notes.filter((n) => n.anchor === `p${page()}` && n.reply_to == null);
}

async function act(op, input) {
  try {
    await fragment.call(op, input);
    failure = null;
    return true;
  } catch (e) {
    failure = e?.message || "That didn't work. Try again.";
    render();
    return false;
  }
}

function composer(replyTo) {
  if (!signedIn()) {
    const back = `${location.pathname}${location.search}`;
    return h("a", { class: "sl-signin", href: `/__signin?return=${encodeURIComponent(back)}` }, "Sign in to leave a note");
  }
  const area = h("textarea", { class: "sl-input", rows: "3", maxlength: String(NOTE_MAX_CHARS), placeholder: replyTo ? "Reply" : `A note on page ${page()}` });
  area.value = draft;
  area.addEventListener("input", () => { draft = area.value; });
  const send = async (e) => {
    e.preventDefault();
    const text = area.value.trim();
    if (!text) return;
    const input = replyTo ? { reply_to: replyTo, text } : { anchor: `p${page()}`, text };
    if (await act("annotate", input)) {
      draft = "";
      replyingTo = null;
      render();
    }
  };
  return h("form", { class: "sl-compose", onsubmit: send }, area, h("button", { type: "submit", class: "sl-btn sl-primary" }, replyTo ? "Reply" : "Add note"));
}

function thread(root) {
  const replies = notes.filter((n) => n.reply_to === root.id);
  const line = (n) => h("div", { class: "sl-note" }, h("div", { class: "sl-who" }, nameOf(n.author)), h("div", { class: "sl-say" }, n.text));
  const actions = [];
  if (!root.resolved && signedIn()) actions.push(button("Reply", () => { replyingTo = root.id; draft = ""; render(); }, "sl-quiet"));
  if (mayChange(root)) actions.push(button(root.resolved ? "Reopen" : "Resolve", () => act("resolve", { id: root.id, resolved: !root.resolved }), "sl-quiet"));
  if (root.resolved) {
    return h("div", { class: "sl-thread sl-resolved", "data-thread": String(root.id) }, h("div", { class: "sl-say" }, `Resolved: ${root.text}`), h("div", { class: "sl-actions" }, actions));
  }
  return h(
    "div",
    { class: "sl-thread", "data-thread": String(root.id) },
    line(root),
    replies.map(line),
    replyingTo === root.id ? composer(root.id) : h("div", { class: "sl-actions" }, actions),
  );
}

function renderPanel() {
  panel.hidden = !open;
  if (!open) return;
  const list = roots();
  const focused = document.activeElement?.closest?.(".sl-panel") ? document.activeElement : null;
  if (focused?.tagName === "TEXTAREA") return; // never redraw under someone typing
  // through h: replaceChildren would write a null or an array as text
  const parts = h(
    "div",
    {},
    h("div", { class: "sl-head" }, h("span", {}, `Page ${page()}`), button("Close", () => { open = false; render(); }, "sl-quiet")),
    notesError ? h("p", { class: "sl-text" }, "Notes didn't load. They'll come back when the page reconnects.") : null,
    list.length ? list.map(thread) : h("p", { class: "sl-empty" }, "No notes on this page."),
    replyingTo === null ? composer(null) : null,
  );
  panel.replaceChildren(...parts.childNodes);
}

function render() {
  renderBar();
  renderPanel();
  bar.querySelector(".sl-error")?.remove();
  if (failure) bar.append(h("span", { class: "sl-error", role: "status" }, failure));
}

function start() {
  // the presenter's own view (open-slide's /presenter window) keeps to itself
  if (location.pathname.endsWith("/presenter")) return;
  document.body.append(panel, bar);
  if (window.navigation) navigation.addEventListener("currententrychange", onPage);
  addEventListener("popstate", onPage);
  // where there is no Navigation API: open-slide moves on a key, a click or the wheel
  for (const ev of ["keyup", "pointerup", "wheel"]) addEventListener(ev, () => requestAnimationFrame(onPage), { passive: true, capture: true });

  fragment.me().then((m) => { me = m; render(); });
  fragment.presence.on((list) => {
    people = list;
    for (const p of list) want(p.principal);
    // a reconnected presenter's socket is new: say where it is again
    if (presenting) fragment.me().then((m) => { me = m; if (latest?.body?.from !== m.id) share(true); });
    follow();
    render();
  });
  fragment.presence.set({});
  fragment.subscribe("present", (rec) => { latest = rec; want(rec.principal); follow(); render(); }, { last: 1 });
  fragment.live("notes", {}, (r) => { notes = r.notes; notesError = null; for (const n of notes) want(n.author); render(); }, (err) => { notesError = err; render(); });
  render();
}

start();
