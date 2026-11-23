-- Computer, step 9: each operator key's calls this hour from this
-- computer, for its row's `calls_an_hour` (FIN-616: Google Places, kept
-- free, never at very high volume): kept across an eviction or a restart,
-- so a new isolate counts on from where the last left off.
CREATE TABLE key_calls (provider TEXT PRIMARY KEY, hour INTEGER NOT NULL, used INTEGER NOT NULL);
