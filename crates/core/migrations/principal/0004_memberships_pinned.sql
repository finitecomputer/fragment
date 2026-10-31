-- Principal, step 4: whether the person pinned a fragment of theirs (`PUT
-- /api/fragments/{name}/pinned`; principal.rs), their own choice as their
-- archiving and their dock are. None is pinned until they pin it.
ALTER TABLE memberships ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
