-- A fragment's sign-in rows end with its name's life (crates/core/src/
-- site_signin.rs): a create, and a delete, forget every site session,
-- unspent redemption and yes the name keys. Each is found by the name.
CREATE INDEX sessions_fragment ON sessions (fragment) WHERE fragment IS NOT NULL;
CREATE INDEX redemptions_fragment ON redemptions (fragment);
CREATE INDEX consents_fragment ON consents (fragment);
