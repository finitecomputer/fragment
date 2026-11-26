import { KIND } from "./frame-messages.js";
import { icon } from "./icons.js";
import { providerMark } from "./provider-marks.js";
import { closeOnOutsidePress } from "./dialog.js";

// A provider's name for people: the platform's (`ProviderView.name`), or
// its id where an older release answered without one (a deploy going out,
// rolled back): the wire adds fields, so a reader takes each as optional.
export const providerLabel = (p) => p.name || p.provider;

// Provider/account UI keeps failed removals for the current settings visit.
export function createConnections({ api, el, section, named, seg, computer, getAgents, computerIs, asks, isOpen, reopenSettings }) {
  // ---- connections (decisions 22, 37 and 44): every provider the platform
  // offers, one row each: what it is, its state, which of the person's agents
  // may use it (all by default; a list narrows one agent), and this month's
  // calls by each agent, with an operator key's cost. Their agents use them
  // through the computer's swap, each with a placeholder of its own.
  const PROVIDER_STATE = {
    connected: "Connected",
    needs_reauthorization: "Needs reauthorization",
    not_connected: "Not connected",
    offered: "Offered",
    set: "Key set",
    not_set: "No key yet",
  };
  // a connection's scope for people: the name its provider's row gives it
  // (`scopeNames`). The shell knows no scope of its own: one a row leaves
  // unnamed is no chip (a sign-in's is never named), and a missing one,
  // which is never a sign-in's, shows by its last path segment
  // what a connection's account lets its agents do, picked as the person
  // connects it (`access`): its name for people, on a button and as said
  const ACCESS = { read: "read only", write: "read and write" };
  const ACCESS_NAME = { read: "Read only", write: "Read and write" };
  const scopeName = (p, scope) => p.scopeNames?.[scope] ?? scope.split("/").pop();
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
  // The accesses a connection offers (`accesses`): read only and read and
  // write, or, where its provider has no read-only scope, read and write
  // alone, the account's own permissions its limit.
  const offered = (p) => p.accesses ?? ["read", "write"];
  // an account's access to connect it again with: its own, else the one
  // its row offers, else read only (the lesser)
  const accessFor = (p, access) => access ?? (offered(p).length === 1 ? offered(p)[0] : "read");
  // A connect for each access the person may pick, read only and read and
  // write (the platform never picks for them): `verb` and the access in
  // words, each its own press (its window opened by that press); of a row
  // that offers one, one press, `verb` alone. `body` as `connectButton`'s;
  // each button's `data-access` is its access.
  function accessButtons(p, verb, said, body = {}, action = "connect") {
    const accesses = offered(p);
    return accesses.map((access) => {
      const go = connectButton(p, accesses.length > 1 ? `${verb} ${ACCESS[access]}` : verb, said, { ...body, access });
      go.dataset.action = action;
      go.dataset.access = access;
      return go;
    });
  }
  // What a connected account lets its agents do, said beside its state.
  const accessSaid = (access) => {
    const said = el("span", "provider-access", ACCESS_NAME[access]);
    said.dataset.access = access;
    return said;
  };
  // A read-only account's Allow changes: a reconnect asking for read and
  // write (of several, that account). Going back to read only is a
  // disconnect and a connect: a provider may keep what it granted before.
  function allowChanges(p, said, body = {}) {
    const go = connectButton(p, "Allow changes", said, { ...body, access: "write" });
    go.dataset.action = "allow-changes";
    return go;
  }
  const readOnlyHow = () => el("span", "muted provider-detail provider-access-how", "To make it read only again, disconnect it, then connect it read only.");
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
      const name = providerLabel(p);
      await reopenSettings(revoked ? null : [`${name} is disconnected`, `Your agents can no longer use it. ${name} may still list this platform's access to your account: remove it in your ${name} account's security settings.`]).catch(() => {});
    };
    return off;
  }
  // What the person left unticked when they connected an account, in words.
  const notAllowed = (p, missing) => el("span", "provider-detail provider-missing", `Not allowed: ${[...new Set(missing.map((s) => scopeName(p, s)))].join(", ")}`);
  // An account of several, for people: its label, or the provider's name.
  const accountLabel = (p, a) => a.account ?? `A ${providerLabel(p)} account`;
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
      if (!a.disconnecting && a.access) who.append(accessSaid(a.access));
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
      // connected again as it was, or, its access unknown, as the person picks
      if (a.state !== "connected") actions.append(...(a.access ? [connectButton(p, "Connect again", said, { account: a.id, access: a.access })] : accessButtons(p, "Connect again", said, { account: a.id })));
      if (a.state === "connected" && a.missing?.length) {
        const again = connectButton(p, "Reconnect to add", said, { account: a.id, access: accessFor(p, a.access) });
        again.dataset.action = "reconnect";
        actions.append(again);
        details.push(notAllowed(p, a.missing));
      }
      if (a.state === "connected" && a.access === "read") actions.append(allowChanges(p, said, { account: a.id }));
      if (a.state === "connected" && a.access === "write") details.push(readOnlyHow());
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
    named(pick, a.fragment, (t) => `${t}'s ${providerLabel(p)} account`, "aria-label");
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
      const visit = asks.now("settings");
      pick.disabled = true;
      // Default is no pick: the agent follows it
      const account = pick.value || null;
      let refused = null;
      try {
        computerIs(await api("PUT", `/api/computers/${seg(computer().computer)}/agents/${seg(a.fragment)}/accounts/${encodeURIComponent(p.provider)}`, { account }));
      } catch (e) {
        refused = e;
      }
      await reopenSettings(refused && asks.current(visit) ? ["That was not changed", refused.message] : null);
    };
    return pick;
  }
  // ---- the section (Skyler's design, 2026-11-04): a card per provider, in
  // two groups (the person's own accounts and keys; the platform's keys,
  // included with their seat), each opening its sheet: its state and what
  // changes it, which agents may use it, and its details. A press reloads
  // settings, and the sheet open in this visit is shown again (`openSheet`),
  // but after a Disconnect or Remove, whose notice is the page's.
  let openSheet = null;
  // a sheet is Settings': leaving them (for a chat, Apps or History) closes it
  const settingsPage = document.getElementById("settings-page");
  new MutationObserver(() => {
    if (settingsPage.hidden) for (const d of settingsPage.querySelectorAll("dialog.provider-sheet[open]")) d.close();
  }).observe(settingsPage, { attributes: true, attributeFilter: ["hidden"] });
  // "2026-11" as people say it: November
  const monthName = (month) => {
    const [y, m] = (month ?? "").split("-").map(Number);
    return y && m ? new Date(Date.UTC(y, m - 1, 1)).toLocaleString(undefined, { month: "long", timeZone: "UTC" }) : "this month";
  };
  const callsSaid = (n) => `${n} ${n === 1 ? "call" : "calls"}`;
  // what a call is charged (an operator key's `charged`, as the ledger charges it)
  const perCall = (c) => (c ? money(Math.ceil(c.micros / c.per)) : null);
  // its light: on while their agents may use it; amber while it waits on the
  // person (connect again, or something they did not allow)
  const lightOf = (p) => {
    if (p.state === "needs_reauthorization" || (p.state === "connected" && p.missing?.length)) return "warn";
    return ["connected", "set", "offered"].includes(p.state) ? "on" : "";
  };
  function connectionsSection(linked, uses) {
    const s = section("Connections");
    s.id = "settings-connections";
    const offered = linked?.providers ?? [];
    if (!offered.length) {
      s.append(el("p", "muted", linked ? "This platform offers none yet." : "Your connections could not be read."));
      return s;
    }
    s.append(el("p", "muted", "Your agents use these to act, but can't see the credentials."));
    const agents = computer()?.agents ?? [];
    const month = monthName(uses?.month);
    const groups = [
      ["Your accounts", null, offered.filter((p) => p.kind !== "operator")],
      ["Included with your seat", "On the platform's keys, metered to your credit.", offered.filter((p) => p.kind === "operator")],
    ];
    for (const [title, sub, list] of groups) {
      if (!list.length) continue;
      const head = el("header", "provider-group-head");
      head.append(el("h3", null, title), ...(sub ? [el("p", null, sub)] : []));
      const grid = el("div", "provider-grid");
      grid.append(...list.map((p) => providerCard(p, offered, agents, uses, month)));
      s.append(head, grid);
    }
    return s;
  }
  function providerCard(p, offered, agents, uses, month) {
    const name = providerLabel(p);
    const card = el("article", "provider");
    card.dataset.provider = p.provider;
    card.dataset.kind = p.kind;
    card.dataset.state = p.state;
    const several = Array.isArray(p.accounts);
    // an own key signed in to is the person's account there
    const kind = p.kind === "operator" ? "Included with your seat" : p.signIn || p.kind === "connection" ? "Your account" : "Your key";
    const stateName =
      p.kind === "operator" ? "On for your agents" : p.signIn ? (p.state === "set" ? "Connected" : "Not connected") : PROVIDER_STATE[p.state] ?? p.state;
    const price = p.kind === "operator" ? perCall(p.charged) : null;
    // this month's calls through the swap, by agent, and an operator key's charge
    const mine = (uses?.uses ?? []).filter((u) => u.provider === p.provider);
    const calls = mine.reduce((n, u) => n + u.calls, 0);
    const micros = mine.reduce((n, u) => n + u.micros, 0);
    const charge = p.kind === "operator" && calls ? ` · ${money(micros)}` : "";
    // its mark (provider-marks.js), else its name's first letter
    const tile = (big) => {
      const mark = providerMark(p.provider);
      const t = el("span", `provider-tile${big ? " big" : ""}`, mark ? "" : name.slice(0, 1));
      if (mark) t.append(mark);
      return t;
    };
    const light = () => {
      const st = el("span", "provider-state");
      st.append(el("span", `provider-light ${lightOf(p)}`));
      return st;
    };
    // the card: its name, a key's price, its light, and this month's calls;
    // the whole card opens its sheet, which alone says the row's note (Paul,
    // 2026-11-20: no explaining on the list)
    const body = el("span", "provider-body");
    body.append(el("span", "provider-name", name));
    if (price) body.append(el("span", "provider-sub provider-price", `${price} a call`));
    const foot = el("span", "provider-foot", calls ? `${callsSaid(calls)} in ${month}${charge}` : `No calls in ${month}`);
    const open = el("button", "provider-open");
    open.type = "button";
    open.dataset.action = "open";
    open.setAttribute("aria-haspopup", "dialog");
    open.setAttribute("aria-label", `${name}: ${stateName}`);
    // the sheet
    const sheet = el("dialog", "provider-sheet");
    sheet.setAttribute("aria-label", name);
    // focused itself as it opens (`show`), not its X, whose ring and tooltip
    // would show: Chrome's dialog focusing skips a dialog's own autofocus
    sheet.tabIndex = -1;
    const show = () => {
      sheet.showModal();
      sheet.focus();
    };
    const top = el("div", "provider-sheet-head");
    const titled = el("div");
    titled.append(el("h3", null, name), el("p", "provider-kind", kind));
    const close = el("button", "icon-button provider-close");
    close.type = "button";
    close.setAttribute("aria-label", "Close");
    close.append(icon("x"));
    close.onclick = () => sheet.close();
    top.append(tile(true), titled, close);
    const inner = el("div", "provider-sheet-body");
    if (p.note) inner.append(el("p", "provider-about provider-note", p.note));
    // what a connection's account allows, in words: each granted scope its
    // row names for people
    const granted = !several && p.kind === "connection" && p.state === "connected" ? p.granted ?? [] : [];
    const scopes = [...new Set(granted.map((g) => p.scopeNames?.[g]).filter(Boolean))];
    if (scopes.length) {
      const chips = el("div", "provider-chips");
      chips.append(...scopes.map((g) => el("span", null, g)));
      inner.append(chips);
    }
    // its state, with what changes it
    const status = el("div", "provider-status");
    const words = el("span", "provider-status-text");
    words.append(el("span", "provider-state-name", stateName));
    if (p.account && !several) {
      const account = el("span", "provider-account", p.account);
      account.dataset.account = p.account;
      words.append(account);
    }
    if (!several && p.kind === "connection" && p.state === "connected" && p.access) words.append(accessSaid(p.access));
    const actions = el("div", "settings-actions");
    status.append(light(), words, actions);
    const details = [];
    // what a Connect press says: a blocked window, or a start refused; and
    // a Disconnect or Remove that did not go through
    const said = el("span", "muted provider-detail provider-said");
    // Disconnect or Remove: settings then reload, and the row they show says
    // whether the key is still there; one that failed says so in it
    const removeKey = (button) => async () => {
      const visit = asks.now("settings");
      button.disabled = true;
      unremoved.delete(p.provider);
      openSheet = null;
      try {
        await api("DELETE", `/api/connections/${encodeURIComponent(p.provider)}/key`);
      } catch {
        // kept only while its visit is the one shown: an older visit's
        // answer never replaces a newer one's
        if (asks.current(visit) && isOpen()) {
          unremoved.set(p.provider, visit);
          openSheet = { provider: p.provider, visit };
        }
      }
      await reopenSettings();
    };
    // Connect, read only or read and write, as the person picks; connected
    // again as it was where its access is known
    if (p.kind === "connection" && p.state !== "connected" && !(several && p.accounts.some((a) => !a.disconnecting))) {
      const again = p.state === "needs_reauthorization";
      actions.append(...(again && p.access ? [connectButton(p, "Connect again", said, { access: p.access })] : accessButtons(p, again ? "Connect again" : "Connect", said)));
      details.push(said);
    }
    // of several, another while the platform takes more; each account is
    // reconnected and disconnected in its own row
    // theirs to use: none they disconnected (which Pipes may hold still)
    const theirs = several ? p.accounts.filter((a) => !a.disconnecting) : [];
    if (several && theirs.length && theirs.length < (p.accountsMax ?? 1)) {
      actions.append(...accessButtons(p, `Add another ${name} account`, said, { add: true }, "add-account"));
      details.push(said);
    }
    // what the person left unticked when they connected it: a reconnect
    // asks for it again (the provider keeps what they allowed before)
    if (!several && p.kind === "connection" && p.state === "connected" && p.missing?.length) {
      const again = connectButton(p, "Reconnect to add", said, { access: accessFor(p, p.access) });
      again.dataset.action = "reconnect";
      actions.append(again);
      details.push(notAllowed(p, p.missing), said);
    }
    // read only: Allow changes; read and write: how to go back
    if (!several && p.kind === "connection" && p.state === "connected" && p.access === "read") {
      actions.append(allowChanges(p, said));
      if (!details.includes(said)) details.push(said);
    }
    if (!several && p.kind === "connection" && p.state === "connected" && p.access === "write") details.push(readOnlyHow());
    if (!several && p.kind === "connection" && p.state !== "not_connected") {
      const off = disconnectButton(p, said);
      const press = off.onclick;
      off.onclick = () => {
        openSheet = null;
        return press();
      };
      actions.append(off);
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
        if (p.models?.length) details.push(el("span", "muted provider-detail", `Your ${name} account pays for its models; then pick one for an agent under Agents.`));
      } else {
        const off = el("button", "quiet", "Disconnect");
        off.type = "button";
        off.dataset.action = "disconnect";
        off.onclick = removeKey(off);
        const manage = el("a", "provider-manage", `Revoke the key at ${name}`);
        manage.href = p.signIn.manage;
        manage.target = "_blank";
        manage.rel = "noopener";
        actions.append(off);
        details.push(manage);
      }
    } else if (p.kind === "own") {
      const form = el("form", "provider-key");
      const input = el("input");
      input.type = "password";
      input.autocomplete = "off";
      input.placeholder = p.state === "set" ? "Replace your key" : `Your ${name} key`;
      input.setAttribute("aria-label", `Your ${name} key`);
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
      details.unshift(form);
      if (p.state === "set") {
        const remove = el("button", "quiet", "Remove");
        remove.type = "button";
        remove.dataset.action = "remove-key";
        remove.onclick = removeKey(remove);
        actions.append(remove);
      }
    }
    if (p.state === "set" && asks.current(unremoved.get(p.provider))) {
      said.textContent = "That didn't go through. Try again.";
      details.push(said);
    } else unremoved.delete(p.provider);
    inner.append(status);
    const more = el("div", "provider-details");
    more.append(...new Set(details));
    if (more.childElementCount) inner.append(more);
    if (several && p.accounts.length) inner.append(accountsList(p));
    // which agents may use it: all by default; one narrowed to a list uses
    // those; of two or more accounts, or a pick saved, which each uses
    if (agents.length) {
      const who = el("div", "provider-block provider-agents");
      if (!lightOf(p)) who.classList.add("dim");
      who.append(el("h4", null, "Which agents may use it"));
      for (const a of agents) {
        const allowed = a.connections == null || a.connections.includes(p.provider);
        const line = el("div", "provider-agent");
        const face = el("span", "settings-agent-name");
        face.dataset.agentFace = a.fragment;
        const used = mine.find((u) => u.agent === a.fragment);
        const use = el("span", "use", used ? (p.kind === "operator" ? `${callsSaid(used.calls)}, ${money(used.micros)}` : callsSaid(used.calls)) : "No calls");
        if (used) {
          use.dataset.use = used.agent;
          use.dataset.calls = String(used.calls);
          use.dataset.micros = String(used.micros);
        }
        const flip = el("button", "provider-switch");
        flip.type = "button";
        flip.dataset.agent = a.fragment;
        flip.setAttribute("aria-pressed", String(allowed));
        named(flip, a.fragment, (t) => `${t} may use ${name}`, "aria-label");
        flip.onclick = async () => {
          const visit = asks.now("settings");
          flip.disabled = true;
          openSheet = { provider: p.provider, visit };
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
          const failed = refused && unchanged && asks.current(visit);
          // said over settings, the sheet closed so it is seen
          if (failed) openSheet = null;
          await reopenSettings(failed ? ["That was not changed", refused.message] : null);
        };
        line.append(face, use, flip);
        who.append(line);
        if (allowed && several && (theirs.length >= 2 || a.accounts?.[p.provider])) who.append(accountPick(p, a, theirs));
      }
      inner.append(who);
    }
    // its details: this month's use, what a call is charged, where it
    // reaches, the variable an agent names it by, and an own key's models
    const facts = el("dl", "provider-facts");
    const fact = (term, ...value) => {
      const dd = el("dd");
      dd.append(...value);
      facts.append(el("dt", null, term), dd);
    };
    const codes = (list) => list.flatMap((x, i) => [...(i ? [", "] : []), el("code", null, x)]);
    fact(`In ${month}`, calls ? `${callsSaid(calls)}${p.kind === "operator" ? `, ${money(micros)} charged to your credit` : ""}` : "No calls yet");
    if (price) fact("Price", `${price} a call, metered to your credit`);
    if (p.models?.length) fact("Models", ...p.models.map((m) => el("span", "provider-model", m.name)));
    if (p.hosts?.length) fact("Reaches", ...codes(p.hosts));
    if (p.env?.length) fact("Variable", ...codes(p.env.map((v) => `$${v}`)));
    const block = el("div", "provider-block");
    block.append(el("h4", null, "Details"), facts);
    inner.append(block);
    sheet.append(top, inner);
    // closed by its X, Escape or a press outside it (its backdrop)
    closeOnOutsidePress(sheet);
    sheet.addEventListener("close", () => {
      // (not one a reload took away: its replacement is shown)
      if (openSheet?.provider === p.provider && sheet.isConnected) openSheet = null;
    });
    open.onclick = () => {
      openSheet = { provider: p.provider, visit: asks.now("settings") };
      show();
    };
    // the sheet open before this reload, shown again once the page has it
    if (openSheet?.provider === p.provider && asks.current(openSheet.visit)) {
      queueMicrotask(() => {
        if (sheet.isConnected && !sheet.open) show();
      });
    }
    card.append(tile(false), body, light(), foot, open, sheet);
    return card;
  }
  return { connectionsSection };
}

// Home reads and writes share the agent.json writer with model settings.
export function createAgentHomes({ el, icon, named, actionLine, settingLabel, modelOption, savePick, agents, chats, own, labelOf, byName, titleOf, agentsOwnChat, agentJson, writeAgentJson, renderChats, readDoor = async () => null, now = () => Date.now() }) {
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
  // ---- the chat bound as an agent's SimpleX door (a connector set up
  // from Settings), found as its computer's bridge finds it (images/bridge
  // api.rs `door_candidates`), so a SimpleX home offered here is the chat
  // its routines run in: of the person's own chats it is in, those
  // labelled as SimpleX chats (`…-simplex`, `…-simplex-<n>`, as Settings
  // labels a connector), its own label's first, then by name, at most
  // HOME_DOORS_MAX; the first whose binding (`GET …/door`, `readDoor`) is
  // its SimpleX door. Each agent read alone, the first time something asks
  // (its pick drawn, its home SimpleX); a look that failed is asked again
  // DOOR_LOOK_AGAIN_MS after, as the page draws again; `forgetDoors` (a
  // binding Settings made) has them read anew. ----
  const HOME_DOORS_MAX = 8;
  const DOOR_LOOK_AGAIN_MS = 30_000;
  const simplexLabelled = (label) => /-simplex(-\d+)?$/.test(label);
  // each agent's look: {door, failed, at}, or one under way
  const doorLooks = new Map();
  const looking = new Set();
  function doorCandidates(agent) {
    const id = [...agents().values()].find((a) => a.fragment === agent)?.identity;
    const sx = `${labelOf(agent)}-simplex`;
    const mine = (f) => !labelOf(f.name).startsWith(sx);
    return chats()
      .filter((f) => own(f) && f.kind === "chat" && !!id && (f.agents ?? []).includes(id) && simplexLabelled(labelOf(f.name)))
      .sort((a, b) => mine(a) - mine(b) || (a.name < b.name ? -1 : a.name > b.name ? 1 : 0))
      .slice(0, HOME_DOORS_MAX);
  }
  function lookForDoor(agent) {
    const was = doorLooks.get(agent);
    if (looking.has(agent) || (was && (!was.failed || now() - was.at < DOOR_LOOK_AGAIN_MS))) return;
    looking.add(agent);
    (async () => {
      let failed = false;
      for (const c of doorCandidates(agent)) {
        try {
          const b = await readDoor(c.name);
          if (b?.door === "simplex" && b.agent === agent) return { door: c.name, failed: false };
        } catch {
          failed = true;
        }
      }
      return { door: null, failed };
    })().then((look) => {
      looking.delete(agent);
      doorLooks.set(agent, { ...look, at: now() });
      renderChats();
      paintHomes();
    });
  }
  function forgetDoors() {
    doorLooks.clear();
    paintHomes();
  }
  // The chat its SimpleX home is, as its bridge picks it (api.rs
  // `home_chat`): the chat bound as its SimpleX door, else its computer's
  // SimpleX chat, else none (its door looked for while unknown)
  const simplexHomeOf = (agent) => {
    lookForDoor(agent);
    const door = doorLooks.get(agent)?.door;
    return (door ? (chats().find((f) => f.name === door) ?? null) : null) ?? simplexChatOf(agent);
  };
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
  // Settings, with Try again (`homeUnread`); Settings shown read it again.
  // That line stays while a read is out (a list's refresh reads it too), so
  // it never jumps from under a hand: an answer takes it away. Reads may
  // overlap (Try again reads afresh): only the newest one's answer applies,
  // success or failure (`homeReads`, each read's number), so an older one
  // never puts back what a newer one replaced. ----
  const homes = new Map();
  const homeWrites = new Map();
  const homeReads = new Map();
  let homeReadsAsked = 0;
  const homeUnread = new Set();
  const wrote = (agent) => homeWrites.set(agent, (homeWrites.get(agent) ?? 0) + 1);
  // (`afresh`: a read of its own, whatever read is out)
  function readHome(agent, afresh = false) {
    if (!afresh && homeReads.has(agent)) return;
    const mine = ++homeReadsAsked;
    homeReads.set(agent, mine);
    const asked = homeWrites.get(agent) ?? 0;
    agentJson(agent)
      .catch((e) => (e.status === 404 ? {} : null))
      .then((j) => {
        if (homeReads.get(agent) !== mine) return;
        homeReads.delete(agent);
        if ((homeWrites.get(agent) ?? 0) !== asked) return;
        if (j) {
          homeUnread.delete(agent);
          return homeIs(agent, j.home);
        }
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
  const homeChatOf = (agent) => (homes.get(agent) === "simplex" && simplexHomeOf(agent)) || agentsOwnChat(agent);
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
      readHome(agent, true);
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
    // a read of its own, whatever read is out: that one was asked before
    again.onclick = () => readHome(a.fragment, true);
    const unread = actionLine(el("span", "muted", "Couldn't read where it reaches you."), again);
    unread.dataset.homeUnread = a.fragment;
    homeUnreadLines.set(pick, unread);
    paintHome(pick);
    if (!homes.has(a.fragment)) readHome(a.fragment);
    return [settingLabel("Reaches you in", pick), unread];
  }
  // A home pick shows its agent's home as the shell knows it (`homes`),
  // with its own chat and, once it has one (`simplexHomeOf`: its computer's,
  // or the chat bound as its SimpleX door), its SimpleX chat to pick; while
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
    // (its door looked for either way: its bell reads the same)
    const simplex = simplexHomeOf(agent);
    if (home === "simplex" || simplex) options.push(modelOption("simplex", SIMPLEX_TITLE));
    pick.replaceChildren(...options);
    pick.value = pick.dataset.was = home === "simplex" ? "simplex" : "";
    pick.disabled = false;
    pick.dataset.read = "1";
  }
  const paintHomes = () => document.querySelectorAll("select[data-home-for]").forEach(paintHome);
  return { chatTitle, homeMark, loadHomes, homePicker, paintHomes, setHome, simplexChatOf, forgetDoors };
}

// The computer port UI owns its frame-message listener and origin checks.
export function createSimplex({ api, el, actionLine, named, seg, computer, titleOf, frameOf, pressSetup, openSetup, simplexChatOf, openChat, notice, setHome, markOffer, refresh }) {
  // ---- an agent's SimpleX: its link page, a port of its computer's own
  // origin as its screen is, through a ticket (docs/bridge.md, "SimpleX"):
  // the one-time QR code, and whom it is linked to. Shown in the connector
  // setup dialog (connector-setup.js), as Telegram's setup is (FIN-393);
  // linking again (Unlink, then Link) is how a person reconnects ----
  const opening = new Set();
  async function openSimplex(agent) {
    if (!computer()?.simplex || !agent || opening.has(agent)) return;
    opening.add(agent);
    const pressed = pressSetup();
    try {
      const t = await api("POST", `/api/computers/${seg(computer().computer)}/ports/6080/ticket`, { path: `/simplex/?agent=${encodeURIComponent(agent)}` });
      const frame = named(frameOf(t.url, `${titleOf(agent)} on SimpleX`), agent, (n) => `${n} on SimpleX`, "aria-label");
      frame.dataset.simplex = agent;
      frame.addEventListener("load", () => nameSimplex(frame));
      // its invitation's QR code, whole, on a wide screen (connector-setup.js)
      openSetup({ pressed, frame, agent, title: "SimpleX", tall: true, onClose: refresh });
    } catch (e) {
      notice("SimpleX did not open", e.message);
    } finally {
      opening.delete(agent);
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
  // The link page asks for a ticket to link, unlink or adopt (FIN-463: keep
  // its owner's SimpleX chat with their old agent) for its agent (`{fragment:
  // "simplex", agent, want}`): only a frame of Settings' dialog onto its computer's
  // port, for its own agent. The platform makes it for the agent's owner
  // alone, signed (an unlink's closes the agent's door at once), and the
  // page hands it to the computer, which takes nothing else (docs/api.md,
  // "An agent's doors"). A link's ask may say whether the agent is to reach
  // its person there (`home`: the page asks as the link starts, yes by
  // default), which sets its home as the ticket is made (`setHome`); an
  // unlink's takes its home back to its own chat.
  const onMessage = async (e) => {
    const m = e.data;
    if (m?.fragment !== KIND.simplex || !["link", "unlink", "adopt"].includes(m.want) || !computer()?.simplex) return;
    const frame = [...document.querySelectorAll("iframe[data-simplex]")].find((f) => f.contentWindow === e.source);
    if (!frame || frame.dataset.simplex !== m.agent || e.origin !== new URL(frame.dataset.src).origin) return;
    const reply = { fragment: KIND.simplex, agent: m.agent, for: m.want };
    try {
      reply.ticket = await api("POST", `/api/computers/${seg(computer().computer)}/agents/${seg(m.agent)}/door`, { door: "simplex", act: m.want });
    } catch (err) {
      reply.error = err.message;
    }
    // an adoption (its chat with their old agent kept) leaves its home be
    const home = m.want === "link" ? (typeof m.home === "boolean" ? m.home : null) : m.want === "unlink" ? false : null;
    if (reply.ticket && home !== null) {
      await setHome(m.agent, home).then(() => { reply.home = home; }, (err) => { reply.homeError = err.message; });
    }
    frame.contentWindow?.postMessage(reply, e.origin);
  };
  addEventListener("message", onMessage);
  // An agent's SimpleX on its computer, where its computer keeps it: an
  // always-on computer whose image does SimpleX (its view's `simplex`: its
  // bridge said so), with its links and a moved chat to keep there
  // (FIN-463), until they move to doors (D4). Its row: Set up, its
  // computer's link page, and Open once it has its SimpleX chat (its
  // computer makes one as its link is made: the list says so, never a
  // guess). Anywhere else, none: Settings' SimpleX is its connector's
  // (createConnector's `SIMPLEX`), on every seat.
  const onComputer = () => !!(computer()?.simplex && computer()?.alwaysOn);
  function computerRow(a) {
    if (!onComputer()) return null;
    const go = el("button", "row", "Set up");
    go.type = "button";
    go.dataset.simplexFor = a.fragment;
    go.dataset.simplexComputer = "";
    go.onclick = () => openSimplex(a.fragment);
    const actions = [go];
    const chat = simplexChatOf(a.fragment);
    if (chat) {
      const open = el("button", "row", "Open");
      open.type = "button";
      open.dataset.simplexOpen = chat.name;
      open.onclick = () => openChat(chat.name);
      actions.unshift(open);
    }
    return markOffer(actionLine(named(el("span"), a.fragment), ...actions), a.fragment, "doors");
  }
  // what its rows are drawn from: a change redraws the section
  const computerKey = () => JSON.stringify([onComputer(), (computer()?.agents ?? []).map((a) => [a.fragment, simplexChatOf(a.fragment)?.name ?? null])]);
  return { computerRow, computerKey, nameSimplex, dispose: () => removeEventListener("message", onMessage) };
}

// Settings' connectors, Telegram's, Slack's and SimpleX's (templates/telegram,
// templates/slack, templates/simplex: their READMEs): each agent's chat on that messenger,
// a connector of the person's own and a hosted door (docs/api.md, "An
// agent's doors"), for an agent whose runtime takes one (agent-runtimes.js
// `doors`), on any seat. Its row: Set up, its connector's own setup page in
// the connector setup dialog (connector-setup.js), a connector made first
// and bound as the agent's door when it has none; and, with one, Open, its
// chat. Setting up again is how a person reconnects (a new token, a new
// link, on that page). Where its computer keeps an agent's SimpleX
// (createSimplex `computerRow`), its row is that one instead.
// Which chat is whose is read, never assumed: each of the person's own
// chats with reach (a door is reach: reach.rs) and each this page bound,
// its door as its owner reads it (`GET /api/f/{name}/door`), once for
// every connector, as Settings opens, as the list's chats with reach
// change, and again as Set up is pressed, the list read anew first
// (another tab may have made one since, which this page's list may not
// show yet). Its doors are the latest read's, whole, or none: while that
// read failed (or the list was not read) its rows say Try again, which
// reads the list again first.
// A connector Set up makes whose binding is refused (a 4xx) is deleted
// again; one whose binding's answer was lost is read: bound to its agent,
// it is set up; bound to another meanwhile (its owner, elsewhere), kept as
// theirs; not bound, deleted; unreadable, kept (its row finds it once its
// door reads). So a press never deletes one it could not account for.

// Each connector: its door (its template's), its name, its section's line,
// what it is (a tag each, beside "Experimental": Paul, 2026-10-31,
// literally so), and its setup page for an agent (`name`, as Settings
// names it). Telegram's offers a bot its owner's Vault holds (`vault`: its
// host); SimpleX's page shows a QR code, and Slack's first step two
// fields under its link and its line (`tall`).
export const TELEGRAM = {
  door: "telegram",
  title: "Telegram",
  said: "Message your agents from Telegram.",
  tag: ["Less secure", "Telegram's servers can read these messages"],
  setup: () => "/setup.html",
  vault: { host: "api.telegram.org", key: "TELEGRAM_BOT_TOKEN" },
};
// Slack's page makes the agent's own Slack app, named as Settings names
// the agent (`name`).
export const SLACK = {
  door: "slack",
  title: "Slack",
  said: "Bring your agents into your team's Slack.",
  tag: ["Less secure", "Slack's servers, and everyone in a room it's in, can read these messages"],
  setup: (name) => `/setup.html?name=${encodeURIComponent(name)}`,
  tall: true,
};
export const SIMPLEX = {
  door: "simplex",
  title: "SimpleX",
  said: "Message your agents from SimpleX Chat.",
  tag: ["Secure", "SimpleX's servers can't read these messages"],
  setup: (name) => `/setup.html?name=${encodeURIComponent(name)}`,
  tall: true,
};

export function createConnectors({ api, el, section, actionLine, named, seg, computer, fragments, listed, own, connectorLabel, markOffer, framed, frameOf, originAt, pressSetup, openSetup, openChat, notice, load, titleOf, computerRow = () => null, computerKey = () => "", doorsRead = () => {} }) {
  // each read connector's door by its name ({door, agent}, or null), as the
  // latest read says; null while it has not answered whole
  let doors = null;
  let reads = 0;
  const bound = new Set();
  // the chats with reach the last read asked about
  let asked = "";
  // each Set up under way: its door and its agent
  const making = new Set();
  const agentsNow = () => computer()?.agents ?? [];
  const reaching = () => [...new Set([...fragments().filter((f) => own(f) && f.kind === "chat" && f.sharing?.reach === true).map((f) => f.name), ...bound])].sort();
  // Its doors read now; whether that read answered whole (and is the latest).
  async function readDoors() {
    const mine = ++reads;
    // a list not read is no word that no chat is a connector
    if (!listed()) {
      doors = null;
      return false;
    }
    const names = reaching();
    asked = JSON.stringify(names);
    // a connector deleted since binds nothing
    const answers = await Promise.all(names.map((name) => api("GET", `/api/f/${seg(name)}/door`).then((r) => [name, r.bound ?? null], (e) => [name, e.status === 404 ? null : undefined])));
    if (mine !== reads) return false;
    const whole = answers.every(([, b]) => b !== undefined);
    doors = whole ? new Map(answers) : null;
    // what reads a binding (an agent's SimpleX home) reads it again
    doorsRead();
    return whole;
  }
  // the connector bound as `agent`'s `door`, if any
  const boundOf = (door, agent) => [...(doors ?? [])].find(([, b]) => b?.door === door && b.agent === agent)?.[0] ?? null;
  // its setup page, offering the bot token its owner's Vault holds for
  // `agent` where its kind takes one (Telegram's, a bot moved from
  // finite.computer, FIN-463 F11): a secret bound to it toward that host
  // alone. A Vault not read offers none
  async function openConnector(kind, pressed, name, agent) {
    const vault = kind.vault ? await api("GET", "/api/vault").then((v) => v.secrets ?? [], () => []) : [];
    const bot = vault.find((v) => v.hosts?.length === 1 && v.hosts[0] === kind.vault.host && v.agents?.includes(agent));
    const page = kind.setup(titleOf(agent));
    const src = bot ? `${page}${page.includes("?") ? "&" : "?"}vault=${encodeURIComponent(bot.name)}` : page;
    const frame = frameOf(framed(name, src), `${kind.title} setup`, name);
    const opened = openSetup({ pressed, src: framed(name, src), name, agent, frame, title: kind.title, tall: !!kind.tall, onClose: () => { offers.delete(frame); refreshConnectors(true); } });
    // the offer its page's Use it asks the shell to move (`onVaultMove`)
    if (opened && bot) offers.set(frame, { name, vault: bot.name, hosts: bot.hosts, key: kind.vault.key });
  }
  // Each setup frame's offer: the Vault bot Settings found for its agent
  // and the connector it opened. A page may not move a Vault secret itself
  // (`fromVault` on `__secrets`: the cell refuses it, members.rs
  // `put_secret`), since a page runs its authors' code with its visitor's
  // session; so its Use it asks here (`KIND.vaultMove`), and the
  // shell moves only what it offered that frame, into that connector,
  // toward the Vault's own hosts. The page names nothing it could change.
  const offers = new Map();
  const onVaultMove = async (e) => {
    if (e.data?.fragment !== KIND.vaultMove) return;
    const frame = [...offers.keys()].find((f) => f.contentWindow === e.source);
    const offer = frame && offers.get(frame);
    if (!offer || e.origin !== originAt(offer.name)) return;
    const reply = { fragment: KIND.vaultMove, nonce: e.data.nonce, ok: false };
    try {
      await api("PUT", `/api/f/${seg(offer.name)}/secrets/${offer.key}`, { fromVault: offer.vault, hosts: offer.hosts });
      reply.ok = true;
    } catch (err) {
      reply.status = err.status;
      reply.error = err.message;
    }
    frame.contentWindow?.postMessage(reply, e.origin);
  };
  addEventListener("message", onVaultMove);
  // Its binding, as `POST …/door` answers or, its answer lost, as its door
  // reads: the binding to `agent`; a refusal, or no binding, throws (the
  // connector deleted), and so do a read that fails and a binding to
  // another agent (`keep`: the connector is left be).
  async function bind(door, name, agent) {
    try {
      return await api("POST", `/api/f/${seg(name)}/door`, { act: "link", agent });
    } catch (e) {
      if (e.status >= 400 && e.status < 500) throw e;
      const now = await api("GET", `/api/f/${seg(name)}/door`).then((r) => r.bound ?? null, () => undefined);
      if (now === undefined) throw Object.assign(new Error("Couldn't tell whether it was set up. Try again."), { keep: true });
      if (now?.door === door && now.agent === agent) return now;
      if (now) throw Object.assign(new Error("It was linked to another agent meanwhile."), { keep: true });
      throw e;
    }
  }
  async function setUp(kind, agent) {
    const key = `${kind.door} ${agent}`;
    if (making.has(key)) return;
    making.add(key);
    const pressed = pressSetup();
    refreshConnectors();
    let made = null;
    try {
      // the list read anew, then its doors: a connector another tab made is
      // found, whatever this page's list said (a list that fails, none made)
      await load().catch(() => {});
      if (!(await readDoors())) throw new Error(`Couldn't read your ${kind.title} chats. Try again.`);
      const connector = boundOf(kind.door, agent);
      if (connector) {
        await openConnector(kind, pressed, connector, agent);
        return;
      }
      made = await api("POST", "/api/fragments", { label: connectorLabel(agent, kind.door), template: kind.door });
      bound.add(made.name);
      const door = await bind(kind.door, made.name, agent);
      reads++;
      doors = new Map(doors ?? []);
      doors.set(made.name, { door: door.door, agent: door.agent });
      // the list read first, so its chat is under its agent as its setup opens
      await load().catch(() => {});
      await openConnector(kind, pressed, made.name, agent);
    } catch (e) {
      if (made && !e.keep) {
        bound.delete(made.name);
        await api("DELETE", `/api/f/${seg(made.name)}`).catch(() => {});
      }
      notice(`${kind.title} was not set up`, e.message);
    } finally {
      making.delete(key);
      await refreshConnectors();
    }
  }
  // Try again: the list first when it was not read, then the doors
  async function again() {
    if (!listed()) await load().catch(() => {});
    await refreshConnectors(true);
  }
  const sectionId = (kind) => `settings-${kind.door}`;
  function sectionOf(kind) {
    const s = section(kind.title);
    s.id = sectionId(kind);
    // hidden, nav link and all, with none of its rows shown (agent-runtimes.js `renderOffers`)
    s.dataset.offers = "";
    fill(kind, s);
    return s;
  }
  const keyOf = (kind) => JSON.stringify([agentsNow().map((a) => a.fragment), doors && [...doors], [...making], kind === SIMPLEX ? computerKey() : ""]);
  function fill(kind, s) {
    s.replaceChildren(...s.querySelectorAll(":scope > h2"));
    s.dataset.key = keyOf(kind);
    const tags = el("div", "connector-tags");
    const what = el("span", "role-tag connector-tag", kind.tag[0]);
    what.title = kind.tag[1];
    tags.append(el("span", "role-tag connector-tag", "Experimental"), what);
    s.append(tags, el("p", "muted", doors ? kind.said : `Couldn't read your ${kind.title} chats.`));
    const rows = agentsNow().map((a) => {
      const its = kind === SIMPLEX ? computerRow(a) : null;
      if (its) return its;
      const connector = boundOf(kind.door, a.fragment);
      const busy = making.has(`${kind.door} ${a.fragment}`);
      const go = el("button", "row", busy ? "Setting up…" : doors ? "Set up" : "Try again");
      go.type = "button";
      go.disabled = busy;
      go.dataset[`${kind.door}For`] = a.fragment;
      go.onclick = () => (doors ? setUp(kind, a.fragment) : again());
      const actions = [go];
      if (connector) {
        const open = el("button", "row", "Open");
        open.type = "button";
        open.dataset[`${kind.door}Open`] = connector;
        open.onclick = () => openChat(connector);
        actions.unshift(open);
      }
      return markOffer(actionLine(named(el("span"), a.fragment), ...actions), a.fragment, "doors");
    });
    s.append(...rows);
    s.hidden = !rows.some((r) => !r.hidden);
    const jump = document.querySelector(`.settings-nav-link[data-section="${sectionId(kind)}"]`);
    if (jump) jump.hidden = s.hidden;
  }
  // The sections drawn again as their view and doors now say, while
  // Settings show them; the doors read first when asked (`read`) or when
  // the list's chats with reach changed since they were.
  async function refreshConnectors(read = false) {
    const shown = [TELEGRAM, SLACK, SIMPLEX].filter((kind) => document.getElementById(sectionId(kind)));
    if (!shown.length) return;
    if (read || (listed() && JSON.stringify(reaching()) !== asked)) await readDoors();
    for (const kind of shown) {
      const s = document.getElementById(sectionId(kind));
      if (s && s.dataset.key !== keyOf(kind)) fill(kind, s);
    }
  }
  return { telegramSection: () => sectionOf(TELEGRAM), slackSection: () => sectionOf(SLACK), simplexSection: () => sectionOf(SIMPLEX), refreshConnectors, readConnectors: readDoors };
}
