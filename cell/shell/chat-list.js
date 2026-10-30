import { avatar } from "./agent-identity.js";

export function createChatList({ api, ui, data, selection, menus, navigation, listLive, load, invites, setInvites }) {
  const { $, el, patch, agentFace, notice } = ui;
  const { byName, own, chats, shown, titleOf, chatTitle, homeMark, isGroup, agentOfChat, agentsOf, agents, allFragments, computer, isGuest, personId } = data;
  const { current, page, getFrame } = selection;
  const { menu, menuEdge, menuOwner, openMenu, shareItem, archiveItem } = menus;
  const { newChat, rename, openChat, openApp, openScreen, leaveSidebar, seg } = navigation;
  // ---- chats: one flat list, newest first, the open one in the middle column ----
  function renderHeading() {
    const open = current() && byName(current()) && !page();
    $("chat-heading").hidden = !open;
    $("page-title").hidden = !page();
    $("page-title").textContent = page() ?? "";
    $("new-chat").hidden = !open || isGuest() || !primaryAgent()?.identity;
    $("chat-options").hidden = !open;
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
  // The person's one agent (Paul, 2026-10-13: "just one", a setting later):
  // their first, the one the first run makes (`defaultAgent`): the first
  // their computer took (its view lists its agents in the order it took
  // them), or, before it took one, their agent fragment. New chats are with
  // it. Any other agents they have stay out of the sidebar; their chats are
  // chats, so they list and search as any other.
  function primaryAgent() {
    const first = [...agents().values()].find((a) => byName(a.fragment)?.role === "owner");
    if (first) return { fragment: first.fragment, identity: first.identity };
    const f = allFragments().find((x) => x.kind === "agent" && own(x));
    return f ? { fragment: f.name, identity: null } : null;
  }
  // What the person's agents are doing, from their chats' rows in the list
  // (docs/api.md, "Listed records"): each agent's newest record of a turn's
  // kind on a chat's `work` (docs/chat-records.md), which the shell reads as
  // the chat's page reads a turn. Started, a step, or a card answered is
  // working; a card open waits for the person (its owner answers its cards);
  // a question waits on whoever it asks (`asks`); an end is nothing. A
  // message waiting for its turn has no record yet, so queued is not said.
  // None while their computer is not awake (its turns stop with it) or this
  // page's list socket is shut (it would not hear the change), and none past
  // the bridge's own bound on a turn that says nothing (15 minutes; a turn
  // waiting on someone waits as long as a card, an hour), each a minute more
  // for the list's own lag. Those three are copies, named as their sources
  // name them (proto's limits; images/bridge/src/limits.rs), each held to its
  // source by a host test (xtask/src/guards.rs, `the_shell_copies_its_limits`).
  // The list's turn kinds are the chat's records (docs/chat-records.md):
  // the shell reads them as the bridge writes them. A listed field's
  // value is any text an agent wrote (up to 128 characters): `asks` is only
  // compared with the person's id here, never shown; what is shown is these
  // words and the chat's title, as text (`el`'s textContent), never markup.
  const DOING_SAYS = { you: "Waiting for you", waiting: "Waiting", working: "Working" };
  const DOING_ORDER = Object.keys(DOING_SAYS);
  const DOING_OF_KIND = { "turn.start": "working", "turn.step": "working", "turn.prompt.closed": "working", "turn.prompt": "you", "turn.asked": "asked" };
  const LISTED_FRESH_MS = 60_000;
  const TURN_IDLE_MS_MAX = 15 * 60_000;
  const PROMPT_TTL_MS_DEFAULT = 60 * 60_000;
  const DOING_QUIET_MS = { working: TURN_IDLE_MS_MAX + LISTED_FRESH_MS, waiting: PROMPT_TTL_MS_DEFAULT + LISTED_FRESH_MS };
  // the chats its menu names, at most
  const DOING_LISTED = 5;
  // the list socket (`watchList`, below): declared here, as the sidebar reads it

  // The chats where agent `identity` is doing something, the most pressing
  // first (waiting for the person, then on someone, then working), the
  // newest first within each; each with when it stops counting (`until`).
  function doingChats(identity) {
    if (!identity || !listLive() || computer()?.phase !== "awake") return [];
    const now = Date.now();
    const out = [];
    for (const f of chats()) {
      for (const r of f.listed ?? []) {
        const kind = r.channel === "work" && r.principal === identity ? DOING_OF_KIND[r.kind] : undefined;
        if (!kind) continue;
        const doing = kind === "asked" ? (r.fields?.asks === personId() ? "you" : "waiting") : kind;
        const until = r.at + DOING_QUIET_MS[doing === "working" ? "working" : "waiting"];
        if (until > now) out.push({ chat: f.name, doing, at: r.at, until });
      }
    }
    return out.sort((x, y) => DOING_ORDER.indexOf(x.doing) - DOING_ORDER.indexOf(y.doing) || y.at - x.at);
  }
  // The sidebar says it again as the first of what it shows stops counting.
  let doingTimer = 0;
  function doingAgain(doing) {
    clearTimeout(doingTimer);
    const next = Math.min(...doing.map((x) => x.until));
    if (Number.isFinite(next)) doingTimer = setTimeout(renderChats, Math.max(1000, next - Date.now()));
  }
  // Newest first: a chat by when its newest message was said (its row's
  // `previewAt`); one with nothing said yet (just made) above them all.
  const newest = (a, b) => (b.previewAt ?? Infinity) - (a.previewAt ?? Infinity) || (a.name < b.name ? -1 : 1);
  function renderChats() {
    const primary = primaryAgent();
    const agentRows = [];
    if (primary) {
      const doing = doingChats(primary.identity);
      doingAgain(doing);
      const says = DOING_SAYS[doing[0]?.doing];
      const row = el("button", "agent-heading sidebar-agent");
      row.id = "sidebar-agent";
      row.type = "button";
      row.dataset.key = primary.fragment;
      row.setAttribute("aria-label", `${titleOf(primary.fragment)}${says ? `, ${says.toLowerCase()}` : ""}: your agent's menu`);
      row.setAttribute("aria-haspopup", "menu");
      row.setAttribute("aria-controls", "menu");
      row.setAttribute("aria-expanded", String(!!menuOwner() && menuOwner().button === $("sidebar-agent")));
      const face = el("span", "sidebar-agent-mark");
      face.append(avatar(agentFace(primary.fragment, primary.identity), "small"));
      // its name, and under it what it is doing, so the name keeps its room
      const named = el("span", "sidebar-agent-named");
      named.append(el("strong", "title", titleOf(primary.fragment)));
      if (says) named.append(el("span", `agent-doing ${doing[0].doing}`, says));
      row.append(face, named);
      row.onclick = () => openPrimaryMenu(row);
      agentRows.push(row);
    }
    patch($("primary-agent"), agentRows);
    // another chat with it, one click from the top (its menu's New chat too)
    const startChat = $("sidebar-start-chat");
    startChat.hidden = !primary?.identity || isGuest();
    startChat.onclick = () => newChat(primary.identity);
    // the launcher: a guest's searches; a seat's also starts a chat
    const launcherLabel = isGuest() ? "Search chats and apps" : "Search or start a new chat";
    for (const id of ["sidebar-new-chat", "new-chat"]) {
      $(id).title = `${launcherLabel} (⌘K)`;
      $(id).dataset.tooltip = `${launcherLabel} (⌘K)`;
      $(id).setAttribute("aria-label", launcherLabel);
    }
    const rows = shown(chats()).sort(newest).map((f) => {
      const active = f.name === current() && !page();
      const row = el("button", `row chat-row${active ? " active" : ""}`);
      row.type = "button";
      row.dataset.key = `chat:${f.name}`;
      row.setAttribute("aria-current", active ? "page" : "false");
      row.append(el("span", "label", chatTitle(f.name)), ...homeMark(f), ...unreadMark(f));
      row.onclick = () => { openChat(f.name); leaveSidebar(); };
      return row;
    });
    patch($("chats"), rows.length ? rows : [el("div", "empty-row", chats().length ? "Every chat is archived" : isGuest() ? "Chats shared with you show here" : "Your chats will appear here")]);
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
  // The agent's own menu: where it is doing something (each chat opens),
  // another chat with it, its name, its profile and its screen. A chat's own
  // (Rename, Invite, Archive) is the chat's ⋯. What it is doing is read as
  // it opens, not from when its row was drawn.
  function openPrimaryMenu(trigger) {
    const primary = primaryAgent();
    if (!primary) return;
    openMenu(trigger, [
      ...doingChats(primary.identity)
        .slice(0, DOING_LISTED)
        .map((x) => ({ icon: "agent", text: `${DOING_SAYS[x.doing]} in ${titleOf(x.chat)}`, onClick: () => { openChat(x.chat); leaveSidebar(); } })),
      ...(primary.identity && !isGuest() ? [{ icon: "newChat", text: "New chat", onClick: () => newChat(primary.identity) }] : []),
      { icon: "rename", text: "Rename…", onClick: () => rename(primary.fragment) },
      { icon: "agent", text: "Its profile", onClick: () => openApp(primary.fragment) },
      ...(computer() ? [{ icon: "screen", text: "Its screen", onClick: () => openScreen(primary.fragment) }] : []),
    ]);
  }
  // Match the chat composer's edge reflection, including its 64px falloff.
  // Follow the pointer across the sidebar and its fixed-position agent menu.
  const sidebarHoverPointer = matchMedia("(hover: hover) and (pointer: fine)");
  let sidebarEdgeFrame = 0;
  let sidebarEdgePointer = null;
  const sidebarEdgeControls = () => $("sidebar").querySelectorAll(".sidebar-agent, .sidebar-new-chat");
  // Keep transient styles off the rendered pill so roster updates preserve its node.
  const expandedAgentEdge = control => control.id === "sidebar-agent" && !menu.hidden && menu.classList.contains("agent-menu");
  const sidebarEdgeSurface = control => expandedAgentEdge(control) ? menuEdge : control.id === "sidebar-agent" ? $("primary-agent") : control;
  function clearSidebarEdge() {
    cancelAnimationFrame(sidebarEdgeFrame);
    sidebarEdgeFrame = 0;
    for (const control of sidebarEdgeControls()) sidebarEdgeSurface(control).style.setProperty("--edge-light", "0");
  }
  function updateSidebarEdge() {
    if (!sidebarEdgePointer || !sidebarHoverPointer.matches) return;
    for (const control of sidebarEdgeControls()) {
      const bounds = control.getBoundingClientRect();
      const rect = { left: bounds.left, top: bounds.top, width: bounds.width, height: expandedAgentEdge(control) ? menu.getBoundingClientRect().bottom - bounds.top : bounds.height };
      const x = sidebarEdgePointer.x - rect.left, y = sidebarEdgePointer.y - rect.top;
      const radius = Math.min(rect.width / 2, rect.height / 2, parseFloat(getComputedStyle(control).borderTopLeftRadius));
      const qx = Math.abs(x - rect.width / 2) - rect.width / 2 + radius;
      const qy = Math.abs(y - rect.height / 2) - rect.height / 2 + radius;
      const distance = Math.max(0, Math.hypot(Math.max(qx, 0), Math.max(qy, 0)) + Math.min(Math.max(qx, qy), 0) - radius);
      const proximity = Math.max(0, 1 - distance / 64);
      const surface = sidebarEdgeSurface(control);
      surface.style.setProperty("--edge-x", `${x}px`);
      surface.style.setProperty("--edge-y", `${y}px`);
      surface.style.setProperty("--edge-light", String(proximity * proximity));
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
  // one agent (`primaryAgent`), whichever chat is open.
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
  return { renderHeading, renderChats, loadInvites, primaryAgent, markSeen, updateSidebarEdge };
}
