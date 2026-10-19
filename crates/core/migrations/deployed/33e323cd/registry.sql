-- The registry schema as the code at 33e323cd ran it at every start, before
-- schema versions: what an object that code made holds. Copied from git
-- (crates/core/src/migrate.rs, `deployed`), never edited. Its `new()` ran:
--   cell/src/registry.rs SCHEMA
--   cell/src/registry/signin.rs SCHEMA
--   cell/src/registry/wipe.rs SCHEMA
--   cell/src/registry/invites.rs SCHEMA
--   cell/src/registry/orgs.rs SCHEMA
--   cell/src/registry/billing.rs SCHEMA
--   cell/src/registry/seats.rs SCHEMA
--   cell/src/registry/trials.rs SCHEMA
--   cell/src/registry/admin.rs SCHEMA
CREATE TABLE IF NOT EXISTS identities (
  id TEXT PRIMARY KEY, kind TEXT NOT NULL, owner TEXT, created_at INTEGER NOT NULL, held TEXT);
CREATE INDEX IF NOT EXISTS identities_owner ON identities (owner) WHERE owner IS NOT NULL;
CREATE TABLE IF NOT EXISTS keys (
  key TEXT PRIMARY KEY, identity TEXT NOT NULL, added_at INTEGER NOT NULL, added_by TEXT NOT NULL, revoked_at INTEGER);
CREATE INDEX IF NOT EXISTS keys_identity ON keys (identity);
CREATE TABLE IF NOT EXISTS subjects (
  issuer TEXT NOT NULL, subject TEXT NOT NULL, identity TEXT NOT NULL, linked_at INTEGER NOT NULL, email TEXT NOT NULL,
  signed_in_at INTEGER NOT NULL, PRIMARY KEY (issuer, subject));
CREATE INDEX IF NOT EXISTS subjects_identity ON subjects (identity);
CREATE INDEX IF NOT EXISTS subjects_email ON subjects (email);
CREATE TABLE IF NOT EXISTS person_keys (
  identity TEXT PRIMARY KEY, sealed TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS pictures (
  identity TEXT PRIMARY KEY, sha TEXT NOT NULL, mime TEXT NOT NULL, set_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS agent_fragments (
  identity TEXT PRIMARY KEY, fragment TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS logins (
  state TEXT PRIMARY KEY, return_to TEXT NOT NULL, link_to TEXT, created_at INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS logins_created ON logins (created_at);
CREATE TABLE IF NOT EXISTS sessions (
  hash TEXT PRIMARY KEY, identity TEXT NOT NULL, fragment TEXT, parent TEXT, workos_sid TEXT,
  created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL, revoked_at INTEGER, embedder TEXT);
CREATE INDEX IF NOT EXISTS sessions_parent_fragment ON sessions (parent, fragment, created_at) WHERE parent IS NOT NULL;
CREATE INDEX IF NOT EXISTS sessions_expires ON sessions (expires_at);
CREATE TABLE IF NOT EXISTS redemptions (
  hash TEXT PRIMARY KEY, session TEXT NOT NULL, fragment TEXT NOT NULL, return_to TEXT NOT NULL,
  expires_at INTEGER NOT NULL, embedder TEXT);
CREATE INDEX IF NOT EXISTS redemptions_session ON redemptions (session, expires_at);
CREATE INDEX IF NOT EXISTS redemptions_expires ON redemptions (expires_at);
CREATE TABLE IF NOT EXISTS consents (
  identity TEXT NOT NULL, fragment TEXT NOT NULL, at INTEGER NOT NULL, PRIMARY KEY (identity, fragment));
CREATE INDEX IF NOT EXISTS consents_at ON consents (identity, at);
CREATE TABLE IF NOT EXISTS e2e_days (
  day INTEGER PRIMARY KEY, people INTEGER NOT NULL, paid_calls INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS wipes (
  identity TEXT PRIMARY KEY, done INTEGER NOT NULL, started_at INTEGER NOT NULL, by TEXT NOT NULL, finished_at INTEGER);
CREATE TABLE IF NOT EXISTS wiped_agents (
  identity TEXT NOT NULL, agent TEXT NOT NULL, PRIMARY KEY (identity, agent));
CREATE TABLE IF NOT EXISTS invites_waiting (
  email TEXT NOT NULL, fragment TEXT NOT NULL, expires_at INTEGER NOT NULL, PRIMARY KEY (email, fragment));
CREATE INDEX IF NOT EXISTS invites_waiting_expiry ON invites_waiting (expires_at);
CREATE TABLE IF NOT EXISTS invite_mails (
  sharer TEXT NOT NULL, day INTEGER NOT NULL, mailed INTEGER NOT NULL, PRIMARY KEY (sharer, day));
CREATE TABLE IF NOT EXISTS orgs (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, created_at INTEGER NOT NULL, created_by TEXT NOT NULL, status TEXT,
  customer TEXT UNIQUE, subscription TEXT UNIQUE, items TEXT, period_end INTEGER, trial_end INTEGER,
  cancel_at_end INTEGER NOT NULL DEFAULT 0, event_at INTEGER, synced_at INTEGER);
CREATE INDEX IF NOT EXISTS orgs_synced ON orgs (synced_at) WHERE subscription IS NOT NULL;
CREATE TABLE IF NOT EXISTS org_members (
  id TEXT PRIMARY KEY, org TEXT NOT NULL, person TEXT UNIQUE, email TEXT NOT NULL, admin INTEGER NOT NULL,
  seat TEXT, comped INTEGER NOT NULL, sleeps INTEGER NOT NULL, added_at INTEGER NOT NULL, added_by TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS org_members_org ON org_members (org);
CREATE UNIQUE INDEX IF NOT EXISTS org_members_org_email ON org_members (org, email);
CREATE INDEX IF NOT EXISTS org_members_offers ON org_members (email, added_at) WHERE person IS NULL;
CREATE TABLE IF NOT EXISTS plan_syncs (
  person TEXT PRIMARY KEY, since INTEGER NOT NULL, due INTEGER NOT NULL, tries INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS plan_syncs_due ON plan_syncs (due);
CREATE TABLE IF NOT EXISTS plan_seq (
  one INTEGER PRIMARY KEY CHECK (one = 1), seq INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS checkouts (
  session TEXT PRIMARY KEY, org TEXT NOT NULL, person TEXT NOT NULL, kind TEXT NOT NULL, at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS quantity_syncs (
  org TEXT PRIMARY KEY, since INTEGER NOT NULL, due INTEGER NOT NULL, tries INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS trial_codes (
  id TEXT PRIMARY KEY, code TEXT NOT NULL UNIQUE, name TEXT NOT NULL, kind TEXT NOT NULL, days INTEGER NOT NULL,
  capacity INTEGER NOT NULL, expires_at INTEGER, active INTEGER NOT NULL, revision INTEGER NOT NULL,
  created_at INTEGER NOT NULL, created_by TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS trial_uses (
  person TEXT PRIMARY KEY, code TEXT NOT NULL, at INTEGER NOT NULL, expires_at INTEGER NOT NULL, subscription TEXT);
CREATE INDEX IF NOT EXISTS trial_uses_code ON trial_uses (code, expires_at);
CREATE TABLE IF NOT EXISTS admin_log (
  n INTEGER PRIMARY KEY AUTOINCREMENT, at INTEGER NOT NULL, operator TEXT NOT NULL, action TEXT NOT NULL,
  target TEXT NOT NULL, detail TEXT NOT NULL);
