// The Apps page (`/apps`): every fragment in the
// person's list but their chats, agents and skills (apps, brains, a kind
// this page does not know), and any that reaches their accounts (a
// connector's chat), each with what they may change of it here, as the
// server says it is:
// - Dock: whether their own dock shows it (`PUT /api/fragments/{name}/dock`),
//   an app's or a brain's; the switch moves only once the list says so.
// - New preview: its card shot again (`POST /api/f/{name}/card`), an
//   editor's; "Taking a new preview…" until the shot's report comes
//   (status's `page.at`), then the card read again.
// - Appearance: the inline icon editor (app-icon.js) and preview controls.
// - Who has access (`GET /api/access`, the owner's), and inline Sharing
//   (the owner's) or People (a member's), the platform's sharing view.
// - A display name edited inline, and the owner's optional public address.
// - Reaches (FIN-291; Paul, 2026-11-01), only on one that reaches their
//   accounts: the keys it holds, by name and the hosts each may go to
//   (never a value), with Remove; the agent it speaks to when it is a
//   connector's door (`/api/f/{name}/door`), with Unlink; their own agents in it. Such an app is theirs and their
//   agents' alone for now: the cell refuses sharing it, and its share sheet
//   (its Sharing here) says so (reach.rs, share.rs).
// Archived ones are folded at the end, each with Unarchive.
//
// The shell owns the page's place and its helpers (`h`): `api`, `el`,
// `icon`, `named`, `titleOf`, `art` (an app's icon, as the dock draws it),
// `retitle`, `originOf`, `openApp`, `reload` (the list again) and `state`.

import { activity } from "./activity.js";
import { createIconEditor } from "./app-icon.js";

const KIND = { brain: "Brain", other: "Other" };
const ROLE = { viewer: "Viewer", editor: "Editor", owner: "Owner" };
const GENERAL = { members: ["lock", "Restricted"], link: ["link", "Anyone with the link"], public: ["globe", "Public"] };
// the list's kinds the dock shows, and a card is shot of (card::is_app)
const dockable = (f) => f.kind === "app" || f.kind === "brain";
// What the page lists: every fragment but their chats, agents, skills and
// Session Archives, and a chat that reaches their accounts (its row's
// `sharing.reach`: a connector, such as Telegram's chat, with its keys and
// its door), so what reaches them is in one place (Paul, 2026-11-01). An
// agent with a door (its SimpleX link) is not: that door is opened and
// closed in Settings, from its own page (Alex on #918).
export const listed = (f) => (f.kind !== "chat" && f.kind !== "agent" && f.kind !== "skills" && f.kind !== "sessions") || (f.kind === "chat" && f.sharing?.reach === true);
const owns = (f) => f.role === "owner";
const edits = (f) => f.role === "owner" || f.role === "editor";
const seg = (s) => encodeURIComponent(s).replace(/%3A/gi, ":");
const SHELL = { "x-fragment-shell": "1" };
// how long a preview is waited for here, asked every second
const SHOT_WAIT_MS = 180_000;
// a card being generated is read at most this often per row and visit
const CARD_READS_MAX = 6;
// how many names a row says of who has access
const NAMES_SHOWN = 2;

export function createAppsPage(page, h) {
  // per app: an action in flight ("dock", "card", "archive"), what the
  // last one said (a refusal, or a preview still being taken), and its card
  const busy = new Map();
  const said = new Map();
  const cards = new Map();
  const cardReads = new Map();
  const cardAttempts = new Map();
  const cardRows = new Map();
  const cardsLoad = { wait: 0, timer: null, batch: 0 };
  const shots = new Map();
  const addresses = new Map();
  const addressInfo = new Map();
  const addressEditors = new Map();
  // What each app can reach (FIN-291, a design's prototype): its secrets by
  // name with the hosts each may go to (an editor's read, values never),
  // and the agent it speaks to for its owner when it is a door (the
  // owner's), as the server says now; and a removal asked (a second press)
  const reach = new Map();
  const confirming = new Map();
  const expanded = new Map();
  const editing = new Map();
  const iconEditors = new Map();
  const sharingFrames = new Map();
  let access = null;
  let accessed = "";
  let accessRead = 0;
  let operation = 0;
  let visit = 0;
  let shown = false;

  const say = (name, text) => {
    if (text) said.set(name, text);
    else said.delete(name);
  };
  const beginBusy = (name, kind) => {
    const token = ++operation;
    busy.set(name, { kind, token, started: Date.now() });
    return token;
  };
  const endBusy = (name, token) => {
    if (busy.get(name)?.token === token) busy.delete(name);
  };

  async function loadAddresses(force = false) {
    const mine = visit;
    const rows = h.state.fragments.filter(listed);
    // The server supplies the actual scheme, suffix and preview mark.
    // Eight at a time, rather than one request per app all at once.
    for (let offset = 0; offset < rows.length; offset += 8) {
      await Promise.all(rows.slice(offset, offset + 8).map(async (f) => {
        if (!force && addressInfo.has(f.name)) return;
        const [origin, info] = await Promise.all([
          h.originOf(f.name).catch(() => null),
          h.api("GET", `/api/f/${seg(f.name)}/address`).catch((error) => ({ error: error.message })),
        ]);
        if (mine !== visit) return;
        if (origin) addresses.set(f.name, info.address?.url ?? origin);
        addressInfo.set(f.name, info);
        addressEditors.delete(f.name);
      }));
      if (mine !== visit) return;
      render();
    }
  }

  // An app's reach, read now: its secrets (an editor's) and its door (its
  // owner's); what cannot be read is left unsaid.
  async function readReach(f) {
    const [secrets, door] = await Promise.all([
      h.api("GET", `/api/f/${seg(f.name)}/secrets`).then((r) => r.secrets ?? [], () => null),
      owns(f) ? h.api("GET", `/api/f/${seg(f.name)}/door`).then((r) => r.bound ?? null, () => null) : null,
    ]);
    return { secrets, door };
  }
  // Each app's reach, eight at a time.
  async function loadReach() {
    const mine = visit;
    const rows = h.state.fragments.filter((f) => listed(f) && edits(f));
    for (let offset = 0; offset < rows.length; offset += 8) {
      await Promise.all(rows.slice(offset, offset + 8).map(async (f) => {
        const read = await readReach(f);
        if (mine !== visit) return;
        reach.set(f.name, read);
      }));
      if (mine !== visit) return;
      render();
    }
  }
  // Whether an app reaches the person's accounts (Paul, 2026-11-01: only
  // then its Reaches tab and line show, and it stays theirs and their
  // agents' alone): a key it holds or a door it is, as their own row says
  // it (`sharing.reach`, the cell's own rule), else, on one shared with
  // them they edit, as read here.
  const reaches = (f) => {
    if (owns(f)) return f.sharing?.reach === true;
    const r = reach.get(f.name);
    return !!r && ((r.secrets?.length ?? 0) > 0 || !!r.door);
  };
  // Their own agents in it, by name and what they may do (Paul: a line of
  // its Reaches); none, nothing.
  function agentsLine(f) {
    const row = access?.fragments?.find((x) => x.name === f.name);
    const mine = (row?.members ?? []).filter((m) => m.kind === "agent" && m.owner === h.state.me?.id);
    const named = mine.map((m) => {
      const fragment = h.state.agents.get(m.principal)?.fragment;
      return `${fragment ? h.titleOf(fragment) : "An agent of yours"} (${m.role === "editor" ? "can edit" : "can view"})`;
    });
    return named.length ? h.el("p", "app-reach-agents", `Your agents in it: ${named.join(", ")}`) : null;
  }
  // what it reaches, in a line: the hosts its secrets go to, and whom it speaks to
  function reachLine(f) {
    const r = reach.get(f.name);
    const hosts = [...new Set((r?.secrets ?? []).flatMap((s) => s.hosts))];
    const parts = [hosts.length ? `Reaches ${hosts.join(", ")}` : "", r?.door ? `Speaks for you to ${h.titleOf(r.door.agent)}` : ""].filter(Boolean);
    return parts.length ? h.el("span", "app-row-meta app-reach-line", parts.join(" · ")) : null;
  }
  // the panel: each secret with its hosts and Remove; the door with Unlink;
  // a press asks once more before it acts
  function reachPanel(f) {
    const r = reach.get(f.name);
    const box = h.el("section", "app-reach");
    if (!r) {
      box.append(h.el("p", "muted", "Reading what it can reach…"));
      return box;
    }
    const ask = (key, label, act) => {
      const b = button(confirming.get(f.name) === key ? `${label}? Yes` : label, "reach", () => {
        if (confirming.get(f.name) !== key) {
          confirming.set(f.name, key);
          return render();
        }
        confirming.delete(f.name);
        act();
      }, busy.get(f.name)?.kind === "reach");
      b.classList.add("quiet");
      return b;
    };
    const keys = r.secrets ?? [];
    box.append(h.el("h2", null, "Keys it holds"));
    if (r.secrets === null) box.append(h.el("p", "muted", "Its keys could not be read."));
    else if (!keys.length) box.append(h.el("p", "muted", "None. It reaches none of your accounts by a key."));
    for (const k of keys) {
      const line = h.el("div", "app-reach-item");
      const what = h.el("div", "app-reach-what");
      what.append(h.el("code", null, k.name), h.el("span", "muted", `sent only to ${k.hosts.join(", ")}`));
      line.append(what, ask(`secret:${k.name}`, "Remove", () => revoke(f.name, "DELETE", `/api/f/${seg(f.name)}/secrets/${encodeURIComponent(k.name)}`)));
      box.append(line);
    }
    if (owns(f) && r.door) {
      box.append(h.el("h2", null, "Door"));
      const line = h.el("div", "app-reach-item");
      const what = h.el("div", "app-reach-what");
      what.append(h.el("span", null, `Speaks for you to ${h.titleOf(r.door.agent)}`), h.el("span", "muted", `through ${r.door.door}`));
      line.append(what, ask("door", "Unlink", () => revoke(f.name, "POST", `/api/f/${seg(f.name)}/door`, { act: "unlink" })));
      box.append(line);
    }
    const through = owns(f) && r.door ? "the keys and door above" : "the keys above";
    const agents = agentsLine(f);
    if (agents) box.append(agents);
    box.append(h.el("p", "muted app-reach-note", `Any app can read public web pages. It reaches your accounts only through ${keys.length || r.door ? through : "a key or a door"}.`));
    return box;
  }
  async function revoke(name, method, path, body) {
    const operation = beginBusy(name, "reach");
    say(name, "");
    render();
    try {
      await h.api(method, path, body);
    } catch (e) {
      say(name, e.message);
    } finally {
      endBusy(name, operation);
    }
    const f = h.state.fragments.find((x) => x.name === name);
    if (f) reach.set(name, await readReach(f));
    render();
  }

  // its card, read with this page's session as the dock's are; "new",
  // "same", "none" or "stale" (a newer read or visit owns the row)
  async function loadCard(name, mine) {
    const request = (cardReads.get(name) ?? 0) + 1;
    cardReads.set(name, request);
    const current = () => mine === visit && cardReads.get(name) === request;
    const known = cards.get(name);
    const headers = { ...SHELL, ...(known ? { "if-none-match": known.etag } : {}) };
    const r = await fetch(`/api/f/${seg(name)}/card`, { headers, credentials: "same-origin" });
    if (!current()) return "stale";
    if (r.status === 304) return "same";
    if (!r.ok) return "none";
    const body = await r.blob();
    if (!current()) return "stale";
    const url = URL.createObjectURL(body);
    if (known) URL.revokeObjectURL(known.url);
    cards.set(name, { etag: r.headers.get("etag"), url });
    return "new";
  }

  // Every card on open; afterwards, a new or changed row's, and one whose
  // card is not there yet, asked again with bounded backoff and retries.
  async function loadCards(all = false) {
    if (!shown) return;
    clearTimeout(cardsLoad.timer);
    const mine = visit;
    const batch = ++cardsLoad.batch;
    const rows = h.state.fragments.filter((f) => listed(f) && dockable(f));
    let fresh = false;
    const asked = [];
    for (const f of rows) {
      const fingerprint = JSON.stringify(f);
      if (!all && cards.has(f.name) && cardRows.get(f.name) === fingerprint) continue;
      if (all || cardAttempts.get(f.name)?.fingerprint !== fingerprint) {
        cardAttempts.set(f.name, { fingerprint, count: 0 });
        fresh = true;
      }
      const attempt = cardAttempts.get(f.name);
      if (attempt.count < CARD_READS_MAX) {
        ++attempt.count;
        asked.push(f);
      }
    }
    if (fresh) cardsLoad.wait = 0;
    const got = await Promise.all(asked.map(async (f) => [f, await loadCard(f.name, mine).catch(() => "none")]));
    if (mine !== visit || batch !== cardsLoad.batch) return;
    for (const [f, result] of got) if (result === "new" || result === "same") cardRows.set(f.name, JSON.stringify(f));
    if (got.some(([, result]) => result === "new")) render();
    if (got.some(([, result]) => result === "none") && cardsLoad.wait < 60_000) {
      cardsLoad.wait = Math.min(60_000, cardsLoad.wait ? cardsLoad.wait * 2 : 2_000);
      cardsLoad.timer = setTimeout(() => { if (mine === visit) loadCards().catch(() => {}); }, cardsLoad.wait);
    }
  }

  async function loadAccess() {
    const mine = visit;
    const request = ++accessRead;
    const fingerprint = sharingOf();
    const next = await h.api("GET", "/api/access").catch((e) => ({ error: e.message }));
    if (mine !== visit || request !== accessRead) return false;
    access = next;
    accessed = fingerprint;
    return true;
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

  function toggle(name, section) {
    if (expanded.get(name) === section) expanded.delete(name);
    else expanded.set(name, section);
    render();
    // its Reaches opened: what it reaches read again, so a key or a door
    // given it since the page was read shows
    const f = h.state.fragments.find((x) => x.name === name);
    if (expanded.get(name) === "reach" && f) {
      const mine = visit;
      readReach(f).then((read) => {
        if (mine !== visit) return;
        reach.set(name, read);
        render();
      });
    }
  }

  function titleEditor(f) {
    const name = f.name;
    const edit = editing.get(name);
    const wrap = h.el("div", "app-title-line");
    if (!edit) {
      const title = button("", "open", () => h.openApp(name));
      title.className = "app-row-title";
      h.named(title, name);
      wrap.append(title);
      if (edits(f)) {
        const rename = button("", "rename", () => {
          editing.set(name, { draft: h.titleOf(name), saving: false });
          render();
          const input = page.querySelector(`[data-fragment="${CSS.escape(name)}"] .app-name-input`);
          input?.focus();
          input?.select();
        });
        rename.className = "app-title-edit";
        rename.setAttribute("aria-label", `Rename ${h.titleOf(name)}`);
        rename.append(h.icon("rename"));
        wrap.append(rename);
      }
      return wrap;
    }
    const form = h.el("form", "app-name-form");
    const input = h.el("input", "app-name-input");
    input.value = edit.draft;
    input.maxLength = 120;
    input.required = true;
    input.setAttribute("aria-label", "App name");
    input.disabled = edit.saving;
    input.oninput = () => { edit.draft = input.value; };
    const cancel = button("Cancel", "rename-cancel", () => { editing.delete(name); render(); }, edit.saving);
    const save = button(edit.saving ? "Saving…" : "Save", "rename-save", null, edit.saving);
    save.type = "submit";
    form.append(input, save, cancel);
    form.onsubmit = async (event) => {
      event.preventDefault();
      if (!edit.draft.trim() || edit.saving) return;
      edit.saving = true;
      say(name, "");
      render();
      try {
        await h.retitle(name, edit.draft.trim(), "rename app");
        editing.delete(name);
        await h.reload();
      } catch (error) { say(name, error.message); }
      finally { edit.saving = false; render(); }
    };
    input.onkeydown = (event) => {
      if (event.key === "Escape" && !edit.saving) { event.preventDefault(); editing.delete(name); render(); }
    };
    wrap.append(form);
    return wrap;
  }

  function row(f) {
    const name = f.name;
    const doing = busy.get(name)?.kind;
    // its Reaches closes with its reach (its last key removed, its door unlinked)
    const section = expanded.get(name) === "reach" && !reaches(f) ? null : expanded.get(name);
    const item = h.el("article", "app-row");
    item.dataset.key = name;
    item.dataset.fragment = name;
    const art = button("", "open", () => h.openApp(name));
    art.className = "app-row-art";
    art.setAttribute("aria-label", `Open ${h.titleOf(name)}`);
    art.append(h.art(name));
    const text = h.el("div", "app-row-text");
    text.append(titleEditor(f));
    const address = addresses.get(name);
    if (address) {
      const url = h.el("a", "app-row-url");
      url.append(h.el("span", null, new URL(address).host), h.icon("external"));
      url.href = address;
      url.target = "_blank";
      url.rel = "noopener";
      text.append(url);
    }
    const about = [KIND[f.kind] ?? (f.kind === "app" ? "" : KIND.other), owns(f) ? "" : `Shared with you · ${ROLE[f.role] ?? f.role}`].filter(Boolean).join(" · ");
    if (about) text.append(h.el("span", "app-row-meta", about));
    if (owns(f)) text.append(whoLine(f));
    const reachSaid = reaches(f) && reachLine(f);
    if (reachSaid) text.append(reachSaid);
    const head = h.el("div", "app-row-head");
    head.append(art, text);
    const card = cards.get(name);
    if (card) {
      const preview = button("", "open", () => h.openApp(name));
      preview.className = "app-preview-open";
      preview.setAttribute("aria-label", `Open ${h.titleOf(name)}`);
      const img = h.el("img", "app-row-card");
      img.alt = "";
      img.src = card.url;
      preview.append(img);
      head.append(preview);
    }
    if (f.archived) head.append(button(doing === "archive" ? "Unarchiving…" : "Unarchive", "unarchive", () => unarchive(name), !!doing));
    else if (dockable(f)) {
      const dock = h.el("button", "app-dock");
      dock.type = "button";
      dock.dataset.action = "dock";
      dock.setAttribute("role", "switch");
      dock.setAttribute("aria-label", `Show ${h.titleOf(name)} in dock`);
      dock.setAttribute("aria-checked", String(f.dock !== false));
      dock.disabled = !!doing;
      if (doing === "dock") dock.setAttribute("aria-busy", "true");
      dock.append(h.el("span", "app-dock-track"), h.el("span", null, "Dock"));
      dock.onclick = () => setDock(name, f.dock === false);
      head.append(dock);
    }
    item.append(head);
    const tools = h.el("div", "app-row-tools");
    const tab = (label, kind) => {
      const b = button(label, kind, () => toggle(name, kind));
      b.setAttribute("aria-expanded", String(section === kind));
      return b;
    };
    if (!f.archived && edits(f)) tools.append(tab("Appearance", "appearance"));
    if (edits(f) && reaches(f)) tools.append(tab("Reaches", "reach"));
    tools.append(tab(owns(f) ? "Sharing" : "People", "sharing"));
    if (owns(f) && !f.archived) tools.append(tab("Address", "address"));
    item.append(tools);
    if (section) {
      const panel = h.el("div", "app-row-panel");
      if (section === "appearance") {
        const editor = h.el("div");
        editor.dataset.iconEditor = name;
        panel.append(editor);
        if (dockable(f) && !(owns(f) && f.sharing?.visibility === "members")) {
          const preview = h.el("section", "app-preview-editor");
          preview.append(h.el("h2", null, "Preview"));
          if (card) { const img = h.el("img", "app-preview-image"); img.alt = ""; img.src = card.url; preview.append(img); }
          preview.append(button(doing === "card" ? "Asking…" : shots.has(name) ? "Taking a new preview…" : "New preview", "card", () => newPreview(name), !!doing || shots.has(name)));
          panel.append(preview);
        }
      } else if (section === "sharing") {
        panel.dataset.shareEditor = name;
      } else if (section === "reach") {
        panel.append(reachPanel(f));
      } else if (section === "address") {
        panel.dataset.addressEditor = name;
        panel.append(h.el("p", "muted", "Loading address…"));
      }
      item.append(panel);
    }
    const shot = shots.get(name);
    if (shot || doing === "card") {
      const progress = h.el("div", "app-row-progress");
      progress.append(activity(shot ? "Taking a new preview…" : "Requesting preview…", shot?.started ?? busy.get(name).started));
      item.append(progress);
      item.setAttribute("aria-busy", "true");
    }
    const words = said.get(name);
    if (words) { const p = h.el("p", "app-row-said", words); p.setAttribute("role", "status"); item.append(p); }
    const edit = editing.get(name);
    item.dataset.shows = JSON.stringify([f, doing ?? null, words ?? null, card?.url ?? null, address ?? null, shots.has(name), section ?? null, edit ? [true, edit.saving] : null, item.querySelector(".app-row-access")?.textContent ?? null, reach.get(name) ?? null, confirming.get(name) ?? null]);
    item.dataset.art = artReadiness(item);
    return item;
  }

  function mountEditors() {
    for (const host of page.querySelectorAll("[data-icon-editor]")) {
      const name = host.dataset.iconEditor;
      if (!iconEditors.has(name)) iconEditors.set(name, createIconEditor(h, name, h.titleOf(name), h.state.fragments.find((f) => f.name === name), true));
      const editor = iconEditors.get(name).element;
      if (editor.parentElement !== host) host.replaceChildren(editor);
    }
    for (const host of page.querySelectorAll("[data-share-editor]")) {
      const name = host.dataset.shareEditor;
      if (!sharingFrames.has(name)) {
        const frame = h.el("iframe", "app-sharing-frame");
        frame.title = `Sharing ${h.titleOf(name)}`;
        frame.allow = "clipboard-write";
        frame.src = `/share/${encodeURIComponent(name)}`;
        sharingFrames.set(name, frame);
      }
      const frame = sharingFrames.get(name);
      if (frame.parentElement !== host) host.replaceChildren(frame);
    }
    for (const host of page.querySelectorAll("[data-address-editor]")) {
      const name = host.dataset.addressEditor;
      if (!addressEditors.has(name)) addressEditors.set(name, addressEditor(name));
      const editor = addressEditors.get(name);
      if (editor.parentElement !== host) host.replaceChildren(editor);
    }
  }

  function addressEditor(name) {
    const info = addressInfo.get(name);
    const box = h.el("div", "app-address-editor");
    box.append(h.el("h2", null, "Public address"));
    if (!info || info.error) {
      box.append(h.el("p", "muted", info?.error ?? "Loading address…"));
      return box;
    }
    if (info.address) {
      const link = h.el("a", "app-address-kept", new URL(info.address.url).host);
      link.href = info.address.url;
      link.target = "_blank";
      link.rel = "noopener";
      box.append(link);
      return box;
    }
    const form = h.el("form", "app-address-form");
    const field = h.el("label", "app-address-field");
    const input = h.el("input");
    input.setAttribute("aria-label", "Public address");
    input.autocomplete = "off";
    input.spellcheck = false;
    input.pattern = "[a-z0-9]+(?:-[a-z0-9]+)*";
    input.maxLength = info.maxLength;
    input.required = true;
    input.value = h.titleOf(name).toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, info.maxLength);
    field.append(input, h.el("span", null, info.suffix));
    const claim = button("Claim address", "claim-address", null);
    claim.type = "submit";
    const message = h.el("p", "app-address-said");
    message.setAttribute("role", "status");
    form.append(field, claim);
    box.append(form, h.el("p", "muted app-address-note", "First come, first served. This address stays with this app."), message);
    form.onsubmit = async (event) => {
      event.preventDefault();
      if (claim.disabled) return;
      claim.disabled = true;
      input.disabled = true;
      message.replaceChildren(activity("Claiming address…"));
      try {
        const out = await h.api("PUT", `/api/f/${seg(name)}/address`, { label: input.value });
        addressInfo.set(name, out);
        addresses.set(name, out.address.url);
        addressEditors.delete(name);
        render();
      } catch (error) {
        message.textContent = error.message;
        claim.disabled = false;
        input.disabled = false;
      }
    };
    return box;
  }

  addEventListener("message", (event) => {
    if (event.origin !== location.origin) return;
    for (const [name, frame] of sharingFrames) {
      if (event.source !== frame.contentWindow) continue;
      if (Number.isFinite(event.data?.height)) frame.style.height = `${Math.max(160, Math.min(1600, event.data.height))}px`;
      if (event.data?.share === "done") {
        expanded.delete(name);
        sharingFrames.delete(name);
        render();
        h.reload().then(() => loadAccess()).then(() => render()).catch(() => {});
      }
    }
  });

  function artReadiness(item) {
    const picture = item.querySelector(".app-row-art .app-picture");
    return JSON.stringify(picture ? [picture.hasAttribute("data-unread"), !!picture.querySelector("img")] : null);
  }

  // Leave connected children in place. Removing/reinserting an iframe's
  // ancestor destroys its document, even when the element itself is reused.
  function updateChildren(parent, next) {
    const kept = new Set(next);
    for (const child of [...parent.childNodes]) if (!kept.has(child)) child.remove();
    let cursor = parent.firstChild;
    for (const child of next) {
      if (child === cursor) cursor = cursor.nextSibling;
      else parent.insertBefore(child, cursor);
    }
  }

  // rows put in place by key, an element kept while it shows the same; the
  // retained art's readiness is read now, not only when its row was made
  function place(box, rows) {
    const was = new Map([...box.children].filter((e) => e.dataset.key).map((e) => [e.dataset.key, e]));
    const next = rows.map((r) => {
      const old = was.get(r.dataset.key);
      if (!old) return r;
      if (old.dataset.shows === r.dataset.shows && artReadiness(old) === r.dataset.art) return old;
      const panel = old.querySelector(":scope > [data-share-editor]");
      const replacement = r.querySelector(":scope > [data-share-editor]");
      if (panel && replacement && panel.dataset.shareEditor === replacement.dataset.shareEditor) {
        // Patch the header/controls around an open sharing view. Its
        // browsing context, typed invite and focus belong to this visit.
        updateChildren(old, [...r.childNodes].map((child) => child === replacement ? panel : child));
        old.dataset.shows = r.dataset.shows;
        old.dataset.art = r.dataset.art;
        if (r.hasAttribute("aria-busy")) old.setAttribute("aria-busy", r.getAttribute("aria-busy"));
        else old.removeAttribute("aria-busy");
        return old;
      }
      return r;
    });
    updateChildren(box, next);
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
    mountEditors();
  }

  async function setDock(name, dock) {
    const operation = beginBusy(name, "dock");
    say(name, "");
    render();
    try {
      await h.api("PUT", `/api/fragments/${seg(name)}/dock`, { dock });
      await h.reload();
    } catch (e) {
      say(name, e.message);
    } finally {
      endBusy(name, operation);
      render();
    }
  }

  async function unarchive(name) {
    const operation = beginBusy(name, "archive");
    say(name, "");
    render();
    try {
      await h.api("PUT", `/api/fragments/${seg(name)}/archived`, { archived: false });
      await h.reload();
    } catch (e) {
      say(name, e.message);
    } finally {
      endBusy(name, operation);
      render();
    }
  }

  // a shot asked for: waited for by its report (status's `page.at` from
  // when it was wanted on), then its card read again
  async function newPreview(name) {
    const mine = visit;
    const operation = beginBusy(name, "card");
    say(name, "");
    render();
    let asked;
    try {
      asked = await h.api("POST", `/api/f/${seg(name)}/card`);
    } catch (e) {
      if (mine === visit) say(name, e.message);
      return;
    } finally {
      endBusy(name, operation);
      if (mine === visit) render();
    }
    if (mine !== visit) return;
    shots.set(name, { since: asked.since, started: Date.now() });
    render();
    const until = Date.now() + SHOT_WAIT_MS;
    let done = false;
    // bounded: until the report comes, the page closes, or SHOT_WAIT_MS
    while (shown && mine === visit && Date.now() < until) {
      const status = await h.api("GET", `/api/f/${seg(name)}/status`).catch(() => null);
      if (status?.page && status.page.at >= asked.since) { done = true; break; }
      await new Promise((r) => setTimeout(r, 1_000));
    }
    if (mine !== visit) return;
    shots.delete(name);
    await loadCard(name, mine).catch(() => "same");
    if (mine !== visit) return;
    say(name, done ? "" : "The preview is still being taken.");
    render();
  }

  return {
    // shown at `/apps`: the list as it is, who has access, and each card
    async open() {
      shown = true;
      ++visit;
      // preview presentation is a visit's; a Dock or archive mutation is
      // still the one server write this app may have in flight
      for (const [name, operation] of busy) if (operation.kind === "card") busy.delete(name);
      shots.clear();
      page.replaceChildren();
      cardsLoad.wait = 0;
      render();
      const reading = loadCards(true).catch(() => {});
      const urls = loadAddresses(true);
      loadReach().catch(() => {});
      if (await loadAccess()) render();
      await reading;
      await urls;
    },
    close() {
      shown = false;
      ++visit;
      ++cardsLoad.batch;
      clearTimeout(cardsLoad.timer);
      shots.clear();
      for (const editor of iconEditors.values()) editor.dispose();
      iconEditors.clear();
      sharingFrames.clear();
      addressEditors.clear();
      editing.clear();
    },
    // the list changed: drawn again, who has access asked again when the
    // list says that changed, and new or changed apps' cards read
    async listed() {
      if (!shown) return;
      render();
      loadCards().catch(() => {});
      loadAddresses().catch(() => {});
      if (sharingOf() !== accessed && await loadAccess()) render();
    },
    // a share sheet closed: who has access, and any changed cards, asked again
    async shared() {
      if (!shown) return;
      render();
      loadCards().catch(() => {});
      if (await loadAccess()) render();
    },
  };
}
