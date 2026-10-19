-- The ledger schema as the code at e62d9991 ran it at every start, before
-- schema versions: what an object that code made holds. Copied from git
-- (crates/core/src/migrate.rs, `deployed`), never edited. Its `new()` ran:
--   cell/src/ledger.rs SCHEMA
--   cell/src/ledger.rs WIPED_SCHEMA
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS head (id INTEGER PRIMARY KEY CHECK (id = 1), ledger TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS entries (ref TEXT PRIMARY KEY, at_ms INTEGER NOT NULL, held INTEGER NOT NULL, entry TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS entries_at ON entries (at_ms);
CREATE TABLE IF NOT EXISTS batches (id TEXT PRIMARY KEY, digest TEXT NOT NULL, answer TEXT NOT NULL, at_ms INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS batches_at ON batches (at_ms);
CREATE TABLE IF NOT EXISTS commands (id TEXT PRIMARY KEY, command TEXT NOT NULL, at_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS spend (month INTEGER NOT NULL, fragment TEXT NOT NULL, micros INTEGER NOT NULL, PRIMARY KEY (month, fragment));
CREATE TABLE IF NOT EXISTS caps (fragment TEXT PRIMARY KEY, micros INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS wiped (at INTEGER NOT NULL);
