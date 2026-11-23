// The Skills page (`/skills`, from Settings' Skills): what the person's
// agents know how to do, in one place, and the optional skills they can
// switch on. Two tabs:
// - On: the optional skills they switched on, then the skills included
//   with Fragment (the managed set, decision 17: always on, each opening
//   its folder in the skills fragment's files).
// - Library: every optional skill of the pinned Hermes image, by category,
//   each with its switch. Nothing is hidden; a few carry words of what
//   they can do (`LABELS`), and switching one of those on asks first.
//
// It reads the person's skills fragment (`skillsFragment`, made from the
// blessed template when they have none): its listing, whose `optional/
// hermes/<id>.on` files are the switches. The library is the release's
// catalog, which the shell publishes (`/__shell/optional-skills-hermes.json`,
// cargo xtask skills-catalog), so a skills fragment that no longer follows
// the template has it too. A switch on an id the catalog no longer names
// stays in On, to be switched off. A switch is one file
// written or removed (`POST /api/f/{name}/files`), so two pages switching
// two skills never undo each other. The platform keeps the names alone:
// our Hermes image installs each from its own copy within ten minutes
// (images/hermes/boot/src/skills.rs, `settle_optional`); other runtimes
// take none.
//
// The shell owns the page's place and its helpers (`h`): `api`, `el`,
// `seg`, `skillsFragment`, `text` (a file's text: `api` reads JSON),
// `releaseJson` (a file the shell publishes, as JSON), `openApp` and
// `openFiles`.

export const ON_PREFIX = "optional/hermes/";
export const ON_SUFFIX = ".on";
export const CATALOG = "optional-skills-hermes.json";
const MANAGED = /^skills\/(?:(.+)\/)?([^/]+)\/SKILL\.md$/;
// managed skills' SKILL.md read at once, for their descriptions
const READS_AT_ONCE = 6;
// a managed file larger is one the computer refuses (hermes-boot's
// skills.rs, `FILE_MAX_BYTES`): never installed, so never listed as on
export const FILE_MAX_BYTES = 256 * 1024;

// What a few optional skills can do that a person should know before
// switching one on (the catalog's `labels`): its chip, and its question.
export const LABELS = {
  spends: { chip: "Spends money", ask: "can spend money" },
  contacts: { chip: "Contacts people", ask: "can contact people for you" },
  care: { chip: "Use with care", ask: "can be misused" },
};

export const markerPath = (id) => `${ON_PREFIX}${id}${ON_SUFFIX}`;
export const categoryOf = (id) => id.split("/")[0];

const WORDS = { ai: "AI", mcp: "MCP", mlops: "MLOps", devops: "DevOps", llm: "LLM", llms: "LLMs", cli: "CLI", ux: "UX", api: "API", pdf: "PDF", rss: "RSS", evm: "EVM", ip: "IP", bci: "BCI", tts: "TTS", osint: "OSINT", pptx: "PPTX", har: "HAR", gif: "GIF", oss: "OSS", qmd: "QMD", md: "MD", dcf: "DCF", lbo: "LBO", "3": "3" };
// A slug in words: `autonomous-ai-agents` → "Autonomous AI agents".
export function words(slug) {
  const parts = slug.split(/[-_]/).filter(Boolean).map((w) => WORDS[w.toLowerCase()] ?? w.toLowerCase());
  const said = parts.join(" ");
  return said.charAt(0).toUpperCase() + said.slice(1);
}

// What the skills fragment's listing says: the managed skills (by path),
// and the optional skills switched on (their ids).
export function readListing(files) {
  const managed = [];
  const on = new Set();
  for (const f of files ?? []) {
    const m = MANAGED.exec(f.path);
    if (m && f.size > FILE_MAX_BYTES) continue;
    if (m) managed.push({ name: m[2], folder: f.path.slice(0, -"SKILL.md".length), path: f.path });
    else if (f.path.startsWith(ON_PREFIX) && f.path.endsWith(ON_SUFFIX)) on.add(f.path.slice(ON_PREFIX.length, -ON_SUFFIX.length));
  }
  managed.sort((a, b) => a.name.localeCompare(b.name));
  return { managed, on };
}

// Whether a skill answers a search: its name, title or description holds
// every word of it.
export function matches(skill, query) {
  const said = `${skill.name} ${words(skill.name)} ${skill.description ?? ""} ${skill.id ?? ""}`.toLowerCase();
  return query.toLowerCase().split(/\s+/).filter(Boolean).every((w) => said.includes(w));
}

// A SKILL.md's frontmatter description, one line.
export function descriptionOf(text) {
  const front = /^﻿?---\r?\n([\s\S]*?)\r?\n---/.exec(text ?? "")?.[1] ?? "";
  const line = front.split(/\r?\n/).find((l) => l.startsWith("description:"));
  if (!line) return "";
  const raw = line.slice("description:".length).trim();
  if (raw.startsWith('"')) {
    try { return JSON.parse(raw); } catch { return raw.slice(1, -1); }
  }
  if (raw.startsWith("'") && raw.endsWith("'")) return raw.slice(1, -1).replaceAll("''", "'");
  return raw;
}

// The optional skills switched on, as rows: the catalog's, then any id it
// no longer names (a computer on an older image may still show it), by
// its id alone, so it can always be switched off.
export function onRows(catalog, on) {
  const known = catalog.filter((s) => on.has(s.id));
  const ids = new Set(catalog.map((s) => s.id));
  const gone = [...on].filter((id) => !ids.has(id)).sort().map((id) => ({ id, name: id.split("/").pop(), description: "No longer in the library. Switch it off to remove it.", gone: true }));
  return [...known, ...gone];
}

// The library's categories, each with its skills, in order.
export function byCategory(skills) {
  const groups = new Map();
  for (const s of skills) {
    const c = categoryOf(s.id);
    if (!groups.has(c)) groups.set(c, []);
    groups.get(c).push(s);
  }
  return [...groups].sort((a, b) => words(a[0]).localeCompare(words(b[0])));
}

export function createSkillsPage(page, h) {
  const { el } = h;
  let shown = false;
  let visit = 0;
  // what the last read found: the skills fragment's name, the catalog, the
  // managed set and what is on; null while reading, `failed` when it did
  let got = null;
  let failed = null;
  let tab = "on";
  let query = "";
  let category = "";
  // the skill whose details are open, and the one asking before it is on
  let opened = null;
  let asking = null;
  // switches being written: id → the state asked for; and the writes, which
  // a fresh read waits for, so it never shows what they replace
  const saving = new Map();
  const writes = new Set();
  // where focus goes after a redraw replaces the control that held it:
  // {id, on: "ask" | "switch" | "main"}
  let refocus = null;
  let focusing = null;
  // each row's controls as last drawn: id → {main, toggle, yes, no}
  let controls = new Map();
  let said = null;
  let parts = null;
  // the skills fragment's name once known, even when a read after it fails
  let fragment = null;

  function build() {
    const wrap = el("div", "skills-layout");
    const head = el("header", "skills-intro");
    const title = el("div", "skills-title");
    const files = el("button", "quiet skills-files", "Files");
    files.type = "button";
    files.title = "Your skills fragment's files";
    files.onclick = () => fragment && h.openApp(fragment);
    title.append(el("h1", null, "Skills"), files);
    head.append(title, el("p", "skills-lede", "What your agents know how to do. Turn on more from the library, and your Hermes agents pick each one up at their next skills update."));
    const tools = el("div", "skills-tools");
    const tabs = el("div", "skills-tabs");
    tabs.setAttribute("role", "tablist");
    const tabOn = tabButton("on");
    const tabLibrary = tabButton("library");
    tabs.append(tabOn, tabLibrary);
    const search = el("input", "skills-search");
    search.type = "search";
    search.placeholder = "Search skills";
    search.setAttribute("aria-label", "Search skills");
    search.oninput = () => {
      query = search.value.trim();
      render();
    };
    const pick = el("select", "skills-category");
    pick.setAttribute("aria-label", "Category");
    pick.onchange = () => {
      category = pick.value;
      render();
    };
    tools.append(tabs, search, pick);
    const status = el("p", "skills-status");
    status.setAttribute("role", "status");
    status.setAttribute("aria-live", "polite");
    const list = el("div", "skills-list");
    const note = el("p", "skills-note");
    wrap.append(head, tools, status, list, note);
    page.replaceChildren(wrap);
    parts = { tabOn, tabLibrary, search, pick, status, list, note };
  }

  function tabButton(which) {
    const b = el("button", "skills-tab");
    b.type = "button";
    b.setAttribute("role", "tab");
    b.dataset.tab = which;
    b.onclick = () => {
      tab = which;
      opened = asking = null;
      render();
    };
    return b;
  }

  function chips(skill) {
    return (skill.labels ?? []).filter((l) => LABELS[l]).map((l) => el("span", `skills-chip skills-chip-${l}`, LABELS[l].chip));
  }

  // An optional skill's row: its title and description, its labels, its
  // switch; its details below when opened.
  function optionalRow(s) {
    const on = saving.has(s.id) ? saving.get(s.id) : got.on.has(s.id);
    const item = el("div", "skills-item");
    item.dataset.skill = s.id;
    const row = el("div", "skills-row");
    const main = el("button", "skills-main");
    main.type = "button";
    main.setAttribute("aria-expanded", String(opened === s.id));
    const line = el("span", "skills-line");
    line.append(el("span", "skills-name", words(s.name)), ...chips(s));
    main.append(line, el("span", "skills-desc", s.description));
    main.onclick = () => {
      opened = opened === s.id ? null : s.id;
      refocus = { id: s.id, on: "main" };
      render();
    };
    const toggle = el("input", "settings-switch skills-switch");
    toggle.type = "checkbox";
    toggle.checked = on;
    toggle.disabled = saving.has(s.id);
    toggle.setAttribute("aria-label", `${words(s.name)}: ${on ? "on" : "off"}`);
    toggle.onchange = () => {
      if (toggle.checked && (s.labels ?? []).some((l) => LABELS[l])) {
        toggle.checked = false;
        asking = s.id;
        refocus = { id: s.id, on: "ask" };
        render();
        return;
      }
      flip(s, toggle.checked);
    };
    row.append(main, toggle);
    item.append(row);
    controls.set(s.id, { main, toggle });
    if (refocus?.id === s.id) focusing = refocus.on === "main" ? main : refocus.on === "switch" ? toggle : focusing;
    if (asking === s.id) item.append(ask(s));
    if (opened === s.id) item.append(details(s));
    return item;
  }

  // Before a labelled skill is on: what it can do, in words, and a choice.
  function ask(s) {
    const box = el("div", "skills-ask");
    const can = (s.labels ?? []).filter((l) => LABELS[l]).map((l) => LABELS[l].ask);
    box.append(el("p", null, `${words(s.name)} ${can.join(" and ")}. Your agents won't ask before they use it.`));
    const actions = el("div", "skills-ask-actions");
    const yes = el("button", "primary", "Turn on");
    yes.type = "button";
    yes.onclick = () => {
      asking = null;
      flip(s, true);
    };
    const no = el("button", "quiet", "Cancel");
    no.type = "button";
    no.onclick = () => {
      asking = null;
      refocus = { id: s.id, on: "switch" };
      render();
    };
    actions.append(yes, no);
    Object.assign(controls.get(s.id), { yes, no });
    if (refocus?.id === s.id) focusing = refocus.on === "ask" ? yes : refocus.on === "cancel" ? no : focusing;
    box.append(actions);
    return box;
  }

  function details(s) {
    const box = el("div", "skills-details");
    const facts = el("p", "skills-facts");
    facts.append(el("code", null, s.name), el("span", null, words(categoryOf(s.id))));
    if (s.license) facts.append(el("span", null, `${s.license} licence`));
    box.append(facts);
    if (got.catalog.source && !s.gone) {
      const read = el("a", "skills-read", "Read the skill");
      read.href = `${got.catalog.source}/${s.id.split("/").map(encodeURIComponent).join("/")}/SKILL.md`;
      read.target = "_blank";
      read.rel = "noopener noreferrer";
      box.append(read);
    }
    return box;
  }

  // A managed skill's row: always on; it opens its folder in the files.
  function managedRow(m) {
    const item = el("div", "skills-item");
    const row = el("button", "skills-row skills-row-managed");
    row.type = "button";
    const main = el("span", "skills-main");
    const line = el("span", "skills-line");
    line.append(el("span", "skills-name", words(m.name.replace(/-finite$/, ""))));
    main.append(line, el("span", "skills-desc", m.description ?? ""));
    row.append(main, el("span", "skills-always", "Always on"));
    row.onclick = () => h.openFiles(got.name, m.folder);
    item.append(row);
    return item;
  }

  function heading(text, count) {
    const head = el("h2", "skills-heading");
    head.append(el("span", null, text), el("span", "skills-count", String(count)));
    return head;
  }

  function empty(text, action) {
    const box = el("div", "skills-empty");
    box.append(el("p", "muted", text));
    if (action) box.append(action);
    return box;
  }

  function button(text, onClick) {
    const b = el("button", "quiet", text);
    b.type = "button";
    b.onclick = onClick;
    return b;
  }

  function render() {
    if (!shown) return;
    if (!parts) build();
    parts.status.textContent = said ?? "";
    parts.status.hidden = !said;
    if (!got) {
      parts.tabOn.hidden = parts.tabLibrary.hidden = parts.search.hidden = parts.pick.hidden = true;
      parts.note.hidden = true;
      parts.list.replaceChildren(failed ? empty("Couldn't load your skills.", button("Try again", () => readAll())) : el("p", "muted skills-loading", "Loading skills…"));
      return;
    }
    const skills = got.catalog.skills;
    const onList = onRows(skills, new Set([...got.on, ...[...saving].filter(([, on]) => on).map(([id]) => id)]));
    parts.tabOn.hidden = parts.tabLibrary.hidden = parts.search.hidden = false;
    parts.tabOn.textContent = `On · ${onList.length + got.managed.length}`;
    parts.tabLibrary.textContent = `Library · ${skills.length}`;
    for (const b of [parts.tabOn, parts.tabLibrary]) b.setAttribute("aria-selected", String(b.dataset.tab === tab));
    parts.pick.hidden = tab !== "library";
    const groups = byCategory(skills);
    if (!parts.picked) {
      parts.picked = true;
      parts.pick.replaceChildren(option("All categories", ""), ...groups.map(([c, list]) => option(`${words(c)} · ${list.length}`, c)));
    }
    parts.pick.value = category;
    const found = (s) => !query || matches(s, query);
    // a row control the person has focused meanwhile keeps it, redrawn
    const held = heldFocus();
    if (held && held.id !== refocus?.id) refocus = held;
    controls = new Map();
    const out = [];
    if (tab === "on") {
      const mine = onList.filter(found);
      const included = got.managed.filter(found);
      out.push(heading("You turned on", mine.length));
      if (mine.length) out.push(...mine.map(optionalRow));
      else if (!query) out.push(empty("Nothing turned on yet.", button("Browse the library", () => { tab = "library"; render(); })));
      out.push(heading("Included with Fragment", included.length), ...included.map(managedRow));
      if (query && !mine.length && !included.length) out.push(empty(`No skills match “${query}”.`, button("Clear search", clear)));
    } else {
      let any = false;
      for (const [c, list] of groups) {
        if (category && c !== category) continue;
        const rows = list.filter(found);
        if (!rows.length) continue;
        any = true;
        out.push(heading(words(c), rows.length), ...rows.map(optionalRow));
      }
      if (!any) out.push(empty(`No skills match “${query}”.`, button("Clear search", clear)));
    }
    parts.list.replaceChildren(...out);
    refocus = null;
    if (focusing) takeFocus(focusing);
    focusing = null;
    parts.note.hidden = false;
    parts.note.textContent = `The library is Hermes' optional skills (${got.catalog.hermes}), by Nous Research and their authors; Fragment doesn't write or review them. Switches apply to your Hermes agents. An agent's own skills are under Settings → Agents.`;
  }

  // The row control holding focus, as {id, on}, if one does.
  function heldFocus() {
    const at = globalThis.document?.activeElement;
    for (const [id, c] of controls) {
      if (at === c.toggle) return { id, on: "switch" };
      if (at === c.main) return { id, on: "main" };
      if (at === c.yes) return { id, on: "ask" };
      if (at === c.no) return { id, on: "cancel" };
    }
    return null;
  }

  // Focus to `target` when the redraw took it (it fell to the body), never
  // from where the person has put it.
  function takeFocus(target) {
    const at = globalThis.document?.activeElement;
    if (!at || at === globalThis.document.body) target.focus?.();
  }

  function option(text, value) {
    const o = el("option", null, text);
    o.value = value;
    return o;
  }

  function clear() {
    query = "";
    category = "";
    if (parts) parts.search.value = "";
    render();
  }

  // One switch written: its file added or removed. Shown at once, put
  // back if the write fails.
  async function flip(s, on) {
    const name = got.name;
    saving.set(s.id, on);
    said = null;
    render();
    const change = on ? { path: markerPath(s.id), text: "on\n" } : { path: markerPath(s.id), delete: true };
    const write = h.api("POST", `/api/f/${h.seg(name)}/files`, { files: [change], message: `${on ? "Turn on" : "Turn off"} ${s.id}` });
    writes.add(write);
    try {
      await write;
      // in whichever visit it lands: a reopened page shows it too
      if (got?.name === name) {
        if (on) got.on.add(s.id);
        else got.on.delete(s.id);
      }
      said = on ? `${words(s.name)} is on. Your Hermes agents pick it up at their next skills update.` : `${words(s.name)} is off.`;
    } catch (e) {
      said = `Couldn't turn ${on ? "on" : "off"} ${words(s.name)}: ${e.message}. Try again.`;
    } finally {
      writes.delete(write);
      saving.delete(s.id);
      refocus = { id: s.id, on: "switch" };
      render();
    }
  }

  async function readAll() {
    const mine = ++visit;
    got = failed = null;
    said = null;
    render();
    try {
      const name = await h.skillsFragment();
      if (mine === visit) fragment = name;
      await Promise.allSettled([...writes]);
      const [listing, catalog] = await Promise.all([h.api("GET", `/api/f/${h.seg(name)}/files`), h.releaseJson(CATALOG)]);
      if (mine !== visit) return;
      if (!Array.isArray(catalog?.skills)) throw new Error("no catalog");
      got = { name, catalog, ...readListing(listing.files) };
      render();
      // the managed skills' descriptions, a few at a time, drawn as they come
      let next = 0;
      const worker = async () => {
        while (next < got.managed.length && mine === visit) {
          const m = got.managed[next++];
          m.description = descriptionOf(await h.text(name, m.path).catch(() => ""));
        }
      };
      await Promise.all(Array.from({ length: READS_AT_ONCE }, worker));
      if (mine === visit) render();
    } catch (e) {
      if (mine !== visit) return;
      console.warn("skills:", e);
      failed = e;
      render();
    }
  }

  return {
    open() {
      shown = true;
      readAll();
    },
    close() {
      shown = false;
      ++visit;
      opened = asking = null;
    },
  };
}
