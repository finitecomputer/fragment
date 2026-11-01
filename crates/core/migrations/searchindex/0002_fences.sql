-- SearchIndex, step 2: each source's fence (cell/src/search_index.rs;
-- docs/api.md, "A fragment's search"): the highest a writer that sends one
-- had a write or delete of it taken at. A lower one is refused, so a
-- writer's stale work never lands over its newer; a delete keeps its row
-- (a tombstone), so a stale write cannot bring a source back. A clear of
-- another space's vectors keeps it; the life's end drops it with the rest.
CREATE TABLE IF NOT EXISTS fences (source TEXT PRIMARY KEY, fence INTEGER NOT NULL);
