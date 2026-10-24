-- The principal schema as the code at 7b84dcd3 ran it at every start, before
-- schema versions: what an object that code made holds. Copied from git
-- (crates/core/src/migrate.rs, `deployed`), never edited. Its `new()` ran:
--   cell/src/principal.rs SCHEMA
--   cell/src/principal.rs WIPED_SCHEMA
CREATE TABLE IF NOT EXISTS memberships (
  fragment TEXT PRIMARY KEY, role TEXT, incarnation INTEGER NOT NULL, version INTEGER NOT NULL,
  sharing TEXT, face TEXT, archived INTEGER NOT NULL DEFAULT 0, searched TEXT);
CREATE TABLE IF NOT EXISTS search_entries (
  id INTEGER PRIMARY KEY, fragment TEXT NOT NULL, n INTEGER NOT NULL, channel TEXT NOT NULL, seq INTEGER NOT NULL,
  at INTEGER NOT NULL, text TEXT NOT NULL, UNIQUE (fragment, n));
CREATE INDEX IF NOT EXISTS search_entries_at ON search_entries (at, id);
CREATE VIRTUAL TABLE IF NOT EXISTS search_text USING fts5(
  text, content = 'search_entries', content_rowid = 'id', tokenize = 'unicode61 remove_diacritics 2');
CREATE TRIGGER IF NOT EXISTS search_entries_added AFTER INSERT ON search_entries BEGIN
  INSERT INTO search_text (rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER IF NOT EXISTS search_entries_dropped AFTER DELETE ON search_entries BEGIN
  INSERT INTO search_text (search_text, rowid, text) VALUES ('delete', old.id, old.text);
END;
CREATE TABLE IF NOT EXISTS seen (fragment TEXT PRIMARY KEY, n INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS exports (
  id TEXT PRIMARY KEY, person TEXT NOT NULL, state TEXT NOT NULL, started_at INTEGER NOT NULL,
  ready_at INTEGER, expires_at INTEGER, size INTEGER, error TEXT, job TEXT, upload TEXT,
  requests INTEGER NOT NULL DEFAULT 0, downloads INTEGER NOT NULL DEFAULT 0, gone_at INTEGER, metered INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS export_tail (export TEXT NOT NULL, n INTEGER NOT NULL, bytes BLOB NOT NULL, PRIMARY KEY (export, n));
CREATE TABLE IF NOT EXISTS export_parts (export TEXT NOT NULL, part INTEGER NOT NULL, etag TEXT NOT NULL, PRIMARY KEY (export, part));
CREATE TABLE IF NOT EXISTS export_blobs (export TEXT NOT NULL, fragment TEXT NOT NULL, sha TEXT NOT NULL, PRIMARY KEY (export, fragment, sha));
CREATE TABLE IF NOT EXISTS export_levers (name TEXT PRIMARY KEY, value INTEGER);
CREATE TABLE IF NOT EXISTS wiped (at INTEGER NOT NULL);
