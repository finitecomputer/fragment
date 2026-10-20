-- Fragment, step 3: scheduled records (docs/api.md, "Scheduled records";
-- cell/src/schedules.rs). A fragment's data schedules: each, added at run
-- time by an editor, appends `body` to `channel` as the fragment itself at
-- each minute its cron names (`next_at`; none while paused). Beside
-- `schedules`, which holds the installed code's cron triggers and is made
-- again at each install; these are the fragment's own data, kept across
-- installs, and the same alarm pass fires both (jobs.rs `fire_cron`).
CREATE TABLE IF NOT EXISTS record_schedules (
  id INTEGER PRIMARY KEY AUTOINCREMENT, cron TEXT NOT NULL, channel TEXT NOT NULL, body TEXT NOT NULL,
  created_by TEXT NOT NULL, created_at INTEGER NOT NULL, next_at INTEGER, last_at INTEGER);
CREATE INDEX IF NOT EXISTS record_schedules_due ON record_schedules (next_at) WHERE next_at IS NOT NULL;
