-- Fragment, step 9: an inbox's own check may be an HMAC, and may name an
-- answer (FIN-508); its `inbox_secret` meta row is then JSON, which the
-- release before reads as no `<header> <SECRET>`, so that inbox answers an
-- error to every delivery. This step makes the release that writes it
-- forward only (docs/MODEL.md, Schemas): an object that took it refuses
-- the older code, and a failed deploy is never rolled back across it
-- (xtask/src/rollout.rs `eligible`). It changes no table: the statement
-- touches no row.
UPDATE meta SET value = value WHERE 0;
