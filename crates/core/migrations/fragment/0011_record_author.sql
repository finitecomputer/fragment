-- Fragment, step 11: who wrote each record (docs/api.md, "Who wrote a
-- record"): a person, an agent, app code, the platform or a door, stamped
-- by the platform as it appends one (cell/src/channels.rs `append`), never
-- by its caller. A record stored before it has none and reads as it did;
-- the release before writes none, and reads past the column.
ALTER TABLE records ADD COLUMN author TEXT;
