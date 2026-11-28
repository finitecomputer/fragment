// The Slack connector's setup, its own page (setup.html, README.md):
// Settings frames it in a dialog of the shell's (cell/shell/connector-setup.js)
// or, where that frame can't sign in (Safari's cookie rule), it opens in a
// tab of its own. Its owner's alone, one action a step:
// 1. the app: made in Slack from a link this page fills in (`manifest`:
//    the agent's name, what the app may do, and this chat's inbox as
//    where Slack sends its events), installed there; then its bot token
//    and signing secret pasted, each set as this fragment's secret toward
//    slack.com (`__secrets`: written, never read back, never through code
//    or a model); `connect` checks the token. Again (New token), the
//    signing secret is kept unless another is pasted;
// 2. Slack's check of this chat's address, which only a delivery signed
//    by that secret passes (`state.verified`): until it comes, the page
//    says where in Slack to ask for it again;
// 3. the account: `pair`'s code sent to the bot in a Slack DM, the
//    account that sends it shown as it comes, its owner confirming it;
// 4. done: the app and the account, with New token and Unpair.
// Each step is the same three parts (its tags, one line, its controls:
// setup-page.js), a failure said in its line, so it fits the dialog.
// Settings binds it as its agent's door as it makes it; `__door` is read
// here only to say so when it is not.
import * as fragment from "./__fragment.js";
import { button, copyButton, el, platformJson, randomHex, row, signedIn, stepper } from "./setup-page.js";

const SLACK_HOST = "slack.com";
const POLL_MS = 1000;
// how long a Connect waits for its job's answer (`state.answered`)
const CONNECT_WAIT_MS = 60_000;
const JSON_TYPE = { "content-type": "application/json" };
// Slack's bounds on an app's name (35 characters) and its bot's (80, of
// a-z, 0-9, `-`, `_` and `.`)
const APP_NAME_MAX = 35;
const BOT_NAME_MAX = 80;
// A fragment's name at the start of its host's first label (docs/api.md, Names).
const NAME_AT = /^[a-z0-9](?:-?[a-z0-9])*--[a-km-np-z2-9]{4}/;

// one of this fragment's secrets, its value never read back
const secret = (name, value) => platformJson(`./__secrets/${name}`, { method: "PUT", headers: JSON_TYPE, body: JSON.stringify({ value, hosts: [SLACK_HOST] }) });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// what it is, a word each (Paul, 2026-10-31): experimental, and less
// secure than SimpleX: Slack's servers, and everyone in its rooms, read
// these
const TAG = ["Less secure", "Slack's servers, and everyone in a room it's in, can read these messages"];

/// The app Slack makes from its link (its manifest: api.slack.com's
/// "Create an app from a manifest"): named for the agent, a bot that
/// reads the messages that mention it and its DMs, and writes back, its
/// events sent to `inbox`.
export function manifest(agent, inbox) {
  const name = [...(agent.trim() || "Agent")].slice(0, APP_NAME_MAX).join("");
  const bot = name.toLowerCase().replace(/[^a-z0-9._-]+/g, "-").replace(/^-+|-+$/g, "").slice(0, BOT_NAME_MAX) || "agent";
  return {
    display_information: { name, description: `${name}, an agent on finite.chat`.slice(0, 140) },
    features: {
      bot_user: { display_name: bot, always_online: true },
      app_home: { home_tab_enabled: false, messages_tab_enabled: true, messages_tab_read_only_enabled: false },
    },
    oauth_config: { scopes: { bot: ["app_mentions:read", "channels:history", "channels:read", "chat:write", "groups:history", "groups:read", "im:history", "reactions:write", "users:read"] } },
    settings: {
      event_subscriptions: { request_url: inbox, bot_events: ["app_mention", "message.im"] },
      org_deploy_enabled: false,
      socket_mode_enabled: false,
      token_rotation_enabled: false,
    },
  };
}

/// Slack's page that makes that app, its manifest filled in.
export const createLink = (m) => `https://api.slack.com/apps?new_app=1&manifest_json=${encodeURIComponent(JSON.stringify(m))}`;

export async function setup(root) {
  const me = await signedIn();
  const platform = me.platform;
  // the agent's name, as Settings names it, and this chat's inbox
  const agent = new URLSearchParams(location.search).get("name") ?? "";
  const mine = location.hostname.match(NAME_AT)?.[0];
  const inbox = mine ? `${new URL(platform).origin}/api/f/${mine}/inbox` : null;

  let state = null;
  let error = "";
  let polling = null;
  // the token step again, from a connected app (New token)
  let replacing = false;

  const step = stepper(root, { title: "Slack", tag: TAG, platform });
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
  // drawn again once `done` says so
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

  // the secrets set, and the token checked: drawn once `known` has this
  // attempt's answer, an app or why not
  async function connect(token, signing) {
    const attempt = randomHex(16);
    if (signing) await secret("SLACK_SIGNING_SECRET", signing);
    await secret("SLACK_BOT_TOKEN", token);
    await fragment.call("connect", { attempt });
    for (const until = Date.now() + CONNECT_WAIT_MS; Date.now() < until; ) {
      await sleep(POLL_MS);
      if ((await fragment.call("state")).answered?.includes(attempt)) {
        replacing = false;
        return;
      }
    }
    throw new Error("Slack didn't answer. Try again.");
  }

  const password = (placeholder) => {
    const input = el("input");
    input.type = "password";
    input.autocomplete = "off";
    input.placeholder = placeholder;
    input.setAttribute("aria-label", placeholder);
    return input;
  };

  async function draw() {
    clearTimeout(polling);
    // its door unread is no word of it either way
    const door = platformJson("./__door").then((d) => d.bound, () => undefined);
    state = await fragment.call("state");
    const bound = await door;
    const bot = state.bot ? `@${state.bot.name}` : "";
    const where = state.bot ? `Connected to ${state.bot.team || "your workspace"} as ${bot}` : "";
    if (state.error && !error) error = state.error;
    const again = () => button("New token", "quiet", () => {
      replacing = true;
      draw();
    });
    // 1: the app, and its token and signing secret (a token Slack refused asks again)
    if (!state.bot || replacing || state.error) {
      const token = password("Bot token");
      // its signing secret is kept on a New token unless another is pasted
      const signing = password(state.bot ? "Signing secret (unchanged if empty)" : "Signing secret");
      const go = button("Connect", "", act(async () => {
        const t = token.value.trim();
        const s = signing.value.trim();
        if (!t) throw new Error("Paste the app's bot token first.");
        if (!s && !state.bot) throw new Error("Paste the app's signing secret too.");
        go.disabled = true;
        token.disabled = true;
        signing.disabled = true;
        go.textContent = "Connecting…";
        await connect(t, s);
      }));
      for (const input of [token, signing]) {
        input.addEventListener("keydown", (e) => {
          if (e.key === "Enter" && !go.disabled) go.click();
        });
      }
      const create = el("a", null, `Create ${agent.trim() || "your agent"}'s Slack app`);
      create.href = inbox ? createLink(manifest(agent, inbox)) : "https://api.slack.com/apps";
      create.target = "_blank";
      create.rel = "noopener";
      const how = el("p", null, "Install it in your workspace, then paste its bot token (OAuth & Permissions) and signing secret (Basic Information).");
      return say(el("p", null, "Make the app in Slack, from this link."), row(create), how, row(token), row(signing, go));
    }
    // 2: Slack's check of this chat's address
    if (!state.verified) {
      pollUntil(async () => (await fragment.call("state")).verified);
      return say(el("p", "setup-now", `${where}.`), el("p", null, "In your app's Event Subscriptions in Slack, press Retry by its Request URL, so Slack checks this chat's address."), row(again()));
    }
    // 3: the account, paired and confirmed here
    if (!state.paired) {
      const p = state.pairing;
      if (p?.candidate) {
        return say(
          el("p", "setup-now", `Is ${p.candidate.name} (${p.candidate.user}) your Slack account?`),
          row(button("Yes, it's me", "", act(() => fragment.call("confirm", { user: p.candidate.user }))), button("No", "quiet", act(() => fragment.call("unpair")))),
        );
      }
      if (p) {
        const said = `pair ${p.code}`;
        const code = el("code", "setup-code", said);
        const copy = copyButton(said, code, "the pairing message");
        pollUntil(async () => {
          const s = await fragment.call("state");
          return !s.pairing || !!s.pairing.candidate;
        });
        return say(el("p", "setup-now", `Send ${bot} this in a Slack DM:`), row(code, copy.button), copy.said);
      }
      return say(el("p", "setup-now", `${where}.`), el("p", null, "Pair your own Slack account, so your words there are yours."), row(button("Pair your Slack", "", act(() => fragment.call("pair"))), again()));
    }
    // done: what it is, and how to change it
    const now = el("p", "setup-now", `${where}, paired with ${state.paired.name}.`);
    const invite = el("p", null, `Add it to a channel with /invite ${bot}.`);
    const unbound = bound === null ? [el("p", null, "Not linked to an agent. Set it up from Settings.")] : [];
    say(now, invite, ...unbound, row(again(), button("Unpair", "quiet", act(() => fragment.call("unpair")))));
  }

  draw().catch((e) => {
    error = e?.message || String(e);
    say(null);
  });
}
