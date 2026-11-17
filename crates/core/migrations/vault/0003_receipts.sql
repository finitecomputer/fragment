-- Vault, step 3: a hand-off's receipts (FIN-463 F11; docs/migrations.md,
-- "The secrets hand-off").
--
-- `receipts.key`: one secret an old finite.computer agent handed to this
-- Vault (`<old> <kind>`), written in the turn that puts it. A hand-off
-- whose receipt is here puts nothing, so a retry or an overlapping call
-- never puts back a secret its person removed, moved or set since. Kept
-- after the secret goes; a wipe empties it with the rest.
CREATE TABLE receipts (
  key TEXT PRIMARY KEY,
  at INTEGER NOT NULL
);
