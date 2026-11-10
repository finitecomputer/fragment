// The move's page (`/move`, FIN-463 F6): a person's old finite.computer
// agents, each where its move is in the server's words
// (`GET /api/migrations/offers`), and the one question the import needs:
// which to bring over, each by the name its new agent takes here.
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
// The shell owns the page's place and its helpers (`h`): `api`, `el`,
// `addAgent`, `setGuest`, `reload` (the list again), `ownAgents` (their
// agent fragments' names), `identityOf` (an agent fragment's identity, once
// its computer has it), and `timers`.

import { platformImage } from "./computer.js";

const IN_FLIGHT = new Set(["asked", "staging", "importing", "retiring", "archiving"]);
const ASKABLE = new Set(["open", "later", "archived"]);
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
  // what the person chose and named, kept across drawings
  const chosen = new Map();
  const named = new Map();

  async function read() {
    try {
      offers = (await api("GET", "/api/migrations/offers")).offers ?? [];
      readFailed = null;
    } catch (e) {
      readFailed = e?.status ? e.message : "Couldn't load your agents. Try again.";
    }
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
      const box = el("input");
      box.type = "checkbox";
      box.checked = chosen.get(o.old) ?? true;
      box.disabled = busy;
      box.onchange = () => {
        chosen.set(o.old, box.checked);
        draw();
      };
      const name = el("input", "move-name");
      Object.assign(name, { type: "text", value: named.get(o.old) ?? o.name, maxLength: 64, disabled: busy, autocomplete: "off" });
      name.setAttribute("aria-label", `Name for ${o.name} here`);
      name.oninput = () => named.set(o.old, name.value);
      pick.append(box, name);
      copy.append(pick);
    } else if (o.state === "never") {
      const again = el("button", null, "Ask again");
      again.type = "button";
      again.disabled = busy;
      again.onclick = () => askAgain(o);
      li.append(again);
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
    if (failed) wrap.append(el("p", "form-error", failed));
    if (offers.some((o) => ASKABLE.has(o.state))) {
      const go = el("button", "primary", "Bring over");
      go.type = "button";
      go.disabled = busy || !offers.some((o) => ASKABLE.has(o.state) && (chosen.get(o.old) ?? true));
      go.onclick = () => bring();
      wrap.append(go);
    }
    root.replaceChildren(wrap);
  }

  // read again while a move is under way, the page shown
  function follow() {
    if (timer !== null) timers.clearTimeout(timer);
    timer = null;
    if (shown && !busy && offers?.some((o) => IN_FLIGHT.has(o.state))) {
      timer = timers.setTimeout(async () => {
        timer = null;
        await read();
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

  // a seat of theirs, as the platform says (the comp's reaches the ledger
  // a moment after it is given); a guest makes no agent
  async function seated() {
    const t0 = Date.now();
    for (;;) {
      const seat = await api("GET", "/api/seat");
      if (!seat.guest) return h.setGuest(false);
      if (Date.now() - t0 > SEAT_WAIT_MS) throw new Error("Your seat isn't ready yet. Try again in a moment.");
      await new Promise((r) => timers.setTimeout(r, 1000));
    }
  }

  // The agent of theirs made for `old` before (its agent.json's `from`)
  // that no move is bound to: `{id}` once its computer has it, else
  // `{fragment}` (its making cut short: `addAgent` makes on from it, found
  // by its name), or null.
  async function madeFor(old) {
    const bound = new Set((offers ?? []).map((o) => o.agent).filter(Boolean));
    for (const fragment of h.ownAgents()) {
      const json = await api("GET", `/api/f/${fragment}/file?path=agent.json`).catch(() => null);
      if (json?.from !== old) continue;
      const id = h.identityOf(fragment);
      if (id && bound.has(id)) continue;
      return id ? { id } : { fragment };
    }
    return null;
  }

  async function bring() {
    const picks = offers.filter((o) => ASKABLE.has(o.state) && (chosen.get(o.old) ?? true)).map((o) => ({ o, name: (named.get(o.old) ?? o.name).trim() || o.name }));
    if (!picks.length || busy) return;
    busy = true;
    failed = null;
    draw();
    try {
      await api("POST", "/api/migrations/offers", {});
      await seated();
      for (const { o, name } of picks) {
        // one new agent an old one: one made for it before (a Yes that
        // never arrived, a reload) is the one, made on if it was cut short
        const before = await madeFor(o.old);
        doing = { old: o.old, said: before?.id ? `Starting ${o.name}'s move…` : `Making ${name}…` };
        draw();
        const made = before?.id ? before : await h.addAgent({ name, role: null, quiet: true, from: o.old });
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
    draw();
    follow();
  }

  async function open() {
    shown = true;
    draw();
    await read();
    draw();
    follow();
  }

  function close() {
    shown = false;
    if (timer !== null) timers.clearTimeout(timer);
    timer = null;
  }

  return { open, close };
}
