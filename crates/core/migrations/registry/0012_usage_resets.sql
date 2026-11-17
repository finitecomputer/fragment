-- An operator's reset of this month's usage (docs/ledger.md, "A month's
-- usage reset"; cell/src/registry/resets.rs): each run, and each payer it
-- resets, a row each, queued as it starts and reset by the alarm a batch
-- at a time. A run's `payer` is NULL when it resets every person and org.
CREATE TABLE usage_resets (
  id TEXT PRIMARY KEY,
  operator TEXT NOT NULL,
  payer TEXT,
  started_at INTEGER NOT NULL,
  finished_at INTEGER
);
-- A payer of a run, by its ledger's name: `due` its next try, `tries` its
-- failed ones. Done (`done_at`), it holds its ledger's answer (`restored`),
-- or, given up on, no answer and its last refusal (`why`).
CREATE TABLE usage_reset_payers (
  run TEXT NOT NULL,
  payer TEXT NOT NULL,
  due INTEGER NOT NULL,
  tries INTEGER NOT NULL,
  done_at INTEGER,
  restored INTEGER,
  why TEXT,
  PRIMARY KEY (run, payer)
);
CREATE INDEX usage_reset_payers_due ON usage_reset_payers (due) WHERE done_at IS NULL;
