-- Computer, step 1: the schema as it stood when its steps began
-- (2026-10-19). Frozen once merged, as every step is: a change to it is
-- a new step (crates/core/src/migrate.rs says how).

-- a computer's record (computer.rs): a wipe drops them and makes them again empty.
-- `agents.connections` is JSON: `null`, every provider the owner has
-- (decision 44), or a list that narrows it; an agent's row names it as it
-- is made (`computer/assign`), so the column's default is never read.
-- `uses` counts each agent's calls to each provider a month (`Month`'s
-- index) and what they were charged; `own_keys` holds the owner's own keys,
-- sealed for this object; `own_signins` the sign-ins to an own key's
-- provider under way (`fragment_core::own_signin`): each nonce's hash, its
-- provider, its verifier sealed, until it expires or is finished, once.
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS agents (fragment TEXT PRIMARY KEY, identity TEXT NOT NULL, owner TEXT NOT NULL, added_at INTEGER NOT NULL, connections TEXT NOT NULL DEFAULT 'null');
CREATE TABLE IF NOT EXISTS awake (from_ms INTEGER PRIMARY KEY, to_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS tickets (hash TEXT PRIMARY KEY, port INTEGER NOT NULL, identity TEXT NOT NULL, expires_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS sessions (hash TEXT PRIMARY KEY, identity TEXT NOT NULL, expires_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS uses (month INTEGER NOT NULL, provider TEXT NOT NULL, agent TEXT NOT NULL, calls INTEGER NOT NULL, micros INTEGER NOT NULL, PRIMARY KEY (month, provider, agent));
CREATE TABLE IF NOT EXISTS own_keys (provider TEXT PRIMARY KEY, sealed TEXT NOT NULL, set_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS own_signins (nonce TEXT PRIMARY KEY, provider TEXT NOT NULL, sealed TEXT NOT NULL, expires_at INTEGER NOT NULL);

-- the one row a wiped computer keeps
CREATE TABLE IF NOT EXISTS wiped (at INTEGER NOT NULL);
