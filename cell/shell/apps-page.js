// The Apps page (`/apps`; docs/api.md, The shell): every fragment in the
// person's list but their chats and agents (apps, brains, their skills, a
// kind this page does not know), each with what they may change of it
// here, as the server says it is:
// - Dock: whether their own dock shows it (`PUT /api/fragments/{name}/dock`),
//   an app's or a brain's; the switch moves only once the list says so.
// - New preview: its card shot again (`POST /api/f/{name}/card`), an
//   editor's; "Taking a new preview…" until the shot's report comes
//   (status's `page.at`), then the card read again.
// - Icon…: the icon sheet (app-icon.js), an editor's.
// - Who has access (`GET /api/access`, the owner's), and Share… (the
//   owner's) or People… (a member's), the platform's share sheet.
// Archived ones are folded at the end, each with Unarchive.
//
// The shell owns the page's place and its helpers (`h`): `api`, `el`,
// `icon`, `named`, `titleOf`, `art` (an app's icon, as the dock draws it),
// `share`, `iconSheet`, `reload` (the list again) and `state`.

const KIND = { brain: "Brain", skills: "Skills", other: "Other" };
const ROLE = { viewer: "Viewer", editor: "Editor", owner: "Owner" };
const GENERAL = { members: ["lock", "Restricted"], link: ["link", "Anyone with the link"], public: ["globe", "Public"] };
// the list's kinds the dock shows, and a card is shot of (card::is_app)
const dockable = (f) => f.kind === "app" || f.kind === "brain";
const listed = (f) => f.kind !== "chat" && f.kind !== "agent";
const owns = (f) => f.role === "owner";
const edits = (f) => f.role === "owner" || f.role === "editor";
const seg = (s) => encodeURIComponent(s).replace(/%3A/gi, ":");
const SHELL = { "x-fragment-shell": "1" };
// how long a preview is waited for here, asked every few seconds
const SHOT_WAIT_MS = 180_000;
// how many names a row says of who has access
const NAMES_SHOWN = 2;

export function createAppsPage(page, h) {
  // per app: an action in flight ("dock", "card", "archive"), what the
  // last one said (a refusal, or a preview still being taken), and its card
  const busy = new Map();
  const said = new Map();
  const cards = new Map();
  const shots = new Map();
  let access = null;
  let accessed = "";
  let shown = false;

  const say = (name, text) => {
    if (text) said.set(name, text);
    else said.delete(name);
  };

  // its card, read with this page's session as the dock's are; "new",
  // "same" or "none"
  async function loadCard(name) {
    const known = cards.get(name);
    const headers = { ...SHELL, ...(known ? { "if-none-match": known.etag } : {}) };
    const r = await fetch(`/api/f/${seg(name)}/card`, { headers, credentials: "same-origin" });
    if (r.status === 304) return "same";
    if (!r.ok) return "none";
    const url = URL.createObjectURL(await r.blob());
    if (known) URL.revokeObjectURL(known.url);
    cards.set(name, { etag: r.headers.get("etag"), url });
    return "new";
  }

  async function loadAccess() {
    access = await h.api("GET", "/api/access").catch((e) => ({ error: e.message }));
    accessed = sharingOf();
  }
  // what the list says of who is in each of theirs: a change asks again
  const sharingOf = () => JSON.stringify(h.state.fragments.filter((f) => listed(f) && owns(f)).map((f) => [f.name, f.sharing ?? null]));

  // who may open one of the person's own, at a glance
  function whoLine(f) {
    const line = h.el("span", "app-row-access");
    const row = access?.fragments?.find((r) => r.name === f.name);
    const [glyph, general] = GENERAL[row?.sharing?.visibility ?? f.sharing?.visibility] ?? GENERAL.members;
    line.append(h.icon(glyph), general);
    if (!row) {
      const n = (f.sharing?.guests ?? 0) + (f.sharing?.invites ?? 0);
      if (n) line.append(` · ${n} ${n === 1 ? "other" : "others"}`);
      return line;
    }
    // their own agents reach what they do, unnamed (decision 63)
    const people = (row.members ?? []).filter((m) => !(m.kind === "agent" && m.owner === h.state.me?.id));
    const names = people.map((m) => (m.kind === "agent" ? `${m.ownerEmail ?? "someone"}'s agent` : m.email ?? `${m.principal.slice(0, 9)}…`));
    const more = names.length - NAMES_SHOWN + (row.more ?? 0);
    if (names.length) line.append(` · ${names.slice(0, NAMES_SHOWN).join(", ")}${more > 0 ? ` +${more}` : ""}`);
    else if (!row.members && row.sharing?.guests) line.append(` · ${row.sharing.guests} ${row.sharing.guests === 1 ? "other" : "others"}`);
    else line.append(" · Only you");
    const invited = row.invites?.length ?? row.sharing?.invites ?? 0;
    if (invited) line.append(` · ${invited} invited`);
    return line;
  }

  function button(text, action, onClick, disabled = false) {
    const b = h.el("button", "quiet", text);
    b.type = "button";
    b.dataset.action = action;
    b.disabled = disabled;
    b.onclick = onClick;
    return b;
  }

  function row(f) {
    const name = f.name;
    const doing = busy.get(name);
    const item = h.el("article", "app-row");
    item.dataset.key = name;
    item.dataset.fragment = name;
    const art = h.el("span", "app-row-art");
    art.append(h.art(name));
    const text = h.el("div", "app-row-text");
    const title = h.named(h.el("strong", "app-row-title"), name);
    const about = [KIND[f.kind] ?? (f.kind === "app" ? "" : KIND.other), owns(f) ? "" : `Shared with you · ${ROLE[f.role] ?? f.role}`].filter(Boolean).join(" · ");
    text.append(title);
    if (about) text.append(h.el("span", "app-row-meta", about));
    if (owns(f)) text.append(whoLine(f));
    const head = h.el("div", "app-row-head");
    head.append(art, text);
    const card = cards.get(name);
    if (card) {
      const img = h.el("img", "app-row-card");
      img.alt = "";
      img.src = card.url;
      head.append(img);
    }
    const controls = h.el("div", "app-row-controls");
    if (f.archived) {
      controls.append(button(doing === "archive" ? "Unarchiving…" : "Unarchive", "unarchive", () => unarchive(name), !!doing));
    } else if (dockable(f)) {
      const on = f.dock !== false;
      const dock = h.el("button", "app-dock");
      dock.type = "button";
      dock.dataset.action = "dock";
      dock.setAttribute("role", "switch");
      dock.setAttribute("aria-checked", String(on));
      dock.disabled = !!doing;
      if (doing === "dock") dock.setAttribute("aria-busy", "true");
      dock.append(h.el("span", "app-dock-track"), "Dock");
      dock.onclick = () => setDock(name, !on);
      controls.append(dock);
    }
    if (!f.archived && dockable(f) && edits(f) && !(owns(f) && f.sharing?.visibility === "members")) {
      const taking = shots.has(name);
      controls.append(button(doing === "card" ? "Asking…" : taking ? "Taking a new preview…" : "New preview", "card", () => newPreview(name), !!doing || taking));
    }
    if (!f.archived && edits(f)) controls.append(button("Icon…", "icon", () => h.iconSheet(name), !!doing));
    controls.append(button(owns(f) ? "Share…" : "People…", "share", () => h.share(name)));
    item.append(head, controls);
    const words = said.get(name);
    if (words) {
      const p = h.el("p", "app-row-said", words);
      p.setAttribute("role", "status");
      item.append(p);
    }
    // what it shows, to keep the element (its focus) while that is the same
    item.dataset.shows = JSON.stringify([f, doing ?? null, words ?? null, card?.url ?? null, shots.has(name), item.querySelector(".app-row-access")?.textContent ?? null]);
    return item;
  }

  // rows put in place by key, an element kept while it shows the same
  function place(box, rows) {
    const was = new Map([...box.children].filter((e) => e.dataset.key).map((e) => [e.dataset.key, e]));
    const next = rows.map((r) => {
      const old = was.get(r.dataset.key);
      return old && old.dataset.shows === r.dataset.shows ? old : r;
    });
    box.replaceChildren(...next);
  }

  function render() {
    if (!shown) return;
    const all = h.state.fragments.filter(listed);
    const live = all.filter((f) => !f.archived);
    const archived = all.filter((f) => f.archived);
    let list = page.querySelector(".apps-list");
    let folded = page.querySelector(".apps-archived");
    if (!list) {
      const wrap = h.el("div", "apps-layout");
      const head = h.el("header", "apps-intro");
      head.append(h.el("h1", null, "Apps"));
      list = h.el("div", "apps-list");
      folded = h.el("details", "apps-archived");
      folded.append(h.el("summary"), h.el("div", "apps-list"));
      wrap.append(head, list, folded);
      page.replaceChildren(wrap);
    }
    if (access?.error && !page.querySelector(".apps-access-failed")) list.before(h.el("p", "settings-warning apps-access-failed", `Who has access did not load: ${access.error}`));
    if (!access?.error) page.querySelector(".apps-access-failed")?.remove();
    place(list, live.map(row));
    if (!live.length) list.replaceChildren(h.el("p", "muted apps-empty", "No apps yet."));
    folded.hidden = !archived.length;
    folded.querySelector("summary").textContent = `Archived (${archived.length})`;
    place(folded.querySelector(".apps-list"), archived.map(row));
  }

  async function setDock(name, dock) {
    busy.set(name, "dock");
    say(name, "");
    render();
    try {
      await h.api("PUT", `/api/fragments/${seg(name)}/dock`, { dock });
      await h.reload();
    } catch (e) {
      say(name, e.message);
    } finally {
      busy.delete(name);
      render();
    }
  }

  async function unarchive(name) {
    busy.set(name, "archive");
    say(name, "");
    render();
    try {
      await h.api("PUT", `/api/fragments/${seg(name)}/archived`, { archived: false });
      await h.reload();
    } catch (e) {
      say(name, e.message);
    } finally {
      busy.delete(name);
      render();
    }
  }

  // a shot asked for: waited for by its report (status's `page.at` from
  // when it was wanted on), then its card read again
  async function newPreview(name) {
    busy.set(name, "card");
    say(name, "");
    render();
    let asked;
    try {
      asked = await h.api("POST", `/api/f/${seg(name)}/card`);
    } catch (e) {
      say(name, e.message);
      return;
    } finally {
      busy.delete(name);
      render();
    }
    shots.set(name, asked.since);
    render();
    const until = Date.now() + SHOT_WAIT_MS;
    let wait = 2_000;
    // bounded: until the report comes, the page closes, or SHOT_WAIT_MS
    while (shown && Date.now() < until) {
      await new Promise((r) => setTimeout(r, wait));
      wait = Math.min(wait * 1.5, 10_000);
      const status = await h.api("GET", `/api/f/${seg(name)}/status`).catch(() => null);
      if (status?.page && status.page.at >= asked.since) break;
    }
    const done = shown && Date.now() < until;
    shots.delete(name);
    await loadCard(name).catch(() => "same");
    say(name, done ? "" : "The preview is still being taken.");
    render();
  }

  return {
    // shown at `/apps`: the list as it is, who has access, and each card
    async open() {
      shown = true;
      page.replaceChildren();
      await loadAccess();
      render();
      const names = h.state.fragments.filter((f) => listed(f) && dockable(f)).map((f) => f.name);
      const got = await Promise.all(names.map((n) => loadCard(n).catch(() => "same")));
      if (got.includes("new")) render();
    },
    close() {
      shown = false;
    },
    // the list changed: drawn again, and who has access asked again when
    // the list says that changed
    async listed() {
      if (!shown) return;
      render();
      if (sharingOf() !== accessed) {
        await loadAccess();
        render();
      }
    },
    // a share sheet closed: who has access, asked again
    async shared() {
      if (!shown) return;
      await loadAccess();
      render();
    },
  };
}
