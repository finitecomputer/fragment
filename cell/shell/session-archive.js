// The person's Session Archive (templates/sessions/README.md): its chats,
// newest first, as its chats' index lists them (`index/chat.json`, read
// with this page's session), which the History page lists with the
// person's chats (history-page.js; Paul, 2026-10-31: in place of a
// sidebar section of their own). One opens in the middle column,
// read-only: the archive's own page for it (`/?s=<key>`), framed as a chat
// is; the archive's page lists the rest, its scheduled runs and side tasks
// folded away. The archive is the one fragment of kind `sessions` the
// person owns; the shell makes none (`fragment session import` does).
//
// The index is read again as the person's list is read (each change it
// tells this page of) at most once a minute, at once when the archive is
// new to this page, and as the page comes back into view: an import moves
// the archive's files and not their list.
// `changed`: the index read again, or the conversation open in it moved.
//
// History's search asks the archive's own search too (`search`), which
// each import writes (templates/sessions/README.md, Search): its chats
// whose words hold the question, best first.

// Results a search asks the archive for (the API's most: RESULTS_MAX)
const SEARCH_LIMIT = 50;
const SESSION_SOURCE = /^sessions\/([^/]+)\.json$/;

// A search's results as the archive's conversations, best first: each
// once, its key from its source (`sessions/<key>.json`) and its best
// snippet; a result of any other source left out.
export function foundSessions(results) {
  const found = new Map();
  for (const r of results ?? []) {
    const key = typeof r?.source === "string" ? SESSION_SOURCE.exec(r.source)?.[1] : undefined;
    if (key && !found.has(key)) found.set(key, typeof r.snippet === "string" ? r.snippet : "");
  }
  return [...found].map(([key, snippet]) => ({ key, snippet }));
}

export function createSessionArchive({
  api, ui: { renderHeading }, data: { allFragments, own, seg },
  selection: { getFrame }, navigation: { openChat, leaveSidebar }, continuing, store, changed,
}) {
  const READ_EVERY_MS = 60_000;
  const read = { name: null, at: 0, asked: 0, entries: [] };
  let open = null;

  const archiveOf = () => allFragments().find((f) => f.kind === "sessions" && own(f))?.name ?? null;

  async function load(force = false) {
    const name = archiveOf();
    if (!name) {
      read.name = null;
      read.entries = [];
      return changed();
    }
    if (!force && name === read.name && Date.now() - read.at < READ_EVERY_MS) return;
    const mine = ++read.asked;
    read.at = Date.now();
    // failed, what was read before stays: a failure is not "none"
    const index = await api("GET", `/api/f/${seg(name)}/file?path=${encodeURIComponent("index/chat.json")}`).catch((e) => (e.status === 404 ? { sessions: [] } : null));
    if (mine !== read.asked || !index) return;
    read.name = name;
    read.entries = Array.isArray(index.sessions) ? index.sessions : [];
    changed();
  }
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden) load().catch(() => {});
  });

  // The archive's page says which conversation it shows (its `?s=`, or none
  // for its list) as it loads, the person moving in it too, answering the
  // theme this page sends each frame: the heading follows.
  // It asks to continue one (`continue`): the shell asks with which agent
  // (continue.js), as the person.
  addEventListener("message", (e) => {
    const d = e.data;
    if (d?.fragment !== "session-archive" || !read.name || e.source !== getFrame(read.name)?.contentWindow) return;
    if (typeof d.continue === "string") return void continuing.ask({ archive: read.name, key: d.continue }, read.entries.find((x) => x.key === d.continue)?.title);
    open = typeof d.key === "string" ? d.key : null;
    renderHeading();
  });

  // its chats whose words hold `q`, as `foundSessions` says them; none
  // without an archive, or on a host without search (404). Any other
  // failure is the caller's to say. By their words alone (`lexical`), as
  // History filters the person's own chats: a search by meaning answers
  // its nearest whatever they hold, so it would filter out none.
  async function search(q) {
    const name = read.name;
    if (!name) return [];
    const found = await api("POST", `/api/f/${seg(name)}/search`, { q, limit: SEARCH_LIMIT, mode: "lexical", fields: { kind: "chat" } }).catch((e) => (e.status === 404 ? null : Promise.reject(e)));
    return foundSessions(found?.results);
  }

  // one conversation (its key), or with none the archive's list
  function openSession(key) {
    if (!read.name) return;
    open = key;
    openChat(read.name, true, key ? `/?s=${encodeURIComponent(key)}` : "/");
  }

  // what the middle column's heading says while the archive is open there
  const title = () => read.entries.find((e) => e.key === open)?.title || "Session Archive";

  return { load, title, archive: () => read.name, entries: () => read.entries, openSession, search };
}
