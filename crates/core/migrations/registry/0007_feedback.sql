-- What people send the deployment's operators from the shell
-- (fragment_core::feedback; docs/api.md, Feedback): a problem or a
-- message, from the page they sent it on. Its screenshots' bytes are in
-- BLOBS under feedback/<person>/<id>/<attempt>/<place>; `screenshots` is
-- their JSON list of {mime, size}, its `attempt`'s. `n` orders the admin's
-- pages; `id`, the sender's one send (their dialog's), makes a send asked
-- again the same one.
-- `state` is `sending` (its screenshots being put), `kept`, or `deleting`
-- (its screenshots being deleted): a row is the record of its bytes until
-- they are gone, and the registry's alarm deletes a send never kept and a
-- deleting one's bytes, then the row (fragment_core::feedback::State).
-- `attempt` is its latest try (each begin makes one; only it may keep the
-- row), `attempt_at` when it began (a send never kept expires an hour on),
-- and `attempts` how many it has had. `fenced_until`, when set, is when
-- the last byte any of its tries could put has landed: before it, its
-- bytes are not deleted, nor an earlier try's pruned.
-- `mailed_at`: when the operators' notice went, so a kept one whose
-- sender heard no answer is mailed when it is asked again, and once.
CREATE TABLE feedback (
  n INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE,
  person TEXT NOT NULL, email TEXT, kind TEXT NOT NULL, message TEXT NOT NULL,
  page TEXT, user_agent TEXT, release TEXT NOT NULL, screenshots TEXT NOT NULL,
  created_at INTEGER NOT NULL, resolved_at INTEGER, resolved_by TEXT,
  state TEXT NOT NULL, attempt TEXT NOT NULL, attempt_at INTEGER NOT NULL,
  attempts INTEGER NOT NULL, fenced_until INTEGER, mailed_at INTEGER);
CREATE INDEX feedback_person ON feedback (person, created_at);
CREATE INDEX feedback_state ON feedback (state, attempt_at);
CREATE INDEX feedback_fence ON feedback (fenced_until) WHERE fenced_until IS NOT NULL;
