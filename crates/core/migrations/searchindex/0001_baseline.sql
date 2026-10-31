-- SearchIndex, step 1: one fragment life's search index (cell/src/
-- search_index.rs; docs/api.md, "A fragment's search"), written only by
-- its fragment's supervisor.
--
-- `meta`: the space its vectors are in (`space`: JSON, '' while it holds
-- none), its chunks' count, when a source last left it (the alarm folds
-- the words index after), and `ended` once its life ended.
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
-- A source's own row: its revision, whatever its sections (none, too).
CREATE TABLE IF NOT EXISTS sources (
  source TEXT PRIMARY KEY, revision TEXT NOT NULL, title TEXT NOT NULL, at TEXT, fields TEXT NOT NULL, chunks INTEGER NOT NULL);
-- `at`: the section's, else its source's; `context` a JSON array.
CREATE TABLE IF NOT EXISTS sections (
  id INTEGER PRIMARY KEY, source TEXT NOT NULL, section TEXT NOT NULL, heading TEXT, context TEXT NOT NULL, at TEXT, location TEXT);
CREATE INDEX IF NOT EXISTS sections_source ON sections (source);
CREATE TABLE IF NOT EXISTS chunks (id INTEGER PRIMARY KEY, section INTEGER NOT NULL, hash TEXT NOT NULL, body TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS chunks_section ON chunks (section);
CREATE INDEX IF NOT EXISTS chunks_hash ON chunks (hash);
-- Each chunk's words by field, its rowid the chunk's: contentless, so no
-- text is held twice, and deletable by rowid.
CREATE VIRTUAL TABLE IF NOT EXISTS chunk_words USING fts5(
  title, heading, context, body, content = '', contentless_delete = 1, tokenize = 'unicode61 remove_diacritics 2');
-- A chunk's vector (fragment_proto::search::SCHEME): its sign bits and
-- its int8 components.
CREATE TABLE IF NOT EXISTS vectors (chunk INTEGER PRIMARY KEY, code BLOB NOT NULL, q BLOB NOT NULL);
