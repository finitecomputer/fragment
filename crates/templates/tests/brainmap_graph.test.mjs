// The brain map's graph (templates/brainmap/site/graph.js), what its page
// draws. Valid: the sample map's flows go through the workflow that brings
// them, else straight to their wiki; every wiki feeds search and search
// every reader; recent flows move; a node's lineage is everything its data
// passes through. Invalid: a hostile map (wrong types, a script URL, huge
// lists, edges to nothing) draws within bounds, never a link that is not
// http(s), and no edge to a node it left out.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const TEMPLATE = new URL("../../../templates/brainmap/", import.meta.url);
const { graph, lineage, safeUrl, normalize, LIST_MAX, RECENT_MS, HEIGHTS } = await import(new URL("site/graph.js", TEMPLATE).href);
const SAMPLE = JSON.parse(readFileSync(new URL("map.json", TEMPLATE), "utf8"));

const edge = (g, id) => g.edges.find((e) => e.id === id);

test("a flow goes through the workflow that brings it, else straight to its wiki", () => {
  const g = graph(SAMPLE);
  assert.equal(edge(g, "src:gmail->wf:hermes--a1b2#1").label, "9", "Gmail into customers/, through Hermes's routine");
  assert.ok(edge(g, "src:gmail->wf:hermes--a1b2#1").className.includes("bm-inferred"), "a routine's sources are read from its words");
  assert.equal(edge(g, "wf:hermes--a1b2#1->wiki:customers").label, "9");
  assert.equal(edge(g, "src:gmail->wiki:launch").label, "5", "no routine brings Gmail into launch/");
  assert.equal(edge(g, "src:gmail->wiki:customers"), undefined, "a flow is drawn once");
  assert.equal(edge(g, "wf:scout--q9w8#1->wiki:research").label, "13", "a workflow's flows are summed: the web's 11 and X's 2");
});

test("every wiki feeds search, and search every reader", () => {
  const g = graph(SAMPLE);
  for (const w of SAMPLE.wikis) assert.ok(edge(g, `wiki:${w.id}->index`), w.id);
  for (const r of SAMPLE.readers) assert.ok(edge(g, `index->reader:${r.id}`), r.id);
  const wikis = g.nodes.filter((n) => n.type === "wiki");
  assert.ok(wikis.every((n) => n.parentId === "brain"), "the wikis sit inside the brain");
  assert.equal(g.nodes[0].id, "brain", "a group comes before its children");
});

test("a flow captured within a week of the snapshot moves; an older one does not", () => {
  const g = graph(SAMPLE);
  assert.equal(edge(g, "src:gmail->wf:hermes--a1b2#1").animated, true, "9 Nov, the day before");
  assert.equal(edge(g, "src:github->wiki:launch").animated, false, "30 Oct");
  const at = Date.parse(SAMPLE.taken_at);
  const day = (t) => new Date(t).toISOString().slice(0, 10);
  const edgeAt = (last) => graph({ ...SAMPLE, sources: [{ id: "s", flows: [{ wiki: "team", count: 1, last }] }], workflows: [] }).edges.find((e) => e.source === "src:s");
  assert.equal(edgeAt(day(at - RECENT_MS + 86400000)).animated, true);
  assert.equal(edgeAt(day(at - RECENT_MS - 86400000)).animated, false);
  assert.equal(graph({ ...SAMPLE, taken_at: undefined }).edges.some((e) => e.animated), false, "no snapshot time, nothing is recent");
});

test("a node's lineage is everything its data passes through, both ways", () => {
  const g = graph(SAMPLE);
  const gmail = lineage(g.edges, "src:gmail");
  assert.deepEqual([...gmail.nodes].sort(), ["index", "reader:npub1atlas", "reader:npub1hermes", "reader:npub1scout", "reader:people", "src:gmail", "wf:hermes--a1b2#1", "wiki:customers", "wiki:launch"]);
  const team = lineage(g.edges, "wiki:team");
  assert.ok(team.nodes.has("src:calendar") && team.nodes.has("src:manual") && team.nodes.has("reader:people"));
  assert.ok(!team.nodes.has("src:gmail"), "upstream is only what feeds it");
});

test("a workflow that names a source nothing came from yet still draws it", () => {
  const g = graph({ version: 1, brain: {}, wikis: [{ id: "news" }], workflows: [{ id: "w", sources: ["slack"], wikis: [] }] });
  assert.ok(g.nodes.some((n) => n.id === "src:slack"));
  assert.ok(edge(g, "src:slack->wf:w"));
  assert.ok(edge(g, "wf:w->brain"), "a routine naming no wiki writes into the brain");
});

test("a hostile map draws within bounds, with only http(s) links and no edge to nothing", () => {
  const many = Array.from({ length: LIST_MAX + 10 }, (_, i) => ({ id: `s${i}`, flows: [{ wiki: `w${i}`, count: 1 }] }));
  const g = graph({
    version: 1,
    brain: { title: { evil: true }, url: "javascript:alert(1)", index: { pages: -4, sections: "x" } },
    sources: [...many, { id: 7, label: "<img onerror=x>", flows: "no" }],
    workflows: [{ id: "w", sources: ["ghost"], wikis: ["w0"] }],
    readers: [{ id: "r", label: "x".repeat(10_000) }],
  });
  const m = normalize({ brain: { url: "javascript:alert(1)" } });
  assert.equal(m.brain.url, null);
  assert.equal(g.map.brain.url, null);
  assert.equal(g.map.brain.title, "[object Object]", "text is text");
  assert.equal(g.map.brain.index.pages, 0);
  assert.ok(g.nodes.filter((n) => n.type === "source").length <= LIST_MAX);
  assert.ok(g.nodes.filter((n) => n.type === "wiki").length <= LIST_MAX);
  assert.deepEqual(g.map.cut, ["sources"]);
  assert.ok(g.nodes.find((n) => n.type === "reader").data.label.length <= 80);
  const ids = new Set(g.nodes.map((n) => n.id));
  assert.ok(g.edges.every((e) => ids.has(e.source) && ids.has(e.target)), "every edge joins two drawn nodes");
  assert.equal(safeUrl("https://hq--k3x9.fragment.localhost:8790/"), "https://hq--k3x9.fragment.localhost:8790/");
  assert.equal(safeUrl("data:text/html,x"), null);
  assert.equal(graph(null).nodes.length, 2, "nothing at all is a brain and its search");
});

test("each column is ordered by where its edges go, so few of them cross", () => {
  const g = graph(SAMPLE);
  const at = new Map(g.nodes.map((n) => [n.id, n]));
  // a node's middle, where its edges meet it
  const y = (id) => at.get(id).position.y + (at.get(id).parentId ? at.get(at.get(id).parentId).position.y : 0) + (HEIGHTS[at.get(id).type] ?? 0) / 2;
  const pull = (id) => {
    const ys = g.edges.filter((e) => e.source === id).map((e) => y(e.target));
    return ys.reduce((a, b) => a + b, 0) / ys.length;
  };
  for (const type of ["workflow", "source"]) {
    const column = g.nodes.filter((n) => n.type === type).sort((a, b) => a.position.y - b.position.y);
    const pulls = column.map((n) => pull(n.id));
    assert.ok(pulls.every((p, i) => i === 0 || p >= pulls[i - 1]), `${type}s top to bottom follow their targets: ${column.map((n) => n.id).join(", ")}`);
  }
  const fromSources = g.edges.filter((e) => e.source.startsWith("src:"));
  let crossings = 0;
  for (const [i, a] of fromSources.entries()) {
    for (const b of fromSources.slice(i + 1)) if ((y(a.source) - y(b.source)) * (y(a.target) - y(b.target)) < 0) crossings++;
  }
  assert.ok(crossings <= 16, `the sample's source edges cross ${crossings} times (34 when sources stood by size)`);
});
