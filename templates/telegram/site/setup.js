// The Telegram connector's setup, its own page (setup.html, README.md):
// Settings frames it in a dialog of the shell's (cell/shell/connector-setup.js)
// or, where that frame can't sign in (Safari's cookie rule), it opens in a
// tab of its own. Its owner's alone, one action a step:
// 1. the bot: its token pasted (from @BotFather) and, the first time, a
//    webhook secret this browser makes, each set as this fragment's secret
//    toward api.telegram.org (`__secrets`: written, never read back, never
//    through code or a model); `connect` checks the token and points the
//    bot's webhook at this chat's inbox. Again (New token: the bot stopped,
//    or another bot), the webhook's secret is kept: Telegram's deliveries
//    still come in while a token it refuses is put right;
// 2. the account: `pair`'s link opened in Telegram, the account that
//    answers it shown as it comes, its owner confirming it (`confirm`);
// 3. done: the bot and the account, with New token and Unpair.
// Each step is the same three parts (its tags, one line, its controls),
// a failure said in its line, so it fits the dialog's one height.
// Settings binds it as its agent's door as it makes it; `__door` is read
// here only to say so when it is not.
import * as fragment from "./__fragment.js";
import { KIND } from "./frame-messages.js";

const TELEGRAM_HOST = "api.telegram.org";
const POLL_MS = 1000;
// how long a Connect waits for its job's answer (`state.attempt`)
const CONNECT_WAIT_MS = 60_000;
const JSON_TYPE = { "content-type": "application/json" };
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
  if (!resp.ok) throw new Error(body.message || `${resp.status}`);
  return body;
}

// one of this fragment's secrets, its value never read back
const secret = (name, value) => platformJson(`./__secrets/${name}`, { method: "PUT", headers: JSON_TYPE, body: JSON.stringify({ value, hosts: [TELEGRAM_HOST] }) });
const randomHex = (n) => [...crypto.getRandomValues(new Uint8Array(n))].map((b) => b.toString(16).padStart(2, "0")).join("");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// what it is, a word each (Paul, 2026-10-31): experimental, and less
// secure than SimpleX, its words end to end, as Telegram's servers read
// these
function tags() {
  const t = el("div", "setup-tags");
  const less = el("span", "setup-tag", "Less secure");
  less.title = "Telegram's servers can read these messages";
  t.append(el("span", "setup-tag", "Experimental"), less);
  return t;
}

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

  let state = null;
  let error = "";
  let polling = null;
  // the token step again, from a connected bot (New token)
  let replacing = false;

  // a step: its line (or what failed, in its place), and its controls
  const say = (line, ...parts) => {
    const head = [];
    if (!framed) head.push(el("h1", null, "Telegram"));
    head.push(tags());
    root.replaceChildren(...head, ...[error ? el("p", "setup-error", error) : line].filter(Boolean), ...parts);
    // in a tab, the way back
    if (!framed && platform) {
      const back = el("a", "quiet", "Back to Settings");
      back.href = `${platform}/settings`;
      root.append(row(back));
    }
  };
  const act = (fn) => async () => {
    error = "";
    try {
      await fn();
    } catch (e) {
      error = e?.message || String(e);
    }
    await draw();
  };
  // drawn again once `done` says so (a pairing answered, or let go)
  const pollUntil = (done) => {
    clearTimeout(polling);
    polling = setTimeout(async () => {
      if (!(await done().catch(() => false))) pollUntil(done);
      else await draw();
    }, POLL_MS * 2);
  };

  if (me.role !== "owner") {
    say(el("p", null, "Only its owner sets this up."));
    return;
  }

  // the token set, and checked: drawn once `known` has this attempt's
  // answer, a bot or why not (an attempt of its own: another dialog's
  // connect, answered meanwhile, is not this one's)
  async function connect(token) {
    const first = !state.bot;
    const attempt = randomHex(16);
    await secret("TELEGRAM_BOT_TOKEN", token);
    if (first) await secret("TELEGRAM_WEBHOOK_SECRET", randomHex(32));
    await fragment.call("connect", { platform: me.platform, attempt });
    for (const until = Date.now() + CONNECT_WAIT_MS; Date.now() < until; ) {
      await sleep(POLL_MS);
      if ((await fragment.call("state")).attempt === attempt) {
        replacing = false;
        return;
      }
    }
    throw new Error("Telegram didn't answer. Try again.");
  }

  async function draw() {
    clearTimeout(polling);
    // its door unread is no word of it either way
    const door = platformJson("./__door").then((d) => d.bound, () => undefined);
    state = await fragment.call("state");
    const bound = await door;
    const bot = state.bot ? `@${state.bot.username}` : "";
    if (state.error && !error) error = state.error;
    // 1: the bot, or another token for it (a token Telegram refused asks again)
    if (!state.bot || replacing || state.error) {
      const input = el("input");
      input.type = "password";
      input.autocomplete = "off";
      input.placeholder = "Bot token";
      input.setAttribute("aria-label", "Bot token");
      const go = button("Connect", "", act(async () => {
        const token = input.value.trim();
        if (!token) throw new Error("Paste the bot's token first.");
        go.disabled = true;
        input.disabled = true;
        go.textContent = "Connecting…";
        await connect(token);
      }));
      input.addEventListener("keydown", (e) => {
        if (e.key === "Enter" && !go.disabled) go.click();
      });
      // no Cancel: the dialog's Done (or the tab's way back) leaves it be
      return say(el("p", null, "Make a bot with @BotFather, then paste its token."), row(input, go));
    }
    // 2: the account, paired and confirmed here
    if (!state.paired) {
      const p = state.pairing;
      if (p?.candidate) {
        return say(
          el("p", "setup-now", `Is ${p.candidate.name} your Telegram?`),
          row(button("Yes, it's me", "", act(() => fragment.call("confirm", { user: p.candidate.user }))), button("No", "quiet", act(() => fragment.call("unpair")))),
        );
      }
      if (p) {
        const open = el("a", null, "Open in Telegram");
        open.href = p.link;
        open.target = "_blank";
        open.rel = "noopener";
        pollUntil(async () => {
          const s = await fragment.call("state");
          return !s.pairing || !!s.pairing.candidate;
        });
        return say(el("p", "setup-now", `Open ${bot} in Telegram and tap Start.`), row(open));
      }
      return say(el("p", "setup-now", `Connected as ${bot}.`), row(button("Pair your Telegram", "", act(() => fragment.call("pair"))), button("New token", "quiet", () => { replacing = true; draw(); })));
    }
    // done: what it is, and how to change it
    const now = el("p", "setup-now", `Connected as ${bot}, paired with ${state.paired.name}.`);
    const unbound = bound === null ? [el("p", null, "Not linked to an agent. Set it up from Settings.")] : [];
    say(now, ...unbound, row(button("New token", "quiet", () => { replacing = true; draw(); }), button("Unpair", "quiet", act(() => fragment.call("unpair")))));
  }

  draw().catch((e) => {
    error = e?.message || String(e);
    say(null);
  });
}
