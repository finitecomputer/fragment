-- Fragment, step 7: a video step's call while it waits on its model
-- (cell/src/ai.rs, `step_video`; docs/technical-debt-ledger.md). Written
-- before the model is asked, by the step's key, with the run's attempt;
-- gone when the model answers. A try that finds its own attempt's row and
-- no receipt never heard its answer, and asks no second time.
CREATE TABLE IF NOT EXISTS making (key TEXT PRIMARY KEY, run INTEGER NOT NULL, attempt INTEGER NOT NULL, at INTEGER NOT NULL);
