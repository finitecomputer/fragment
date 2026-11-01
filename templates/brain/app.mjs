// The brain template's code (docs/cloudflare-v1.md, decision 30). A brain is
// a fragment app whose files are the knowledge, laid out as Finite Brain
// lays it out: each top-level folder a wiki (raw/ wiki/ inventory/
// datasets/ output/, index.md, log.md), and the brain's AGENTS.md
// (applib/guide.mjs) on how an agent keeps one. It runs from the platform's
// release (decision 40): a brain's repo holds only its face (fragment.json)
// and its files.
//
// Search is the fragment's own index (docs/api.md, "A fragment's search"),
// by words and by meaning, written and asked through `this.searchIndex`:
// each markdown page a source (its path; its bytes' sha256 under `FORMAT`
// the revision, so a new format writes every page again), cut into sections
// by applib/sections.mjs, a section's wiki links as their words (`linkWords`:
// so every reader of the index, ⌘K's Knowledge, `fragment search` and the
// brain's own page, shows "frost-dates", not "[[frost-dates]]"). Each move
// of main runs `changed` (fragment.json's file trigger), and so does each install of a
// new release (its installed trigger: an index of an older `FORMAT` is
// rebuilt with no one acting), a job: it marks the paths that
// moved (`mark`, a mutation, which open pages follow), then takes them in
// batches (`reindex`, a query that reads the files at main and writes the
// index) until none are left for it, and goes on in a run of its own past
// one run's steps. A batch claims its pages (a claim lapses in
// `CLAIM_MS`), so runs side by side take different pages and nothing is
// read or embedded twice: any member may start `changed` (the page does,
// when the index is behind), and a start with nothing left to do takes two
// steps. Each write and delete carries its batch's generation as a fence,
// and the index refuses one below the last it took for the page, so a
// batch that read a page before a newer one wrote or deleted it never
// lands over that. Every page a batch takes is written through its fence,
// its bytes unchanged too: the index writes (and embeds) nothing for a
// revision it holds, but takes the newer fence, so an older batch's write
// still on its way is refused. A full pass (an editor's, or an index its
// embedding model emptied) takes every page; one main deletes leaves the
// index.
//
// The viewer (site/) is the notes template's, reading api/tree and
// api/file here, with a search box of the brain's own beside it.
import { DurableObject } from "cloudflare:workers";
import { GUIDE } from "./applib/guide.mjs";
import { SECTIONS_MAX, linkWords, sections, stem } from "./applib/sections.mjs";

// The index's shape: a brain whose index is another rebuilds it from its files.
// 2: a section's wiki links as their words (FIN-367)
const FORMAT = "brain-index-2";
// Capability calls (`this.files`, `this.searchIndex`) one call into the
// app may make: one subrequest each (measured on a preview, 2026-11-01).
// The platform once held a call to 50 subrequests, which Cloudflare counts
// for a facet's whole life instead (docs/technical-debt-ledger.md, "An app
// facet's subrequests are the account's"); a batch keeps to them still.
export const CALLS_MAX = 50;
// A page's calls in a batch: its listing, its read, and its write or delete.
const PAGE_CALLS = 3;
// A full pass's own call, before its first batch: the listing of every file.
const PASS_CALLS = 1;
// Files one reindex reads and writes: as many as its calls allow, and no
// more than 8 (each embedded: well inside a call's time).
const BATCH = Math.min(8, Math.floor((CALLS_MAX - PASS_CALLS) / PAGE_CALLS));
// A run's reindex steps (a job takes 256 steps; mark is one, and the next
// run's start another).
const STEPS_MAX = 240;
// Runs that go on from one another (a run's job call is one hop deeper, and
// 16 hops is the platform's bound).
const HOPS_MAX = 12;
// The paths one mark takes (what a file trigger passes: past it, `more`).
const PATHS_MAX = 200;
// A page is read whole up to the platform's read limit; a larger one is a blob.
const PAGE_MAX_BYTES = 1024 * 1024;
// A batch's claim on its pages: past this, another batch may take them.
const CLAIM_MS = 2 * 60 * 1000;
// A wait the search service asks for (429 `rate_limited`, its
// `retry_after_ms`), within these bounds; one that names none, a minute (a
// rate window's).
const WAIT_DEFAULT_MS = 60 * 1000;
const WAIT_MIN_MS = 1000;
const WAIT_MAX_MS = 10 * 60 * 1000;
const RESULTS_DEFAULT = 10;
const RESULTS_MAX = 50;
const SKIPPED_LISTED = 20;
// What a source takes (fragment_proto::search and fragment_core::searchable).
const SOURCE_MAX_BYTES = 512;
const TITLE_MAX_BYTES = 512;
const HEADING_MAX_BYTES = 512;
const CONTEXT_MAX = 8;
const CONTEXT_ITEM_MAX_BYTES = 256;
const FIELD_MAX_BYTES = 64;

// the fragment's own machinery, which is not the brain's knowledge
const hidden = (p) =>
  p === "fragment.json" || p === "app.mjs" || p.startsWith("site/") || p.startsWith("applib/") ||
  p.startsWith(".") || p.includes("/.") || p.includes(".conflict-");
// what search reads: markdown, but neither a wiki's instructions nor a made index
const indexable = (p) => /\.(md|markdown)$/i.test(p) && !hidden(p) && !/(^|\/)(AGENTS|_index)\.md$/i.test(p);
const wikiOf = (p) => (p.includes("/") ? p.slice(0, p.indexOf("/")) : "");
const editor = (role) => role === "editor" || role === "owner";
const NOTHING_DONE = Object.freeze({ indexed: 0, unchanged: 0, removed: 0, skipped: 0, stale: 0 });
// The wait a 429 asks for, within bounds.
const waitFor = (ms) => (Number.isFinite(ms) && ms > 0 ? Math.min(Math.max(Math.ceil(ms), WAIT_MIN_MS), WAIT_MAX_MS) : WAIT_DEFAULT_MS);

const MIME = {
  md: "text/plain; charset=utf-8", markdown: "text/plain; charset=utf-8", txt: "text/plain; charset=utf-8",
  json: "application/json", csv: "text/csv", svg: "image/svg+xml", png: "image/png", jpg: "image/jpeg",
  jpeg: "image/jpeg", gif: "image/gif", webp: "image/webp", pdf: "application/pdf", mp3: "audio/mpeg",
  wav: "audio/wav", mp4: "video/mp4", webm: "video/webm",
};
const mimeOf = (p) => MIME[(p.match(/\.([a-z0-9]+)$/i) || [])[1]?.toLowerCase()] || "application/octet-stream";

async function sha256(bytes) {
  return [...new Uint8Array(await crypto.subtle.digest("SHA-256", bytes))].map((b) => b.toString(16).padStart(2, "0")).join("");
}

const utf8 = new TextEncoder();
// At most `max` bytes of `text`, cut at a whole character.
function clip(text, max) {
  if (utf8.encode(text).length <= max) return text;
  let out = "";
  for (const ch of text) {
    if (utf8.encode(out + ch).length > max) break;
    out += ch;
  }
  return out;
}
// Text as the index takes it: no control characters but newlines and tabs.
const clean = (text) => String(text).replace(/\r\n?/g, "\n").replace(/[\u0000-\u0008\u000b-\u001f\u007f]/g, "");
const wikiField = (path) => clip(clean(wikiOf(path)), FIELD_MAX_BYTES);

// A page as its index's source: its sections, each with the headings above
// it as context, its wiki a field (a search narrows by it).
function source(path, revision, page) {
  const wiki = wikiField(path);
  const out = { source: path, revision, title: clip(clean(page.title), TITLE_MAX_BYTES), sections: [] };
  if (wiki) out.fields = { wiki };
  page.sections.forEach((s, i) => {
    const text = clean(linkWords(s.body));
    if (!text.trim()) return;
    const section = { id: `s${i}`, text, context: s.ancestry.slice(-CONTEXT_MAX).map((a) => clip(clean(a), CONTEXT_ITEM_MAX_BYTES)) };
    if (s.heading) section.heading = clip(clean(s.heading), HEADING_MAX_BYTES);
    out.sections.push(section);
  });
  return out;
}

// Markdown text that reads as itself: a name from the brain's files, written into a made page.
const literal = (s) => String(s).replace(/[\\`*_[\]<>#|]/g, (c) => `\\${c}`);

export class App extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    const sql = ctx.storage.sql;
    sql.exec("CREATE TABLE IF NOT EXISTS brain_state (k TEXT PRIMARY KEY, v TEXT NOT NULL)");
    if (this.#get("format") !== FORMAT) {
      // a new shape (or none yet): drop what was (an older index of the
      // app's own included), and rebuild it all from the files
      ctx.storage.transactionSync(() => {
        sql.exec("DROP TABLE IF EXISTS sections");
        sql.exec("DROP TABLE IF EXISTS pages");
        sql.exec("DROP TABLE IF EXISTS dirty");
        this.#set("format", FORMAT);
        this.#set("full", "1");
      });
    }
    sql.exec("CREATE TABLE IF NOT EXISTS changes (id INTEGER PRIMARY KEY CHECK (id = 1), at INTEGER NOT NULL, paths TEXT NOT NULL)");
    // a marked path: its newest mark's generation, and a batch's claim on
    // it (when, and which batch)
    sql.exec("CREATE TABLE IF NOT EXISTS dirty (path TEXT PRIMARY KEY, gen INTEGER NOT NULL, claimed INTEGER, by TEXT)");
    // a page as last written (its hash) or left out (`skipped`, why)
    sql.exec(`CREATE TABLE IF NOT EXISTS pages (
      path TEXT PRIMARY KEY, wiki TEXT NOT NULL, title TEXT NOT NULL, hash TEXT NOT NULL,
      bytes INTEGER NOT NULL, sections INTEGER NOT NULL, skipped TEXT, at INTEGER NOT NULL)`);
  }

  #get(k) {
    return this.ctx.storage.sql.exec("SELECT v FROM brain_state WHERE k = ?", k).toArray()[0]?.v ?? null;
  }

  #set(k, v) {
    this.ctx.storage.sql.exec("INSERT INTO brain_state (k, v) VALUES (?, ?) ON CONFLICT (k) DO UPDATE SET v = excluded.v", k, String(v));
  }

  #unset(k) {
    this.ctx.storage.sql.exec("DELETE FROM brain_state WHERE k = ?", k);
  }

  // A counter of the index's own: generations of marks, and pages written.
  #bump(k) {
    const n = Number(this.#get(k) ?? 0) + 1;
    this.#set(k, n);
    return n;
  }

  #pending() {
    return this.ctx.storage.sql.exec("SELECT COUNT(*) AS n FROM dirty").one().n + (this.#get("full") !== null ? 1 : 0);
  }

  // What is left of a wait the search service asked for, in milliseconds.
  #waiting() {
    return Math.max(0, Number(this.#get("waiting") ?? 0) - Date.now());
  }

  // Whether a batch is on the index now (a claim is fresh).
  #running() {
    return this.ctx.storage.sql.exec("SELECT COUNT(*) AS n FROM dirty WHERE claimed >= ?", Date.now() - CLAIM_MS).one().n > 0;
  }

  // `path` marked at `gen`. A batch's claim on it stays: that batch, done,
  // lets it go to be read again, so one batch at a time reads a page.
  #dirty(path, gen) {
    this.ctx.storage.sql.exec("INSERT INTO dirty (path, gen) VALUES (?, ?) ON CONFLICT (path) DO UPDATE SET gen = excluded.gen", path, gen);
  }

  // ---- the index, kept by the file trigger (or a member's start) ----

  // A run: mark what moved, then index it a batch at a time until no page
  // is left for it; past its steps, a run of its own goes on. What is left
  // that another batch holds, it waits out one claim for, then takes what a
  // batch that died left.
  async changed(input = {}, job) {
    const paths = (Array.isArray(input.paths) ? input.paths : []).filter((p) => typeof p === "string").slice(0, PATHS_MAX);
    // `full` (or more paths than a mark takes) reads every file again
    const full = input.full === true || (Number.isSafeInteger(input.more) && input.more > 0);
    const hop = Number.isSafeInteger(input.hop) && input.hop > 0 ? input.hop : 0;
    const marked = await job.call("mark", { paths, full });
    let last = { claimed: 0, pending: 0 };
    let steps = 0;
    let waited = false;
    let rated = 0;
    while (steps < STEPS_MAX) {
      last = await job.call("reindex", {});
      steps++;
      // the search service asked for a wait (its claims let go): slept out,
      // a durable step, and the run goes on. In the run, not a run of its
      // own: a rebuild may meet more rate windows than a chain has hops.
      if (last.wait > 0) {
        await job.sleep(last.wait);
        steps++;
        rated++;
        continue;
      }
      if (last.claimed > 0) continue;
      if (last.pending === 0 || waited) break;
      await job.sleep(CLAIM_MS + 5000);
      steps++;
      waited = true;
    }
    // its steps ran out with pages left: the work goes on in a run of its own
    const next = (last.claimed > 0 || last.wait > 0) && hop < HOPS_MAX;
    if (next) await job.call("changed", { hop: hop + 1 });
    return { marked: marked.marked, full: marked.full, steps, pending: last.pending, rated, next };
  }

  // The paths that moved, each at a new generation; `full`, an editor's (or
  // a trigger's), asks for every file to be read again. Open pages re-run
  // last_change after a trigger's mark, and refresh their tree.
  mark({ paths = [], full = false }, call) {
    const sql = this.ctx.storage.sql;
    const gen = this.#bump("gen");
    const mine = paths.filter(indexable);
    for (const p of mine) this.#dirty(p, gen);
    const may = editor(call.role);
    if (full && may) this.#set("full", "1");
    if (may) {
      sql.exec(
        "INSERT INTO changes (id, at, paths) VALUES (1, ?, ?) ON CONFLICT (id) DO UPDATE SET at = excluded.at, paths = excluded.paths",
        Date.now(),
        JSON.stringify(paths),
      );
    }
    return { marked: mine.length, full: full && may, gen };
  }

  // One batch: up to `BATCH` marked files no other batch holds, claimed,
  // read at main and written to the index. `full` (an editor's) reads every
  // file again. `claimed` 0: nothing was left for it. A batch that fails
  // lets go of what it holds, for the next to take at once. One the search
  // service turns away for now (429) lets go too, and answers `wait`, its
  // milliseconds: until then a batch claims nothing, and answers what is
  // left of it.
  async reindex({ full = false } = {}, call) {
    const sql = this.ctx.storage.sql;
    // an editor's full pass is kept first: a pause defers it, never drops it
    if (full === true && editor(call?.role)) this.#set("full", "1");
    const until = Number(this.#get("waiting") ?? 0);
    if (until > Date.now()) return { ...NOTHING_DONE, claimed: 0, pending: this.#pending(), wait: until - Date.now() };
    if (until) this.#unset("waiting");
    if (this.#get("full") !== null) await this.#markAll();
    const now = Date.now();
    const by = crypto.randomUUID();
    const rows = sql.exec(
      `UPDATE dirty SET claimed = ?, by = ? WHERE path IN
         (SELECT path FROM dirty WHERE claimed IS NULL OR claimed < ? ORDER BY path LIMIT ?)
       RETURNING path, gen`,
      now, by, now - CLAIM_MS, BATCH,
    ).toArray();
    const done = { ...NOTHING_DONE };
    try {
      for (const row of rows) done[await this.#index(row, by)]++;
    } catch (e) {
      sql.exec("UPDATE dirty SET claimed = NULL, by = NULL WHERE by = ?", by);
      if (e.code !== "rate_limited") throw e;
      const wait = waitFor(e.retry_after_ms);
      this.#set("waiting", String(Date.now() + wait));
      return { ...done, claimed: rows.length, pending: this.#pending(), wait };
    }
    return { ...done, claimed: rows.length, pending: this.#pending(), wait: 0 };
  }

  // A full pass: every markdown file at main, and every page that left it.
  async #markAll() {
    const listed = (await this.files.list("")).map((f) => f.path).filter(indexable);
    const sql = this.ctx.storage.sql;
    this.ctx.storage.transactionSync(() => {
      const gen = this.#bump("gen");
      const have = new Set(listed);
      const gone = sql.exec("SELECT path FROM pages").toArray().map((r) => r.path).filter((p) => !have.has(p));
      for (const p of [...listed, ...gone]) this.#dirty(p, gen);
      this.#unset("full");
    });
  }

  // `src` written at fence `gen`. "indexed", "held" (the index held that
  // revision already), "full" (refused for room), or "stale": a newer
  // batch's write or delete of the page was taken, and this one is not.
  async #write(src, gen) {
    try {
      const wrote = await this.searchIndex.upsert({ ...src, fence: gen });
      if (wrote.cleared) this.#cleared();
      return wrote.written === false ? "held" : "indexed";
    } catch (e) {
      // whatever came of the write, an index it emptied is written whole again
      if (e.cleared) this.#cleared();
      if (e.code === "storage_full") return "full";
      if (e.code === "conflict" && e.fence != null) return "stale";
      throw e;
    }
  }

  // `path` out of the index at fence `gen` (its tombstone stays): false if
  // a newer batch's write or delete of it was taken.
  async #remove(path, gen) {
    try {
      await this.searchIndex.delete({ source: path, fence: gen });
      return true;
    } catch (e) {
      if (e.code === "conflict" && e.fence != null) return false;
      throw e;
    }
  }

  // The index was emptied (its embedding model changed): every page is
  // written again.
  #cleared() {
    this.#set("full", "1");
  }

  // One page, as main holds it now, by the batch `by` that claimed it at
  // `gen`: written to the index (which writes nothing for a revision it
  // holds), or taken out when it is gone, each at fence `gen`. Answers what
  // happened to it.
  // Marked again while it was read, it stays marked, its claim let go, and
  // a batch reads it again; taken by another batch (this one's claim
  // lapsed), that batch decides, and the fences keep this one's effects
  // from landing over its.
  async #index({ path, gen }, by) {
    const sql = this.ctx.storage.sql;
    const prior = sql.exec("SELECT hash FROM pages WHERE path = ?", path).toArray()[0] ?? null;
    const listed = indexable(path) ? ((await this.files.list(path)).find((f) => f.path === path) ?? null) : null;
    const large = listed !== null && (listed.blob === true || listed.size > PAGE_MAX_BYTES);
    const bytes = listed && !large ? await this.files.readBytes(path) : null;
    // what the page is now: its bytes' hash, or its size when it is too large to read
    const hash = listed === null ? null : large ? `large:${listed.size}` : await sha256(bytes);
    let outcome;
    let page = { title: stem(path), sections: [], truncated: false };
    let why = null;
    if (listed === null) {
      // gone, or no longer searched: out of the index, its tombstone kept
      // (the platform forgets a file main deletes too)
      outcome = !(await this.#remove(path, gen)) ? "stale" : prior === null ? "unchanged" : "removed";
    } else if (large || utf8.encode(path).length > SOURCE_MAX_BYTES) {
      outcome = (await this.#remove(path, gen)) ? "skipped" : "stale";
      why = large ? "1 MiB or more: a blob, not read" : `its path is over ${SOURCE_MAX_BYTES} bytes`;
    } else {
      page = sections(path, new TextDecoder().decode(bytes));
      outcome = await this.#write(source(path, `${FORMAT}:${hash}`, page), gen);
      if (outcome === "indexed" || outcome === "held") {
        if (page.truncated) why = `only its first ${SECTIONS_MAX} sections`;
      } else if (outcome === "full") {
        outcome = "skipped";
        why = "the index is full: split this brain into child brains";
      }
    }
    // a revision the index held already is no write
    const told = outcome === "held" ? "unchanged" : outcome;
    return this.ctx.storage.transactionSync(() => {
      const row = sql.exec("SELECT gen, by FROM dirty WHERE path = ?", path).toArray()[0];
      if (row?.by !== by) return told;
      if (row.gen === gen) sql.exec("DELETE FROM dirty WHERE path = ?", path);
      else sql.exec("UPDATE dirty SET claimed = NULL, by = NULL WHERE path = ?", path);
      if (outcome === "unchanged" || outcome === "stale") return told;
      if (outcome === "removed") {
        sql.exec("DELETE FROM pages WHERE path = ?", path);
        this.#bump("writes");
        return told;
      }
      const written = outcome === "indexed" || outcome === "held";
      sql.exec(
        `INSERT INTO pages (path, wiki, title, hash, bytes, sections, skipped, at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (path) DO UPDATE SET wiki = excluded.wiki, title = excluded.title, hash = excluded.hash,
           bytes = excluded.bytes, sections = excluded.sections, skipped = excluded.skipped, at = excluded.at`,
        path, wikiOf(path), page.title, hash, listed.size,
        written ? page.sections.length : 0, why, Date.now(),
      );
      if (outcome !== "held") this.#bump("writes");
      return told;
    });
  }

  // ---- what agents and pages ask ----

  // Ranked sections, by words and by meaning: `{q, results: [{rank, path,
  // wiki, title, heading, ancestry, snippet, signals}], semantic, pending,
  // indexing, waiting_ms, hint?}`. While the index is behind (`indexing`),
  // its answer may be incomplete, and `hint` says how to finish it, or that
  // the search service asked for a pause (`waiting_ms` left of it).
  async search({ q, limit = RESULTS_DEFAULT, wiki = null }) {
    const question = { q, limit: Math.min(Math.max(1, Math.trunc(limit)), RESULTS_MAX) };
    if (wiki !== null) question.fields = { wiki: clip(clean(wiki), FIELD_MAX_BYTES) };
    const found = await this.searchIndex.query(question);
    const results = found.results.map((h, i) => ({
      rank: i + 1,
      path: h.source,
      wiki: wikiOf(h.source),
      title: h.title,
      heading: h.heading ?? null,
      ancestry: h.context ?? [],
      snippet: h.snippet,
      signals: h.signals,
    }));
    const pending = this.#pending();
    const waiting_ms = pending > 0 ? this.#waiting() : 0;
    const answer = { q, results, semantic: found.semantic, pending, indexing: pending > 0, waiting_ms };
    if (waiting_ms > 0) answer.hint = "Indexing: waiting for the search service, which asked for a pause. Its changed job goes on by itself.";
    else if (pending > 0) answer.hint = "Indexing: results may be incomplete. Its changed job finishes it: fragment call <this brain> changed";
    return answer;
  }

  // What the index holds: the brain's pages and their sections, what is
  // pending, whether a run is on it and what is left of a pause the search
  // service asked for, how many pages it has written, the pages it skipped,
  // and the fragment's index's own counts and limits.
  async index_status() {
    const sql = this.ctx.storage.sql;
    const one = (q) => sql.exec(q).one().n;
    const skipped = sql.exec("SELECT path, skipped FROM pages WHERE skipped IS NOT NULL ORDER BY path LIMIT ?", SKIPPED_LISTED).toArray();
    const pending = this.#pending();
    const index = await this.searchIndex.status();
    return {
      format: FORMAT,
      pages: one("SELECT COUNT(*) AS n FROM pages"),
      sections: one("SELECT COALESCE(SUM(sections), 0) AS n FROM pages"),
      pending,
      indexing: pending > 0,
      running: this.#running(),
      waiting_ms: pending > 0 ? this.#waiting() : 0,
      writes: Number(this.#get("writes") ?? 0),
      skipped: skipped.map((r) => ({ path: r.path, why: r.skipped })),
      index: { chunks: index.chunks, chunks_split: index.chunks_split, chunks_max: index.chunks_max, semantic: index.semantic },
    };
  }

  // The brain's AGENTS.md, for an agent with the CLI.
  guide() {
    return { text: GUIDE };
  }

  last_change() {
    const row = this.ctx.storage.sql.exec("SELECT at, paths FROM changes WHERE id = 1").toArray()[0];
    return row ? { at: row.at, paths: JSON.parse(row.paths) } : null;
  }

  // ---- the viewer's routes ----

  // The brain's title, from its face.
  async #title() {
    try {
      const m = JSON.parse((await this.files.read("fragment.json")) ?? "{}");
      return typeof m.meta?.title === "string" && m.meta.title ? m.meta.title : "Brain";
    } catch {
      return "Brain";
    }
  }

  // _index.md, made: the brain's wikis, each with its pages, and the guide.
  async #landing(files) {
    const wikis = new Map();
    for (const f of files) {
      const w = wikiOf(f.path);
      if (!w) continue;
      const e = wikis.get(w) ?? { pages: 0, sources: 0, index: false };
      if (/\.(md|markdown)$/i.test(f.path)) e.pages++;
      if (f.path.startsWith(`${w}/raw/`)) e.sources++;
      if (f.path === `${w}/index.md`) e.index = true;
      wikis.set(w, e);
    }
    const lines = [`# ${literal(await this.#title())}`, "", "A brain: each folder is a wiki, kept as [[AGENTS.md]] says. Search it with the box above.", ""];
    if (!wikis.size) lines.push("No wikis yet. An agent starts one by writing its index.md and log.md (see [[AGENTS.md]]).");
    else lines.push("## Wikis", "");
    for (const [w, e] of [...wikis].sort(([a], [b]) => a.localeCompare(b))) {
      const name = e.index ? `[[${w}/index.md|${literal(w)}]]` : literal(w);
      lines.push(`- ${name}: ${e.pages} page${e.pages === 1 ? "" : "s"}, ${e.sources} in raw/`);
    }
    return lines.join("\n") + "\n";
  }

  // api/tree: the brain's files at main, and the two it makes (its
  // _index.md, and the guide as AGENTS.md, each unless the brain holds one
  // of its own); api/file?path=: one of them (a blob's bytes come from the
  // platform's __file, beside this route).
  async fetch(request) {
    const url = new URL(request.url);
    const text = (body) => new Response(body, { headers: { "content-type": "text/plain; charset=utf-8", "cache-control": "no-store" } });
    if (url.pathname.endsWith("/api/tree")) {
      const files = (await this.files.list("")).filter((f) => !hidden(f.path));
      const made = [];
      if (!files.some((f) => f.path === "_index.md")) made.push({ path: "_index.md", size: 0, made: true });
      if (!files.some((f) => f.path === "AGENTS.md")) made.push({ path: "AGENTS.md", size: GUIDE.length, made: true });
      return Response.json({ files: [...made, ...files] }, { headers: { "cache-control": "no-store" } });
    }
    if (!url.pathname.endsWith("/api/file")) return new Response(`no route ${url.pathname}`, { status: 404 });
    const path = url.searchParams.get("path") || "";
    if (hidden(path)) return new Response("not a file of the brain", { status: 404 });
    const stat = (await this.files.list(path)).find((f) => f.path === path);
    if (!stat && path === "AGENTS.md") return text(GUIDE);
    if (!stat && path === "_index.md") return text(await this.#landing((await this.files.list("")).filter((f) => !hidden(f.path))));
    if (!stat) return new Response("no such file", { status: 404 });
    if (stat.blob) {
      const to = new URL("../__file", url);
      to.searchParams.set("path", path);
      const view = url.searchParams.get("view");
      if (view) to.searchParams.set("view", view);
      return Response.redirect(to.href, 302);
    }
    return new Response(await this.files.readBytes(path), {
      headers: { "content-type": mimeOf(path), "cache-control": "no-store", "x-content-type-options": "nosniff" },
    });
  }
}
