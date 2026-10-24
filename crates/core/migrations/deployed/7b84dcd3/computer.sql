-- The computer schema as the code at 7b84dcd3 ran it at every start, before
-- schema versions: what an object that code made holds. Copied from git
-- (crates/core/src/migrate.rs, `deployed`), never edited. Its `new()` ran:
--   cell/src/computer.rs SCHEMA
--   cell/src/computer.rs WIPED_SCHEMA
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS agents (fragment TEXT PRIMARY KEY, identity TEXT NOT NULL, owner TEXT NOT NULL, added_at INTEGER NOT NULL, connections TEXT NOT NULL DEFAULT 'null');
CREATE TABLE IF NOT EXISTS awake (from_ms INTEGER PRIMARY KEY, to_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS tickets (hash TEXT PRIMARY KEY, port INTEGER NOT NULL, identity TEXT NOT NULL, expires_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS sessions (hash TEXT PRIMARY KEY, identity TEXT NOT NULL, expires_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS uses (month INTEGER NOT NULL, provider TEXT NOT NULL, agent TEXT NOT NULL, calls INTEGER NOT NULL, micros INTEGER NOT NULL, PRIMARY KEY (month, provider, agent));
CREATE TABLE IF NOT EXISTS own_keys (provider TEXT PRIMARY KEY, sealed TEXT NOT NULL, set_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS own_signins (nonce TEXT PRIMARY KEY, provider TEXT NOT NULL, sealed TEXT NOT NULL, expires_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS wiped (at INTEGER NOT NULL);
