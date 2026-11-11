-- Vault, step 2: a secret's move out (FIN-466; fragment_core::vault::take).
--
-- `secrets.claim`: the destination (`fragment/KEY`) a move's take claimed
-- the value it read for, in that same turn; only it removes the secret
-- (`vault/moved`), and another destination's take is refused while it
-- stands. Setting the secret again clears it. NULL: no move under way.
ALTER TABLE secrets ADD COLUMN claim TEXT;
