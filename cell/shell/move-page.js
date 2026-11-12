// The move's page (`/move`, FIN-463 F6): a person's old finite.computer
// agents, each where its move is in the server's words
// (`GET /api/migrations/offers`), and the one question the import needs:
// which to bring over. Each new agent takes its old one's name.
//
// Bring over: the person opens the move (`POST /api/migrations/offers`,
// which binds them and gives its comp once: a month's seat, so a guest
// may make agents), then, one at a time, a new agent is made as Settings'
// Add makes one (onboarding.js `addAgent`, its chat not opened) and its
// Yes posted (`…/offers/{old}/yes {agent}`), which stops the old agent and
// starts the move. One new agent an old one: the agent keeps which old one
// it was made for (agent.json `from`), so a Yes that never arrived, or a
// reload, is gone on with that agent, never a second (`madeFor`). A Never'd one has Ask again. Nothing is drawn ahead of
// the server: each step shows while its call is out, and every line after
// is the server's, read again while a move is under way. A refusal is the
// server's words.
//
// Once its new agent exists, a move is that agent's to tell (FIN-463: the
// first move's agent asked its person a question in its own chat while
// this page said "Bringing it over…"): each such row opens its agent's
// chat, and an import that starts while the page shows takes the person
// there, the first if several do. One importing when the page opened is
// left to its person, who came here to see it.
//
// The shell owns the page's place and its helpers (`h`): `api`, `el`,
// `addAgent`, `setGuest`, `reload` (the list again), `ownAgents` (their
// agent fragments' names), `identityOf` (an agent fragment's identity, once
// its computer has it), `chatOf` (an agent identity's own chat, of
// theirs), `openChat`, and `timers`.

import { platformImage } from "./computer.js";

const IN_FLIGHT = new Set(["asked", "staging", "importing", "retiring", "archiving"]);
const ASKABLE = new Set(["open", "later", "archived"]);
// a move far enough along that its SimpleX chat, if it had one, waits in
// the new agent's Settings (FIN-463: its bridge sets it aside as it imports)
const CARRIED = new Set(["importing", "retiring", "moved"]);
// how often a move under way is read again, and how long a seat may take
// to reach the ledger after the comp (the registry's push)
const READ_EVERY_MS = 5_000;
const SEAT_WAIT_MS = 30_000;

export function createMovePage(root, h) {
  const { api, el, timers = globalThis } = h;
  let offers = null;
  let readFailed = null;
  let failed = null;
  // the old agent whose step is out, and what it is ("Making Juniper…")
  let doing = null;
  let busy = false;
  let shown = false;
  let timer = null;
  // the moves importing at this opening's first read (`arrive`), and which
  // opening it is: a read answered after the page closed is no one's
  let importingAtOpen = null;
  let opening = 0;
  // what the person chose, kept across drawings, and Bring over as drawn
  const chosen = new Map();
  let go = null;
  const anyChosen = () => (offers ?? []).some((o) => ASKABLE.has(o.state) && (chosen.get(o.old) ?? true));

  // whether it was this opening's
  async function read() {
    const at = opening;
    try {
      const got = (await api("GET", "/api/migrations/offers")).offers ?? [];
      if (at !== opening) return false;
      offers = got;
      readFailed = null;
      importingAtOpen ??= new Set(offers.filter((o) => o.state === "importing").map((o) => o.old));
    } catch (e) {
      if (at !== opening) return false;
      readFailed = e?.status ? e.message : "Couldn't load your agents. Try again.";
    }
    return true;
  }

  function row(o) {
    const li = el("li", "move-row");
    li.dataset.old = o.old;
    li.dataset.state = o.state;
    if (o.image) {
      const pic = el("img", "move-pic");
      pic.alt = "";
      platformImage(pic, o.image);
      li.append(pic);
    }
    const copy = el("div", "move-copy");
    copy.append(el("strong", null, o.name || "Your agent"), el("p", null, doing?.old === o.old ? doing.said : o.said));
    li.append(copy);
    if (ASKABLE.has(o.state) && !doing) {
      const pick = el("label", "move-pick");
      // the shell's check (shell.css `input.check`); a choice changes Bring
      // over alone, so the box, and the person's focus on it, stay put
      const box = el("input", "check");
      box.type = "checkbox";
      box.checked = chosen.get(o.old) ?? true;
      box.disabled = busy;
      box.onchange = () => {
        chosen.set(o.old, box.checked);
        if (go) go.disabled = busy || !anyChosen();
      };
      pick.append(box, el("span", null, "Bring over"));
      copy.append(pick);
    } else if (o.state === "never") {
      const again = el("button", null, "Ask again");
      again.type = "button";
      again.disabled = busy;
      again.onclick = () => askAgain(o);
      li.append(again);
    } else if (o.agent && h.chatOf(o.agent)) {
      const chat = el("button", null, "Open chat");
      chat.type = "button";
      chat.disabled = busy;
      chat.onclick = () => h.openChat(h.chatOf(o.agent));
      li.append(chat);
    }
    return li;
  }

  function draw() {
    if (!shown) return;
    const wrap = el("div", "move-layout");
    wrap.append(el("h1", null, "Bring your agents over"));
    if (readFailed && !offers) {
      const again = el("button", "primary", "Try again");
      again.type = "button";
      again.onclick = () => open();
      wrap.append(el("p", "form-error", readFailed), again);
      return root.replaceChildren(wrap);
    }
    if (!offers) return root.replaceChildren(wrap);
    if (!offers.length) wrap.append(el("p", "muted", "You have no agents on finite.computer."));
    const list = el("ul", "move-list");
    list.append(...offers.map(row));
    wrap.append(list);
    if (offers.some((o) => CARRIED.has(o.state))) wrap.append(el("p", "muted move-simplex", "If you chatted with it on SimpleX, keep that chat in Settings."));
    if (failed) wrap.append(el("p", "form-error", failed));
    go = null;
    if (offers.some((o) => ASKABLE.has(o.state))) {
      go = el("button", "primary", "Bring over");
      go.type = "button";
      go.disabled = busy || !anyChosen();
      go.onclick = () => bring();
      wrap.append(go);
    }
    root.replaceChildren(wrap);
  }

  // an import begun since the page opened: its agent's chat, the page left
  function arrive() {
    if (!shown || busy) return false;
    const o = offers?.find((x) => x.state === "importing" && !importingAtOpen?.has(x.old) && x.agent && h.chatOf(x.agent));
    if (!o) return false;
    h.openChat(h.chatOf(o.agent));
    return true;
  }

  // read again while a move is under way, the page shown
  function follow() {
    if (timer !== null) timers.clearTimeout(timer);
    timer = null;
    if (shown && !busy && offers?.some((o) => IN_FLIGHT.has(o.state))) {
      timer = timers.setTimeout(async () => {
        timer = null;
        if (!(await read()) || arrive()) return;
        draw();
        follow();
      }, READ_EVERY_MS);
    }
  }

  async function askAgain(o) {
    busy = true;
    failed = null;
    draw();
    try {
      await api("POST", `/api/migrations/offers/${o.old}/ask-again`, {});
    } catch (e) {
      failed = e?.status ? e.message : "Couldn't do that. Try again.";
    }
    busy = false;
    await read();
    draw();
  }

  // a seat of theirs, as their ledger says (the comp's plan reaches it a
  // moment after the registry gives it, and the ledger is what refuses a
  // guest's making), as the welcome waits (onboarding.js `seated`)
  async function seated() {
    const t0 = Date.now();
    for (;;) {
      const ledger = await api("GET", "/api/ledger");
      if (ledger.plan !== "guest") return h.setGuest(false);
      if (Date.now() - t0 > SEAT_WAIT_MS) throw new Error("Your seat isn't ready yet. Try again in a moment.");
      await new Promise((r) => timers.setTimeout(r, 1000));
    }
  }

  // The agent of theirs made for `o` before (its agent.json's `from`) that
  // no move is bound to, by its fragment's name, or null. An agent.json that
  // could not be read (not missing: unread) is no answer, and nothing may
  // be made on none: it stops, to be tried again, as Settings' agent.json
  // writes do (agent-settings.js `writeAgentJson`).
  async function madeFor(o) {
    const bound = new Set((offers ?? []).map((x) => x.agent).filter(Boolean));
    for (const fragment of h.ownAgents()) {
      const json = await api("GET", `/api/f/${fragment}/file?path=agent.json`).catch((e) => {
        if (e?.status === 404) return null;
        throw new Error(`Couldn't check whether ${o.name} was brought over already, so nothing was made. Try again.`);
      });
      if (json?.from !== o.old) continue;
      const id = h.identityOf(fragment);
      if (id && bound.has(id)) continue;
      return fragment;
    }
    return null;
  }

  async function bring() {
    // each new agent named as its old one (the import brings who it is;
    // its person renames it as any agent), so a making cut short is found
    // again by the same name whatever was typed (onboarding.js `addAgent`)
    const picks = offers.filter((o) => ASKABLE.has(o.state) && (chosen.get(o.old) ?? true)).map((o) => ({ o, name: o.name.trim() || "Your agent" }));
    if (!picks.length || busy) return;
    busy = true;
    failed = null;
    draw();
    try {
      await api("POST", "/api/migrations/offers", {});
      await seated();
      for (const { o, name } of picks) {
        // one new agent an old one: one made for it before (a Yes that
        // never arrived, a making cut short, a reload) is the one, handed
        // back to Add, which goes on with what it lacks
        const before = await madeFor(o);
        doing = { old: o.old, said: `Making ${name}…` };
        draw();
        const made = await h.addAgent({ name, role: null, quiet: true, from: o.old, agent: before });
        if (!made?.id) throw new Error(`${name} wasn't made. Try again.`);
        doing = { old: o.old, said: `Starting ${o.name}'s move…` };
        draw();
        await api("POST", `/api/migrations/offers/${o.old}/yes`, { agent: made.id });
        doing = null;
        await read();
        draw();
      }
    } catch (e) {
      failed = e?.message || "Couldn't do that. Try again.";
    }
    doing = null;
    busy = false;
    await read();
    h.reload?.();
    if (arrive()) return;
    draw();
    follow();
  }

  async function open() {
    shown = true;
    opening += 1;
    importingAtOpen = null;
    draw();
    if (!(await read())) return;
    draw();
    follow();
  }

  function close() {
    shown = false;
    opening += 1;
    if (timer !== null) timers.clearTimeout(timer);
    timer = null;
  }

  return { open, close };
}
