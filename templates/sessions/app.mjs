// The session archive's list (templates/sessions/README.md, "The list"): a
// row a conversation, its index entry and the commit of its file the entry
// describes. The conversations are files (`sessions/<key>.json`): a kept
// archive, read whole by the archive's page and by agents with file tools.
// The list is state, so it is rows (docs/MODEL.md: git only where git is
// fundamental to a fragment's state). It runs from the platform's release
// (decision 40), as every blessed template's code does.
//
// The import writes the rows (`filed`), as the writer holding each
// conversation's bytes: an app reads no file of 1 MiB or more (a blob), and a
// conversation may be 8 MiB, so a files trigger could not describe them all.
// A row names the commit its file was last changed in as described; the next
// import reads again any file whose last commit is not its row's, so a row
// is never trusted for bytes it did not describe (cli/src/session/mod.rs).
// The list is `whole` once an import has filed a row for every conversation
// the archive held: until then (an archive from before its rows, its first
// fill cut off) a reader lists its index files instead.
import { DurableObject } from "cloudflare:workers";

// a conversation's key and a kind's name, as fragment_core::session makes them
const KEY = /^[a-z][a-z0-9]{0,31}-[A-Za-z0-9._-]{1,140}$/;
const KIND = /^[a-z][a-z0-9_-]{0,31}$/;
const COMMIT = /^[0-9a-f]{40}([0-9a-f]{24})?$/;
const SHA256 = /^[0-9a-f]{64}$/;
// an entry's bytes, at most (each field is bounded by the import's own limits)
const ENTRY_MAX_BYTES = 8 * 1024;
// rows one answer lists, and their bytes, at most (an answer is at most 1 MiB)
const PAGE_MAX = 1000;
const PAGE_MAX_BYTES = 768 * 1024;

const utf8 = new TextEncoder();

export class App extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    const sql = ctx.storage.sql;
    sql.exec("CREATE TABLE IF NOT EXISTS sessions (key TEXT PRIMARY KEY, kind TEXT NOT NULL, entry TEXT NOT NULL, commit_sha TEXT)");
    sql.exec("CREATE INDEX IF NOT EXISTS sessions_by_kind ON sessions (kind, key)");
    sql.exec("CREATE TABLE IF NOT EXISTS list_state (k TEXT PRIMARY KEY, v TEXT NOT NULL)");
  }

  #whole() {
    return this.ctx.storage.sql.exec("SELECT v FROM list_state WHERE k = 'whole'").toArray().length > 0;
  }

  // Its rows after `after`, by key, of one kind or of all: `{sessions:
  // [entry, with its `commit` when one is named], next, whole}`, `next` the
  // key to ask after for more, null at the end.
  sessions({ kind = null, after = "" } = {}) {
    const sql = this.ctx.storage.sql;
    const rows =
      kind === null
        ? sql.exec("SELECT key, entry, commit_sha FROM sessions WHERE key > ? ORDER BY key LIMIT ?", after, PAGE_MAX + 1)
        : sql.exec("SELECT key, entry, commit_sha FROM sessions WHERE kind = ? AND key > ? ORDER BY key LIMIT ?", kind, after, PAGE_MAX + 1);
    const sessions = [];
    let bytes = 0;
    for (const r of rows) {
      const size = utf8.encode(r.entry).length + 64;
      if (sessions.length === PAGE_MAX || bytes + size > PAGE_MAX_BYTES) return { sessions, next: sessions.at(-1).key, whole: this.#whole() };
      const entry = JSON.parse(r.entry);
      if (r.commit_sha) entry.commit = r.commit_sha;
      sessions.push(entry);
      bytes += size;
    }
    return { sessions, next: null, whole: this.#whole() };
  }

  // The import's rows (an entry, and the commit its file was last changed
  // in as described, or null when it cannot say), the keys whose files are
  // gone, and `whole`: true once every conversation the archive holds has
  // its row, false to say it may not (its readers list its index files).
  // One that is not an entry refuses the whole call.
  filed({ rows = [], gone = [], whole = null }) {
    const sql = this.ctx.storage.sql;
    const count = (v) => Number.isSafeInteger(v) && v >= 0;
    const text = (v) => typeof v === "string";
    for (const { commit = null, ...entry } of rows) {
      const json = JSON.stringify(entry);
      if (!text(entry.key) || !KEY.test(entry.key)) throw new Error(`no conversation's key: ${String(entry.key).slice(0, 160)}`);
      if (!text(entry.kind) || !KIND.test(entry.kind)) throw new Error(`${entry.key}: no kind's name`);
      if (!text(entry.id) || !text(entry.origin) || !text(entry.title) || !text(entry.sha256) || !SHA256.test(entry.sha256)) throw new Error(`${entry.key}: no entry (id, origin, title, sha256)`);
      if (![entry.reader ?? 0, entry.messages, entry.bytes, entry.media ?? 0].every(count)) throw new Error(`${entry.key}: no entry (reader, messages, bytes, media)`);
      if (![entry.agent, entry.source, entry.with, entry.started_at, entry.ended_at, entry.preview].every((v) => v === undefined || text(v))) throw new Error(`${entry.key}: no entry (its text fields)`);
      if (commit !== null && (!text(commit) || !COMMIT.test(commit))) throw new Error(`${entry.key}: no commit`);
      if (utf8.encode(json).length > ENTRY_MAX_BYTES) throw new Error(`${entry.key}: an entry is at most ${ENTRY_MAX_BYTES} bytes`);
      sql.exec(
        `INSERT INTO sessions (key, kind, entry, commit_sha) VALUES (?, ?, ?, ?)
         ON CONFLICT (key) DO UPDATE SET kind = excluded.kind, entry = excluded.entry, commit_sha = excluded.commit_sha`,
        entry.key, entry.kind, json, commit,
      );
    }
    for (const key of gone) sql.exec("DELETE FROM sessions WHERE key = ?", key);
    if (whole === true) sql.exec("INSERT OR REPLACE INTO list_state (k, v) VALUES ('whole', '1')");
    if (whole === false) sql.exec("DELETE FROM list_state WHERE k = 'whole'");
    return { filed: rows.length, gone: gone.length, whole: this.#whole() };
  }
}
