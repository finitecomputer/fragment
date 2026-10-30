// The Telegram connector's setup, for its owner, above the chat: the bot's
// token and the webhook's secret (set as this fragment's secrets, written
// and never read back: `__secrets`), the Telegram account paired (its
// owner confirms it here), and the agent it is a door of (`__door`). Each
// step is one action; the chat below is the conversation itself.
import * as fragment from "./__fragment.js";

const TELEGRAM_HOST = "api.telegram.org";
const POLL_MS = 2000;

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

function button(text, primary, onclick) {
  const b = el("button", primary ? "primary" : "", text);
  b.type = "button";
  b.addEventListener("click", onclick);
  return b;
}

async function platformJson(path, init) {
  const resp = await fetch(new URL(path, location.href), { credentials: "same-origin", ...init });
  const body = await resp.json().catch(() => ({}));
  if (!resp.ok) throw new Error(body.message || `${resp.status}`);
  return body;
}

// one of this fragment's secrets, its value never read back
const secret = (name, value) => platformJson(`./__secrets/${name}`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ value, hosts: [TELEGRAM_HOST] }) });
const randomHex = (n) => [...crypto.getRandomValues(new Uint8Array(n))].map((b) => b.toString(16).padStart(2, "0")).join("");

export async function setup(root) {
  const me = await fragment.me();
  if (me.role !== "owner") return;
  const panel = el("section", "telegram");
  panel.setAttribute("aria-label", "Telegram");
  (root.querySelector("#scroll") ?? root.firstElementChild)?.before(panel);
  let error = "";
  let polling = null;

  const say = (...parts) => {
    panel.replaceChildren(...parts);
    if (error) panel.append(el("p", "telegram-error", error));
  };
  const act = (fn) => async () => {
    error = "";
    try {
      await fn();
    } catch (e) {
      error = e?.message || String(e);
    }
    draw();
  };
  const pollUntil = (done) => {
    clearTimeout(polling);
    polling = setTimeout(async () => {
      if (!(await done().catch(() => false))) pollUntil(done);
      else draw();
    }, POLL_MS);
  };

  async function draw() {
    const [state, door, members] = await Promise.all([fragment.call("state"), platformJson("./__door"), platformJson("./__members")]);
    if (state.error && !error) error = state.error;
    // 1: the bot
    if (!state.bot) {
      const input = el("input");
      input.type = "password";
      input.autocomplete = "off";
      input.placeholder = "Bot token from @BotFather";
      const go = button("Connect", true, act(async () => {
        const token = input.value.trim();
        if (!token) throw new Error("Paste the bot's token first.");
        go.disabled = true;
        await secret("TELEGRAM_BOT_TOKEN", token);
        await secret("TELEGRAM_WEBHOOK_SECRET", randomHex(32));
        await fragment.call("connect", { platform: me.platform });
        pollUntil(async () => {
          const s = await fragment.call("state");
          return !!(s.bot || s.error);
        });
      }));
      const row = el("div", "telegram-row");
      row.append(input, go);
      return say(el("strong", "", "Connect Telegram"), el("p", "", "Make a bot with @BotFather, then paste its token. Telegram messages aren't end-to-end encrypted."), row);
    }
    // 2: the account, paired and confirmed here
    if (!state.paired) {
      const p = state.pairing;
      if (p?.candidate) {
        const row = el("div", "telegram-row");
        row.append(button(`Yes, ${p.candidate.name} is me`, true, act(() => fragment.call("confirm", { user: p.candidate.user }))), button("No", false, act(() => fragment.call("unpair"))));
        return say(el("strong", "", "Is this your Telegram?"), row);
      }
      if (p) {
        const open = el("a", "telegram-open", "Open in Telegram");
        open.href = p.link;
        open.target = "_blank";
        open.rel = "noopener";
        pollUntil(async () => !!(await fragment.call("state")).pairing?.candidate);
        const row = el("div", "telegram-row");
        row.append(open);
        return say(el("strong", "", `Tap Start in @${state.bot.username}`), row);
      }
      const row = el("div", "telegram-row");
      row.append(button("Pair your Telegram", true, act(() => fragment.call("pair"))));
      return say(el("strong", "", `@${state.bot.username} is connected`), row);
    }
    // 3: the agent it is a door of
    if (!door.bound) {
      const agents = (members.members ?? []).filter((m) => m.kind === "agent" && m.owner === me.principal).map((m) => m.principal);
      const people = agents.length ? await platformJson(`./__people?${agents.map((a) => `id=${encodeURIComponent(a)}`).join("&")}`) : { profiles: {} };
      const choices = agents.map((a) => people.profiles?.[a]).filter((p) => p?.fragment);
      const row = el("div", "telegram-row");
      for (const p of choices) row.append(button(`Link to ${p.title || p.name || p.fragment}`, true, act(() => platformJson("./__door", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ act: "link", agent: p.fragment }) }))));
      return say(el("strong", "", "Link your agent"), el("p", "", choices.length ? "Your Telegram messages will reach it as yours." : "Add your agent to this chat first."), row);
    }
    // done: a quiet line, and how to undo it
    const row = el("div", "telegram-row");
    row.append(el("span", "telegram-done", `@${state.bot.username} · ${state.paired.name}`), button("Unpair", false, act(() => fragment.call("unpair"))), button("Unlink", false, act(() => platformJson("./__door", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ act: "unlink" }) }))));
    say(row);
  }

  draw().catch((e) => {
    error = e?.message || String(e);
    say(el("strong", "", "Telegram"));
  });
}
