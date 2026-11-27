-- Principal, step 5: the person's yes to a fragment's page using their
-- microphone (`PUT /api/fragments/{name}/microphone`; principal.rs), asked
-- once in the shell (FIN-615, PR 11): on its row, as their pin is; and their
-- one yes for every chat of the release's chat template (`person_own`, the
-- person's own settings that are no row's: `microphone-chats`). None is
-- lent until they say so.
ALTER TABLE memberships ADD COLUMN microphone INTEGER NOT NULL DEFAULT 0;
CREATE TABLE IF NOT EXISTS person_own (name TEXT PRIMARY KEY, value INTEGER NOT NULL);
