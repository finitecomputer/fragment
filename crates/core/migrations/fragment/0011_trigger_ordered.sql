-- Fragment, step 11: whether a channel trigger's runs go one at a time, in
-- its records' order (docs/api.md, "Jobs and triggers": `ordered`; cell
-- jobs.rs `LAUNCHABLE`). Stored with its trigger as installed. A release
-- before reads past the column, so its triggers run as they always have,
-- as many at once as are queued, and writes it as its default.
ALTER TABLE code_triggers ADD COLUMN ordered INTEGER NOT NULL DEFAULT 0;
