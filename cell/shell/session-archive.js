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
export function createSessionArchive({ api, ui: { $, el, patch, renderHeading }, data: { allFragments, own, seg }, selection: { current, page, getFrame }, navigation: { openChat, leaveSidebar }, store }) {
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
  addEventListener("message", (e) => {
    if (e.data?.fragment !== "session-archive" || !read.name || e.source !== getFrame(read.name)?.contentWindow) return;
    open = typeof e.data.key === "string" ? e.data.key : null;
    renderHeading();
    render();
  });

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
