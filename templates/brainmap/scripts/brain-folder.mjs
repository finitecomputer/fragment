// What a brain's synced folder says about where its knowledge came from,
// read as the brain's AGENTS.md lays a brain out: each top-level folder a
// wiki, sources under raw/ (frontmatter `source`, `captured`, and the
// optional `via` naming the connector that brought it), pages under wiki/,
// deliverables under output/, a log.md, and, in a parent brain,
// datasets/sources.json naming the child brains it draws on. Then an
// agent's routines (`fragment schedule list`) that name the brain, and the
// readers its members are. scripts/snapshot.mjs puts them together as a
// map.json (README.md, "The map").
//
// Nothing here is the brain's own record of a connector or a workflow: a
// source's connector is its `via`, else guessed from its URL's host; a
// routine's sources and wikis are guessed from its words.
import fs from "node:fs";
import path from "node:path";

export const VERSION = 1;
export const ITEMS_MAX = 50;
export const LOG_MAX = 5;
export const OUTPUTS_MAX = 20;
// A page read for its frontmatter and title: past this, its head is enough.
const HEAD_BYTES = 16 * 1024;

export const CONNECTORS = {
  gmail: { label: "Gmail", provider: "Google" },
  drive: { label: "Google Drive", provider: "Google" },
  calendar: { label: "Google Calendar", provider: "Google" },
  telegram: { label: "Telegram", provider: "Telegram connector" },
  x: { label: "X", provider: "X API" },
  github: { label: "GitHub", provider: "" },
  slack: { label: "Slack", provider: "" },
  notion: { label: "Notion", provider: "" },
  linear: { label: "Linear", provider: "" },
  youtube: { label: "YouTube", provider: "" },
  web: { label: "The web", provider: "Pages an agent read" },
  manual: { label: "Written in", provider: "No source: typed by a person or an agent" },
  upload: { label: "Uploads", provider: "Files added by hand" },
};

const HOSTS = [
  [/(^|\.)(mail\.google\.com|gmail\.com)$/, "gmail"],
  [/(^|\.)(docs|drive|sheets|slides)\.google\.com$/, "drive"],
  [/(^|\.)calendar\.google\.com$/, "calendar"],
  [/(^|\.)(t\.me|telegram\.me|telegram\.org)$/, "telegram"],
  [/(^|\.)(x\.com|twitter\.com)$/, "x"],
  [/(^|\.)github\.com$/, "github"],
  [/(^|\.)slack\.com$/, "slack"],
  [/(^|\.)(notion\.so|notion\.site)$/, "notion"],
  [/(^|\.)linear\.app$/, "linear"],
  [/(^|\.)(youtube\.com|youtu\.be)$/, "youtube"],
];

// A routine's words, and the connector each names.
const WORDS = [
  [/\b(gmail|e-?mails?|mailbox)\b/i, "gmail"],
  [/\b(google drive|drive|google docs?|docs folder|sheets?|slides)\b/i, "drive"],
  [/\b(calendar|agenda)\b/i, "calendar"],
  [/\btelegram\b/i, "telegram"],
  [/\b(twitter|tweets?|x\.com|x api)\b/i, "x"],
  // "X" alone, capital: case counts, as a lowercase x is rarely the network
  [/(^|[^\w.])X(?![\w.])/, "x"],
  [/\bgithub\b/i, "github"],
  [/\bslack\b/i, "slack"],
  [/\bnotion\b/i, "notion"],
  [/\blinear\b/i, "linear"],
  [/\byoutube\b/i, "youtube"],
  [/\b(the web|web pages?|news|rss|articles?|blogs?|websites?)\b/i, "web"],
];

const DATE = /\d{4}-\d{2}-\d{2}/;
const unquote = (v) => v.replace(/^(["'])(.*)\1$/, "$2");
const slug = (s) => s.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 48);
const escapeRe = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** A page's frontmatter, the YAML subset the brain's guide writes: `key: value`, and lists of `- item`. */
export function frontmatter(text) {
  const m = /^---\r?\n([\s\S]*?)\r?\n---(\r?\n|$)/.exec(text);
  if (!m) return {};
  const out = {};
  let listKey = null;
  for (const line of m[1].split(/\r?\n/)) {
    const item = /^\s+-\s+(.*)$/.exec(line);
    if (item && listKey) {
      out[listKey] = [...(Array.isArray(out[listKey]) ? out[listKey] : []), unquote(item[1].trim())];
      continue;
    }
    const kv = /^([A-Za-z_][\w-]*):\s*(.*)$/.exec(line);
    if (!kv) continue;
    const value = kv[2].trim();
    listKey = value === "" ? kv[1] : null;
    out[kv[1]] = value.startsWith("[") && value.endsWith("]") ? value.slice(1, -1).split(",").map((v) => unquote(v.trim())).filter(Boolean) : unquote(value);
  }
  return out;
}

/** The connector a source came through: its `via`, else its URL's host, else none (written in). */
export function connectorOf({ via, source, type }) {
  if (typeof via === "string" && via.trim()) {
    const v = via.trim().toLowerCase();
    const known = Object.entries(CONNECTORS).find(([id, c]) => id === v || c.label.toLowerCase() === v);
    return known ? known[0] : slug(via) || "manual";
  }
  if (typeof source === "string" && source.trim()) {
    try {
      const host = new URL(source.trim()).hostname.toLowerCase();
      return HOSTS.find(([re]) => re.test(host))?.[1] ?? "web";
    } catch {
      return "web";
    }
  }
  return type === "asset" ? "upload" : "manual";
}

/** Each file under `dir`, relative and `/`-separated, skipping hidden ones and sync's conflict copies. */
function walk(dir, rel = "") {
  const at = path.join(dir, rel);
  if (!fs.existsSync(at)) return [];
  return fs.readdirSync(at, { withFileTypes: true })
    .filter((e) => !e.name.startsWith(".") && !e.name.includes(".conflict-"))
    .flatMap((e) => (e.isDirectory() ? walk(dir, path.posix.join(rel, e.name)) : e.isFile() ? [path.posix.join(rel, e.name)] : []))
    .sort();
}

function head(file) {
  const fd = fs.openSync(file, "r");
  try {
    const buf = Buffer.alloc(HEAD_BYTES);
    return buf.subarray(0, fs.readSync(fd, buf, 0, HEAD_BYTES, 0)).toString("utf8");
  } finally {
    fs.closeSync(fd);
  }
}

function titleOf(text, fm, file) {
  if (typeof fm.title === "string" && fm.title) return fm.title;
  const h = /^#\s+(.+)$/m.exec(text.replace(/^---[\s\S]*?\n---/, ""));
  return h ? h[1].trim() : path.basename(file).replace(/\.[^.]+$/, "");
}

/** A log.md's entries, newest last as written: each a dated heading and what it says. */
export function logEntries(text) {
  const lines = text.split(/\r?\n/);
  return lines.flatMap((line, i) => {
    const h = /^#{2,3}\s+(.+)$/.exec(line);
    if (!h || !DATE.test(h[1])) return [];
    const date = DATE.exec(h[1])[0];
    const rest = h[1].replace(DATE, "").replace(/^[\s:·—–-]+|[\s:·—–-]+$/g, "");
    const next = lines.slice(i + 1).find((l) => l.trim() && !l.startsWith("#"));
    return [{ date, title: (rest || next?.replace(/^[-*]\s*/, "").trim() || "").slice(0, 200) }];
  });
}

/** One wiki's pages, sources, log and deliverables. */
function readWiki(dir, id) {
  const files = walk(dir, id);
  const md = (prefix) => files.filter((f) => f.startsWith(`${id}/${prefix}/`) && /\.(md|markdown)$/i.test(f));
  const notes = md("raw").filter((f) => !f.startsWith(`${id}/raw/assets/`)).map((f) => {
    const text = head(path.join(dir, f));
    const fm = frontmatter(text);
    const captured = (typeof fm.captured === "string" && DATE.exec(fm.captured)?.[0]) || DATE.exec(path.basename(f))?.[0] || null;
    return { path: f, title: titleOf(text, fm, f), captured, connector: connectorOf(fm) };
  });
  const pages = md("wiki");
  const updated = pages.map((f) => frontmatter(head(path.join(dir, f)))).map((fm) => DATE.exec(String(fm.updated ?? fm.created ?? ""))?.[0]).filter(Boolean);
  const outputs = files.filter((f) => f.startsWith(`${id}/output/`)).slice(0, OUTPUTS_MAX).map((f) => {
    const text = /\.(md|markdown)$/i.test(f) ? head(path.join(dir, f)) : "";
    return { path: f, title: titleOf(text, frontmatter(text), f) };
  });
  const log = fs.existsSync(path.join(dir, id, "log.md")) ? logEntries(fs.readFileSync(path.join(dir, id, "log.md"), "utf8")).sort((a, b) => b.date.localeCompare(a.date)).slice(0, LOG_MAX) : [];
  const newest = [...updated, ...notes.map((n) => n.captured).filter(Boolean)].sort().pop() ?? null;
  return { wiki: { id, pages: pages.length, updated: newest, log, outputs }, notes };
}

/** The child brains a parent wiki's datasets/sources.json names. */
function childrenOf(dir, id) {
  const file = path.join(dir, id, "datasets", "sources.json");
  if (!fs.existsSync(file)) return [];
  let list;
  try {
    list = JSON.parse(fs.readFileSync(file, "utf8"));
  } catch (e) {
    throw new Error(`${id}/datasets/sources.json is not JSON: ${e.message}`);
  }
  if (!Array.isArray(list)) throw new Error(`${id}/datasets/sources.json is not a list`);
  return list.filter((c) => typeof c?.fragment === "string").map((c) => {
    const paths = Array.isArray(c.paths) ? c.paths.filter((p) => typeof p === "string") : [];
    return { id: `brain:${c.fragment}`, label: c.fragment, kind: "brain", provider: "A child brain", wiki: id, paths };
  });
}

/** Each child brain once, however many wikis (or entries) name it: per wiki, its paths, each once. */
function childSources(entries) {
  return [...Map.groupBy(entries, (c) => c.id)].map(([id, cs]) => {
    const flows = [...Map.groupBy(cs, (c) => c.wiki)].map(([wiki, ws]) => ({ wiki, count: new Set(ws.flatMap((w) => w.paths)).size, last: null }));
    const paths = [...new Set(cs.flatMap((c) => c.paths))].slice(0, ITEMS_MAX);
    return { id, label: cs[0].label, kind: "brain", provider: cs[0].provider, flows, items: paths.map((p) => ({ path: p, title: p, captured: null })) };
  });
}

/** Sources grouped by connector: how many each brought into each wiki, the newest, and the newest items. */
function grouped(notes) {
  const by = Map.groupBy(notes, (n) => n.connector);
  return [...by].map(([id, ns]) => {
    const known = CONNECTORS[id];
    const flows = [...Map.groupBy(ns, (n) => n.path.split("/")[0])].map(([wiki, w]) => ({
      wiki, count: w.length, last: w.map((n) => n.captured).filter(Boolean).sort().pop() ?? null,
    }));
    const items = [...ns].sort((a, b) => (b.captured ?? "").localeCompare(a.captured ?? "")).slice(0, ITEMS_MAX).map(({ path: p, title, captured }) => ({ path: p, title, captured }));
    return { id, label: known?.label ?? id, kind: "connector", provider: known?.provider ?? "", flows, items };
  });
}

/** Everything a synced brain folder says: its wikis, and its sources by connector, child brains among them. */
export function readFolder(dir) {
  if (!fs.statSync(dir, { throwIfNoEntry: false })?.isDirectory()) throw new Error(`${dir} is not a folder`);
  const ids = fs.readdirSync(dir, { withFileTypes: true })
    .filter((e) => e.isDirectory() && !e.name.startsWith(".") && !["site", "applib"].includes(e.name))
    .map((e) => e.name).sort();
  const read = ids.map((id) => readWiki(dir, id));
  return { wikis: read.map((r) => r.wiki), sources: [...grouped(read.flatMap((r) => r.notes)), ...childSources(ids.flatMap((id) => childrenOf(dir, id)))] };
}

/** An agent's routines that name the brain, as workflows: what each reads and where it writes, from its words. */
export function routinesFor(schedules, { brain, title, wikis, agent, agentTitle }) {
  const names = [brain, brain.split("--")[0], title].filter((n) => typeof n === "string" && n.length >= 3 && n.toLowerCase() !== "brain").map((n) => n.toLowerCase());
  return schedules.flatMap((s) => {
    const text = typeof s?.record?.text === "string" ? s.record.text : "";
    if (!text || !names.some((n) => text.toLowerCase().includes(n))) return [];
    return [{
      id: `${agent}#${s.id}`,
      agent,
      agent_title: agentTitle ?? agent,
      said: s.said ?? s.cron ?? "",
      text: text.slice(0, 600),
      sources: [...new Set(WORDS.filter(([re]) => re.test(text)).map(([, id]) => id))],
      wikis: wikis.filter((w) => new RegExp(`(^|[^\\w-])${escapeRe(w)}(/|\\b)`, "i").test(text)),
      paused: s.paused === true,
      last_at: s.lastAt ?? null,
      next_at: s.nextAt ?? null,
    }];
  });
}

const short = (npub) => (npub.length > 16 ? `${npub.slice(0, 10)}…${npub.slice(-4)}` : npub);

/** Who reads the brain: each agent by name, people as one count (never their identities), and anyone a link or the public lets in. */
export function readersOf({ owner, visibility }, members, agentNames) {
  const agents = members.filter((m) => m.kind === "agent").map((m) => ({
    id: m.principal, kind: "agent", label: agentNames.get(m.principal) ?? `Agent ${short(m.principal)}`, role: m.role,
  }));
  const persons = members.filter((m) => m.kind !== "agent");
  // the members list names the owner too, on a platform that lists it
  const people = persons.some((m) => m.principal === owner) ? persons : [{ role: "owner", principal: owner }, ...persons];
  const roles = Object.entries(Object.groupBy(people, (p) => p.role)).map(([role, ps]) => `${ps.length} ${role}${ps.length === 1 ? "" : "s"}`);
  const open = visibility === "public" ? [{ id: "public", kind: "public", label: "Anyone", role: "viewer" }]
    : visibility === "link" ? [{ id: "link", kind: "link", label: "Anyone with the link", role: "viewer" }] : [];
  return [...agents, { id: "people", kind: "people", label: "People", role: roles.join(", "), count: people.length }, ...open];
}

/** The map.json: a brain's status and index, its folder, its workflows and readers, at `takenAt`. */
export function assemble({ status, index, folder, workflows, readers, takenAt }) {
  const pending = Array.isArray(index?.pending) ? index.pending.length : Number.isInteger(index?.pending) ? index.pending : 0;
  return {
    version: VERSION,
    taken_at: takenAt,
    brain: {
      name: status.name,
      title: status.title ?? status.name,
      url: status.urls?.canonical ?? null,
      visibility: status.visibility ?? null,
      index: { line: index?.line ?? "", pages: index?.pages ?? 0, sections: index?.sections ?? 0, pending },
    },
    wikis: folder.wikis,
    sources: folder.sources,
    workflows,
    readers,
  };
}
