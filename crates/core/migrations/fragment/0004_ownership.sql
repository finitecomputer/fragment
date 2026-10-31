-- Fragment, step 4: who owns it (docs/cloudflare-v1.md, decisions 66 to
-- 71; fragment_core::owner): a person, by their identity, or an org, by
-- its id, in one row, the one record of it; `generation` counts its
-- owners from 1. The `owner` meta row it is read from goes.
CREATE TABLE ownership (
  one INTEGER PRIMARY KEY CHECK (one = 1),
  kind TEXT NOT NULL CHECK (kind IN ('person', 'org')),
  owner TEXT NOT NULL CHECK (length(owner) > 0),
  generation INTEGER NOT NULL CHECK (generation >= 1)
);
INSERT INTO ownership (one, kind, owner, generation) SELECT 1, 'person', value, 1 FROM meta WHERE key = 'owner';
DELETE FROM meta WHERE key = 'owner';
-- In an org's fragment, the epoch of the org's content a member came in
-- at (an agent: its person's): they count only while their place there
-- is still at it. NULL in a person's fragment.
ALTER TABLE members ADD COLUMN epoch INTEGER;
