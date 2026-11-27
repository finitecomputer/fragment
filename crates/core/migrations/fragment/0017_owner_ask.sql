-- Fragment, step 17: one ask (FIN-615; docs/api.md, "One ask"). The cards
-- the platform asked here (`owner_asks`): each its record on `work`, its agent,
-- whose ask it is and whom it asks, what (`spend`, `accounts`, `delete`,
-- `cap`), what about (a lend's place, a dangerous action's fragment and
-- cap) and that place's key, when it runs out, its answer once given, and
-- whether that answer's write is done (once, whatever retries it). And
-- what an agent member's owner allowed once here (`lent_once`): its use,
-- for one asker, of what its lend keeps off, lent to that asker's first
-- record on `chat` after `after` (a seq) and to no other.
CREATE TABLE IF NOT EXISTS owner_asks (
  seq INTEGER PRIMARY KEY, agent TEXT NOT NULL, asker TEXT NOT NULL, asks TEXT NOT NULL, what TEXT NOT NULL, about TEXT NOT NULL, key TEXT, until INTEGER NOT NULL, answer TEXT, done INTEGER);
CREATE TABLE IF NOT EXISTS lent_once (
  agent TEXT NOT NULL, asker TEXT NOT NULL, what TEXT NOT NULL, after INTEGER NOT NULL, PRIMARY KEY (agent, asker, what));
