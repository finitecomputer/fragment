-- The finite.computer move (cell/src/migrations.rs; docs/migrations.md):
-- one object, `migrations`, removable whole with the move.
-- Each old agent's move (fragment_core::migrations::Move), a column a
-- field (`COLUMNS`), its deadline derived from the rest on every write for
-- the supervisor to find it by; `offer` is the roster's last word on it
-- (RosterAgent, as JSON).
CREATE TABLE IF NOT EXISTS moves (
  old TEXT PRIMARY KEY, workos_id TEXT NOT NULL, state TEXT NOT NULL, reason TEXT, held_from TEXT, phase TEXT NOT NULL,
  attempt INTEGER NOT NULL, agent TEXT, command TEXT, seq INTEGER NOT NULL, since_ms INTEGER NOT NULL, progress_ms INTEGER,
  staged_ms INTEGER, routine_ms INTEGER, routines INTEGER NOT NULL, opened INTEGER NOT NULL, expires_ms INTEGER,
  by_staff INTEGER NOT NULL, deadline_ms INTEGER, offer TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS moves_deadline ON moves (deadline_ms);
CREATE INDEX IF NOT EXISTS moves_command ON moves (command, since_ms);
CREATE INDEX IF NOT EXISTS moves_state ON moves (state);
CREATE INDEX IF NOT EXISTS moves_workos ON moves (workos_id);
-- The roster's people: their email on the old platform, and who they are
-- here once known.
CREATE TABLE IF NOT EXISTS people (workos_id TEXT PRIMARY KEY, email TEXT NOT NULL, person TEXT);
-- Stages to delete (a prefix of the STAGE bucket), and routines to append
-- to a new agent's fragment: each queued in its change's transaction and
-- sent after it, from the alarm.
CREATE TABLE IF NOT EXISTS sweeps (prefix TEXT PRIMARY KEY, at_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS routines (stage TEXT PRIMARY KEY, agent TEXT NOT NULL, tries INTEGER NOT NULL, next_ms INTEGER NOT NULL);
-- The mover's uploads: each stage's multipart upload in STAGE (its id),
-- and each part taken (its size, hash and etag, never its bytes), until its
-- manifest is written (its SHA-256, and the zip's size).
CREATE TABLE IF NOT EXISTS uploads (stage TEXT PRIMARY KEY, upload_id TEXT NOT NULL, created_ms INTEGER NOT NULL, manifest_sha256 TEXT, zip_bytes INTEGER);
CREATE TABLE IF NOT EXISTS parts (stage TEXT NOT NULL, n INTEGER NOT NULL, bytes INTEGER NOT NULL, sha256 TEXT NOT NULL, etag TEXT NOT NULL, PRIMARY KEY (stage, n));
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
