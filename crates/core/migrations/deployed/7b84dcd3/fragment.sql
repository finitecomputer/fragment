-- The fragment schema as the code at 7b84dcd3 ran it at every start, before
-- schema versions: what an object that code made holds. Copied from git
-- (crates/core/src/migrate.rs, `deployed`), never edited. Its `new()` ran:
--   cell/src/fragment.rs SCHEMA
--   cell/src/ended.rs SCHEMA
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS members (
  principal TEXT PRIMARY KEY, role TEXT NOT NULL, added_by TEXT NOT NULL, added_at INTEGER NOT NULL,
  kind TEXT, owner TEXT);
CREATE INDEX IF NOT EXISTS members_owner ON members (owner) WHERE owner IS NOT NULL;
CREATE TABLE IF NOT EXISTS invites (
  email TEXT PRIMARY KEY, role TEXT NOT NULL, expires_at INTEGER NOT NULL, created_by TEXT NOT NULL, created_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS index_outbox (
  principal TEXT PRIMARY KEY, role TEXT, version INTEGER NOT NULL, attempts INTEGER NOT NULL, next_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS search_log (
  n INTEGER PRIMARY KEY AUTOINCREMENT, channel TEXT NOT NULL, seq INTEGER NOT NULL, at INTEGER NOT NULL, text TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS search_outbox (
  principal TEXT PRIMARY KEY, sent INTEGER NOT NULL, attempts INTEGER NOT NULL, next_at INTEGER);
CREATE INDEX IF NOT EXISTS search_outbox_due ON search_outbox (next_at);
CREATE TABLE IF NOT EXISTS joined_outbox (
  principal TEXT PRIMARY KEY, owner TEXT NOT NULL, added_at INTEGER NOT NULL, attempts INTEGER NOT NULL, next_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS secrets (
  name TEXT PRIMARY KEY, sealed TEXT NOT NULL, set_by TEXT NOT NULL, set_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS secret_hosts (
  name TEXT NOT NULL, host TEXT NOT NULL, PRIMARY KEY (name, host));
CREATE TABLE IF NOT EXISTS records (
  channel TEXT NOT NULL, seq INTEGER NOT NULL, at INTEGER NOT NULL, principal TEXT NOT NULL, kind TEXT NOT NULL,
  body TEXT NOT NULL, op TEXT, idx INTEGER, outboxed INTEGER NOT NULL DEFAULT 1, PRIMARY KEY (channel, seq));
CREATE UNIQUE INDEX IF NOT EXISTS records_effect ON records (op, idx) WHERE op IS NOT NULL;
CREATE TABLE IF NOT EXISTS tree (
  ref TEXT NOT NULL, path TEXT NOT NULL, size INTEGER NOT NULL, mode TEXT NOT NULL, last_commit TEXT NOT NULL,
  PRIMARY KEY (ref, path));
CREATE TABLE IF NOT EXISTS code (
  id INTEGER PRIMARY KEY CHECK (id = 1), sha TEXT NOT NULL, loader_id TEXT NOT NULL, source TEXT NOT NULL,
  cpu_ms INTEGER NOT NULL, installed_at INTEGER NOT NULL,
  modules TEXT NOT NULL DEFAULT '{}');
CREATE TABLE IF NOT EXISTS code_ops (op TEXT PRIMARY KEY, kind TEXT NOT NULL, role TEXT NOT NULL, input TEXT, ephemeral INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS code_op_descriptions (op TEXT PRIMARY KEY, description TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS code_channels (channel TEXT PRIMARY KEY, read TEXT NOT NULL, post TEXT, signed_in INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS code_channel_from (channel TEXT PRIMARY KEY, from_kind TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS code_triggers (idx INTEGER PRIMARY KEY, kind TEXT NOT NULL, target TEXT NOT NULL, run TEXT NOT NULL, from_kind TEXT);
CREATE INDEX IF NOT EXISTS code_triggers_on ON code_triggers (kind, target);
CREATE TABLE IF NOT EXISTS runs (
  id INTEGER PRIMARY KEY AUTOINCREMENT, op TEXT NOT NULL, via TEXT NOT NULL, trigger TEXT, principal TEXT NOT NULL,
  role TEXT NOT NULL, depth INTEGER NOT NULL, call_id TEXT, input_sha TEXT, input TEXT NOT NULL, status TEXT NOT NULL,
  attempt INTEGER NOT NULL, output TEXT, error TEXT, created_at INTEGER NOT NULL, launched_at INTEGER, finished_at INTEGER,
  code TEXT);
CREATE UNIQUE INDEX IF NOT EXISTS runs_call ON runs (principal, call_id) WHERE call_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS runs_status ON runs (status, op);
CREATE TABLE IF NOT EXISTS steps (
  run INTEGER NOT NULL, attempt INTEGER NOT NULL, idx INTEGER NOT NULL, kind TEXT NOT NULL, value TEXT, error TEXT,
  CHECK ((value IS NULL) <> (error IS NULL)), PRIMARY KEY (run, attempt, idx));
CREATE TABLE IF NOT EXISTS schedules (idx INTEGER PRIMARY KEY, op TEXT NOT NULL, cron TEXT NOT NULL, next_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS paused_ops (op TEXT PRIMARY KEY, by TEXT NOT NULL, at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS op_breakers (op TEXT PRIMARY KEY, reset_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS file_commits (key TEXT PRIMARY KEY, sha TEXT NOT NULL, at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS own_commits (sha TEXT PRIMARY KEY, depth INTEGER NOT NULL, at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS blobs (
  sha TEXT PRIMARY KEY, size INTEGER NOT NULL, uploaded_at INTEGER NOT NULL, seen_at INTEGER NOT NULL, mime TEXT);
CREATE TABLE IF NOT EXISTS pointers (
  ref TEXT NOT NULL, path TEXT NOT NULL, sha TEXT NOT NULL, size INTEGER NOT NULL, PRIMARY KEY (ref, path));
CREATE TABLE IF NOT EXISTS subs (
  id INTEGER PRIMARY KEY AUTOINCREMENT, principal TEXT NOT NULL, channel TEXT NOT NULL, url TEXT NOT NULL,
  created_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS push_subs (
  id INTEGER PRIMARY KEY AUTOINCREMENT, endpoint TEXT NOT NULL UNIQUE, p256dh TEXT NOT NULL, auth TEXT NOT NULL,
  who TEXT NOT NULL, principal TEXT NOT NULL, created_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS sent (key TEXT PRIMARY KEY, at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS pending (
  seq INTEGER PRIMARY KEY AUTOINCREMENT, ledger_id TEXT NOT NULL UNIQUE, op TEXT NOT NULL, principal TEXT NOT NULL,
  depth INTEGER NOT NULL, tries INTEGER NOT NULL, next_at INTEGER NOT NULL, at INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS pending_due ON pending (next_at);
CREATE TABLE IF NOT EXISTS delivery_outbox (
  id INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL CHECK (kind IN ('record', 'push')),
  sub INTEGER, channel TEXT, seq INTEGER, who TEXT, body TEXT,
  after_sub INTEGER NOT NULL DEFAULT 0, upto_sub INTEGER NOT NULL DEFAULT 0,
  attempts INTEGER NOT NULL DEFAULT 0, next_at INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS delivery_outbox_due ON delivery_outbox (next_at);
CREATE INDEX IF NOT EXISTS delivery_outbox_waiting ON delivery_outbox (id) WHERE attempts > 0;
CREATE TABLE IF NOT EXISTS charges (
  ref TEXT PRIMARY KEY, run INTEGER NOT NULL, micros INTEGER NOT NULL, held INTEGER NOT NULL, at INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS charges_run ON charges (run);
CREATE TABLE IF NOT EXISTS paid (
  key TEXT PRIMARY KEY, run INTEGER NOT NULL, result TEXT NOT NULL, usage TEXT, settled INTEGER NOT NULL, at INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS paid_run ON paid (run);
CREATE TABLE IF NOT EXISTS meter_requests (minute INTEGER PRIMARY KEY, count INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS meter_rows (ref TEXT PRIMARY KEY, row TEXT NOT NULL, batch INTEGER);
CREATE INDEX IF NOT EXISTS meter_rows_batch ON meter_rows (batch);
CREATE TABLE IF NOT EXISTS meter_batches (id INTEGER PRIMARY KEY AUTOINCREMENT, sent_at INTEGER);
CREATE TABLE IF NOT EXISTS ended (
  incarnation INTEGER PRIMARY KEY, name TEXT NOT NULL, npub TEXT NOT NULL, facet TEXT NOT NULL, ended_at INTEGER NOT NULL,
  stored INTEGER NOT NULL DEFAULT 1, attempts INTEGER NOT NULL DEFAULT 0, next_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS ended_index (
  incarnation INTEGER NOT NULL, principal TEXT NOT NULL, version INTEGER NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0, next_at INTEGER NOT NULL, PRIMARY KEY (incarnation, principal));
CREATE INDEX IF NOT EXISTS ended_index_due ON ended_index (next_at);
CREATE TABLE IF NOT EXISTS ended_repos (
  incarnation INTEGER PRIMARY KEY, repo TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0, next_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS ended_agents (
  incarnation INTEGER PRIMARY KEY, identity TEXT NOT NULL, computer TEXT, attempts INTEGER NOT NULL DEFAULT 0, next_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS ended_errors (
  incarnation INTEGER NOT NULL, part TEXT NOT NULL, error TEXT NOT NULL, at INTEGER NOT NULL, PRIMARY KEY (incarnation, part));
