-- Principal, step 2: listed records (docs/api.md, "Listed records";
-- principal.rs). Each row's listed records, whole, as its fragment last
-- sent them.
CREATE TABLE IF NOT EXISTS listed (fragment TEXT PRIMARY KEY, records TEXT NOT NULL);
