// The brain map's page: map.json (the app's `map`, a live query, so a new
// snapshot redraws every open page) drawn with React Flow. Selecting a node
// lights everything its data passes through, upstream and downstream
// (graph.js `lineage`), and opens its panel (panel.js).
import { useEffect, useMemo, useState } from "react";
import { createRoot } from "react-dom/client";
import { ReactFlow, ReactFlowProvider, Background, Controls, MiniMap } from "@xyflow/react";
import * as fragment from "./__fragment.js";
import { graph, lineage } from "./graph.js";
import { NODE_TYPES, html, plural } from "./nodes.js";
import { Panel } from "./panel.js";

// Below this width the minimap is left out and the panel is a sheet.
const NARROW_PX = 720;
const FIT = { padding: 0.12, maxZoom: 1.1 };
// a phone opens it readable, centred on the brain, and pans to the rest
const FIT_NARROW = { padding: 0.04, minZoom: 0.75, maxZoom: 1.1 };
// a node's colour in the minimap: its kind's accent (brainmap.css)
const MINIMAP = { source: "--bm-source", workflow: "--bm-workflow", wiki: "--bm-wiki", index: "--accent", reader: "--bm-reader", brain: "--line-strong" };
const minimapColor = (n) => `var(${MINIMAP[n.type] ?? "--line-strong"})`;

function ago(t) {
  if (t === null) return "";
  const s = Math.max(0, (Date.now() - t) / 1000);
  const [n, unit] = s < 90 ? [Math.round(s), "second"] : s < 5400 ? [Math.round(s / 60), "minute"] : s < 129600 ? [Math.round(s / 3600), "hour"] : [Math.round(s / 86400), "day"];
  return `${plural(n, unit)} ago`;
}

function useMap() {
  const [state, setState] = useState({ loading: true });
  useEffect(() => {
    const stop = fragment.live("map", {}, (r) => setState({ map: r.map, problem: r.problem ?? null }), (e) => setState({ problem: `The map didn't load: ${e.message}` }));
    const unclosed = fragment.closed(({ code }) => setState({ problem: code === 4004 ? "This map was deleted." : "Your access changed: reload to see it." }));
    return () => {
      stop();
      unclosed();
    };
  }, []);
  return state;
}

function useNarrow() {
  const query = `(max-width: ${NARROW_PX}px)`;
  const [narrow, setNarrow] = useState(() => matchMedia(query).matches);
  useEffect(() => {
    const m = matchMedia(query);
    const on = () => setNarrow(m.matches);
    m.addEventListener("change", on);
    return () => m.removeEventListener("change", on);
  }, [query]);
  return narrow;
}

function Header({ map }) {
  return html`<header class="bm-head">
    <div>
      <h1>${map.brain.title}</h1>
      <p>Where its knowledge comes from, and who reads it${map.takenAt !== null ? html` · <span title=${new Date(map.takenAt).toLocaleString()}>snapshot ${ago(map.takenAt)}</span>` : null}</p>
    </div>
    <ul class="bm-legend" aria-label="Legend">
      <li><i class="bm-key bm-key-recorded"></i>recorded</li>
      <li><i class="bm-key bm-key-inferred"></i>inferred</li>
      <li><i class="bm-key bm-key-recent"></i>last 7 days</li>
    </ul>
    ${map.sample ? html`<p class="bm-sample">Sample data. Run <code>scripts/snapshot.mjs</code> to map your own brain (README.md).</p>` : null}
    ${map.cut.length ? html`<p class="bm-sample">Too many ${map.cut.join(", ")} to draw them all.</p>` : null}
  </header>`;
}

function Canvas({ raw }) {
  const narrow = useNarrow();
  const [picked, setSelected] = useState(null);
  const { map, nodes, edges } = useMemo(() => graph(raw), [raw]);
  // a node a new snapshot left out is no longer selected
  const selected = nodes.some((n) => n.id === picked) ? picked : null;
  // the brain holds everything it is lit with, so selecting it dims nothing
  const lit = useMemo(() => (selected && selected !== "brain" ? lineage(edges, selected) : null), [edges, selected]);
  const shownNodes = useMemo(() => nodes.map((n) => ({ ...n, selected: n.id === selected, className: lit && n.type !== "brain" && !lit.nodes.has(n.id) ? "bm-dim" : "" })), [nodes, lit, selected]);
  const shownEdges = useMemo(() => edges.map((e) => ({ ...e, className: `${e.className}${lit ? (lit.edges.has(e.id) ? " bm-lit" : " bm-dim") : ""}` })), [edges, lit]);
  const node = nodes.find((n) => n.id === selected) ?? null;
  return html`<div class=${`bm-app${narrow ? " bm-narrow" : ""}`}>
    <${Header} map=${map} />
    <div class="bm-canvas">
      <${ReactFlow}
        nodes=${shownNodes} edges=${shownEdges} nodeTypes=${NODE_TYPES}
        colorMode="system" fitView=${true} fitViewOptions=${narrow ? FIT_NARROW : FIT} minZoom=${0.2}
        nodesDraggable=${false} nodesConnectable=${false} elementsSelectable=${true}
        onNodeClick=${(_, n) => setSelected(n.id === selected ? null : n.id)}
        onPaneClick=${() => setSelected(null)}>
        <${Background} gap=${24} />
        <${Controls} showInteractive=${false} />
        ${narrow ? null : html`<${MiniMap} pannable=${true} zoomable=${true} nodeColor=${minimapColor} nodeStrokeWidth=${0} />`}
      <//>
      <${Panel} node=${node} url=${map.brain.url} onClose=${() => setSelected(null)} />
    </div>
  </div>`;
}

function Missing({ problem }) {
  return html`<main class="bm-missing">
    <h1>No map yet</h1>
    <p>${problem}</p>
    <p>Sync a brain, then write its map here: <code>node scripts/snapshot.mjs &lt;brain&gt; --dir &lt;its folder&gt; --out map.json</code> and <code>fragment write &lt;this map&gt; map.json --from map.json</code>.</p>
  </main>`;
}

function App() {
  const state = useMap();
  if (state.loading) return html`<p class="bm-loading">Loading the map…</p>`;
  if (!state.map) return html`<${Missing} problem=${state.problem ?? "map.json is empty."} />`;
  return html`<${ReactFlowProvider}><${Canvas} raw=${state.map} /><//>`;
}

createRoot(document.getElementById("root")).render(html`<${App} />`);
