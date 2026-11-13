-- Requests for access to a fragment (docs/api.md, Asking for access;
-- registry/asks.rs, FIN-502): someone signed in, not in it, asks its
-- owner, who sees it among their notices and is mailed. One row a
-- fragment and person: whom the fragment named its owner as they asked,
-- its title then, when it was asked and runs out, and when the owner
-- dismissed it (a dismissed row keeps a request asked again quiet a
-- while, then goes).
CREATE TABLE IF NOT EXISTS access_asks (
  fragment TEXT NOT NULL, requester TEXT NOT NULL, owner TEXT NOT NULL, title TEXT NOT NULL,
  asked_at INTEGER NOT NULL, expires_at INTEGER NOT NULL, dismissed_at INTEGER,
  PRIMARY KEY (fragment, requester));
CREATE INDEX IF NOT EXISTS access_asks_owner ON access_asks (owner, asked_at);
CREATE INDEX IF NOT EXISTS access_asks_requester ON access_asks (requester);
CREATE INDEX IF NOT EXISTS access_asks_expiry ON access_asks (expires_at);
-- How many requests a person made in a day (`who`, kind `asked`), and how
-- many mails about them an owner was sent (kind `mailed`): today's and
-- yesterday's are kept.
CREATE TABLE IF NOT EXISTS access_ask_days (
  who TEXT NOT NULL, kind TEXT NOT NULL, day INTEGER NOT NULL, n INTEGER NOT NULL, PRIMARY KEY (who, kind, day));
