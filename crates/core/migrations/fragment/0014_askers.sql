-- Fragment, step 14: whose ask each thing is (FIN-720; docs/api.md, "Who
-- asked: mandates"). A schedule's asker (whom its maker acted for, else its
-- maker), checked again at each fire, and the record a fire appends for
-- them; a run's asker when that is someone else than its agent's owner,
-- whose call ids are then their own (`runs_asked` in place of `runs_call`,
-- the same key with the asker after it, so a lookup seeks as before: none
-- for a run from before); an agent member's lend here, what its owner lets
-- it use of theirs for someone else's ask (the lend card); and, on an agent
-- fragment, the turns its bridge runs, each with whose ask it is, its
-- mandate and the bridge life that told it, and the lives heard. A row
-- from before has none of them: no asker, as before, and a lend never set
-- (`Lend::DEFAULT`).
ALTER TABLE record_schedules ADD COLUMN asker TEXT;
ALTER TABLE records ADD COLUMN asker TEXT;
ALTER TABLE runs ADD COLUMN asker TEXT;
DROP INDEX IF EXISTS runs_call;
CREATE UNIQUE INDEX IF NOT EXISTS runs_asked ON runs (principal, call_id, COALESCE(asker, '')) WHERE call_id IS NOT NULL;
ALTER TABLE members ADD COLUMN lend INTEGER;
CREATE TABLE IF NOT EXISTS turns (
  turn TEXT PRIMARY KEY, place TEXT NOT NULL, asker TEXT NOT NULL, mandate TEXT, until INTEGER NOT NULL, life TEXT NOT NULL, at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS turn_lives (life TEXT PRIMARY KEY, at INTEGER NOT NULL);
