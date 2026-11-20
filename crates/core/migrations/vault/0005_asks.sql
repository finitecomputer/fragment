-- Vault, step 5: an agent's asks of its person (FIN-466; docs/secrets.md,
-- "An agent asks").
--
-- `asks`: each by its id (32 hex), the ask as JSON (`AskSecret` with the
-- agent fragment that asked and what its card calls it), when it was
-- made and when it expires unanswered (ms), and where it stands
-- (`waiting`, `saved`, `declined`, `expired`). No value is ever kept here:
-- an answer's goes into `secrets`, sealed, in the turn that ends the ask.
-- One ended is read by its agent a while, then deleted.
CREATE TABLE asks (
  id TEXT PRIMARY KEY,
  ask TEXT NOT NULL,
  at INTEGER NOT NULL,
  expires INTEGER NOT NULL,
  state TEXT NOT NULL
);
