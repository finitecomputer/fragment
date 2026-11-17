-- Fragment, step 6: a trigger and a run may say `installed` (FIN-328), a
-- kind and a `via` the release before reads as corruption (its
-- `triggers()` refuses the kind, its `run_row` panics on the via). This
-- step makes the release that writes them forward only (docs/MODEL.md,
-- Schemas): an object that took it refuses the older code, and a failed
-- deploy is never rolled back across it (xtask/src/rollout.rs
-- `eligible`). It changes no table: the statement touches no row.
UPDATE code_triggers SET kind = kind WHERE 0;
