# brainmap

A map of one brain (templates/brain): where its knowledge comes from and
who reads it, drawn with [React Flow](https://reactflow.dev/). Left to
right, as knowledge moves:

1. **Sources**: the connectors its sources came through (Gmail, Google
   Drive, Telegram, the web, X…), and the child brains a parent draws on.
2. **Workflows**: agents' routines (`fragment schedule list <agent>`) that
   name the brain.
3. **The brain**: its wikis, each feeding its search.
4. **Readers**: its agents by name, its people as one count, and anyone
   its link or the public lets in.

An edge's number is how many sources moved along it. A solid edge is
recorded in the brain's files; a dotted one is inferred from a routine's
words; a moving one carried something captured in the week before the
snapshot. Select a node to light everything its data passes through,
upstream and downstream, and to read what it holds in the panel.

    fragment new my-map --template brainmap
    fragment create my-map --visibility members
    fragment deploy my-map --dir my-map

It opens on a sample map (`map.json`, `"sample": true`) until you write
one of your own.

## Mapping a brain

An app reads only its own files, never a brain's, so a map is made by
someone who can read the brain (its owner, or their agent), as a parent
brain is (the brain's AGENTS.md, "A parent brain"):

    fragment sync <brain> --dir ~/brains/<brain>
    node scripts/snapshot.mjs <brain> --dir ~/brains/<brain> --agent <agent> --out map.json
    fragment write <my-map> map.json --from map.json

Each write of `map.json` redraws every open page (its file trigger runs
`touched`, whose commit answers the page's live `map` again). Write it as
often as you like within the platform's 120 triggered runs an hour; a
routine of the agent's own can keep it fresh.

`scripts/snapshot.mjs` (Node 22 or newer, no packages) reads:

- the synced folder (`scripts/brain-folder.mjs`): each top-level folder a
  wiki; its `raw/` notes' frontmatter (`source`, `captured`, `via`), its
  `wiki/` pages, `output/` deliverables and `log.md`; a parent wiki's
  `datasets/sources.json`;
- `fragment status`, `fragment call <brain> index_status` and
  `fragment members list`;
- for each `--agent`, `fragment status` (its name and npub) and
  `fragment schedule list` (its routines).

## What is recorded and what is guessed

A brain records where each source came from (`source`, a URL) but not
through what: the connector is the note's `via` when it has one
(`via: gmail`), else a guess from the URL's host, else "Written in". A
routine's sources and wikis are guessed from its words. To make an edge
recorded rather than guessed, give raw notes a `via`.

What no brain records yet, so no map shows: who wrote each page (the
fragment's git history), and who searched it, when, for what.

## The map

`map.json`, version 1, at most 512 KiB (`app.mjs` `MAP_MAX_BYTES`). The
page reads it as untrusted (`site/graph.js` `normalize`): it draws at
most 64 of each list, and links only to http(s).

    {
      "version": 1,
      "sample": false,
      "taken_at": "2026-11-10T15:00:00Z",
      "brain": {"name", "title", "url", "visibility",
                "index": {"line", "pages", "sections", "pending"}},
      "wikis": [{"id", "pages", "updated", "log": [{"date", "title"}],
                 "outputs": [{"path", "title"}]}],
      "sources": [{"id", "label", "kind": "connector" | "brain", "provider",
                   "flows": [{"wiki", "count", "last"}],
                   "items": [{"path", "title", "captured"}]}],
      "workflows": [{"id", "agent", "agent_title", "said", "text",
                     "sources": [<source id>], "wikis": [<wiki id>],
                     "paused", "last_at", "next_at"}],
      "readers": [{"id", "kind": "agent" | "people" | "link" | "public",
                   "label", "role", "count"}]
    }

A flow goes through the first workflow that names both its source and its
wiki (or its source and no wiki), else straight to its wiki.

## Who should see it

A map discloses the brain's wiki names, source titles, its agents' names
and routines. Keep its audience inside the brain's: create it with
`--visibility members` and share it with no one the brain is not shared
with. The snapshot copies no token, no member's email or npub (people are
one count) and no source's body; it does copy titles, log headings and
routines' words as written, so whatever those say, the map says.

## Why a CDN

The page imports React 19.3.1, React Flow (`@xyflow/react`) 12.12.0 and
htm 3.1.1 from esm.sh, each version exact, through an import map
(`site/index.html`), so the template needs no build and no `package.json`.
The cost: a viewer's browser fetches them from esm.sh and jsDelivr, and
the page does not draw when those are down. A template the platform
serves would bundle them instead (crates/templates/bundle.rs).

## Files

- `fragment.json`: `map` (a query) and `touched` (a mutation, and the
  trigger on `map.json` that runs it).
- `app.mjs`: reads and checks `map.json`.
- `map.json`: the sample map.
- `site/index.html`, `site/brainmap.js` (the page), `site/graph.js`
  (nodes, edges, layout and lineage, tested in Node:
  crates/templates/tests/brainmap_graph.test.mjs), `site/nodes.js`,
  `site/panel.js`, `site/brainmap.css`.
- `scripts/snapshot.mjs`, `scripts/brain-folder.mjs` (tested:
  crates/templates/tests/brainmap_snapshot.test.mjs).
