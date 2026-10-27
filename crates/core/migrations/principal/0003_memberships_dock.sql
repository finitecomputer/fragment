-- Principal, step 3: whether the person's dock shows a fragment of theirs
-- (`PUT /api/fragments/{name}/dock`; principal.rs), their own choice as
-- their archiving is. Every row is on it until they take it off, as every
-- app was before the choice (#587).
ALTER TABLE memberships ADD COLUMN dock INTEGER NOT NULL DEFAULT 1;
