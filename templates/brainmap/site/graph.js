// The brain map's graph: a map.json (README.md, "The map") made into React
// Flow's nodes and edges, in columns left to right as knowledge moves:
// where it comes from (connectors, other brains), the workflows that bring
// it (agents' routines), the brain (its wikis, then its search), and who
// reads it. Pure, with no React, so Node tests it
// (crates/templates/tests/brainmap_graph.test.mjs).
//
// map.json is written by an agent or a script, so everything here is read
// as untrusted: lists are cut at LIST_MAX, text is a string, counts are
// whole numbers, and a link is only ever http(s).

// Data captured this long before the snapshot moves along its edges.
export const RECENT_MS = 7 * 24 * 60 * 60 * 1000;
// The most of each kind the map draws: past it, `cut` says how many were left out.
export const LIST_MAX = 64;
// The most items one source lists in its panel.
export const ITEMS_MAX = 50;

const W = 232;
const GAP_Y = 18;
const GAP_X = 120;
// Each kind of node's height: given to React Flow as the node's own (so the
// minimap and fit need no measuring, and an edge meets a node at its middle).
export const HEIGHTS = { source: 80, workflow: 100, reader: 60, wiki: 64, index: 112 };
const H = HEIGHTS;
const BRAIN = { pad: 16, head: 56, wikiW: 210, indexW: 168, gap: 56 };
const BRAIN_W = BRAIN.pad + BRAIN.wikiW + BRAIN.gap + BRAIN.indexW + BRAIN.pad;

const text = (v, max = 400) => (typeof v === "string" ? v.slice(0, max) : v == null ? "" : String(v).slice(0, max));
const count = (v) => (Number.isInteger(v) && v > 0 ? v : 0);
const list = (v) => (Array.isArray(v) ? v.slice(0, LIST_MAX) : []);
const time = (v) => {
  const t = typeof v === "number" ? v : Date.parse(text(v, 40));
  return Number.isFinite(t) ? t : null;
};

/** `url` if it is http(s), else null: a link the page may follow. */
export function safeUrl(url) {
  try {
    const u = new URL(text(url, 2048));
    return u.protocol === "https:" || u.protocol === "http:" ? u.href : null;
  } catch {
    return null;
  }
}

/** The map as the graph reads it: every field present, typed and bounded. */
export function normalize(map) {
  const m = map && typeof map === "object" ? map : {};
  const brain = m.brain && typeof m.brain === "object" ? m.brain : {};
  const index = brain.index && typeof brain.index === "object" ? brain.index : {};
  const flow = (f) => ({ wiki: text(f?.wiki, 128), count: count(f?.count), last: text(f?.last, 40) || null });
  const item = (i) => ({ path: text(i?.path, 300), title: text(i?.title, 200), captured: text(i?.captured, 40) || null });
  const ids = (v) => list(v).map((x) => text(x, 128)).filter(Boolean);
  return {
    sample: m.sample === true,
    takenAt: time(m.taken_at),
    brain: {
      name: text(brain.name, 128),
      title: text(brain.title, 200) || text(brain.name, 128) || "Brain",
      url: safeUrl(brain.url),
      visibility: text(brain.visibility, 16),
      index: { line: text(index.line, 200), pages: count(index.pages), sections: count(index.sections), pending: count(index.pending) },
    },
    wikis: list(m.wikis).map((w) => ({
      id: text(w?.id, 128),
      pages: count(w?.pages),
      updated: text(w?.updated, 40) || null,
      log: list(w?.log).slice(0, 10).map((e) => ({ date: text(e?.date, 40), title: text(e?.title, 200) })),
      outputs: list(w?.outputs).slice(0, 20).map(item),
    })).filter((w) => w.id),
    sources: list(m.sources).map((s) => ({
      id: text(s?.id, 128),
      label: text(s?.label, 80) || text(s?.id, 80),
      kind: text(s?.kind, 16) || "connector",
      provider: text(s?.provider, 80),
      flows: list(s?.flows).map(flow).filter((f) => f.wiki),
      items: list(s?.items).slice(0, ITEMS_MAX).map(item),
    })).filter((s) => s.id),
    workflows: list(m.workflows).map((w) => ({
      id: text(w?.id, 160),
      agent: text(w?.agent_title, 80) || text(w?.agent, 128),
      said: text(w?.said, 160),
      text: text(w?.text, 600),
      sources: ids(w?.sources),
      wikis: ids(w?.wikis),
      paused: w?.paused === true,
      lastAt: time(w?.last_at),
      nextAt: time(w?.next_at),
    })).filter((w) => w.id),
    readers: list(m.readers).map((r) => ({
      id: text(r?.id, 128),
      kind: text(r?.kind, 16),
      label: text(r?.label, 80) || text(r?.id, 24),
      role: text(r?.role, 16),
      count: count(r?.count),
    })).filter((r) => r.id),
    cut: ["wikis", "sources", "workflows", "readers"].filter((k) => Array.isArray(m[k]) && m[k].length > LIST_MAX),
  };
}

/** The wikis the map names anywhere: its list, then any a flow or a workflow names. */
function wikisOf(map) {
  const known = new Map(map.wikis.map((w) => [w.id, w]));
  const named = [...map.sources.flatMap((s) => s.flows.map((f) => f.wiki)), ...map.workflows.flatMap((w) => w.wikis)];
  const extra = [...new Set(named)].filter((id) => !known.has(id)).map((id) => ({ id, pages: 0, updated: null, log: [], outputs: [] }));
  return [...map.wikis, ...extra].slice(0, LIST_MAX);
}

/** The sources the map names: its list, then any a workflow names that captured nothing yet. */
function sourcesOf(map) {
  const known = new Set(map.sources.map((s) => s.id));
  const named = [...new Set(map.workflows.flatMap((w) => w.sources))].filter((id) => !known.has(id));
  const extra = named.map((id) => ({ id, label: id, kind: "connector", provider: "", flows: [], items: [] }));
  const total = (s) => s.flows.reduce((n, f) => n + f.count, 0);
  return [...map.sources, ...extra].slice(0, LIST_MAX).sort((a, b) => total(b) - total(a) || a.label.localeCompare(b.label));
}

/** The edges, each pair once: a source's flows go through the workflow that brings them, when one does. */
export function edgesOf(map, wikis, sources) {
  const recentSince = map.takenAt === null ? null : map.takenAt - RECENT_MS;
  const edges = new Map();
  const add = (source, target, { n = 0, last = null, inferred = false, kind = "flow" } = {}) => {
    const id = `${source}->${target}`;
    const was = edges.get(id);
    const at = time(last);
    const newest = [was?.data.last ?? null, at].filter((t) => t !== null);
    const lastAt = newest.length ? Math.max(...newest) : null;
    edges.set(id, {
      id, source, target,
      data: { count: (was?.data.count ?? 0) + n, last: lastAt, inferred: (was?.data.inferred ?? false) || inferred, kind },
    });
  };
  const wikiIds = new Set(wikis.map((w) => w.id));
  for (const s of sources) {
    for (const f of s.flows.filter((f) => wikiIds.has(f.wiki))) {
      const wf = map.workflows.find((w) => w.sources.includes(s.id) && (w.wikis.length === 0 || w.wikis.includes(f.wiki)));
      if (wf) {
        add(`src:${s.id}`, `wf:${wf.id}`, { n: f.count, last: f.last, inferred: true });
        add(`wf:${wf.id}`, `wiki:${f.wiki}`, { n: f.count, last: f.last });
      } else {
        add(`src:${s.id}`, `wiki:${f.wiki}`, { n: f.count, last: f.last });
      }
    }
  }
  for (const wf of map.workflows) {
    for (const sid of wf.sources) if (!edges.has(`src:${sid}->wf:${wf.id}`)) add(`src:${sid}`, `wf:${wf.id}`, { inferred: true });
    const targets = wf.wikis.filter((id) => wikiIds.has(id)).map((id) => `wiki:${id}`);
    const out = [...edges.values()].some((e) => e.source === `wf:${wf.id}`);
    if (!out) for (const t of targets.length ? targets : ["brain"]) add(`wf:${wf.id}`, t, { inferred: true });
  }
  for (const w of wikis) add(`wiki:${w.id}`, "index", { kind: "search" });
  for (const r of map.readers) add("index", `reader:${r.id}`, { kind: "read" });
  return [...edges.values()].map((e) => ({
    ...e,
    animated: recentSince !== null && e.data.last !== null && e.data.last >= recentSince,
    label: e.data.count > 0 ? String(e.data.count) : undefined,
    className: ["bm-edge", `bm-edge-${e.data.kind}`, e.data.inferred ? "bm-inferred" : ""].filter(Boolean).join(" "),
  }));
}

/** A column of nodes, stacked and centred on `mid`. */
function column(items, x, height, mid, make) {
  const total = items.length * (height + GAP_Y) - GAP_Y;
  return items.map((it, i) => make(it, { x, y: mid - total / 2 + i * (height + GAP_Y) }));
}

/** The middle of the `i`th of `n` nodes `height` tall, in a column centred on `mid`. */
const centre = (i, n, height, mid) => mid - (n * (height + GAP_Y) - GAP_Y) / 2 + i * (height + GAP_Y) + height / 2;

/** `items` by `key` (a node's mean target height), ties in their order; one with no targets last. */
function byKey(items, key) {
  return items.map((it, i) => ({ it, i, k: key(it) })).sort((a, b) => a.k - b.k || a.i - b.i).map((x) => x.it);
}

const mean = (ys) => (ys.length ? ys.reduce((a, b) => a + b, 0) / ys.length : Infinity);

/** Each column ordered by where its edges go (a barycentre pass, right to left), so few edges cross. */
export function ordered(wikis, workflows, sources, edges, mid) {
  const wikiY = new Map(wikis.map((w, i) => [`wiki:${w.id}`, BRAIN.head + i * (H.wiki + GAP_Y) + H.wiki / 2]));
  const targetsOf = (id) => edges.filter((e) => e.source === id).map((e) => e.target);
  const flows = byKey(workflows, (w) => mean(targetsOf(`wf:${w.id}`).map((t) => wikiY.get(t) ?? mid)));
  const wfY = new Map(flows.map((w, i) => [`wf:${w.id}`, centre(i, flows.length, H.workflow, mid)]));
  const srcs = byKey(sources, (s) => mean(targetsOf(`src:${s.id}`).map((t) => wfY.get(t) ?? wikiY.get(t) ?? mid)));
  return { workflows: flows, sources: srcs };
}

/** React Flow's nodes and edges for a map.json, laid out. */
export function graph(raw) {
  const map = normalize(raw);
  const wikis = wikisOf(map);
  const sources = sourcesOf(map);
  const brainH = Math.max(BRAIN.head + wikis.length * (H.wiki + GAP_Y) - GAP_Y + BRAIN.pad, BRAIN.head + H.index + 2 * BRAIN.pad);
  const mid = brainH / 2;
  const xWorkflows = W + GAP_X;
  const xBrain = map.workflows.length ? xWorkflows + W + GAP_X : xWorkflows;
  const xReaders = xBrain + BRAIN_W + GAP_X;
  const node = (id, type, position, data, extra = {}) => ({ id, type, position, data, ...extra });
  const edges = edgesOf(map, wikis, sources);
  const totals = (id) => edges.filter((e) => e.target === id).reduce((n, e) => n + e.data.count, 0);
  const order = ordered(wikis, map.workflows, sources, edges, mid);
  const nodes = [
    node("brain", "brain", { x: xBrain, y: 0 }, { brain: map.brain, wikis: wikis.length }, { width: BRAIN_W, height: brainH, selectable: true }),
    ...wikis.map((w, i) => node(`wiki:${w.id}`, "wiki", { x: BRAIN.pad, y: BRAIN.head + i * (H.wiki + GAP_Y) }, { ...w, sources: totals(`wiki:${w.id}`) }, { parentId: "brain", extent: "parent", width: BRAIN.wikiW, height: H.wiki })),
    node("index", "index", { x: BRAIN.pad + BRAIN.wikiW + BRAIN.gap, y: Math.max(BRAIN.head, mid - H.index / 2) }, { ...map.brain.index, wikis: wikis.length }, { parentId: "brain", extent: "parent", width: BRAIN.indexW, height: H.index }),
    ...column(order.sources, 0, H.source, mid, (s, p) => node(`src:${s.id}`, "source", p, { ...s, total: s.flows.reduce((n, f) => n + f.count, 0), url: s.kind === "brain" ? null : map.brain.url }, { width: W, height: H.source })),
    ...column(order.workflows, xWorkflows, H.workflow, mid, (w, p) => node(`wf:${w.id}`, "workflow", p, { ...w, sourceLabels: w.sources.map((id) => sources.find((s) => s.id === id)?.label ?? id) }, { width: W, height: H.workflow })),
    ...column(map.readers, xReaders, H.reader, mid, (r, p) => node(`reader:${r.id}`, "reader", p, r, { width: W, height: H.reader })),
  ];
  const ids = new Set(nodes.map((n) => n.id));
  return { map, nodes, edges: edges.filter((e) => ids.has(e.source) && ids.has(e.target)) };
}

/** Everything `id`'s data passes through: upstream and downstream along the edges. */
export function lineage(edges, id) {
  const walk = (from, next, key) => {
    const seen = new Set([from]);
    const via = new Set();
    const queue = [from];
    while (queue.length) {
      const at = queue.shift();
      for (const e of edges.filter((e) => e[key === "down" ? "source" : "target"] === at)) {
        via.add(e.id);
        const n = next(e);
        if (!seen.has(n)) {
          seen.add(n);
          queue.push(n);
        }
      }
    }
    return { nodes: seen, edges: via };
  };
  const down = walk(id, (e) => e.target, "down");
  const up = walk(id, (e) => e.source, "up");
  return { nodes: new Set([...down.nodes, ...up.nodes]), edges: new Set([...down.edges, ...up.edges]) };
}
