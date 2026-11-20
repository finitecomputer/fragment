-- The operators' glance's reads of the deployment's logs, kept
-- (fragment_core::glance::Part): each part's answer, as the cell's JSON,
-- when it was read and for which day's start, and until when a load is
-- reading it again. A cache: a part's key names its answer's shape, and
-- an answer that does not decode is read again.
CREATE TABLE glance_parts (
  part TEXT PRIMARY KEY,
  busy_until INTEGER NOT NULL,
  at INTEGER,
  day_start INTEGER,
  answer TEXT
);
