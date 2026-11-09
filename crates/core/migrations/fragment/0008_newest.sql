-- Fragment, step 8: each poster's newest record of the kinds a channel
-- keeps the newest of (docs/api.md, "Newest records"; cell/src/channels.rs
-- `keep_newest`). What a channel's `newest` declares, as installed from
-- live, beside its other declarations; and the place of each poster's
-- newest record of each such kind, which `…/channels/{c}/newest` reads.
CREATE TABLE IF NOT EXISTS code_channel_newest (channel TEXT PRIMARY KEY, kinds TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS newest (
  channel TEXT NOT NULL, principal TEXT NOT NULL, kind TEXT NOT NULL, seq INTEGER NOT NULL, PRIMARY KEY (channel, principal, kind));
