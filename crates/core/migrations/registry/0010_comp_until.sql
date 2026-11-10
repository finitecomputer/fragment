-- A comped seat that ends by itself (FIN-463 F5: the move's month free):
-- when, in ms since the epoch; the alarm ends it then, as an operator's
-- end would. NULL: it lasts until ended.
ALTER TABLE org_members ADD COLUMN until_ms INTEGER;
CREATE INDEX IF NOT EXISTS org_members_until ON org_members (until_ms) WHERE until_ms IS NOT NULL;
