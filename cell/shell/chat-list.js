import { avatar } from "./agent-identity.js";
import { icon } from "./icons.js";

// Newest first, with a newly made chat (no preview yet) before older ones.
export const newestChat = (a, b) => (b.previewAt ?? Infinity) - (a.previewAt ?? Infinity) || (a.name < b.name ? -1 : 1);

export function createChatList({ api, ui, data, selection, menus, navigation, listLive, load, invites, setInvites }) {
  const { $, el, patch, agentFace, mark, notice } = ui;
  const { byName, own, chats, shown, titleOf, chatTitle, homeMark, isGroup, agentOfChat, agentsOf, agents, allFragments, computer, isGuest, personId } = data;
  const { current, page, getFrame } = selection;
  const { menuOwner, openMenu, shareItem, archiveItem } = menus;
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
  // When a chat's newest message was said, as its row says it: today's
  // time, then "Yesterday", a weekday within the week, else its date.
  function saidAt(at) {
    const d = new Date(at), now = new Date();
    const day = (x) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
    const days = Math.round((day(now) - day(d)) / 86_400_000);
    const t = el("time", null,
      days <= 0 ? d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })
      : days === 1 ? "Yesterday"
      : days < 7 ? d.toLocaleDateString([], { weekday: "short" })
      : d.toLocaleDateString([], { month: "short", day: "numeric", ...(d.getFullYear() === now.getFullYear() ? {} : { year: "numeric" }) }));
    t.dateTime = d.toISOString();
    return t;
  }
  // A chat's row, its agent first (Paul, 2026-10-30: it reads as talking to
  // agents, not as threads piling up): whose chat it is (its agents' faces,
  // a group's stacked, their names, one agent's role) and what it is doing
  // there, else when its newest message was said; then the chat's own name
  // and its marks. A chat with none of the person's agents (someone
  // else's) is its name alone.
  function chatRow(f, theirs, doing) {
    const active = f.name === current() && !page();
    const row = el("button", `row chat-row${active ? " active" : ""}`);
    row.type = "button";
    row.dataset.key = `chat:${f.name}`;
    row.setAttribute("aria-current", active ? "page" : "false");
    row.onclick = () => { openChat(f.name); leaveSidebar(); };
    const marks = [el("span", "label", chatTitle(f.name)), ...homeMark(f), ...unreadMark(f)];
    if (!theirs.length) {
      row.append(...marks);
      return row;
    }
    const face = el("span", "chat-face");
    face.append(mark(f.name));
    const who = el("span", "chat-who");
    who.append(el("span", "chat-agent", theirs.map((a) => titleOf(a.fragment)).join(", ")));
    // one agent's role (its `meta.role`, on its row in the list), as a tag
    const role = theirs.length === 1 ? byName(theirs[0].fragment)?.agentRole : null;
    if (role) who.append(el("span", "role-tag", role));
    // what it is doing here, while it is, in place of when
    if (doing) who.append(el("span", `agent-doing ${doing.doing}`, DOING_SAYS[doing.doing]));
    else if (f.previewAt) who.append(saidAt(f.previewAt));
    const what = el("span", "chat-what");
    what.append(...marks);
    const lines = el("span", "chat-lines");
    lines.append(who, what);
    row.classList.add("two");
    row.append(face, lines);
    return row;
  }
  // A row's day ("2:14 PM", then "Yesterday") moves at midnight: the
  // sidebar is drawn again then.
  let dayTimer = 0;
  function dayAgain() {
    clearTimeout(dayTimer);
    const now = new Date();
    dayTimer = setTimeout(renderChats, new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1).getTime() - now.getTime() + 1000);
  }
  // Whom a new chat is with unless the person picks another (Paul,
  // 2026-10-30): the open chat's agent (a group's lead) when it is one of
  // theirs, else their main agent.
  function newChatAgent() {
    const mine = ownAgents();
    const open = current() && !page() ? agentsOf(current()).map((id) => mine.find((a) => a.identity === id)).find(Boolean) : null;
    return open ?? primaryAgent();
  }
  // The launcher's key, as the person's keyboard names it.
  const LAUNCHER_KEY = /Mac|iPhone|iPad|iPod/.test(navigator.userAgentData?.platform ?? navigator.platform ?? "") ? "⌘K" : "Ctrl K";
  // "New chat with" and the agent it is with (`newChatAgent`), its chip;
  // with more than one agent, the chip lists them all, and the one picked
  // gets that chat alone. A guest makes no chat.
  function renderStart() {
    const agent = newChatAgent();
    const start = $("sidebar-start-chat");
    start.hidden = !agent?.identity || isGuest();
    $("sidebar-start").hidden = start.hidden;
    if (start.hidden) return patch($("new-chat-agent"), []);
    const name = titleOf(agent.fragment);
    start.setAttribute("aria-label", `New chat with ${name}`);
    start.onclick = () => newChat(agent.identity);
    const many = ownAgents().length > 1;
    const chip = el(many ? "button" : "span", "agent-pick");
    chip.dataset.key = `${agent.fragment} ${many}`;
    chip.append(avatar(agentFace(agent.fragment, agent.identity)), el("span", "name", name));
    if (many) {
      chip.type = "button";
      chip.setAttribute("aria-label", `${name}: pick another agent`);
      chip.setAttribute("aria-haspopup", "menu");
      chip.setAttribute("aria-controls", "menu");
      const shown = $("new-chat-agent").firstElementChild;
      chip.setAttribute("aria-expanded", String(!!shown && menuOwner()?.button === shown));
      chip.append(icon("caret"));
      chip.onclick = () => openMenu(chip, ownAgents().map((a) => ({
        face: avatar(agentFace(a.fragment, a.identity)),
        text: titleOf(a.fragment),
        tag: byName(a.fragment)?.agentRole,
        current: a.identity === newChatAgent()?.identity,
        onClick: () => newChat(a.identity),
      })));
    }
    patch($("new-chat-agent"), [chip]);
  }
  function renderChats() {
    renderStart();
    // the launcher: a guest's searches; a seat's also starts a chat
    const launcherLabel = isGuest() ? "Search chats and apps" : "Search or start a new chat";
    $("launcher-key").textContent = LAUNCHER_KEY;
    for (const id of ["sidebar-new-chat", "new-chat"]) {
      $(id).title = `${launcherLabel} (${LAUNCHER_KEY})`;
      $(id).dataset.tooltip = `${launcherLabel} (${LAUNCHER_KEY})`;
      $(id).setAttribute("aria-label", launcherLabel);
    }
    const mine = new Map(ownAgents().map((a) => [a.identity, a]));
    const seen = [];
    const rows = shown(chats()).sort(newestChat).map((f) => {
      const theirs = agentsOf(f.name).map((id) => mine.get(id)).filter(Boolean);
      const doing = doingIn(f, theirs.map((a) => a.identity));
      seen.push(...doing);
      return chatRow(f, theirs, doing[0]);
    });
    doingAgain(seen);
    dayAgain();
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
  // Match the chat composer's edge reflection, including its 64px falloff.
  const sidebarHoverPointer = matchMedia("(hover: hover) and (pointer: fine)");
  let sidebarEdgeFrame = 0;
  let sidebarEdgePointer = null;
  const sidebarEdgeControls = () => $("sidebar").querySelectorAll(".sidebar-new-chat");
  function clearSidebarEdge() {
    cancelAnimationFrame(sidebarEdgeFrame);
    sidebarEdgeFrame = 0;
    for (const control of sidebarEdgeControls()) control.style.setProperty("--edge-light", "0");
  }
  function updateSidebarEdge() {
    if (!sidebarEdgePointer || !sidebarHoverPointer.matches) return;
    for (const control of sidebarEdgeControls()) {
      const rect = control.getBoundingClientRect();
      const x = sidebarEdgePointer.x - rect.left, y = sidebarEdgePointer.y - rect.top;
      const radius = Math.min(rect.width / 2, rect.height / 2, parseFloat(getComputedStyle(control).borderTopLeftRadius));
      const qx = Math.abs(x - rect.width / 2) - rect.width / 2 + radius;
      const qy = Math.abs(y - rect.height / 2) - rect.height / 2 + radius;
      const distance = Math.max(0, Math.hypot(Math.max(qx, 0), Math.max(qy, 0)) + Math.min(Math.max(qx, qy), 0) - radius);
      const proximity = Math.max(0, 1 - distance / 64);
      control.style.setProperty("--edge-x", `${x}px`);
      control.style.setProperty("--edge-y", `${y}px`);
      control.style.setProperty("--edge-light", String(proximity * proximity));
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
  return { renderHeading, renderChats, loadInvites, primaryAgent, newChatAgent, markSeen };
}
