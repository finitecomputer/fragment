// The Telegram connector's setup, its own page (setup.html, README.md):
// Settings frames it in a dialog of the shell's (cell/shell/connector-setup.js)
// or, where that frame can't sign in (Safari's cookie rule), it opens in a
// tab of its own. Its owner's alone, one action a step:
// 1. the bot: its token, from its owner's Vault when Settings found one
//    there for this agent (`?vault=<name>`: a bot moved from
//    finite.computer, FIN-463 F11), lent to this fragment by Settings
//    itself (`vault-lend`: a page may not write a Vault, docs/secrets.md;
//    it stays in the Vault, and fills this fragment's `TELEGRAM_BOT_TOKEN`),
//    or pasted (from @BotFather) as this fragment's own; and, the first time, a
//    webhook secret this browser makes, each set as this fragment's secret
//    toward api.telegram.org (`__secrets`: written, never read back, never
//    through code or a model); `connect` checks the token and points the
//    bot's webhook at this chat's inbox. Again (New token: the bot stopped,
//    or another bot), the webhook's secret is kept: Telegram's deliveries
//    still come in while a token it refuses is put right;
// 2. the account: `pair`'s link opened in Telegram, the account that
//    answers it shown as it comes, its owner confirming it (`confirm`);
// 3. done: the bot and the account, with New token and Unpair.
// Each step is the same three parts (its tags, one line, its controls:
// setup-page.js, SimpleX's too), a failure said in its line, so it fits
// the dialog's one height.
// Settings binds it as its agent's door as it makes it; `__door` is read
// here only to say so when it is not.
import * as fragment from "./__fragment.js";
import { KIND } from "./frame-messages.js";
import { button, el, framed, platformJson, randomHex, row, signedIn, stepper } from "./setup-page.js";

const TELEGRAM_HOST = "api.telegram.org";
const POLL_MS = 1000;
// how long a Connect waits for its job's answer (`state.answered`)
const CONNECT_WAIT_MS = 60_000;
const JSON_TYPE = { "content-type": "application/json" };

// one of this fragment's secrets, its value never read back
const secret = (name, value) => platformJson(`./__secrets/${name}`, { method: "PUT", headers: JSON_TYPE, body: JSON.stringify({ ...value, hosts: [TELEGRAM_HOST] }) });
// the bot token in its owner's Vault that Settings found for this agent,
// which only Settings lends: offered in its frame alone (in a tab of its
// own there is no Settings to ask, so a token is pasted)
const VAULT = framed ? new URLSearchParams(location.search).get("vault") : null;
// how long Use it waits for Settings to lend the Vault's bot
const MOVE_WAIT_MS = 30_000;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// what it is, a word each (Paul, 2026-10-31): experimental, and less
// secure than SimpleX, its words end to end, as Telegram's servers read
// these
const TAG = ["Less secure", "Telegram's servers can read these messages"];

export async function setup(root) {
  const me = await signedIn();
  const platform = me.platform;

  let state = null;
  let error = "";
  let polling = null;
  // the token step again, from a connected bot (New token)
  let replacing = false;
  // the Vault's bot (named here, or taken already: `held`) is offered
  // until it connects, unless another is asked for
  let declined = false;

  // a step: its line (or what failed, in its place), and its controls
  const step = stepper(root, { title: "Telegram", tag: TAG, platform });
  const say = (line, ...parts) => step(error ? el("p", "setup-error", error) : line, ...parts);
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

  // the Vault's bot lent here by Settings, which framed this page with
  // its offer (`vault-lend`, frame-messages.js): the page asks, naming
  // nothing, and hears its answer from its parent at the platform's origin
  function lendIt() {
    const nonce = randomHex(16);
    return new Promise((resolve, reject) => {
      const done = (fn, v) => {
        clearTimeout(timer);
        removeEventListener("message", heard);
        fn(v);
      };
      const heard = (e) => {
        if (e.source !== window.parent || e.origin !== platform || e.data?.fragment !== KIND.vaultLend || e.data.nonce !== nonce) return;
        if (e.data.ok) done(resolve);
        else done(reject, Object.assign(new Error(e.data.error || "Settings couldn't use the bot."), { status: e.data.status }));
      };
      const timer = setTimeout(() => done(reject, new Error("Settings didn't answer. Try again.")), MOVE_WAIT_MS);
      addEventListener("message", heard);
      window.parent.postMessage({ fragment: KIND.vaultLend, nonce }, platform);
    });
  }

  // the token set, and checked: drawn once `known` has this attempt's
  // answer, a bot or why not (an attempt of its own: another dialog's
  // connect, answered meanwhile, is not this one's)
  async function connect(setToken) {
    const first = !state.bot;
    const attempt = randomHex(16);
    // the webhook's secret first: once the token is here, connecting is
    // all a try cut short needs
    if (first) await secret("TELEGRAM_WEBHOOK_SECRET", { value: randomHex(32) });
    await setToken();
    await fragment.call("connect", { platform: me.platform, attempt });
    for (const until = Date.now() + CONNECT_WAIT_MS; Date.now() < until; ) {
      await sleep(POLL_MS);
      if ((await fragment.call("state")).answered?.includes(attempt)) {
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
    if (!declined && !state.bot && (VAULT || state.held)) {
      const go = button("Use it", "", act(async () => {
        go.disabled = true;
        go.textContent = "Connecting…";
        // said here first, so a try cut short anywhere is offered again
        await fragment.call("hold");
        // lent again, it is lent as before (its answer lost); with no
        // offer here, the one lent already (`held`) fills it
        await connect(async () => {
          if (VAULT) await lendIt();
        });
      }));
      const other = button("Use another bot", "quiet", () => {
        declined = true;
        draw();
      });
      return say(el("p", null, "Your bot is ready to connect."), row(go, other));
    }
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
        await connect(() => secret("TELEGRAM_BOT_TOKEN", { value: token }));
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
