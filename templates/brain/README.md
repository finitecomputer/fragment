# brain

The blessed brain template (docs/cloudflare-v1.md, decisions 30 and 40): a
brain is a fragment app whose files are a knowledge base that people and
their agents keep together, searched by section. A brain's repo names the
template and holds only its face (`fragment.json`'s `meta`) and its files;
the platform's release serves the rest, so every brain runs the same code
and one deploy updates them all. The platform knows nothing of brains:
everything here is this template's app code and files.

Make one with the CLI (yours, or your agent's) or the API; the shell
makes none (its Add suggests what to ask an agent for: `APP_PROMPTS` in
cell/shell/shell.js):

    fragment create garden --template brain --title Garden
    POST /api/fragments {"label": "garden", "template": "brain", "title": "Garden"}

A create names a label, and the platform adds its suffix (`garden--k3x9`;
a `name` must be one in full: docs/api.md, Names).

## The files

Kept as Finite Brain keeps them (its wiki skill, adapted to the fragment
CLI in the brain's AGENTS.md, `applib/guide.mjs`):

- Each top-level folder is a wiki: `raw/` (sources as captured, never
  edited), `wiki/` (synthesized pages), `inventory/`, `datasets/`,
  `output/`, with its own `index.md` and append-only `log.md`, and an
  optional `AGENTS.md` of its own conventions.
- An asset (an image, a PDF, a recording) is a file under `raw/assets/`
  with one source note under `raw/` whose frontmatter names it
  (`type: asset`, `title`, `resource: raw/assets/<file>`, and
  `finite_asset: {content_type, size, content_hash}`). A file of 1 MiB or
  more is one of the fragment's blobs (a pointer in git: docs/api.md,
  Blobs), which the CLI uploads when it syncs, and which a pointer at
  `main` keeps: no record needs to name it.
- Agents sync a folder with `fragment sync <brain> --dir <folder>` and
  write with ordinary file tools; a brain needs no deploy (it reads its
  files at `main`).
- Access is the fragment's members: everyone it is shared with reads all
  of it. There are no folder permissions and no encryption.

## Search

`search {q, limit?, wiki?}` (a query, viewers and up) answers ranked
sections, best first, by words and by meaning: `{q, results: [{rank, path,
wiki, title, heading, ancestry, snippet, signals}], semantic, pending,
indexing, waiting_ms, hint?}`.

- **The index** is the fragment's own (docs/api.md, "A fragment's
  search"), written and asked through the app's `this.searchIndex`: each
  markdown page a source (its path, its bytes' sha256 the revision), cut
  into sections (the text under a heading, with the headings above it as
  context and the page's title: `applib/sections.mjs`), its wiki a field
  that `wiki` narrows by. It indexes markdown, not a wiki's `AGENTS.md`
  or a made `_index.md`. A question asked through the brain (its page, or
  this operation) is paid by its owner; `fragment search --in` by its
  asker.
- **Kept by a file trigger.** `{"files": "**", "run": "changed"}`: each
  move of `main` runs the job `changed`, which marks the paths that moved
  (`mark`, a mutation, which open pages follow), then indexes them 8 files
  a step (`reindex`, a query that reads the files at `main` and writes the
  index) until none are left for it, going on in a run of its own past one
  run's steps. An installed trigger (`{"installed": true, "run":
  "changed"}`) runs it once per install of a new release too, at the
  brain's next request, so a release that changes the index's shape
  (`FORMAT`) is taken in with no member acting. A batch claims its pages
  (for two minutes), so runs side by
  side take different pages and nothing is read or embedded twice: any
  member may start `changed` (the page does when the index is behind), and
  one with nothing left to do takes two steps. A batch that fails lets go
  of its claims; a run that finds the rest held waits out one claim, then
  takes what a batch that died left. Each path is marked at a generation,
  and a new mark keeps a batch's claim: one batch at a time reads a page,
  and a read overtaken by a newer mark is read again. Each write and
  delete carries its batch's generation as a fence (docs/api.md, "A
  fragment's search"), so a batch whose claim lapsed never lands a write
  or a delete over a newer batch's. Every page a batch takes is written
  through its fence, its bytes unchanged too: the index writes (and
  embeds) nothing for a revision it holds, but takes the newer fence, so
  an older batch's write still on its way is refused. A page `main` deletes
  is taken out, its tombstone kept. More than 200 paths in one move, an
  index of an older shape (`FORMAT`, which drops the app's old tables),
  `reindex {full: true}` (an editor's), or a write the index answers
  `cleared` (its embedding model changed and it emptied) take every page
  again.
- **Behind its files**, a search answers `indexing: true` and a `hint`
  naming the job that finishes it, and the page says so.
- **A pause the search service asks for** (429 `rate_limited`, as a rate
  limit answers, with its `retry_after_ms`: kept within a second and ten
  minutes, a minute if it names none) fails nothing: the batch lets go of
  its claims, no batch claims until the pause is over, and a run sleeps it
  out (a durable step, in the run: a rebuild may meet more rate windows
  than a chain has hops) and goes on. Meanwhile a search answers
  `waiting_ms`, what is left of it, its `hint` says so, and the page says
  "Indexing: waiting for the search service." and starts no run.
- **Limits.** `q` is 1 to 256 characters; `limit` 1 to 50 (10 by default).
  A page of 1 MiB or more is a blob, not read, and is listed as skipped;
  a page keeps at most 500 sections of 8000 characters. The index holds
  at most 50,000 chunks and should be split into child brains past 25,000
  (a page refused for room is skipped "the index is full", and tried
  again at its next pass).
- `index_status {}` answers `{format, pages, sections, pending, indexing,
  running, waiting_ms, writes, skipped, index: {chunks, chunks_split,
  chunks_max, semantic}}`; `reindex {full: true}` (editors) reads every file again.

## The page

`site/` is the notes template's viewer (`site/assets`, a symlink to
`templates/notes/site/assets`, embedded once: crates/templates/build.rs),
which reads `api/tree` and `api/file` from `app.mjs`, plus the brain's
search box (`brain.js`, `brain.css`). The viewer is plain source and
renders markdown with the platform's renderer (`./__markdown.js`: DOM
only, so a note's HTML shows as text). `api/tree` lists the
brain's files and two it makes: `_index.md` (its wikis, the landing) and
`AGENTS.md` (the guide), each unless the brain holds one of its own. An
asset's bytes come from the platform's `__file`.

## Parent brains

A meta-brain is a convention on this template: an agent explicitly refreshes
synthesis from child fragments, citing their pages and the revisions read.
The served guide (`applib/guide.mjs`, “A parent brain”) defines the source
list, incorporated revision file and restart reconciliation. Synthesis is
publication: keep the parent members-only and its audience inside every
child's. A denied child makes dependent pages stale or withdrawn; cached
copies must never feed a new synthesis. Search a parent and its children
as one, as the reader: `fragment search --in <parent>,<children>`; browser
search covers the parent alone.
