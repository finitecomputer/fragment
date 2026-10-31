-- Fragment, step 4: the sources a push deleted that the search service may
-- still hold (docs/api.md, "A fragment's search"; cell/src/searchable.rs).
-- A source named by a path is forgotten when a move of main deletes its
-- file: written here in the move's turn, due at once (`next_at`), deleted
-- from the service by the alarm (a failure due again later), and from here
-- once it is gone there. A search leaves out what is here.
CREATE TABLE IF NOT EXISTS search_forget (source TEXT PRIMARY KEY, next_at INTEGER NOT NULL);
