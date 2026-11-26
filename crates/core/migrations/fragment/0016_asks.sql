-- Fragment, step 16: whose ask each turn is, as its place says it
-- (FIN-720; docs/api.md, "Who asked: mandates"). On a place: each ask its
-- agent members were handed, whose it is, the turn its agent claimed it
-- with, and until when it holds once its agent ended it (open while none);
-- and for each agent, its agent fragment's name, the newest record of
-- `chat` it was handed, the version of its asks here, the newest version
-- that opened someone else's (its agent waits for that one to be taken),
-- and the version its agent fragment took (the rest is due, retried from
-- the alarm). On an agent fragment: the version of each place's turns it
-- holds (`turns`, step 15, as its places tell them), so a late one
-- changes nothing.
CREATE TABLE IF NOT EXISTS asks (
  agent TEXT NOT NULL, channel TEXT NOT NULL, seq INTEGER NOT NULL, asker TEXT NOT NULL, proven INTEGER NOT NULL, at INTEGER NOT NULL,
  turn TEXT, until INTEGER, PRIMARY KEY (agent, channel, seq));
CREATE INDEX IF NOT EXISTS asks_turn ON asks (agent, turn) WHERE turn IS NOT NULL;
CREATE TABLE IF NOT EXISTS asks_told (
  agent TEXT PRIMARY KEY, fragment TEXT, handed INTEGER NOT NULL, version INTEGER NOT NULL, gate INTEGER NOT NULL, told INTEGER NOT NULL, attempts INTEGER NOT NULL, next_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS turn_places (place TEXT PRIMARY KEY, version INTEGER NOT NULL);
