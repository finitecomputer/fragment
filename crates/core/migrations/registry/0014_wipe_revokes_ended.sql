-- A wipe's `connections` step (FIN-588; fragment_core::wipe_connections::
-- Ended): how an account's revoke ended, kept until its delete at Pipes
-- answered, so a call cut after the revoke (or a delete that failed)
-- resumes at the delete. 0: its revoke still to try; 1: revoked, or known
-- gone; 2: given up.
ALTER TABLE wipe_revokes ADD COLUMN ended INTEGER NOT NULL DEFAULT 0;
