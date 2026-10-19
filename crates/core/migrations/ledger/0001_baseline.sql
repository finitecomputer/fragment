-- Ledger, step 1: the schema as it stood when its steps began
-- (2026-10-19). Frozen once merged, as every step is: a change to it is
-- a new step (crates/core/src/migrate.rs says how).

-- the ledger's store (ledger.rs): a wipe drops them and makes them again empty
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS head (id INTEGER PRIMARY KEY CHECK (id = 1), ledger TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS entries (ref TEXT PRIMARY KEY, at_ms INTEGER NOT NULL, held INTEGER NOT NULL, entry TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS entries_at ON entries (at_ms);
CREATE TABLE IF NOT EXISTS batches (id TEXT PRIMARY KEY, digest TEXT NOT NULL, answer TEXT NOT NULL, at_ms INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS batches_at ON batches (at_ms);
CREATE TABLE IF NOT EXISTS commands (id TEXT PRIMARY KEY, command TEXT NOT NULL, at_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS spend (month INTEGER NOT NULL, fragment TEXT NOT NULL, micros INTEGER NOT NULL, PRIMARY KEY (month, fragment));
CREATE TABLE IF NOT EXISTS caps (fragment TEXT PRIMARY KEY, micros INTEGER NOT NULL);

-- the one row a wiped ledger keeps
CREATE TABLE IF NOT EXISTS wiped (at INTEGER NOT NULL);
