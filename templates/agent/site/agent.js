// The agent's page: its name (the fragment's title), its picture
// (`avatar.png`, else its image in its colour, by its identity as the shell
// and its chats choose it), the model it runs (`runs`, below), its job
// (`SOUL.md`) and the skills in its repo, read from the fragment's own
// files (`__file`, `__files`). Its name and picture are changed from
// Settings, or by the agent (`fragment agent`: docs/api.md, "An agent's
// name and picture").
import { colorOf } from "./agent-color.js";

// This fragment's name, at the start of its host (docs/api.md, Names).
const NAME_AT = /^[a-z0-9](?:-?[a-z0-9])*--[a-km-np-z2-9]{4}/;
// Identities one `__people` call names (the platform's limit).
const PEOPLE_MAX = 64;

const text = async (path) => {
  const r = await fetch(`__file?path=${encodeURIComponent(path)}`, { credentials: "same-origin" });
  return r.ok ? r.text() : null;
};
const json = (path) => fetch(path, { headers: { accept: "application/json" }, credentials: "same-origin" }).then((r) => (r.ok ? r.json() : null)).catch(() => null);

// Its identity: of its agent members (`__members`; once its computer took
// it, it is an editor of its own fragment), the one made from this
// fragment (`__people`'s `fragment`); none before a computer takes it.
async function identity() {
  const here = location.hostname.match(NAME_AT)?.[0];
  const agents = ((await json("__members"))?.members ?? []).filter((m) => m.kind === "agent").map((m) => m.principal).slice(0, PEOPLE_MAX);
  if (!here || !agents.length) return null;
  const people = await json(`__people?${agents.map((id) => `id=${encodeURIComponent(id)}`).join("&")}`);
  return agents.find((id) => people?.profiles?.[id]?.fragment === here) ?? null;
}

// The model it runs, by its id, as its owner's shell says their computer
// runs it (its `agents` answer, docs/api.md, The shell): the model of a
// provider its owner connected (`agent.json`'s `model: {provider, id}`)
// while the computer gives it that provider's models (`modelProviders`),
// else its tier's (`medium`, else the default `cheap`: `high` is off,
// decision 23), as the deployment names it (`models`). Without that answer
// (opened outside its owner's shell), or with the agent on no computer of
// theirs, it names none: `agent.json` alone does not say what runs
// (docs/computers.md, "An agent's own model").
function runs(settings, me, models) {
  if (!me) return "";
  const own = settings.model;
  if (own?.provider && own?.id && me.modelProviders?.includes(own.provider)) return `${own.id} (${own.provider})`;
  return models?.[settings.tier === "medium" ? "medium" : "cheap"] ?? "";
}

async function main() {
  let settings = null, roster = null;
  const here = location.hostname.match(NAME_AT)?.[0];
  // Its name (docs/api.md, "An agent's name and picture"): the Open Graph
  // title the platform puts on its page, its name as every reader shows it
  // (its title, trimmed; none, or only spaces, its label:
  // `fragment_core::face::agent_name`), whether it runs on a computer yet
  // or not; then, framed by its owner's shell, as the shell says it, again
  // on any change (a rename while it is open)
  let shown = null;
  // Its job (SOUL.md), read again on a rename while it is open: its owner's
  // rename says the new name there too (docs/api.md, "An agent's name and
  // picture"), so the job shown never names it by an old one
  const showJob = async () => {
    const soul = await text("SOUL.md");
    const box = document.getElementById("soul");
    box.textContent = soul?.trim() || "No job written down yet.";
    box.classList.toggle("muted", !soul);
  };
  const named = (name) => {
    if (shown !== null && shown !== name) showJob();
    shown = name;
    document.title = name;
    document.getElementById("name").textContent = name;
  };
  const og = document.querySelector('meta[property="og:title"]')?.content?.trim();
  named(og || (here ?? location.hostname).split("--")[0] || "Agent");
  const model = () => {
    if (settings && roster) document.getElementById("tier").textContent = runs(settings, roster.agents.find((a) => a.fragment === here), roster.models);
  };
  window.addEventListener("message", (e) => {
    if (e.data?.fragment === "theme" && (e.data.mode === "light" || e.data.mode === "dark")) document.documentElement.dataset.theme = e.data.mode;
    // its owner's shell, framing it: their agents, again on any change
    if (e.source === window.parent && e.data?.fragment === "agents" && Array.isArray(e.data.agents)) {
      roster = e.data;
      const me = roster.agents.find((a) => a.fragment === here);
      if (typeof me?.title === "string" && me.title.trim()) named(me.title.trim());
      model();
    }
  });
  if (window.parent !== window) window.parent.postMessage({ fragment: "agents?" }, "*");
  const who = identity();
  try {
    settings = JSON.parse((await text("agent.json")) || "{}");
  } catch {}
  // anything but an object names the default tier, as its computer reads it
  if (!settings || typeof settings !== "object") settings = {};
  model();
  const avatar = document.getElementById("avatar");
  avatar.style.setProperty("--agent-color", colorOf(await who));
  const picture = new Image();
  picture.alt = "";
  picture.onload = () => {
    avatar.classList.add("picture");
    avatar.replaceChildren(picture);
  };
  picture.src = "__file?path=avatar.png";
  await showJob();
  const files = (await json("__files")) ?? { files: [] };
  const skills = [...new Set(files.files.map((f) => f.path).filter((p) => p.startsWith("skills/") && p.endsWith("/SKILL.md")).map((p) => p.split("/")[1]))];
  if (skills.length) {
    const list = document.getElementById("skills");
    list.replaceChildren(...skills.map((s) => Object.assign(document.createElement("li"), { textContent: s })));
  }
}

// Its routines: the scheduled records on its own `tasks` that ask it
// something (`{kind: "routine", text, chat?}`: docs/chat-records.md; no
// chat, its owner's home as it fires), as
// the platform keeps them (`__schedules`, docs/api.md "Scheduled records":
// its viewers', as every fragment's). Each says when in words, the next
// time in this page's own clock, the chat it is done in, and what it is
// asked. Read only: its owner asks the agent, and the agent adds them
// (`fragment schedule add --routine`).
async function routines() {
  const list = ((await json("__schedules"))?.schedules ?? []).filter((s) => s.channel === "tasks" && s.record?.kind === "routine");
  if (!list.length) return;
  const rows = list.map((s) => {
    const next = s.nextAt ? `next ${new Date(s.nextAt).toLocaleString()}` : "paused";
    const where = typeof s.record.chat === "string" && s.record.chat ? `in ${s.record.chat}` : "at home";
    const when = Object.assign(document.createElement("p"), { className: "when", textContent: `${s.said} · ${where} · ${next}` });
    const item = document.createElement("li");
    item.append(when, Object.assign(document.createElement("p"), { textContent: s.record.text }));
    return item;
  });
  document.getElementById("routines").replaceChildren(...rows);
  document.getElementById("routines-section").hidden = false;
}

main();
routines();
