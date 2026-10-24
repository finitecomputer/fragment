// An app's icon (docs/api.md, Icons): its own, when its editors made or
// uploaded one (its list row's `picture`), read with this page's session
// into a blob URL as its card is, and shown as it is: an object on
// nothing, never on a tile. An app without one, and one whose icon is
// still loading, shows the shell's glyph in its colour (`appIcon`).
import { appIcon } from "./icons.js";

const SHELL = { "x-fragment-shell": "1" };
// an icon's address names its bytes (`?v=`), so each is read once a page
const read = new Map();
function blobUrl(path) {
  if (!read.has(path)) {
    read.set(path, fetch(path, { headers: SHELL, credentials: "same-origin" })
      .then((r) => (r.ok ? r.blob() : null))
      .then((b) => (b ? URL.createObjectURL(b) : null))
      .catch(() => null));
  }
  return read.get(path);
}

export function appArt(f, label) {
  const glyph = appIcon(label);
  if (!f?.picture || f.kind === "agent") return glyph;
  const art = document.createElement("span");
  art.className = "app-picture";
  art.setAttribute("aria-hidden", "true");
  art.append(glyph);
  blobUrl(f.picture).then((url) => {
    if (!url) return;
    const img = document.createElement("img");
    img.alt = "";
    img.src = url;
    glyph.replaceWith(img);
  });
  return art;
}

// The icon sheet: make icons (an optional description), pick one, upload
// one, or remove it. `h`: the shell's `api`, `notice` and `reload` (its
// list again, which carries the new icon to the dock), and the app's
// name, title and row.
export function openIconSheet(h, name, title, f) {
  const path = `/api/f/${encodeURIComponent(name)}/icon`;
  const dialog = document.createElement("dialog");
  dialog.className = "icon-sheet";
  dialog.setAttribute("aria-label", `${title} icon`);
  const head = document.createElement("h2");
  head.textContent = "Icon";
  const current = document.createElement("div");
  current.className = "icon-current";
  const showCurrent = (row) => current.replaceChildren(appArt(row, title));
  showCurrent(f);

  const form = document.createElement("form");
  form.className = "icon-make";
  const hint = document.createElement("input");
  hint.name = "hint";
  hint.maxLength = 200;
  hint.placeholder = "Describe it (optional)";
  hint.autocomplete = "off";
  const make = document.createElement("button");
  make.type = "submit";
  make.textContent = "Make icons";
  form.append(hint, make);

  const picks = document.createElement("div");
  picks.className = "icon-picks";
  const said = document.createElement("p");
  said.className = "icon-said";
  said.hidden = true;
  const say = (text) => { said.textContent = text; said.hidden = !text; };

  const file = document.createElement("input");
  file.type = "file";
  file.accept = "image/png,image/jpeg,image/webp,image/gif";
  file.hidden = true;
  const upload = document.createElement("button");
  upload.type = "button";
  upload.textContent = "Upload…";
  const remove = document.createElement("button");
  remove.type = "button";
  remove.textContent = "Remove";
  remove.hidden = !f?.picture;
  const done = document.createElement("button");
  done.type = "button";
  done.textContent = "Done";
  const actions = document.createElement("div");
  actions.className = "icon-actions";
  actions.append(upload, remove, done, file);

  // keeps `body` (the icon's bytes) as the icon, then the list again
  async function keep(body) {
    const r = await fetch(path, { method: "PUT", headers: SHELL, body, credentials: "same-origin" });
    const out = await r.json().catch(() => ({}));
    if (!r.ok) throw new Error(out.message || r.statusText);
    const rows = await h.reload();
    const row = rows?.find((x) => x.name === name) ?? { ...f, picture: out.icon };
    showCurrent(row);
    remove.hidden = false;
  }

  form.onsubmit = async (e) => {
    e.preventDefault();
    make.disabled = true;
    make.textContent = "Making…";
    picks.replaceChildren();
    say("");
    try {
      const out = await h.api("POST", `${path}/make`, { hint: hint.value.trim() || undefined });
      for (const c of out.candidates) {
        const pick = document.createElement("button");
        pick.type = "button";
        pick.className = "icon-pick";
        pick.title = "Use this icon";
        const img = document.createElement("img");
        img.alt = out.subject;
        img.src = `data:image/png;base64,${c.png}`;
        pick.append(img);
        pick.onclick = async () => {
          const bytes = Uint8Array.from(atob(c.png), (ch) => ch.charCodeAt(0));
          for (const b of picks.children) b.classList.toggle("picked", b === pick);
          try { await keep(bytes); } catch (err) { h.notice("The icon was not kept", err.message); }
        };
        picks.append(pick);
      }
      if (!out.candidates.length) say("None came out right. Try again, or describe it.");
    } catch (err) {
      say(err.message);
    } finally {
      make.disabled = false;
      make.textContent = "Make icons";
    }
  };
  upload.onclick = () => file.click();
  file.onchange = async () => {
    const chosen = file.files[0];
    file.value = "";
    if (!chosen) return;
    try { await keep(chosen); } catch (err) { h.notice("The icon was not uploaded", err.message); }
  };
  remove.onclick = async () => {
    try {
      await h.api("DELETE", path);
      await h.reload();
      showCurrent({ ...f, picture: undefined });
      remove.hidden = true;
    } catch (err) {
      h.notice("The icon was not removed", err.message);
    }
  };
  done.onclick = () => dialog.close();
  dialog.onclick = (e) => { if (e.target === dialog) dialog.close(); };
  dialog.onclose = () => dialog.remove();
  dialog.append(head, current, form, said, picks, actions);
  document.body.append(dialog);
  dialog.showModal();
  hint.focus();
}
