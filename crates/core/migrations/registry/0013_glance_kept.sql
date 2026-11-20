-- The operators' glance's read of the deployment's logs, kept
-- (fragment_core::glance): the answer, as the cell's JSON, when it was
-- read and for which day's start, the computers its trouble names (one a
-- line: the registry says whose), and until when a load is reading the
-- logs again. A cache, keyed by its answer's shape (`KEPT_SHAPE`): a new
-- shape is a new row, never read as the old.
CREATE TABLE glance_kept (
  shape TEXT PRIMARY KEY,
  busy_until INTEGER NOT NULL,
  at INTEGER,
  day_start INTEGER,
  answer TEXT,
  computers TEXT
);
