-- Vault, step 1: a person's secrets (cell/src/vault.rs; docs/secrets.md,
-- "A person's Vault"), one object a person, named by their identity.
--
-- `meta`: `person`, whose it is, named by its first put.
-- `secrets`: each by its name, its value sealed for this object
-- (keys::seal), never answered; `hosts`, `place` and `agents` JSON, set
-- only with the value. `wiped`: one row once its person's wipe emptied it.
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS secrets (
  name TEXT PRIMARY KEY, sealed TEXT NOT NULL, hosts TEXT NOT NULL, place TEXT NOT NULL, agents TEXT NOT NULL, set_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS wiped (at INTEGER NOT NULL);
