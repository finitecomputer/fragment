// The brain map's side panel: what the selected node holds, as the map
// says it. A page in the brain links to it (the brain's files view opens a
// path from its hash: `<brain>/#/<path>`), only when the map's brain URL is
// http(s) (graph.js `safeUrl`).
import { html, plural } from "./nodes.js";

const at = (t) => (t === null ? "never" : new Date(t).toLocaleString());

function PageLink({ url, path, title }) {
  const label = title || path;
  return url && path ? html`<a href=${`${url}#/${path}`} target="_blank" rel="noopener">${label}</a>` : html`<span>${label}</span>`;
}

function SourcePanel({ data }) {
  return html`
    <h2>${data.label}</h2>
    <p class="bm-muted">${data.kind === "brain" ? "Another brain this one draws on" : data.provider || "A source"}</p>
    <dl>
      ${data.flows.map((f) => html`<div key=${f.wiki}><dt>${f.wiki}/</dt><dd>${plural(f.count, "source")}${f.last ? `, newest ${f.last}` : ""}</dd></div>`)}
    </dl>
    ${data.items.length ? html`<h3>Captured</h3><ul class="bm-list">
      ${data.items.map((i) => html`<li key=${i.path}><${PageLink} url=${data.url} path=${i.path} title=${i.title} /> <small>${i.captured ?? ""}</small></li>`)}
    </ul>` : null}`;
}

function WorkflowPanel({ data }) {
  return html`
    <h2>${data.agent || "A routine"}</h2>
    <p class="bm-muted">${data.paused ? "Paused" : data.said}</p>
    <blockquote>${data.text}</blockquote>
    <dl>
      <div><dt>Reads</dt><dd>${data.sourceLabels.join(", ") || "—"}</dd></div>
      <div><dt>Into</dt><dd>${data.wikis.map((w) => `${w}/`).join(", ") || "the brain"}</dd></div>
      <div><dt>Last run</dt><dd>${at(data.lastAt)}</dd></div>
      ${data.paused ? null : html`<div><dt>Next run</dt><dd>${at(data.nextAt)}</dd></div>`}
    </dl>
    <p class="bm-note">Which sources and wikis a routine touches is read from its words, so these links are a guess (dashed).</p>`;
}

function WikiPanel({ data, url }) {
  return html`
    <h2>${data.id}/</h2>
    <p class="bm-muted">${plural(data.pages, "page")} · ${plural(data.sources, "source")} in${data.updated ? ` · updated ${data.updated}` : ""}</p>
    ${data.log.length ? html`<h3>Its log</h3><ul class="bm-list">
      ${data.log.map((e, i) => html`<li key=${i}><small>${e.date}</small> ${e.title}</li>`)}
    </ul>` : null}
    ${data.outputs.length ? html`<h3>Deliverables</h3><ul class="bm-list">
      ${data.outputs.map((o) => html`<li key=${o.path}><${PageLink} url=${url} path=${o.path} title=${o.title} /></li>`)}
    </ul>` : null}`;
}

function IndexPanel({ data }) {
  return html`
    <h2>Search</h2>
    <p class="bm-muted">${data.line || "The brain's own index"}</p>
    <dl>
      <div><dt>Pages</dt><dd>${data.pages}</dd></div>
      <div><dt>Sections</dt><dd>${data.sections}</dd></div>
      <div><dt>To index</dt><dd>${data.pending}</dd></div>
    </dl>
    <p class="bm-note">Every reader on the right searches it by words and by meaning: <code>fragment search --in</code>, the brain's page, or an agent's <code>search</code> call.</p>`;
}

function ReaderPanel({ data }) {
  const what = { agent: "An agent with access", people: plural(data.count, "person", "people"), link: "Anyone who holds the link", public: "Anyone" };
  return html`
    <h2>${data.label}</h2>
    <p class="bm-muted">${what[data.kind] ?? data.kind}${data.role ? ` · ${data.role}` : ""}</p>
    <p class="bm-note">Everyone a brain is shared with reads all of it: there are no folder permissions.</p>`;
}

function BrainPanel({ data }) {
  return html`
    <h2>${data.brain.title}</h2>
    <p class="bm-muted">${data.brain.name}${data.brain.visibility ? ` · ${data.brain.visibility}` : ""}</p>
    ${data.brain.url ? html`<p><a href=${data.brain.url} target="_blank" rel="noopener">Open the brain</a></p>` : null}
    <p>${plural(data.wikis, "wiki")}, ${plural(data.brain.index.pages, "page")} searchable.</p>`;
}

const PANELS = { source: SourcePanel, workflow: WorkflowPanel, wiki: WikiPanel, index: IndexPanel, reader: ReaderPanel, brain: BrainPanel };

export function Panel({ node, url, onClose }) {
  if (!node) return null;
  const Body = PANELS[node.type];
  return html`<aside class="bm-panel" aria-label="Details">
    <button type="button" class="bm-close" aria-label="Close" onClick=${onClose}>×</button>
    <${Body} data=${node.data} url=${url} />
  </aside>`;
}
