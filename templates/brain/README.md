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
sections, best first: `{q, words, results: [{rank, path, wiki, title,
heading, ancestry, snippet}], pending}`.

- **The index** is FTS5 in the app's own SQLite (`unicode61`, accents
  removed): a row per section (the text under a heading, with the headings
  above it and the page's title, `applib/sections.mjs`), ranked by BM25
  weighted title 4, heading 3, path and ancestry 2, body 1. It indexes
  markdown, not a wiki's `AGENTS.md` or a made `_index.md`.
- **Kept by a file trigger.** `{"files": "**", "run": "changed"}`: each
  move of `main` runs the job `changed`, which marks the paths that moved
  (`mark`, a mutation, which open pages follow through `last_change`),
  then indexes them 32 files a step (`reindex`, an editor's query that
  reads the files at `main` and writes the index) until none are left.
  Each path is marked at a generation, so a read overtaken by a newer
  mark writes nothing. A page whose bytes hash the same keeps its sections:
  the same files again change nothing. More than 200 paths in one move,
  or an index of an older shape (`FORMAT`), read every file again. A
  search first takes up to 8 pending files itself, so an index behind a
  sync catches up even when no trigger runs.
- **A query is words**: runs of letters, digits and marks, each a quoted
  prefix phrase, all required. FTS5's syntax (`OR`, `NOT`, `NEAR(…)`,
  quotes, `column:`, `*`, `^`) is never read.
- **Limits.** `q` is 1 to 256 characters (the schema: 400 past it) and at
  most 16 words (422); `limit` 1 to 50 (10 by default); a snippet at most
  300 bytes. A page of 1 MiB or more is a blob, not read, and is listed as
  skipped; a page keeps at most 500 sections of 8000 characters. Past 12
  MiB of database the index takes no new page (they are skipped "the index
  is full", and tried again at their next change), so the app's mutations
  keep room under its 16 MiB.
- `index_status {}` answers `{format, pages, sections, pending, writes,
  skipped}`; `reindex {full: true}` (editors) reads every file again.

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

## Search the current local folder

    fragment search garden--k3x9 'orchard pruning' --dir ./garden
    fragment search garden--k3x9 'orchard pruning' --dir ./garden --limit 5 --json

This CLI search reads the actual working files without syncing or contacting
any server, and needs no login or watcher. Each invocation rebuilds a disposable
SQLite FTS5 index **in memory**, outside the synced content. Unsynced edits and
new Markdown files are included; deleted files disappear immediately. There is
no disk index to invalidate or repair. A missing, old or corrupt sync journal
makes files `local_only`; a valid journal naming another fragment is refused.
The supplied name identifies the source; without a journal it is a label, not
proof that the folder has ever synced to that fragment.

`--json` uses the CLI's usual `{ok:true,data:…}` envelope. Data is
`{q, words, results, skipped}`; each result has `{rank, fragment, path, title,
heading, ancestry, state, snippet}`. Rank starts at 1, ancestry is an array of
parent headings, heading is null for a page's introductory text, and snippet
is a plain excerpt of at most 300 UTF-8 bytes. States are:

- `synced`: bytes match the **last local sync journal**. This does not verify
  the server's current version or a deployed release.
- `local_only`: a new local file or bytes changed since the last sync.
- `conflicted`: sync recorded divergence, a sibling `.conflict-…` copy remains,
  or the working file contains merge conflict markers. Search returns the
  working file's text with this warning; remote conflict copies are evidence,
  not separate search hits. Sync clears its journal flag when it next adopts,
  pushes or pulls agreed content; retained copies keep warning until removed.
  Older journals cannot report a conflict whose copy was never saved.

Query words use the server search's NFKC/lowercase, all-required prefix
semantics, with no FTS operators: 1–256 characters, 1–16 words, and `--limit`
1–50 (default 10). Ranking uses BM25 with title 4, heading 3, path and ancestry
2, body 1, with stable path/section tie breaks. Local files obey sync's ignore
rules and never follow symlinks. Brain machinery, `AGENTS.md` and `_index.md`
are excluded, as on the server. Pages at least 1 MiB and non-UTF-8 pages are
reported in `skipped`; only the first 500 sections per page are retained
(also reported). Other read failures fail the command rather than silently
returning an incomplete answer.

The local parser uses CommonMark (including Setext headings), ignoring headings
inside code fences. It recognizes scalar frontmatter title, summary,
description and inline tags; unlike the server parser it does not interpret
multiline YAML tags. Long sections split at 8000 Unicode characters, including
inside paragraphs. Local and server rank values are not interchangeable.
Rebuilding means each search reads every eligible page: this favors freshness
and a simple rebuildable interface over incremental performance. A search is
not an atomic filesystem snapshot; files edited during a scan can reflect
different moments. It never changes working files or the sync journal.

## CLI collections

A collection is a trusted local JSON manifest of references to independently
shared fragments. Source membership still controls each source; a reference
adds no access. Sync each source separately first, using your own identity.
The collection commands only read and search those existing local folders.
They never sync, publish, edit, or copy content into another knowledge repo.
The existing `fragment sync` storage-token endpoint requires an editor role,
even for `--mode pull`. Collections do not change that acquisition requirement;
an online collection access check itself works for viewers with retained copies.

For example, save `collection.json` beside your two synced folders:

```json
{
  "version": 1,
  "host": "https://finite.chat",
  "sources": [
    {"alias": "personal", "fragment": "garden--k3x9", "dir": "garden"},
    {"alias": "team", "fragment": "research--k3x9", "dir": "research"}
  ]
}
```

```sh
fragment collection --manifest collection.json list
fragment collection --manifest collection.json search 'orchard pruning' --json
fragment collection --manifest collection.json --offline search 'orchard pruning'
```

Directories resolve relative to the manifest (absolute paths are also allowed).
The manifest accepts version 1, 1–32 sources, unique aliases of 1–64 ASCII
letters/digits/`-`/`_`, and full fragment names. Unknown fields are refused;
the manifest is limited to 64 KiB. Its host must match the CLI's selected host;
it never selects a destination for credentials. Use the ordinary `--host`
option when browsing a collection on a different host. Each source needs a
valid sync journal bound to the manifest's host and a repository. An online
check also verifies that repository identity against the source's status.
Missing, corrupt, mismatched or old unbound journals require a separate sync.

**Audience and trust:** keep this manifest private by default. It contains
private source names, aliases, and local filesystem paths. Sharing it reveals
those names even to someone who cannot read the sources. If stored in a
fragment, every reader of that fragment can see all its metadata: choose an
audience allowed to know every reference, or keep separate manifests for
separate audiences. Aliases are display labels, not redaction or permissions.
Only open manifests you trust: they select local directories to read. Do not
automatically run a downloaded or shared manifest. Output and snippets inherit
the source content's audience; do not publish them into a broader collection.
There is no platform Mount entity, path ACL, browser federation, or new agent
capability. Hermes can use these ordinary CLI commands under its existing
identity and delegated authority.

Online mode checks each source with a signed, read-only status request before
opening its local journal or searching. `checked` means access was verified at
that moment, **not** that local bytes equal current remote content. A source
refused with 401/403/404/410 is `denied` (which can mean revoked or absent).
Other failures are `unavailable`; neither uses a local fallback. Partial results
still succeed, including zero accessible sources: always inspect `sources`.
This command does not erase local files on a refusal.

`--offline` needs no login and makes no requests: each source says
`unchecked_offline`. Like reading an existing local sync folder, it can read
bytes retained before revocation. It cannot discover revocations or retract
previously downloaded knowledge. Use online mode for a current access check;
never treat offline output as proof of membership. Access can also change after
an online check; this command is not an atomic authorization/filesystem snapshot.

JSON data is `{host, sources, q, results}` in the normal CLI envelope. Each
source reports `alias`, `fragment`, `dir`, `access`, `local`, `reason`, and
`skipped`. Local disposition is `journal_bound` when listed, `searched_current`
after a successful scan, or `unavailable` if it could not be read. Search hits
reuse the local search's provenance, section, snippet and file `state` fields,
adding `alias` and `source_rank`. File state `synced` still means only equality
to the last local journal. Results interleave each source's ranked hits in
manifest order, capped by the global `--limit` (1–50, default 10); independent
BM25 scores are not a global relevance ranking. Current edits and deletions,
conflicts, exclusions and page limits behave exactly as single-source local
search does. No persistent index or collection state is written.
