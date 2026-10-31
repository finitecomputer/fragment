// An app's icon (docs/api.md, Icons): its own, when its editors made or
// uploaded one (its list row's `picture`), read with this page's session
// into a blob URL as its card is, and shown as it is: an object on
// nothing, never on a tile. An app without one, and one whose icon is
// still loading or did not load, shows the shell's glyph in its colour
// (`appIcon`).
import { appIcon } from "./icons.js";
import { activity } from "./activity.js";

const SHELL = { "x-fragment-shell": "1" };
// An icon's address names its bytes (`?v=`), so one read is kept for the
// page's life (`loaded`: its blob URL). One that failed is forgotten: a
// draw at least RETRY_MS later reads it again, so a blip heals at the next
// draw, and an address that keeps failing is asked at most that often.
const RETRY_MS = 2000;
const loaded = new Map();
const reading = new Map();
const failedAt = new Map();
function read(path) {
  if (!reading.has(path)) {
    reading.set(path, fetch(path, { headers: SHELL, credentials: "same-origin" })
      .then((r) => {
        if (!r.ok) throw new Error(r.statusText);
        return r.blob();
      })
      .then((b) => {
        const url = URL.createObjectURL(b);
        loaded.set(path, url);
        return url;
      })
      .catch(() => {
        failedAt.set(path, Date.now());
        return null;
      })
      .finally(() => reading.delete(path)));
  }
  return reading.get(path);
}

function picture(url) {
  const img = document.createElement("img");
  img.alt = "";
  img.src = url;
  return img;
}

export function appArt(f, label) {
  const glyph = appIcon(label);
  if (!f?.picture || f.kind === "agent") return glyph;
  const art = document.createElement("span");
  art.className = "app-picture";
  art.setAttribute("aria-hidden", "true");
  // which icon, and whether it did not load, on the node itself: a row
  // drawn for another icon, or to read a failed one again, is not the row
  // shown (shell.js `patch` keeps a row only when it is the same)
  art.dataset.picture = f.picture;
  const url = loaded.get(f.picture);
  if (url) {
    art.append(picture(url));
    return art;
  }
  art.append(glyph);
  const unread = () => { art.dataset.unread = ""; };
  if (failedAt.has(f.picture) && Date.now() - failedAt.get(f.picture) < RETRY_MS) unread();
  else read(f.picture).then((u) => (u ? glyph.replaceWith(picture(u)) : unread()));
  return art;
}

// One write of an app's icon at a time (a pick, an upload, a removal),
// whichever sheet asked it: until it answers, every sheet open on that app
// offers none, so none can land after it out of order. Its answer is shown
// in the sheet open on the app then, if one is, and nowhere else.
const writing = new Set();
const sheets = new Set();

// The icon sheet: make icons (an optional description), pick one, upload
// one, or remove it. `h`: the shell's `api` and `reload` (its list again,
// which carries the new icon to the dock), and the app's name, title and
// row. A pick is shown kept once the platform has kept it.
export function createIconEditor(h, name, title, f, inline = false) {
  const path = `/api/f/${encodeURIComponent(name)}/icon`;
  const dialog = document.createElement(inline ? "section" : "dialog");
  dialog.className = inline ? "icon-editor" : "icon-sheet";
  dialog.setAttribute("aria-label", `${title} icon`);
  const head = document.createElement("h2");
  head.textContent = inline ? "App icon" : "Icon";
  const current = document.createElement("div");
  current.className = "icon-current";

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
  const progress = document.createElement("div");
  progress.className = "icon-progress";
  progress.hidden = true;
  const said = document.createElement("p");
  said.className = "icon-said";
  said.setAttribute("role", "alert");
  said.hidden = true;
  const say = (text) => { said.textContent = text; said.hidden = !text; };

  const file = document.createElement("input");
  file.type = "file";
  file.accept = "image/png,image/jpeg,image/webp,image/gif";
  file.hidden = true;
  const button = (cls, text) => {
    const b = document.createElement("button");
    b.type = "button";
    b.className = cls;
    b.textContent = text;
    return b;
  };
  const upload = button("icon-upload", "Upload…");
  const remove = button("icon-remove", "Remove");
  const done = button("icon-done", "Done");
  done.hidden = inline;
  const actions = document.createElement("div");
  actions.className = "icon-actions";
  actions.append(upload, remove, done, file);

  // the icon as the platform last answered: the app's row, and this
  // sheet's pick that is it, when one is
  let row = f;
  let kept = null;
  const showCurrent = () => {
    current.replaceChildren(appArt(row, name.replace(/--[a-km-np-z2-9]{4}$/, "")));
    remove.hidden = !row?.picture;
  };
  const controls = () => {
    const busy = writing.has(name);
    for (const b of [upload, remove, ...picks.children]) b.disabled = busy;
    for (const b of picks.children) b.classList.toggle("picked", b === kept);
  };
  const sheet = {
    name,
    controls,
    answered(out, by) {
      if (out.row) {
        row = out.row;
        kept = by;
        showCurrent();
      } else {
        say(out.error);
      }
      controls();
    },
  };

  // `ask` writes the icon and answers the row as it is then; `control`,
  // what asked, busy until it answers; `failed`, what a failure says
  async function change(control, failed, ask) {
    if (writing.has(name)) return;
    writing.add(name);
    say("");
    control.setAttribute("aria-busy", "true");
    for (const s of sheets) if (s.name === name) s.controls();
    let out;
    try {
      out = { row: await ask() };
      h.reload().catch(() => {});
    } catch (err) {
      out = { error: `${failed}: ${err.message}` };
    }
    writing.delete(name);
    control.removeAttribute("aria-busy");
    for (const s of sheets) if (s.name === name) s.answered(out, s === sheet ? control : null);
  }
  // `body` (an image's bytes) kept as the icon
  async function put(body) {
    const out = await h.api("PUT", path, body);
    return { ...row, picture: out.icon };
  }

  form.onsubmit = async (e) => {
    e.preventDefault();
    make.disabled = true;
    make.textContent = "Making…";
    form.setAttribute("aria-busy", "true");
    progress.replaceChildren(activity("Making icons…"));
    progress.hidden = false;
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
        pick.onclick = () => change(pick, "The icon was not kept", () => put(Uint8Array.from(atob(c.png), (ch) => ch.charCodeAt(0))));
        picks.append(pick);
      }
      controls();
      if (!out.candidates.length) say("None came out right. Try again, or describe it.");
    } catch (err) {
      say(err.message);
    } finally {
      progress.hidden = true;
      progress.replaceChildren();
      form.removeAttribute("aria-busy");
      make.disabled = false;
      make.textContent = "Make icons";
    }
  };
  upload.onclick = () => file.click();
  file.onchange = () => {
    const chosen = file.files[0];
    file.value = "";
    if (chosen) change(upload, "The icon was not uploaded", () => put(chosen));
  };
  remove.onclick = () => change(remove, "The icon was not removed", async () => {
    await h.api("DELETE", path);
    return { ...row, picture: undefined };
  });
  const dispose = () => {
    sheets.delete(sheet);
    dialog.remove();
  };
  if (!inline) {
    done.onclick = () => dialog.close();
    dialog.onclick = (e) => { if (e.target === dialog) dialog.close(); };
    dialog.onclose = dispose;
  }
  showCurrent();
  sheets.add(sheet);
  controls();
  dialog.append(head, current, form, progress, said, picks, actions);
  return { element: dialog, dispose };
}

export function openIconSheet(h, name, title, f) {
  const { element } = createIconEditor(h, name, title, f);
  document.body.append(element);
  element.showModal();
  element.querySelector("input[name=hint]").focus();
}
