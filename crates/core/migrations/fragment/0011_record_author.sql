-- Fragment, step 11: who wrote each record (docs/api.md, "Who wrote a
-- record"): a person, an agent, app code, the platform or a door, stamped
-- by the platform as it appends one (cell/src/channels.rs `append`), never
-- by its caller. A record stored before it has none and reads as it did;
-- the release before writes none, and reads past the column. A mutation
-- waiting for its effects keeps the code that ran it (`pending.code`), so
-- its records are stamped by that code, not by one installed since; a row
-- from before has none, and its records are app code's.
ALTER TABLE records ADD COLUMN author TEXT;
ALTER TABLE pending ADD COLUMN code TEXT;
