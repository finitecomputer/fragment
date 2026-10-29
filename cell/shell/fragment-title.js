// A fragment's display name, kept in its manifest. Its address stays put.
// Shared by Apps, chats and agents; the shell supplies its API and list.
export function createTitles({ $, api, seg, byName, titleOf, reload, heard, ApiError }) {
  async function retitle(name, title, message) {
    const manifest = await api("GET", `/api/f/${seg(name)}/manifest`);
    const old = manifest.meta?.title;
    manifest.meta = { ...(manifest.meta ?? {}), title };
    const files = [{ path: "fragment.json", text: JSON.stringify(manifest, null, 2) + "\n" }];
    // an agent's job says its new name where it said an old one, in the same
    // commit: its name at main, and the one the shell's first job gave it
    if (byName(name)?.kind === "agent") {
      const job = await jobOf(name);
      const renamed = job === null ? null : jobRenamed(job, [old, jobFirstName(job)], title);
      if (renamed !== null) files.push({ path: JOB, text: renamed });
    }
    await api("POST", `/api/f/${seg(name)}/files`, { message, files });
    await api("POST", `/api/f/${seg(name)}/deploy`, {});
  }
  // An agent's job (its SOUL.md) as its owner's rename leaves it (docs/api.md,
  // "An agent's name and picture"; `fragment_core::face::job_renamed`, which
  // `fragment agent --name` runs, is the same rule): each whole-word mention
  // of an old name (case and all; whole when no letter, digit or `_` touches
  // it) said as the new one, longest first, in one pass; null when there is
  // nothing to rewrite, or the job is past JOB_RENAME_MAX bytes before or
  // after. Asked again with the name it has now, it changes nothing.
  const JOB = "SOUL.md";
  const JOB_RENAME_MAX = 64 * 1024;
  const utf8Bytes = (s) => new TextEncoder().encode(s).length;
  function jobRenamed(job, olds, name) {
    name = name.trim();
    const words = [...new Set(olds.map((o) => (typeof o === "string" ? o.trim() : "")).filter((o) => o && o !== name))].sort((a, b) => b.length - a.length);
    if (!name || !words.length || utf8Bytes(job) > JOB_RENAME_MAX) return null;
    const escaped = words.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
    const mention = new RegExp(`(?<![\\p{Alphabetic}\\p{N}_])(?:${escaped.join("|")})(?![\\p{Alphabetic}\\p{N}_])`, "gu");
    let changed = false;
    const out = job.replace(mention, () => ((changed = true), name));
    return changed && utf8Bytes(out) <= JOB_RENAME_MAX ? out : null;
  }
  // The name the shell's first job gave its agent until 2026-10-16: "You are
  // <name>, <owner>'s default agent: …" on its first line (the owner holds no
  // ", "); null for any other (`fragment_core::face::job_first_name`).
  function jobFirstName(job) {
    const line = job.split("\n", 1)[0];
    if (!line.startsWith("You are ")) return null;
    const end = line.indexOf("'s default agent: ");
    const said = end < 0 ? "" : line.slice("You are ".length, end);
    const name = said.lastIndexOf(", ") < 0 ? "" : said.slice(0, said.lastIndexOf(", "));
    return name.trim() && [...name].length <= 120 ? name : null;
  }
  // an agent's job at main, as text; null when it has none
  async function jobOf(agent) {
    const r = await fetch(`/api/f/${seg(agent)}/file?path=${JOB}`, { headers: { "x-fragment-shell": "1" }, credentials: "same-origin" });
    heard(r);
    if (r.status === 404) return null;
    if (!r.ok) throw new ApiError(r.status, "", `Its job did not read (${r.status}): nothing was renamed`);
    return r.text();
  }

  const dialog = $("rename-agent");
  const form = $("rename-form");
  const input = $("agent-name");
  const error = $("rename-error");
  const save = form.querySelector("button[type=submit]");
  const saveText = save.textContent;
  let renaming = null;
  let saving = false;
  function rename(name) {
    if (saving) return;
    renaming = name;
    const kind = byName(name)?.kind;
    const noun = kind === "agent" ? "agent" : kind === "chat" ? "chat" : "app";
    $("rename-title").textContent = noun === "agent" ? "Name your agent" : noun === "chat" ? "Name this chat" : "Rename app";
    $("rename-label").textContent = noun === "agent" ? "Agent name" : noun === "chat" ? "Chat name" : "App name";
    input.maxLength = noun === "app" ? 120 : 48;
    input.value = titleOf(name);
    error.hidden = true;
    dialog.showModal();
    input.select();
  }
  $("rename-cancel").onclick = () => dialog.close();
  form.onsubmit = async (event) => {
    event.preventDefault();
    const title = input.value.trim();
    const name = renaming;
    if (!title || !name || saving) return;
    saving = true;
    save.disabled = true;
    save.textContent = "Saving…";
    error.hidden = true;
    try {
      await retitle(name, title, "rename");
      dialog.close();
      await reload();
    } catch (err) {
      error.textContent = err.message;
      error.hidden = false;
    } finally {
      saving = false;
      save.disabled = false;
      save.textContent = saveText;
    }
  };
  return { retitle, rename };
}
