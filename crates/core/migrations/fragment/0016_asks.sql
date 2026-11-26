-- Fragment, step 16: one ask (FIN-615; docs/api.md, "One ask"). The cards
-- the platform asked here (`asks`): each its record on `work`, its agent,
-- whose ask it is and whom it asks, what (`spend`, `accounts`, `delete`,
-- `cap`), what about (a lend's place, a dangerous action's fragment and
-- cap) and that place's key, when it runs out, and its answer once given.
-- And what an agent member's owner allowed once here (`lent_once`): its
-- use, for one asker, of what its lend keeps off, from `after` (ms), taken
-- by that asker's first request since (its record, `channel` and `seq`) and
-- by no other.
CREATE TABLE IF NOT EXISTS asks (
  seq INTEGER PRIMARY KEY, agent TEXT NOT NULL, asker TEXT NOT NULL, asks TEXT NOT NULL, what TEXT NOT NULL, about TEXT NOT NULL, key TEXT, until INTEGER NOT NULL, answer TEXT);
CREATE TABLE IF NOT EXISTS lent_once (
  agent TEXT NOT NULL, asker TEXT NOT NULL, what TEXT NOT NULL, after INTEGER NOT NULL, channel TEXT, seq INTEGER, PRIMARY KEY (agent, asker, what));
