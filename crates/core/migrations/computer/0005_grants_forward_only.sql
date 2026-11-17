-- Computer, step 5: an agent's grants (`agents.connections`,
-- `agent_accounts`) move to its agent fragment (fragment_core::grants),
-- which takes an agent's row once and ignores it after; the release before
-- would write those rows again, and nothing it wrote could be taken. This
-- step makes the release forward only (docs/MODEL.md, Schemas): an object
-- that took it refuses the older code, and a failed deploy is never rolled
-- back across it (xtask/src/rollout.rs `eligible`). It changes no table:
-- the statement touches no row.
UPDATE agents SET connections = connections WHERE 0;
