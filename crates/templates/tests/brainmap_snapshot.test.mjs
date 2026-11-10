// The brain map's snapshot (templates/brainmap/scripts/brain-folder.mjs):
// what a synced brain folder, an agent's routines and a brain's members
// say, as map.json. Valid: a folder laid out as the brain's AGENTS.md says
// (wikis, raw/ sources with frontmatter, wiki/ pages, output/, log.md, a
// parent's datasets/sources.json) reads as wikis and sources by connector;
// a routine naming the brain is a workflow; members are readers. Invalid:
// a source with no URL, a bad URL, a datasets/sources.json that is not a
// list; hidden folders and sync's conflict copies are not knowledge.
// Privacy: no person's identity reaches the map.
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { register } from "node:module";

const BASE = "export class DurableObject { constructor(ctx, env) { this.ctx = ctx; this.env = env; } }";
const HOOKS = `export async function resolve(specifier, context, next) {
  if (specifier === "cloudflare:workers") return { url: ${JSON.stringify("data:text/javascript," + encodeURIComponent(BASE))}, shortCircuit: true };
  return next(specifier, context);
}`;
register("data:text/javascript," + encodeURIComponent(HOOKS));
const TEMPLATE = new URL("../../../templates/brainmap/", import.meta.url);
const { frontmatter, connectorOf, logEntries, readFolder, routinesFor, readersOf, assemble, VERSION } = await import(new URL("scripts/brain-folder.mjs", TEMPLATE).href);
const { problemOf } = await import(new URL("app.mjs", TEMPLATE).href);

function folder(files) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "brainmap-"));
  for (const [rel, text] of Object.entries(files)) {
    fs.mkdirSync(path.dirname(path.join(dir, rel)), { recursive: true });
    fs.writeFileSync(path.join(dir, rel), text);
  }
  return dir;
}

const note = (fm, body = "") => `---\n${Object.entries(fm).map(([k, v]) => `${k}: ${v}`).join("\n")}\n---\n${body}`;

test("frontmatter reads the guide's subset: values, quoted values, block and inline lists", () => {
  const fm = frontmatter('---\ntitle: "Solar: tariffs"\nsources:\n  - garden/raw/a.md\n  - garden/raw/b.md\ntags: [energy, policy]\nfinite_asset:\n  content_type: image/jpeg\ncaptured: 2026-10-03\n---\nbody');
  assert.deepEqual(fm, { title: "Solar: tariffs", sources: ["garden/raw/a.md", "garden/raw/b.md"], tags: ["energy", "policy"], finite_asset: "", captured: "2026-10-03" });
  assert.deepEqual(frontmatter("no frontmatter"), {});
});

test("a source's connector is its via, else its URL's host, else written in", () => {
  const cases = [
    [{ source: "https://mail.google.com/mail/u/0/#inbox/1" }, "gmail"],
    [{ source: "https://docs.google.com/document/d/1" }, "drive"],
    [{ source: "https://t.me/c/123/45" }, "telegram"],
    [{ source: "https://x.com/someone/status/1" }, "x"],
    [{ source: "https://example.com/post" }, "web"],
    [{ source: "not a url" }, "web"],
    [{}, "manual"],
    [{ type: "asset" }, "upload"],
    [{ via: "Gmail", source: "https://example.com" }, "gmail"],
    [{ via: "Granola notes" }, "granola-notes"],
  ];
  for (const [fm, id] of cases) assert.equal(connectorOf(fm), id, JSON.stringify(fm));
});

test("a log's entries are its dated headings, each with what it says", () => {
  const log = "# Log\n\n## 2026-11-01 — Ingested tariffs\n\n## 2026-11-02\n- Added the frost dates page\n\n## Notes\n";
  assert.deepEqual(logEntries(log), [{ date: "2026-11-01", title: "Ingested tariffs" }, { date: "2026-11-02", title: "Added the frost dates page" }]);
});

test("a brain folder reads as its wikis and its sources by connector, child brains among them", () => {
  const dir = folder({
    "fragment.json": "{}",
    ".fragment/journal": "x",
    "garden/index.md": "# Garden",
    "garden/log.md": "## 2026-11-01 Tariffs\n## 2026-11-03 Frost dates\n",
    "exec/log.md": "## 2026-11-05 Newest first\n## 2026-10-02 Oldest\n## 2026-11-04 Between\n",
    "garden/raw/2026-11-03-frost.md": note({ type: "source", title: "Frost dates", source: "https://example.com/frost", captured: "2026-11-03" }),
    "garden/raw/2026-11-01-tariffs.md": note({ type: "source", source: "https://mail.google.com/x" }, "# Tariff thread\n"),
    "garden/raw/2026-11-02-call.md": note({ type: "source", title: "Call", via: "telegram" }),
    "garden/raw/2026-11-02-call.conflict-1.md": note({ title: "a conflict copy" }),
    "garden/raw/assets/photo.jpg": "bytes",
    "garden/raw/photo.md": note({ type: "asset", title: "Photo", resource: "raw/assets/photo.jpg" }),
    "garden/wiki/frost-dates.md": note({ title: "Frost dates", updated: "2026-11-04" }),
    "garden/output/brief.md": "# Weekly brief\n",
    "exec/datasets/sources.json": JSON.stringify([{ fragment: "research--a1b2", paths: ["team/wiki/launch.md"] }, { nope: 1 }]),
  });
  const { wikis, sources } = readFolder(dir);
  assert.deepEqual(wikis.map((w) => w.id), ["exec", "garden"], "each top-level folder, never a hidden one");
  const garden = wikis.find((w) => w.id === "garden");
  assert.equal(garden.pages, 1);
  assert.equal(garden.updated, "2026-11-04");
  assert.deepEqual(garden.log.map((e) => e.date), ["2026-11-03", "2026-11-01"], "newest first");
  assert.deepEqual(wikis.find((w) => w.id === "exec").log.map((e) => e.date), ["2026-11-05", "2026-11-04", "2026-10-02"], "by date, whatever order the log is written in");
  assert.deepEqual(garden.outputs, [{ path: "garden/output/brief.md", title: "Weekly brief" }]);
  const by = Object.fromEntries(sources.map((s) => [s.id, s]));
  assert.deepEqual(Object.keys(by).sort(), ["brain:research--a1b2", "gmail", "telegram", "upload", "web"], "no conflict copy, no asset's bytes");
  assert.deepEqual(by.gmail.flows, [{ wiki: "garden", count: 1, last: "2026-11-01" }], "a date from the file's name");
  assert.equal(by.gmail.items[0].title, "Tariff thread", "a title from the first heading");
  assert.deepEqual(by["brain:research--a1b2"].flows, [{ wiki: "exec", count: 1, last: null }]);
  assert.equal(by["brain:research--a1b2"].kind, "brain");
});

test("a datasets/sources.json that is not a list is refused, naming it", () => {
  const dir = folder({ "exec/datasets/sources.json": '{"fragment": "x"}' });
  assert.throws(() => readFolder(dir), /exec\/datasets\/sources\.json is not a list/);
  assert.throws(() => readFolder(path.join(dir, "nope")), /is not a folder/);
});

test("an agent's routines that name the brain are its workflows, their sources and wikis read from their words", () => {
  const schedules = [
    { id: 1, cron: "0 8 * * 1-5", said: "at 08:00, Monday through Friday", record: { kind: "routine", text: "Read new Gmail threads and file them in garden--k3x9's garden/ wiki" }, paused: false, nextAt: 1 },
    { id: 2, said: "daily", record: { kind: "routine", text: "Water the plants" } },
    { id: 3, said: "weekly", record: { kind: "routine", text: "Summarize news articles and X posts into the Garden brain" }, paused: true },
    { id: 4, said: "weekly", channel: "inbox", record: { source: "x" } },
  ];
  const wfs = routinesFor(schedules, { brain: "garden--k3x9", title: "Garden", wikis: ["garden", "exec"], agent: "hermes--a1b2", agentTitle: "Hermes" });
  assert.deepEqual(wfs.map((w) => w.id), ["hermes--a1b2#1", "hermes--a1b2#3"], "routines naming it by its name or its title");
  assert.deepEqual([wfs[0].sources, wfs[0].wikis, wfs[0].agent_title], [["gmail"], ["garden"], "Hermes"]);
  assert.deepEqual([wfs[1].sources, wfs[1].paused], [["x", "web"], true], "X, standing alone and capital, is X");
  assert.deepEqual(routinesFor(schedules, { brain: "brain--k3x9", title: "Brain", wikis: [], agent: "a" }).length, 0, "a brain called Brain is not named by the word brain");
});

test("readers: agents by name, people as one count with no identity, and the link's holders", () => {
  const members = [
    { principal: "npub1agentaaaaaaaaaaaaaaaaaaaaaaaaaa", role: "editor", kind: "agent" },
    { principal: "npub1unnamedagentbbbbbbbbbbbbbbbbbb", role: "viewer", kind: "agent" },
    { principal: "npub1personcccccccccccccccccccccccc", role: "viewer", kind: "person", email: "someone@example.com" },
    { principal: "npub1persondddddddddddddddddddddddd", role: "viewer" },
  ];
  const readers = readersOf({ owner: "npub1owner", visibility: "link" }, members, new Map([["npub1agentaaaaaaaaaaaaaaaaaaaaaaaaaa", "Hermes"]]));
  assert.deepEqual(readers.map((r) => r.label), ["Hermes", "Agent npub1unnam…bbbb", "People", "Anyone with the link"]);
  assert.deepEqual(readers[2], { id: "people", kind: "people", label: "People", role: "1 owner, 2 viewers", count: 3 });
  const listed = readersOf({ owner: "npub1owner", visibility: "members" }, [{ principal: "npub1owner", role: "owner", kind: "person" }, ...members], new Map());
  assert.deepEqual(listed.find((r) => r.id === "people"), { id: "people", kind: "people", label: "People", role: "1 owner, 2 viewers", count: 3 }, "an owner the members list names too is counted once");
  const json = JSON.stringify(readers);
  assert.ok(!json.includes("someone@example.com") && !json.includes("npub1person") && !json.includes("npub1owner"), "no person's identity");
});

test("assemble makes a map the app takes, with no token of the brain's", () => {
  const status = { name: "garden--k3x9", title: "Garden", visibility: "members", urls: { canonical: "https://garden--k3x9.example/" }, viewToken: "secret-view", inboxToken: "secret-inbox" };
  const map = assemble({ status, index: { line: "2 pages", pages: 2, sections: 5, pending: ["a.md"] }, folder: { wikis: [], sources: [] }, workflows: [], readers: [], takenAt: "2026-11-10T00:00:00Z" });
  assert.equal(map.version, VERSION);
  assert.equal(map.brain.index.pending, 1, "a pending list counts");
  assert.ok(!JSON.stringify(map).includes("secret"), "no token");
  assert.equal(problemOf(map), null);
});
