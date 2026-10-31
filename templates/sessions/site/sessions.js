// The session archive's page (templates/sessions/README.md): with no `s`,
// its conversations, newest first, chats shown and scheduled runs and side
// tasks folded away; with `?s=<key>`, that conversation, read-only, in a
// chat's look: what the person and the agent said, and the agent's tool
// calls, their results and its reasoning folded into steps, as a chat
// folds them. Everything is read from the archive's own files (`__file`).
// Framed by the shell, a conversation offers Continue: the shell asks with
// which of the person's agents, and makes a chat of it with that one (or
// opens the chat it made before).
import { renderMarkdown } from "./markdown.js";
import { icon } from "./icons.js";

const KINDS = [["chat", "Chats"], ["cron", "Scheduled runs"], ["subagent", "Side tasks"]];
const KIND_ONE = { chat: "Chat", cron: "Scheduled run", subagent: "Side task" };
// a conversation's key, as the archive files one (fragment_core::session::key)
const KEY = /^[a-z][a-z0-9]{0,31}-[A-Za-z0-9._-]{1,140}$/;

// the shell framing this page: its origin, from the theme it sends
let shell = null;
const toShell = (message) => shell && window.parent.postMessage({ fragment: "session-archive", ...message }, shell);

const el = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
};

async function file(path) {
  const r = await fetch(`__file?path=${encodeURIComponent(path)}`, { credentials: "same-origin" }).catch(() => null);
  if (!r?.ok) return null;
  return r.json().catch(() => null);
}

const day = (at) => (at ? new Date(at).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" }) : "");
const when = (from, to) => (day(from) && day(to) && day(from) !== day(to) ? `${day(from)} – ${day(to)}` : day(to || from));

// ---- the list ----
function row(e) {
  const li = el("li");
  const a = el("a");
  a.href = `?s=${encodeURIComponent(e.key)}`;
  a.append(el("strong", null, e.title || "Untitled"), el("small", null, [day(e.ended_at ?? e.started_at), e.preview].filter(Boolean).join(" · ")));
  li.append(a);
  return li;
}

async function list(page) {
  const lists = await Promise.all(KINDS.map(([kind]) => file(`index/${kind}.json`).then((v) => (Array.isArray(v?.sessions) ? v.sessions : []))));
  const head = el("header", "archive-head");
  head.append(el("h1", null, "Session Archive"));
  if (lists.every((l) => !l.length)) {
    head.append(el("p", null, "No conversations yet."));
    page.replaceChildren(head);
    return;
  }
  const out = [head];
  KINDS.forEach(([kind, heading], i) => {
    const items = lists[i];
    if (!items.length) return;
    const ul = el("ul", "sessions");
    ul.append(...items.map(row));
    if (kind === "chat") return out.push(ul);
    const d = el("details", "kind");
    d.append(el("summary", null, `${heading} (${items.length})`), ul);
    out.push(d);
  });
  page.replaceChildren(...out);
}

// ---- a conversation ----
// What a run of the agent's work folds into: its steps, each one line (the
// tool and what it was given, or Thought), opening to the whole of it.
function steps(list) {
  const d = el("details", "tools");
  const summary = el("summary");
  summary.append(el("span", null, `${list.length} ${list.length === 1 ? "step" : "steps"}`), icon("chevron", "chev"));
  d.append(summary);
  for (const s of list) {
    const step = el("div", "step");
    const line = el("button", "step-name");
    line.type = "button";
    const label = el("span", "step-label");
    label.append(el("span", "step-verb", s.verb));
    if (s.args) label.append(el("span", "step-args", s.args));
    line.append(icon(s.verb === "Thought" ? "brain" : "wrench"), label);
    const detail = el("div", "step-detail");
    detail.hidden = true;
    for (const [text, cls] of [[s.args, "step-call"], [s.detail, ""], [s.result, ""]]) if (text) detail.append(el("pre", cls, text));
    line.onclick = () => { detail.hidden = !detail.hidden; };
    step.append(line, detail);
    d.append(step);
  }
  return d;
}

function folded(title, text) {
  const d = el("details", "tools");
  const summary = el("summary");
  summary.append(el("span", null, title), icon("chevron", "chev"));
  const detail = el("div", "step-detail");
  detail.append(el("pre", null, text));
  d.append(summary, detail);
  return d;
}

// a message's files, as the archive keeps them (`media/<sha256>.<ext>`)
const MEDIA = /^media\/[0-9a-f]{64}\.[a-z0-9]{1,8}$/;
const filesOf = (m) => (Array.isArray(m.attachments) ? m.attachments.filter((a) => typeof a?.path === "string" && MEDIA.test(a.path)) : []);

// a file a message held: a picture shown, anything else to open
function fileNode(a) {
  const src = `__file?path=${encodeURIComponent(a.path)}`;
  if (typeof a.media_type === "string" && a.media_type.startsWith("image/")) {
    const img = el("img", "archive-file");
    img.src = src;
    img.alt = a.name ?? "";
    img.loading = "lazy";
    return img;
  }
  const link = el("a", "archive-file", a.name || "File");
  link.href = src;
  link.download = a.name || "";
  return link;
}

function said(role, text, files = []) {
  const m = el("div", `msg ${role === "user" ? "user mine" : "agent"}`);
  if (role === "user") {
    if (text) m.append(el("div", "bubble", text));
  } else if (text) {
    const md = el("div", "md");
    md.append(renderMarkdown(text));
    m.append(md);
  }
  m.append(...files.map(fileNode));
  return m;
}

function conversation(c) {
  const out = [];
  let work = [];
  let lastDay = "";
  const flush = () => {
    if (work.length) out.push(steps(work));
    work = [];
  };
  const dated = (at) => {
    const d = day(at);
    if (d && d !== lastDay) {
      flush();
      if (lastDay) out.push(el("div", "day", d));
      lastDay = d;
    }
  };
  for (const m of c.messages ?? []) {
    dated(m.at);
    const text = typeof m.text === "string" && m.text.trim() ? m.text : "";
    const calls = Array.isArray(m.tool_calls) ? m.tool_calls : [];
    const files = filesOf(m);
    if (m.summary) {
      flush();
      out.push(folded("Earlier, summarized", text));
    } else if (m.role === "user" && (text || files.length) && !calls.length) {
      flush();
      out.push(said("user", text, files));
    } else if (m.role === "assistant") {
      if (m.reasoning) work.push({ verb: "Thought", detail: m.reasoning });
      if (text || files.length) {
        flush();
        out.push(said("assistant", text, files));
      }
      for (const call of calls) work.push({ verb: call.name || "tool", args: call.arguments || "" });
    } else if (m.role === "tool") {
      // its result, beside the call it answers: the first of that tool's still unanswered
      const call = work.find((s) => s.verb === m.tool && s.result === undefined && s.args !== undefined);
      if (call) call.result = text;
      else work.push({ verb: m.tool || "Result", detail: text });
    } else if (text) work.push({ verb: m.role || "Note", detail: text });
  }
  flush();
  return out;
}

async function reader(page, key) {
  const head = el("header", "archive-head");
  const back = el("a", "back", "← All conversations");
  back.href = "./";
  head.append(back);
  const c = KEY.test(key) ? await file(`sessions/${key}.json`) : null;
  if (!c || !Array.isArray(c.messages)) {
    head.append(el("h1", null, "This conversation isn't here"));
    page.replaceChildren(head);
    return;
  }
  document.title = c.title || "Session Archive";
  head.append(el("h1", null, c.title || "Untitled"));
  const about = [c.kind !== "chat" ? KIND_ONE[c.kind] : "", when(c.started_at, c.ended_at), c.with ? `with ${c.with}` : ""].filter(Boolean).join(" · ");
  if (about) head.append(el("p", null, about));
  // Continue: the shell asks with which agent (or opens the chat one made)
  const go = el("button", "archive-continue", "Continue");
  go.type = "button";
  go.hidden = !shell;
  go.onclick = () => toShell({ continue: key });
  head.append(go);
  page.replaceChildren(head, ...conversation(c));
}

function main() {
  const key = new URLSearchParams(location.search).get("s");
  window.addEventListener("message", (e) => {
    if (e.data?.fragment !== "theme") return;
    if (e.data.mode === "light" || e.data.mode === "dark") document.documentElement.dataset.theme = e.data.mode;
    // the shell framing it: which conversation this shows, for its heading
    if (e.source !== window.parent || window.parent === window) return;
    shell = e.origin;
    toShell({ key });
    for (const b of document.querySelectorAll(".archive-continue")) b.hidden = false;
  });
  const page = document.getElementById("page");
  (key ? reader(page, key) : list(page)).catch(() => page.replaceChildren(el("p", "muted", "This didn't load. Reload to try again.")));
}

main();
