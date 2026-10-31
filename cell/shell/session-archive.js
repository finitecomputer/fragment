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
// A Continue's history record (docs/chat-records.md, "History"): the
// conversation's title, its summary and the newest of what the person and
// the agent said, oldest first, bounded so its body (its JSON, as the cell
// measures a record's: `limits::RECORD_BODY_MAX_BYTES`, 64 KiB) fits,
// whatever its text is (CJK, escaped).
export const HISTORY_BODY_MAX_BYTES = 63 * 1024;
const HISTORY_LIMITS = { messages: 40, text: 4_000, summary: 8_000, title: 120 };
const bytesOf = (v) => new TextEncoder().encode(JSON.stringify(v)).length;
const cutTo = (text, max) => ([...text].length > max ? `${[...text].slice(0, max - 1).join("")}…` : text);

export function historyOf(c, key, archive) {
  const said = (c.messages ?? []).filter((m) => (m.role === "user" || m.role === "assistant") && typeof m.text === "string" && m.text.trim() && !m.summary && !m.tool_calls?.length);
  const summary = [...(c.messages ?? [])].reverse().find((m) => m.summary && typeof m.text === "string" && m.text.trim())?.text ?? "";
  const base = {
    kind: "history", title: cutTo(c.title || "Untitled", HISTORY_LIMITS.title), archive, key, started_at: c.started_at, ended_at: c.ended_at,
    ...(summary ? { summary: cutTo(summary, HISTORY_LIMITS.summary) } : {}), messages: [], earlier: said.length,
  };
  // a summary too large for the body alone is cut until it fits
  while (base.summary && bytesOf(base) > HISTORY_BODY_MAX_BYTES) base.summary = cutTo(base.summary, Math.floor([...base.summary].length / 2));
  // the newest that fit, oldest first: each adds its JSON and a comma
  let size = bytesOf(base);
  const tail = [];
  for (const m of [...said].reverse()) {
    if (tail.length >= HISTORY_LIMITS.messages) break;
    const one = { role: m.role, at: m.at, text: cutTo(m.text.trim(), HISTORY_LIMITS.text) };
    const more = bytesOf(one) + (tail.length ? 1 : 0);
    if (size + more > HISTORY_BODY_MAX_BYTES) break;
    size += more;
    tail.unshift(one);
  }
  return { ...base, messages: tail, earlier: said.length - tail.length };
}

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
  // there (`historyOf`, bounded before anything is made). The agent joins
  // after it, so it is the chat's first record. The archive keeps the
  // chat's name (`continued/<key>.json`), so the conversation offers that
  // chat again rather than another, and a Continue asked again (a stale
  // tab) opens it. Each step can be taken again: the chat's name is made
  // here and kept before the chat is (`PENDING`), so a try cut short, or
  // whose answer was lost, is finished by the next in the same chat.

  // a chat's title, as the sidebar's room has it
  const TITLE_MAX = 48;
  // a chat's name, made here so a try whose answer was lost is found again
  // by it (decision 47's suffix: `fragment_proto::NAME_SUFFIX_ALPHABET`)
  const SUFFIX = "abcdefghijkmnpqrstuvwxyz23456789";
  const suffix = () => [...crypto.getRandomValues(new Uint8Array(4))].map((b) => SUFFIX[b & 31]).join("");
  // the chat a Continue not yet finished is making, by archive and key
  const PENDING = "shell.archive.continuing.v1";
  const status = (name) => api("GET", `/api/f/${seg(name)}/status`).catch((e) => (e.status === 404 ? null : Promise.reject(e)));
  let continuing = false;
  async function continueSession(key) {
    const agent = primaryAgent();
    const fragment = agents().get(agent?.identity)?.fragment;
    if (continuing || !read.name) return;
    if (!fragment) return notice("No agent yet", "Continue needs your agent.");
    continuing = true;
    const archive = read.name;
    const slot = `${archive} ${key}`;
    const remember = (name) => {
      const all = store.get(PENDING, {});
      if (name) all[slot] = name;
      else delete all[slot];
      store.set(PENDING, all);
    };
    const go = async (name) => {
      remember(null);
      await loadList();
      openChat(name);
      leaveSidebar();
    };
    let made = false;
    try {
      // continued already (in another tab, or by a try whose last answer was
      // lost): its chat, while it is there
      const marked = await api("GET", `/api/f/${seg(archive)}/file?path=${encodeURIComponent(`continued/${key}.json`)}`).catch((e) => (e.status === 404 ? null : Promise.reject(e)));
      if (typeof marked?.chat === "string" && (await status(marked.chat))) return void (await go(marked.chat));
      const c = await api("GET", `/api/f/${seg(archive)}/file?path=${encodeURIComponent(`sessions/${key}.json`)}`);
      // bounded before anything is made
      const body = historyOf(c, key, archive);
      // the chat a try before this one began, or a new one, kept before it is made
      const name = store.get(PENDING, {})[slot] ?? `${chatLabel(fragment)}--${suffix()}`;
      remember(name);
      try {
        await api("POST", "/api/fragments", { name, template: "chat", title: cutTo(c.title || "Untitled", TITLE_MAX) });
      } catch (e) {
        // its answer lost, it may be made: the next try finds it by its name
        if (!(e.status >= 400)) made = true;
        // made by a try whose answer was lost: the person's own
        const mine = e.status === 409 ? (await status(name))?.role === "owner" : false;
        if (!mine) {
          if (e.status === 409) remember(null);
          throw e;
        }
      }
      made = true;
      // posted by a try before (the same id; another body when the conversation was imported again since)
      await api("POST", `/api/f/${seg(name)}/channels/chat`, { id: "history", body }).catch((e) => (e.status === 409 ? null : Promise.reject(e)));
      // adding it wakes its computer (the platform's `joined`)
      await api("PUT", `/api/f/${seg(name)}/members/${seg(agent.identity)}`, { role: "editor" });
      await api("POST", `/api/f/${seg(archive)}/files`, { message: `continued in ${name}`, files: [{ path: `continued/${key}.json`, text: `${JSON.stringify({ chat: name })}\n` }] });
      await go(name);
    } catch (e) {
      if (made) notice("The chat is not ready yet", `${e.message} Continue again to finish it.`);
      else notice("The chat was not made", e.message);
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

  return { load, render, title, continueSession };
}
