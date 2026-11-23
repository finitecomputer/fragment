-- Fragment, step 12: a mutation waiting for its effects keeps the app code
-- that ran it (its loader id), so the records it publishes are stamped by
-- that code (step 11, `records.author`; cell/src/channels.rs
-- `code_author`), never by code installed since. A row from before has
-- none, and its records are app code's. The release before writes none,
-- and reads past the column.
ALTER TABLE pending ADD COLUMN code TEXT;
