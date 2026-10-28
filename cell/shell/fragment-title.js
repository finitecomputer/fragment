// A fragment's display name, kept in its manifest. Its address stays put.
// Shared by Apps, chats and agents; the shell supplies its API and list.
export function createTitles({ $, api, seg, byName, titleOf, reload }) {
  async function retitle(name, title, message) {
    const manifest = await api("GET", `/api/f/${seg(name)}/manifest`);
    manifest.meta = { ...(manifest.meta ?? {}), title };
    await api("POST", `/api/f/${seg(name)}/files`, { message, files: [{ path: "fragment.json", text: JSON.stringify(manifest, null, 2) + "\n" }] });
    await api("POST", `/api/f/${seg(name)}/deploy`, {});
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
