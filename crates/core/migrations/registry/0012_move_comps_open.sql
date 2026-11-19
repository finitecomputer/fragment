-- The move's comps last until staff end them (FIN-463, 2026-11-19): a
-- comp the move gave with an end (step 10, 30 days on) loses it. A
-- move's comp is a row whose last comp in the operators' log Migrations
-- made, or, none logged (the log keeps its newest 10,000), a row
-- Migrations added. An operator's comp keeps its own end.
UPDATE org_members SET until_ms = NULL
WHERE until_ms IS NOT NULL AND comped = 1
  AND COALESCE((SELECT a.operator FROM admin_log a WHERE a.action = 'comp' AND a.target = org_members.id ORDER BY a.n DESC LIMIT 1), added_by) = 'migrations';
