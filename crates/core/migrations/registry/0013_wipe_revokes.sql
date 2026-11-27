-- A wipe's `connections` step (FIN-588; fragment_core::wipe_connections):
-- each account of the wiped person's whose revoke at its provider failed,
-- by its provider and Pipes id, how many calls it failed and when it first
-- did, so the step gives it up only after enough tries over long enough.
-- Ids and counts, nothing of anyone; deleted with the wipe's other rows at
-- its `registry` step.
CREATE TABLE IF NOT EXISTS wipe_revokes (
  identity TEXT NOT NULL, provider TEXT NOT NULL, account TEXT NOT NULL, tries INTEGER NOT NULL, first_ms INTEGER NOT NULL,
  PRIMARY KEY (identity, provider, account));
