-- Fragment, step 10: an app as a member of another fragment (decision 72,
-- docs/api.md "An app as a member"). Forward only: the release before
-- reads an app member's row (`kind` 'app') as a person's.

-- Where an app is a member: its name, its owner and the life it is (its
-- incarnation), beside its `members` row, which names no owner: its owner
-- reads nothing through it.
CREATE TABLE IF NOT EXISTS member_apps (
  principal TEXT PRIMARY KEY, name TEXT NOT NULL, owner TEXT NOT NULL, incarnation INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS member_apps_name ON member_apps (name);
-- In the app: each fragment it is a member of, from the turn it answered
-- that fragment's ask, at that fragment's life and version, as a list
-- keeps its rows (principal.rs `index`); `role` NULL once it is out, so a
-- late change never undoes a newer one.
CREATE TABLE IF NOT EXISTS member_of (
  name TEXT PRIMARY KEY, role TEXT, incarnation INTEGER NOT NULL, version INTEGER NOT NULL);
