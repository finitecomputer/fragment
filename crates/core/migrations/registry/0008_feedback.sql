-- What people send the deployment's operators from the shell
-- (fragment_core::feedback; docs/api.md, Feedback): a problem or a
-- message, from the page they sent it on. Its screenshots' bytes are in
-- BLOBS under feedback/<person>/<id>/<place>; `screenshots` is their JSON
-- list of {mime, size}. `n` orders the admin's pages; `id`, the sender's
-- one request, makes a send asked again the same one.
CREATE TABLE feedback (
  n INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE,
  person TEXT NOT NULL, email TEXT, kind TEXT NOT NULL, message TEXT NOT NULL,
  page TEXT, user_agent TEXT, release TEXT NOT NULL, screenshots TEXT NOT NULL,
  created_at INTEGER NOT NULL, resolved_at INTEGER, resolved_by TEXT);
CREATE INDEX feedback_person ON feedback (person, created_at);
