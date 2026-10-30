// The person's Session Archive in the sidebar (templates/sessions/README.md):
// a heading below their chats listing the archive's chats, newest first, as
// its chats' index lists them (`index/chat.json`, read with this page's
// session). One opens in the middle column, read-only: the archive's own
// page for it (`/?s=<key>`), framed as a chat is; the archive's page lists
// the rest, its scheduled runs and side tasks folded away. The archive is
// the one fragment of kind `sessions` the person owns; the shell makes
// none (`fragment session import` does).
//
// The index is read again as the person's list is read (each change it
// tells this page of) at most once a minute, at once when the archive is
// new to this page, and as the page comes back into view: an import moves
// the archive's files and not their list.
export function createSessionArchive({
  api, ui: { $, el, patch, renderHeading, notice }, data: { allFragments, own, seg, primaryAgent, agents, chatLabel },
  selection: { current, page, getFrame }, navigation: { openChat, leaveSidebar }, load: loadList, store,
}) {
  // the rows the sidebar shows; the archive's page lists every one
  const SHOWN = 30;
  const READ_EVERY_MS = 60_000;
  const OPEN = "shell.archive.open.v1";
  const read = { name: null, at: 0, asked: 0, entries: [] };
  let open = null;

  const archiveOf = () => allFragments().find((f) => f.kind === "sessions" && own(f))?.name ?? null;

  async function load(force = false) {
    const name = archiveOf();
    if (!name) {
      read.name = null;
      read.entries = [];
      return render();
    }
    if (!force && name === read.name && Date.now() - read.at < READ_EVERY_MS) return render();
    const mine = ++read.asked;
    read.at = Date.now();
    // failed, what was read before stays: a failure is not "none"
    const index = await api("GET", `/api/f/${seg(name)}/file?path=${encodeURIComponent("index/chat.json")}`).catch((e) => (e.status === 404 ? { sessions: [] } : null));
    if (mine !== read.asked || !index) return render();
    read.name = name;
    read.entries = Array.isArray(index.sessions) ? index.sessions : [];
    render();
  }
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden) load().catch(() => {});
  });

  // The archive's page says which conversation it shows (its `?s=`, or none
  // for its list) as it loads, the person moving in it too, answering the
  // theme this page sends each frame: the heading and the row follow.
  // It asks to continue one (`continue`), or to open the chat it was
  // continued in (`chat`): the shell does either, as the person.
  addEventListener("message", (e) => {
    const d = e.data;
    if (d?.fragment !== "session-archive" || !read.name || e.source !== getFrame(read.name)?.contentWindow) return;
    if (typeof d.continue === "string") return void continueSession(d.continue);
    if (typeof d.chat === "string") {
      if (allFragments().some((f) => f.name === d.chat && f.kind === "chat")) { openChat(d.chat); leaveSidebar(); }
      return;
    }
    open = typeof d.key === "string" ? d.key : null;
    renderHeading();
    render();
  });

  // ---- Continue: a conversation of the archive, picked up as a chat ----
  // A new chat with the person's main agent, titled as the conversation was,
  // whose first record is its `history` (docs/chat-records.md, "History"):
  // a summary and the newest of what was said, bounded, which the chat's
  // page shows above the chat and the agent is handed at its first turn
  // there. The agent joins after it, so it is the chat's first record. The
  // archive keeps the chat's name (`continued/<key>.json`), so the
  // conversation offers that chat again rather than another.
  const HISTORY = { messages: 40, text: 4_000, all: 24_000, summary: 8_000, title: 48 };
  const cut = (text, max) => ([...text].length > max ? `${[...text].slice(0, max - 1).join("")}…` : text);
  function history(c, key) {
    const said = (c.messages ?? []).filter((m) => (m.role === "user" || m.role === "assistant") && typeof m.text === "string" && m.text.trim() && !m.summary && !m.tool_calls?.length);
    const summary = [...(c.messages ?? [])].reverse().find((m) => m.summary && typeof m.text === "string" && m.text.trim())?.text ?? "";
    // the newest that fit, oldest first
    const tail = [];
    let room = HISTORY.all;
    for (const m of [...said].reverse()) {
      const text = cut(m.text.trim(), HISTORY.text);
      if (tail.length >= HISTORY.messages || text.length > room) break;
      room -= text.length;
      tail.unshift({ role: m.role, at: m.at, text });
    }
    return {
      kind: "history", title: cut(c.title || "Untitled", 120), archive: read.name, key, started_at: c.started_at, ended_at: c.ended_at,
      ...(summary ? { summary: cut(summary, HISTORY.summary) } : {}), messages: tail, earlier: said.length - tail.length,
    };
  }
  let continuing = false;
  async function continueSession(key) {
    const agent = primaryAgent();
    const fragment = agents().get(agent?.identity)?.fragment;
    if (continuing || !read.name || !fragment) return;
    continuing = true;
    try {
      const c = await api("GET", `/api/f/${seg(read.name)}/file?path=${encodeURIComponent(`sessions/${key}.json`)}`);
      const chat = await api("POST", "/api/fragments", { label: chatLabel(fragment), template: "chat", title: cut(c.title || "Untitled", HISTORY.title) });
      await api("POST", `/api/f/${seg(chat.name)}/channels/chat`, { id: "history", body: history(c, key) });
      // adding it wakes its computer (the platform's `joined`)
      await api("PUT", `/api/f/${seg(chat.name)}/members/${seg(agent.identity)}`, { role: "editor" });
      await api("POST", `/api/f/${seg(read.name)}/files`, { message: `continued in ${chat.name}`, files: [{ path: `continued/${key}.json`, text: `${JSON.stringify({ chat: chat.name })}\n` }] });
      await loadList();
      openChat(chat.name);
      leaveSidebar();
    } catch (e) {
      notice("The chat was not made", e.message);
    } finally {
      continuing = false;
    }
  }

  const shownOpen = () => store.get(OPEN, true) !== false;

  function render() {
    const box = $("archive");
    box.hidden = !read.name || !read.entries.length;
    if (box.hidden) return;
    const toggle = $("archive-toggle");
    toggle.setAttribute("aria-expanded", String(shownOpen()));
    toggle.onclick = () => {
      store.set(OPEN, !shownOpen());
      render();
    };
    $("archive-rows").hidden = !shownOpen();
    const here = current() === read.name && !page();
    const rows = read.entries.slice(0, SHOWN).map((e) => {
      const active = here && open === e.key;
      const row = el("button", `row archive-row${active ? " active" : ""}`);
      row.type = "button";
      row.dataset.key = `archived:${e.key}`;
      row.setAttribute("aria-current", active ? "page" : "false");
      row.append(el("span", "label", e.title || "Untitled"));
      row.onclick = () => { openSession(e.key); leaveSidebar(); };
      return row;
    });
    if (read.entries.length > SHOWN) {
      const all = el("button", "row archive-all");
      all.type = "button";
      all.dataset.key = "archived:all";
      all.append(el("span", "label", `All ${read.entries.length}`));
      all.onclick = () => { openSession(null); leaveSidebar(); };
      rows.push(all);
    }
    patch($("archive-rows"), rows);
  }

  // one conversation (its key), or with none the archive's list
  function openSession(key) {
    if (!read.name) return;
    open = key;
    openChat(read.name, true, key ? `/?s=${encodeURIComponent(key)}` : "/");
  }

  // what the middle column's heading says while the archive is open there
  const title = () => read.entries.find((e) => e.key === open)?.title || "Session Archive";

  return { load, render, title };
}
