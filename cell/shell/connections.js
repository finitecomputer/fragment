// a provider's name for people: google-places is Google Places, xai xAI
import { KIND } from "./frame-messages.js";

const PROVIDER_NAMES = { xai: "xAI", elevenlabs: "ElevenLabs", openrouter: "OpenRouter" };
export const providerName = (p) => PROVIDER_NAMES[p] ?? p.replace(/(^|-)([a-z])/g, (_, d, l) => `${d ? " " : ""}${l.toUpperCase()}`);

// Provider/account UI keeps failed removals for the current settings visit.
export function createConnections({ api, el, section, named, seg, computer, getAgents, computerIs, visitOf, isOpen, reopenSettings }) {
  // ---- connections (decisions 22, 37 and 44): every provider the platform
  // offers, one row each: what it is, its state, which of the person's agents
  // may use it (all by default; a list narrows one agent), and this month's
  // calls by each agent, with an operator key's cost. Their agents use them
  // through the computer's swap, each with a placeholder of its own.
  const PROVIDER_KIND = { connection: "Your account", operator: "The platform's key", own: "Your key" };
  const PROVIDER_STATE = {
    connected: "Connected",
    needs_reauthorization: "Needs reauthorization",
    not_connected: "Not connected",
    offered: "Offered",
    set: "Key set",
    not_set: "No key yet",
  };
  // a connection's scope for people, by its last path segment (Google's
  // https://www.googleapis.com/auth/<name>), else that segment itself; the
  // list never names a sign-in scope as missing
  const SCOPE_NAMES = {
    "gmail.readonly": "Gmail (read)",
    "gmail.modify": "Gmail (manage)",
    "gmail.send": "Gmail (send)",
    drive: "Drive",
    spreadsheets: "Sheets",
    documents: "Docs",
    calendar: "Calendar",
    "contacts.readonly": "Contacts",
  };
  const scopeName = (scope) => {
    const last = scope.split("/").pop();
    return SCOPE_NAMES[last] ?? last;
  };
  // micro-dollars, to a hundredth of a cent under a dollar (a call is often less than a cent)
  const money = (micros) => (micros / 1_000_000).toLocaleString(undefined, { style: "currency", currency: "USD", minimumFractionDigits: 2, maximumFractionDigits: micros < 1_000_000 ? 4 : 2 });
  // Connect: a window opened by the press itself, before any await, and only
  // then sent to the provider's page (#442): one opened once the authorize
  // call answers is a popup Safari blocks, as Chrome does once the press is
  // 5 s old. One attempt at a time: each start replaces the browser's one
  // binding cookie. The section reads again when the person comes back to it
  // (`focus`), never by watching `w.closed`: the shell's COOP has that read
  // true once the window is at the provider, still open. `body`, where the
  // person connects several accounts: `{add: true}` for another, `{account}`
  // for one of theirs again.
  function connectButton(p, label, said, body = {}) {
    const go = el("button", "quiet", label);
    go.type = "button";
    go.onclick = async () => {
      const w = window.open("", "_blank", "popup,width=520,height=720");
      if (!w) {
        said.textContent = "Your browser blocked the window. Allow pop-ups for this site, then try again.";
        return;
      }
      go.disabled = true;
      said.textContent = "";
      try {
        const { url } = await api("POST", `/api/connections/${encodeURIComponent(p.provider)}/authorize`, body);
        w.location = url;
        addEventListener("focus", () => reopenSettings().catch(() => {}), { once: true });
      } catch (e) {
        w.close();
        said.textContent = e.message;
      } finally {
        go.disabled = false;
      }
    };
    return go;
  }
  // The own keys whose last Disconnect or Remove did not go through, each
  // with the visit of settings it was pressed in: said in their row by
  // whichever load of settings renders in that visit, while the row still
  // has the key, until the next press; in another visit, never (#439).
  const unremoved = new Map();
  // Disconnect: the account goes from Pipes, so no agent's call gets its token
  // again, and its grant is revoked at the provider where the platform can;
  // when it could not, settings says where the person removes it themselves.
  // Of several accounts, the one `account` is.
  function disconnectButton(p, said, account = null) {
    const off = el("button", "quiet", "Disconnect");
    off.type = "button";
    off.dataset.action = "disconnect";
    const which = account ? `?account=${encodeURIComponent(account.id)}` : "";
    off.onclick = async () => {
      off.disabled = true;
      said.textContent = "";
      let revoked;
      try {
        ({ revoked } = await api("DELETE", `/api/connections/${encodeURIComponent(p.provider)}${which}`));
      } catch (e) {
        said.textContent = e.message;
        off.disabled = false;
        return;
      }
      const name = providerName(p.provider);
      await reopenSettings(revoked ? null : [`${name} is disconnected`, `Your agents can no longer use it. ${name} may still list this platform's access to your account: remove it in your ${name} account's security settings.`]).catch(() => {});
    };
    return off;
  }
  // What the person left unticked when they connected an account, in words.
  const notAllowed = (missing) => el("span", "provider-detail provider-missing", `Not allowed: ${[...new Set(missing.map(scopeName))].join(", ")}`);
  // An account of several, for people: its label, or the provider's name.
  const accountLabel = (p, a) => a.account ?? `A ${providerName(p.provider)} account`;
  // A connection that takes several accounts: each of the person's, oldest
  // first, with its label and state, what it was not allowed with a
  // Reconnect to add (that account), and its own Disconnect. One whose
  // disconnect did not finish (Pipes holds it still) says so, with
  // Disconnect again, and nothing else.
  function accountsList(p) {
    const list = el("div", "provider-accounts");
    for (const a of p.accounts) {
      const item = el("div", "provider-account-row");
      item.dataset.accountId = a.id;
      item.dataset.state = a.disconnecting ? "disconnecting" : a.state;
      const head = el("div", "provider-head");
      const who = el("div", "provider-identity");
      const stateName = a.disconnecting ? "Disconnecting didn't finish" : PROVIDER_STATE[a.state] ?? a.state;
      who.append(el("span", "provider-account", accountLabel(p, a)), el("span", `provider-state ${a.disconnecting ? "disconnecting" : a.state}`, stateName));
      const actions = el("div", "settings-actions");
      const said = el("span", "muted provider-detail provider-said");
      const details = [];
      if (a.disconnecting) {
        const again = disconnectButton(p, said, a);
        again.textContent = "Disconnect again";
        actions.append(again);
        head.append(who, actions);
        item.append(head, said);
        list.append(item);
        continue;
      }
      if (a.state !== "connected") actions.append(connectButton(p, "Connect again", said, { account: a.id }));
      if (a.state === "connected" && a.missing?.length) {
        const again = connectButton(p, "Reconnect to add", said, { account: a.id });
        again.dataset.action = "reconnect";
        actions.append(again);
        details.push(notAllowed(a.missing));
      }
      actions.append(disconnectButton(p, said, a));
      head.append(who, actions);
      item.append(head, ...details, said);
      list.append(item);
    }
    return list;
  }
  // Which of the person's accounts an agent uses there: Default (the oldest
  // connected) first, then each of theirs, its state said where it is not
  // connected, and the agent's saved pick as it is stored, gone or not, so
  // a pick the platform refuses (one to reconnect) is never shown as another.
  // Shown with two or more accounts, or a saved pick to change.
  function accountPick(p, a, theirs) {
    const pick = el("select", "provider-account-pick");
    pick.dataset.agent = a.fragment;
    named(pick, a.fragment, (t) => `${t}'s ${providerName(p.provider)} account`, "aria-label");
    const option = (value, text) => {
      const o = el("option", "", text);
      o.value = value;
      pick.append(o);
    };
    const first = theirs.find((c) => c.state === "connected");
    option("", first ? `Default (${accountLabel(p, first)})` : "Default");
    for (const c of theirs) option(c.id, c.state === "connected" ? accountLabel(p, c) : `${accountLabel(p, c)} (needs reconnecting)`);
    const mine = a.accounts?.[p.provider];
    if (mine && !theirs.some((c) => c.id === mine)) option(mine, "An account no longer connected (the default is used)");
    pick.value = mine ?? "";
    pick.onchange = async () => {
      const visit = visitOf();
      pick.disabled = true;
      // Default is no pick: the agent follows it
      const account = pick.value || null;
      let refused = null;
      try {
        computerIs(await api("PUT", `/api/computers/${seg(computer().computer)}/agents/${seg(a.fragment)}/accounts/${encodeURIComponent(p.provider)}`, { account }));
      } catch (e) {
        refused = e;
      }
      await reopenSettings(refused && visit === visitOf() ? ["That was not changed", refused.message] : null);
    };
    return pick;
  }
  function connectionsSection(linked, uses) {
    const s = section("Connections");
    s.id = "settings-connections";
    const offered = linked?.providers ?? [];
    if (!offered.length) {
      s.append(el("p", "muted", linked ? "This platform offers none yet." : "Your connections could not be read."));
      return s;
    }
    s.append(el("p", "muted", "Your agents use these through their computer, which adds each credential on the way out. None is ever on the computer."));
    const agents = computer()?.agents ?? [];
    for (const p of offered) {
      const row = el("div", "provider");
      row.dataset.provider = p.provider;
      row.dataset.kind = p.kind;
      row.dataset.state = p.state;
      const head = el("div", "provider-head");
      const identity = el("div", "provider-identity");
      // an own key signed in to is the person's account there, its models theirs to pick
      const kind = p.signIn ? (p.models?.length ? "Your account: its models for your agents" : "Your account") : PROVIDER_KIND[p.kind] ?? p.kind;
      identity.append(el("span", "provider-name", providerName(p.provider)), el("span", "provider-kind", kind));
      const stateName = p.signIn ? (p.state === "set" ? "Connected" : "Not connected") : PROVIDER_STATE[p.state] ?? p.state;
      const st = el("span", `provider-state ${p.state}`, stateName);
      identity.append(st);
      // which account a connection is: the person's sign-in here may be another
      // (of several, each is named in its own row)
      const several = Array.isArray(p.accounts);
      if (p.account && !several) {
        const account = el("span", "provider-account", p.account);
        account.dataset.account = p.account;
        identity.append(account);
      }
      head.append(identity);
      row.append(head);
      // the deployment's word on it (its catalog row's note), connected or not
      if (p.note) row.append(el("span", "muted provider-detail provider-note", p.note));
      const actions = el("div", "settings-actions");
      const details = [];
      // what a Connect press says: a blocked window, or a start refused; and
      // a Disconnect or Remove that did not go through
      const said = el("span", "muted provider-detail provider-said");
      // Disconnect or Remove: settings then reload, and the row they show says
      // whether the key is still there; one that failed says so in it
      const removeKey = (button) => async () => {
        const visit = visitOf();
        button.disabled = true;
        unremoved.delete(p.provider);
        try {
          await api("DELETE", `/api/connections/${encodeURIComponent(p.provider)}/key`);
        } catch {
          // kept only while its visit is the one shown: an older visit's
          // answer never replaces a newer one's
          if (visit === visitOf() && isOpen()) unremoved.set(p.provider, visit);
        }
        await reopenSettings();
      };
      if (p.kind === "connection" && p.state !== "connected" && !(several && p.accounts.some((a) => !a.disconnecting))) {
        actions.append(connectButton(p, p.state === "needs_reauthorization" ? "Connect again" : "Connect", said));
        details.push(said);
      }
      // of several, another while the platform takes more; each account is
      // reconnected and disconnected in its own row
      // theirs to use: none they disconnected (which Pipes may hold still)
      const theirs = several ? p.accounts.filter((a) => !a.disconnecting) : [];
      if (several && theirs.length && theirs.length < (p.accountsMax ?? 1)) {
        const add = connectButton(p, `Add another ${providerName(p.provider)} account`, said, { add: true });
        add.dataset.action = "add-account";
        actions.append(add);
        details.push(said);
      }
      // what the person left unticked when they connected it: a reconnect
      // asks for it again (the provider keeps what they allowed before)
      if (!several && p.kind === "connection" && p.state === "connected" && p.missing?.length) {
        const again = connectButton(p, "Reconnect to add", said);
        again.dataset.action = "reconnect";
        actions.append(again);
        details.push(notAllowed(p.missing), said);
      }
      if (!several && p.kind === "connection" && p.state !== "not_connected") {
        actions.append(disconnectButton(p, said));
        if (!details.includes(said)) details.push(said);
      }
      if (p.kind === "own" && p.signIn) {
        // connected through the provider's own page (never a key pasted here):
        // it makes a key of the person's, which their computer keeps sealed
        if (p.state !== "set") {
          const go = connectButton(p, "Connect", said);
          go.dataset.action = "sign-in";
          actions.append(go);
          details.push(said);
          if (p.models?.length) details.push(el("span", "muted provider-detail", `Your ${providerName(p.provider)} account pays for its models; then pick one for an agent under Agents.`));
        } else {
          const off = el("button", "quiet", "Disconnect");
          off.type = "button";
          off.dataset.action = "disconnect";
          off.onclick = removeKey(off);
          const manage = el("a", "quiet", `Revoke the key at ${providerName(p.provider)}`);
          manage.href = p.signIn.manage;
          manage.target = "_blank";
          manage.rel = "noopener";
          actions.append(off, manage);
        }
      } else if (p.kind === "own") {
        const form = el("form", "provider-key");
        const input = el("input");
        input.type = "password";
        input.autocomplete = "off";
        input.placeholder = p.state === "set" ? "Replace your key" : `Your ${providerName(p.provider)} key`;
        const save = el("button", "quiet", "Save");
        save.type = "submit";
        form.append(input, save);
        form.onsubmit = async (e) => {
          e.preventDefault();
          save.disabled = true;
          try {
            await api("PUT", `/api/connections/${encodeURIComponent(p.provider)}/key`, { key: input.value });
            await reopenSettings();
          } catch (err) {
            save.textContent = err.message;
            save.disabled = false;
          }
        };
        actions.append(form);
        if (p.state === "set") {
          const remove = el("button", "quiet", "Remove");
          remove.type = "button";
          remove.dataset.action = "remove-key";
          remove.onclick = removeKey(remove);
          actions.append(remove);
        }
      }
      if (p.state === "set" && unremoved.get(p.provider) === visitOf()) {
        said.textContent = "That didn't go through. Try again.";
        details.push(said);
      } else unremoved.delete(p.provider);
      if (p.kind === "operator" && p.price) {
        details.push(el("span", "muted provider-detail", `${money(p.price.micros / p.price.per)} a call at list, metered to your credit`));
      }
      if (actions.childElementCount) head.append(actions);
      row.append(...details);
      if (several && p.accounts.length) row.append(accountsList(p));
      // which agents may use it: all by default; one narrowed to a list uses
      // those; of two or more accounts, or a pick saved, which each uses
      if (agents.length) {
        const who = el("div", "provider-agents");
        who.append(el("span", "settings-key", "Agents"));
        for (const a of agents) {
          const allowed = a.connections == null || a.connections.includes(p.provider);
          const chip = named(el("button", "chip"), a.fragment);
          chip.type = "button";
          chip.dataset.agent = a.fragment;
          chip.setAttribute("aria-pressed", String(allowed));
          named(chip, a.fragment, (t) => (allowed ? `${t} may use it: press to take it away` : `${t} may not use it: press to let it`), "title");
          chip.onclick = async () => {
            const visit = visitOf();
            chip.disabled = true;
            const all = offered.map((x) => x.provider);
            const now = a.connections ?? all;
            const next = allowed ? now.filter((x) => x !== p.provider) : [...new Set([...now, p.provider])];
            const list = all.every((x) => next.includes(x)) ? null : next;
            let refused = null;
            try {
              computerIs(await api("PUT", `/api/computers/${seg(computer().computer)}/agents/${seg(a.fragment)}/connections`, { connections: list }));
            } catch (e) {
              refused = e;
            }
            // a failure is said by the reload, in the visit it was pressed in,
            // while the agent may use it or not as before (a newer press may
            // have changed it since)
            const was = getAgents().get(a.identity) ?? a;
            const unchanged = (was.connections == null || was.connections.includes(p.provider)) === allowed;
            await reopenSettings(refused && unchanged && visit === visitOf() ? ["That was not changed", refused.message] : null);
          };
          who.append(chip);
          if (allowed && several && (theirs.length >= 2 || a.accounts?.[p.provider])) who.append(accountPick(p, a, theirs));
        }
        row.append(who);
      }
      // this month's calls through the swap, by agent, and an operator key's cost
      const mine = (uses?.uses ?? []).filter((u) => u.provider === p.provider);
      const used = el("div", "provider-uses");
      used.append(el("span", "settings-key", uses?.month ? `In ${uses.month}` : "This month"));
      if (!mine.length) used.append(el("span", "muted", "No calls yet"));
      for (const u of mine) {
        const calls = `${u.calls} ${u.calls === 1 ? "call" : "calls"}`;
        const item = named(el("span", "use"), u.agent, (t) => `${t}: ${p.kind === "operator" ? `${calls}, ${money(u.micros)}` : calls}`);
        item.dataset.use = u.agent;
        item.dataset.calls = String(u.calls);
        item.dataset.micros = String(u.micros);
        used.append(item);
      }
      row.append(used);
      s.append(row);
    }
    return s;
  }
  return { connectionsSection };
}

// Home reads and writes share the agent.json writer with model settings.
export function createAgentHomes({ el, icon, named, actionLine, settingLabel, modelOption, savePick, agents, chats, own, labelOf, byName, titleOf, agentsOwnChat, agentJson, writeAgentJson, renderChats }) {
  // An agent's SimpleX chat: the person's chat labelled `<label>-simplex`,
  // which the agent's computer makes for them as its SimpleX is linked, and
  // which mirrors their SimpleX app both ways (docs/bridge.md, "SimpleX").
  // Titled "SimpleX" (`SIMPLEX_TITLE`), it is shown as "<agent> · SimpleX":
  // its title is its own, never the agent's name, which a rename changes.
  const SIMPLEX_TITLE = "SimpleX";
  // (with the agent in it: a chat of its label an earlier agent left is none)
  const simplexChatOf = (agent) => {
    const id = [...agents().values()].find((a) => a.fragment === agent)?.identity;
    return chats().find((f) => own(f) && labelOf(f.name) === `${labelOf(agent)}-simplex` && !!id && (f.agents ?? []).includes(id)) ?? null;
  };
  const simplexAgentOf = (name) => [...agents().values()].find((a) => simplexChatOf(a.fragment)?.name === name)?.fragment ?? null;
  function chatTitle(name) {
    const agent = byName(name)?.title === SIMPLEX_TITLE ? simplexAgentOf(name) : null;
    return agent ? `${titleOf(agent)} · ${SIMPLEX_TITLE}` : titleOf(name);
  }
  // ---- an agent's home: where it reaches its person (Paul, 2026-10-26):
  // its own chat, or its SimpleX chat, as its agent.json's `home` says
  // (`"simplex"`; absent, its chat). Its routines without a chat run there,
  // and its computer tells it so (docs/api.md, "An agent's home").
  // One value per agent, `homes`, which the sidebar's bell and
  // Settings' pick both render from (`homeIs`): read from agent.json while
  // unknown (`readHome`), and set by each write that lands (`setHome`: a pick,
  // a link's box, an unlink), each through the agent's one writer. A read is
  // let go when a write began or ended after it was asked (`homeWrites`), so
  // it never undoes one. One that fails leaves it unknown and says so in
  // Settings, with Try again (`homeUnread`); Settings shown read it again. ----
  const homes = new Map();
  const homeWrites = new Map();
  const homeReads = new Set();
  const homeUnread = new Set();
  const wrote = (agent) => homeWrites.set(agent, (homeWrites.get(agent) ?? 0) + 1);
  function readHome(agent) {
    if (homeReads.has(agent)) return;
    homeReads.add(agent);
    if (homeUnread.delete(agent)) paintHomes();
    const asked = homeWrites.get(agent) ?? 0;
    agentJson(agent)
      .catch((e) => (e.status === 404 ? {} : null))
      .then((j) => {
        homeReads.delete(agent);
        if ((homeWrites.get(agent) ?? 0) !== asked) return;
        if (j) return homeIs(agent, j.home);
        homeUnread.add(agent);
        paintHomes();
      });
  }
  function loadHomes() {
    for (const a of agents().values()) {
      if (!homes.has(a.fragment) && own(byName(a.fragment) ?? {})) readHome(a.fragment);
    }
    paintHomes();
  }
  function homeIs(agent, home) {
    homes.set(agent, home === "simplex" ? "simplex" : "chat");
    renderChats();
    paintHomes();
  }
  // the chat an agent's home names: its SimpleX chat when picked and there,
  // else its own chat
  const homeChatOf = (agent) => (homes.get(agent) === "simplex" && simplexChatOf(agent)) || agentsOwnChat(agent);
  function homeMark(f) {
    const agent = [...agents().values()].find((a) => homes.has(a.fragment) && homeChatOf(a.fragment)?.name === f.name)?.fragment;
    if (!agent) return [];
    const m = el("span", "home-mark");
    m.replaceChildren(icon("bell"));
    m.title = `${titleOf(agent)} reaches you here`;
    m.setAttribute("role", "img");
    m.setAttribute("aria-label", m.title);
    return [m];
  }
  // Its home set on agent.json as it is now, through its writer whatever is
  // shown (`writeAgentJson`: nothing written when it is so already), then
  // shown; a write that fails has it read again.
  async function setHome(agent, simplex) {
    wrote(agent);
    try {
      await writeAgentJson(agent, (next) => {
        if ((next.home === "simplex") === simplex) return null;
        if (simplex) next.home = "simplex";
        else delete next.home;
        return `its home: ${simplex ? "its SimpleX chat" : "its chat"}`;
      });
    } catch (e) {
      wrote(agent);
      homeReads.delete(agent);
      readHome(agent);
      throw e;
    }
    wrote(agent);
    homeIs(agent, simplex ? "simplex" : null);
  }

  // ---- where an agent reaches its person (its home, above): its own chat,
  // or its SimpleX chat once it has one. Always shown, so where its
  // reminders go is never a guess: read again while unknown, and a read that
  // failed says so under it, with Try again. ----
  const homeUnreadLines = new WeakMap();
  function homePicker(a, said) {
    const pick = el("select", "agent-model");
    pick.dataset.homeFor = a.fragment;
    named(pick, a.fragment, (t) => `Where ${t} reaches you`, "aria-label");
    pick.disabled = true;
    pick.onchange = async () => {
      const value = pick.value;
      pick.dataset.saving = "1";
      await savePick(pick, value, said, setHome(a.fragment, value === "simplex"), "Saved.");
      delete pick.dataset.saving;
      paintHome(pick);
    };
    const again = el("button", "quiet", "Try again");
    again.type = "button";
    again.onclick = () => readHome(a.fragment);
    const unread = actionLine(el("span", "muted", "Couldn't read where it reaches you."), again);
    unread.dataset.homeUnread = a.fragment;
    homeUnreadLines.set(pick, unread);
    paintHome(pick);
    if (!homes.has(a.fragment)) readHome(a.fragment);
    return [settingLabel("Reaches you in", pick), unread];
  }
  // A home pick shows its agent's home as the shell knows it (`homes`),
  // with its own chat and, once it has one, its SimpleX chat to pick; while
  // its home is unknown it waits (or says its read failed), and while its own
  // pick saves it is left be.
  function paintHome(pick) {
    const agent = pick.dataset.homeFor;
    const home = homes.get(agent);
    const unread = homeUnreadLines.get(pick);
    if (unread) unread.hidden = Boolean(home) || !homeUnread.has(agent);
    if (!home || pick.dataset.saving) return;
    const chat = agentsOwnChat(agent);
    const options = [modelOption("", chat ? chatTitle(chat.name) : "Its chat")];
    if (home === "simplex" || simplexChatOf(agent)) options.push(modelOption("simplex", SIMPLEX_TITLE));
    pick.replaceChildren(...options);
    pick.value = pick.dataset.was = home === "simplex" ? "simplex" : "";
    pick.disabled = false;
    pick.dataset.read = "1";
  }
  const paintHomes = () => document.querySelectorAll("select[data-home-for]").forEach(paintHome);
  return { chatTitle, homeMark, loadHomes, homePicker, paintHomes, setHome };
}

// The computer port UI owns its frame-message listener and origin checks.
export function createSimplex({ api, el, section, actionLine, named, seg, computer, getSeat, titleOf, frameOf, show, paneIcon, notice, setHome }) {
  // ---- an agent's SimpleX: its link page, a port of its computer's own
  // origin as its screen is, through a ticket (docs/bridge.md, "SimpleX"):
  // the one-time QR code, and whom it is linked to ----
  async function openSimplex(agent) {
    if (!computer()?.simplex || !agent) return;
    try {
      const t = await api("POST", `/api/computers/${seg(computer().computer)}/ports/6080/ticket`, { path: `/simplex/?agent=${encodeURIComponent(agent)}` });
      const frame = named(frameOf(t.url, `${titleOf(agent)} on SimpleX`), agent, (n) => `${n} on SimpleX`, "aria-label");
      frame.dataset.simplex = agent;
      frame.addEventListener("load", () => nameSimplex(frame));
      show({ key: `simplex:${agent}`, title: titleOf(agent), subtitle: "SimpleX", icon: paneIcon("link"), body: frame, persist: false });
    } catch (e) {
      notice("SimpleX did not open", e.message);
    }
  }
  // Its page knows the agent by its label: told its name as it loads, and
  // again on a rename (`renderNames`), as a screen is (`nameScreen`).
  function nameSimplex(frame) {
    const agent = frame.dataset.simplex;
    try {
      frame.contentWindow?.postMessage({ fragment: KIND.simplex, agent, name: titleOf(agent) }, new URL(frame.dataset.src).origin);
    } catch {}
  }
  // The link page asks for a ticket to link or unlink its agent (`{fragment:
  // "simplex", agent, want}`): only a pane of Settings' onto its computer's
  // port, for its own agent. The platform makes it for the agent's owner
  // alone, signed (an unlink's closes the agent's door at once), and the
  // page hands it to the computer, which takes nothing else (docs/api.md,
  // "An agent's doors"). A link's ask may say whether the agent is to reach
  // its person there (`home`: the page asks as the link starts, yes by
  // default), which sets its home as the ticket is made (`setHome`); an
  // unlink's takes its home back to its own chat.
  const onMessage = async (e) => {
    const m = e.data;
    if (m?.fragment !== KIND.simplex || !["link", "unlink"].includes(m.want) || !computer()?.simplex) return;
    const frame = [...document.querySelectorAll("iframe[data-simplex]")].find((f) => f.contentWindow === e.source);
    if (!frame || frame.dataset.simplex !== m.agent || e.origin !== new URL(frame.dataset.src).origin) return;
    const reply = { fragment: KIND.simplex, agent: m.agent, for: m.want };
    try {
      reply.ticket = await api("POST", `/api/computers/${seg(computer().computer)}/agents/${seg(m.agent)}/door`, { door: "simplex", act: m.want });
    } catch (err) {
      reply.error = err.message;
    }
    const home = m.want === "link" ? (typeof m.home === "boolean" ? m.home : null) : false;
    if (reply.ticket && home !== null) {
      await setHome(m.agent, home).then(() => { reply.home = home; }, (err) => { reply.homeError = err.message; });
    }
    frame.contentWindow?.postMessage(reply, e.origin);
  };
  addEventListener("message", onMessage);
  // Settings' SimpleX: each agent's link, on an always-on computer. Only on
  // a computer whose image does SimpleX (its view's `simplex`: its bridge
  // said so); on any other, the section is hidden, nav link and all. It
  // follows the view while Settings are shown (`refreshSimplex`), with
  // nothing else on the page redrawn.
  function simplexSection() {
    const s = section("SimpleX");
    s.id = "settings-simplex";
    fillSimplex(s);
    return s;
  }
  const simplexKey = (c) => JSON.stringify([!!c?.simplex, !!c?.alwaysOn, (c?.agents ?? []).map((a) => a.fragment), getSeat()?.good, getSeat()?.kind, getSeat()?.sleeps]);
  // A computer's view read again (its list's socket told of a change): the
  // section shown, changed or hidden as the view now says.
  function refreshSimplex() {
    const s = document.getElementById("settings-simplex");
    if (s && s.dataset.key !== simplexKey(computer())) fillSimplex(s);
  }
  function fillSimplex(s) {
    const c = computer();
    s.replaceChildren(...s.querySelectorAll(":scope > h2"));
    s.dataset.key = simplexKey(c);
    s.hidden = !c?.simplex;
    const jump = document.querySelector('.settings-nav-link[data-section="settings-simplex"]');
    if (jump) jump.hidden = s.hidden;
    if (s.hidden) return;
    const agents = c.agents ?? [];
    // what it is, a word each (Paul, 2026-10-31), beside Telegram's
    // "Less secure" (templates/telegram): not in the heading, which names
    // its nav link
    const tags = el("div", "connector-tags");
    const secure = el("span", "role-tag connector-tag", "Secure");
    secure.title = "End-to-end encrypted to your computer";
    tags.append(el("span", "role-tag connector-tag", "Experimental"), secure);
    s.append(tags, el("p", "muted", "Message your agents from SimpleX Chat, a private messenger for your phone. Your messages are encrypted between your SimpleX app and your computer."));
    if (!agents.length) {
      s.append(el("p", "muted", "Make an agent first."));
      return;
    }
    if (!c.alwaysOn) {
      // an always-on seat whose holder let its computer sleep (Billing's
      // checkbox) is told how to take it back; anyone else, what keeps it on
      const seat = getSeat();
      const chose = seat?.good && seat.kind === "seat_always_on" && seat.sleeps;
      s.append(el("p", "muted", chose ? 'SimpleX needs your computer to stay on. Untick "Let my computer sleep when nothing is open" in Billing.' : "SimpleX works while your computer stays on, which the always-on plan does."));
      return;
    }
    for (const a of agents) {
      const open = el("button", "row", "Set up");
      open.type = "button";
      open.onclick = () => openSimplex(a.fragment);
      s.append(actionLine(named(el("span"), a.fragment), open));
    }
  }
  return { simplexSection, refreshSimplex, nameSimplex, dispose: () => removeEventListener("message", onMessage) };
}
