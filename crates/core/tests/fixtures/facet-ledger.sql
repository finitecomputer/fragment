-- The app ledger deployed before schema versions. Keep this shape frozen:
-- CREATE TABLE IF NOT EXISTS never changes a table an existing facet holds.
CREATE TABLE _fragment_ops (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, input_sha TEXT NOT NULL, result TEXT NOT NULL, at INTEGER NOT NULL,
  effects TEXT NOT NULL, run INTEGER NOT NULL);
