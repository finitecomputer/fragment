-- A move's imports so far (F7): a routine's key names it, so a retried
-- import's routines never repeat an earlier import's keys, which the
-- agent's fragment appends once.
ALTER TABLE moves ADD COLUMN generation INTEGER NOT NULL DEFAULT 0;
