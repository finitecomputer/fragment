// The SimpleX connector's setup, its own page (setup.html, README.md):
// Settings frames it in a dialog of the shell's (cell/shell/connector-setup.js)
// or, where that frame can't sign in (Safari's cookie rule), it opens in a
// tab of its own. Its owner's alone, and nothing to fill in: opened, it
// takes the next step itself, once a visit, each as `state` says it is due:
// 1. the door: the inbox's secret, made by this browser and set toward
//    door.fragment.internal (`__secrets`: written, never read back), then
//    `connect` as the agent's name (`?name=`, Settings' word for it);
// 2. a link: `link`, its invitation's link to open or copy as it comes,
//    and its QR code for their SimpleX app, until their app answers it
//    (`link` watches; an invitation older than its watch is looked at
//    once, `check`);
// 3. done: whom it linked, with Unlink (then Link, for another).
// Each step is the same three parts (its tags, one line, its controls), a
// failure said in its line in a few words, with Try again. It follows
// `state` live (`fragment.live`), so a step's answer draws as it comes.
// Settings binds it as its agent's door as it makes it; `__door` is read
// here only to say so when it is not.
import * as fragment from "./__fragment.js";
import { KIND } from "./frame-messages.js";

const DOOR_HOST = "door.fragment.internal";
const JSON_TYPE = { "content-type": "application/json" };
// how long `link` watches for their app (app.mjs `LINK_WAITS_MS`): an
// invitation older is looked at once as the page opens
const WATCH_MS = 8.5 * 60_000;
// a QR code's quiet zone, in modules, each side
const QUIET = 4;
const SVG_NS = "http://www.w3.org/2000/svg";
const framed = window.parent !== window;

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

function button(text, cls, onclick) {
  const b = el("button", cls, text);
  b.type = "button";
  b.addEventListener("click", onclick);
  return b;
}

const row = (...items) => {
  const r = el("div", "setup-row");
  r.append(...items);
  return r;
};

async function platformJson(path, init) {
  const resp = await fetch(new URL(path, location.href), { credentials: "same-origin", ...init });
  const body = await resp.json().catch(() => ({}));
  if (!resp.ok) throw Object.assign(new Error(body.message || `${resp.status}`), { status: resp.status });
  return body;
}

const randomHex = (n) => [...crypto.getRandomValues(new Uint8Array(n))].map((b) => b.toString(16).padStart(2, "0")).join("");

// An invitation's QR code (images/simplex-door `Qr`): its dark modules as
// one path, each row's runs of them a rectangle, on a light square of its
// own in either theme (a scanner reads dark on light), with its quiet zone.
export function qrCode({ size, dark }, label) {
  const side = size + 2 * QUIET;
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("viewBox", `0 0 ${side} ${side}`);
  svg.setAttribute("class", "setup-qr");
  svg.setAttribute("role", "img");
  svg.setAttribute("aria-label", label);
  svg.setAttribute("shape-rendering", "crispEdges");
  const back = document.createElementNS(SVG_NS, "rect");
  back.setAttribute("width", String(side));
  back.setAttribute("height", String(side));
  back.setAttribute("fill", "#fff");
  const d = [];
  // bounded: the code's size² modules, each looked at once
  for (let y = 0; y < size; y += 1) {
    for (let x = 0; x < size; x += 1) {
      if (dark[y * size + x] !== "1") continue;
      let run = 1;
      while (x + run < size && dark[y * size + x + run] === "1") run += 1;
      d.push(`M${x + QUIET} ${y + QUIET}h${run}v1h-${run}z`);
      x += run - 1;
    }
  }
  const path = document.createElementNS(SVG_NS, "path");
  path.setAttribute("d", d.join(""));
  path.setAttribute("fill", "#000");
  svg.append(back, path);
  return svg;
}

// what it is, a word each (Paul, 2026-10-31), as Settings says it: its
// words end to end between their app and the platform's door
function tags() {
  const t = el("div", "setup-tags");
  const secure = el("span", "setup-tag", "Secure");
  secure.title = "SimpleX's servers can't read these messages";
  t.append(el("span", "setup-tag", "Experimental"), secure);
  return t;
}

// A step's failure in a few words (`state.why`, the door's code): a
// deployment that runs no doors, or any other, which Try again asks again.
const failure = (why) => (why.code === "no_doors" ? "SimpleX isn't available here yet." : "Couldn't reach SimpleX. Try again.");

export async function setup(root) {
  // the shell's light or dark, from its frame's parent at the platform's
  // origin alone (frame-messages.js), heard from the start: the shell says
  // it as the frame loads, before `me` answers
  let platform = null;
  const heard = [];
  const theme = (e) => {
    if (e.origin === platform && (e.data.mode === "light" || e.data.mode === "dark")) document.documentElement.dataset.theme = e.data.mode;
  };
  if (framed) {
    addEventListener("message", (e) => {
      if (e.source !== window.parent || e.data?.fragment !== KIND.theme) return;
      if (platform) theme(e);
      else heard.push(e);
    });
  }
  const me = await fragment.me();
  platform = me.platform;
  heard.splice(0).forEach(theme);

  // a step: its line (or what failed, in its place), and its controls
  const say = (line, ...parts) => {
    const head = [];
    if (!framed) head.push(el("h1", null, "SimpleX"));
    head.push(tags());
    root.replaceChildren(...head, line, ...parts);
    // in a tab, the way back
    if (!framed && platform) {
      const back = el("a", "quiet", "Back to Settings");
      back.href = `${platform}/settings`;
      root.append(row(back));
    }
  };

  if (me.role !== "owner") {
    say(el("p", null, "Only its owner sets this up."));
    return;
  }

  const name = (new URLSearchParams(location.search).get("name") ?? "").trim().slice(0, 64) || "Agent";
  // this visit's steps taken by the page itself, each once (a failure
  // waits for Try again, an unlink for Link)
  const took = new Set();
  // the step under way, asked here: what it is, and the failure and the
  // invitation shown when it was asked (a newer one answers it)
  let waiting = null;
  // what failed here, outside `state` (its secret not set, a call refused)
  let error = "";
  // its door's binding, read once: unread is no word of it either way
  const bound = platformJson("./__door").then((d) => d.bound, () => undefined);
  let state = null;

  const ask = async (step, line, fn) => {
    took.add(step);
    waiting = { step, line, why: state?.why?.at ?? null, invitation: state?.invitation?.at ?? null };
    error = "";
    draw();
    try {
      await fn();
    } catch {
      waiting = null;
      error = "Couldn't reach SimpleX. Try again.";
    }
    draw();
  };
  const connect = () =>
    ask("connect", "Starting…", async () => {
      // the inbox's secret first: once it is here, connecting is all a try
      // cut short needs; the door is given it at the platform's egress
      await platformJson(`./__secrets/SIMPLEX_DOOR_SECRET`, { method: "PUT", headers: JSON_TYPE, body: JSON.stringify({ value: randomHex(32), hosts: [DOOR_HOST] }) });
      await fragment.call("connect", { name });
    });
  const link = () => ask("link", "Making a link…", () => fragment.call("link"));
  // unlinked, it waits for Link
  const unlink = () => (took.add("link"), ask("unlink", "Unlinking…", () => fragment.call("unlink")));

  // whether `state` answers the step under way
  const answers = (s) => {
    if (!waiting) return true;
    if (s.why && s.why.at !== waiting.why) return true;
    if (waiting.step === "connect") return s.connected;
    if (waiting.step === "link") return !!((s.invitation && s.invitation.at !== waiting.invitation) || s.linked);
    return !s.linked && !s.invitation;
  };

  // the step `state` says is due, taken once a visit
  function next(s) {
    if (waiting || error || s.why) return;
    if (!s.connected && !took.has("connect")) return connect();
    if (s.connected && !s.invitation && !s.linked && !took.has("link")) return link();
    if (s.invitation && !s.linked && !took.has("check") && Date.now() - s.invitation.at > WATCH_MS) {
      took.add("check");
      fragment.call("check").catch(() => {});
    }
  }

  async function draw() {
    if (!state) return;
    const s = state;
    if (waiting && answers(s)) waiting = null;
    const again = button("Try again", "", () => (s.connected ? link : connect)());
    if (waiting) return say(el("p", "setup-now", waiting.line));
    if (error) return say(el("p", "setup-error", error), row(again));
    if (s.why) return say(el("p", "setup-error", failure(s.why)), ...(s.why.code === "no_doors" ? [] : [row(again)]));
    if (s.linked) {
      const who = s.linked.name ? `Linked with ${s.linked.name}.` : "Linked.";
      const unbound = (await bound) === null ? [el("p", null, "Not linked to an agent. Set it up from Settings.")] : [];
      return say(el("p", "setup-now", who), ...unbound, row(button("Unlink", "quiet", unlink)));
    }
    if (s.invitation) {
      const url = s.invitation.short || s.invitation.link;
      const open = el("a", null, "Open in SimpleX");
      open.href = url;
      open.target = "_blank";
      open.rel = "noopener";
      const copy = button("Copy link", "quiet", async () => {
        try {
          await navigator.clipboard.writeText(url);
          copy.textContent = "Copied";
        } catch {
          copy.textContent = "Couldn't copy";
        }
      });
      // its links first, what a phone needs; its code under them, to scan
      const code = s.invitation.qr ? [qrCode(s.invitation.qr, "QR code for SimpleX")] : [];
      return say(el("p", "setup-now", "Scan with SimpleX, or open the link."), row(open, copy), ...code);
    }
    if (s.connected) return say(el("p", "setup-now", "Not linked."), row(button("Link", "", link)));
    return say(el("p", "setup-now", "Starting…"));
  }

  fragment.live(
    "state",
    {},
    (s) => {
      state = s;
      draw().then(() => next(s));
    },
    () => {
      error = "Couldn't reach SimpleX. Try again.";
      if (state) draw();
      else say(el("p", "setup-error", error));
    },
  );
}
