-- Fragment, step 2: listed records (docs/api.md, "Listed records";
-- cell/src/listed.rs). What a channel's `listed` declares, as installed
-- from live, beside its other declarations; and each member's newest
-- record of a listed kind, which its members' list rows carry.
CREATE TABLE IF NOT EXISTS code_channel_listed (channel TEXT PRIMARY KEY, kinds TEXT NOT NULL, fields TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS listed (
  channel TEXT NOT NULL, principal TEXT NOT NULL, seq INTEGER NOT NULL, at INTEGER NOT NULL, kind TEXT NOT NULL,
  fields TEXT NOT NULL, last INTEGER NOT NULL, PRIMARY KEY (channel, principal));
