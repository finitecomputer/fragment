// The shell: the platform's one page (docs/cloudflare-v1.md, decisions
// 6–12). Its sidebar is the person's fragments by kind: their chats (a
// direct chat with one agent, in its colour, or a group of several, their
// colours stacked) and their apps, less the ones they archived. The open
// chat fills the middle column and apps open as windows in the viewer;
// each is a frame of the fragment's own origin, signed in there by the
// platform's frame mint (`/auth/frame`), for this page only. This page
// holds no key: it calls the API with the person's platform session
// (`x-fragment-shell`, same-origin only). It knows nothing of Hermes:
// everything in a frame is a fragment. Design: Skyler's handoff
// (2026-10-02); what it did not draw is the older prototype's, in its look.
import "./tooltips.js";
import { avatar } from "./agent-identity.js";
import { colorOf } from "./agent-color.js";
import { icon } from "./icons.js";
import { appArt, openIconSheet } from "./app-icon.js";
import { createAppsPage } from "./apps-page.js";
import { createTitles } from "./fragment-title.js";
import { createLayout, store, reducedMotion, springFrames } from "./layout.js";
import { createViewer } from "./viewer.js";
import { seatGives } from "./billing.js";
import { createChatList, newestChat } from "./chat-list.js";
import { createSessionRefresh } from "./session-refresh.js";
import { createOnboarding } from "./onboarding.js";
import { createComputer } from "./computer.js";
import { createAgentSettings } from "./agent-settings.js";
import { createConnections, createAgentHomes, createSimplex } from "./connections.js";
import { el, section, line, actionLine, createSettings } from "./settings.js";
import { createLauncher } from "./launcher.js";
import { createThreads } from "./threads.js";

const $ = (id) => document.getElementById(id);
const CURRENT = "shell.chat.v1";
// How to ask an agent for an app (Paul, 2026-10-14: "rather than spawning
// generic apps we teach the user how to prompt for new apps"): each says
// who it is for and what it does, `…` the part to make their own. The
// dock's Add and an empty desktop show three of them at a time, and a die
// deals three more (`promptDeck`); pressed, one goes into their chat's
// composer, theirs to change and send (`composeApp`). Suggestions, not
// data: nothing is made or sent, and nothing pretends to exist. Paul's
// twelve (2026-10-16), in his order; change them here, in threes.
const APP_PROMPTS = [
  "Make a shared grocery list my family can add to from their phones",
  "Tell me on my phone as soon as … comes back in stock or drops in price",
  "Every morning, read these sites and send me a short summary: …",
  "Make a page for our trip to … where we log who paid for what and it shows who owes whom",
  "Build a habit tracker that shows my weekly streak for: …",
  "Make an RSVP page for my dinner on Saturday that tells me who's coming",
  "Make a reading list where I paste a link and it keeps the title and a one-line summary",
  "Build a chore chart for our house that rotates the jobs each week between: …",
  "Make a page where my team votes on … and everyone sees the results live",
  "Make a page where friends pick which evenings work for …, and it shows the best night",
  "Make a status board that pings me when a build or deploy fails for …",
  "Make a trip planner for … that my friends can add ideas and dates to",
];
// The ways to ask on screen, three of APP_PROMPTS' (Paul, 2026-10-16: "we
// shouldn't overwhelm people with prompts. we could even just show three
// at a time, and then have a 'dice roll' button that reveals a fresh set
// of 3"): one hand a page load, wherever it shows, the first three first.
// The die deals the next three off a deck of the rest, shuffled; dealt
// out, the next deck is every one shuffled again, none of the hand on
// screen in its first three. So a deal never repeats what shows, and every
// way shows once before any shows again. Not kept: a reload deals 1–3.
const PROMPT_HAND = 3;
if (APP_PROMPTS.length < 2 * PROMPT_HAND || APP_PROMPTS.length % PROMPT_HAND) throw new Error(`APP_PROMPTS holds whole hands of ${PROMPT_HAND}, at least two: ${APP_PROMPTS.length}`);
const promptDeck = { hand: APP_PROMPTS.map((_, i) => i).slice(0, PROMPT_HAND), deck: shuffled(APP_PROMPTS.map((_, i) => i).slice(PROMPT_HAND)) };

// me: the signed-in person; guest: whether they are a guest (`GET /api/seat`), who
// makes nothing (decision 49); fragments: their list (name, role, kind,
// title, agents, preview, sharing, archived: a chat's agents the lead
// first, and its newest message); computer: theirs, with its agents;
// agents: by identity; computerFailed: whether the last read of their computer
// failed (what an earlier one said stays: a failure is not "none");
// invites: those waiting on their emails, theirs to answer (`GET /api/invites`);
// seat: theirs, as Settings last read it (`GET /api/seat`'s; none unread)
const state = { me: null, guest: false, fragments: [], invites: [], computer: null, agents: new Map(), computerFailed: false, current: store.get(CURRENT, null), frames: new Map(), page: null, seat: null };

// ---- the API, as this page calls it: the person's platform session ----
class ApiError extends Error {
  constructor(status, code, message, reason) {
    super(message);
    this.status = status;
    this.code = code;
    // why, where the platform names it (billing's refusals: docs/api.md)
    this.reason = reason ?? null;
  }
}
// this page's writes still going: a reload for a new release waits for them
let writing = 0;
async function api(method, path, body) {
  const headers = { "x-fragment-shell": "1" };
  if (body !== undefined) headers["content-type"] = "application/json";
  const writes = method !== "GET";
  if (writes) writing++;
  try {
    const r = await fetch(path, { method, headers, body: body === undefined ? undefined : JSON.stringify(body), credentials: "same-origin" });
    heard(r);
    const out = await r.json().catch(() => ({}));
    if (!r.ok) throw new ApiError(r.status, out.error, out.message || r.statusText, out.reason);
    return out;
  } finally {
    if (writes) writing--;
  }
}

const byName = (name) => state.fragments.find((f) => f.name === name);
// a name is its label and a random suffix (decision 47): `todo--k3x9`
const labelOf = (name) => name.replace(/--[a-km-np-z2-9]{4}$/, "");
const own = (f) => f.role === "owner";
// A fragment's title: an agent's is its name, its agent fragment's alone
// (docs/api.md, "An agent's name and picture"); a chat's is its own,
// whoever is in it.
const titleOf = (name) => byName(name)?.title || labelOf(name);
// What names a fragment (an agent above all) is drawn through `named`: the
// node's text, or one of its attributes, made from the fragment's title as
// the list says now (`make`), and made again at every change of the list
// (`renderNames`, from `load`), wherever the node is (a sheet, a pane, a
// dialog). So no copy of a name outlives a rename, whatever draws it once.
// Its fragment's name stays on the node (`data-named`), for a reader.
const namers = new WeakMap();
function named(node, fragment, make = (t) => t, attr = "text") {
  const each = namers.get(node) ?? new Map();
  each.set(attr, { fragment, make });
  namers.set(node, each);
  node.dataset.named = fragment;
  nameNode(node, attr, each.get(attr));
  return node;
}
function nameNode(node, attr, { fragment, make }) {
  const said = make(titleOf(fragment));
  if (attr === "text") {
    if (node.textContent !== said) node.textContent = said;
  } else if (node.getAttribute(attr) !== said) node.setAttribute(attr, said);
}
function renderNames() {
  for (const node of document.querySelectorAll("[data-named]")) for (const [attr, n] of namers.get(node) ?? []) nameNode(node, attr, n);
  // a pane's title, whatever opened it: an app's (an agent's own page), its
  // files, an agent's screen or its SimpleX
  for (const pane of document.querySelectorAll("#stack .pane[data-key]")) {
    const of = pane.dataset.key.match(/^(?:app|tree|screen|simplex):(.+)$/)?.[1];
    const title = of && pane.querySelector(".pane-title");
    if (title && title.textContent !== titleOf(of)) title.textContent = titleOf(of);
  }
  for (const f of document.querySelectorAll("iframe[data-simplex]")) nameSimplex(f);
}
const chats = () => state.fragments.filter((f) => f.kind === "chat");
const isChat = (name) => byName(name)?.kind === "chat";
const apps = () => state.fragments.filter((f) => f.kind === "app" || f.kind === "brain");
// the sidebar's: what the person has not archived (search still finds the rest)
const shown = (list) => list.filter((f) => !f.archived);
// the dock's: their apps shown, less those they took off it (`dock: false`)
const docked = () => shown(apps()).filter((f) => f.dock !== false);
// a frame of one of the person's fragments, signed in there by the platform
// A path segment: the API's ids keep their ":" (`computer:…`) as
// sent, since the router matches segments as they are, not decoded.
const seg = (s) => encodeURIComponent(s).replace(/%3A/gi, ":");
const framed = (name, path = "/") => `/auth/frame?name=${encodeURIComponent(name)}&return=${encodeURIComponent(path)}`;
// An agent's face: its colour, by its identity (agent-color.js, the one
// its own page and its chats' pages choose by), and its picture (its agent
// fragment's avatar.png, as the person's list last said: an address on this
// platform), shown instead of its tinted image when it has one.
const agentFace = (fragment, id) => ({ color: colorOf(id), picture: byName(fragment)?.picture ?? null });
const faceOf = (id) => agentFace(state.agents.get(id)?.fragment, id);
// The identity of the agent a fragment is (`maple--k3x9`), or of a direct
// chat's (`maple-chat--p2m4`, labeled for it), while its row names no agents.
const agentOf = (name) => {
  const direct = labelOf(name).match(/^(.+)-chat$/)?.[1];
  return [...state.agents.values()].find((a) => a.fragment === name || labelOf(a.fragment) === direct)?.identity ?? null;
};
// A chat's agents, the lead first, as its row names them; a direct chat's
// agent while it names none.
const agentsOf = (name) => byName(name)?.agents ?? [agentOf(name)].filter(Boolean);
const isGroup = (name) => agentsOf(name).length >= 2;
// a chat's face: its title, its lead agent's face, and its agents' faces
const identity = (name) => {
  const faces = agentsOf(name).map(faceOf);
  return { title: titleOf(name), ...(faces[0] ?? faceOf(null)), faces, preview: byName(name)?.preview ?? "" };
};
// A chat's mark: its agent's avatar, or a group's agents' stacked (at most
// three, the lead first), as the chat's page shows them.
function mark(name, size = "") {
  const who = identity(name);
  if (!isGroup(name)) return avatar(who, size);
  const stack = el("span", `avatar-stack ${size}`);
  const faces = who.faces.slice(0, 3);
  stack.dataset.count = String(faces.length);
  stack.setAttribute("aria-hidden", "true");
  stack.append(...faces.map((face) => avatar(face)));
  return stack;
}

// A list's rows, put in place (`data-key` names each): a row the same as
// one shown keeps that element (its focus, its card, the scroll around
// it), a new one goes where it belongs, and one no longer listed goes.
function patch(box, rows) {
  const shown = new Map([...box.children].filter((e) => e.dataset.key).map((e) => [e.dataset.key, e]));
  const next = rows.map((row) => {
    const was = shown.get(row.dataset.key);
    return was?.isEqualNode(row) ? was : row;
  });
  next.forEach((row, i) => {
    if (box.children[i] !== row) box.insertBefore(row, box.children[i] ?? null);
  });
  while (box.children.length > next.length) box.lastElementChild.remove();
}

// The notice's generation: each notice shown, and each one taken away (a
// chat or settings opened, settings loaded again), is a new one. What
// finishes later says or takes away a notice only under the generation it
// began under, never over a newer notice or another page (#457 review).
let noticeGen = 0;
function hideNotice() {
  $("notice").hidden = true;
  return ++noticeGen;
}
// `text`: a line, or lines and elements; `action`: one, or several in a row;
// its generation
function notice(title, text, action = null) {
  const n = $("notice");
  n.replaceChildren(el("strong", null, title));
  if (text) n.append(...[text].flat().map((t) => (typeof t === "string" ? el("span", null, t) : t)));
  if (action) {
    const row = el("div", "notice-row");
    row.append(...[action].flat());
    n.append(row);
  }
  n.hidden = false;
  return ++noticeGen;
}

// ---- layout: sidebar | chat | viewer ----
const layout = createLayout({
  grid: $("layout"),
  gutters: [$("gutter-left"), $("gutter-right")],
  onChange: (shown) => { $("scrim").classList.toggle("on", $("layout").classList.contains("narrow") && (shown.left || shown.right)); },
});
const viewer = createViewer({
  stack: $("stack"),
  onChange: (keys) => {
    $("viewer").classList.toggle("is-empty", !keys.length);
    $("viewer-count").hidden = !keys.length;
    $("viewer-count").textContent = keys.length;
    for (const row of document.querySelectorAll("#apps .dock-app[data-key]")) row.classList.toggle("open", keys.includes(row.dataset.key));
  },
});
$("toggle-left").onclick = () => layout.toggle("left");
$("toggle-right").onclick = () => layout.toggle("right");
$("scrim").onclick = () => { layout.hide("left"); layout.hide("right"); };
addEventListener("keydown", (e) => {
  if (!(e.metaKey || e.ctrlKey) || e.code !== "KeyB") return;
  e.preventDefault();
  layout.toggle(e.altKey ? "right" : "left");
});
const leaveSidebar = () => { if (layout.narrow) layout.hide("left"); };
// A window is seen beside the chat: settings fill the work area, so one
// opened from there (an agent's Open, a screen, the skills) goes back to
// the open chat first, or, with none, lets the viewer show beside them.
function show(spec) {
  if (state.page && byName(state.current)) openChat(state.current);
  else $("layout").classList.remove("settings-view");
  viewer.open(spec);
  layout.show("right");
}

// A frame of `src` (the mint's, for fragment `name`; a port's ticket).
function frameOf(src, title, name = null) {
  const frame = el("iframe");
  frame.setAttribute("aria-label", title);
  frame.allow = "clipboard-write; microphone";
  frame.dataset.src = src;
  if (name) {
    // A bare `microphone` is the src's origin's, the mint's on this page's;
    // the page lands at the fragment's own, which the browser would refuse
    // without asking. So it names that origin before the frame navigates
    // (a policy is read as a navigation starts), and loads regardless:
    // until then a navigation asked of it waits (`go`), the last one kept.
    frame.dataset.fragment = name;
    unready.set(frame, src);
    originOf(name)
      .then((origin) => { frame.allow = `clipboard-write; microphone ${origin}`; }, () => {})
      .finally(() => {
        const next = unready.get(frame);
        unready.delete(frame);
        frame.src = next;
      });
  } else frame.src = src;
  frame.addEventListener("load", () => theme(frame));
  return frame;
}
// a fragment's frame whose policy does not name its origin yet -> the
// navigation it makes once it does (frameOf)
const unready = new WeakMap();
// A frame sent to `src`: at once, or, its origin not known yet, once it is.
function go(frame, src) {
  if (unready.has(frame)) unready.set(frame, src);
  else frame.src = src;
}

// A frame that could not sign in says so, and how on (the mint's note,
// auth.rs `frame_note`, or a fragment's page whose browser keeps its frame
// cookie from it): in this page's look, in the frame's place (a note
// before it, the frame hidden), tried again when the person comes back
// from the tab it sent them to.
const BLOCKED = {
  consent: (t) => [`Open ${t} to continue`, "It isn't yours, and no one shared it with you: it learns who you are only once you say so, in a tab of its own.", "Open it in a tab", "_blank"],
  cookies: (t) => [`${t} can't sign in here`, "This browser keeps a window's own sign-in from it. Open it in a tab instead.", "Open it in a tab", "_blank"],
  "signed-out": (t) => [`Sign in to see ${t}`, "You're signed out.", "Sign in", "_top"],
};
const blockedNote = (frame) => (frame.previousElementSibling?.classList.contains("frame-note") ? frame.previousElementSibling : null);
addEventListener("message", (event) => {
  const d = event.data;
  if (d?.fragment !== "signin-blocked" || !Object.hasOwn(BLOCKED, d.why)) return;
  const frame = [...document.querySelectorAll("iframe[data-fragment]")].find((f) => f.contentWindow === event.source);
  if (!frame || frame.dataset.fragment !== d.name) return;
  const [title, text, go, target] = BLOCKED[d.why](titleOf(d.name));
  const href = d.why === "signed-out" ? `/auth/login?return=${encodeURIComponent(location.pathname)}` : `/auth/fragment?name=${encodeURIComponent(d.name)}&return=/`;
  frame.dataset.blocked = d.why;
  // no chat's page is in it: it holds nothing, and is busy with nothing
  refresh.resetFrame(frame);
  const allow = el("a", "allow", go);
  Object.assign(allow, { href, target, rel: "noopener" });
  const notice = el("div", "notice");
  notice.append(el("strong", null, title), el("span", null, text), allow);
  const note = el("div", "frame-note");
  note.append(notice);
  blockedNote(frame)?.remove();
  frame.before(note);
});
addEventListener("focus", () => {
  for (const frame of document.querySelectorAll("iframe[data-blocked]")) {
    delete frame.dataset.blocked;
    blockedNote(frame)?.remove();
    refresh.resetFrame(frame);
    go(frame, frame.dataset.src);
  }
});
// The frames follow this page's light or dark (they may not see its media).
const dark = matchMedia("(prefers-color-scheme: dark)");
const mobileChat = matchMedia("(max-width: 760px)");
// Load and decode both landscapes once. Hold the current one until its
// replacement is ready; a failed load keeps it, and a rapid theme reversal
// never applies an image that is no longer wanted.
function loadWallpaper(file) {
  const image = new Image();
  image.src = `/__shell/${file}`;
  return image.decode().then(() => image, () => null);
}
const wallpapers = {
  light: loadWallpaper("wallpaper.jpg"),
  dark: loadWallpaper("wallpaper-dark.jpg"),
};
document.documentElement.style.setProperty("--wallpaper-dark-opacity", dark.matches ? "1" : "0");
async function updateWallpaper() {
  const mode = dark.matches ? "dark" : "light";
  const image = await wallpapers[mode];
  if (image && mode === (dark.matches ? "dark" : "light")) {
    document.documentElement.style.setProperty("--wallpaper-dark-opacity", mode === "dark" ? "1" : "0");
  }
}
function theme(frame) {
  try {
    frame.contentWindow?.postMessage({ fragment: "theme", mode: dark.matches ? "dark" : "light", mobile: mobileChat.matches }, "*");
  } catch {}
}
dark.addEventListener("change", () => {
  void updateWallpaper();
  for (const f of document.querySelectorAll("iframe")) theme(f);
});
mobileChat.addEventListener("change", () => { for (const f of document.querySelectorAll("iframe")) theme(f); });

// ---- what is visible: the page is as tall as the visual viewport, from
// where it is (shell.css, --viewport-*). A phone's keyboard shrinks that
// viewport and Safari pans it, leaving the layout viewport be (it ignores
// `interactive-widget`), so the body follows it: a chat's composer, the
// launcher's input and a window's fields sit just above the keyboard, and
// the page itself never scrolls under it. While a keyboard is up, what is
// visible is shorter than the layout viewport: the screen's bottom edge is
// under the keyboard, though Safari may pan the layout viewport's bottom
// into view above it, and nothing keeps clear of that edge's inset (the
// home indicator's; shell.css, data-keyboard). ----
const visible = window.visualViewport;
function fitViewport() {
  if (!visible) return;
  const root = document.documentElement;
  root.style.setProperty("--viewport-height", `${Math.round(visible.height)}px`);
  root.style.setProperty("--viewport-top", `${Math.round(visible.offsetTop)}px`);
  root.toggleAttribute("data-keyboard", visible.height * visible.scale < root.clientHeight - 1);
}
visible?.addEventListener("resize", fitViewport);
visible?.addEventListener("scroll", fitViewport);
fitViewport();

// ---- the person's agents, for a page of theirs that asks (a chat's @) ----
// A frame of a fragment the person owns may ask for their agents
// (`{fragment: "agents?"}`); the shell answers it, at that fragment's own
// origin only (its status's canonical URL), `{fragment: "agents", agents:
// [{identity, fragment, name, title, picture?, phase?, modelProviders?}],
// models?}` (`picture` its picture's address, whole, on this platform;
// `phase` their computer's, as the platform last said: `asleep`,
// `starting`, `awake`, `sleeping` or `wont_wake`; `modelProviders` and
// `models` as their computer's view says them: the providers whose models
// each may run now, and the model route's models' ids by its names, which
// with an agent's `agent.json` say the model it runs), and again whenever
// any of it changes: a rename or a new picture reaches the page as it
// reaches this one.
// It may then ask for one of them in its fragment (`{fragment: "add-agent",
// identity, nonce}`): the shell asks its person, in its own dialog (never
// in the frame: a page is code its author or an agent wrote, so it asks
// and never grants), "Add Fred to <title>?", and only on their Add adds that
// agent as an editor, as making a chat does (decision 36: an owner shares
// their own fragment with their own agent). It answers `{fragment:
// "agent-added", nonce, identity, ok, error?}`: `error` "declined" on
// Cancel, "not answered" after ADD_CONFIRM_MS, "busy" while another ask is
// open. Nothing is remembered: every add is asked. A frame of a fragment
// the person does not own (shared with them) learns nothing and adds no
// one, and no page adds anyone but the person's own agents. It names no
// template: any page of theirs may use it.
const ADD_CONFIRM_MS = 90_000;
// Add arms this long after the dialog shows, so the click or key that sent
// the page's message cannot confirm it (the share sheet's 800 ms).
const ADD_ARM_MS = 800;
const rosterTo = new Map(); // a frame's window -> its fragment's origin
const origins = new Map(); // fragment name -> its origin (a promise)
function originOf(name) {
  if (!origins.has(name)) {
    const asked = api("GET", `/api/f/${seg(name)}/status`).then((s) => new URL(s.urls.canonical).origin);
    asked.catch(() => origins.delete(name));
    origins.set(name, asked);
  }
  return origins.get(name);
}
const roster = () => {
  const phase = state.computer?.phase;
  return [...state.agents.values()].map((a) => {
    const picture = agentFace(a.fragment, a.identity).picture;
    return { identity: a.identity, fragment: a.fragment, name: a.name || labelOf(a.fragment), title: titleOf(a.fragment), ...(picture ? { picture: new URL(picture, location.origin).href } : {}), ...(phase ? { phase } : {}), ...(a.modelProviders?.length ? { modelProviders: a.modelProviders } : {}) };
  });
};
// the answer to `name`'s frame
const rosterFor = (name) => ({ fragment: "agents", agents: roster(), ...(state.computer?.models ? { models: state.computer.models } : {}), setup: setupFor(name) });
let rosterSent = "";
function rosterChanged() {
  // the first run's chat named too: its steps are the same before it is
  // named, and a chat already open (`readAgain`) hears them only then
  const now = JSON.stringify([roster(), state.computer?.models, onboarding.setupState()]);
  if (now === rosterSent) return;
  rosterSent = now;
  const live = new Set([...document.querySelectorAll("iframe[data-fragment]")].map((f) => f.contentWindow));
  for (const [win, origin] of rosterTo) {
    if (!live.has(win)) rosterTo.delete(win);
    else {
      const frame = [...document.querySelectorAll("iframe[data-fragment]")].find((f) => f.contentWindow === win);
      win.postMessage(rosterFor(frame?.dataset.fragment), origin);
    }
  }
}
addEventListener("message", async (event) => {
  const d = event.data;
  if (!["agents?", "add-agent", "retry-setup"].includes(d?.fragment)) return;
  // the frame that sent it shows one of the person's own fragments, and
  // the page in it is that fragment's
  const frame = [...document.querySelectorAll("iframe[data-fragment]")].find((f) => f.contentWindow === event.source);
  const name = frame?.dataset.fragment;
  if (!name || byName(name)?.role !== "owner") return;
  let origin;
  try {
    origin = await originOf(name);
  } catch {
    return;
  }
  if (event.origin !== origin || frame.contentWindow !== event.source) return;
  if (d.fragment === "retry-setup") {
    onboarding.retrySetup(name);
    return;
  }
  if (d.fragment === "agents?") {
    rosterTo.set(event.source, origin);
    event.source.postMessage(rosterFor(name), origin);
    // a prompt pressed before its chat's page was there (`composeApp`)
    const text = composing.get(name);
    composing.delete(name);
    if (text !== undefined) event.source.postMessage({ fragment: "compose", text }, origin);
    return;
  }
  const asker = event.source;
  const answer = (ok, error) => asker.postMessage({ fragment: "agent-added", nonce: d.nonce, identity: d.identity, ok, ...(error ? { error } : {}) }, origin);
  if (typeof d.nonce !== "string" || d.nonce.length > 64) return;
  const agent = state.agents.get(d.identity);
  if (!agent) return answer(false, "that is not one of your agents");
  const said = await confirmAdd(agent, name);
  if (said !== "added") return answer(false, said);
  // the frame may have gone, or shown another page, while its person read
  if (frame.contentWindow !== asker || !frame.isConnected) return;
  try {
    await api("PUT", `/api/f/${seg(name)}/members/${seg(agent.identity)}`, { role: "editor" });
    answer(true);
  } catch (e) {
    answer(false, e.message);
  }
});
// The person's answer to one page's add, in the shell's own dialog:
// "added" on Add; "declined" on Cancel or Escape; "not answered" when left
// ADD_CONFIRM_MS. One at a time: another ask while it is open is "busy".
const addDialog = $("add-agent-dialog");
let adding = null; // { resolve, timer, arm, outcome }
function confirmAdd(agent, name) {
  if (adding) return Promise.resolve("busy");
  return new Promise((resolve) => {
    named($("add-agent-title"), agent.fragment, (who) => `Add ${who}?`);
    named($("add-agent-text"), agent.fragment, (who) => `Add ${who} to ${titleOf(name)}? ${who} will be able to read and edit it.`);
    addDialog.dataset.agent = agent.identity;
    addDialog.dataset.fragment = name;
    $("add-agent-go").disabled = true;
    adding = {
      resolve,
      outcome: "declined",
      arm: setTimeout(() => { $("add-agent-go").disabled = false; }, ADD_ARM_MS),
      timer: setTimeout(() => {
        if (adding) adding.outcome = "not answered";
        addDialog.close();
      }, ADD_CONFIRM_MS),
    };
    addDialog.showModal();
    $("add-agent-cancel").focus();
  });
}
// whatever closes it (Add, Cancel, Escape, the timeout) settles the ask once
addDialog.addEventListener("close", () => {
  if (!adding) return;
  const { resolve, timer, arm, outcome } = adding;
  adding = null;
  clearTimeout(timer);
  clearTimeout(arm);
  resolve(outcome);
});
$("add-agent-form").onsubmit = (e) => {
  e.preventDefault();
  if ($("add-agent-go").disabled || !adding) return;
  adding.outcome = "added";
  addDialog.close();
};
$("add-agent-cancel").onclick = () => addDialog.close();

// ---- a chat's alerts (docs/api.md, The shell, "A chat's alerts are the
// shell's"; docs/chat-records.md, Push). A browser lets no cross-origin
// frame ask for notifications, so the shell asks, on this origin, for the
// chats it frames, and holds their subscriptions: one registration of its
// alerts worker (alerts-sw.js) a chat, its scope /__shell/alerts/<chat>/,
// subscribed with that chat's own VAPID key, which its page hands over.
// The page stores the subscription on its fragment as its person
// (`__push-sub`), as in a tab of its own, so whose alerts are on stays
// each chat's, and only that chat's fragment can push to it. Only a frame
// of one of the person's chats on the release's chat template asks, from
// its fragment's own origin. The scope names its person too: the worker
// shows nothing once another, or no one, is signed in here, and lets itself
// go (alerts-sw.js), whatever ended the session ----
const ALERTS_WORKER = "/__shell/alerts-sw.js";
const ALERTS_SCOPE = "/__shell/alerts/";
const alertsScope = (name) => new URL(`${ALERTS_SCOPE}${encodeURIComponent(state.me.id)}/${encodeURIComponent(name)}/`, location.origin).href;
// the person leaving (signing out, or in as another): no alerts are
// turned on from then, and one made as they left lets go at once
let alertsEnded = false;
const alertable = () => "serviceWorker" in navigator && typeof PushManager !== "undefined" && typeof Notification !== "undefined";
const b64u = (buf) => btoa(String.fromCharCode(...new Uint8Array(buf))).replaceAll("+", "-").replaceAll("/", "_").replaceAll("=", "");
// a P-256 public key, uncompressed (65 bytes): a VAPID key
function vapidKey(s) {
  try {
    const key = Uint8Array.from(atob(s.replaceAll("-", "+").replaceAll("_", "/")), (c) => c.charCodeAt(0));
    return key.length === 65 && key[0] === 4 ? key : null;
  } catch {
    return null;
  }
}
// a chat's registration (none when its alerts are off)
async function alertsOf(name) {
  const reg = await navigator.serviceWorker.getRegistration(alertsScope(name));
  return reg?.scope === alertsScope(name) ? reg : null;
}
// a registration's worker, once it may hold a subscription
function activated(reg) {
  const w = reg.installing ?? reg.waiting;
  if (!w) return Promise.resolve(reg);
  return new Promise((resolve, reject) => {
    w.addEventListener("statechange", () => {
      if (w.state === "activated") resolve(reg);
      else if (w.state === "redundant") reject(new Error("the alerts worker did not start"));
    });
  });
}
async function alertsOn(name, key) {
  // the press in the frame is this page's too: the browser asks once
  if (Notification.permission === "default") await Notification.requestPermission();
  if (Notification.permission !== "granted") return { state: Notification.permission === "denied" ? "denied" : "off" };
  const reg = await activated(await navigator.serviceWorker.register(ALERTS_WORKER, { scope: alertsScope(name) }));
  const sub = (await reg.pushManager.getSubscription()) ?? (await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: key }));
  if (alertsEnded) {
    await alertsOff(name);
    return { state: "failed", error: "signed out" };
  }
  return { state: "on", subscription: { endpoint: sub.endpoint, p256dh: b64u(sub.getKey("p256dh")), auth: b64u(sub.getKey("auth")) } };
}
async function alertsOff(name) {
  const reg = await alertsOf(name);
  const sub = await reg?.pushManager.getSubscription();
  await sub?.unsubscribe();
  await reg?.unregister();
  return { state: "off", ...(sub ? { endpoint: sub.endpoint } : {}) };
}
addEventListener("message", async (event) => {
  const d = event.data;
  if (!["alerts?", "alerts"].includes(d?.fragment) || typeof d.nonce !== "string" || d.nonce.length > 64 || alertsEnded) return;
  const frame = [...document.querySelectorAll("iframe[data-fragment]")].find((f) => f.contentWindow === event.source);
  const name = frame?.dataset.fragment;
  if (!name || byName(name)?.kind !== "chat") return;
  let status;
  try {
    status = await api("GET", `/api/f/${seg(name)}/status`);
  } catch {
    return;
  }
  const origin = new URL(status.urls.canonical).origin;
  if (event.origin !== origin || frame.contentWindow !== event.source) return;
  // alerts are turned on for the release's chat alone, whose page is the
  // platform's own code (decision 40): a fork's is its author's, which
  // could push anything in this origin's name (the worker, too, shows
  // nothing of a chat that left it). Read and turned off, any chat's: one
  // that left the release may let go of what it had
  const blessed = String(status.code?.id ?? "").startsWith("blessed:chat@");
  const answer = (a) => event.source.postMessage({ fragment: "alerts", nonce: d.nonce, ...a }, origin);
  if (!alertable()) return answer({ state: "unsupported" });
  try {
    if (d.fragment === "alerts?") {
      if (Notification.permission === "denied") return answer({ state: "denied" });
      const on = Notification.permission === "granted" && !!(await (await alertsOf(name))?.pushManager.getSubscription());
      return answer({ state: on ? "on" : "off" });
    }
    const key = d.on === true && typeof d.key === "string" ? vapidKey(d.key) : null;
    if (key) return answer(blessed ? await alertsOn(name, key) : { state: "failed", error: "only the release's chat turns alerts on" });
    if (d.on === false) return answer(await alertsOff(name));
  } catch (e) {
    const denied = e?.name === "NotAllowedError" || e?.name === "PermissionDeniedError";
    return answer(denied ? { state: "denied" } : { state: "failed", error: String(e?.message ?? e) });
  }
});
// every chat's alerts this browser holds, let go (signing out: the next
// person in it gets none of the last one's)
async function dropAlerts() {
  if (!alertable()) return;
  const regs = await navigator.serviceWorker.getRegistrations().catch(() => []);
  await Promise.all(
    regs
      .filter((r) => new URL(r.scope).pathname.startsWith(ALERTS_SCOPE))
      .map(async (r) => {
        await (await r.pushManager.getSubscription().catch(() => null))?.unsubscribe().catch(() => {});
        await r.unregister().catch(() => {});
      }),
  );
}
// an alert clicked: its chat, here (alerts-sw.js; one opened for it comes
// with `?chat=`, at the page's start)
navigator.serviceWorker?.addEventListener("message", (event) => {
  const d = event.data;
  if (d?.fragment === "open-chat" && typeof d.name === "string" && byName(d.name)?.kind === "chat") {
    openChat(d.name);
    leaveSidebar();
  }
});
navigator.serviceWorker?.startMessages();

// ---- sharing: the platform's own sheet, framed (it is this origin's) ----
function badges(f) {
  const out = [];
  const { guests = 0, invites = 0, visibility } = f.sharing ?? {};
  if (guests > 0 || invites > 0) {
    const b = el("span", "shared");
    b.replaceChildren(icon("people"));
    b.append(String(guests + invites));
    b.title = [
      guests > 0 ? `Shared with ${guests} ${guests === 1 ? "person" : "people"}` : "",
      invites > 0 ? `${invites} ${invites === 1 ? "invite waits" : "invites wait"} on an email` : "",
    ].filter(Boolean).join("; ");
    out.push(b);
  }
  if (visibility === "public") {
    const b = el("span", "shared");
    b.replaceChildren(icon("globe"));
    b.title = "Anyone can open it";
    out.push(b);
  }
  return out;
}
const sheet = $("sheet");
// Its owner shares it; anyone else in it sees who is, and may leave
// (the sheet decides who may do what: share.rs).
const someoneElses = (name) => !!byName(name) && !own(byName(name));
const shareItem = (name) => (someoneElses(name) ? { icon: "people", text: "People…", onClick: () => share(name) } : { icon: "invite", text: "Invite…", onClick: () => share(name) });
function share(name) {
  const frame = el("iframe");
  named(frame, name, (t) => (someoneElses(name) ? `Who has access to ${t}` : `Share ${t}`), "aria-label");
  frame.allow = "clipboard-write";
  frame.src = `/share/${encodeURIComponent(name)}`;
  sheet.replaceChildren(frame);
  sheet.showModal();
}
addEventListener("message", (e) => {
  const frame = sheet.querySelector("iframe");
  if (!sheet.open || e.source !== frame?.contentWindow || e.origin !== location.origin) return;
  if (Number.isFinite(e.data?.height)) frame.style.height = `${e.data.height}px`;
  if (e.data?.share === "done") sheet.close();
});
sheet.onclick = (e) => { if (e.target === sheet) sheet.close(); };
sheet.onclose = () => {
  sheet.replaceChildren();
  load().catch(() => {});
  // the Apps page says who has access as the sheet may have changed it
  appsPage.shared().catch(() => {});
};

// ---- one menu, for whichever button asked ----
const menu = $("menu");
let menuFor = null;
let menuClosing = false;
let menuAnimations = [];
let menuParts = [];
let menuMotionId = 0;
function cancelMenuMotion() {
  ++menuMotionId;
  for (const animation of menuAnimations) animation.cancel();
  menuAnimations = [];
}
function finishMenuClose() {
  cancelMenuMotion();
  if (!menuFor) return;
  menuFor.button.setAttribute("aria-expanded", "false");
  menuFor = null;
  menu.hidden = true;
  // a closed menu keeps nothing it said (a name renamed since, among them)
  menu.replaceChildren();
  menu.inert = false;
  menuClosing = false;
}
function animateMenu(opening) {
  const current = menuParts.map(({ node }) => ({ opacity: getComputedStyle(node).opacity, transform: getComputedStyle(node).transform }));
  cancelMenuMotion();
  if (reducedMotion.matches) { if (!opening) finishMenuClose(); return; }
  const id = menuMotionId;
  menuAnimations = menuParts.map(({ node, origin }, i) => node.animate(opening
    ? springFrames(t => ({ opacity: node === menuFor.button ? 1 : Math.min(1, t), transform: `scale(${.97 + .03 * t})`, transformOrigin: origin }))
    : [{ ...current[i], transformOrigin: origin }, { opacity: node === menuFor.button ? 1 : 0, transform: node === menuFor.button ? "scale(1)" : "scale(.97)", transformOrigin: origin }],
  { duration: opening ? 280 : 160, easing: opening ? "linear" : "cubic-bezier(.4, 0, 1, 1)", fill: "both" }));
  Promise.all(menuAnimations.map(a => a.finished)).then(() => {
    if (id !== menuMotionId) return;
    if (opening) cancelMenuMotion(); else finishMenuClose();
  }).catch(() => {});
}
function closeMenu(immediate = false) {
  if (!menuFor) return;
  if (immediate === true || reducedMotion.matches) return finishMenuClose();
  if (menuClosing) return;
  menuClosing = true;
  menu.inert = true;
  animateMenu(false);
}
reducedMotion.addEventListener("change", () => {
  if (reducedMotion.matches) { if (menuClosing) finishMenuClose(); else cancelMenuMotion(); }
});
// `items`: each `{icon, text, onClick}`, or an agent's `{face, text, tag?,
// current?}`: its face in the icon's place, its role a tag, and whether it
// is the one picked now.
function openMenu(button, items) {
  const again = menuFor?.button === button && !menuClosing;
  if (again) return closeMenu();
  closeMenu(true);
  menuFor = { button };
  menu.replaceChildren(...items.map((it) => {
    const b = el("button");
    b.type = "button";
    b.setAttribute("role", "menuitem");
    b.replaceChildren(it.face ?? icon(it.icon));
    b.append(it.face ? el("span", "name", it.text) : it.text);
    if (it.tag) b.append(el("span", "role-tag", it.tag));
    if (it.current) b.setAttribute("aria-current", "true");
    b.onclick = () => { closeMenu(); it.onClick(); };
    return b;
  }));
  button.setAttribute("aria-expanded", "true");
  menu.hidden = false;
  const r = button.getBoundingClientRect();
  menu.style.top = `${Math.min(r.bottom + 4, innerHeight - menu.offsetHeight - 8)}px`;
  menu.style.left = `${Math.max(8, Math.min(r.left, innerWidth - menu.offsetWidth - 8))}px`;
  menu.querySelector("button")?.focus();
  menuParts = [{ node: menu, origin: "50% 50%" }];
  animateMenu(true);
}
addEventListener("pointerdown", (e) => { if (menuFor && !menu.contains(e.target) && !menuFor.button.contains(e.target)) closeMenu(); });
addEventListener("keydown", (e) => {
  if (!menuFor || menuClosing) return;
  if (e.key === "Escape") {
    const trigger = menuFor.button;
    closeMenu();
    trigger.focus();
    e.preventDefault();
  } else if (["ArrowDown", "ArrowUp"].includes(e.key)) {
    const buttons = [...menu.querySelectorAll("button")];
    const i = buttons.indexOf(document.activeElement);
    buttons[(i + (e.key === "ArrowDown" ? 1 : -1) + buttons.length) % buttons.length]?.focus();
    e.preventDefault();
  } else if (e.key === "Tab") closeMenu();
});
addEventListener("blur", closeMenu);
addEventListener("resize", () => closeMenu(true));

const watching = { on: false, socket: null, wait: 1000, timer: null, settle: null };
const listLive = () => watching.socket?.readyState === WebSocket.OPEN;
const { renderHeading, renderChats, loadInvites, primaryAgent, newChatAgent, ownAgents, markSeen } = createChatList({
  api, ui: { $, el, patch, agentFace, mark, notice },
  data: { byName, own, chats, shown, titleOf, chatTitle: (name) => chatTitle(name), homeMark: (f) => homeMark(f), isGroup, agentOfChat: (name) => agentOfChat(name), agentsOf,
    agents: () => state.agents, allFragments: () => state.fragments, computer: () => state.computer, isGuest: () => state.guest, personId: () => state.me?.id,
    isThread: (f) => threads.isPair(f), waitingOn: (name) => threads.waitingOn(name), renderThreads: () => threads.render() },
  selection: { current: () => state.current, page: () => state.page, getFrame: (name) => state.frames.get(name) },
  menus: { menuOwner: () => menuFor, openMenu, shareItem, archiveItem },
  navigation: { newChat, rename: (name) => rename(name), openChat, openApp, openScreen, leaveSidebar, seg },
  listLive, load, invites: () => state.invites, setInvites: (v) => { state.invites = v; },
});

// Delegation threads: a line each over the chat an agent asked from, its
// pair chat its transcript (threads.js)
const threads = createThreads({
  $, el, patch, api, seg, avatar,
  data: { byName, own, chats, titleOf, labelOf, agentFace, ownAgents, current: () => state.current, page: () => state.page },
  navigation: { openChat, leaveSidebar },
  rendered: () => renderChats(),
});

// The identity of the person's own agent a direct chat is with (its row's
// one agent, or the agent its label is for while its row names none:
// `agentsOf`); null for a group, or a chat with someone else's agent.
function chatAgentOf(name) {
  if (!name || isGroup(name)) return null;
  const id = agentsOf(name)[0];
  return id && state.agents.has(id) ? id : null;
}
// the agent fragment a direct chat is with (a group's agents keep names of their own)
const agentOfChat = (name) => state.agents.get(chatAgentOf(name))?.fragment ?? null;
// An agent's own chat: the one the shell made with it first, `<label>-chat`
// beside its `<label>` (defaultAgent), which Hermes' Bot Mode messages
// (images/hermes/boot/src/bots.rs, `bot_chat`) and `fragment ask` find. It
// is titled on its own, as any chat ("Your first chat": FIRST_CHAT), and
// never by its agent: the agent's name is its fragment's alone.
const agentsOwnChat = (agent) => chats().find((f) => own(f) && labelOf(f.name) === `${labelOf(agent)}-chat`) ?? null;
const { faceControls, renderAgentFaces, agentSettings, addAgentForm, agentJson, writeAgentJson, savePick, settingLabel, modelOption } = createAgentSettings({
  api, el, seg, byName, agentFace, titleOf, agents: () => state.agents, named, rename: (name) => rename(name), load, nameScreen, renderNames,
  computer: () => state.computer, homePicker: (a, said) => homePicker(a, said), addAgent: (spec) => onboarding.addAgent(spec),
});
const { chatTitle, homeMark, loadHomes, homePicker, paintHomes, setHome } = createAgentHomes({
  el, icon, named, actionLine, settingLabel, modelOption, savePick,
  agents: () => state.agents, chats, own, labelOf, byName, titleOf, agentsOwnChat,
  agentJson: (agent) => agentJson(agent), writeAgentJson, renderChats,
});

// Each time the middle column is given a view (a chat, settings): a
// launch's late answer changes the screen only if none was given since
// (launcher.js).
let views = 0;
function openChat(name, push = true) {
  if (!byName(name)) return;
  views++;
  if (push) at("/");
  state.page = null;
  $("layout").classList.remove("settings-view");
  $("settings-page").hidden = true;
  leaveApps();
  $("frames").hidden = false;
  state.current = name;
  store.set(CURRENT, name);
  hideNotice();
  chatFrame(name);
  for (const [n, frame] of state.frames) frame.hidden = n !== name;
  renderHeading();
  renderChats();
  markSeen();
  prewake();
  navigated();
  // home: a first run a failed read put off goes on from here too
  if (onboarding.putOff()) resumeFirstRun();
}

// A chat's frame, made once and kept.
function chatFrame(name, path = setupFor(name) ? "/?setup=1" : "/") {
  if (state.frames.has(name)) return;
  const frame = frameOf(framed(name, path), titleOf(name), name);
  $("frames").append(frame);
  state.frames.set(name, frame);
}

// ---- apps: every other fragment, each a window in the viewer ----
function paneIcon(name) {
  const i = el("span", "pane-icon");
  i.replaceChildren(icon(name));
  return i;
}
const iconOf = (name) => appArt(byName(name), labelOf(name));
// An app's card: its page as the platform shot it after its last deploy
// (`GET /api/f/<name>/card`, read with this page's session and shown as a
// blob URL), shown larger over its dock icon (`showPeek`).
const cards = new Map();
// "new", "same", or "none" (no card yet)
async function loadCard(name) {
  const known = cards.get(name);
  const headers = { "x-fragment-shell": "1" };
  if (known) headers["if-none-match"] = known.etag;
  const r = await fetch(`/api/f/${seg(name)}/card`, { headers, credentials: "same-origin" });
  heard(r);
  if (r.status === 304) return "same";
  if (r.status === 404 && !known) return "none";
  if (!r.ok) return "same";
  const url = URL.createObjectURL(await r.blob());
  if (known) URL.revokeObjectURL(known.url);
  cards.set(name, { etag: r.headers.get("etag"), url });
  return "new";
}
// every listed app's, again after a deploy may have made one: an app with
// none yet is asked again, waiting longer each time (2 s to a minute), and
// the rest when the page comes back after a minute away. `only` (a set of
// names, after a change to the list): those, and the apps still without one.
const cardsLoad = { wait: 0, timer: null, at: 0 };
async function loadCards(only = null) {
  clearTimeout(cardsLoad.timer);
  cardsLoad.at = Date.now();
  const listed = docked().map((f) => f.name);
  for (const [name, card] of cards) if (!listed.includes(name)) { URL.revokeObjectURL(card.url); cards.delete(name); }
  const asked = listed.filter((name) => !only || only.has(name) || !cards.has(name));
  const got = await Promise.all(asked.map((name) => loadCard(name).catch(() => "same")));
  if (got.includes("new")) renderApps();
  if (got.includes("none") && cardsLoad.wait < 60_000) {
    cardsLoad.wait = Math.min(60_000, cardsLoad.wait ? cardsLoad.wait * 2 : 2_000);
    cardsLoad.timer = setTimeout(() => loadCards().catch(() => {}), cardsLoad.wait);
  }
}
addEventListener("focus", () => { if (Date.now() - cardsLoad.at > 60_000) loadCards().catch(() => {}); });
// a pointer over an app's icon in the dock shows its card larger, above it
const peek = el("div", "card-peek");
peek.hidden = true;
peek.setAttribute("aria-hidden", "true");
peek.append(el("img"));
document.body.append(peek);
function showPeek(row, name) {
  const card = cards.get(name);
  if (!card) return;
  peek.firstChild.src = card.url;
  peek.hidden = false;
  const r = row.getBoundingClientRect();
  peek.style.left = `${Math.max(8, Math.min(r.left + r.width / 2 - peek.offsetWidth / 2, innerWidth - peek.offsetWidth - 8))}px`;
  peek.style.top = `${Math.max(8, r.top - peek.offsetHeight - 12)}px`;
}
const hidePeek = () => { peek.hidden = true; };
// The desktop's dock: the person's apps (their list's, less the archived
// and those they took off it), each its icon and title, one shared with
// them saying their role there; then, for a seat, Add (how to ask for one:
// `showPrompts`).
function renderApps() {
  hidePeek();
  const rows = docked().map((f) => {
    const key = `app:${f.name}`;
    const title = titleOf(f.name);
    const row = el("button", `dock-app${viewer.keys.includes(key) ? " open" : ""}`);
    row.type = "button";
    row.dataset.key = key;
    row.dataset.role = f.role;
    row.title = own(f) ? title : `${title} (${f.role})`;
    const art = el("span", "dock-art");
    art.append(iconOf(f.name));
    row.append(art, el("span", "label", title));
    row.onclick = () => { hidePeek(); openApp(f.name); };
    row.onpointerenter = (e) => { if (e.pointerType !== "touch") showPeek(row, f.name); };
    row.onpointerleave = hidePeek;
    return row;
  });
  const empty = !rows.length;
  if (!state.guest) {
    const add = el("button", "dock-app dock-add");
    add.type = "button";
    add.id = "add-app";
    add.dataset.key = "prompts";
    add.title = "Ask for an app";
    const art = el("span", "dock-art");
    art.replaceChildren(icon("plus"));
    add.append(art, el("span", "label", "Add"));
    add.onclick = () => { hidePeek(); showPrompts(); leaveSidebar(); };
    rows.push(add);
  }
  patch($("apps"), rows);
  $("apps").parentElement.hidden = !rows.length;
  renderDesktopEmpty(empty);
}
// A desktop with no apps says what lands there. A seat's also offers the
// hand of ways to ask their agent for one (`promptHand`), each put in
// their chat's composer as the dock's Add's are.
function renderDesktopEmpty(empty) {
  const box = $("desktop-empty");
  const primary = primaryAgent();
  const asks = empty && !state.guest && !!primary?.identity;
  const sig = JSON.stringify([empty, state.guest, asks && titleOf(primary.fragment)]);
  box.hidden = !empty;
  if (box.dataset.sig === sig) return;
  box.dataset.sig = sig;
  box.replaceChildren();
  if (!empty) return;
  box.append(el("p", "desktop-empty-title", state.guest ? "Apps shared with you land here" : "Your apps land here"));
  if (!asks) return;
  box.append(el("p", "desktop-empty-text", `Ask ${titleOf(primary.fragment)} for one:`));
  box.append(...promptHand("desktop-suggestions", "desktop-suggestion"));
}
// An app's window, at `path` (a link's, from a chat). One its row has not
// reached yet (an agent's app, just made) opens all the same, by its name:
// the mint signs it in, or its frame says why not. Already open, it shows
// the page asked for.
function openApp(name, path = "/") {
  const f = byName(name);
  const shown = document.querySelector(`#stack .pane[data-key="${CSS.escape(`app:${name}`)}"] iframe`);
  if (shown && path !== "/") go(shown, framed(name, path));
  const frame = named(frameOf(framed(name, path), titleOf(name), name), name, (t) => t, "aria-label");
  const status = el("span", "pane-sharing");
  if (f) status.append(...badges(f));
  // its own URL, for a tab of its own or the clipboard
  const at = originOf(name).then((o) => `${o}${path}`);
  at.catch(() => {});
  show({
    key: `app:${name}`, title: titleOf(name), subtitle: !f || own(f) ? undefined : f.role, icon: iconOf(name), body: frame, status,
    actions: [
      ...(f && own(f) ? [{ icon: "share", title: "Share…", onClick: () => share(name) }] : []),
      { icon: "folder", title: "Files", onClick: () => show({ key: `tree:${name}`, title: titleOf(name), subtitle: "files", icon: paneIcon("folder"), body: named(frameOf(framed(name, "/__files"), `${titleOf(name)} files`, name), name, (t) => `${t} files`, "aria-label") }) },
      { icon: "reload", title: "Reload", onClick: () => go(frame, framed(name, path)) },
      { icon: "external", title: "Open in a new tab", onClick: () => window.open(`/auth/fragment?name=${encodeURIComponent(name)}&return=${encodeURIComponent(path)}`, "_blank", "noopener") },
      {
        icon: "more", title: "More", onClick: (e) => openMenu(e.currentTarget, [
          { icon: "link", text: "Copy link", onClick: () => at.then((u) => navigator.clipboard.writeText(u)).catch((err) => notice("The link was not copied", err.message)) },
          // someone else's: who else is in it, and leaving it
          ...(f && !own(f) ? [shareItem(name)] : []),
          ...(f && (own(f) || f.role === "editor") ? [{ icon: "image", text: "Icon…", onClick: () => openIconSheet(iconHelpers, name, titleOf(name), byName(name)) }] : []),
          ...(f ? [archiveItem(name)] : []),
        ]),
      },
    ],
  });
  navigated();
}

// the icon sheet's (app-icon.js): the list again, which takes a new icon to the dock
const iconHelpers = { api, reload: () => load() };

// ---- archiving: the person's own view (their list's row), not the fragment's ----
function archiveItem(name) {
  const archived = !!byName(name)?.archived;
  return { icon: "archive", text: archived ? "Unarchive" : "Archive", onClick: () => setArchived(name, !archived) };
}
async function setArchived(name, archived) {
  try {
    await api("PUT", `/api/fragments/${seg(name)}/archived`, { archived });
    await load();
    // the open chat, archived, gives the middle column to another
    const next = shown(chats()).find((f) => f.name !== name);
    if (archived && state.current === name && !state.page && next) openChat(next.name);
  } catch (e) {
    notice(archived ? "It was not archived" : "It was not unarchived", e.message);
  }
}
// A frame asks for a page as a window (`{fragment: "open", url}`): a file of
// its own origin (a fragment's file viewer) opens as a file's window; a page
// of another fragment of this deployment (a link in a chat: "Open"), as that
// fragment's window beside the chat, signed in by the mint as its row's is.
// Anything else opens nothing: the frame names a URL, the shell decides
// whether it is a fragment's (`fragmentAt`).
addEventListener("message", async (e) => {
  const ask = e.data;
  if (ask?.fragment !== "open" || typeof ask.url !== "string") return;
  const from = [...document.querySelectorAll("iframe")].find((f) => f.contentWindow === e.source);
  if (!from) return;
  let url;
  try {
    url = new URL(ask.url, e.origin);
  } catch {
    return;
  }
  if (url.origin === e.origin) {
    const path = url.searchParams.get("path") || String(ask.title || "");
    const frame = el("iframe");
    frame.src = url.href;
    frame.setAttribute("aria-label", path);
    show({ key: `file:${url.href}`, title: path.split("/").pop(), icon: paneIcon("folder"), body: frame });
    return;
  }
  const sender = from.dataset.fragment;
  if (!sender || !byName(sender)) return;
  let origin;
  try {
    origin = await originOf(sender);
  } catch {
    return;
  }
  // the page in the frame is still its fragment's own
  if (e.origin !== origin || from.contentWindow !== e.source) return;
  const name = fragmentAt(url, sender, origin);
  if (!name) return;
  if (isChat(name)) openChat(name);
  else openApp(name, `${url.pathname}${url.search}${url.hash}`);
});
// A fragment's name, when `url` is a page of one on this deployment: its
// host is the asking fragment's (`sender`, at `origin`, its status's
// canonical URL) with another fragment's name in its place (a branch's mark
// and the zone, the scheme and the port all the same). Else null.
const FRAGMENT_NAME = /^[a-z0-9](?:-?[a-z0-9])*--[a-km-np-z2-9]{4}$/;
function fragmentAt(url, sender, origin) {
  const own = new URL(origin);
  if (!own.hostname.startsWith(sender) || url.protocol !== own.protocol || url.port !== own.port || url.username || url.password) return null;
  const tail = own.hostname.slice(sender.length);
  if (!url.hostname.endsWith(tail)) return null;
  const name = url.hostname.slice(0, url.hostname.length - tail.length);
  return FRAGMENT_NAME.test(name) ? name : null;
}

// ---- an agent's screen: its own desktop, a port of its computer's own
// origin, through a ticket that lands on the agent's page (docs/computers.md, Ports) ----
async function openScreen(agent) {
  if (!state.computer || !agent) return;
  try {
    const t = await api("POST", `/api/computers/${seg(state.computer.computer)}/ports/6080/ticket`, { path: `/?agent=${encodeURIComponent(agent)}` });
    const frame = frameOf(t.url, `${titleOf(agent)}'s screen`);
    frame.dataset.screen = agent;
    frame.addEventListener("load", () => nameScreen(frame));
    show({ key: `screen:${agent}`, title: titleOf(agent), subtitle: "screen", icon: paneIcon("screen"), body: frame, persist: false });
  } catch (e) {
    notice("Its screen did not open", e.message);
  }
}
// Whose screen it is, by the agent's name as the person's list says it now
// (its fragment's title): its window's, and its page's, told at the port's
// own origin as it loads and again on a rename (`{fragment: "screen",
// agent, name}`; docs/computers.md, Ports). Its computer knows the agent by
// its label alone, which is no name.
function nameScreen(frame) {
  const agent = frame.dataset.screen;
  frame.setAttribute("aria-label", `${titleOf(agent)}'s screen`);
  const title = frame.closest(".pane")?.querySelector(".pane-title");
  if (title) title.textContent = titleOf(agent);
  try {
    frame.contentWindow?.postMessage({ fragment: "screen", agent, name: titleOf(agent) }, new URL(frame.dataset.src).origin);
  } catch {}
}

const { simplexSection, refreshSimplex, nameSimplex } = createSimplex({
  api, el, section, actionLine, named, seg, computer: () => state.computer, getSeat: () => state.seat,
  titleOf, frameOf, show, paneIcon, notice, setHome,
});
// Their seat as an action answered it (Billing's sleep choice, #609's
// review): what Settings' SimpleX says follows it at once.
function seatIs(seat) {
  state.seat = seat;
  refreshSimplex();
}

// ---- making the agent: its fragment, its computer, its chat (decision 10, 16) ----
// A label from a name, short enough that with freeLabel's `-<n>` and a
// chat's `-chat` its address fits any deployment's hosts (docs/api.md,
// Names: every deployment leaves 39 bytes for labels); never ending in a
// dash.
const LABEL_ROOM = 39;
const slug = (s) => s.toLowerCase().normalize("NFKD").replace(/[^a-z0-9]+/g, "-").replace(/^-+/, "").slice(0, LABEL_ROOM - "-99-chat".length).replace(/-+$/, "") || "agent";
function freeLabel(base) {
  const taken = new Set(state.fragments.map((f) => labelOf(f.name)));
  for (let i = 1; ; i++) {
    const l = i === 1 ? base : `${base}-${i}`;
    // an agent's chats are its label's: none an earlier agent left is taken over
    if (!taken.has(l) && !taken.has(`${l}-chat`) && !taken.has(`${l}-simplex`)) return l;
  }
}
// ---- another chat with an agent: a fresh start (its context is the chat's:
// a runtime keys an agent's session by its chat), titled from its first
// message unless renamed. Its own chat (`<label>-chat`) stays its own. ----
const UNTITLED = "New chat";
// a title from a first message, as finite-mono's: at most 48 characters (the rename field's)
const TITLE_MAX_CHARS = 48;
// A chat's label: its agent's, `-chat` and a number (`maple-chat-2`), short
// enough for any deployment's hosts with four digits' room.
const chatLabel = (agent) => freeLabel(`${labelOf(agent).slice(0, LABEL_ROOM - "-chat-9999".length).replace(/-+$/, "")}-chat`);
// An untitled chat with only this agent that no one has said anything in
// yet is the one to open again, not another.
const blank = (identity) => chats().find((f) => own(f) && !f.archived && f.title === UNTITLED && !f.preview && agentsOf(f.name).length === 1 && agentsOf(f.name)[0] === identity);
let starting = false;
// `open: false` makes it (or finds the blank one) without opening it: the
// launcher opens it itself, as it posts its first message.
async function newChat(identity, { quiet = false, open = true } = {}) {
  const agent = state.agents.get(identity);
  if (!agent || starting) return;
  starting = true;
  try {
    let name = blank(identity)?.name;
    if (!name) {
      const chat = await api("POST", "/api/fragments", { label: chatLabel(agent.fragment), template: "chat", title: UNTITLED });
      // adding it wakes its computer (the platform's `joined`)
      await api("PUT", `/api/f/${seg(chat.name)}/members/${seg(identity)}`, { role: "editor" });
      await load();
      name = chat.name;
    }
    if (!open) return name;
    openChat(name);
    leaveSidebar();
    return name;
  } catch (e) {
    if (quiet) throw e;
    notice("The chat was not made", e.message);
  } finally {
    starting = false;
  }
}
// A chat of the person's still titled UNTITLED that has a message is titled
// from its first message with words (its first line's words, cut to
// TITLE_MAX_CHARS); one renamed in the meantime keeps its name. Once per
// chat a page, after the list says it has a message.
const titling = new Set();
async function titleFromFirst(name) {
  if (titling.has(name)) return;
  titling.add(name);
  try {
    const r = await api("GET", `/api/f/${seg(name)}/channels/chat?after=0&limit=20`);
    // a message is a body with text and no kind but "message" (docs/chat-records.md)
    const said = (r.records ?? []).map((x) => (typeof x.body === "string" ? x.body : (x.body?.kind ?? "message") === "message" ? x.body?.text : null)).find((t) => typeof t === "string" && t.trim());
    if (!said) return titling.delete(name);
    const words = said.trim().split("\n").find((l) => l.trim()).replace(/\s+/g, " ").trim();
    const title = [...words].length > TITLE_MAX_CHARS ? `${[...words].slice(0, TITLE_MAX_CHARS - 1).join("").trimEnd()}…` : words;
    const manifest = await api("GET", `/api/f/${seg(name)}/manifest`);
    if (manifest.meta?.title !== UNTITLED) return;
    await retitle(name, title, "its title, from its first message");
  } catch {
    titling.delete(name);
  }
}

// ---- asking for an app: the dock's Add, a window of ways to ask
// (APP_PROMPTS). An app is a message to the person's agent like any other;
// one pressed goes into their chat's composer, to change and send. The
// templates stay the CLI's and the agents' (`fragment create --template`).
function shuffled(items) {
  const a = [...items];
  for (let i = a.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [a[i], a[j]] = [a[j], a[i]];
  }
  return a;
}
// The hand on screen, in a list (its buttons `cls`), and the die that
// deals the next one to every list showing it.
function promptHand(listCls, cls) {
  const list = el("div", listCls);
  list.dataset.prompts = cls;
  list.setAttribute("aria-live", "polite");
  dealTo(list);
  const die = el("button", "prompt-dice", "🎲");
  die.type = "button";
  die.title = "Show three more ideas";
  die.setAttribute("aria-label", die.title);
  die.onclick = rollPrompts;
  return [list, die];
}
function dealTo(list) {
  list.replaceChildren(...promptDeck.hand.map((i) => {
    const b = el("button", list.dataset.prompts, APP_PROMPTS[i]);
    b.type = "button";
    b.onclick = () => composeApp(APP_PROMPTS[i]);
    return b;
  }));
}
function rollPrompts() {
  if (!promptDeck.deck.length) {
    const deck = shuffled(APP_PROMPTS.map((_, i) => i).filter((i) => !promptDeck.hand.includes(i)));
    for (const i of promptDeck.hand) deck.splice(PROMPT_HAND + Math.floor(Math.random() * (deck.length - PROMPT_HAND + 1)), 0, i);
    promptDeck.deck = deck;
  }
  promptDeck.hand = promptDeck.deck.splice(0, PROMPT_HAND);
  for (const list of document.querySelectorAll("[data-prompts]")) {
    list.classList.add("dealt");
    dealTo(list);
  }
}
function showPrompts() {
  const primary = primaryAgent();
  const box = el("div", "app-prompts");
  const lede = (who) => `${who} makes an app from a message. Say who it's for and what it does: start from one of these, make it yours, then send it.`;
  box.append(primary ? named(el("p", "app-prompts-lede"), primary.fragment, lede) : el("p", "app-prompts-lede", lede("Your agent")));
  box.append(...promptHand("app-prompt-list", "app-prompt"));
  show({ key: "prompts", title: "Ask for an app", icon: paneIcon("plus"), body: box, persist: false });
}
// The chat a prompt goes to: the open one when it is the person's own;
// else their agent's newest, or a new chat with it (`newChat`).
async function promptChat() {
  const open = !state.page && byName(state.current);
  if (open && open.kind === "chat" && own(open)) return open.name;
  const primary = primaryAgent();
  if (!primary?.identity) return null;
  const theirs = shown(chats()).filter((f) => own(f) && agentsOf(f.name).includes(primary.identity)).sort(newestChat)[0];
  return theirs?.name ?? (await newChat(primary.identity, { quiet: true })) ?? null;
}
// A prompt waiting for its chat's page to ask `agents?` (a frame just
// made): chat -> text.
const composing = new Map();
// Puts `text` in the composer of the person's chat and focuses it, sending
// nothing: `{fragment: "compose", text}` to its frame, at its fragment's
// origin (docs/api.md, The shell). On a phone the desktop's sheet goes, so
// the composer is what shows.
async function composeApp(text) {
  let name;
  try {
    name = await promptChat();
  } catch (e) {
    return notice("The chat was not made", e.message);
  }
  if (!name) return notice("No agent yet", "Ask for an app once your agent is in your chat.");
  viewer.close("prompts");
  if (layout.narrow) {
    layout.hide("right");
    layout.hide("left");
  }
  composeIn(name, text);
}
function composeIn(name, text) {
  if (state.page || state.current !== name) openChat(name);
  const frame = state.frames.get(name);
  frame?.focus();
  const origin = frame && rosterTo.get(frame.contentWindow);
  if (origin) frame.contentWindow.postMessage({ fragment: "compose", text }, origin);
  else composing.set(name, text);
}

// The same display-name editor for chats, agents and Apps.
const { retitle, rename } = createTitles({ $, api, seg, byName, titleOf, reload: () => load(), heard: (r) => heard(r), ApiError });

// ---- the launcher (launcher.js): one input for starting a chat or finding one again ----
const launcher = createLauncher({
  $, el, api, ApiError, seg, state, byName, isChat, titleOf, chatTitle, mark, iconOf, primaryAgent, newChatAgent, agentsOwnChat, UNTITLED,
  openChat, openApp, chatFrame, composeIn, newChat, leaveSidebar, closeMenu, notice, prewake: () => prewake(), originOf,
  views: () => views,
  busy: (name) => refresh.isBusy(state.frames.get(name)),
  setupChat: () => onboarding.setupChat(),
});

const { computerOf, prewake, renderComputer, restartComputer, computerSection } = createComputer({
  api, el, seg, named, section, line, actionLine, computer: () => state.computer,
  agents: () => state.agents, computerFailed: () => state.computerFailed,
  support: () => state.support, setupPending: () => onboarding.setupPending(),
  made: (c) => { state.computer = c; }, computerIs, reopenSettings, openScreen,
  reads: () => reads, load,
});
// The computer as the platform last answered: shown at once.
function computerIs(v) {
  if (!v?.computer) return;
  state.computer = v;
  renderComputer();
  renderChats();
  rosterChanged();
  refreshSimplex();
}

// ---- where the page is: `/` its chats, `/settings` its settings, `/apps`
// its apps, `/welcome` a new person's welcome; each view says so in the
// address, so a reload or a link stays put ----
const SETTINGS = "/settings";
const APPS = "/apps";
const WELCOME = "/welcome";
function at(path) {
  if (location.pathname !== path) history.pushState(null, "", path);
}
addEventListener("popstate", () => {
  const shown = !$("layout").hidden;
  if (shown && location.pathname === SETTINGS) openSettings(false).catch((e) => notice("Settings did not open", e.message));
  else if (shown && location.pathname === APPS) openApps(false);
  else if (shown && location.pathname === "/" && byName(state.current)) openChat(state.current, false);
  else start().catch((e) => notice("This page did not load", e.message));
});

// ---- the Apps page (apps-page.js): every fragment of theirs but chats
// and agents, its dock, card, icon and who has access, in the work area
// as settings are ----
const appsPage = createAppsPage($("apps-page"), {
  api,
  el,
  icon,
  named,
  titleOf,
  art: (name) => iconOf(name),
  share: (name) => share(name),
  iconSheet: (name) => openIconSheet(iconHelpers, name, titleOf(name), byName(name)),
  retitle,
  originOf,
  openApp,
  reload: () => load(),
  state,
});
function openApps(push = true) {
  if (push) at(APPS);
  hideNotice();
  state.page = "Apps";
  $("layout").classList.add("settings-view");
  $("frames").hidden = true;
  $("settings-page").hidden = true;
  $("apps-page").hidden = false;
  renderHeading();
  renderChats();
  leaveSidebar();
  appsPage.open().catch((e) => notice("Apps did not open", e.message));
}
// a chat or settings in its place
function leaveApps() {
  $("apps-page").hidden = true;
  appsPage.close();
}
$("apps-link").onclick = () => openApps();

const settings = createSettings({
  api, $, seg,
  account: { person: () => state.me, setPerson: (p) => { state.me = p; }, isGuest: () => state.guest, setGuest,
    setSeat: (seat) => { state.seat = seat; }, support: () => state.support, seatIs,
    switchForm: (...args) => switchForm(...args), noSeatSaid: () => noSeatSaid() },
  navigation: { at, path: SETTINGS, isOpen: () => state.page === "Settings",
    enter: () => {
      if (state.page !== "Settings") views++;
      state.page = "Settings";
      $("layout").classList.add("settings-view");
      $("frames").hidden = true;
      leaveApps();
    },
    entered: () => { renderHeading(); renderChats(); leaveSidebar(); } },
  notices: { hideNotice, noticeCurrent: (gen) => gen === noticeGen, notice },
  sections: { computerSection, connectionsSection: (...args) => connectionsSection(...args), simplexSection },
  agents: { getComputer: () => state.computer, getAgents: () => state.agents, faceControls, agentSettings, addAgentForm, renderAgentFaces, paintHomes },
  releaseInfo: () => ({ mine: release.mine, deployed: DEPLOYED }), beginWrite: () => { writing++; return () => { writing--; }; },
  heard: (r) => heard(r), allFragments: () => state.fragments, own, freeLabel, load, named, openApp,
});
function openSettings(push = true, failed = null) { return settings.openSettings(push, failed); }
function reopenSettings(failed = null) { return settings.reopenSettings(failed); }
function skillsFragment() { return settings.skillsFragment(); }
$("settings").onclick = () => openSettings().catch((e) => notice("Settings did not open", e.message));

const { connectionsSection } = createConnections({
  api, el, section, named, seg, computer: () => state.computer, getAgents: () => state.agents,
  computerIs: (v) => {
    computerIs(v);
    state.agents = new Map((state.computer?.agents ?? []).map((x) => [x.identity, x]));
  },
  visitOf: () => settings.visit(), isOpen: () => state.page === "Settings", reopenSettings,
});

const onboarding = createOnboarding({
  api, ui: { $, el, notice },
  account: { person: () => state.me, isGuest: () => state.guest, setGuest, hasInvites: () => state.invites.length },
  fragments: { allFragments: () => state.fragments, chats, own, labelOf, titleOf, primaryAgent, agentsOwnChat, freeLabel, slug, seg },
  computer: { computerView: () => state.computer, computerFailed: () => state.computerFailed, agents: () => state.agents, computerOf, computerIs },
  navigation: { start, openChat, atHome: () => location.pathname === "/" && !state.page, hasFrame: (name) => state.frames.has(name), welcomePath: WELCOME },
  reads: () => reads, load, skillsFragment, rosterChanged,
});
const { signIn, showLayout, welcome, switchForm, noSeatSaid, setupFor, owesFirstAgent, resumeFirstRun, creatingAgent } = onboarding;

// ---- the person's things, read again whenever they may have changed ----
// One read of their list is the sidebar: each chat's row names its agents
// and its newest message. After a change their list told this page of
// (`watchList`: `changed`), only the apps whose rows it touched (new, or
// not as they were) have their card read again. An older read's answer
// that comes after a newer one's is not shown.
// Cloudflare can keep busy objects on the previous release for minutes.
// Retry only a confirmed release mismatch, at 3, 6, 12, 24, then 30s,
// bounded by eight minutes; other failures keep their usual Try again.
const READ_DEPLOY_WAIT_MS = 8 * 60_000;
const reads = { asked: 0, shown: 0, since: null, wait: 3000, timer: null, updating: false, listFailed: false };
async function shellRead(path) {
  try { return { value: await api("GET", path) }; }
  catch (error) { return { error, deploying: error.status === 503 && error.code === "deploying" }; }
}
async function load(changed = false) {
  const mine = ++reads.asked;
  const [listRead, computerRead] = await Promise.all([shellRead("/api/fragments"), shellRead("/api/computers")]);
  if (mine < reads.shown) return;
  reads.shown = mine;
  clearTimeout(reads.timer);
  // A list's ordinary failure still goes to its caller, as before.
  if (listRead.error && !listRead.deploying) {
    reads.updating = false;
    renderComputer();
    throw listRead.error;
  }
  const deploying = listRead.deploying || computerRead.deploying;
  if (deploying) reads.since ??= Date.now();
  else { reads.since = null; reads.wait = 3000; }
  const left = deploying ? READ_DEPLOY_WAIT_MS - (Date.now() - reads.since) : 0;
  reads.updating = left > 0 && ![listRead, computerRead].some((r) => r.error && !r.deploying);
  reads.listFailed = !!listRead.error;
  if (reads.updating) {
    reads.timer = setTimeout(() => load().catch(() => {}), Math.min(reads.wait, left));
    reads.wait = Math.min(reads.wait * 2, 30_000);
  }
  const list = listRead.value, computers = computerRead.value;
  const before = new Map(state.fragments.map((f) => [f.name, JSON.stringify(f)]));
  if (list) state.fragments = list.fragments ?? [];
  // failed, it is not "no computer": what was read before stays, and is said to be old
  const unknown = state.computerFailed && !state.computer;
  state.computerFailed = !computers;
  if (computers) {
    state.computer = computers.computers?.[0] ?? null;
    state.support = computers.support ?? null;
    state.agents = new Map((state.computer?.agents ?? []).map((a) => [a.identity, a]));
  }
  renderChats();
  threads.refresh().catch(() => {});
  renderApps();
  appsPage.listed().catch(() => {});
  renderHeading();
  renderComputer();
  rosterChanged();
  refreshSimplex();
  markSeen();
  launcher.agentsRead();
  renderAgentFaces();
  loadHomes();
  for (const f of chats()) if (own(f) && f.title === UNTITLED && f.preview) titleFromFirst(f.name);
  // answered, it goes on with what a failure put off, whoever asked
  if (computers && unknown) reopenSettings().catch(() => {});
  if (list && computers && onboarding.putOff()) resumeFirstRun();
  const touched = changed ? new Set(state.fragments.filter((f) => before.get(f.name) !== JSON.stringify(f)).map((f) => f.name)) : null;
  cardsLoad.wait = 0;
  loadCards(touched).catch(() => {});
}

// ---- live: the person's list tells this page when it changes ----
// An app their agent makes, one someone shares with them, a delete or an
// archive elsewhere (Paul on p5, 2026-10-05: his agent's app showed only
// after a reload), a chat's new message: their Principal tells this page's socket
// (`GET /api/fragments/watch`, which names nothing), and a moment after,
// so a burst is one read, the page reads the list again and patches the
// sidebar in place. A socket that closes is opened again after a jittered
// wait (1 s, doubling to 30 s); each opening reads the list again, for
// what changed while it was shut. One not open within LIST_OPEN_WAIT_MS
// (and its wait) is closed, and so opened again: an upgrade never answered
// would otherwise hold the page's one socket for good (the fragment
// library's OPEN_WAIT_MS, cell/client.mjs, says why ten seconds).
const LIST_SETTLE_MS = 200;
const LIST_OPEN_WAIT_MS = 10_000;
function listChanged() {
  clearTimeout(watching.settle);
  watching.settle = setTimeout(() => load(true).catch(() => {}), LIST_SETTLE_MS);
}
function watchList() {
  watching.on = true;
  clearTimeout(watching.timer);
  if (watching.socket) return;
  const ws = new WebSocket(`${location.protocol === "https:" ? "wss:" : "ws:"}//${location.host}/api/fragments/watch`);
  watching.socket = ws;
  const opening = setTimeout(() => ws.close(), LIST_OPEN_WAIT_MS + watching.wait);
  ws.onopen = () => {
    clearTimeout(opening);
    watching.wait = 1000;
    listChanged();
    loadInvites().catch(() => {});
  };
  ws.onmessage = (e) => {
    try {
      const type = JSON.parse(e.data).type;
      if (type === "changed") listChanged();
      // the invites waiting on their emails (the registry's), and only those
      else if (type === "invited") loadInvites().catch(() => {});
    } catch {}
  };
  ws.onclose = () => {
    clearTimeout(opening);
    watching.socket = null;
    // what the agent is doing goes until the socket is back to hear it
    renderChats();
    watching.timer = setTimeout(watchList, watching.wait * (0.5 + Math.random()));
    watching.wait = Math.min(watching.wait * 2, 30_000);
  };
}
// back on the page, a socket waiting to open again opens now
document.addEventListener("visibilitychange", () => {
  if (!document.hidden && watching.on && !watching.socket) watchList();
});

const refresh = createSessionRefresh({
  held: () => !menu.hidden || writing > 0 || composing.size > 0 || launcher.holds() || starting,
  getFrames: () => state.frames.values(), personId: () => state.me?.id,
  viewerKeys: () => viewer.keys, validFragment: (name) => FRAGMENT_NAME.test(name), openApp, originOf,
  endAlerts: () => { alertsEnded = true; }, dropAlerts,
});
const { heard, navigated, reopenWindows, release, DEPLOYED } = refresh;
const frameOfWindow = (win) => [...state.frames.values()].find((f) => f.contentWindow === win);
addEventListener("message", (event) => {
  const d = event.data;
  if (d?.fragment !== "agents?" && d?.fragment !== "busy") return;
  const frame = frameOfWindow(event.source);
  if (!frame) return;
  if (d.fragment === "agents?") {
    refresh.greet(frame);
  } else if (d.busy === true) {
    refresh.busy(frame, true);
    // busy as the reload is under way (a file picked, a memo begun, a
    // message failed while its answer was slow): the reload is stopped

  } else refresh.busy(frame, false);
});

async function start(open) {
  let me;
  try {
    me = await api("GET", "/api/identities/me");
  } catch (e) {
    if (e.status === 401) return signIn();
    throw e;
  }
  state.me = me;
  const [, mine] = await Promise.all([load(), api("GET", "/api/seat").catch(() => null), loadInvites()]);
  setGuest(!!mine?.guest);
  watchList();
  if (!open && location.pathname === WELCOME) return welcome();
  // the first agent is made at home; settings open as asked, chats or not;
  // a guest makes nothing (decision 49): with nothing shared with them, nor
  // an invite waiting, their home is the welcome; once something is (an
  // invitee's), it is what is shared with them, and a seat offered. A seat
  // of theirs (not the deployment's plan) may reach their ledger a moment
  // after it is given: its first run waits for it (`seatFirst`)
  // settings or the Apps page, as the address asks
  const settings = !open && (location.pathname === SETTINGS || location.pathname === APPS);
  if (!open && !settings && await owesFirstAgent()) return creatingAgent(mine?.seat ? {} : null);
  if (!state.fragments.length && !state.invites.length && !open && !settings && state.guest) return welcome();
  showLayout();
  const pick = open ?? (byName(state.current) ? state.current : (shown(chats())[0] ?? chats()[0])?.name);
  if (settings && location.pathname === APPS) openApps(false);
  else if (settings) await openSettings(false);
  else if (pick) openChat(pick);
  else if (state.guest) guestHome(mine.offer);
  else notice("No chats yet", "Make an agent to start.");
  // an Add a reload cut short goes on from what it made (`resumeFirstRun`)
  if (!settings) resumeFirstRun().catch(() => {});
  // an invite waits, and nothing it shares is open yet: a phone's sidebar,
  // where it is answered, shows over the page
  if (!settings && !pick && state.invites.length && layout.narrow) layout.show("left");
  // after a reload for a new release, its windows where they were
  reopenWindows(settings);
  prewake();
}

// ---- a guest (`GET /api/seat`'s `guest`: no seat, and no plan of their
// own) sees and edits what is shared with them, and makes nothing
// (decision 49), so no button offers to make something ----
function setGuest(guest) {
  state.guest = guest;
  renderChats();
  renderApps();
  renderHeading();
}
// With no chat to show: the first app shared with them in its window, and
// beside it what a guest may do, what a seat gives (the welcome's; where
// nothing is sold, that seats are not open, and no Get a seat), and who is
// signed in, with another account.
function guestHome(offer) {
  const app = shown(apps())[0];
  if (app) openApp(app.name);
  const seat = el("a", "allow", "Get a seat");
  seat.href = WELCOME;
  seat.onclick = (e) => {
    e.preventDefault();
    at(WELCOME);
    welcome().catch((err) => notice("This page did not load", err.message));
  };
  notice("You're a guest", [`You see and edit what people share with you, once you accept their invite in the sidebar. ${seatGives(offer)}`, noSeatSaid()], [...(offer ? [seat] : []), switchForm("/", "allow")]);
}

// a chat's alert clicked with no shell open (alerts-sw.js): that chat
const alerted = location.pathname === "/" ? new URLSearchParams(location.search).get("chat") : null;
if (alerted) {
  state.current = alerted;
  history.replaceState(null, "", "/");
}
start().catch((e) => {
  showLayout();
  notice("This page did not load", e.message);
});
