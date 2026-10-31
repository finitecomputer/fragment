// The brain's AGENTS.md: how an agent keeps a brain, shown as AGENTS.md at
// the top of its viewer (api/file?path=AGENTS.md) and answered by its
// guide operation (app.mjs; fragment call <brain> guide --input '{}').
// It is the release's (decision 40), so every brain reads the
// same one and it updates with the platform; a wiki's own AGENTS.md, in the
// brain's files, adds to it. Adapted from Finite Brain's wiki skill
// (llm-wiki-finite and finitebrain) to the fragment CLI. Commands are
// indented blocks, so the text holds no backtick and stays one template
// literal.
export const GUIDE = String.raw`# This brain

This fragment is a brain: a knowledge base of markdown files that people
and their agents keep together. Its files are in git; the fragment CLI
syncs them, and the brain's search operation finds them by section. This
guide is the platform's, the same in every brain; a wiki's own AGENTS.md
adds to it, and wins where they differ. Read it again any time:

    fragment call <brain> guide --input '{}'

## Layout

Each top-level folder is a wiki: one subject, area or project, with its
own index and log.

    <wiki>/
      AGENTS.md     optional: this wiki's own conventions
      index.md      the wiki's map: every durable page, a line each
      log.md        what changed, appended, never rewritten
      raw/          sources as captured, never edited after
      raw/assets/   the bytes of sources that are not markdown
      wiki/         synthesized pages: the knowledge itself
      inventory/    open questions, source candidates, tasks, watch items
      datasets/     manifests, schemas, samples, query recipes
      output/       reports, plans, summaries and other deliverables

At the top level, fragment.json holds the brain's title, and _index.md is
the brain's own list of its wikis (made for you: never write one).
Everything else at the top level is a wiki.

## Working on a brain

Sync a folder with the brain, read, write with ordinary file tools, and
sync again. A brain needs no deploy: it reads its files at main. Only a
change to fragment.json (its title) needs one.

    fragment sync <brain> --dir ~/brains/<brain>
    fragment sync <brain> --dir ~/brains/<brain> --watch

Before writing, orient: this guide, the wiki's AGENTS.md, its index.md,
the end of its log.md, and a search for what you are about to add, so you
update a page rather than duplicate it.

## Ingest a source

1. Capture it once under the wiki's raw/, as markdown, named by date and
   subject (raw/2026-10-03-solar-tariffs.md), with frontmatter:

       ---
       type: source
       title: Solar tariffs in 2026
       source: https://example.com/article
       captured: 2026-10-03
       ---

   then its text, cleaned but not summarized. Raw is immutable: correct
   and interpret in wiki/, never in raw/.

2. A source that is not markdown (an image, a PDF, a recording) is an
   asset: put its bytes at raw/assets/<file> and give it one source note
   under raw/, whose resource names the file from the wiki's folder:

       ---
       type: asset
       title: Roof survey, south face
       resource: raw/assets/roof-survey.jpg
       description: The installer's photo survey.
       finite_asset:
         content_type: image/jpeg
         size: 2457600
         content_hash: sha256:<the file's SHA-256, hex>
       ---

   A file of 1 MiB or more is one of the brain's blobs: git holds a
   pointer and the bytes sit beside it. The CLI uploads them when it
   syncs, and the brain serves them at the file's path. Cite the note,
   not the bytes.

3. Synthesize into wiki/: update the pages it bears on, or make one per
   durable topic (wiki/solar-tariffs.md) with frontmatter title, summary,
   tags, sources (the raw notes it draws from), created and updated.
   Connect claims, dates, people and open questions; do not copy the
   source. Link pages with wikilinks by path from the brain's top
   ([[garden/wiki/solar-tariffs.md|Solar tariffs]]) or by file name
   ([[solar-tariffs]]).

4. Close the change: give every new page a line in index.md, append one
   entry to log.md (a dated heading, what was ingested, the pages it
   changed), put follow-ups in inventory/, sync, and search for what you
   added to see that it is found.

## Search

    fragment call <brain> search --input '{"q": "battery storage"}'
    fragment call <brain> search --input '{"q": "tariff", "wiki": "garden", "limit": 20}'

The answer is ranked sections, best first (BM25 over each section's text,
its headings, its page's title and its path):

    {"results": [{"rank", "path", "wiki", "title", "heading", "ancestry", "snippet"}], "pending"}

A query is plain words, at most 16 of them and 256 characters. Each word
must be in the section, as a word or the start of one, ignoring case and
accents. Search syntax means nothing: OR, NOT, NEAR, quotes, a colon or a
star are words or nothing. "wiki" narrows it to one wiki; "limit" is 1 to
50 (10 unless you ask). "pending" above 0 means the index is still taking
in a sync: ask again in a moment.

A result points at a section: open its page and read it whole before you
answer from it, and cite the pages and sources you used. When the brain
has no answer, say so, and say what to ingest.

    fragment call <brain> index_status --input '{}'

says how many pages and sections are indexed, what is pending, and which
pages were skipped and why (a page of 1 MiB or more is not indexed: split
it). The index is the brain's own, rebuilt from its files; a sync is all
it needs.

## A parent brain (the meta-brain convention)

A parent is an ordinary brain curated from child fragments, useful when
one executive overview draws on several department brains. References
grant no access. Synthesis publishes to the parent's audience: create it
with --visibility members, and keep everyone who can read it (including
its owner and agents) within every child's audience. Check this before
refreshing or changing sharing. Source names and citations disclose too.

Keep these ordinary files in the parent's wiki, for example executive/:

- datasets/sources.json: a list of full child names and selected paths:
      [{"fragment":"research--a1b2","paths":["team/wiki/launch.md"]},
       {"fragment":"delivery--c3d4","paths":["team/wiki/launch.md"]}]
- datasets/incorporated.json: the last successfully published revisions
  and page dependencies, keyed by child name:
      {"research--a1b2":{"revision":"<main SHA actually read>",
        "pages":["team/wiki/launch.md"],"outputs":["executive/wiki/launch.md"]}}
- wiki/launch.md: synthesis with status (current, stale or withdrawn),
  sources naming child, path and revision, and readable source links.
  For example: "Research and delivery both target November. Sources:
  [Research](https://<child-origin>/#/team/wiki/launch.md), main <SHA>;
  [Delivery](https://<other-child-origin>/#/team/wiki/launch.md), main <SHA>."
  Links open today's page; the cited SHA records what was read. Each
  child has its own read time, not a common snapshot.

Refresh only when asked, as the agent with viewer access to children and
editor access to the parent:

1. Read the parent's source list, incorporated file and existing outputs
   from the fragment, even after a restart. Reconcile current child state
   against them; a local working copy or a missed notification is no
   authority. Never acquire editor access just to sync a child.
2. Read each child's status pin (GET /api/f/<child>/status, pins.main),
   selected files (GET /api/f/<child>/file?path=...), then its status pin
   again. Require successful reads and equal nonempty pins; otherwise
   discard the attempt and retry from status, with a bounded retry count.
   If it keeps moving, report incomplete refresh and mark affected pages
   stale. Only record a revision whose files you actually read.
3. Synthesize into stable output paths, citing each child page and its
   verified revision. Update existing pages, index and log rather than
   appending duplicate summaries. Publish outputs and incorporated.json
   together in one file batch (POST /api/f/<parent>/files, at most 16
   files / 1 MiB); split larger work into independently consistent batches.
   On an uncertain write result, reread the parent before retrying. An
   unchanged source revision with matching outputs needs no new write.
4. Reconcile removals too: a deleted source page withdraws its dependent
   claims. If a child becomes unreadable, mark dependent pages stale or
   withdrawn and report incomplete coverage; never regenerate from cached
   copies. Withdraw the text when it should no longer be disclosed. Access
   revocation cannot recall old git versions, chats, exports or downloads.

For questions spanning children, call each child's existing search as the
reader (fragment call <child> search --input '{"q":"launch","limit":10}').
Bound the source count and results; group by source, report denied or
unavailable sources and pending indexes, and don't compare BM25 ranks
across brains. Reopen pages with the pin check before citing them. Search
snippets, answers and the chat receiving them need the same audience
check. The parent's browser search covers only its own synthesis. Child
references and a parent's job confer no reader credentials.

## Rules

- Raw is immutable; synthesis lives in wiki/.
- Each wiki's index.md and log.md speak of that wiki alone.
- Prefer updating a page to making a near-duplicate. Delete a page only
  when someone asks you to.
- Never write _index.md, app.mjs, applib/ or site/: the brain's code is
  the platform's, and a brain that carries code of its own is a fork.
- Everyone the brain is shared with (fragment members) reads all of it.
  There are no folder permissions and no encryption: put what a narrower
  group should see in a brain of its own.
- Write long pages in pieces, a section at a time.
`;
