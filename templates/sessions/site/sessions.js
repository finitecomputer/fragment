// The session archive's page: its conversations, newest first, a kind a
// list, read from the archive's indexes (`__file`, templates/sessions/README.md).
const KINDS = [["chat", "Chats"], ["cron", "Scheduled runs"], ["subagent", "Side tasks"]];

const el = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
};

// a kind's index, or none
const index = async (kind) => {
  const r = await fetch(`__file?path=${encodeURIComponent(`index/${kind}.json`)}`, { credentials: "same-origin" }).catch(() => null);
  if (!r?.ok) return [];
  const v = await r.json().catch(() => null);
  return Array.isArray(v?.sessions) ? v.sessions : [];
};

const day = (at) => (at ? new Date(at).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" }) : "");

async function main() {
  window.addEventListener("message", (e) => {
    if (e.data?.fragment === "theme" && (e.data.mode === "light" || e.data.mode === "dark")) document.documentElement.dataset.theme = e.data.mode;
  });
  const lists = await Promise.all(KINDS.map(([kind]) => index(kind)));
  const box = document.getElementById("kinds");
  if (lists.every((l) => !l.length)) {
    box.replaceChildren(el("p", "muted", "No conversations yet."));
    return;
  }
  box.replaceChildren(...KINDS.map(([, heading], i) => [heading, lists[i]]).filter(([, list]) => list.length).map(([heading, list]) => {
    const section = el("section");
    const items = el("ul");
    items.append(...list.map((e) => {
      const li = el("li");
      li.append(el("strong", null, e.title || "Untitled"), el("small", null, [day(e.ended_at ?? e.started_at), e.preview].filter(Boolean).join(" · ")));
      return li;
    }));
    section.append(el("h2", null, heading), items);
    return section;
  }));
}

main();
