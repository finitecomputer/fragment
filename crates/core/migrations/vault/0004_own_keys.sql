-- Vault, step 4: a person's own keys and their sign-ins, moved here from
-- their computer (M16; docs/secrets.md, "A person's Vault").
--
-- `own_keys`: the person's key for each `own` provider of the catalog,
-- used by every agent of theirs it is not narrowed from: `sealed` for this
-- object, or NULL once its person removed it, so a key their computer
-- kept from before the move and hands over late never takes the place of
-- what they said since (`vault/own-handover` takes a provider only where
-- no row is). `set_at`: when its person set or removed it.
-- `own_signins`: a sign-in to an own key's provider under way, kept under
-- its nonce's hash (`own_signin::nonce_key`) with its verifier sealed,
-- until `expires_at`.
CREATE TABLE own_keys (provider TEXT PRIMARY KEY, sealed TEXT, set_at INTEGER NOT NULL);
CREATE TABLE own_signins (nonce TEXT PRIMARY KEY, provider TEXT NOT NULL, sealed TEXT NOT NULL, expires_at INTEGER NOT NULL);
