-- Fragment, step 14: whose ask each thing is (FIN-720; docs/api.md, "Who
-- asked: mandates"). A schedule's asker (whom its maker acted for, else its
-- maker), checked again at each fire, and the record a fire appends for
-- them; a run's asker when that is someone else than its agent's owner,
-- whose call ids are then their own (`runs_asked` in place of `runs_call`,
-- the same key with the asker beside it: none for a run from before); an
-- agent member's lend here, what its owner lets it use of theirs for
-- someone else's ask (the lend card); and, on an agent fragment, the turns
-- its bridge runs, each with whose ask it is and its mandate. A row from
-- before has none of them: no asker, as before, and a lend never set
-- (`Lend::DEFAULT`).
ALTER TABLE record_schedules ADD COLUMN asker TEXT;
ALTER TABLE records ADD COLUMN asker TEXT;
ALTER TABLE runs ADD COLUMN asker TEXT;
DROP INDEX IF EXISTS runs_call;
CREATE UNIQUE INDEX IF NOT EXISTS runs_asked ON runs (principal, COALESCE(asker, ''), call_id) WHERE call_id IS NOT NULL;
ALTER TABLE members ADD COLUMN lend INTEGER;
CREATE TABLE IF NOT EXISTS turns (
  turn TEXT PRIMARY KEY, place TEXT NOT NULL, asker TEXT NOT NULL, mandate TEXT, until INTEGER NOT NULL, at INTEGER NOT NULL);
