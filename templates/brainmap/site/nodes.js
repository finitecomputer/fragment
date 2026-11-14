// The brain map's nodes, one React component a kind (graph.js names each
// node's `type`): a source, a workflow, the brain (a group holding its
// wikis and its search), a wiki, the search index, and a reader. Data flows
// left to right, so each takes edges on its left and gives them on its
// right. React escapes every string drawn here, whatever map.json holds.
import { createElement } from "react";
import { Handle, Position } from "@xyflow/react";
import htm from "htm";

export const html = htm.bind(createElement);

const PROVIDER_ICON = {
  gmail: "mail", drive: "doc", calendar: "calendar", telegram: "chat", slack: "chat", x: "post",
  github: "code", linear: "code", notion: "doc", youtube: "play", web: "globe", manual: "pen", upload: "clip",
};

// Small line icons, drawn at 16 px in the text's colour.
const PATHS = {
  mail: "M2 4h12v8H2z M2 4l6 5 6-5",
  doc: "M4 2h6l3 3v9H4z M10 2v3h3",
  calendar: "M2 4h12v10H2z M2 7h12 M5 2v3 M11 2v3",
  chat: "M2 3h12v8H7l-3 3v-3H2z",
  post: "M3 3l10 10 M13 3L3 13",
  code: "M6 4L2 8l4 4 M10 4l4 4-4 4",
  play: "M2 3h12v10H2z M7 6v4l3-2z",
  globe: "M8 2a6 6 0 100 12A6 6 0 008 2z M2 8h12 M8 2c-2 2-2 10 0 12 M8 2c2 2 2 10 0 12",
  pen: "M3 13l1-4 7-7 3 3-7 7z",
  clip: "M10 4l-5 5a2 2 0 003 3l6-6a3 3 0 00-4-4L4 8",
  brain: "M6 3a3 3 0 00-3 3v4a3 3 0 003 3h1V3z M10 3a3 3 0 013 3v4a3 3 0 01-3 3H9V3z",
  clock: "M8 2a6 6 0 100 12A6 6 0 008 2z M8 5v3l2 2",
  book: "M3 3h4a1 1 0 011 1v9a1 1 0 00-1-1H3z M13 3H9a1 1 0 00-1 1v9a1 1 0 011-1h4z",
  search: "M7 3a4 4 0 100 8 4 4 0 000-8z M10 10l4 4",
  agent: "M4 5h8v7H4z M8 2v3 M6 8h0 M10 8h0",
  person: "M8 3a2.5 2.5 0 100 5 2.5 2.5 0 000-5z M3 14c0-3 2-4.5 5-4.5s5 1.5 5 4.5",
  link: "M7 9l2-2 M6 6L4 8a2 2 0 003 3l2-2 M10 10l2-2a2 2 0 00-3-3L7 7",
};

export function Icon({ name }) {
  return html`<svg class="bm-icon" viewBox="0 0 16 16" aria-hidden="true"><path d=${PATHS[name] ?? PATHS.globe} /></svg>`;
}

export const plural = (n, one, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

const In = () => html`<${Handle} type="target" position=${Position.Left} isConnectable=${false} />`;
const Out = () => html`<${Handle} type="source" position=${Position.Right} isConnectable=${false} />`;

function SourceNode({ data }) {
  const icon = data.kind === "brain" ? "brain" : PROVIDER_ICON[data.id] ?? "globe";
  const wikis = data.flows.length;
  return html`<div class=${`bm-node bm-source bm-kind-${data.kind}`}>
    <div class="bm-row"><${Icon} name=${icon} /><strong>${data.label}</strong></div>
    <small>${data.total ? `${plural(data.total, "source")} into ${plural(wikis, "wiki")}` : "nothing captured yet"}</small>
    ${data.provider ? html`<small class="bm-faint">${data.provider}</small>` : null}
    <${Out} />
  </div>`;
}

function WorkflowNode({ data }) {
  return html`<div class=${`bm-node bm-workflow${data.paused ? " bm-paused" : ""}`}>
    <${In} />
    <div class="bm-row"><${Icon} name="clock" /><strong>${data.agent || "A routine"}</strong></div>
    <small>${data.paused ? "paused" : data.said}</small>
    <p class="bm-clamp">${data.text}</p>
    <${Out} />
  </div>`;
}

function BrainNode({ data }) {
  return html`<div class="bm-brain">
    <${In} />
    <div class="bm-brain-head">
      <div class="bm-row"><${Icon} name="brain" /><strong>${data.brain.title}</strong></div>
      <small>${data.brain.name}${data.brain.visibility ? ` · ${data.brain.visibility}` : ""}</small>
    </div>
  </div>`;
}

function WikiNode({ data }) {
  return html`<div class="bm-node bm-wiki">
    <${In} />
    <div class="bm-row"><${Icon} name="book" /><strong>${data.id}/</strong></div>
    <small>${plural(data.pages, "page")} · ${plural(data.sources, "source")} in</small>
    <${Out} />
  </div>`;
}

function IndexNode({ data }) {
  return html`<div class="bm-node bm-index">
    <${In} />
    <div class="bm-row"><${Icon} name="search" /><strong>Search</strong></div>
    <small>${data.line || `${plural(data.pages, "page")}, ${plural(data.sections, "section")}`}</small>
    ${data.pending ? html`<small class="bm-warn">${plural(data.pending, "page")} to index</small>` : null}
    <${Out} />
  </div>`;
}

function ReaderNode({ data }) {
  const icon = data.kind === "agent" ? "agent" : data.kind === "link" || data.kind === "public" ? "link" : "person";
  return html`<div class=${`bm-node bm-reader bm-kind-${data.kind}`}>
    <${In} />
    <div class="bm-row"><${Icon} name=${icon} /><strong>${data.label}</strong></div>
    <small>${[data.kind === "people" ? plural(data.count, "person", "people") : data.kind, data.role].filter(Boolean).join(" · ")}</small>
  </div>`;
}

export const NODE_TYPES = { source: SourceNode, workflow: WorkflowNode, brain: BrainNode, wiki: WikiNode, index: IndexNode, reader: ReaderNode };
