-- What people send the deployment's operators from the shell
-- (fragment_core::feedback; docs/api.md, Feedback): a problem or a
-- message, from the page they sent it on. Its screenshots' bytes are in
-- BLOBS under feedback/<person>/<id>/<place>; `screenshots` is their JSON
-- list of {mime, size}. `n` orders the admin's pages; `id`, the sender's
-- one send (their dialog's), makes a send asked again the same one.
-- `state` is `sending` (its screenshots being put), `kept`, or `deleting`
-- (its screenshots being deleted): a row is the record of its bytes until
-- they are gone, and the registry's alarm deletes a send never kept and a
-- deleting one's bytes, then the row (fragment_core::feedback::State).
-- `mailed_at`: when the operators' notice went, so a kept one whose
-- sender heard no answer is mailed when it is asked again, and once.
CREATE TABLE feedback (
  n INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE,
  person TEXT NOT NULL, email TEXT, kind TEXT NOT NULL, message TEXT NOT NULL,
  page TEXT, user_agent TEXT, release TEXT NOT NULL, screenshots TEXT NOT NULL,
  created_at INTEGER NOT NULL, resolved_at INTEGER, resolved_by TEXT,
  state TEXT NOT NULL, mailed_at INTEGER);
CREATE INDEX feedback_person ON feedback (person, created_at);
CREATE INDEX feedback_state ON feedback (state, created_at);
