-- The principal schema as the code at 76adcb35 ran it at every start, before
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
CREATE TABLE IF NOT EXISTS wiped (at INTEGER NOT NULL);
