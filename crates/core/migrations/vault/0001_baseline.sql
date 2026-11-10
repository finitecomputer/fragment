-- Vault, step 1: a person's secrets (cell/src/vault.rs; docs/secrets.md,
-- "A person's Vault"), one object a person, named by their identity.
--
-- `secrets`: each by its name, its value sealed for this object
-- (keys::seal), never answered, and `secret`, its `Secret` as JSON (its
-- hosts, place, agents and when it was set), set only with the value.
-- `wiped`: one row once its person's wipe emptied it.
CREATE TABLE IF NOT EXISTS secrets (name TEXT PRIMARY KEY, sealed TEXT NOT NULL, secret TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS wiped (at INTEGER NOT NULL);
