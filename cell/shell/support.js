// Support uses the same private chat frames as the rest of the shell.
// Customer details are fetched only when a staff member opens the info control.
export function createSupport({ api, showPage, openChat, load, current, changed }) {
  const button = document.getElementById("support");
  const banner = document.getElementById("support-banner");
  const controls = document.getElementById("support-controls");
  let directory = null, page = null, reading = null, opening = false;
  const known = new Map();
  const node = (tag, text, cls) => {
    const e = document.createElement(tag);
    if (text) e.textContent = text;
    if (cls) e.className = cls;
    return e;
  };
  const email = () => {
    const a = node("a", "Email support@finite.computer");
    a.href = "mailto:support@finite.computer";
    return a;
  };
  const action = (text, fn, cls) => {
    const b = node("button", text, cls);
    b.type = "button";
    b.onclick = fn;
    return b;
  };
  function error(e) {
    page = null;
    banner.hidden = true;
    controls.hidden = true;
    showPage().replaceChildren(node("h1", "Contact support"), node("p", e.message), email(), action("Try again", () => open()));
  }
  async function info(t) {
    const dialog = document.getElementById("support-info");
    dialog.replaceChildren(node("h2", "Customer details"), node("p", "Loading…"), action("Close", () => dialog.close()));
    dialog.showModal();
    try {
      const v = await api("GET", `/api/support/${encodeURIComponent(t.person)}/info`);
      if (!dialog.open) return;
      const facts = node("dl");
      for (const [key, value] of [["Email", v.email || "Not available"], ["Organization", v.org?.name || "No organization"], ["Bot", (v.agents ?? []).map((a) => a.title || a.name || a.id).join(", ") || "No bot yet"]]) {
        facts.append(node("dt", key), node("dd", value));
      }
      dialog.replaceChildren(node("h2", "Customer details"), facts, email(), action("Close", () => dialog.close()));
    } catch (e) {
      dialog.replaceChildren(node("h2", "Details could not load"), node("p", e.message), action("Close", () => dialog.close()));
    }
  }
  function infoButton(t) {
    const b = action("ⓘ", () => info(t), "icon-button");
    b.setAttribute("aria-label", `Customer details for ${t.email || "this user"}`);
    b.title = "Customer details";
    return b;
  }
  function chatOpened(name) {
    page = null;
    const t = known.get(name);
    controls.replaceChildren();
    controls.hidden = !t || !directory?.staff;
    banner.hidden = !t || !!directory?.staff;
    banner.replaceChildren();
    if (!t) return;
    if (directory.staff) { controls.append(infoButton(t)); return; }
    banner.append(node("span", "Chat with the Finite team"));
    banner.append(email());
  }
  async function thread(t) {
    if (opening) return;
    opening = true;
    button.disabled = true;
    try {
      const v = await api("POST", t ? `/api/support/${encodeURIComponent(t.person)}` : "/api/support");
      await refresh();
      await load();
      openChat(v.name);
    } catch (e) { error(e); }
    finally { opening = false; button.disabled = false; }
  }
  function render() {
    if (!page || page.hidden) return;
    const empty = node("div", null, "support-empty");
    empty.append(node("span", "SUPPORT", "support-eyebrow"), node("h1", "You're all caught up"),
      node("p", "Customer conversations appear in your chat list. Open one to reply, just like any other chat."), email());
    page.replaceChildren(empty);
  }
  async function more() {
    if (!directory?.next) return;
    try {
      const v = await api("GET", `/api/support?after=${encodeURIComponent(directory.next)}`);
      for (const t of v.threads) known.set(t.name, t);
      const names = new Set(directory.threads.map(t => t.name));
      directory.threads.push(...v.threads.filter(t => !names.has(t.name)));
      directory.next = v.next;
      changed();
    } catch (e) { error(e); }
  }
  function refresh() {
    if (reading) return reading;
    reading = (async () => {
      const v = await api("GET", "/api/support");
      for (const t of v.threads) known.set(t.name, t);
      // Keep a staff member's older pages in view while incoming chats refresh.
      if (directory?.threads.length > 50) {
        const names = new Set(v.threads.map((t) => t.name));
        v.threads.push(...directory.threads.filter((t) => !names.has(t.name)));
        v.next = directory.next;
      }
      directory = v;
      changed();
      const unread = v.threads.reduce((n, t) => n + t.unread, 0);
      button.title = unread ? `Support · ${unread} unread` : "Support";
      button.classList.toggle("has-unread", unread > 0);
      if (page && !page.hidden) render();
      else chatOpened(current());
    })().finally(() => { reading = null; });
    return reading;
  }
  async function open() {
    try {
      await refresh();
      if (!directory?.staff) return thread();
      const threads = [...directory.threads].sort((a, b) => (b.at || 0) - (a.at || 0));
      const selected = threads.find(t => t.name === current()) || threads.find(t => t.unread) || threads[0];
      if (selected) return thread(selected);
      banner.hidden = true;
      page = showPage();
      render();
    } catch (e) { error(e); }
  }
  button.onclick = open;
  // This also finds incoming conversations when no fragment has been opened yet.
  setInterval(() => { if (directory && !document.hidden) refresh().catch(() => {}); }, 15000);
  return { open, refresh, chatOpened, more,
    hasMore: () => !!directory?.staff && !!directory.next,
    threads: () => directory?.staff ? directory.threads : [],
    title: name => directory?.staff ? known.get(name)?.email : null,
    openThread: name => thread(known.get(name)),
    close: () => { page = null; banner.hidden = true; controls.hidden = true; } };
}
