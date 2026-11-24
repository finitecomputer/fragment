// The SimpleX connector's setup, its own page (setup.html, README.md),
// drawn as Telegram's is (setup-page.js, Telegram's own, linked): framed in
// Settings' dialog, or a tab of its own. Its owner's alone, and nothing to
// fill in: opened, it
// takes the next step itself, once a visit, each as `state` says it is due:
// 1. the door: the inbox's secret, made by this browser and set toward
//    door.fragment.internal (`__secrets`: written, never read back), then
//    `connect` as the agent's name (`?name=`, Settings' word for it);
// 2. a link: `link`, its invitation's link to open or copy as it comes,
//    and its QR code for their SimpleX app, until their app answers it
//    (`link` watches; an invitation older than its watch is looked at
//    once, `check`);
// 3. done: whom it linked, with Unlink (then Link, for another).
// A failure is said in its step's line in a few words, with Try again. It follows
// `state` live (`fragment.live`), so a step's answer draws as it comes.
// Settings binds it as its agent's door as it makes it; `__door` is read
// here only to say so when it is not.
import * as fragment from "./__fragment.js";
import { button, el, platformJson, randomHex, row, signedIn, stepper } from "./setup-page.js";

const DOOR_HOST = "door.fragment.internal";
const JSON_TYPE = { "content-type": "application/json" };
// how long `link` watches for their app (app.mjs `LINK_WAITS_MS`): an
// invitation still unanswered then is looked at once (`check`)
const WATCH_MS = 8.5 * 60_000;
// how long a step asked here waits for `state` to answer it before it is
// said failed, with Try again: a job that failed outright writes nothing
// (a connect whose door never came up). Its answer, if it comes later,
// still counts (a door's first start may be slow)
const STEP_WAIT_MS = 120_000;
// a QR code's quiet zone, in modules, each side
const QUIET = 4;
const SVG_NS = "http://www.w3.org/2000/svg";

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
const TAG = ["Secure", "SimpleX's servers can't read these messages"];

// A step's failure in a few words (`state.why`, the door's code): a
// deployment that runs no doors, or any other, which Try again asks again.
const failure = (why) => (why.code === "no_doors" ? "SimpleX isn't available here yet." : "Couldn't reach SimpleX. Try again.");

export async function setup(root) {
  const me = await signedIn();
  // a step: its line, and its controls
  const say = stepper(root, { title: "SimpleX", tag: TAG, platform: me.platform });

  if (me.role !== "owner") {
    say(el("p", null, "Only its owner sets this up."));
    return;
  }

  const name = (new URLSearchParams(location.search).get("name") ?? "").trim().slice(0, 64) || "Agent";
  // this visit's steps taken by the page itself, each once (a failure
  // waits for Try again, an unlink for Link); opened on a link or an
  // invitation, it asks none (another tab's unlink is no wish for one)
  const took = new Set();
  // the step under way, asked here: what it is, and the failure and the
  // invitation shown when it was asked (a newer one answers it); and the
  // last step asked here, which Try again asks again
  let waiting = null;
  let last = null;
  let waited = null;
  // the invitation looked at, or to be, as its link's watch ends
  let checking = null;
  // what failed here, outside `state` (its secret not set, a call refused)
  let error = "";
  // its live `state` lost (its socket down), until it answers again
  let lost = false;
  // its door's binding, read once: unread is no word of it either way
  const bound = platformJson("./__door").then((d) => d.bound, () => undefined);
  let state = null;

  const ask = async (step, line, fn) => {
    took.add(step);
    last = step;
    const asked = (waiting = { step, line, why: state?.why?.at ?? null, invitation: state?.invitation?.at ?? null, late: false });
    error = "";
    clearTimeout(waited);
    waited = setTimeout(() => {
      if (waiting !== asked) return;
      asked.late = true;
      draw();
    }, STEP_WAIT_MS);
    draw();
    try {
      await fn();
    } catch {
      // refused before its job began: only this ask's wait ends
      if (waiting !== asked) return;
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

  // whether `state` answers the step under way: a failure newer than the
  // one shown when it was asked, or its own outcome
  const answers = (s) => {
    if (!waiting) return true;
    if (s.why) return s.why.at !== waiting.why;
    if (waiting.step === "connect") return s.connected;
    if (waiting.step === "link") return !!((s.invitation && s.invitation.at !== waiting.invitation) || s.linked);
    return !s.linked && !s.invitation;
  };

  // the step `state` says is due, taken once a visit
  function next(s) {
    if (waiting || error || s.why) return;
    if (!s.connected && !took.has("connect")) return connect();
    if (s.connected && !s.invitation && !s.linked && !took.has("link")) return link();
    if (s.invitation && !s.linked && checking !== s.invitation.at) {
      // answered after its watch ended: looked at once, then (or now,
      // an invitation older than its watch)
      const at = (checking = s.invitation.at);
      const look = () => state?.invitation?.at === at && !state.linked && fragment.call("check").catch(() => {});
      const due = at + WATCH_MS - Date.now();
      if (due > 0) setTimeout(look, due);
      else look();
    }
  }
  const steps = { connect, link, unlink };

  async function draw() {
    if (!state) return;
    const s = state;
    if (lost) return say(el("p", "setup-error", "Couldn't reach SimpleX."));
    if (waiting && answers(s)) waiting = null;
    // the step its failure names, again (whichever tab asked it); one that
    // failed here before its job began, again; else the next due
    const again = button("Try again", "", () => (steps[s.why?.step] ?? steps[last] ?? (s.connected ? link : connect))());
    // a step still unanswered past its wait: said failed, its answer
    // still taken when it comes
    if (waiting?.late) return say(el("p", "setup-error", "Couldn't reach SimpleX. Try again."), row(again));
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
      if (!state && (s.linked || s.invitation)) took.add("link");
      state = s;
      lost = false;
      draw().then(() => next(s));
    },
    // read again by itself as its socket comes back: nothing to press
    () => {
      lost = true;
      if (state) draw();
      else say(el("p", "setup-error", "Couldn't reach SimpleX."));
    },
  );
}
