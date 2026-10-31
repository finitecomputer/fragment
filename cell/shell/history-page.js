// The History page (`/history`; Paul, 2026-10-31): every chat the person
// has had, the newest first, with room to search them, so the sidebar can
// stay short ("just like gmail makes it low cost to archive a thread
// because you know you can always search it"). It lists:
// - their chats, live and archived (the list's `archived`), and those
//   shared with them; a delegation thread's chat is its thread's line
//   (threads.js), not a chat of its own here, and a chat nothing was ever
//   said in is no history (its first record, below);
// - their Session Archive's conversations (session-archive.js: its
//   index), each opening read-only as the archive's page shows it.
// A row says its title, whom it was with (its agent's face and name;
// "Imported" for the archive's), when it was last said, and "Archived" on
// the ones the person archived. All, Archived and Imported filter it.
// A chat whose list row says nothing was said there yet (no `previewAt`:
// a continued chat holds its history card alone until its agent speaks)
// has its first record read, once while its row is the same: a history
// card gives its time and its newest words, a message its time and first
// line, and none leaves it out. Until it is read it is not listed: no
// row shows without its time, nor sorts as the newest for want of one.
// The search box filters by title as it is typed, and asks the person's
// search (`GET /api/search`) for the messages that hold its words: each
// chat found there once, with its newest snippet under it. The archive's
// conversations are matched by their titles and first words alone until
// the archive is searched too (#774), through `h.archive.search` (q →
// [{key, snippet}]) once it has one.
// Opening a row opens it in the middle column; a message the person sends
// in a chat they archived brings it back to the sidebar (shell.js, `said`).
// Archived and imported rows end with `h.continueControl(source)` once it
// is given (continue.js: Continue with an agent of theirs).
//
// The shell owns the page's place and its helpers (`h`): `api`, `el`,
// `icon`, `seg`, `mark` (a chat's face), `chats`, `isThread`, `own`, `chatTitle`,
// `agentsOf`, `agents` (the person's own, by identity), `titleOf`,
// `archive` ({name, entries, openSession}), `openChat`, `leaveSidebar` and
// `continueControl`.

// The search API's hint: eight words, 256 bytes at most (launcher.js's).
const HINT_WORDS = 8;
const HINT_MAX_BYTES = 256;
// how long typing rests before the person's search is asked
const SEARCH_WAIT_MS = 250;
// the rows drawn at first, and again with each Show more
const ROWS_STEP = 100;
// chats whose first record is read at once, and at most per drawing
const FIRSTS_AT_ONCE = 4;
const FIRSTS_MAX = 32;
const FILTERS = [["all", "All"], ["archived", "Archived"], ["imported", "Imported"]];

const wordsOf = (q) => q.toLowerCase().split(/\s+/).filter(Boolean);
const holds = (text, words) => words.every((w) => text.toLowerCase().includes(w));
export function hintOf(q) {
  const hint = [...q.trim().split(/\s+/).slice(0, HINT_WORDS).join(" ")];
  while (new TextEncoder().encode(hint.join("")).length > HINT_MAX_BYTES) hint.pop();
  return hint.join("");
}
// An imported conversation's time: when it was last said, else begun
// (the archive's UTC times; README's "Times are UTC").
const importedAt = (e) => Date.parse(e.ended_at ?? e.started_at ?? "") || 0;
const firstLine = (text) => (typeof text === "string" ? text.split("\n").map((l) => l.trim()).find(Boolean) ?? "" : "");
// What a chat's first record says of it (docs/chat-records.md): a history
// card its time and its newest words (else its summary's), a message its
// time and its first line; anything else its time alone.
export function firstOf(record) {
  if (!record) return null;
  const b = record.body;
  if (b?.kind === "history") return { at: record.at, line: firstLine(b.messages?.at(-1)?.text) || firstLine(b.summary) };
  return { at: record.at, line: firstLine(typeof b === "string" ? b : b?.text) };
}
// When a row was last said, as History says it: the time today, else the
// day, with its year when it is not this one.
export function when(at, now = Date.now()) {
  if (!at) return "";
  const d = new Date(at), today = new Date(now);
  if (d.toDateString() === today.toDateString()) return d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  return d.toLocaleDateString([], { month: "short", day: "numeric", ...(d.getFullYear() === today.getFullYear() ? {} : { year: "numeric" }) });
}
// The rows, the newest first: chats (`kind: "chat"`) and imported
// conversations (`kind: "imported"`), filtered, and with words, those whose
// title holds them all (an import's first words too) or whose messages
// the search found (`found`: chat name → snippet; `foundImported`: key →
// snippet).
export function historyRows({ chats, entries, filter = "all", q = "", found = new Map(), foundImported = new Map() }) {
  const words = wordsOf(q);
  const rows = [];
  if (filter !== "imported") {
    for (const c of chats) {
      if (filter === "archived" && !c.archived) continue;
      const snippet = found.get(c.name);
      if (words.length && !holds(c.title, words) && snippet === undefined) continue;
      rows.push({ kind: "chat", key: `chat:${c.name}`, at: c.at, ...c, snippet });
    }
  }
  if (filter !== "archived") {
    for (const e of entries) {
      const snippet = foundImported.get(e.key);
      if (words.length && !holds(`${e.title ?? ""}\n${e.preview ?? ""}`, words) && snippet === undefined) continue;
      rows.push({ kind: "imported", key: `imported:${e.key}`, at: importedAt(e), entry: e, title: e.title || "Untitled", snippet });
    }
  }
  // one that says no time last, never the newest
  return rows.sort((a, b) => (b.at || 0) - (a.at || 0) || (a.key < b.key ? -1 : 1));
}

export function createHistoryPage(page, h) {
  const { el, icon } = h;
  let shown = false;
  let filter = "all";
  let q = "";
  let limit = ROWS_STEP;
  // the person's search for `q`: what it found, or why it failed
  const search = { q: "", asked: 0, timer: 0, found: new Map(), foundImported: new Map(), failed: "" };
  // each row's Continue, kept across drawings (what it is doing is its own)
  const controls = new Map();
  // a chat with nothing said yet: its first record as read, by name, with
  // its list row as it was then (`row`), read again once that changes
  // (`first`: {at, line}; null for none; undefined while it is read)
  const firsts = new Map();
  let reading = 0;
  let parts = null;

  function build() {
    const wrap = el("div", "history-layout");
    const head = el("header", "history-intro");
    head.append(el("h1", null, "History"));
    const bar = el("label", "history-search");
    const input = el("input");
    Object.assign(input, { type: "search", placeholder: "Search chats", autocomplete: "off", spellcheck: false, maxLength: 256, value: q });
    input.setAttribute("aria-label", "Search chats");
    input.oninput = () => {
      q = input.value;
      limit = ROWS_STEP;
      render();
      ask();
    };
    input.onkeydown = (e) => {
      if (e.key === "Escape" && input.value) {
        e.stopPropagation();
        input.value = "";
        input.oninput();
      }
    };
    bar.append(icon("search"), input);
    const chips = el("div", "history-filters");
    chips.setAttribute("role", "group");
    chips.setAttribute("aria-label", "Show");
    for (const [key, text] of FILTERS) {
      const chip = el("button", "history-filter", text);
      chip.type = "button";
      chip.dataset.filter = key;
      chip.onclick = () => {
        filter = key;
        limit = ROWS_STEP;
        render();
      };
      chips.append(chip);
    }
    const status = el("p", "history-status");
    status.setAttribute("role", "status");
    const list = el("div", "history-list");
    const more = el("button", "history-more", "Show more");
    more.type = "button";
    more.onclick = () => {
      limit += ROWS_STEP;
      render();
    };
    head.append(bar, chips);
    wrap.append(head, status, list, more);
    page.replaceChildren(wrap);
    parts = { input, chips, status, list, more };
  }

  // The person's search, once typing rests: a slower answer to an older
  // query never replaces a newer one's.
  function ask() {
    clearTimeout(search.timer);
    const hint = hintOf(q);
    if (!hint) {
      Object.assign(search, { q: "", found: new Map(), foundImported: new Map(), failed: "" });
      return;
    }
    if (hint === search.q) return;
    search.timer = setTimeout(async () => {
      const mine = ++search.asked;
      try {
        const [answer, imported] = await Promise.all([
          h.api("GET", `/api/search?q=${encodeURIComponent(hint)}`),
          h.archive.search ? h.archive.search(hint) : [],
        ]);
        if (mine !== search.asked) return;
        // each chat's newest snippet (the answer is the newest first)
        const found = new Map();
        for (const m of answer.messages ?? []) if (!found.has(m.fragment)) found.set(m.fragment, m.snippet);
        const foundImported = new Map();
        for (const m of imported) if (!foundImported.has(m.key)) foundImported.set(m.key, m.snippet);
        Object.assign(search, { q: hint, found, foundImported, failed: "" });
      } catch (e) {
        if (mine !== search.asked) return;
        Object.assign(search, { q: hint, found: new Map(), foundImported: new Map(), failed: e.message });
      }
      render();
    }, SEARCH_WAIT_MS);
  }

  // The first records of the chats that need them, a few at a time; each
  // answer draws the page again. One that failed is read again only once
  // its row changes, or the page opens again.
  function readFirsts(list) {
    for (const f of list) {
      if (reading >= FIRSTS_AT_ONCE) return;
      const row = JSON.stringify(f);
      if (firsts.get(f.name)?.row === row) continue;
      firsts.set(f.name, { row, first: undefined });
      reading++;
      h.api("GET", `/api/f/${h.seg(f.name)}/channels/chat?after=0&limit=1`)
        .then((r) => { if (firsts.get(f.name)?.row === row) firsts.set(f.name, { row, first: firstOf(r.records?.[0]) }); })
        .catch(() => { if (firsts.get(f.name)?.row === row) firsts.set(f.name, { row, first: null, failed: true }); })
        .finally(() => {
          reading--;
          render();
        });
    }
  }

  // whom a chat was with: its lead agent's name, when it is one of the
  // person's own; a chat shared with them says so
  function withWhom(f) {
    const lead = h.agentsOf(f.name)[0];
    const agent = lead && h.agents().get(lead);
    if (agent) return h.titleOf(agent.fragment);
    return h.own(f) ? "" : "Shared";
  }

  function row(r) {
    const item = el("div", "history-row");
    item.dataset.key = r.key;
    item.dataset.kind = r.kind;
    const open = el("button", "history-open");
    open.type = "button";
    const face = el("span", "history-face");
    if (r.kind === "chat") face.append(h.mark(r.name, "small"));
    else face.append(icon("archive"));
    const main = el("span", "history-main");
    const line = el("span", "history-line");
    line.append(el("span", "label", r.title));
    if (r.archived) line.append(el("span", "history-chip", "Archived"));
    const time = el("time", null, when(r.at));
    if (r.at) {
      time.dateTime = new Date(r.at).toISOString();
      time.title = new Date(r.at).toLocaleString();
    }
    line.append(time);
    const sub = el("span", "history-sub");
    const who = r.kind === "chat" ? r.with : "Imported";
    if (who) sub.append(el("span", "history-with", who));
    const words = r.snippet ?? (r.kind === "chat" ? r.preview : r.entry.preview) ?? "";
    if (words) sub.append(el("span", r.snippet === undefined ? "history-preview" : "history-preview found", words));
    main.append(line, sub);
    open.append(face, main);
    open.onclick = () => {
      if (r.kind === "chat") h.openChat(r.name);
      else h.archive.openSession(r.entry.key);
      h.leaveSidebar();
    };
    item.append(open);
    return item;
  }
  // Continue with an agent of theirs: an archived chat's, or an imported
  // one's, made once and kept
  function controlOf(r) {
    const source = r.kind === "imported" ? { archive: h.archive.name(), key: r.entry.key } : r.archived ? { chat: r.name } : null;
    if (!source || !h.continueControl) return null;
    if (!controls.has(r.key)) controls.set(r.key, h.continueControl(source));
    return controls.get(r.key);
  }
  // The rows put in place by key, as the sidebar's are (shell.js `patch`):
  // a row that shows the same keeps its element (its focus, the scroll
  // around it), its Continue kept in it.
  function place(rows) {
    const box = parts.list;
    const was = new Map([...box.children].filter((e) => e.dataset.key).map((e) => [e.dataset.key, e]));
    const next = rows.map((r) => {
      const fresh = row(r);
      const old = was.get(r.key);
      const item = old?.firstElementChild.isEqualNode(fresh.firstElementChild) ? old : fresh;
      const control = controlOf(r);
      if (control && control.parentNode !== item) item.append(control);
      return item;
    });
    next.forEach((item, i) => {
      if (box.children[i] !== item) box.insertBefore(item, box.children[i] ?? null);
    });
    while (box.children.length > next.length) box.lastElementChild.remove();
  }

  function render() {
    if (!shown) return;
    if (!parts) build();
    const all = h.chats().filter((f) => !h.isThread(f));
    const unsaid = all.filter((f) => !f.previewAt);
    readFirsts(unsaid.filter((f) => firsts.get(f.name)?.row !== JSON.stringify(f)).slice(0, FIRSTS_MAX));
    const chats = all.flatMap((f) => {
      const first = f.previewAt ? null : firsts.get(f.name)?.first;
      if (!f.previewAt && !first?.at) return [];
      return [{ name: f.name, title: h.chatTitle(f.name), at: f.previewAt ?? first.at, archived: !!f.archived, preview: f.preview ?? first.line, with: withWhom(f) }];
    });
    const entries = h.archive.name() ? h.archive.entries() : [];
    const searched = hintOf(q) === search.q;
    const rows = historyRows({ chats, entries, filter, q, ...(searched ? { found: search.found, foundImported: search.foundImported } : {}) });
    for (const chip of parts.chips.children) chip.setAttribute("aria-pressed", String(chip.dataset.filter === filter));
    const words = wordsOf(q).length > 0;
    const waiting = words && !searched;
    parts.status.textContent = search.failed && searched ? `Messages were not searched: ${search.failed}` : waiting ? "Searching messages…" : "";
    parts.status.hidden = !parts.status.textContent;
    if (!rows.length) {
      const empty = words ? (waiting ? "" : `Nothing found for “${q.trim()}”`) : filter === "archived" ? "No archived chats" : filter === "imported" ? "No imported chats" : "No chats yet";
      parts.list.replaceChildren(...(empty ? [el("p", "muted history-empty", empty)] : []));
    } else place(rows.slice(0, limit));
    parts.more.hidden = rows.length <= limit;
    // Continue controls of rows no longer listed go
    const keys = new Set(rows.map((r) => r.key));
    for (const key of controls.keys()) if (!keys.has(key)) controls.delete(key);
  }

  return {
    open() {
      shown = true;
      // what failed to be read is read again
      for (const [name, f] of firsts) if (f.failed) firsts.delete(name);
      render();
      parts.input.focus({ preventScroll: true });
    },
    close() {
      shown = false;
    },
    // the person's list changed, or the archive's index: drawn again
    render,
  };
}
