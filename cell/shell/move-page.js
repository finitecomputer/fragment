// The move's page (`/move`, FIN-463 F6): a person's old finite.computer
// agents, each where its move is in the server's words
// (`GET /api/migrations/offers`), and the one question the import needs:
// which to upgrade. Each new agent takes its old one's name. Its words call
// the move the agent's upgrade: the same agent, in a better home (the
// server's words say so too, promising only what comes).
//
// Upgrade: the person opens the move (`POST /api/migrations/offers`,
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
// Someone who has an agent here already (one who moved by hand, Paul
// 11-19) is asked, for each old agent: a new one, or one of theirs
// (`question`), with nothing chosen for them. One of theirs is taken as it
// is: its Yes alone, nothing made, and the import adds to it (the
// import-agent-home skill keeps its self). Nothing is written to it: until
// its Yes lands nothing is decided, so a reload asks again; once it lands,
// the move names it (the offer's `agent`), and no question is left. One an
// agent was made for already (its `from`) is not asked: that agent is its
// answer, gone on with as before.
//
// Once its new agent exists, a move is that agent's to tell (FIN-463: the
// first move's agent asked its person a question in its own chat while
// this page said it was being brought over): each such row opens its agent's
// chat, and an import that starts while the page shows takes the person
// there, the first if several do. One importing when the page opened is
// left to its person, who came here to see it.
//
// The shell owns the page's place and its helpers (`h`): `api`, `el`,
// `addAgent`, `setGuest`, `reload` (the list again), `ownAgents` (their
// agent fragments' names), `identityOf` (an agent fragment's identity, once
// its computer has it), `named` (a node named after an agent, following
// its renames), `chatOf` (an agent identity's own chat, of theirs),
// `openChat`, and `timers`.

import { platformImage } from "./computer.js";

const IN_FLIGHT = new Set(["asked", "staging", "importing", "retiring", "archiving"]);
const ASKABLE = new Set(["open", "later", "archived"]);
// a move whose agent it may still import into (its Yes given, its import
// not done; a held one may be tried again): that agent is no one else's
// choice until then (the server refuses it too, `imports_into`)
const TAKEN = new Set(["asked", "staging", "importing", "held"]);
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
  // the old agent whose step is out, and what it is ("Preparing Juniper's upgrade…")
  let doing = null;
  let busy = false;
  let shown = false;
  let timer = null;
  // the moves importing at this opening's first read (`arrive`), and which
  // opening it is: a read answered after the page closed is no one's
  let importingAtOpen = null;
  let opening = 0;
  // what the person chose, kept across drawings, and Upgrade as drawn:
  // which to upgrade, and into what ("" a new one, else an agent of theirs
  // by its fragment's name), each question's choices as drawn
  const chosen = new Map();
  const into = new Map();
  const drawnInto = new Map();
  let go = null;
  // their agents as their agent.json said at this opening (`readMine`)
  let mine = null;
  const picked = () => (offers ?? []).filter((o) => ASKABLE.has(o.state) && (chosen.get(o.old) ?? true));
  // Upgrade's leave: something chosen, each with its question answered
  const ready = () => {
    const p = picked();
    return p.length > 0 && p.every((o) => !choices(o).length || answer(o) !== undefined);
  };

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

  // Their agents (FIN-463, Paul 11-19): each with its identity once a
  // computer has it (what a Yes binds), whether it runs on Hermes (the
  // import is a Hermes skill), and the old agent it was made for (`from`);
  // null for one whose agent.json could not be read (not missing: unread).
  function readMine() {
    return Promise.all(h.ownAgents().map(async (fragment) => {
      const json = await api("GET", `/api/f/${fragment}/file?path=agent.json`).then((j) => j ?? {}, (e) => (e?.status === 404 ? {} : null));
      if (!json) return null;
      return { fragment, identity: h.identityOf(fragment), hermes: (typeof json.runtime === "string" ? json.runtime : "hermes") === "hermes", from: typeof json.from === "string" ? json.from : null };
    }));
  }

  // what the page asks from, while an old agent is still to answer for:
  // those read (Upgrade reads them again, whole)
  async function mineAgain() {
    const at = opening;
    mine = null;
    if (!offers?.some((o) => ASKABLE.has(o.state))) return;
    const got = await readMine();
    if (at === opening) mine = got.filter(Boolean);
  }

  // What `o` may be upgraded into besides a new one: none once an agent was
  // made for it (its answer, gone on with as before); else each of theirs
  // on a computer and on Hermes that is free: not made for another old
  // agent still to answer, and not one a move may still import into.
  function choices(o) {
    if (!mine) return [];
    if (mine.some((a) => a.from === o.old)) return [];
    const open = new Set((offers ?? []).filter((x) => ASKABLE.has(x.state)).map((x) => x.old));
    const taken = new Set((offers ?? []).filter((x) => TAKEN.has(x.state) && x.agent).map((x) => x.agent));
    return mine.filter((a) => a.identity && a.hermes && !taken.has(a.identity) && !(a.from && open.has(a.from)));
  }

  // what the person answered for `o` while it is one of its choices: ""
  // (a new one), an agent's fragment, or undefined (not yet)
  function answer(o) {
    const a = into.get(o.old);
    return a === "" || choices(o).some((c) => c.fragment === a) ? a : undefined;
  }

  // The one question for `o`, when they have agents it may go into: a new
  // one, or one of theirs, nothing chosen for them. Shown while `o` is to
  // be upgraded. A choice changes Upgrade alone (the page, and the
  // person's focus, stay put), and takes that agent from another old
  // agent that had it: one agent takes in one upgrade at a time.
  function question(o, theirs) {
    const q = el("div", "move-into");
    q.setAttribute("role", "radiogroup");
    const ask = el("p");
    ask.id = `move-into-${o.old}`;
    if (theirs.length === 1) h.named(ask, theirs[0].fragment, (t) => `You already have an agent here. Make a new one for this upgrade, or use ${t}?`);
    else ask.textContent = "You already have agents here. Make a new one for this upgrade, or use one of yours?";
    q.setAttribute("aria-labelledby", ask.id);
    q.append(ask);
    const radios = [];
    for (const a of [null, ...theirs]) {
      const value = a?.fragment ?? "";
      const line = el("label", "move-pick");
      const r = el("input", "check");
      r.type = "radio";
      r.name = `move-into-${o.old}`;
      r.value = value;
      r.checked = answer(o) === value;
      r.disabled = busy;
      r.onchange = () => {
        into.set(o.old, value);
        for (const [other, rs] of drawnInto) {
          if (other === o.old || !value || into.get(other) !== value) continue;
          into.delete(other);
          for (const x of rs) x.checked = false;
        }
        if (go) go.disabled = busy || !ready();
      };
      const said = el("span");
      if (a) h.named(said, a.fragment, (t) => `Use ${t}`);
      else said.textContent = "Make a new one";
      line.append(r, said);
      q.append(line);
      radios.push(r);
    }
    drawnInto.set(o.old, radios);
    q.hidden = !(chosen.get(o.old) ?? true);
    return q;
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
      // the shell's check (shell.css `input.check`); a choice changes
      // Upgrade alone, so the box, and the person's focus on it, stay put
      const box = el("input", "check");
      box.type = "checkbox";
      box.checked = chosen.get(o.old) ?? true;
      box.disabled = busy;
      const theirs = choices(o);
      const q = theirs.length ? question(o, theirs) : null;
      box.onchange = () => {
        chosen.set(o.old, box.checked);
        if (q) q.hidden = !box.checked;
        if (go) go.disabled = busy || !ready();
      };
      pick.append(box, el("span", null, "Upgrade"));
      copy.append(pick);
      if (q) copy.append(q);
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
    wrap.append(el("h1", null, "Upgrade your agents"));
    if (readFailed && !offers) {
      const again = el("button", "primary", "Try again");
      again.type = "button";
      again.onclick = () => open();
      wrap.append(el("p", "form-error", readFailed), again);
      return root.replaceChildren(wrap);
    }
    if (!offers) return root.replaceChildren(wrap);
    if (!offers.length) wrap.append(el("p", "muted", "You have no agents to upgrade."));
    const list = el("ul", "move-list");
    drawnInto.clear();
    list.append(...offers.map(row));
    wrap.append(list);
    if (offers.some((o) => CARRIED.has(o.state))) wrap.append(el("p", "muted move-simplex", "If you chatted with it on SimpleX, keep that chat in Settings."));
    if (failed) wrap.append(el("p", "form-error", failed));
    go = null;
    if (offers.some((o) => ASKABLE.has(o.state))) {
      go = el("button", "primary", "Upgrade");
      go.type = "button";
      go.disabled = busy || !ready();
      go.onclick = () => bring();
      wrap.append(go);
    }
    root.replaceChildren(wrap);
  }

  // an import begun since the page opened: its agent's chat, the page left.
  // None before this opening's first answered read: what the page holds
  // until then is an earlier opening's, and no news
  function arrive() {
    if (!shown || busy || !importingAtOpen) return false;
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
  // no move is bound to, by its fragment's name, or null, as Upgrade read
  // them, whole
  function madeFor(o) {
    const bound = new Set((offers ?? []).map((x) => x.agent).filter(Boolean));
    return mine.find((a) => a.from === o.old && !(a.identity && bound.has(a.identity)))?.fragment ?? null;
  }

  async function bring() {
    if (!ready() || busy) return;
    busy = true;
    failed = null;
    draw();
    try {
      // their agents as they are now, each read: one that could not be is
      // no answer (it may be one to ask about, or one made for it already),
      // and nothing may be made on none: it stops, to be tried again, as
      // Settings' agent.json writes do (agent-settings.js `writeAgentJson`).
      // A question new since the page drew is the person's first.
      const now = await readMine();
      if (now.some((a) => !a)) throw new Error("Couldn't check your agents, so nothing changed. Try again.");
      mine = now;
      if (ready()) await upgrade();
    } catch (e) {
      failed = e?.message || "Couldn't do that. Try again.";
    }
    doing = null;
    busy = false;
    await read();
    h.reload?.();
    if (arrive()) return;
    await mineAgain();
    draw();
    follow();
  }

  // each chosen into its answer: an agent of theirs, as it is, or a new one,
  // named as its old one (the import brings who it is; its person renames
  // it as any agent), so a making cut short is found again by the same name
  // whatever was typed (onboarding.js `addAgent`)
  async function upgrade() {
    const picks = picked().map((o) => ({ o, name: o.name.trim() || "Your agent", theirs: mine.find((a) => a.fragment === answer(o)) ?? null }));
    // one of theirs chosen that is no longer free (another move took it):
    // asked again, never a new one in its place
    const lost = picks.find(({ o, theirs }) => into.get(o.old) && !theirs);
    if (lost) {
      into.delete(lost.o.old);
      throw new Error(`Choose again for ${lost.o.name}.`);
    }
    await api("POST", "/api/migrations/offers", {});
    await seated();
    for (const { o, name, theirs } of picks) {
      if (theirs) {
        // one of theirs: its Yes alone, nothing made or written
        doing = { old: o.old, said: `Starting ${o.name}'s upgrade…` };
        draw();
        await api("POST", `/api/migrations/offers/${o.old}/yes`, { agent: theirs.identity });
        doing = null;
        await read();
        draw();
        continue;
      }
      // one new agent an old one: one made for it before (a Yes that
      // never arrived, a making cut short, a reload) is the one, handed
      // back to Add, which goes on with what it lacks
      const before = madeFor(o);
      doing = { old: o.old, said: `Preparing ${name}'s upgrade…` };
      draw();
      const made = await h.addAgent({ name, role: null, quiet: true, from: o.old, agent: before });
      if (!made?.id) throw new Error(`${name}'s upgrade didn't start. Try again.`);
      doing = { old: o.old, said: `Starting ${o.name}'s upgrade…` };
      draw();
      await api("POST", `/api/migrations/offers/${o.old}/yes`, { agent: made.id });
      doing = null;
      await read();
      draw();
    }
  }

  async function open() {
    shown = true;
    opening += 1;
    importingAtOpen = null;
    draw();
    if (!(await read())) return;
    await mineAgain();
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
