-- Fragment, step 10: a fragment's title and an agent's role are its
-- cell's own (`meta` `face`, set through its face route), and an agent's
-- picture is its icon (`meta` `icon`, one of its blobs). The release
-- before reads them from live at each install: it would write a rename
-- away, and serve no taken picture. This step makes the release forward
-- only (docs/MODEL.md, Schemas): an object that took it refuses the older
-- code, and a failed deploy is never rolled back across it
-- (xtask/src/rollout.rs `eligible`). It changes no table: the statement
-- touches no row.
UPDATE meta SET value = value WHERE 0;
