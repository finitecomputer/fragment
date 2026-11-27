// A connector's setup page, what both draw with: Telegram's (setup.js) and
// SimpleX's, linked (templates/simplex/site), as their look is (setup.css).
// Settings frames each in a dialog of the shell's
// (cell/shell/connector-setup.js) or, where that frame can't sign in
// (Safari's cookie rule), it opens in a tab of its own. Each step is the
// same three parts: its tags, one line, its controls.
import * as fragment from "./__fragment.js";
import { KIND } from "./frame-messages.js";
import { icon } from "./icons.js";

export const framed = window.parent !== window;

export function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

export function button(text, cls, onclick) {
  const b = el("button", cls, text);
  b.type = "button";
  b.addEventListener("click", onclick);
  return b;
}

// A Copy, as the shell's (settings.js `buildSection`): its copy icon and
// word, then a check and "Copied" for a moment once copied. A clipboard
// that refuses (none, no permission, a webview's) selects what it copies
// (`shown`), to copy by hand, and says so in `said`; Copy stays Copy, to
// try again. Each press numbered: an older one's answer changes nothing.
export function copyButton(text, shown, what) {
  const b = el("button", "quiet setup-copy");
  b.type = "button";
  const said = el("p", "setup-error");
  said.setAttribute("role", "status");
  said.hidden = true;
  const shows = (copied) => {
    b.replaceChildren(icon(copied ? "check" : "copy"), copied ? "Copied" : "Copy");
    b.setAttribute("aria-label", copied ? "Copied" : `Copy ${what}`);
  };
  shows(false);
  let presses = 0;
  b.addEventListener("click", async () => {
    const mine = ++presses;
    said.hidden = true;
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      if (mine !== presses) return;
      shows(false);
      const range = document.createRange();
      range.selectNodeContents(shown);
      getSelection()?.removeAllRanges();
      getSelection()?.addRange(range);
      said.textContent = "Couldn't copy it. It's selected: copy it yourself (Ctrl+C, or ⌘C on a Mac).";
      said.hidden = false;
      return;
    }
    if (mine !== presses) return;
    shows(true);
    setTimeout(() => mine === presses && shows(false), 1500);
  });
  return { button: b, said };
}

export const row = (...items) => {
  const r = el("div", "setup-row");
  r.append(...items);
  return r;
};

// A platform route of this fragment's own (`./__…`), as its page asks it.
export async function platformJson(path, init) {
  const resp = await fetch(new URL(path, location.href), { credentials: "same-origin", ...init });
  const body = await resp.json().catch(() => ({}));
  if (!resp.ok) throw Object.assign(new Error(body.message || `${resp.status}`), { status: resp.status });
  return body;
}

export const randomHex = (n) => [...crypto.getRandomValues(new Uint8Array(n))].map((b) => b.toString(16).padStart(2, "0")).join("");

// Who opened it (`me`), the shell's light or dark taken from its frame's
// parent at the platform's origin alone (frame-messages.js), heard from
// the start: the shell says it as the frame loads, before `me` answers.
export async function signedIn() {
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
  return me;
}

// A step's drawing in `root`: its heading in a tab (`title`), its tags
// (what it is, a word each, as Settings says it: Paul, 2026-10-31:
// "Experimental", and `tag`, [its word, its title]), its line (none: left
// out) and its parts; in a tab, the way back to Settings.
export function stepper(root, { title, tag, platform }) {
  return (line, ...parts) => {
    const head = [];
    if (!framed) head.push(el("h1", null, title));
    const tags = el("div", "setup-tags");
    const what = el("span", "setup-tag", tag[0]);
    what.title = tag[1];
    tags.append(el("span", "setup-tag", "Experimental"), what);
    head.push(tags);
    root.replaceChildren(...head, ...[line].filter(Boolean), ...parts);
    if (!framed && platform) {
      const back = el("a", "quiet", "Back to Settings");
      back.href = `${platform}/settings`;
      root.append(row(back));
    }
  };
}
