import { avatar } from "./agent-identity.js";

// Newest first, with a newly made chat (no preview yet) before older ones.
export const newestChat = (a, b) => (b.previewAt ?? Infinity) - (a.previewAt ?? Infinity) || (a.name < b.name ? -1 : 1);

// Whom the ways to ask for an app ask (the desktop's, the dock's Add's),
// as their words name it (Paul, 2026-10-31: an agent's chosen name, every
// surface): a prompt goes into the open chat when it is the person's own
// (shell.js `promptChat`), so that chat's agent of theirs, the lead first;
// else their main agent's chat, so their main agent.
export function askedAgentOf({ open, own, agentsOf, mine, main }) {
  if (open?.kind !== "chat" || !own(open)) return main;
  return agentsOf(open.name).map((id) => mine.find((a) => a.identity === id)).find(Boolean) ?? main;
}

export function createChatList({ api, ui, data, selection, menus, navigation, listLive, load, invites, setInvites }) {
  const { $, el, patch, agentFace, notice } = ui;
  const { byName, own, chats, shown, titleOf, chatTitle, homeMark, isGroup, agentOfChat, agentsOf, agents, allFragments, computer, isGuest, personId, isThread, waitingOn, renderThreads } = data;
  const { current, page, getFrame } = selection;
  const { menu, menuEdge, menuOwner, openMenu, closeMenu, shareItem, archiveItem } = menus;
  const { newChat, rename, openChat, openApp, openScreen, leaveSidebar, seg } = navigation;
  // ---- chats: one flat list, newest first, the open one in the middle column ----
  function renderHeading() {
    const open = current() && byName(current()) && !page();
    // the session archive is read here, never a chat to add to or share
    const archive = byName(current())?.kind === "sessions";
    $("chat-heading").hidden = !open;
    $("page-title").hidden = !page();
    $("page-title").textContent = page() ?? "";
    $("new-chat").hidden = !open || archive || isGuest() || !primaryAgent()?.identity;
    $("chat-options").hidden = !open || archive;
    if (!open) return;
    const title = chatTitle(current());
    $("chat-title").textContent = title;
    $("chat-title").title = title;
    const frame = getFrame(current());
    if (frame) frame.setAttribute("aria-label", title);
  }
  // A chat's unread, as its row shows them: none on the open one (it is
  // being seen), and past the count's bound "99+" (the list counts at most
  // UNREAD_MAX: proto's `LISTED_UNREAD_MAX`, held to it by a host test,
  // xtask/src/guards.rs `the_shell_copies_its_limits`).
  const UNREAD_MAX = 100;
  function unreadMark(f) {
    const n = f.name === current() && !page() ? 0 : f.unread ?? 0;
    if (!n) return [];
    const b = el("span", "unread", n >= UNREAD_MAX ? "99+" : String(n));
    b.setAttribute("aria-label", n >= UNREAD_MAX ? "More than 99 unread" : `${n} unread`);
    return [b];
  }
  // The person's agents: those of their computer's that are theirs, in the
  // order it took them (its view's order).
  const ownAgents = () => [...agents().values()].filter((a) => byName(a.fragment)?.role === "owner");
  // Their main agent (Paul, 2026-10-30: several agents, one of them main):
  // their first, the one the first run makes (`defaultAgent`), or, before
  // their computer took one, their agent fragment. New chats are with it
  // unless the person picks another agent's New chat; the sidebar shows it
  // first, and each of their other agents on a row of its own below it.
  function primaryAgent() {
    const first = ownAgents()[0];
    if (first) return { fragment: first.fragment, identity: first.identity };
    const f = allFragments().find((x) => x.kind === "agent" && own(x));
    return f ? { fragment: f.name, identity: null } : null;
  }
  // What the person's agents are doing, from their chats' rows in the list
  // (docs/api.md, "Listed records"): each agent's newest record of a turn's
  // kind on a chat's `work` (docs/chat-records.md), which the shell reads as
  // the chat's page reads a turn. Started, a step, or a card answered is
  // working (a card closed any other way starts nothing: nothing); a card
  // open waits for the person (its owner answers its cards); a question
  // waits on whoever it asks (`asks`); an end is nothing. A message waiting
  // for its turn has no record yet, so queued is not said.
  // Working and waiting on someone else are none while their computer is
  // not awake (its turns stop with it); a card open still waits for them,
  // as it outlives its turn and its computer (docs/bridge.md, "A card
  // outlives its turn"). None while this page's list socket is shut (it
  // would not hear the change), and none past the bridge's own bound on a
  // turn that says nothing (15 minutes; a turn waiting on someone waits as
  // long as a card, an hour), each a minute more for the list's own lag.
  // Those three are copies, named as their sources name them (proto's
  // limits; images/bridge/src/limits.rs), each held to its source by a host
  // test (xtask/src/guards.rs, `the_shell_copies_its_limits`).
  // The list's turn kinds are the chat's records (docs/chat-records.md):
  // the shell reads them as the bridge writes them. A listed field's
  // value is any text an agent wrote (up to 128 characters): `asks` and
  // `outcome` are only compared here, never shown; what is shown is these
  // words and the chat's title, as text (`el`'s textContent), never markup.
  const DOING_SAYS = { you: "Waiting for you", waiting: "Waiting", working: "Working" };
  const DOING_ORDER = Object.keys(DOING_SAYS);
  const DOING_OF_KIND = { "turn.start": "working", "turn.step": "working", "turn.prompt.closed": "working", "turn.prompt": "you", "turn.asked": "asked" };
  const LISTED_FRESH_MS = 60_000;
  const TURN_IDLE_MS_MAX = 15 * 60_000;
  const PROMPT_TTL_MS_DEFAULT = 60 * 60_000;
  const DOING_QUIET_MS = { working: TURN_IDLE_MS_MAX + LISTED_FRESH_MS, waiting: PROMPT_TTL_MS_DEFAULT + LISTED_FRESH_MS };
  // the list socket (`watchList`, below): declared here, as the sidebar reads it

  // The most pressing first (waiting for the person, then on someone, then
  // working), the newest first within each.
  const pressing = (x, y) => DOING_ORDER.indexOf(x.doing) - DOING_ORDER.indexOf(y.doing) || y.at - x.at;
  // What agents `who` (identities) are doing in chat `f`, the most pressing
  // first, each with when it stops counting (`until`).
  function doingIn(f, who) {
    if (!who.length || !listLive()) return [];
    const awake = computer()?.phase === "awake";
    const now = Date.now();
    const out = [];
    for (const r of f.listed ?? []) {
      const kind = r.channel === "work" && who.includes(r.principal) ? DOING_OF_KIND[r.kind] : undefined;
      if (!kind) continue;
      if (r.kind === "turn.prompt.closed" && r.fields?.outcome !== "answered") continue;
      if (!awake && r.kind !== "turn.prompt") continue;
      const doing = kind === "asked" ? (r.fields?.asks === personId() ? "you" : "waiting") : kind;
      const until = r.at + DOING_QUIET_MS[doing === "working" ? "working" : "waiting"];
      if (until > now) out.push({ chat: f.name, doing, at: r.at, until });
    }
    return out.sort(pressing);
  }
  // The sidebar says it again as the first of what it shows stops counting.
  let doingTimer = 0;
  function doingAgain(doing) {
    clearTimeout(doingTimer);
    const next = Math.min(...doing.map((x) => x.until));
    if (Number.isFinite(next)) doingTimer = setTimeout(renderChats, Math.max(1000, next - Date.now()));
  }
  // How long ago a chat's newest message was said, as its row says it: now,
  // minutes, hours, days within the week, else its date; with how long until
  // that says something else.
  function ago(at, now) {
    const ms = Math.max(0, now - at);
    const steps = [[60_000, null], [3_600_000, 60_000, "m"], [86_400_000, 3_600_000, "h"], [7 * 86_400_000, 86_400_000, "d"]];
    for (const [under, unit, suffix] of steps) {
      if (ms >= under) continue;
      if (!unit) return ["now", under - ms];
      const n = Math.floor(ms / unit);
      return [`${n}${suffix}`, (n + 1) * unit - ms];
    }
    const d = new Date(at);
    return [d.toLocaleDateString([], { month: "short", day: "numeric", ...(d.getFullYear() === new Date(now).getFullYear() ? {} : { year: "numeric" }) }), Infinity];
  }
  // The sidebar is drawn again as the first row's time says something else.
  let agoTimer = 0;
  function agoAgain(next) {
    clearTimeout(agoTimer);
    if (Number.isFinite(next)) agoTimer = setTimeout(renderChats, Math.max(1000, next));
  }
  // A chat's row (Paul, 2026-10-31): its name, and how long ago its newest
  // message was said; under it what its agent is doing there, while it is,
  // else whom it waits on (a thread asked from here: threads.js), else that
  // message's first line. Its section names its agent. Its ⋯ (in the
  // time's place on hover, always on touch) renames or archives it.
  function chatRow(f, doing, now) {
    const active = f.name === current() && !page();
    const row = el("div", `row chat-row${active ? " active" : ""}`);
    row.dataset.key = `chat:${f.name}`;
    const open = el("button", "chat-open");
    open.type = "button";
    open.setAttribute("aria-current", active ? "page" : "false");
    open.onclick = () => { openChat(f.name); leaveSidebar(); };
    const line = el("span", "chat-line");
    line.append(el("span", "label", chatTitle(f.name)), ...homeMark(f));
    let next = Infinity;
    if (f.previewAt) {
      const [said, until] = ago(f.previewAt, now);
      const t = el("time", null, said);
      t.dateTime = new Date(f.previewAt).toISOString();
      line.append(t);
      next = until;
    }
    const sub = el("span", "chat-sub");
    const waits = doing ? null : waitingOn(f.name);
    if (doing) sub.append(el("span", `agent-doing ${doing.doing}`, DOING_SAYS[doing.doing]));
    else if (waits) sub.append(el("span", "agent-doing waiting", `Waiting on ${titleOf(waits.fragment)}`));
    else sub.append(el("span", "chat-preview", f.preview ?? ""));
    sub.append(...unreadMark(f));
    open.append(line, sub);
    const more = el("button", "row-more", "⋯");
    more.type = "button";
    more.title = "Rename or archive";
    more.setAttribute("aria-label", `${chatTitle(f.name)}: rename or archive`);
    more.setAttribute("aria-haspopup", "menu");
    more.setAttribute("aria-controls", "menu");
    const shownMore = $("chats").querySelector(`[data-key="${CSS.escape(row.dataset.key)}"] .row-more`);
    more.setAttribute("aria-expanded", String(!!shownMore && menuOwner()?.button === shownMore));
    more.onclick = () => openMenu(more, [{ icon: "rename", text: "Rename…", onClick: () => rename(f.name) }, archiveItem(f.name)]);
    row.append(open, more);
    return [row, next];
  }
  // Whom the launcher's new chat is with unless the person picks another
  // (Paul, 2026-10-30): the open chat's agent (a group's lead) when it is one
  // of theirs, else their main agent.
  function newChatAgent() {
    const mine = ownAgents();
    const open = current() && !page() ? agentsOf(current()).map((id) => mine.find((a) => a.identity === id)).find(Boolean) : null;
    return open ?? primaryAgent();
  }
  // The launcher's key, as the person's keyboard names it.
  const LAUNCHER_KEY = /Mac|iPhone|iPad|iPod/.test(navigator.userAgentData?.platform ?? navigator.platform ?? "") ? "⌘K" : "Ctrl K";
  // An agent's hero (Paul, 2026-10-31: the sidebar is a section per agent,
  // each headed by the original pill): its face, its name, and under it
  // what it is doing in any of its chats (the most pressing), real state
  // only. It opens its agent's menu, attached below it. Drawn again, it
  // keeps the edge its pointer lit and whether its menu is open, so it stays
  // the same node unless what it shows changed.
  function hero(agent, doing) {
    const says = DOING_SAYS[doing?.doing];
    const name = titleOf(agent.fragment);
    const row = el("button", "row agent-hero");
    row.type = "button";
    row.dataset.key = `agent:${agent.fragment}`;
    const was = $("chats").querySelector(`[data-key="${CSS.escape(row.dataset.key)}"]`);
    if (was?.hasAttribute("style")) row.setAttribute("style", was.getAttribute("style"));
    row.setAttribute("aria-label", `${name}${says ? `, ${says.toLowerCase()}` : ""}: its menu`);
    row.setAttribute("aria-haspopup", "menu");
    row.setAttribute("aria-controls", "menu");
    row.setAttribute("aria-expanded", String(!!was && menuOwner()?.button === was));
    const face = el("span", "agent-hero-face");
    face.append(avatar(agentFace(agent.fragment, agent.identity), "small"));
    const named = el("span", "agent-hero-named");
    named.append(el("strong", "title", name));
    if (says) named.append(el("span", `agent-doing ${doing.doing}`, says));
    row.append(face, named);
    // its agent as it is when pressed: a hero kept from an earlier drawing
    // (the same node) may have been drawn before its computer was read
    row.onclick = () => openAgentMenu(agentNow(agent.fragment), row);
    return row;
  }
  // An agent of theirs by its fragment, as the list and their computer say
  // now: its identity once their computer has it.
  const agentNow = (fragment) => ownAgents().find((a) => a.fragment === fragment) ?? (primaryAgent()?.fragment === fragment ? primaryAgent() : { fragment, identity: null });
  // An agent's menu: where it is doing something (each chat opens; read as
  // it opens, not from when its hero was drawn), a new chat with it, its
  // name, its profile and its screen. Its hero is first scrolled up enough
  // for the menu to hang below it in the window, as far as the list scrolls.
  const DOING_LISTED = 5;
  function openAgentMenu(agent, trigger) {
    const doing = agent.identity ? chats().flatMap((f) => doingIn(f, [agent.identity])).sort(pressing).slice(0, DOING_LISTED) : [];
    const items = [
      ...doing.map((x) => ({ icon: "agent", text: `${DOING_SAYS[x.doing]} in ${titleOf(x.chat)}`, onClick: () => { openChat(x.chat); leaveSidebar(); } })),
      ...(agent.identity && !isGuest() ? [{ icon: "newChat", text: "New chat", onClick: () => { newChat(agent.identity); leaveSidebar(); } }] : []),
      { icon: "rename", text: "Rename…", onClick: () => rename(agent.fragment) },
      { icon: "agent", text: "Its profile", onClick: () => openApp(agent.fragment) },
      ...(computer() ? [{ icon: "screen", text: "Its screen", onClick: () => openScreen(agent.fragment) }] : []),
    ];
    const nav = $("chats");
    const short = items.length * 34 + 16 - (innerHeight - trigger.getBoundingClientRect().bottom - 8);
    const room = Math.min(short, nav.scrollHeight - nav.clientHeight - nav.scrollTop);
    if (room > 0 && menuOwner()?.button !== trigger) {
      scrolledForMenu = true;
      nav.scrollTop += room;
    }
    openMenu(trigger, items);
  }
  // A hero's menu hangs from it where it was: the list scrolled by the
  // person (not for the menu's room, above), it closes.
  let scrolledForMenu = false;
  $("chats").addEventListener("scroll", () => {
    if (scrolledForMenu) scrolledForMenu = false;
    else if (menuOwner()?.button.classList.contains("agent-hero")) closeMenu(true);
  }, { passive: true });
  // A section's chats: its five newest (and the open one, wherever it is),
  // then More, which shows the rest, and Less, which folds them again.
  const SECTION_CHATS = 5;
  const unfolded = new Set();
  function sectionRows(key, list, rowOf) {
    const open = unfolded.has(key);
    const shown = open ? list : list.filter((f, i) => i < SECTION_CHATS || (f.name === current() && !page()));
    const rows = shown.map(rowOf);
    if (list.length > SECTION_CHATS) {
      const more = el("button", "row more-row", open ? "Less" : "More");
      more.type = "button";
      more.dataset.key = `more:${key}`;
      more.setAttribute("aria-expanded", String(open));
      more.onclick = () => { if (open) unfolded.delete(key); else unfolded.add(key); renderChats(); };
      rows.push(more);
    }
    return rows;
  }
  // The sidebar's chats, a section per agent of the person's (Paul,
  // 2026-10-31): their main agent first, then the others as their computer
  // took them, an order nothing reshuffles; each its hero, then its chats
  // (a chat goes under its lead agent, else the first of theirs in it, else
  // their main agent), the newest first. Chats someone else shared with them
  // are the last section, Shared. Before their computer is read, their main
  // agent's fragment heads theirs.
  function renderChats() {
    // the launcher: a guest's searches; a seat's also starts a chat
    const launcherLabel = isGuest() ? "Search chats and apps" : "Search or start a new chat";
    $("launcher-key").textContent = LAUNCHER_KEY;
    for (const id of ["sidebar-new-chat", "new-chat"]) {
      $(id).title = `${launcherLabel} (${LAUNCHER_KEY})`;
      $(id).dataset.tooltip = `${launcherLabel} (${LAUNCHER_KEY})`;
      $(id).setAttribute("aria-label", launcherLabel);
    }
    const mine = ownAgents();
    const ids = new Map(mine.map((a) => [a.identity, a]));
    const sections = new Map(mine.map((a) => [a.identity, { agent: a, chats: [] }]));
    const main = primaryAgent();
    if (!mine.length && main) sections.set(main.fragment, { agent: main, chats: [] });
    const first = sections.keys().next().value;
    const loose = [], shared = [];
    // a delegation thread's chat is no chat of the list: its line, over the
    // chat it was asked from, opens it (threads.js)
    for (const f of shown(chats()).filter((x) => !isThread(x)).sort(newestChat)) {
      if (!own(f)) shared.push(f);
      else {
        const lead = agentsOf(f.name).find((id) => ids.has(id)) ?? first;
        if (lead === undefined) loose.push(f);
        else sections.get(lead).chats.push(f);
      }
    }
    const seen = [];
    const now = Date.now();
    let next = Infinity;
    const rowOf = (f) => {
      const theirs = agentsOf(f.name).filter((id) => ids.has(id));
      const doing = doingIn(f, theirs);
      seen.push(...doing);
      const [row, until] = chatRow(f, doing[0], now);
      next = Math.min(next, until);
      return row;
    };
    const rows = [];
    for (const [key, { agent, chats: list }] of sections) {
      const doing = agent.identity ? chats().flatMap((f) => doingIn(f, [agent.identity])).sort(pressing) : [];
      seen.push(...doing);
      rows.push(hero(agent, doing[0]), ...sectionRows(key, list, rowOf));
    }
    rows.push(...sectionRows("own", loose, rowOf));
    if (shared.length) {
      const head = el("div", "section-head", "Shared");
      head.dataset.key = "head:shared";
      rows.push(head, ...sectionRows("shared", shared, rowOf));
    }
    if (!rows.some((r) => r.dataset.key.startsWith("chat:"))) rows.push(el("div", "empty-row", chats().length ? "Every chat is archived" : isGuest() ? "Chats shared with you show here" : "Your chats will appear here"));
    doingAgain(seen);
    agoAgain(next);
    patch($("chats"), rows);
    renderThreads();
  }

  // ---- invites: shared by email, and theirs only once they accept (decision 63) ----
  // The invites waiting on the emails the person signs in with, above their
  // chats: what each shares, who shared it, and as what, with Accept and
  // Decline. Read as the page starts, as its list's socket opens, and when
  // the socket says they changed (`invited`: one made, renewed or taken
  // back). Accepted, it is in their list, and opens; declined, it goes.
  const invitesRead = { asked: 0, shown: 0 };
  async function loadInvites() {
    const mine = ++invitesRead.asked;
    // failed, what was read before stays: a failure is not "none"
    const got = await api("GET", "/api/invites").catch(() => null);
    if (!got || mine < invitesRead.shown) return;
    invitesRead.shown = mine;
    setInvites(got.invites ?? []);
    renderInvites();
  }
  function renderInvites() {
    const rows = invites().map((i) => {
      const box = el("div", "invite");
      box.dataset.key = `invite:${i.fragment}`;
      box.dataset.invite = i.fragment;
      const may = i.role === "editor" ? "edit" : "view";
      box.append(el("strong", "invite-title", i.title), el("span", "invite-from", `${i.from ?? "Someone"} invited you to ${may} it`));
      const actions = el("div", "invite-actions");
      actions.append(inviteButton(i, "accept", "Accept"), inviteButton(i, "decline", "Decline"));
      box.append(actions);
      return box;
    });
    patch($("invites"), rows);
    $("invites").hidden = !rows.length;
  }
  // What it shares opens as its kind does: a chat in the middle, else a window.
  const openShared = (name) => (byName(name)?.kind === "chat" ? openChat(name) : openApp(name));
  function inviteButton(invite, verb, text) {
    const b = el("button", `invite-${verb}`, text);
    b.type = "button";
    b.onclick = async () => {
      const both = [...b.parentElement.children];
      for (const x of both) x.disabled = true;
      try {
        await api("POST", `/api/invites/${seg(invite.fragment)}/${verb}`, {});
        await Promise.all([load(), loadInvites()]);
        if (verb === "accept") { openShared(invite.fragment); leaveSidebar(); }
      } catch (e) {
        // gone (taken back, answered in another tab, run out): read again, and
        // one theirs already (accepted elsewhere) opens
        await Promise.all([load(), loadInvites()]).catch(() => {});
        if (e.status === 404 && verb === "accept" && byName(invite.fragment)) return openShared(invite.fragment);
        const gone = e.status === 404 ? "It isn't open any more: it was taken back, answered, or ran out." : "Try again in a moment.";
        notice(verb === "accept" ? "The invite wasn't accepted" : "The invite wasn't declined", gone);
        for (const x of both) x.disabled = false;
      }
    };
    return b;
  }
  // Match the chat composer's edge reflection, including its 64px falloff.
  // The pointer's reflection on the launcher's button and each agent's hero,
  // as the chat composer's (its 64px falloff): each frame the pointer moved,
  // every box read, then only their custom properties written (no list
  // drawn, no read after a write). A hero whose menu is open lights the
  // menu's edge in its place, over the hero and its menu.
  const sidebarHoverPointer = matchMedia("(hover: hover) and (pointer: fine)");
  let sidebarEdgeFrame = 0;
  let sidebarEdgePointer = null;
  const sidebarEdgeControls = () => $("sidebar").querySelectorAll(".agent-hero, .sidebar-new-chat");
  const expandedEdge = (control) => control.getAttribute("aria-expanded") === "true" && !menu.hidden && menu.classList.contains("agent-menu");
  const edgeLit = new WeakMap();
  function lightEdge(surface, x, y, light) {
    if (!light && !edgeLit.get(surface)) return;
    edgeLit.set(surface, light);
    surface.style.setProperty("--edge-x", `${x}px`);
    surface.style.setProperty("--edge-y", `${y}px`);
    surface.style.setProperty("--edge-light", String(light));
  }
  function clearSidebarEdge() {
    cancelAnimationFrame(sidebarEdgeFrame);
    sidebarEdgeFrame = 0;
    for (const surface of [...sidebarEdgeControls(), menuEdge]) lightEdge(surface, 0, 0, 0);
  }
  function updateSidebarEdge() {
    if (!sidebarEdgePointer || !sidebarHoverPointer.matches) return;
    const controls = [...sidebarEdgeControls()];
    const menuBottom = controls.some(expandedEdge) ? menu.getBoundingClientRect().bottom : 0;
    const boxes = controls.map((control) => {
      const r = control.getBoundingClientRect();
      const open = expandedEdge(control);
      return { surface: open ? menuEdge : control, left: r.left, top: r.top, width: r.width, height: open ? menuBottom - r.top : r.height, radius: open ? 24 : Math.min(r.width, r.height) / 2 };
    });
    for (const { surface, left, top, width, height, radius } of boxes) {
      const x = sidebarEdgePointer.x - left, y = sidebarEdgePointer.y - top;
      const qx = Math.abs(x - width / 2) - width / 2 + radius;
      const qy = Math.abs(y - height / 2) - height / 2 + radius;
      const distance = Math.max(0, Math.hypot(Math.max(qx, 0), Math.max(qy, 0)) + Math.min(Math.max(qx, qy), 0) - radius);
      const proximity = Math.max(0, 1 - distance / 64);
      lightEdge(surface, x, y, proximity * proximity);
    }
  }
  document.addEventListener("pointermove", event => {
    if (!sidebarHoverPointer.matches || event.pointerType === "touch") return;
    sidebarEdgePointer = { x: event.clientX, y: event.clientY };
    if (sidebarEdgeFrame) return;
    sidebarEdgeFrame = requestAnimationFrame(() => {
      sidebarEdgeFrame = 0;
      updateSidebarEdge();
    });
  });
  document.addEventListener("pointerleave", clearSidebarEdge);
  sidebarHoverPointer.addEventListener("change", clearSidebarEdge);
  addEventListener("blur", clearSidebarEdge);
  // The open chat, on screen, is seen (`PUT /api/fragments/{name}/seen`): its
  // row counts nothing unread, here and in the person's other tabs and
  // devices (their list tells them). Asked again whenever the list counts
  // more on it (a reply that came while it was open), never while this page
  // is hidden; one ask at a time.
  let marking = null;
  function markSeen() {
    const f = byName(current());
    if (!f || page() || document.hidden || !f.unread || marking === f.name) return;
    marking = f.name;
    api("PUT", `/api/fragments/${seg(f.name)}/seen`)
      .then(() => {
        f.unread = 0;
        renderChats();
      })
      .catch(() => {})
      .finally(() => { marking = null; });
  }
  document.addEventListener("visibilitychange", markSeen);
  // The open chat's ⋯: its name, its people, its agents' profiles and
  // screens, and archiving. New chats are the launcher's, with the person's
  // main agent (`primaryAgent`), whichever chat is open, and each agent's
  // menu's, with that agent.
  $("chat-options").onclick = () => openChatMenu(current(), $("chat-options"));
  function openChatMenu(name, trigger) {
    const group = isGroup(name);
    const agent = agentOfChat(name);
    // a group's agents each have a profile; a direct chat's agent has one
    const profiles = group
      ? agentsOf(name).map((id) => agents().get(id)?.fragment).filter(Boolean).map((a) => ({ icon: "agent", text: `${titleOf(a)}'s profile`, onClick: () => openApp(a) }))
      : agent ? [{ icon: "agent", text: "Its profile", onClick: () => openApp(agent) }] : [];
    // each agent's own desktop: a group's agents each have one to pick from
    const screens = !computer() ? [] : group
      ? agentsOf(name).map((id) => agents().get(id)?.fragment).filter(Boolean).map((a) => ({ icon: "screen", text: `${titleOf(a)}'s screen`, onClick: () => openScreen(a) }))
      : agent ? [{ icon: "screen", text: "Its screen", onClick: () => openScreen(agent) }] : [];
    openMenu(trigger, [
      { icon: "rename", text: "Rename…", onClick: () => rename(name) },
      shareItem(name),
      ...profiles,
      ...screens,
      archiveItem(name),
    ]);
  }
  const askedAgent = () => askedAgentOf({ open: current() && !page() ? byName(current()) : null, own, agentsOf, mine: ownAgents(), main: primaryAgent() });
  return { renderHeading, renderChats, loadInvites, primaryAgent, newChatAgent, askedAgent, ownAgents, markSeen, edgeAgain: updateSidebarEdge };
}
