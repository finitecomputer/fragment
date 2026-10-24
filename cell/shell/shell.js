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
import { appIcon, icon } from "./icons.js";
import { createLayout, store, reducedMotion, springFrames } from "./layout.js";
import { createViewer } from "./viewer.js";
import { billingSections, returnSaid, returning, seatGives, seatOffer } from "./billing.js";

const $ = (id) => document.getElementById(id);
// New agents run on the cheap tier: GLM-5.3 Flash, DeepSeek V4 Flash its fallback (Paul, 2026-10-09).
const DEFAULT_AGENT_TIER = "cheap";
// A new agent's name, when its maker names none (Paul, 2026-10-03): the
// beige boxes of an alternate 1990s, never a real one's.
const NAMES = ["XBT-2000", "Starfire 40K", "Turbo Quasar 486", "Novatron DX", "Hyperion 9000", "Cobalt Prism 66", "Megastation LX", "Orbitron 3D", "Datastar Pro", "Pulsar 360", "Zephyr XL", "Titan MX", "Vortex 7", "Nimbus 4K", "Galaxion SE", "Powerframe 99"];
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
// invites: those waiting on their emails, theirs to answer (`GET /api/invites`)
const state = { me: null, guest: false, fragments: [], invites: [], computer: null, agents: new Map(), computerFailed: false, current: store.get(CURRENT, null), frames: new Map(), page: null };

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

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
const chats = () => state.fragments.filter((f) => f.kind === "chat");
const apps = () => state.fragments.filter((f) => f.kind === "app" || f.kind === "brain");
// the sidebar's: what the person has not archived (search still finds the rest)
const shown = (list) => list.filter((f) => !f.archived);
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
  frame.src = src;
  frame.allow = "clipboard-write; microphone";
  if (name) frame.dataset.fragment = name;
  frame.dataset.src = src;
  frame.addEventListener("load", () => theme(frame));
  return frame;
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
  greeted.delete(frame);
  busyFrames.delete(frame);
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
    greeted.delete(frame);
    busyFrames.delete(frame);
    frame.src = frame.dataset.src;
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
  const now = JSON.stringify([roster(), state.computer?.models, setup && [setup.chat, setupFor(setup.chat)]]);
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
    if (name === setup?.chat && setup.error) creatingAgent();
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
    const who = titleOf(agent.fragment);
    $("add-agent-title").textContent = `Add ${who}?`;
    $("add-agent-text").textContent = `Add ${who} to ${titleOf(name)}? ${who} will be able to read and edit it.`;
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
// of one of the person's chats asks, from its fragment's own origin ----
const ALERTS_WORKER = "/__shell/alerts-sw.js";
const ALERTS_SCOPE = "/__shell/alerts/";
const alertsScope = (name) => new URL(`${ALERTS_SCOPE}${encodeURIComponent(name)}/`, location.origin).href;
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
  if (!["alerts?", "alerts"].includes(d?.fragment) || typeof d.nonce !== "string" || d.nonce.length > 64) return;
  const frame = [...document.querySelectorAll("iframe[data-fragment]")].find((f) => f.contentWindow === event.source);
  const name = frame?.dataset.fragment;
  if (!name || byName(name)?.kind !== "chat") return;
  let origin;
  try {
    origin = await originOf(name);
  } catch {
    return;
  }
  if (event.origin !== origin || frame.contentWindow !== event.source) return;
  const answer = (a) => event.source.postMessage({ fragment: "alerts", nonce: d.nonce, ...a }, origin);
  if (!alertable()) return answer({ state: "unsupported" });
  try {
    if (d.fragment === "alerts?") {
      if (Notification.permission === "denied") return answer({ state: "denied" });
      const on = Notification.permission === "granted" && !!(await (await alertsOf(name))?.pushManager.getSubscription());
      return answer({ state: on ? "on" : "off" });
    }
    const key = d.on === true && typeof d.key === "string" ? vapidKey(d.key) : null;
    if (key) return answer(await alertsOn(name, key));
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
  frame.setAttribute("aria-label", someoneElses(name) ? `Who has access to ${titleOf(name)}` : `Share ${titleOf(name)}`);
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
  // settings' Sharing says what the sheet may have changed
  const shown = $("settings-access");
  if (shown && state.page === "Settings") api("GET", "/api/access").then((a) => shown.replaceWith(accessSection(a))).catch(() => {});
};

// ---- one menu, for whichever button asked ----
const menu = $("menu");
const menuEdge = el("div", "agent-menu-edge");
menuEdge.hidden = true;
menuEdge.setAttribute("aria-hidden", "true");
document.body.append(menuEdge);
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
  menuEdge.hidden = true;
  menu.inert = false;
  menuClosing = false;
  menu.classList.remove("agent-menu");
  menu.style.removeProperty("width");
  menu.style.removeProperty("max-height");
  updateSidebarEdge();
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
function openMenu(button, items) {
  const again = menuFor?.button === button && !menuClosing;
  if (again) return closeMenu();
  closeMenu(true);
  menuFor = { button };
  menu.replaceChildren(...items.map((it) => {
    const b = el("button");
    b.type = "button";
    b.setAttribute("role", "menuitem");
    b.replaceChildren(icon(it.icon));
    b.append(it.text);
    b.onclick = () => { closeMenu(); it.onClick(); };
    return b;
  }));
  button.setAttribute("aria-expanded", "true");
  const attached = button.id === "sidebar-agent";
  menu.classList.toggle("agent-menu", attached);
  menu.hidden = false;
  const r = button.getBoundingClientRect();
  if (attached) {
    menu.style.width = `${r.width}px`;
    menu.style.maxHeight = `${Math.max(0, innerHeight - r.bottom - 8)}px`;
    menu.style.top = `${r.bottom - 1}px`;
    menu.style.left = `${r.left}px`;
    menuEdge.style.left = `${r.left}px`;
    menuEdge.style.top = `${r.top}px`;
    menuEdge.style.width = `${r.width}px`;
    menuEdge.style.height = `${menu.getBoundingClientRect().bottom - r.top}px`;
    menuEdge.hidden = false;
  } else {
    menu.style.top = `${Math.min(r.bottom + 4, innerHeight - menu.offsetHeight - 8)}px`;
    menu.style.left = `${Math.max(8, Math.min(r.left, innerWidth - menu.offsetWidth - 8))}px`;
  }
  menu.querySelector("button")?.focus();
  updateSidebarEdge();
  const bounds = attached ? menuEdge.getBoundingClientRect() : menu.getBoundingClientRect();
  menuParts = (attached ? [menu, button, menuEdge] : [menu]).map(node => {
    const rect = node.getBoundingClientRect();
    return { node, origin: `${bounds.left + bounds.width / 2 - rect.left}px ${bounds.top + bounds.height / 2 - rect.top}px` };
  });
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

// ---- chats: one flat list, newest first, the open one in the middle column ----
function renderHeading() {
  const open = state.current && byName(state.current) && !state.page;
  $("chat-heading").hidden = !open;
  $("page-title").hidden = !state.page;
  $("page-title").textContent = state.page ?? "";
  $("new-chat").hidden = !open || state.guest || !primaryAgent()?.identity;
  $("chat-options").hidden = !open;
  if (!open) return;
  const title = titleOf(state.current);
  $("chat-title").textContent = title;
  $("chat-title").title = title;
  const frame = state.frames.get(state.current);
  if (frame) frame.setAttribute("aria-label", title);
}
// A chat's unread, as its row shows them: none on the open one (it is
// being seen), and past the count's bound "99+" (the list counts at most
// UNREAD_MAX: proto's `LISTED_UNREAD_MAX`, held to it by a host test,
// xtask/src/guards.rs `the_shell_copies_its_limits`).
const UNREAD_MAX = 100;
function unreadMark(f) {
  const n = f.name === state.current && !state.page ? 0 : f.unread ?? 0;
  if (!n) return [];
  const b = el("span", "unread", n >= UNREAD_MAX ? "99+" : String(n));
  b.setAttribute("aria-label", n >= UNREAD_MAX ? "More than 99 unread" : `${n} unread`);
  return [b];
}
// The person's one agent (Paul, 2026-10-13: "just one", a setting later):
// their first, the one the first run makes (`defaultAgent`): the first
// their computer took (its view lists its agents in the order it took
// them), or, before it took one, their agent fragment. New chats are with
// it. Any other agents they have stay out of the sidebar; their chats are
// chats, so they list and search as any other.
function primaryAgent() {
  const first = [...state.agents.values()].find((a) => byName(a.fragment)?.role === "owner");
  if (first) return { fragment: first.fragment, identity: first.identity };
  const f = state.fragments.find((x) => x.kind === "agent" && own(x));
  return f ? { fragment: f.name, identity: null } : null;
}
// What the person's agents are doing, from their chats' rows in the list
// (docs/api.md, "Listed records"): each agent's newest record of a turn's
// kind on a chat's `work` (docs/chat-records.md), which the shell reads as
// the chat's page reads a turn. Started, a step, or a card answered is
// working; a card open waits for the person (its owner answers its cards);
// a question waits on whoever it asks (`asks`); an end is nothing. A
// message waiting for its turn has no record yet, so queued is not said.
// None while their computer is not awake (its turns stop with it) or this
// page's list socket is shut (it would not hear the change), and none past
// the bridge's own bound on a turn that says nothing (15 minutes; a turn
// waiting on someone waits as long as a card, an hour), each a minute more
// for the list's own lag. Those three are copies, named as their sources
// name them (proto's limits; images/bridge/src/limits.rs), each held to its
// source by a host test (xtask/src/guards.rs, `the_shell_copies_its_limits`).
// The list's turn kinds are the chat's records (docs/chat-records.md):
// the shell reads them as the bridge writes them. A listed field's
// value is any text an agent wrote (up to 128 characters): `asks` is only
// compared with the person's id here, never shown; what is shown is these
// words and the chat's title, as text (`el`'s textContent), never markup.
const DOING_SAYS = { you: "Waiting for you", waiting: "Waiting", working: "Working" };
const DOING_ORDER = Object.keys(DOING_SAYS);
const DOING_OF_KIND = { "turn.start": "working", "turn.step": "working", "turn.prompt.closed": "working", "turn.prompt": "you", "turn.asked": "asked" };
const LISTED_FRESH_MS = 60_000;
const TURN_IDLE_MS_MAX = 15 * 60_000;
const PROMPT_TTL_MS_DEFAULT = 60 * 60_000;
const DOING_QUIET_MS = { working: TURN_IDLE_MS_MAX + LISTED_FRESH_MS, waiting: PROMPT_TTL_MS_DEFAULT + LISTED_FRESH_MS };
// the chats its menu names, at most
const DOING_LISTED = 5;
// the list socket (`watchList`, below): declared here, as the sidebar reads it
const watching = { on: false, socket: null, wait: 1000, timer: null, settle: null };
const listLive = () => watching.socket?.readyState === WebSocket.OPEN;
// The chats where agent `identity` is doing something, the most pressing
// first (waiting for the person, then on someone, then working), the
// newest first within each; each with when it stops counting (`until`).
function doingChats(identity) {
  if (!identity || !listLive() || state.computer?.phase !== "awake") return [];
  const now = Date.now();
  const out = [];
  for (const f of chats()) {
    for (const r of f.listed ?? []) {
      const kind = r.channel === "work" && r.principal === identity ? DOING_OF_KIND[r.kind] : undefined;
      if (!kind) continue;
      const doing = kind === "asked" ? (r.fields?.asks === state.me?.id ? "you" : "waiting") : kind;
      const until = r.at + DOING_QUIET_MS[doing === "working" ? "working" : "waiting"];
      if (until > now) out.push({ chat: f.name, doing, at: r.at, until });
    }
  }
  return out.sort((x, y) => DOING_ORDER.indexOf(x.doing) - DOING_ORDER.indexOf(y.doing) || y.at - x.at);
}
// The sidebar says it again as the first of what it shows stops counting.
let doingTimer = 0;
function doingAgain(doing) {
  clearTimeout(doingTimer);
  const next = Math.min(...doing.map((x) => x.until));
  if (Number.isFinite(next)) doingTimer = setTimeout(renderChats, Math.max(1000, next - Date.now()));
}
// Newest first: a chat by when its newest message was said (its row's
// `previewAt`); one with nothing said yet (just made) above them all.
const newest = (a, b) => (b.previewAt ?? Infinity) - (a.previewAt ?? Infinity) || (a.name < b.name ? -1 : 1);
function renderChats() {
  const primary = primaryAgent();
  const agentRows = [];
  if (primary) {
    const doing = doingChats(primary.identity);
    doingAgain(doing);
    const says = DOING_SAYS[doing[0]?.doing];
    const row = el("button", "agent-heading sidebar-agent");
    row.id = "sidebar-agent";
    row.type = "button";
    row.dataset.key = primary.fragment;
    row.setAttribute("aria-label", `${titleOf(primary.fragment)}${says ? `, ${says.toLowerCase()}` : ""}: your agent's menu`);
    row.setAttribute("aria-haspopup", "menu");
    row.setAttribute("aria-controls", "menu");
    row.setAttribute("aria-expanded", String(!!menuFor && menuFor.button === $("sidebar-agent")));
    const face = el("span", "sidebar-agent-mark");
    face.append(avatar(agentFace(primary.fragment, primary.identity), "small"));
    // its name, and under it what it is doing, so the name keeps its room
    const named = el("span", "sidebar-agent-named");
    named.append(el("strong", "title", titleOf(primary.fragment)));
    if (says) named.append(el("span", `agent-doing ${doing[0].doing}`, says));
    row.append(face, named);
    row.onclick = () => openPrimaryMenu(row);
    agentRows.push(row);
  }
  patch($("primary-agent"), agentRows);
  // the launcher: a guest's searches; a seat's also starts a chat
  const launcherLabel = state.guest ? "Search chats and apps" : "Search or start a new chat";
  for (const id of ["sidebar-new-chat", "new-chat"]) {
    $(id).title = `${launcherLabel} (⌘K)`;
    $(id).dataset.tooltip = `${launcherLabel} (⌘K)`;
    $(id).setAttribute("aria-label", launcherLabel);
  }
  const rows = shown(chats()).sort(newest).map((f) => {
    const active = f.name === state.current && !state.page;
    const row = el("button", `row chat-row${active ? " active" : ""}`);
    row.type = "button";
    row.dataset.key = `chat:${f.name}`;
    row.setAttribute("aria-current", active ? "page" : "false");
    row.append(el("span", "label", titleOf(f.name)), ...unreadMark(f));
    row.onclick = () => { openChat(f.name); leaveSidebar(); };
    return row;
  });
  patch($("chats"), rows.length ? rows : [el("div", "empty-row", chats().length ? "Every chat is archived" : state.guest ? "Chats shared with you show here" : "Your chats will appear here")]);
}

// ---- invites: shared by email, and theirs only once they accept (decision 63) ----
// The invites waiting on the emails the person signs in with, above their
// chats: what each shares, who shared it, and as what, with Accept and
// Decline. Read as the page starts, as its list's socket opens, and when
// the socket says they changed (`invited`: one made, renewed or taken
// back). Accepted, it is in their list, and opens; declined, it goes.
const invitesRead = { asked: 0, shown: 0 };
async function loadInvites() {
  const mine = ++invitesRead.asked;
  // failed, what was read before stays: a failure is not "none"
  const got = await api("GET", "/api/invites").catch(() => null);
  if (!got || mine < invitesRead.shown) return;
  invitesRead.shown = mine;
  state.invites = got.invites ?? [];
  renderInvites();
}
function renderInvites() {
  const rows = state.invites.map((i) => {
    const box = el("div", "invite");
    box.dataset.key = `invite:${i.fragment}`;
    box.dataset.invite = i.fragment;
    const may = i.role === "editor" ? "edit" : "view";
    box.append(el("strong", "invite-title", i.title), el("span", "invite-from", `${i.from ?? "Someone"} invited you to ${may} it`));
    const actions = el("div", "invite-actions");
    actions.append(inviteButton(i, "accept", "Accept"), inviteButton(i, "decline", "Decline"));
    box.append(actions);
    return box;
  });
  patch($("invites"), rows);
  $("invites").hidden = !rows.length;
}
// What it shares opens as its kind does: a chat in the middle, else a window.
const openShared = (name) => (byName(name)?.kind === "chat" ? openChat(name) : openApp(name));
function inviteButton(invite, verb, text) {
  const b = el("button", `invite-${verb}`, text);
  b.type = "button";
  b.onclick = async () => {
    const both = [...b.parentElement.children];
    for (const x of both) x.disabled = true;
    try {
      await api("POST", `/api/invites/${seg(invite.fragment)}/${verb}`, {});
      await Promise.all([load(), loadInvites()]);
      if (verb === "accept") { openShared(invite.fragment); leaveSidebar(); }
    } catch (e) {
      // gone (taken back, answered in another tab, run out): read again, and
      // one theirs already (accepted elsewhere) opens
      await Promise.all([load(), loadInvites()]).catch(() => {});
      if (e.status === 404 && verb === "accept" && byName(invite.fragment)) return openShared(invite.fragment);
      const gone = e.status === 404 ? "It isn't open any more: it was taken back, answered, or ran out." : "Try again in a moment.";
      notice(verb === "accept" ? "The invite wasn't accepted" : "The invite wasn't declined", gone);
      for (const x of both) x.disabled = false;
    }
  };
  return b;
}
// The agent's own menu: where it is doing something (each chat opens),
// another chat with it, its name, its profile and its screen. A chat's own
// (Rename, Invite, Archive) is the chat's ⋯. What it is doing is read as
// it opens, not from when its row was drawn.
function openPrimaryMenu(trigger) {
  const primary = primaryAgent();
  if (!primary) return;
  openMenu(trigger, [
    ...doingChats(primary.identity)
      .slice(0, DOING_LISTED)
      .map((x) => ({ icon: "agent", text: `${DOING_SAYS[x.doing]} in ${titleOf(x.chat)}`, onClick: () => { openChat(x.chat); leaveSidebar(); } })),
    ...(primary.identity && !state.guest ? [{ icon: "newChat", text: "New chat", onClick: () => newChat(primary.identity) }] : []),
    { icon: "rename", text: "Rename…", onClick: () => rename(primary.fragment) },
    { icon: "agent", text: "Its profile", onClick: () => openApp(primary.fragment) },
    ...(state.computer ? [{ icon: "screen", text: "Its screen", onClick: () => openScreen(primary.fragment) }] : []),
  ]);
}
$("sidebar-new-chat").onclick = () => openSearch($("sidebar-new-chat"));
// Match the chat composer's edge reflection, including its 64px falloff.
// Follow the pointer across the sidebar and its fixed-position agent menu.
const sidebarHoverPointer = matchMedia("(hover: hover) and (pointer: fine)");
let sidebarEdgeFrame = 0;
let sidebarEdgePointer = null;
const sidebarEdgeControls = () => $("sidebar").querySelectorAll(".sidebar-agent, .sidebar-new-chat");
// Keep transient styles off the rendered pill so roster updates preserve its node.
const expandedAgentEdge = control => control.id === "sidebar-agent" && !menu.hidden && menu.classList.contains("agent-menu");
const sidebarEdgeSurface = control => expandedAgentEdge(control) ? menuEdge : control.id === "sidebar-agent" ? $("primary-agent") : control;
function clearSidebarEdge() {
  cancelAnimationFrame(sidebarEdgeFrame);
  sidebarEdgeFrame = 0;
  for (const control of sidebarEdgeControls()) sidebarEdgeSurface(control).style.setProperty("--edge-light", "0");
}
function updateSidebarEdge() {
  if (!sidebarEdgePointer || !sidebarHoverPointer.matches) return;
  for (const control of sidebarEdgeControls()) {
    const bounds = control.getBoundingClientRect();
    const rect = { left: bounds.left, top: bounds.top, width: bounds.width, height: expandedAgentEdge(control) ? menu.getBoundingClientRect().bottom - bounds.top : bounds.height };
    const x = sidebarEdgePointer.x - rect.left, y = sidebarEdgePointer.y - rect.top;
    const radius = Math.min(rect.width / 2, rect.height / 2, parseFloat(getComputedStyle(control).borderTopLeftRadius));
    const qx = Math.abs(x - rect.width / 2) - rect.width / 2 + radius;
    const qy = Math.abs(y - rect.height / 2) - rect.height / 2 + radius;
    const distance = Math.max(0, Math.hypot(Math.max(qx, 0), Math.max(qy, 0)) + Math.min(Math.max(qx, qy), 0) - radius);
    const proximity = Math.max(0, 1 - distance / 64);
    const surface = sidebarEdgeSurface(control);
    surface.style.setProperty("--edge-x", `${x}px`);
    surface.style.setProperty("--edge-y", `${y}px`);
    surface.style.setProperty("--edge-light", String(proximity * proximity));
  }
}
document.addEventListener("pointermove", event => {
  if (!sidebarHoverPointer.matches || event.pointerType === "touch") return;
  sidebarEdgePointer = { x: event.clientX, y: event.clientY };
  if (sidebarEdgeFrame) return;
  sidebarEdgeFrame = requestAnimationFrame(() => {
    sidebarEdgeFrame = 0;
    updateSidebarEdge();
  });
});
document.addEventListener("pointerleave", clearSidebarEdge);
sidebarHoverPointer.addEventListener("change", clearSidebarEdge);
addEventListener("blur", clearSidebarEdge);
// The open chat, on screen, is seen (`PUT /api/fragments/{name}/seen`): its
// row counts nothing unread, here and in the person's other tabs and
// devices (their list tells them). Asked again whenever the list counts
// more on it (a reply that came while it was open), never while this page
// is hidden; one ask at a time.
let marking = null;
function markSeen() {
  const f = byName(state.current);
  if (!f || state.page || document.hidden || !f.unread || marking === f.name) return;
  marking = f.name;
  api("PUT", `/api/fragments/${seg(f.name)}/seen`)
    .then(() => {
      f.unread = 0;
      renderChats();
    })
    .catch(() => {})
    .finally(() => { marking = null; });
}
document.addEventListener("visibilitychange", markSeen);
// The open chat's ⋯: its name, its people, its agents' profiles and
// screens, and archiving. New chats are the launcher's, with the person's
// one agent (`primaryAgent`), whichever chat is open.
$("chat-options").onclick = () => openChatMenu(state.current, $("chat-options"));
function openChatMenu(name, trigger) {
  const group = isGroup(name);
  const agent = agentOfChat(name);
  // a group's agents each have a profile; a direct chat's agent has one
  const profiles = group
    ? agentsOf(name).map((id) => state.agents.get(id)?.fragment).filter(Boolean).map((a) => ({ icon: "agent", text: `${titleOf(a)}'s profile`, onClick: () => openApp(a) }))
    : agent ? [{ icon: "agent", text: "Its profile", onClick: () => openApp(agent) }] : [];
  // each agent's own desktop: a group's agents each have one to pick from
  const screens = !state.computer ? [] : group
    ? agentsOf(name).map((id) => state.agents.get(id)?.fragment).filter(Boolean).map((a) => ({ icon: "screen", text: `${titleOf(a)}'s screen`, onClick: () => openScreen(a) }))
    : agent ? [{ icon: "screen", text: "Its screen", onClick: () => openScreen(agent) }] : [];
  openMenu(trigger, [
    { icon: "rename", text: "Rename…", onClick: () => rename(name) },
    shareItem(name),
    ...profiles,
    ...screens,
    archiveItem(name),
  ]);
}
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

function openChat(name, push = true) {
  if (!byName(name)) return;
  if (push) at("/");
  state.page = null;
  $("layout").classList.remove("settings-view");
  $("settings-page").hidden = true;
  $("frames").hidden = false;
  state.current = name;
  store.set(CURRENT, name);
  hideNotice();
  if (!state.frames.has(name)) {
    const frame = frameOf(framed(name, setupFor(name) ? "/?setup=1" : "/"), titleOf(name), name);
    $("frames").append(frame);
    state.frames.set(name, frame);
  }
  for (const [n, frame] of state.frames) frame.hidden = n !== name;
  renderHeading();
  renderChats();
  markSeen();
  prewake();
  navigated();
  // home: a first run a failed read put off goes on from here too
  if (firstRunPutOff) resumeFirstRun();
}

// ---- apps: every other fragment, each a window in the viewer ----
function paneIcon(name) {
  const i = el("span", "pane-icon");
  i.replaceChildren(icon(name));
  return i;
}
const iconOf = (name) => appIcon(labelOf(name));
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
  const listed = shown(apps()).map((f) => f.name);
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
// The desktop's dock: the person's apps (their list's, less the archived),
// each its icon and title, one shared with them saying their role there;
// then, for a seat, Add (how to ask for one: `showPrompts`).
function renderApps() {
  hidePeek();
  const rows = shown(apps()).map((f) => {
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
  if (shown && path !== "/") shown.src = framed(name, path);
  const frame = frameOf(framed(name, path), titleOf(name), name);
  const status = el("span", "pane-sharing");
  if (f) status.append(...badges(f));
  // its own URL, for a tab of its own or the clipboard
  const at = originOf(name).then((o) => `${o}${path}`);
  at.catch(() => {});
  show({
    key: `app:${name}`, title: titleOf(name), subtitle: !f || own(f) ? undefined : f.role, icon: iconOf(name), body: frame, status,
    actions: [
      ...(f && own(f) ? [{ icon: "share", title: "Share…", onClick: () => share(name) }] : []),
      { icon: "folder", title: "Files", onClick: () => show({ key: `tree:${name}`, title: titleOf(name), subtitle: "files", icon: paneIcon("folder"), body: frameOf(framed(name, "/__files"), `${titleOf(name)} files`, name) }) },
      { icon: "reload", title: "Reload", onClick: () => { frame.src = framed(name, path); } },
      { icon: "external", title: "Open in a new tab", onClick: () => window.open(`/auth/fragment?name=${encodeURIComponent(name)}&return=${encodeURIComponent(path)}`, "_blank", "noopener") },
      {
        icon: "more", title: "More", onClick: (e) => openMenu(e.currentTarget, [
          { icon: "link", text: "Copy link", onClick: () => at.then((u) => navigator.clipboard.writeText(u)).catch((err) => notice("The link was not copied", err.message)) },
          // someone else's: who else is in it, and leaving it
          ...(f && !own(f) ? [shareItem(name)] : []),
          ...(f ? [archiveItem(name)] : []),
        ]),
      },
    ],
  });
  navigated();
}

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
    if (!taken.has(l) && !taken.has(`${l}-chat`)) return l;
  }
}
function pickName() {
  const taken = new Set(state.fragments.map((f) => titleOf(f.name)));
  const free = NAMES.filter((n) => !taken.has(n));
  return free.length ? free[Math.floor(Math.random() * free.length)] : `Unit ${state.fragments.length + 1}`;
}
// The agent speaks first: a routine on its own `tasks`, which its computer
// takes as a turn in its chat asked by its owner (docs/chat-records.md,
// `tasks`), so its chat opens with its hello and no message of the
// person's. Posted once it is in the chat, where its turn posts; its id
// makes a retry the same record.
const hello = (agent, chat) =>
  api("POST", `/api/f/${agent}/channels/tasks`, {
    id: "hello",
    body: { kind: "routine", chat, text: "You were just made, and this is your chat with the person you work for. Say hello in a sentence or two: who you are, and what you will do for them (your SOUL.md). Start no work until they answer." },
  });
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
async function newChat(identity, { quiet = false } = {}) {
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
$("new-chat").onclick = () => openSearch($("new-chat"));
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
  box.append(el("p", "app-prompts-lede", `${primary ? titleOf(primary.fragment) : "Your agent"} makes an app from a message. Say who it's for and what it does: start from one of these, make it yours, then send it.`));
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
  const theirs = shown(chats()).filter((f) => own(f) && agentsOf(f.name).includes(primary.identity)).sort(newest)[0];
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
  if (state.page || state.current !== name) openChat(name);
  const frame = state.frames.get(name);
  frame?.focus();
  const origin = frame && rosterTo.get(frame.contentWindow);
  if (origin) frame.contentWindow.postMessage({ fragment: "compose", text }, origin);
  else composing.set(name, text);
}

// ---- renaming: a chat's title, or an agent's (its name), each alone ----
// A fragment's title, its fragment.json's `meta.title`, deployed.
async function retitle(name, title, message) {
  const manifest = await api("GET", `/api/f/${seg(name)}/manifest`);
  manifest.meta = { ...(manifest.meta ?? {}), title };
  await api("POST", `/api/f/${seg(name)}/files`, { message, files: [{ path: "fragment.json", text: JSON.stringify(manifest, null, 2) + "\n" }] });
  await api("POST", `/api/f/${seg(name)}/deploy`, {});
}
let renaming = null;
function rename(name) {
  renaming = name;
  const agent = byName(name)?.kind === "agent";
  $("rename-title").textContent = agent ? "Name your agent" : "Name this chat";
  $("rename-label").textContent = agent ? "Agent name" : "Chat name";
  $("agent-name").value = titleOf(name);
  $("rename-error").hidden = true;
  $("rename-agent").showModal();
  $("agent-name").select();
}
$("rename-cancel").onclick = () => $("rename-agent").close();
$("rename-form").onsubmit = async (e) => {
  e.preventDefault();
  const title = $("agent-name").value.trim();
  if (!title || !renaming) return;
  try {
    await retitle(renaming, title, "rename");
    $("rename-agent").close();
    await load();
  } catch (err) {
    $("rename-error").textContent = err.message;
    $("rename-error").hidden = false;
  }
};

// ---- the launcher: one input for starting a chat or finding one again ----
// The sidebar's +, the phone's top bar + and ⌘K (Skyler, 2026-10-12).
// Empty stays quiet. Words ask the person's search (`GET /api/search`:
// their chats and apps by title and label, then messages, newest first,
// archived ones included): a chat found opens in the middle column, an app
// in the desktop, a message its chat. Enter sends those exact words as the
// first message of a new chat with their agent (`primaryAgent`), one record
// however often it is tried (its id, kept). A request for an app is a
// message like any other: their agent makes it.
const isChat = (name) => byName(name)?.kind === "chat";
function openFound(name) {
  closeSearch();
  if (isChat(name)) openChat(name); else openApp(name);
  leaveSidebar();
}
function foundRow(name, preview, extra) {
  const f = byName(name);
  const row = el("button", "row search-result");
  row.type = "button";
  row.dataset.fragment = name;
  const text = el("span", "agent-copy");
  text.append(el("span", "label", titleOf(name)));
  if (preview) text.append(el("span", "agent-preview", preview));
  row.append(isChat(name) ? mark(name, "small") : iconOf(name), text);
  if (f?.archived) row.append(el("span", "meta", "Archived"));
  if (extra) row.append(extra);
  row.onclick = () => openFound(name);
  return row;
}
const when = (at) => {
  const d = new Date(at);
  return d.toDateString() === new Date().toDateString() ? d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" }) : d.toLocaleDateString(undefined, { month: "short", day: "numeric" });
};
function renderFound(fragments, messages) {
  const results = $("search-results");
  results.replaceChildren();
  const q = $("workspace-search").value.trim();
  if (!q) return;
  if (canLaunch()) {
    const action = el("button", "row search-result search-start");
    action.type = "button";
    const mark = el("span", "search-action-icon");
    mark.append(icon("newChat"));
    const copy = el("span", "agent-copy");
    copy.append(el("span", "label", `Start a new chat with ${titleOf(state.agents.get(searchIdentity).fragment)}`), el("span", "agent-preview", q));
    const enter = el("span", "meta search-enter");
    enter.title = "Enter";
    enter.replaceChildren(icon("cornerDownLeft"));
    action.append(mark, copy, enter);
    action.onclick = startFromSearch;
    results.append(action);
  }
  const groups = [
    ["Chats", fragments.filter((f) => f.kind === "chat").map((f) => foundRow(f.name, f.preview ?? ""))],
    // the desktop's apps, as the dock lists them (`apps`)
    ["Apps", fragments.filter((f) => f.kind === "app" || f.kind === "brain").map((f) => foundRow(f.name, ""))],
    // a message in a fragment the list has not shown yet is left out until it has
    ["Messages", messages.filter((m) => byName(m.fragment)).map((m) => {
      const row = foundRow(m.fragment, m.snippet, el("span", "meta", when(m.at)));
      row.classList.add("search-message");
      row.dataset.seq = String(m.seq);
      return row;
    })],
  ];
  let count = 0;
  for (const [heading, rows] of groups) {
    if (!rows.length) continue;
    count += rows.length;
    results.append(el("h3", null, heading), ...rows);
  }
  // a seat's agent not made yet starts nothing: said, not offered
  const waits = !state.guest && !canLaunch() ? "A new chat can start once your agent is made." : "";
  $("search-status").textContent = [count || canLaunch() ? "" : "Nothing found.", waits].filter(Boolean).join(" ");
}
let asked = 0;
let typing = null;
let searchIdentity = null;
let searchTrigger = null;
let launching = false;
let closingSearch = false;
// Keep the same chat and record id after an uncertain send, so retry cannot
// duplicate the first message. This is only the current dialog's draft.
let pendingLaunch = null;
const canLaunch = () => !state.guest && !!searchIdentity && state.agents.has(searchIdentity);
async function search() {
  const q = $("workspace-search").value.trim();
  const mine = ++asked;
  if (!q) return renderFound([], []);
  try {
    // A prompt can be longer than the search API's eight words / 256 bytes.
    // Only the search hint is bounded; the complete prompt is sent to the chat.
    const hint = [...q.split(/\s+/).slice(0, 8).join(" ")];
    while (new TextEncoder().encode(hint.join("")).length > 256) hint.pop();
    const found = await api("GET", `/api/search?q=${encodeURIComponent(hint.join(""))}`);
    // a slower answer to an older query never replaces a newer one's
    if (mine === asked) renderFound(found.fragments ?? [], found.messages ?? []);
  } catch (e) {
    if (mine === asked) $("search-status").textContent = e.message;
  }
}
// `text`: what a suggestion asks, in place of the last unsent one
function openSearch(trigger, text = null) {
  if ($("search-dialog").open || launching) return;
  closeMenu(true);
  searchTrigger = trigger;
  searchIdentity = primaryAgent()?.identity ?? null;
  $("workspace-search").value = text ?? pendingLaunch?.text ?? "";
  updateSearch();
  $("search-dialog").showModal();
  trigger?.setAttribute("aria-expanded", "true");
  $("workspace-search").focus();
}
async function closeSearch(force = false) {
  const dialog = $("search-dialog");
  if (!dialog.open || closingSearch || launching && !force) return;
  closingSearch = true;
  ++asked;
  clearTimeout(typing);
  const panel = dialog.querySelector(".search-panel");
  // Start at the current visual state, even if Escape interrupts the entrance.
  const animations = matchMedia("(prefers-reduced-motion: reduce)").matches ? [] : [
    dialog.animate([{ opacity: getComputedStyle(dialog).opacity }, { opacity: 0 }], { duration: 280, easing: "cubic-bezier(.4, 0, 1, 1)", fill: "forwards" }),
    panel.animate([{ opacity: getComputedStyle(panel).opacity, transform: getComputedStyle(panel).transform }, { opacity: 0, transform: "translateY(8px)" }], { duration: 240, easing: "cubic-bezier(.4, 0, 1, 1)", fill: "forwards" }),
  ];
  dialog.inert = true;
  await Promise.allSettled(animations.map(animation => animation.finished));
  dialog.close();
  for (const animation of animations) animation.cancel();
  dialog.inert = false;
  closingSearch = false;
}
async function startFromSearch() {
  const text = $("workspace-search").value.trim();
  if (!text || !canLaunch() || launching || closingSearch) return;
  launching = true;
  ++asked;
  clearTimeout(typing);
  $("workspace-search").readOnly = true;
  $("search-send").disabled = true;
  $("search-results").inert = true;
  $("search-status").textContent = "Starting your chat…";
  try {
    if (!pendingLaunch || pendingLaunch.text !== text || pendingLaunch.identity !== searchIdentity) pendingLaunch = { text, identity: searchIdentity, id: crypto.randomUUID(), chat: null };
    pendingLaunch.chat ||= await newChat(searchIdentity, { quiet: true });
    if (!pendingLaunch.chat) throw new Error("Another new chat is being made; try again in a moment.");
    await api("POST", `/api/f/${seg(pendingLaunch.chat)}/channels/chat`, { id: pendingLaunch.id, body: { text } });
    openChat(pendingLaunch.chat);
    pendingLaunch = null;
    await closeSearch(true);
    leaveSidebar();
  } catch (e) {
    $("search-status").textContent = `Your message wasn't sent. ${e.message}`;
  } finally {
    launching = false;
    $("workspace-search").readOnly = false;
    $("search-results").inert = false;
    $("search-send").disabled = !canLaunch() || !$("workspace-search").value.trim();
    if ($("search-dialog").open) $("workspace-search").focus();
  }
}
function updateSearch() {
  ++asked; // Invalidate in-flight responses immediately, before the debounce.
  clearTimeout(typing);
  $("search-status").textContent = "";
  $("search-send").disabled = !canLaunch() || !$("workspace-search").value.trim();
  renderFound([], []);
  if ($("workspace-search").value.trim()) typing = setTimeout(search, 150);
}
for (const id of ["sidebar-new-chat", "new-chat"]) {
  $(id).setAttribute("aria-keyshortcuts", "Meta+K Control+K");
  $(id).dataset.tooltip = "Search or start a new chat (⌘K)";
  $(id).setAttribute("aria-haspopup", "dialog");
  $(id).setAttribute("aria-controls", "search-dialog");
  $(id).setAttribute("aria-expanded", "false");
}
function launcherShortcut(e) {
  if (e.isComposing || e.repeat || !(e.metaKey || e.ctrlKey) || e.altKey || e.shiftKey || e.code !== "KeyK" || $("layout").hidden) return;
  e.preventDefault();
  if ($("search-dialog").open) { $("workspace-search").focus(); return; }
  openSearch($("sidebar-new-chat"));
}
addEventListener("keydown", launcherShortcut);
// Keyboard events inside the chat iframe cannot bubble to the desktop.
addEventListener("message", async event => {
  if (event.data?.fragment !== "open-launcher" || $("layout").hidden) return;
  const frame = [...document.querySelectorAll("iframe[data-fragment]")].find(f => f.contentWindow === event.source);
  const name = frame?.dataset.fragment;
  if (!name || !byName(name) || document.activeElement !== frame) return;
  try { if (event.origin !== await originOf(name)) return; } catch { return; }
  if (frame.contentWindow === event.source && document.activeElement === frame) openSearch($("sidebar-new-chat"));
});
$("search-dialog").addEventListener("click", (e) => {
  if (e.target === $("search-dialog")) closeSearch();
});
$("search-dialog").addEventListener("cancel", (e) => { e.preventDefault(); closeSearch(); });
$("search-dialog").addEventListener("close", () => {
  ++asked;
  clearTimeout(typing);
  searchTrigger?.setAttribute("aria-expanded", "false");
});
$("search-form").onsubmit = (e) => { e.preventDefault(); startFromSearch(); };
$("workspace-search").addEventListener("input", updateSearch);
$("workspace-search").addEventListener("keydown", (e) => {
  if (e.isComposing) return;
  const first = $("search-results").querySelector("button");
  if (e.key === "ArrowDown" && first) { e.preventDefault(); first.focus(); }
  if (e.key === "Enter") { e.preventDefault(); if (canLaunch()) startFromSearch(); else first?.click(); }
});
$("search-results").addEventListener("keydown", (e) => {
  if (!["ArrowDown", "ArrowUp"].includes(e.key)) return;
  const rows = [...$("search-results").querySelectorAll("button")];
  const index = rows.indexOf(document.activeElement);
  if (index < 0) return;
  e.preventDefault();
  if (e.key === "ArrowUp" && index === 0) $("workspace-search").focus();
  else rows[Math.min(rows.length - 1, index + (e.key === "ArrowDown" ? 1 : -1))]?.focus();
});

// ---- the computer: awake while the person is here, and its updates ----
let woke = 0;
// asking for a wake as the person arrives or focuses a chat hides the
// computer's start (decision 39's pre-wake, from the shell)
// The person's computer, made once (making it again is the same one:
// docs/api.md).
let making = null;
function computerOf() {
  making ??= api("POST", "/api/computers", {}).then((c) => { state.computer = c; return c; });
  making.catch(() => { making = null; });
  return making;
}
// A computer that won't start is not woken by the person's presence: its
// starts kept failing, so the next is theirs to ask for (Restart, in its
// notice), never one every visit pays for unasked. While the first run
// makes the agent, its computer starts once it is in its chat (the
// platform's `joined`), with the agent assigned, not before.
function prewake() {
  if (setup && !setup.joined) return;
  const c = state.computer;
  if (!c || document.hidden || Date.now() - woke < 60_000 || ["awake", "starting", "wont_wake"].includes(c.phase)) return;
  woke = Date.now();
  api("POST", `/api/computers/${seg(c.computer)}/wake`, {}).then(computerIs).catch(() => {});
}
document.addEventListener("visibilitychange", prewake);
// The computer as the platform last answered: shown at once.
function computerIs(v) {
  if (!v?.computer) return;
  state.computer = v;
  renderComputer();
  renderChats();
  rosterChanged();
}

// ---- what the person should know of their computer (docs/computers.md,
// "What its owner is told"): the view's notices, the most pressing first,
// each in plain words with the one way back to working. Its computer tells
// this page's list socket when they change, so they show while there is
// still time. Times are the person's own clock. ----
const clock = (ms) => {
  const d = new Date(ms);
  const today = d.toDateString() === new Date().toDateString();
  const time = d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  return today ? time : `${d.toLocaleDateString([], { month: "short", day: "numeric" })}, ${time}`;
};
// why a computer won't start, in plain words (the platform's own words
// stay beside them, for whoever helps)
function wontStart(why) {
  if (/not ready within/.test(why)) return "It didn't finish starting within a few minutes.";
  if (/kept stopping/.test(why)) return "It kept stopping as soon as it started.";
  if (/check/.test(why)) return "Its saved files didn't pass their check.";
  if (/restore|archive|backup/i.test(why)) return "Its save couldn't be restored.";
  return "It couldn't start.";
}
// a restart the person asked for in this page, and of which start: a
// computer that still won't start after one says where to get help
let restarted = 0;
function noticeRow(n) {
  const row = el("div", "computer-notice");
  row.dataset.kind = n.kind;
  const copy = el("div", "notice-copy");
  const actions = el("div", "notice-actions");
  const say = (title, text, detail) => {
    copy.append(el("strong", null, title), el("p", null, text));
    if (detail) copy.append(el("p", "notice-detail", detail));
  };
  const restart = (label) => {
    const b = el("button", "primary", label);
    b.type = "button";
    b.dataset.action = "restart";
    b.onclick = () => restartComputer(b);
    actions.append(b);
  };
  if (n.kind === "wont_wake") {
    row.classList.add("warn");
    const again = restarted && state.computer?.generation > restarted;
    say(
      again ? "Your computer still won't start" : "Your computer won't start",
      `${wontStart(n.why)} Your agents can't answer until it does.${again ? " If restarting again doesn't help, get help and give them the details below." : ""}`,
      `${state.computer?.computer ?? ""}: ${n.why}`,
    );
    restart(again ? "Try again" : "Restart");
    if (again && state.support) {
      const help = el("a", null, "Get help");
      help.href = state.support;
      help.target = "_blank";
      help.rel = "noopener";
      actions.append(help);
    }
  } else if (n.kind === "unsaved") {
    row.classList.add("warn");
    const then = n.save ? "starts again from that save" : "starts again with nothing kept";
    const stop = n.stopsAt ? ` If it still can't by ${clock(n.stopsAt)}, it stops and ${then}.` : "";
    say("Your computer can't save right now", `It hasn't saved since ${clock(n.since)}, so what its agents did after that isn't kept yet. It keeps trying.${stop} Restarting tries to save first.`, n.why);
    restart("Restart now");
  } else if (n.kind === "went_back") {
    // told only when something was done on it after that save (a stop
    // that lost nothing tells nothing), so it says what went, calmly
    const since = n.save && n.savedAt ? clock(n.savedAt) : null;
    const back = n.pending
      ? (since ? `When it starts again, changes on it since ${since} won't be there.` : "Nothing on it was saved yet, so it will start fresh.")
      : (since ? `Changes on it since ${since} weren't kept.` : "Nothing on it was saved yet, so it started fresh.");
    say(n.pending ? "Your computer stopped" : "Your computer restarted", `${back} Your chats are all here.`);
    const ok = el("button", null, "OK");
    ok.type = "button";
    ok.dataset.action = "seen";
    ok.onclick = () => sawNotice(n.life, ok);
    actions.append(ok);
  } else if (n.kind === "update") {
    // told only between turns, never restarted behind their back; an
    // always-on computer never sleeps, so never updates by itself
    const itself = state.computer?.alwaysOn ? "" : " It also updates by itself the next time your computer sleeps.";
    say("An update is ready for your computer", `Restart to get it — about a minute; your chats and work are kept.${itself}`);
    restart("Restart");
    const later = el("button", null, "Later");
    later.type = "button";
    later.dataset.action = "later";
    later.onclick = () => updateLater(n.version, later);
    actions.append(later);
  } else if (n.kind === "refresh") {
    copy.append(el("strong", null, "Couldn't refresh your computer"));
    const again = el("button", "primary", "Try again");
    again.type = "button";
    again.dataset.action = "refresh";
    again.onclick = () => readAgain(again);
    actions.append(again);
  } else return null;
  row.append(copy, actions);
  return row;
}
// a read of it that failed (a blip, a 5xx) says so first, over what the
// last read said, with Try again
function renderComputer() {
  const box = $("computer-notices");
  const rows = [...(state.computerFailed ? [{ kind: "refresh" }] : []), ...(state.computer?.notices ?? [])].map(noticeRow).filter(Boolean);
  box.replaceChildren(...rows);
  box.hidden = !rows.length;
}
// Try again: the list and the computer read again (`load`, which goes on
// with what the failure put off). The list's read failing too, the line
// stays, to try again.
async function readAgain(button) {
  button.disabled = true;
  await load().catch(() => {});
  button.disabled = false;
}
// Restart: the computer saves if it can, then starts fresh from its newest
// good save. It names the start the person saw, so pressed twice (or in two
// tabs) it restarts once. What went wrong is said beside the button.
async function restartComputer(button) {
  const c = state.computer;
  if (!c) return;
  const buttons = [...document.querySelectorAll("[data-action=restart]")];
  for (const b of buttons) b.disabled = true;
  const label = button.textContent;
  button.textContent = "Restarting…";
  try {
    restarted = c.generation || 0;
    computerIs(await api("POST", `/api/computers/${seg(c.computer)}/restart`, { generation: c.generation || 0 }));
    await reopenSettings();
  } catch (e) {
    button.textContent = label;
    const said = button.parentElement.querySelector(".form-error") ?? el("p", "form-error");
    said.textContent = `It did not restart: ${e.message}`;
    button.parentElement.append(said);
  } finally {
    for (const b of buttons) b.disabled = false;
  }
}
// The person read what a start went back to: told no more, anywhere.
async function sawNotice(life, button) {
  const c = state.computer;
  if (!c) return;
  button.disabled = true;
  try {
    computerIs(await api("POST", `/api/computers/${seg(c.computer)}/notices/seen`, { life }));
  } catch {
    computerIs(await api("GET", `/api/computers/${seg(c.computer)}`).catch(() => null));
  }
}
// Later: this update is told no more, anywhere, until another is ready.
async function updateLater(version, button) {
  const c = state.computer;
  if (!c) return;
  button.disabled = true;
  try {
    computerIs(await api("POST", `/api/computers/${seg(c.computer)}/notices/later`, { version }));
  } catch {
    computerIs(await api("GET", `/api/computers/${seg(c.computer)}`).catch(() => null));
  }
}
// ---- where the page is: `/` its chats, `/settings` its settings,
// `/welcome` a new person's welcome; each view says so in the address, so
// a reload or a link stays put ----
const SETTINGS = "/settings";
const WELCOME = "/welcome";
function at(path) {
  if (location.pathname !== path) history.pushState(null, "", path);
}
addEventListener("popstate", () => {
  const shown = !$("layout").hidden;
  if (shown && location.pathname === SETTINGS) openSettings(false).catch((e) => notice("Settings did not open", e.message));
  else if (shown && location.pathname === "/" && byName(state.current)) openChat(state.current, false);
  else start().catch((e) => notice("This page did not load", e.message));
});

// ---- settings: the person's account, credit, computer and agents ----
const usd = (micros) => (micros / 1_000_000).toLocaleString(undefined, { style: "currency", currency: "USD" });
// what a standing short of `ok` stops, and why (crates/core/src/ledger.rs `Refused`)
const STOPPED = { agents_stopped: "Your agents are stopped", read_only: "Your fragments are read-only, and your agents are stopped" };
// a computer's phase, as a person reads it (proto's `ComputerPhase`)
const PHASE = { asleep: "Asleep", starting: "Starting", awake: "Awake", sleeping: "Going to sleep", wont_wake: "Won't start" };
const WHY = {
  seat_canceled: "the seat was canceled",
  no_credit: "the credit is used up; adding credit starts them again",
  overdrawn: "the balance reached the overdraft; credit that brings it above zero ends it",
};
// The CLI's one-line install, for macOS and Linux (cli/SKILL.md's: `cargo
// xtask check` holds the two to each other and to the release's assets),
// and a coding agent's skill, in Claude Code's folder: behind settings'
// "Install it yourself", for whoever wants them (a coding agent given the
// prompt reads /llms.txt and installs it itself).
const INSTALL = "mkdir -p ~/.local/bin && curl -fsSL https://github.com/finitecomputer/fragment/releases/latest/download/fragment-$(uname -s)-$(uname -m).tar.gz | tar -xzf - -C ~/.local/bin";
const SKILL = "mkdir -p ~/.claude/skills/fragment && fragment skill > ~/.claude/skills/fragment/SKILL.md";
function section(title, ...children) {
  const s = el("section", "settings-section");
  s.append(el("h2", null, title), ...children);
  return s;
}
function line(label, value, action) {
  const p = el(action ? "div" : "p", action ? "settings-line settings-line-with-action" : "settings-line");
  const key = el("span", "settings-key", label);
  const text = el("span", "settings-value", value);
  if (action) {
    const info = el("div", "settings-info");
    info.append(key, text);
    const control = el("div", "settings-control");
    control.append(action);
    p.append(info, control);
  } else p.append(key, text);
  return p;
}
function actionLine(label, ...actions) {
  const row = el("div", "settings-line settings-action-line");
  const key = el("span", "settings-key");
  key.append(label);
  const control = el("div", "settings-control");
  control.append(...actions);
  row.append(key, control);
  return row;
}
// Finite's privacy policy and terms (`/privacy`, `/terms`), small and
// muted, each in a tab of its own, at the end of Settings' Account (the
// first run's footer, index.html, has About, Contact and these)
function legalLinks() {
  const p = el("p", "muted legal-links");
  const pages = [["/privacy", "Privacy"], ["/terms", "Terms"]];
  pages.forEach(([href, text], i) => {
    const a = el("a", null, text);
    a.href = href;
    a.target = "_blank";
    a.rel = "noopener";
    p.append(...(i ? [" · ", a] : [a]));
  });
  return p;
}
// a sentence whose `quoted` parts are code
function say(text) {
  const p = el("p");
  text.split("`").forEach((part, i) => p.append(i % 2 ? el("code", null, part) : part));
  return p;
}
function renderSettingsPage(page, sections) {
  const layout = el("div", "settings-layout");
  const nav = el("nav", "settings-nav");
  nav.setAttribute("aria-label", "Settings sections");
  nav.append(el("span", "settings-nav-title", "Settings"));
  const content = el("div", "settings-content");
  const intro = el("header", "settings-intro");
  intro.append(el("h1", null, "Settings"), el("p", null, state.guest ? "Manage your account and billing." : "Manage your account, your computer, and the tools your agents use."));
  content.append(intro);
  const links = [];
  sections.forEach((section, index) => {
    const heading = section.querySelector(":scope > h2");
    if (heading) {
      if (!section.id) section.id = `settings-section-${index}`;
      heading.tabIndex = -1;
      const jump = el("button", "settings-nav-link", heading.textContent);
      jump.type = "button";
      jump.onclick = () => {
        section.scrollIntoView({ behavior: matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth", block: "start" });
        heading.focus({ preventScroll: true });
      };
      nav.append(jump);
      links.push([section, jump]);
    }
    content.append(section);
  });
  layout.append(nav, content);
  page.replaceChildren(layout);
  page.onscroll = () => {
    const top = page.getBoundingClientRect().top + 130;
    const current = [...links].reverse().find(([section]) => section.getBoundingClientRect().top <= top)?.[1] ?? links[0]?.[1];
    for (const [, button] of links) {
      if (button === current) button.setAttribute("aria-current", "location");
      else button.removeAttribute("aria-current");
    }
  };
  page.onscroll();
}
// Each load of settings is numbered, and only the latest renders, while
// settings are shown: an older one that answers last (an action's reload, a
// second press, Back) read older state, and is dropped (#441).
let settingsLoads = 0;
// Each visit to settings, from another page (never a reload of them): an
// action's failure is said only in the visit it was pressed in (#439).
let visits = 0;
// `failed`: what the action that asked for this load says once it renders
// (a failure's notice, [title, text]).
async function openSettings(push = true, failed = null) {
  const ticket = ++settingsLoads;
  const lost = () => ticket !== settingsLoads || state.page !== "Settings";
  if (push) at(SETTINGS);
  // settings render with no notice: one of the page they came from stays
  // there, an action that worked takes its earlier failure's away, and
  // one that failed says so once this load renders, if nothing newer (a
  // load, a page, a notice) has come since it began
  const gen = hideNotice();
  const tell = () => {
    if (failed && gen === noticeGen) notice(...failed);
  };
  if (state.page !== "Settings") visits++;
  state.page = "Settings";
  $("layout").classList.add("settings-view");
  $("frames").hidden = true;
  const page = $("settings-page");
  page.hidden = false;
  // in the loaded page's own layout, so nothing moves as it loads
  renderSettingsPage(page, [el("p", "muted settings-loading", "Loading…")]);
  renderHeading();
  renderChats();
  leaveSidebar();
  // a credit pack's Checkout returning, before the ledger is read (its
  // settlement every load awaits, said by the one that renders)
  const billingHelpers = { api, el, section, line, actionLine, usd, reopen: () => reopenSettings(), noSeatSaid, switchForm, support: state.support };
  const back = await returning(billingHelpers);
  if (lost()) return;
  const [ledger, mySeat, linked, uses, access] = await Promise.all([
    api("GET", "/api/ledger").catch(() => null),
    api("GET", "/api/seat").catch(() => null),
    api("GET", "/api/connections").catch(() => null),
    state.computer ? api("GET", `/api/computers/${seg(state.computer.computer)}/uses`).catch(() => null) : null,
    api("GET", "/api/access").catch((e) => ({ error: e.message })),
  ]);
  if (lost()) return;
  if (mySeat) setGuest(mySeat.guest);
  const emails = (state.me.subjects ?? []).map((x) => x.email).filter(Boolean);
  // who is signed in (their latest sign-in's email), signing out, and
  // another account: a seat may wait under another email
  const signout = el("form");
  signout.method = "post";
  signout.action = "/auth/logout";
  const out = el("button", "quiet", "Sign out");
  out.type = "submit";
  signout.append(out);
  const ways = el("span", "settings-actions");
  ways.append(signout, switchForm(SETTINGS));
  const who = line("Signed in as", "", ways);
  who.classList.add("settings-who", "settings-stack");
  who.querySelector(".settings-value").append(state.me.email ? el("strong", null, state.me.email) : "—");
  const id = line("Identity", state.me.id);
  id.lastChild.classList.add("mono");
  const account = section("Account", who, id);
  // a picture: PNG, JPEG, WebP or GIF, at most 256 KiB (PUT /api/identities/me/picture)
  const picture = el("label", "quiet picture");
  const shown = el("img");
  shown.alt = "";
  shown.hidden = !state.me.picture;
  if (state.me.picture) shown.src = state.me.picture;
  const file = el("input");
  file.type = "file";
  file.accept = "image/png,image/jpeg,image/webp,image/gif";
  file.hidden = true;
  picture.append(shown, el("span", null, state.me.picture ? "Change your picture" : "Add a picture"), file);
  file.onchange = async () => {
    const f = file.files?.[0];
    if (!f) return;
    writing++;
    let r;
    try {
      r = await fetch("/api/identities/me/picture", { method: "PUT", headers: { "x-fragment-shell": "1", "content-type": f.type }, body: f, credentials: "same-origin" });
      heard(r);
    } finally {
      writing--;
    }
    if (r.ok) {
      state.me = await api("GET", "/api/identities/me");
      reopenSettings();
    } else picture.querySelector("span").textContent = (await r.json().catch(() => ({}))).message || "That picture was not taken";
  };
  account.append(actionLine("Picture", picture));
  // their sign-ins, when they linked several before linking went (off
  // until WorkOS offers an account picker: docs/api.md, Sign-in)
  if (emails.length > 1) account.append(line("Sign-ins", emails.join(", ")));
  account.append(legalLinks());
  // the person's seat, credit and org (billing.js), and why agents stop
  const billing = await billingSections(billingHelpers, ledger, mySeat, back);
  if (lost()) return;
  // a guest has no agents to stop, nor a computer, skills or connections
  // for them, nor apps to publish: a seat to get comes first, then who they
  // are, then everything that is theirs, to download (what is shared with
  // them, named); Your data is built only by a load that renders, as it
  // reads its export again while one builds
  if (state.guest) {
    returnSaid(back);
    renderSettingsPage(page, [...billing, account, dataSection()]);
    return tell();
  }
  const standing = ledger?.standing?.standing;
  if (standing && standing !== "ok") {
    billing[0].append(el("p", "settings-warning", [STOPPED[standing] ?? "Your agents are stopped", WHY[ledger.standing.why]].filter(Boolean).join(": ") + "."));
  }
  const c = state.computer;
  // the version it runs (asleep: the one it starts on), and the latest when
  // it runs an older one (docs/computers.md, "What its owner is told")
  const behind = !!(c?.version && c.latest && c.version !== c.latest);
  const computer = section(
    "Computer",
    ...(c
      ? [
          line("State", PHASE[c.phase] ?? c.phase),
          line("Version", c.version ?? c.latest ?? c.image),
          ...(behind
            ? [line("Latest version", c.latest), el("p", "muted", `Restart to get the latest version.${c.alwaysOn ? "" : " It also updates by itself the next time your computer sleeps."}`)]
            : []),
          ...(c.why ? [el("p", "settings-warning", c.why)] : []),
        ]
      : // never read: a failed read is not "none" (shown again once one answers: `load`)
        [el("p", "muted", state.computerFailed ? "Couldn't load your computer." : "Your computer starts with your first agent.")]),
  );
  // each agent's own desktop
  const screens = [];
  for (const a of c ? state.agents.values() : []) {
    const screen = el("button", "quiet", `Open ${titleOf(a.fragment)}'s screen`);
    screen.type = "button";
    screen.dataset.screenFor = a.fragment;
    screen.onclick = () => openScreen(a.fragment);
    screens.push(screen);
  }
  if (screens.length) computer.append(actionLine("Screens", ...screens));
  // the way back to working when something is wrong with it: asked once
  // more before it cuts what its agents are doing
  if (c) {
    computer.append(el("p", "muted", "Restarting saves your computer if it can, then starts it fresh from that save. Anything its agents are doing stops."));
    const row = el("span", "settings-actions");
    const go = el("button", "quiet", "Restart computer");
    go.type = "button";
    go.dataset.action = "restart-ask";
    go.onclick = () => {
      const sure = el("button", "quiet", "Restart now");
      sure.type = "button";
      sure.dataset.action = "restart";
      sure.onclick = () => restartComputer(sure);
      const cancel = el("button", "quiet", "Cancel");
      cancel.type = "button";
      cancel.onclick = () => row.replaceChildren(go);
      row.replaceChildren(sure, cancel);
    };
    row.append(go);
    computer.append(actionLine("Restart", row));
  }
  const agents = section("Agents");
  const mine = [...state.agents.values()];
  // the models a provider the person connected offers (its own credit pays)
  const modeled = (linked?.providers ?? []).filter((p) => p.models?.length && p.state === "set");
  if (mine.length && modeled.length) agents.append(say("Each agent runs on the platform's own model, included in your credit, unless you pick one of a provider you connected, which your account there pays for."));
  agents.append(...(mine.length ? mine.flatMap((a) => {
    const row = el("button", "row", "Open");
    row.type = "button";
    row.onclick = () => openApp(a.fragment);
    const name = el("span", "settings-agent-name");
    name.dataset.agentFace = a.fragment;
    const [actions, said] = faceControls(a);
    return [agentSettings(a, actionLine(name, actions, row), modeled), said];
  }) : [el("p", "muted", "None yet.")]));
  const connections = connectionsSection(linked, uses);
  const skills = await skillsSection();
  if (lost()) return;
  returnSaid(back);
  renderSettingsPage(page, [account, ...billing, computer, agents, accessSection(access), skills, connections, buildSection(), dataSection()]);
  renderAgentFaces();
  tell();
}
// An action's reload of settings, only while they are shown: an answer that
// comes once the person has gone to a chat leaves them there, and says
// nothing there (#439). `failed`: openSettings'.
async function reopenSettings(failed = null) {
  if (state.page === "Settings") await openSettings(false, failed);
}

// Building with a coding agent: one prompt that names this platform's
// /llms.txt (its handoff: the install, `fragment login --host` here, the
// manual), copied into Claude Code or Codex with what to build. The
// install line stays, folded, for whoever runs it themselves.
function buildSection() {
  const prompt = el("textarea", "build-prompt");
  prompt.id = "build-prompt";
  prompt.rows = 3;
  prompt.spellcheck = false;
  prompt.setAttribute("aria-label", "A prompt for your coding agent");
  prompt.value = `Read ${location.origin}/llms.txt and help me build: `;
  const copy = el("button", "quiet", "Copy");
  copy.type = "button";
  copy.dataset.action = "copy-prompt";
  // a clipboard that refuses (no permission, a webview's, none at all) is
  // said under the prompt, which is selected to copy by hand; Copy stays
  // Copy, to try again (#438)
  const said = el("p", "form-error");
  said.id = "build-prompt-said";
  said.setAttribute("role", "status");
  said.hidden = true;
  // each press, numbered: one that answers after a newer press changes
  // nothing (that one says how it went)
  let presses = 0;
  copy.onclick = async () => {
    const mine = ++presses;
    said.hidden = true;
    try {
      await navigator.clipboard.writeText(prompt.value);
    } catch {
      if (mine !== presses) return;
      copy.textContent = "Copy";
      said.textContent = "Couldn't copy it. The prompt is selected: copy it yourself (Ctrl+C, or ⌘C on a Mac).";
      said.hidden = false;
      prompt.focus();
      prompt.select();
      return;
    }
    if (mine !== presses) return;
    copy.textContent = "Copied";
    setTimeout(() => {
      if (mine === presses) copy.textContent = "Copy";
    }, 1500);
  };
  const actions = el("div", "settings-actions");
  actions.append(copy);
  const yourself = el("details", "settings-install");
  yourself.append(
    el("summary", null, "Install it yourself"),
    say("On macOS or Linux (on Windows, in WSL), then pair it with you:"),
    el("pre", "command", `${INSTALL}\nfragment login --host ${location.origin}`),
    say("To give Claude Code its skill:"),
    el("pre", "command", SKILL),
  );
  return section(
    "Build with Claude Code or Codex",
    say("Paste this into your coding agent and finish the sentence: it sets up the `fragment` command line itself, and you approve it here once."),
    prompt,
    actions,
    said,
    yourself,
  );
}

// ---- an agent's name and picture (docs/api.md, "An agent's name and
// picture"): its agent fragment's title and its avatar.png, written and
// put live as any of its files are (POST …/files, then …/deploy). Every
// page and the platform show it by them; the rows here follow the list. ----
const AGENT_PICTURE = "avatar.png";
// What the platform takes (fragment_core::face): a PNG, JPEG, WebP or GIF
// of at most 256 KiB. A larger one, or any other image the browser reads,
// is drawn again at most PICTURE_PX square: a PNG, else a JPEG, that fits.
const PICTURE_MAX_BYTES = 256 * 1024;
const PICTURE_PX = 512;
async function pictureOf(file) {
  if (file.size <= PICTURE_MAX_BYTES && /^image\/(png|jpeg|webp|gif)$/.test(file.type)) return file;
  let bitmap;
  try {
    bitmap = await createImageBitmap(file);
  } catch {
    throw new Error("That file is not a picture: choose a PNG, JPEG, WebP or GIF.");
  }
  const scale = Math.min(1, PICTURE_PX / Math.max(bitmap.width, bitmap.height));
  const canvas = el("canvas");
  canvas.width = Math.max(1, Math.round(bitmap.width * scale));
  canvas.height = Math.max(1, Math.round(bitmap.height * scale));
  canvas.getContext("2d").drawImage(bitmap, 0, 0, canvas.width, canvas.height);
  for (const [type, quality] of [["image/png"], ["image/jpeg", 0.9], ["image/jpeg", 0.7]]) {
    const blob = await new Promise((done) => canvas.toBlob(done, type, quality));
    if (blob && blob.size <= PICTURE_MAX_BYTES) return blob;
  }
  throw new Error("That picture stays over 256 KiB however it is drawn: choose a smaller one.");
}
async function base64Of(blob) {
  const bytes = new Uint8Array(await blob.arrayBuffer());
  let text = "";
  for (let i = 0; i < bytes.length; i += 0x8000) text += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(text);
}
// The agent's picture from `file`, or none (null), put live.
async function setPicture(agent, file) {
  const write = file ? { path: AGENT_PICTURE, base64: await base64Of(await pictureOf(file)) } : { path: AGENT_PICTURE, delete: true };
  await api("POST", `/api/f/${seg(agent)}/files`, { message: file ? "its picture" : "no picture", files: [write] });
  await api("POST", `/api/f/${seg(agent)}/deploy`, {});
}
// An agent's row's Rename, its picture (Add, Change, Remove), and a line
// that says what they did or why not.
function faceControls(a) {
  const said = el("p", "muted agent-face-said");
  said.hidden = true;
  const tell = (text) => {
    said.textContent = text;
    said.hidden = !text;
  };
  const renameIt = el("button", "quiet", "Rename");
  renameIt.type = "button";
  renameIt.dataset.renameAgent = a.fragment;
  renameIt.onclick = () => rename(a.fragment);
  const picture = el("label", "quiet picture");
  const label = el("span");
  label.dataset.pictureLabel = a.fragment;
  const file = el("input");
  file.type = "file";
  file.accept = "image/png,image/jpeg,image/webp,image/gif";
  file.dataset.pictureFor = a.fragment;
  file.hidden = true;
  picture.append(label, file);
  const remove = el("button", "quiet", "Remove picture");
  remove.type = "button";
  remove.dataset.pictureRemove = a.fragment;
  const change = async (chosen) => {
    for (const b of [renameIt, remove, file]) b.disabled = true;
    tell(chosen ? "Saving its picture…" : "Taking its picture away…");
    try {
      await setPicture(a.fragment, chosen);
      tell("");
      await load();
    } catch (e) {
      tell(e.message);
    } finally {
      for (const b of [renameIt, remove, file]) b.disabled = false;
      file.value = "";
    }
  };
  file.onchange = () => file.files?.[0] && change(file.files[0]);
  remove.onclick = () => change(null);
  const actions = el("span", "agent-face-actions");
  actions.append(renameIt, picture, remove);
  return [actions, said];
}
// Settings' agent rows as the list says now: each one's face and name, and
// whether it has a picture to change or remove; and the names its other
// controls carry (each agent's screen, its model), and its open screens.
function renderAgentFaces() {
  for (const b of document.querySelectorAll("[data-screen-for]")) b.textContent = `Open ${titleOf(b.dataset.screenFor)}'s screen`;
  for (const f of document.querySelectorAll("iframe[data-screen]")) nameScreen(f);
  for (const pick of document.querySelectorAll("[data-model-for]")) pick.setAttribute("aria-label", `${titleOf(pick.dataset.modelFor)}'s model`);
  for (const name of document.querySelectorAll("[data-agent-face]")) {
    const fragment = name.dataset.agentFace;
    const id = [...state.agents.values()].find((a) => a.fragment === fragment)?.identity ?? null;
    const face = agentFace(fragment, id);
    const shown = JSON.stringify([face.picture, titleOf(fragment)]);
    if (name.dataset.shown === shown) continue;
    name.dataset.shown = shown;
    name.replaceChildren(avatar(face, "small"), titleOf(fragment));
  }
  for (const label of document.querySelectorAll("[data-picture-label]")) label.textContent = byName(label.dataset.pictureLabel)?.picture ? "Change picture" : "Add a picture";
  for (const remove of document.querySelectorAll("[data-picture-remove]")) remove.hidden = !byName(remove.dataset.pictureRemove)?.picture;
}

// ---- settings' Sharing: who has access to what the person owns (GET
// /api/access), by fragment or by person; each fragment opens its share
// sheet, where it changes ----
const GENERAL = {
  members: { icon: "lock", name: "Restricted" },
  link: { icon: "link", name: "Anyone with the link" },
  public: { icon: "globe", name: "Public" },
};
const ROLE = { viewer: "Viewer", editor: "Editor", owner: "Owner" };
let accessBy = "fragment";
// an identity too long to read whole: its npub's ends
const shortId = (id) => (id.length > 16 ? `${id.slice(0, 9)}…${id.slice(-4)}` : id);
// a member as the view names them, and what they are to the person; a
// person's agents reach what they do, never named (decision 63: "I invite
// Skyler and his agents, not Skyler and his agent Agent123")
// (an agent member by its title, its agent fragment's: docs/api.md, "An
// agent's name and picture")
function accessWho(m) {
  if (m.kind !== "agent") return { name: m.email ?? shortId(m.principal), about: "" };
  if (m.owner === state.me.id) {
    const mine = state.agents.get(m.principal);
    return { name: mine ? titleOf(mine.fragment) : m.title ?? m.name ?? shortId(m.principal), about: "your agent" };
  }
  const whose = m.ownerEmail ?? (m.owner ? shortId(m.owner) : null);
  return { name: m.title ?? m.name ?? shortId(m.principal), about: whose ? `${whose}'s agent: they read what it reads` : "an agent" };
}
// a fragment's own line: its title (its sheet), what it is, and who may open it
function accessHead(row) {
  const head = el("div", "access-head");
  const open = el("button", "access-name", byName(row.name) ? titleOf(row.name) : row.title || labelOf(row.name));
  open.type = "button";
  open.title = "Open its share sheet";
  open.dataset.share = row.name;
  open.onclick = () => share(row.name);
  const general = GENERAL[row.sharing?.visibility];
  const tag = el("span", "access-tag");
  if (general) {
    tag.replaceChildren(icon(general.icon));
    tag.append(general.name);
  }
  head.append(open, el("span", "muted access-kind", row.kind), tag);
  return head;
}
function accessByFragment(rows) {
  const list = el("div", "access-list");
  const shared = rows.filter((r) => r.members?.length || r.invites?.length || r.sharing?.guests || r.sharing?.invites || r.sharing?.visibility === "public");
  const rest = rows.filter((r) => !shared.includes(r));
  for (const row of shared) {
    const item = el("div", "access-row");
    item.dataset.fragment = row.name;
    const people = el("ul", "access-people");
    for (const m of row.members ?? []) {
      const who = accessWho(m);
      const li = el("li");
      li.append(el("span", "access-who", who.name), el("span", "muted", [who.about, ROLE[m.role] ?? m.role].filter(Boolean).join(" · ")));
      people.append(li);
    }
    for (const i of row.invites ?? []) {
      const li = el("li");
      li.append(el("span", "access-who", i.email), el("span", "muted", `invited as ${(ROLE[i.role] ?? i.role).toLowerCase()}, waiting for their yes`));
      people.append(li);
    }
    if (row.more) people.append(el("li", "muted", `and ${row.more} more`));
    // asked of no fragment (past the view's bound): its counts
    if (!row.members && row.sharing) people.append(el("li", "muted", `${row.sharing.guests} ${row.sharing.guests === 1 ? "person" : "people"}${row.sharing.invites ? `, ${row.sharing.invites} invited` : ""}: its sheet names them`));
    if (!people.children.length) people.append(el("li", "muted", "No one else is in it"));
    item.append(accessHead(row), people);
    list.append(item);
  }
  if (rest.length) {
    const more = el("details", "access-rest");
    more.append(el("summary", null, `${rest.length} more no one else is in`));
    for (const row of rest) {
      const item = el("div", "access-row");
      item.dataset.fragment = row.name;
      item.append(accessHead(row));
      more.append(item);
    }
    list.append(more);
  }
  if (!rows.length) list.append(el("p", "muted", "You own nothing yet."));
  return list;
}
// by person: who can open what of the person's, everyone else's agents by
// their owners, and those anyone may open
function accessByPerson(rows) {
  const people = new Map();
  const add = (key, name, about, row, role) => {
    if (!people.has(key)) people.set(key, { name, about, rows: [] });
    people.get(key).rows.push({ row, role });
  };
  for (const row of rows) {
    for (const m of row.members ?? []) {
      // the person's own agents see all of theirs anyway (decision 44)
      if (m.kind === "agent" && m.owner === state.me.id) continue;
      const who = accessWho(m);
      add(m.principal, who.name, m.kind === "agent" ? who.about : "", row, ROLE[m.role] ?? m.role);
    }
    for (const i of row.invites ?? []) add(`invite:${i.email}`, i.email, "invited, waiting for their yes", row, ROLE[i.role] ?? i.role);
  }
  const anyone = rows.filter((r) => r.sharing?.visibility === "public");
  const linked = rows.filter((r) => r.sharing?.visibility === "link");
  const list = el("div", "access-list");
  const group = (name, about, entries, open = true) => {
    const item = el("details", "access-person");
    item.open = open;
    const summary = el("summary");
    summary.append(el("span", "access-who", name), el("span", "muted", [about, `${entries.length} ${entries.length === 1 ? "fragment" : "fragments"}`].filter(Boolean).join(" · ")));
    const ul = el("ul", "access-people");
    for (const { row, role } of entries) {
      const li = el("li");
      const open = el("button", "access-name", byName(row.name) ? titleOf(row.name) : row.title || labelOf(row.name));
      open.type = "button";
      open.onclick = () => share(row.name);
      li.append(open, el("span", "muted", role));
      ul.append(li);
    }
    item.append(summary, ul);
    list.append(item);
  };
  for (const p of people.values()) group(p.name, p.about, p.rows, true);
  if (anyone.length) group("Everyone", "public: no link needed", anyone.map((row) => ({ row, role: "Viewer" })));
  if (linked.length) group("Anyone with the share link", "you hold the links; each sheet copies its own", linked.map((row) => ({ row, role: "Viewer" })), false);
  if (!list.children.length) list.append(el("p", "muted", "No one else can open anything of yours."));
  return list;
}
function accessSection(access) {
  const s = section("Sharing");
  s.id = "settings-access";
  if (!access || access.error) {
    s.append(el("p", "settings-warning", `Who has access did not load${access?.error ? `: ${access.error}` : ""}.`));
    return s;
  }
  s.append(el("p", "muted", "Who can open what you own. A title opens its share sheet, where that changes."));
  const tabs = el("div", "settings-actions");
  const body = el("div", "access-body");
  const draw = () => {
    for (const b of tabs.children) b.setAttribute("aria-pressed", String(b.dataset.by === accessBy));
    body.replaceChildren(accessBy === "person" ? accessByPerson(access.fragments) : accessByFragment(access.fragments));
  };
  for (const [by, text] of [["fragment", "By fragment"], ["person", "By person"]]) {
    const b = el("button", "chip", text);
    b.type = "button";
    b.dataset.by = by;
    b.onclick = () => { accessBy = by; draw(); };
    tabs.append(b);
  }
  draw();
  s.append(actionLine("Show", tabs), body);
  return s;
}

// ---- your data: an export of everything that is the person's (docs/api.md,
// Export), built in the background, which they are mailed about once it is
// ready, and which downloads from here (the platform session, on a click
// on this page) for seven days. One at a time; three a day.
const EXPORT_STATE = { building: "Building", ready: "Ready to download", failed: "Failed", expired: "Expired" };
const sized = (n) => (n >= 1e9 ? `${(n / 1e9).toFixed(1)} GB` : n >= 1e6 ? `${(n / 1e6).toFixed(1)} MB` : n >= 1e3 ? `${(n / 1e3).toFixed(1)} KB` : `${n} bytes`);
function dataSection() {
  const s = section("Your data");
  s.id = "settings-data";
  s.append(
    el(
      "p",
      "muted",
      "Download everything that is yours: your fragments and your agents' (their files, app data and messages), your computer's newest save, and the names of what others share with you. It is one file, made in the background: we email you when it is ready, and it is kept for seven days.",
    ),
  );
  // its rows, drawn again as the export changes
  const shown = el("div", "settings-export");
  shown.dataset.state = "loading";
  s.append(shown);
  let timer = null;
  const render = (v, refused) => {
    shown.dataset.state = v?.state ?? "none";
    const go = el("button", "quiet", v?.state === "building" ? "Exporting…" : v?.state === "ready" ? "Export again" : "Export everything");
    go.type = "button";
    go.dataset.action = "export";
    go.disabled = v?.state === "building";
    go.onclick = async () => {
      go.disabled = true;
      try {
        render(await api("POST", "/api/export"));
      } catch (e) {
        render(v, e.message);
      }
    };
    const rows = [];
    if (v?.state === "ready" && v.download) {
      const get = el("a", "quiet", "Download");
      get.href = v.download;
      get.dataset.action = "export-download";
      rows.push(line("Export", EXPORT_STATE.ready, get), line("Size", sized(v.size ?? 0)), line("Kept until", new Date(v.expiresAt).toLocaleString()), line("New export", "Replaces this one, and its download", go));
    } else if (v?.state === "building") {
      rows.push(line("Export", EXPORT_STATE.building, go), line("Progress", `${v.progress.done} of ${v.progress.total} parts, ${sized(v.progress.bytes)} so far`));
    } else {
      rows.push(line("Export", v ? EXPORT_STATE[v.state] ?? v.state : "None yet", go));
      if (v?.state === "failed" && v.error) rows.push(el("p", "settings-warning", v.error.message));
    }
    if (refused) rows.push(el("p", "settings-warning", refused));
    shown.replaceChildren(...rows);
    // while it builds, read it again, as long as settings show this section
    clearTimeout(timer);
    if (v?.state === "building") timer = setTimeout(refresh, 2000);
  };
  // A read that fails keeps the rows shown, adds one line that says so,
  // and stops reading: its Try again reads again (never a new export), and
  // what it reads goes on as any read does (#440).
  const unread = () => {
    const again = el("button", "quiet", "Try again");
    again.type = "button";
    again.dataset.action = "export-read";
    again.onclick = () => {
      again.disabled = true;
      refresh();
    };
    const said = actionLine(el("span", "settings-warning", "Couldn't check your export."), again);
    said.dataset.unread = "";
    shown.querySelector("[data-unread]")?.remove();
    shown.append(said);
  };
  const refresh = () => {
    // only while settings show it (its first read is before it is placed);
    // settings shown again build it again, which reads again
    if (shown.dataset.state !== "loading" && (!s.isConnected || state.page !== "Settings")) return;
    api("GET", "/api/export").then((r) => render(r.export), unread);
  };
  refresh();
  return s;
}

// ---- an agent's settings in Settings' Agents: its row, then its
// agent.json's picks, read once for them all. Each pick is written as the
// others are, one write at a time (each reads what the one before wrote),
// and put live, as its name and picture are: its computer reads main, its
// page live (`__file`), and both then say the same. Its computer reads
// agent.json again within seconds and runs its next turn so. A pick is
// written only onto agent.json as it is now, read just before (none there,
// 404, is nothing to keep): a read that fails otherwise writes nothing and
// says so, so no other setting of the agent (its tier, its model) is lost.
const agentJson = (agent) => api("GET", `/api/f/${seg(agent)}/file?path=agent.json`);
const UNREAD_SETTINGS = "Couldn't read this agent's settings, so nothing was changed. Try again.";
function agentSettings(a, row, modeled) {
  const box = el("div", "agent-setting");
  box.dataset.agent = a.fragment;
  const said = el("span", "muted agent-model-said");
  // what the picks show: agent.json, none (404: the defaults), or unread (null)
  const read = agentJson(a.fragment).catch((e) => (e.status === 404 ? {} : null));
  read.then((j) => { if (j === null) said.textContent = "Couldn't read this agent's settings: reload to see them."; });
  let last = Promise.resolve();
  // `change` sets its pick on what agent.json holds now (its tier kept),
  // and says what changed
  const write = (change) => (last = last.catch(() => {}).then(async () => {
    const now = await agentJson(a.fragment).catch((e) => {
      if (e.status === 404) return {};
      throw new Error(UNREAD_SETTINGS);
    });
    const next = { ...now, tier: now.tier ?? DEFAULT_AGENT_TIER };
    const message = change(next);
    await api("POST", `/api/f/${seg(a.fragment)}/files`, { message, files: [{ path: "agent.json", text: JSON.stringify(next, null, 2) + "\n" }] });
    await api("POST", `/api/f/${seg(a.fragment)}/deploy`, {});
  }));
  const picks = [...(modeled.length ? [modelPicker(a, modeled, read, write, said)] : []), reasoningPicker(a, read, write, said)];
  box.append(row, ...picks, said);
  return box;
}
// a pick (`value`) being written (`written`), said under its agent's
// settings; one not written shows again what agent.json holds (`was`)
async function savePick(pick, value, said, written) {
  pick.disabled = true;
  said.textContent = "Saving…";
  try {
    await written;
    said.textContent = "Saved: its next message uses it.";
    pick.dataset.saved = value;
    pick.dataset.was = value;
  } catch (e) {
    said.textContent = e.message;
    pick.value = pick.dataset.was ?? "";
  } finally {
    pick.disabled = false;
  }
}
function settingLabel(key, pick) {
  const label = el("label", "agent-model-label");
  label.append(el("span", "settings-key", key), pick);
  return label;
}
function modelOption(value, text) {
  const o = el("option", null, text);
  o.value = value;
  return o;
}

// ---- an agent's model: its agent.json's `model` ({provider, id}), a model
// of a provider the person connected, or none: its `tier`'s (`medium`, else
// the default: `high` is off, decision 23), shown by the tier's name and its
// model's id, as their computer's view names it (the deployment's `models`).
// Vision and voice notes stay on the platform's own models.
const TIER = { cheap: "Cheap", medium: "Medium" };
const tierName = (tier) => {
  const t = tier === "medium" ? "medium" : DEFAULT_AGENT_TIER;
  const id = state.computer?.models?.[t];
  return id ? `${TIER[t]} (${id})` : TIER[t];
};
const modelValue = (provider, id) => `${provider} ${id}`;
function modelPicker(a, modeled, read, write, said) {
  const pick = el("select", "agent-model");
  pick.dataset.modelFor = a.fragment;
  pick.setAttribute("aria-label", `${titleOf(a.fragment)}'s model`);
  const tiered = modelOption("", tierName());
  pick.append(tiered);
  for (const p of modeled) {
    const group = el("optgroup");
    group.label = `${providerName(p.provider)} (your account)`;
    for (const m of p.models) group.append(modelOption(modelValue(p.provider, m.id), m.name));
    pick.append(group);
  }
  pick.disabled = true;
  read
    .then((j) => {
      tiered.textContent = tierName(j?.tier);
      const v = j?.model?.provider && j?.model?.id ? modelValue(j.model.provider, j.model.id) : "";
      // one named by hand, not offered here, is shown as it is
      if (v && ![...pick.options].some((o) => o.value === v)) pick.append(modelOption(v, j.model.id));
      pick.value = v;
      pick.dataset.was = v;
    })
    .finally(() => { pick.disabled = false; pick.dataset.read = "1"; });
  pick.onchange = () => {
    const value = pick.value;
    const [provider, id] = value ? value.split(" ") : [];
    savePick(pick, value, said, write((next) => {
      if (provider) next.model = { provider, id };
      else delete next.model;
      return `its model: ${provider ? id : next.tier}`;
    }));
  };
  return settingLabel("Model", pick);
}

// ---- how hard an agent thinks (Paul, 2026-10-22: "expose the reasoning
// level in the model picker … high is a good default"): its agent.json's
// `reasoning`, `low` or `high` (the efforts the platform's model route
// passes on), or none: the default, high. Its computer runs any other it
// names at the default too, and so it is shown.
const REASONING = [["", "Default (High)"], ["low", "Low"], ["high", "High"]];
function reasoningPicker(a, read, write, said) {
  const pick = el("select", "agent-model");
  pick.dataset.reasoningFor = a.fragment;
  pick.setAttribute("aria-label", `${titleOf(a.fragment)}'s reasoning`);
  for (const [value, text] of REASONING) pick.append(modelOption(value, text));
  pick.disabled = true;
  read
    .then((j) => { pick.value = pick.dataset.was = REASONING.some(([v]) => v && v === j?.reasoning) ? j.reasoning : ""; })
    .finally(() => { pick.disabled = false; pick.dataset.read = "1"; });
  pick.onchange = () => {
    const value = pick.value;
    savePick(pick, value, said, write((next) => {
      if (value) next.reasoning = value;
      else delete next.reasoning;
      return `its reasoning: ${value || "the default"}`;
    }));
  };
  return settingLabel("Reasoning", pick);
}

// ---- connections (decisions 22, 37 and 44): every provider the platform
// offers, one row each: what it is, its state, which of the person's agents
// may use it (all by default; a list narrows one agent), and this month's
// calls by each agent, with an operator key's cost. Their agents use them
// through the computer's swap, each with a placeholder of its own.
const PROVIDER_KIND = { connection: "Your account", operator: "The platform's key", own: "Your key" };
const PROVIDER_STATE = {
  connected: "Connected",
  needs_reauthorization: "Needs reauthorization",
  not_connected: "Not connected",
  offered: "Offered",
  set: "Key set",
  not_set: "No key yet",
};
// a provider's name for people: google-places is Google Places, xai xAI
const PROVIDER_NAMES = { xai: "xAI", elevenlabs: "ElevenLabs", openrouter: "OpenRouter" };
const providerName = (p) => PROVIDER_NAMES[p] ?? p.replace(/(^|-)([a-z])/g, (_, d, l) => `${d ? " " : ""}${l.toUpperCase()}`);
// a connection's scope for people, by its last path segment (Google's
// https://www.googleapis.com/auth/<name>), else that segment itself; the
// list never names a sign-in scope as missing
const SCOPE_NAMES = {
  "gmail.readonly": "Gmail (read)",
  "gmail.modify": "Gmail (manage)",
  "gmail.send": "Gmail (send)",
  drive: "Drive",
  spreadsheets: "Sheets",
  documents: "Docs",
  calendar: "Calendar",
  "contacts.readonly": "Contacts",
};
const scopeName = (scope) => {
  const last = scope.split("/").pop();
  return SCOPE_NAMES[last] ?? last;
};
// micro-dollars, to a hundredth of a cent under a dollar (a call is often less than a cent)
const money = (micros) => (micros / 1_000_000).toLocaleString(undefined, { style: "currency", currency: "USD", minimumFractionDigits: 2, maximumFractionDigits: micros < 1_000_000 ? 4 : 2 });
// Connect: a window opened by the press itself, before any await, and only
// then sent to the provider's page (#442): one opened once the authorize
// call answers is a popup Safari blocks, as Chrome does once the press is
// 5 s old. One attempt at a time: each start replaces the browser's one
// binding cookie. The section reads again when the person comes back to it
// (`focus`), never by watching `w.closed`: the shell's COOP has that read
// true once the window is at the provider, still open. `body`, where the
// person connects several accounts: `{add: true}` for another, `{account}`
// for one of theirs again.
function connectButton(p, label, said, body = {}) {
  const go = el("button", "quiet", label);
  go.type = "button";
  go.onclick = async () => {
    const w = window.open("", "_blank", "popup,width=520,height=720");
    if (!w) {
      said.textContent = "Your browser blocked the window. Allow pop-ups for this site, then try again.";
      return;
    }
    go.disabled = true;
    said.textContent = "";
    try {
      const { url } = await api("POST", `/api/connections/${encodeURIComponent(p.provider)}/authorize`, body);
      w.location = url;
      addEventListener("focus", () => reopenSettings().catch(() => {}), { once: true });
    } catch (e) {
      w.close();
      said.textContent = e.message;
    } finally {
      go.disabled = false;
    }
  };
  return go;
}
// The own keys whose last Disconnect or Remove did not go through, each
// with the visit of settings it was pressed in: said in their row by
// whichever load of settings renders in that visit, while the row still
// has the key, until the next press; in another visit, never (#439).
const unremoved = new Map();
// Disconnect: the account goes from Pipes, so no agent's call gets its token
// again, and its grant is revoked at the provider where the platform can;
// when it could not, settings says where the person removes it themselves.
// Of several accounts, the one `account` is.
function disconnectButton(p, said, account = null) {
  const off = el("button", "quiet", "Disconnect");
  off.type = "button";
  off.dataset.action = "disconnect";
  const which = account ? `?account=${encodeURIComponent(account.id)}` : "";
  off.onclick = async () => {
    off.disabled = true;
    said.textContent = "";
    let revoked;
    try {
      ({ revoked } = await api("DELETE", `/api/connections/${encodeURIComponent(p.provider)}${which}`));
    } catch (e) {
      said.textContent = e.message;
      off.disabled = false;
      return;
    }
    const name = providerName(p.provider);
    await reopenSettings(revoked ? null : [`${name} is disconnected`, `Your agents can no longer use it. ${name} may still list this platform's access to your account: remove it in your ${name} account's security settings.`]).catch(() => {});
  };
  return off;
}
// What the person left unticked when they connected an account, in words.
const notAllowed = (missing) => el("span", "provider-detail provider-missing", `Not allowed: ${[...new Set(missing.map(scopeName))].join(", ")}`);
// An account of several, for people: its label, or the provider's name.
const accountLabel = (p, a) => a.account ?? `A ${providerName(p.provider)} account`;
// A connection that takes several accounts: each of the person's, oldest
// first, with its label and state, what it was not allowed with a
// Reconnect to add (that account), and its own Disconnect. One whose
// disconnect did not finish (Pipes holds it still) says so, with
// Disconnect again, and nothing else.
function accountsList(p) {
  const list = el("div", "provider-accounts");
  for (const a of p.accounts) {
    const item = el("div", "provider-account-row");
    item.dataset.accountId = a.id;
    item.dataset.state = a.disconnecting ? "disconnecting" : a.state;
    const head = el("div", "provider-head");
    const who = el("div", "provider-identity");
    const stateName = a.disconnecting ? "Disconnecting didn't finish" : PROVIDER_STATE[a.state] ?? a.state;
    who.append(el("span", "provider-account", accountLabel(p, a)), el("span", `provider-state ${a.disconnecting ? "disconnecting" : a.state}`, stateName));
    const actions = el("div", "settings-actions");
    const said = el("span", "muted provider-detail provider-said");
    const details = [];
    if (a.disconnecting) {
      const again = disconnectButton(p, said, a);
      again.textContent = "Disconnect again";
      actions.append(again);
      head.append(who, actions);
      item.append(head, said);
      list.append(item);
      continue;
    }
    if (a.state !== "connected") actions.append(connectButton(p, "Connect again", said, { account: a.id }));
    if (a.state === "connected" && a.missing?.length) {
      const again = connectButton(p, "Reconnect to add", said, { account: a.id });
      again.dataset.action = "reconnect";
      actions.append(again);
      details.push(notAllowed(a.missing));
    }
    actions.append(disconnectButton(p, said, a));
    head.append(who, actions);
    item.append(head, ...details, said);
    list.append(item);
  }
  return list;
}
// Which of the person's accounts an agent uses there: Default (the oldest
// connected) first, then each of theirs, its state said where it is not
// connected, and the agent's saved pick as it is stored, gone or not, so
// a pick the platform refuses (one to reconnect) is never shown as another.
// Shown with two or more accounts, or a saved pick to change.
function accountPick(p, a, theirs) {
  const pick = el("select", "provider-account-pick");
  pick.dataset.agent = a.fragment;
  pick.setAttribute("aria-label", `${titleOf(a.fragment)}'s ${providerName(p.provider)} account`);
  const option = (value, text) => {
    const o = el("option", "", text);
    o.value = value;
    pick.append(o);
  };
  const first = theirs.find((c) => c.state === "connected");
  option("", first ? `Default (${accountLabel(p, first)})` : "Default");
  for (const c of theirs) option(c.id, c.state === "connected" ? accountLabel(p, c) : `${accountLabel(p, c)} (needs reconnecting)`);
  const mine = a.accounts?.[p.provider];
  if (mine && !theirs.some((c) => c.id === mine)) option(mine, "An account no longer connected (the default is used)");
  pick.value = mine ?? "";
  pick.onchange = async () => {
    const visit = visits;
    pick.disabled = true;
    // Default is no pick: the agent follows it
    const account = pick.value || null;
    let refused = null;
    try {
      computerIs(await api("PUT", `/api/computers/${seg(state.computer.computer)}/agents/${seg(a.fragment)}/accounts/${encodeURIComponent(p.provider)}`, { account }));
      state.agents = new Map((state.computer?.agents ?? []).map((x) => [x.identity, x]));
    } catch (e) {
      refused = e;
    }
    await reopenSettings(refused && visit === visits ? ["That was not changed", refused.message] : null);
  };
  return pick;
}
function connectionsSection(linked, uses) {
  const s = section("Connections");
  s.id = "settings-connections";
  const offered = linked?.providers ?? [];
  if (!offered.length) {
    s.append(el("p", "muted", linked ? "This platform offers none yet." : "Your connections could not be read."));
    return s;
  }
  s.append(el("p", "muted", "Your agents use these through their computer, which adds each credential on the way out. None is ever on the computer."));
  const agents = state.computer?.agents ?? [];
  for (const p of offered) {
    const row = el("div", "provider");
    row.dataset.provider = p.provider;
    row.dataset.kind = p.kind;
    row.dataset.state = p.state;
    const head = el("div", "provider-head");
    const identity = el("div", "provider-identity");
    // an own key signed in to is the person's account there, its models theirs to pick
    const kind = p.signIn ? (p.models?.length ? "Your account: its models for your agents" : "Your account") : PROVIDER_KIND[p.kind] ?? p.kind;
    identity.append(el("span", "provider-name", providerName(p.provider)), el("span", "provider-kind", kind));
    const stateName = p.signIn ? (p.state === "set" ? "Connected" : "Not connected") : PROVIDER_STATE[p.state] ?? p.state;
    const st = el("span", `provider-state ${p.state}`, stateName);
    identity.append(st);
    // which account a connection is: the person's sign-in here may be another
    // (of several, each is named in its own row)
    const several = Array.isArray(p.accounts);
    if (p.account && !several) {
      const account = el("span", "provider-account", p.account);
      account.dataset.account = p.account;
      identity.append(account);
    }
    head.append(identity);
    row.append(head);
    // the deployment's word on it (its catalog row's note), connected or not
    if (p.note) row.append(el("span", "muted provider-detail provider-note", p.note));
    const actions = el("div", "settings-actions");
    const details = [];
    // what a Connect press says: a blocked window, or a start refused; and
    // a Disconnect or Remove that did not go through
    const said = el("span", "muted provider-detail provider-said");
    // Disconnect or Remove: settings then reload, and the row they show says
    // whether the key is still there; one that failed says so in it
    const removeKey = (button) => async () => {
      const visit = visits;
      button.disabled = true;
      unremoved.delete(p.provider);
      try {
        await api("DELETE", `/api/connections/${encodeURIComponent(p.provider)}/key`);
      } catch {
        // kept only while its visit is the one shown: an older visit's
        // answer never replaces a newer one's
        if (visit === visits && state.page === "Settings") unremoved.set(p.provider, visit);
      }
      await reopenSettings();
    };
    if (p.kind === "connection" && p.state !== "connected" && !(several && p.accounts.some((a) => !a.disconnecting))) {
      actions.append(connectButton(p, p.state === "needs_reauthorization" ? "Connect again" : "Connect", said));
      details.push(said);
    }
    // of several, another while the platform takes more; each account is
    // reconnected and disconnected in its own row
    // theirs to use: none they disconnected (which Pipes may hold still)
    const theirs = several ? p.accounts.filter((a) => !a.disconnecting) : [];
    if (several && theirs.length && theirs.length < (p.accountsMax ?? 1)) {
      const add = connectButton(p, `Add another ${providerName(p.provider)} account`, said, { add: true });
      add.dataset.action = "add-account";
      actions.append(add);
      details.push(said);
    }
    // what the person left unticked when they connected it: a reconnect
    // asks for it again (the provider keeps what they allowed before)
    if (!several && p.kind === "connection" && p.state === "connected" && p.missing?.length) {
      const again = connectButton(p, "Reconnect to add", said);
      again.dataset.action = "reconnect";
      actions.append(again);
      details.push(notAllowed(p.missing), said);
    }
    if (!several && p.kind === "connection" && p.state !== "not_connected") {
      actions.append(disconnectButton(p, said));
      if (!details.includes(said)) details.push(said);
    }
    if (p.kind === "own" && p.signIn) {
      // connected through the provider's own page (never a key pasted here):
      // it makes a key of the person's, which their computer keeps sealed
      if (p.state !== "set") {
        const go = connectButton(p, "Connect", said);
        go.dataset.action = "sign-in";
        actions.append(go);
        details.push(said);
        if (p.models?.length) details.push(el("span", "muted provider-detail", `Your ${providerName(p.provider)} account pays for its models; then pick one for an agent under Agents.`));
      } else {
        const off = el("button", "quiet", "Disconnect");
        off.type = "button";
        off.dataset.action = "disconnect";
        off.onclick = removeKey(off);
        const manage = el("a", "quiet", `Revoke the key at ${providerName(p.provider)}`);
        manage.href = p.signIn.manage;
        manage.target = "_blank";
        manage.rel = "noopener";
        actions.append(off, manage);
      }
    } else if (p.kind === "own") {
      const form = el("form", "provider-key");
      const input = el("input");
      input.type = "password";
      input.autocomplete = "off";
      input.placeholder = p.state === "set" ? "Replace your key" : `Your ${providerName(p.provider)} key`;
      const save = el("button", "quiet", "Save");
      save.type = "submit";
      form.append(input, save);
      form.onsubmit = async (e) => {
        e.preventDefault();
        save.disabled = true;
        try {
          await api("PUT", `/api/connections/${encodeURIComponent(p.provider)}/key`, { key: input.value });
          await reopenSettings();
        } catch (err) {
          save.textContent = err.message;
          save.disabled = false;
        }
      };
      actions.append(form);
      if (p.state === "set") {
        const remove = el("button", "quiet", "Remove");
        remove.type = "button";
        remove.dataset.action = "remove-key";
        remove.onclick = removeKey(remove);
        actions.append(remove);
      }
    }
    if (p.state === "set" && unremoved.get(p.provider) === visits) {
      said.textContent = "That didn't go through. Try again.";
      details.push(said);
    } else unremoved.delete(p.provider);
    if (p.kind === "operator" && p.price) {
      details.push(el("span", "muted provider-detail", `${money(p.price.micros / p.price.per)} a call at list, metered to your credit`));
    }
    if (actions.childElementCount) head.append(actions);
    row.append(...details);
    if (several && p.accounts.length) row.append(accountsList(p));
    // which agents may use it: all by default; one narrowed to a list uses
    // those; of two or more accounts, or a pick saved, which each uses
    if (agents.length) {
      const who = el("div", "provider-agents");
      who.append(el("span", "settings-key", "Agents"));
      for (const a of agents) {
        const allowed = a.connections == null || a.connections.includes(p.provider);
        const chip = el("button", "chip", titleOf(a.fragment));
        chip.type = "button";
        chip.dataset.agent = a.fragment;
        chip.setAttribute("aria-pressed", String(allowed));
        chip.title = allowed ? `${titleOf(a.fragment)} may use it: press to take it away` : `${titleOf(a.fragment)} may not use it: press to let it`;
        chip.onclick = async () => {
          const visit = visits;
          chip.disabled = true;
          const all = offered.map((x) => x.provider);
          const now = a.connections ?? all;
          const next = allowed ? now.filter((x) => x !== p.provider) : [...new Set([...now, p.provider])];
          const list = all.every((x) => next.includes(x)) ? null : next;
          let refused = null;
          try {
            computerIs(await api("PUT", `/api/computers/${seg(state.computer.computer)}/agents/${seg(a.fragment)}/connections`, { connections: list }));
            state.agents = new Map((state.computer?.agents ?? []).map((x) => [x.identity, x]));
          } catch (e) {
            refused = e;
          }
          // a failure is said by the reload, in the visit it was pressed in,
          // while the agent may use it or not as before (a newer press may
          // have changed it since)
          const was = state.agents.get(a.identity) ?? a;
          const unchanged = (was.connections == null || was.connections.includes(p.provider)) === allowed;
          await reopenSettings(refused && unchanged && visit === visits ? ["That was not changed", refused.message] : null);
        };
        who.append(chip);
        if (allowed && several && (theirs.length >= 2 || a.accounts?.[p.provider])) who.append(accountPick(p, a, theirs));
      }
      row.append(who);
    }
    // this month's calls through the swap, by agent, and an operator key's cost
    const mine = (uses?.uses ?? []).filter((u) => u.provider === p.provider);
    const used = el("div", "provider-uses");
    used.append(el("span", "settings-key", uses?.month ? `In ${uses.month}` : "This month"));
    if (!mine.length) used.append(el("span", "muted", "No calls yet"));
    for (const u of mine) {
      const calls = `${u.calls} ${u.calls === 1 ? "call" : "calls"}`;
      const item = el("span", "use", `${titleOf(u.agent)}: ${p.kind === "operator" ? `${calls}, ${money(u.micros)}` : calls}`);
      item.dataset.use = u.agent;
      item.dataset.calls = String(u.calls);
      item.dataset.micros = String(u.micros);
      used.append(item);
    }
    row.append(used);
    s.append(row);
  }
  return s;
}

// ---- skills (decision 17): the managed set, the person's skills fragment's
// (the platform's release, which their computers install for every agent),
// by category, and each agent's own (its fragment's skills/, which win on a
// name). Both read as the agents' computers read them: the fragments' files.
// A skill is skills/<category>/<name>/SKILL.md, or skills/<name>/SKILL.md.
function skillsIn(paths) {
  const out = [];
  for (const path of paths) {
    const parts = path.split("/");
    if (parts[0] !== "skills" || parts[parts.length - 1] !== "SKILL.md") continue;
    if (parts.length === 4) out.push({ category: parts[1], name: parts[2] });
    else if (parts.length === 3) out.push({ category: "", name: parts[1] });
  }
  return out.sort((a, b) => a.category.localeCompare(b.category) || a.name.localeCompare(b.name));
}
const skillsFragmentOf = () => state.fragments.find((f) => f.kind === "skills" && own(f));
// The person's skills fragment, made from the blessed template when they
// have none: at setup, as their default agent is, or from settings' Skills
// for a person without one (deleted, or set up before 2026-10-03).
async function skillsFragment() {
  const have = skillsFragmentOf();
  if (have) return have.name;
  const made = await api("POST", "/api/fragments", { label: freeLabel("skills"), template: "skills" });
  await load();
  return made.name;
}
function skillNames(list) {
  const value = el("span", "settings-value");
  list.forEach((s, i) => {
    if (i) value.append(", ");
    const name = el("span", "skill", s.name);
    name.dataset.skill = s.name;
    value.append(name);
  });
  return value;
}
async function skillsSection() {
  const s = section("Skills");
  s.id = "settings-skills";
  const filesOf = (name) => api("GET", `/api/f/${name}/files`).then((v) => (v.files ?? []).map((f) => f.path)).catch(() => null);
  const mine = skillsFragmentOf();
  if (!mine) {
    const add = el("button", "quiet", "Add the managed skills");
    add.type = "button";
    add.onclick = () => skillsFragment().then(() => reopenSettings()).catch((e) => { add.textContent = e.message; });
    s.append(el("p", "muted", "Your agents have no managed skills yet."), actionLine("Managed skills", add));
  } else {
    const paths = await filesOf(mine.name);
    if (!paths) s.append(el("p", "muted", "Your skills could not be read."));
    else {
      const groups = new Map();
      for (const k of skillsIn(paths)) groups.set(k.category, [...(groups.get(k.category) ?? []), k]);
      s.dataset.fragment = mine.name;
      s.append(...[...groups].map(([category, list]) => {
        const row = el("p", "settings-line");
        row.dataset.category = category || "general";
        row.append(el("span", "settings-key", (category || "general").replace(/-/g, " ")), skillNames(list));
        return row;
      }));
      const open = el("button", "quiet", "Open your skills");
      open.type = "button";
      open.onclick = () => openApp(mine.name);
      s.append(actionLine("Managed skills", open));
    }
  }
  // each agent's own skills, from its fragment
  const agents = state.fragments.filter((f) => f.kind === "agent" && own(f));
  const owns = await Promise.all(agents.map((a) => filesOf(a.name)));
  agents.forEach((a, i) => {
    const row = el("p", "settings-line");
    row.dataset.agent = a.name;
    const list = skillsIn(owns[i] ?? []);
    row.append(el("span", "settings-key", `${titleOf(a.name)}'s own`), list.length ? skillNames(list) : el("span", "settings-value muted", owns[i] ? "None yet" : "Not read"));
    s.append(row);
  });
  return s;
}
$("settings").onclick = () => openSettings().catch((e) => notice("Settings did not open", e.message));

// ---- first run: sign in, a seat, then the default agent is made
// (decision 10); it never goes through settings ----
// Match the chat composer's rim reflection, including its 64px approach
// and rounded corners, on every first-run card (Skyler's onboarding design,
// 2026-10-22). Listen once: firstRun replaces only the card's children.
const splashPointer = matchMedia("(hover: hover) and (pointer: fine)");
let splashEdgeFrame = 0;
let splashEdgePointer = null;
function clearSplashEdge() {
  cancelAnimationFrame(splashEdgeFrame);
  splashEdgeFrame = 0;
  $("first-run-card").style.setProperty("--edge-light", "0");
}
$("first-run").addEventListener("pointermove", (event) => {
  if (!splashPointer.matches || event.pointerType === "touch") return;
  splashEdgePointer = { x: event.clientX, y: event.clientY };
  if (splashEdgeFrame) return;
  splashEdgeFrame = requestAnimationFrame(() => {
    splashEdgeFrame = 0;
    const card = $("first-run-card");
    const rect = card.getBoundingClientRect();
    const x = splashEdgePointer.x - rect.left;
    const y = splashEdgePointer.y - rect.top;
    const radius = parseFloat(getComputedStyle(card).borderRadius);
    const qx = Math.abs(x - rect.width / 2) - rect.width / 2 + radius;
    const qy = Math.abs(y - rect.height / 2) - rect.height / 2 + radius;
    const distance = Math.max(0, Math.hypot(Math.max(qx, 0), Math.max(qy, 0)) + Math.min(Math.max(qx, qy), 0) - radius);
    const proximity = Math.max(0, 1 - distance / 64);
    card.style.setProperty("--edge-x", `${x}px`);
    card.style.setProperty("--edge-y", `${y}px`);
    card.style.setProperty("--edge-light", String(proximity * proximity));
  });
});
$("first-run").addEventListener("pointerleave", clearSplashEdge);
$("first-run").addEventListener("scroll", clearSplashEdge, { passive: true });
addEventListener("blur", clearSplashEdge);
addEventListener("resize", clearSplashEdge);
// The two ways the shell is first drawn (its first run's card, here, and
// its layout, `showLayout`) take away "Still loading…" (index.html), so it
// is never said beside the shell, and is said while nothing is drawn: its
// scripts not come, or their first answers.
function firstRun(...children) {
  $("still-loading")?.remove();
  clearSplashEdge();
  stopCardStep();
  $("layout").hidden = true;
  $("first-run").hidden = false;
  $("first-run-year").textContent = String(new Date().getFullYear());
  $("first-run-card").replaceChildren(...children);
}
// One step of the first run's card to another, in place (the welcome's
// seat and its trial code: billing.js seatOffer): the card's height moves
// from the one to the other and the new step's parts rise in, as Skyler's
// design moves them; at once with reduced motion. Focus that was in the
// card goes to the new step's heading.
let cardMotion = { run: 0, animations: [] };
function stopCardStep() {
  cardMotion.run++;
  for (const a of cardMotion.animations) a.cancel();
  cardMotion.animations = [];
  $("first-run-card").classList.remove("moving");
}
async function cardStep(from, to) {
  const card = $("first-run-card");
  const before = card.getBoundingClientRect().height;
  const focused = card.contains(document.activeElement);
  stopCardStep();
  const run = cardMotion.run;
  from.hidden = true;
  to.hidden = false;
  const heading = to.querySelector("h1");
  if (focused && heading) {
    heading.tabIndex = -1;
    heading.focus({ preventScroll: true });
  }
  if (!before || matchMedia("(prefers-reduced-motion: reduce)").matches) return;
  const after = card.getBoundingClientRect().height;
  const ease = "cubic-bezier(.22, 1, .36, 1)";
  card.classList.add("moving");
  cardMotion.animations = [
    card.animate([{ height: `${before}px` }, { height: `${after}px` }], { duration: 520, easing: ease }),
    ...[...to.children].map((part, i) => part.animate([{ opacity: 0, transform: "translateY(9px)" }, { opacity: 1, transform: "none" }], { duration: 380, delay: 65 + Math.min(i, 8) * 45, easing: ease, fill: "backwards" })),
  ];
  await Promise.all(cardMotion.animations.map((a) => a.finished.catch(() => {})));
  if (run === cardMotion.run) stopCardStep();
}
function showLayout() {
  $("still-loading")?.remove();
  $("first-run").hidden = true;
  $("layout").hidden = false;
}
const PITCH = "Welcome to Finite";
const SUBTITLE = "Superintelligent agents for you and your organization";
function signIn() {
  // back to this view, with what its address carries (a mailed trial's
  // code), whichever screen of AuthKit's each opens on. Sign In is a
  // sign-in, not a switch: one WorkOS answers from a session it holds in
  // the browser comes back here too, as that account.
  const back = location.pathname + location.search;
  const go = el("a", "primary", "Sign Up");
  go.href = `/auth/login?screen_hint=sign-up&return=${encodeURIComponent(back)}`;
  const other = el("a", "secondary splash-secondary", "Sign In");
  other.href = `/auth/login?screen_hint=sign-in&return=${encodeURIComponent(back)}`;
  firstRun(el("h1", null, PITCH), el("p", null, SUBTITLE), go, other);
}
// "Use another account" (`POST /auth/switch`, docs/api.md, Sign-in), back
// to `back`: WorkOS keeps its own sign-in in the browser (clearing this
// site's data leaves it), and Sign in comes back as that account without
// asking. A switch ends this session here, if there is one, and that one
// there (whose logout comes back to the platform's root, not `back`), so
// the next sign-in asks who is signing in. A form: the shell's
// `form-action 'self'` lets the browser leave for WorkOS only from the
// page its answer is.
function switchForm(back, cls = "quiet") {
  const other = el("form");
  other.method = "post";
  other.action = `/auth/switch?return=${encodeURIComponent(back)}`;
  const use = el("button", cls, "Use another account");
  use.type = "submit";
  other.append(use);
  return other;
}
// Who is signed in, wherever someone is told they have no seat (a guest:
// `GET /api/seat`): a browser's autofill can sign someone in as an email
// their seat is not under, a new guest, whose seat waits under the other
// (2026-10-20, in production). The email is their latest sign-in's.
function noSeatSaid() {
  const p = el("p", "no-seat");
  if (state.me.email) p.append("You're signed in as ", el("strong", null, state.me.email), ". ");
  p.append("This account has no seat. If your team gave you a seat under another email, use another account.");
  return p;
}
// A new person's welcome (docs/billing.md, "The shell"): what this is, the
// one seat and a trial code, then Stripe's Checkout, whose return comes
// back here (`?checkout=<session>`, or `?checkout=canceled`), as a mailed
// trial's link does (`?trial=<code>`). Someone who is no guest (a seat
// bought, an org's claimed at sign-in, or the deployment's plan) goes home,
// or on to the first agent; a guest is offered the seat, whatever they
// own (one whose seat ended keeps their chats).
async function welcome() {
  const q = new URLSearchParams(location.search);
  const back = q.get("checkout");
  const paid = back && back !== "canceled" ? back : null;
  const goHome = () => {
    history.replaceState(null, "", "/");
    return start();
  };
  if (paid && chats().some(own)) {
    // a seat paid for again: applied, then home
    await api("POST", `/api/billing/sessions/${encodeURIComponent(paid)}`, {}).catch(() => {});
    return goHome();
  }
  if (paid) return creatingAgent({ session: paid });
  const mine = await api("GET", "/api/seat").catch(() => null);
  if (mine) setGuest(mine.guest);
  if (!state.guest) return chats().some(own) ? goHome() : creatingAgent(mine?.seat ? {} : null);
  history.replaceState(null, "", WELCOME);
  // LATER (fbfc's import; not built): "Import your finite.computer agent",
  // a choice beside the seats for someone moving their agent here. Its
  // place is kept, hidden, until the import is built.
  const imports = el("button", "plan", "Import your finite.computer agent");
  imports.type = "button";
  imports.dataset.slot = "import";
  imports.hidden = true;
  // who is signed in, and another account (back here, its trial code kept),
  // under whichever step the card shows
  const trial = q.get("trial");
  const foot = el("div", "welcome-foot");
  foot.append(noSeatSaid(), switchForm(trial ? `${WELCOME}?trial=${encodeURIComponent(trial)}` : WELCOME, "text"));
  // someone shared something with them first, or invited them: it is still
  // theirs to open, or to answer
  if (state.fragments.length || state.invites.length) {
    const home = el("a", "text", "Not now");
    home.href = "/";
    foot.append(home);
  }
  const out = el("form");
  out.method = "post";
  out.action = "/auth/logout";
  const leave = el("button", "text", "Sign out");
  leave.type = "submit";
  out.append(leave);
  foot.append(out);
  const canceled = el("div", "ticket");
  canceled.append(el("p", null, "Checkout was canceled: nothing was bought."));
  const top = [el("h1", null, PITCH), el("p", null, SUBTITLE), ...(back === "canceled" ? [canceled] : [])];
  firstRun(...seatOffer({ api, el, step: cardStep }, mine, trial, top), imports, foot);
}
// A seat before the first agent. Checkout's return names its session,
// applied at once (whatever Stripe's webhook's timing; again, it changes
// nothing) and kept in the address until its seat holds, so a reload
// applies it again. Then the seat's plan reaches the ledger from the
// registry, a moment after, and the first run's creates are a seat's.
// Bounded: SEAT_WAIT_MS.
const SEAT_WAIT_MS = 30_000;
async function seated(session) {
  if (session) {
    // a refusal said plainly, with what to do (Try again retries it; the
    // platform's words go to the console, for us)
    const mine = await api("POST", `/api/billing/sessions/${encodeURIComponent(session)}`, {}).catch((e) => {
      console.warn("A seat's Checkout return was not applied:", e);
      throw new Error(
        e.status === 404
          ? "This payment wasn't found. Choose a seat to start again."
          : "We couldn't confirm your payment yet. If you were charged, try again in a moment.",
      );
    });
    if (!mine.seat?.good) throw new Error("Your payment has not reached us yet: try again in a moment.");
    history.replaceState(null, "", "/");
  }
  const t0 = Date.now();
  for (;;) {
    const ledger = await api("GET", "/api/ledger").catch(() => null);
    if (ledger && ledger.plan !== "guest") return;
    if (Date.now() - t0 > SEAT_WAIT_MS) throw new Error("Your seat has not reached your account yet: try again in a moment.");
    await new Promise((r) => setTimeout(r, 500));
  }
}
// The first agent (Paul, 2026-10-03: no question asked): the person's
// default agent, in charge, made with its computer and its chat. The chat
// is made first and opens at once (Skyler, 2026-10-12), so they may write
// while the rest is made; its page sends only once the agent is a member
// (a message from before its joining starts nothing: docs/chat-records.md).
// What the chat shows of the making is the platform's word alone (Paul,
// 2026-10-13: nothing fake or optimistic): a step is done once its call
// answered, the computer's phase is its view's, and nothing is said in the
// agent's name. Nothing of it is kept in this browser: a reload reads the
// list again (`owesFirstAgent`) and goes on from what was made.
const SETUP_WAIT_MS = 4 * 60_000;
// the first run while it runs, or after it failed (Try again, in its chat):
// { label, title, chat, made, joined, ready, error }; null otherwise
let setup = null;
let setupRun = null;
// What the first run's chat shows of it (the roster's `setup`, to that
// chat's frame alone): each step done, being done, not yet, or failed,
// and what failed.
function setupFor(name) {
  if (!setup || setup.chat !== name) return null;
  const t = setup.title;
  const step = (text, done, now) => ({ text, state: done ? "done" : !now ? "todo" : setup.error ? "failed" : "doing" });
  return {
    steps: [
      step(setup.made ? `${t} is made` : `Making ${t}`, setup.made, !setup.made),
      step(setup.joined ? `${t} is in this chat` : `Adding ${t} to this chat`, setup.joined, setup.made && !setup.joined),
      step(setup.ready ? `${t} is ready` : state.computer?.phase === "awake" ? `${t} is starting on its computer` : `Its computer: ${PHASE[state.computer?.phase] ?? "not made yet"}`, setup.ready, setup.joined && !setup.ready),
    ],
    error: setup.error,
  };
}
function setupChanged(change) {
  Object.assign(setup, change);
  rosterChanged();
}
// Its job (its SOUL.md, written once: an agent already set up keeps its
// own), never its name: that is its agent fragment's title alone, which a
// rename changes (Paul, 2026-10-16: renamed, its job still said the old one).
const firstSoul = (owner) => `You are ${owner}'s default agent: the first one they talk to, and in charge of the rest. Help with whatever they ask. When a job would be better as an app, or as an agent of its own, say so and offer to set it up. The first time you talk, say hello briefly and ask what they'd like to start with.\n`;
// Its chat's own title (Paul, 2026-10-16), never the agent's name, which the
// sidebar shows above it.
const FIRST_CHAT = "Your first chat";
// The name of NAMES an earlier try's label was made from (`freeLabel(slug(name))`,
// maybe with its `-<n>`), so a retry names its agent as that try would have.
const nameOfLabel = (label) => NAMES.find((n) => new RegExp(`^${slug(n)}(-[0-9]+)?$`).test(label));
// a chat an earlier try made before its agent: theirs, `<label>-chat`, no agent in it
const unjoined = (f) => own(f) && !f.agents?.length && /-chat$/.test(labelOf(f.name));
// Whether the first run is owed, from the platform alone: a seat's person
// with no agent of theirs in a chat of their own. No chats at all; or only
// the chat an earlier try made; or their agent not on their computer, or
// not a member of its own chat (asked of the chat: a row may not name its
// agents yet). With chats, and their computer's read failed, their agents
// are unknown, not none: the question is put off until a read answers
// (`resumeFirstRun`).
async function owesFirstAgent() {
  if (state.guest) return false;
  if (!chats().length) return true;
  if (state.computerFailed) {
    firstRunPutOff = true;
    return false;
  }
  if (chats().some((f) => own(f) && (f.agents ?? []).some((id) => state.agents.has(id)))) return false;
  const agent = primaryAgent();
  if (!agent) return chats().every(unjoined);
  const { identity: id } = agent;
  const itsChat = agentsOwnChat(agent.fragment);
  if (!id || !itsChat) return true;
  const members = await api("GET", `/api/f/${seg(itsChat.name)}/members`).catch(() => null);
  return !!members && !(members.members ?? []).some((m) => m.principal === id);
}
// The first run's question, put off while their computer could not be read
// (`owesFirstAgent`), is asked again by each read that answers (`load`:
// Try again's, or the list's socket's) and each arrival home (`openChat`,
// the one way there), while the person is at home, and after it too (they
// may have gone to Settings meanwhile: then their way back asks it). Owed,
// it is made once (`creatingAgent`).
let firstRunPutOff = false;
const atHome = () => location.pathname === "/" && !state.page;
async function resumeFirstRun() {
  if (!atHome()) return;
  firstRunPutOff = false;
  const owed = await owesFirstAgent();
  if (!atHome()) firstRunPutOff = true;
  else if (owed) creatingAgent();
}
// Each step reuses what an earlier try made, including a create whose
// response was lost. The chat's frame opens once: a draft in it stays put.
async function defaultAgent() {
  await load();
  const owner = state.me.email || "your person";
  if (!setup) {
    // an earlier try's names, by what it made: its agent, else its chat's
    // label (the chat's title is its own)
    const agent = primaryAgent();
    const chat = agent ? null : chats().find(unjoined);
    const earlier = agent ? labelOf(agent.fragment) : chat ? labelOf(chat.name).slice(0, -"-chat".length) : null;
    const title = agent ? titleOf(agent.fragment) : (earlier && nameOfLabel(earlier)) || pickName();
    setup = { label: earlier ?? freeLabel(slug(title)), title, chat: null, made: false, joined: false, ready: false, error: null };
  }
  setupChanged({ error: null });
  const { title, label } = setup;
  const chatName = state.fragments.find((f) => f.kind === "chat" && own(f) && labelOf(f.name) === `${label}-chat`)?.name ??
    (await api("POST", "/api/fragments", { label: `${label}-chat`, template: "chat", title: FIRST_CHAT })).name;
  setupChanged({ chat: chatName });
  await load();
  if (!state.frames.has(chatName)) openChat(chatName);

  // These can run together once the chat is on screen. The computer is
  // assigned before it wakes, with the agent's configuration deployed.
  const prepared = await Promise.allSettled([skillsFragment(), computerOf()]);
  // Finish both before offering Retry: no previous create is left in flight.
  for (const result of prepared) if (result.status === "rejected") throw result.reason;
  const computer = prepared[1].value;
  let agent = state.fragments.find((f) => f.kind === "agent" && own(f) && labelOf(f.name) === label);
  if (!agent) agent = await api("POST", "/api/fragments", { label, template: "agent", title });
  const assigned = await api("PUT", `/api/computers/${seg(computer.computer)}/agents/${seg(agent.name)}`, {});
  const id = assigned.agents.find((a) => a.fragment === agent.name)?.identity;
  if (!id) throw new Error("its computer did not take it");
  // its job and tier, written once: an agent already set up keeps its own
  const written = await api("GET", `/api/f/${seg(agent.name)}/file?path=agent.json`).then(() => true, (e) => { if (e.status === 404) return false; throw e; });
  if (!written) {
    await api("POST", `/api/f/${agent.name}/files`, {
      key: "agent-default",
      message: "the default agent",
      files: [{ path: "SOUL.md", text: firstSoul(owner) }, { path: "agent.json", text: JSON.stringify({ tier: DEFAULT_AGENT_TIER }, null, 2) + "\n" }],
    });
  }
  await api("POST", `/api/f/${agent.name}/deploy`, {});
  setupChanged({ made: true });
  // its joining is what lets the chat's page send (and wakes its computer)
  await api("PUT", `/api/f/${chatName}/members/${seg(id)}`, { role: "editor" });
  setupChanged({ joined: true });
  await hello(agent.name, chatName);
  api("POST", `/api/computers/${seg(computer.computer)}/wake`, {}).then(computerIs).catch(() => {});
  await load();
  return { chat: chatName, id, computer: computer.computer };
}
// Ready: its computer awake, and the agent following its chat (its wake
// subscription, which its guest makes as it follows: the chat's owner sees
// it). The computer as it answered is shown at once (its phase, in the chat).
async function agentReady({ chat, id, computer }) {
  const [c, subs] = await Promise.all([
    api("GET", "/api/computers").then((v) => v.computers?.find((x) => x.computer === computer)),
    api("GET", `/api/f/${chat}/subscriptions`),
  ]);
  computerIs(c);
  // a wake refused (its owner's credit) is said, not waited out
  if (c?.why && c.phase !== "awake") throw Object.assign(new Error(c.why), { refused: true });
  return c?.phase === "awake" && (subs.subscriptions ?? []).some((x) => x.wake && x.principal === id && x.channel === "chat");
}
// `seat` (`{session?}`): a seat comes first (`seatFirst`), then the agent.
function creatingAgent(seat = null) {
  if (seat) return seatFirst(seat);
  if (setupRun) return setupRun;
  history.replaceState(null, "", "/");
  showLayout();
  // until its chat is made, the middle column says what is being made
  if (!setup?.chat || !state.frames.has(setup.chat)) notice("Making your agent…");
  setupRun = (async () => {
    const made = await defaultAgent();
    const t0 = Date.now();
    for (;;) {
      let ready = false;
      try {
        ready = await agentReady(made);
      } catch (e) {
        if (e.refused) throw e;
      }
      if (ready) break;
      if (Date.now() - t0 > SETUP_WAIT_MS) throw new Error("Its computer is taking longer than usual to start. What you send waits for it; Try again looks again.");
      await new Promise((r) => setTimeout(r, 1500));
    }
    setupChanged({ ready: true });
    setup = null;
    rosterChanged();
    await load();
  })().catch((err) => {
    if (setup) setupChanged({ error: err.message || "It did not answer." });
    if (!setup?.chat || !state.frames.has(setup.chat)) {
      const retry = el("button", "primary", "Try again");
      retry.type = "button";
      retry.onclick = () => creatingAgent();
      notice("Your agent could not start", err.message, retry);
    }
  }).finally(() => { setupRun = null; });
  return setupRun;
}
// A seat before the first agent (the welcome's: a Checkout's return, or a
// seat given whose plan is on its way): the welcome's card says it is
// being set up until the platform says it holds (`seated`; Checkout's
// return stays in the address until then, so a reload applies it again),
// then the agent is made in its chat (`creatingAgent`). Refused, it says
// why, with Try again and Choose a seat (the welcome's seats again).
function seatFirst(seat) {
  const step = el("li", "doing", "Setting up your seat");
  const list = el("ol", "creating-steps ticket");
  list.append(step);
  const error = el("p", "form-error");
  error.hidden = true;
  const retry = el("button", "primary", "Try again");
  retry.type = "button";
  retry.hidden = true;
  retry.onclick = () => creatingAgent(seat);
  const choose = el("button", "text", "Choose a seat");
  choose.type = "button";
  choose.hidden = true;
  choose.onclick = () => {
    history.replaceState(null, "", WELCOME);
    welcome().catch((e) => notice("This page did not load", e.message));
  };
  firstRun(el("h1", null, "Setting up your seat…"), list, error, retry, choose);
  seated(seat.session).then(
    () => {
      step.className = "done";
      // a seat now, as the ledger said: the shell offers what a seat makes
      setGuest(false);
      return creatingAgent();
    },
    (err) => {
      step.className = "failed";
      error.textContent = err.message;
      error.hidden = false;
      retry.hidden = false;
      choose.hidden = false;
    },
  );
}

// ---- the person's things, read again whenever they may have changed ----
// One read of their list is the sidebar: each chat's row names its agents
// and its newest message. After a change their list told this page of
// (`watchList`: `changed`), only the apps whose rows it touched (new, or
// not as they were) have their card read again. An older read's answer
// that comes after a newer one's is not shown.
const reads = { asked: 0, shown: 0 };
async function load(changed = false) {
  const mine = ++reads.asked;
  const [list, computers] = await Promise.all([api("GET", "/api/fragments"), api("GET", "/api/computers").catch(() => null)]);
  if (mine < reads.shown) return;
  reads.shown = mine;
  const before = new Map(state.fragments.map((f) => [f.name, JSON.stringify(f)]));
  state.fragments = list.fragments ?? [];
  // failed, it is not "no computer": what was read before stays, and is said to be old
  const unknown = state.computerFailed && !state.computer;
  state.computerFailed = !computers;
  if (computers) {
    state.computer = computers.computers?.[0] ?? null;
    state.support = computers.support ?? null;
    state.agents = new Map((state.computer?.agents ?? []).map((a) => [a.identity, a]));
  }
  renderChats();
  renderApps();
  renderHeading();
  renderComputer();
  rosterChanged();
  markSeen();
  renderAgentFaces();
  for (const f of chats()) if (own(f) && f.title === UNTITLED && f.preview) titleFromFirst(f.name);
  // answered, it goes on with what a failure put off, whoever asked
  if (computers && unknown) reopenSettings().catch(() => {});
  if (computers && firstRunPutOff) resumeFirstRun();
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

// ---- a new release: the page reloads itself at a safe moment ----
// A page outlives a deploy: a home screen's app or a tab left open runs the
// shell, and its chats' pages, of the release it loaded with while the
// platform under it moves on (Paul's phone, 2026-10-20: after the day's
// deploys one chat would not load in the home screen's app, and did in a
// tab of its own, until the app was quit). So the page names the release
// that answered it (index.html's meta, `RELEASE`), every answer of the
// platform names its own (`x-fragment-deploy`), and once one names another
// the page reloads whole, its chats' and apps' frames with it, where it
// was (its path, its chat, its apps' windows): while it is hidden, or at
// the person's next move (a chat or an app opened). Words typed in a chat
// are its page's to keep (in its own origin's storage, as its person's),
// so they survive this reload as any other; so the shell reloads only
// when no chat's page says it is busy with what no storage keeps (files
// chosen, a voice memo, a message on its way or one that failed, a quote
// chosen: `{fragment: "busy", busy}`,
// told as it changes), and none of this page's own dialogs, menu, fields
// typed in, prompts on their way or writes still going hold it. Nothing
// is asked at the moment of the reload, so nothing can change between an
// answer and the reload; it is decided a moment after the page hid or the
// person moved, so what a page posted just before is heard, and a page
// busy as the reload is under way stops it (`reloadWhenSafe`, its one
// window left). Each release is reloaded for at most
// RELEASE_TRIES times, RELEASE_AGAIN_MS apart (a reload can still meet the
// old release), and the release a reload left is no reason to reload for
// RELEASE_LEFT_MS (a deploy's old side answers for a while). The page asks
// for itself (`HEAD /healthz`) as it is hidden or shown again, comes back
// from the back-forward cache or comes online, at most once a
// RELEASE_LOOK_MS; it hears every answer it reads.
const RELEASE_ID = /^[A-Za-z0-9._-]{1,64}$/;
const RELEASE_HEADER = "x-fragment-deploy";
const RELEASE_LOOK_MS = 60_000;
const RELEASE_LEFT_MS = 10 * 60_000;
const RELEASE_TRIES = 3;
const RELEASE_AGAIN_MS = 60_000;
const WINDOWS_KEPT_MAX = 16;
// a click that opens a chat from the launcher closes its dialog first
const NAVIGATED_SETTLE_MS = 400;
// and the page hidden, a moment for what a chat's page posted just before to be heard
const RELOAD_SETTLE_MS = 100;
// signing out waits at most this long for the chats' pages to let their drafts go
const FORGET_WAIT_MS = 1_000;
const RELOADED_KEY = "shell.release.v1"; // {from, to, at, tries}: this tab's last reload for a release
const WINDOWS_KEY = "shell.windows.v2"; // {who, windows: [{name, path}]}: the app windows to open again
// this tab's own storage, which a reload keeps
const tab = {
  get(key, fallback) { try { return JSON.parse(sessionStorage.getItem(key)) ?? fallback; } catch { return fallback; } },
  set(key, value) { try { sessionStorage.setItem(key, JSON.stringify(value)); return true; } catch { return false; } },
  drop(key) { try { sessionStorage.removeItem(key); } catch {} },
};
// #403's, which kept chats' words here and named no person: let go
tab.drop("shell.unsent.v1");
tab.drop("shell.windows.v1");
const RELEASE = document.querySelector('meta[name="fragment-release"]')?.content ?? "";
// mine: the release this page is (none, an unfilled page: it never reloads);
// next: the release it waits to reload for
const release = { mine: RELEASE_ID.test(RELEASE) ? RELEASE : null, next: null, lookedAt: 0, reloading: false };

// An answer of the platform's, heard: one naming a release to reload for
// is waited on, and reloaded for at once while the page is hidden.
function heard(r) {
  const id = r.headers.get(RELEASE_HEADER);
  if (!release.mine || !id || id === release.mine || id === release.next || !RELEASE_ID.test(id) || !reloadsFor(id)) return;
  release.next = id;
  // what it waits for, for whoever looks (the e2e, a person's devtools)
  document.documentElement.dataset.releaseNext = id;
  if (document.hidden) reloadWhenSafe("hidden");
}
function lastReload() {
  const r = tab.get(RELOADED_KEY, null);
  return r && typeof r.from === "string" && typeof r.to === "string" && Number.isFinite(r.at) && Number.isInteger(r.tries) ? r : null;
}
// Whether another release is one to reload for: not the one this tab's
// last reload left while that deploy may still be going out, nor one it
// reloaded for and still is not, RELEASE_TRIES times or within
// RELEASE_AGAIN_MS of the last.
function reloadsFor(id) {
  const last = lastReload();
  if (!last) return true;
  const since = Date.now() - last.at;
  if (id === last.from && since < RELEASE_LEFT_MS) return false;
  return !(id === last.to && (last.tries >= RELEASE_TRIES || since < RELEASE_AGAIN_MS));
}

// the fields of this page the person typed in
const TYPED = new Set(["text", "search", "email", "url", "tel", "password", "number"]);
const typedIn = new Set();
document.addEventListener("input", (e) => {
  const f = e.target;
  if (f instanceof HTMLTextAreaElement || (f instanceof HTMLInputElement && TYPED.has(f.type))) typedIn.add(f);
}, true);
// What this page itself holds that a reload would lose.
function shellHolds() {
  for (const f of typedIn) if (!f.isConnected) typedIn.delete(f);
  const typed = [...typedIn].some((f) => f.value.trim() && f.getClientRects().length > 0);
  return typed || !!document.querySelector("dialog[open]") || !menu.hidden || writing > 0 || composing.size > 0 || !!pendingLaunch || starting;
}

// The chats' frames whose page said hello (`agents?`, which the chat's
// page sends as it starts), and those of them that say they are busy with
// what no storage keeps. A hello is a new page, busy with nothing yet; a
// frame sent elsewhere (a sign-in it could not make, tried again) is
// neither. A page in a frame that says so wrongly only holds a reload back.
const greeted = new WeakSet();
const busyFrames = new WeakSet();
const frameOfWindow = (win) => [...state.frames.values()].find((f) => f.contentWindow === win);
addEventListener("message", (event) => {
  const d = event.data;
  if (d?.fragment !== "agents?" && d?.fragment !== "busy") return;
  const frame = frameOfWindow(event.source);
  if (!frame) return;
  if (d.fragment === "agents?") {
    greeted.add(frame);
    busyFrames.delete(frame);
  } else if (d.busy === true) {
    busyFrames.add(frame);
    // busy as the reload is under way (a file picked, a memo begun, a
    // message failed while its answer was slow): the reload is stopped
    if (release.reloading) stopReload();
  } else busyFrames.delete(frame);
});
const chatsBusy = () => [...state.frames.values()].some((f) => f.isConnected && busyFrames.has(f));

// Reloads for `release.next` if nothing holds it: as the page is hidden
// (`moment` "hidden"), and as the person opens a chat or an app
// ("navigated"). Decided in a task of its own a moment later
// (RELOAD_SETTLE_MS, NAVIGATED_SETTLE_MS), so what a chat's page posted
// just before is heard first; nothing is asked or awaited. A chat's page
// that says it is busy as the reload is under way stops it (`stopReload`).
// What is left: a page that becomes busy once the new page has begun to
// come in (its answer committed) is gone with it. That window opens only
// while the page is hidden, or as the person themselves moves in it.
function reloadWhenSafe(moment) {
  if (!release.next || release.reloading) return;
  setTimeout(() => {
    if ((moment === "hidden" && !document.hidden) || !release.next || release.reloading || chatsBusy() || shellHolds()) return;
    const last = lastReload();
    const tries = last?.to === release.next ? last.tries + 1 : 1;
    if (!tab.set(WINDOWS_KEY, { who: state.me?.id ?? null, windows: openWindows() }) || !tab.set(RELOADED_KEY, { from: release.mine, to: release.next, at: Date.now(), tries })) return;
    release.reloading = { last };
    location.reload();
  }, moment === "navigated" ? NAVIGATED_SETTLE_MS : RELOAD_SETTLE_MS);
}
// The reload under way, stopped (`window.stop`), as if it had not begun:
// the tab's last reload as it was, no windows kept; `release.next` waits
// on for the next safe moment.
function stopReload() {
  const { last } = release.reloading;
  window.stop();
  release.reloading = false;
  if (last) tab.set(RELOADED_KEY, last);
  else tab.drop(RELOADED_KEY);
  tab.drop(WINDOWS_KEY);
}
// The app windows open, each at the page it opened (`openApp`).
function openWindows() {
  return viewer.keys.filter((k) => k.startsWith("app:")).slice(0, WINDOWS_KEPT_MAX).map((key) => {
    const src = document.querySelector(`#stack .pane[data-key="${CSS.escape(key)}"] iframe`)?.dataset.src ?? "";
    return { name: key.slice("app:".length), path: new URL(src, location.origin).searchParams.get("return") || "/" };
  });
}
// The windows the reload kept, taken as the page loads (a later reload by
// hand opens none), and open again once its chat is, for the person who
// had them open (on settings, which fill the work area, they are let go).
const keptWindows = tab.get(WINDOWS_KEY, null);
tab.drop(WINDOWS_KEY);
function reopenWindows(settings) {
  if (settings || keptWindows?.who !== state.me?.id || !Array.isArray(keptWindows?.windows)) return;
  for (const w of keptWindows.windows.slice(0, WINDOWS_KEPT_MAX)) {
    if (typeof w?.name === "string" && FRAGMENT_NAME.test(w.name) && typeof w.path === "string" && w.path.startsWith("/")) openApp(w.name, w.path);
  }
}

// Signing out, or in with another account, ends the tab's person: what was
// kept for them goes first, its windows here, each chat page's draft
// (`{fragment: "forget-draft"}`, answered `draft-forgotten`, waited for at
// most FORGET_WAIT_MS) and their chats' alerts (`dropAlerts`), then the
// form goes. A draft of theirs a page not
// open here keeps is never shown to anyone else (its page gives a draft
// back only to its own person) and goes when someone else opens it.
document.addEventListener("submit", (e) => {
  const form = e.target;
  if (!(form instanceof HTMLFormElement) || !/^\/auth\/(logout|switch)(\?|$)/.test(form.getAttribute("action") ?? "")) return;
  tab.drop(WINDOWS_KEY);
  const frames = [...state.frames.values()].filter((f) => f.isConnected && greeted.has(f));
  e.preventDefault();
  Promise.all([frames.length ? forgetDrafts(frames) : null, dropAlerts()]).then(() => HTMLFormElement.prototype.submit.call(form));
}, true);
function forgetDrafts(frames) {
  return new Promise((done) => {
    const left = new Set(frames.map((f) => f.contentWindow));
    const timer = setTimeout(() => finish(), FORGET_WAIT_MS);
    const forgotten = (event) => {
      if (event.data?.fragment === "draft-forgotten" && left.delete(event.source) && !left.size) finish();
    };
    const finish = () => {
      clearTimeout(timer);
      removeEventListener("message", forgotten);
      done();
    };
    addEventListener("message", forgotten);
    for (const f of frames) originOf(f.dataset.fragment).then((origin) => f.contentWindow?.postMessage({ fragment: "forget-draft" }, origin), () => {});
  });
}

// the person opened a chat or an app: once the click has settled
function navigated() {
  reloadWhenSafe("navigated");
}
// Asks the platform which release it is now (`HEAD /healthz`), at most
// once a RELEASE_LOOK_MS; then, hidden, reloads if it may.
async function lookForRelease() {
  if (!release.next && release.mine && Date.now() - release.lookedAt >= RELEASE_LOOK_MS) {
    release.lookedAt = Date.now();
    try {
      heard(await fetch("/healthz", { method: "HEAD", cache: "no-store" }));
    } catch {
      // offline (a phone just woken): its coming online asks again
      release.lookedAt = 0;
      return;
    }
  }
  if (document.hidden) reloadWhenSafe("hidden");
}
document.addEventListener("visibilitychange", () => lookForRelease());
addEventListener("pagehide", () => reloadWhenSafe("hidden"));
addEventListener("pageshow", (e) => { if (e.persisted) lookForRelease(); });
addEventListener("online", () => lookForRelease());

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
  const settings = !open && location.pathname === SETTINGS;
  if (!open && !settings && await owesFirstAgent()) return creatingAgent(mine?.seat ? {} : null);
  if (!state.fragments.length && !state.invites.length && !open && !settings && state.guest) return welcome();
  showLayout();
  const pick = open ?? (byName(state.current) ? state.current : (shown(chats())[0] ?? chats()[0])?.name);
  if (settings) await openSettings(false);
  else if (pick) openChat(pick);
  else if (state.guest) guestHome(mine.offer);
  else notice("No chats yet", "Make an agent to start.");
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
