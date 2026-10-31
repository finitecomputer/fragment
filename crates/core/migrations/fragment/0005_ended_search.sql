-- Fragment, step 5: whether an ended life may have written its search
-- index (cell/src/ended.rs, cell/src/searchable.rs): its end ends that
-- index (a SearchIndex object of its own), and a life that never wrote
-- one wakes none. Lives ended before step 5 indexed in the search
-- service, which is gone: none of theirs.
ALTER TABLE ended ADD COLUMN search INTEGER NOT NULL DEFAULT 0;
