-- The move's comps last until staff end them (FIN-463, 2026-11-19): a
-- comp the move gave with an end (step 10, 30 days on) loses it. A
-- move's comp is a row whose last comp or comp's end in the operators'
-- log is a comp Migrations made. A row with neither logged is the move's
-- when Migrations added it and the log may have forgotten the move's
-- comps (it keeps its newest 10,000): its oldest entry is after the move's
-- comp came, 2026-11-10 (1794268800000 ms). An operator's comp keeps its
-- own end.
UPDATE org_members SET until_ms = NULL
WHERE until_ms IS NOT NULL AND comped = 1
  AND COALESCE(
    (SELECT a.operator || ' ' || a.action FROM admin_log a WHERE a.action IN ('comp', 'comp-end') AND a.target = org_members.id ORDER BY a.n DESC LIMIT 1),
    CASE WHEN (SELECT MIN(at) FROM admin_log) > 1794268800000 THEN added_by || ' comp' END
  ) = 'migrations comp';
