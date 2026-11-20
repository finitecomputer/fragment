// A fragment's display name, kept by its cell (its face: docs/api.md, "An agent's
// name and picture"), set with no commit and no
// deploy. Its address stays put. Shared by Apps, chats and agents; the
// shell supplies its API and list.
export function createTitles({ $, api, byName, seg, titleOf, reload }) {
  // With `was`, only while its title is that (409 otherwise). An agent's
  // cell says its new name in its job (its SOUL.md) as well, in the same
  // request (members.rs `face_put`).
  async function retitle(name, title, was) {
    await api("PUT", `/api/f/${seg(name)}/face`, was === undefined ? { title } : { title, was });
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
    // as wide as it was: its words change, its size does not
    save.style.minWidth = `${save.offsetWidth}px`;
    save.textContent = "Saving…";
    error.hidden = true;
    try {
      await retitle(name, title);
      dialog.close();
      await reload();
    } catch (err) {
      error.textContent = err.message;
      error.hidden = false;
    } finally {
      saving = false;
      save.disabled = false;
      save.textContent = saveText;
      save.style.removeProperty("min-width");
    }
  };
  return { retitle, rename };
}
