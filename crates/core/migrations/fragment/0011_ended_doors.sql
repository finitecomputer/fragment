-- Fragment, step 11: a SimpleX connector's door ends with it (cell/door.mjs
-- `/door/end`): its container, its saves in the bucket and its object's
-- storage. A life that ends records its door here, beside its other
-- parts, and so does a life that installs other code (no door again), and
-- the alarm ends each, retried as the others are (cell/src/ended.rs). Kept
-- through a life's end, as the other `ended_` tables are.
CREATE TABLE IF NOT EXISTS ended_doors (
  incarnation INTEGER PRIMARY KEY, door TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0, next_at INTEGER NOT NULL);
