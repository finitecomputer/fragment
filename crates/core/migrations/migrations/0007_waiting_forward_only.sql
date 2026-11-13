-- Migrations, step 7: a good import waits for staff's word in `retiring`
-- with no command (#1263). The release before reads that row, but not
-- its meaning: its staff ops refuse it, and once its stage's keep passes
-- it holds it as `retire_timed_out`, whose `retry` retires the old agent
-- instead of importing again. This step makes the release that writes it
-- forward only (docs/MODEL.md, Schemas): the object that took it refuses
-- the older code, and a failed deploy is never rolled back across it
-- (xtask/src/rollout.rs `eligible`). It changes no table: the statement
-- touches no row.
UPDATE moves SET state = state WHERE 0;
