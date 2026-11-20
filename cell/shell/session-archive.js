// The person's Session Archive (templates/sessions/README.md): its chats,
// as its rows list them (its app's `sessions`, asked with this page's
// session) and, beside them, any its `index/chat.json` lists that no row
// does (older clients keep that file), which the History page lists with the
// person's chats (history-page.js; Paul, 2026-10-31: in place of a
// sidebar section of their own). One opens in the middle column,
// read-only: the archive's own page for it (`/?s=<key>`), framed as a chat
// is; the archive's page lists the rest, its scheduled runs and side tasks
// folded away. The archive is the one fragment of kind `sessions` the
// person owns; the shell makes none (`fragment session import` does).
//
// The list is read again as the person's list is read (each change it
// tells this page of) at most once a minute, at once when the archive is
// new to this page, and as the page comes back into view: an import moves
// the archive's files and not their list.
// `changed`: the list read again, or the conversation open in it moved.
//
// History's search asks the archive's own search too (`search`), which
// each import writes (templates/sessions/README.md, Search): its chats
// whose words hold the question, best first.
import { KIND } from "./frame-messages.js";

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
    const entries = await chatsOf(name).catch(() => null);
    if (mine !== read.asked || !entries) return;
    read.name = name;
    read.entries = entries;
    changed();
  }

  // the archive's chats: every page of its rows, and any chat of the index
  // file older clients keep (`index/chat.json`) that no row lists yet
  async function chatsOf(name) {
    const out = [];
    for (let after = "", i = 0; i < 64; i++) {
      const page = await api("POST", `/api/f/${seg(name)}/ops/sessions`, { id: `history-${Date.now()}-${i}`, input: { kind: "chat", after } })
        .then((r) => r?.result, (e) => (e.status === 404 && i === 0 ? null : Promise.reject(e)));
      if (!page) break;
      if (!Array.isArray(page.sessions)) throw new Error("the archive answered no list");
      out.push(...page.sessions);
      if (!page.next || page.next <= after) break;
      after = page.next;
    }
    const index = await api("GET", `/api/f/${seg(name)}/file?path=${encodeURIComponent("index/chat.json")}`).catch((e) => (e.status === 404 ? null : Promise.reject(e)));
    const held = new Set(out.map((e) => e.key));
    return [...out, ...(Array.isArray(index?.sessions) ? index.sessions.filter((e) => typeof e?.key === "string" && !held.has(e.key)) : [])];
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
    if (d?.fragment !== KIND.sessionArchive || !read.name || e.source !== getFrame(read.name)?.contentWindow) return;
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
