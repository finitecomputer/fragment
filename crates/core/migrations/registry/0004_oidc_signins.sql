-- Sign-in on OpenID Connect (docs/self-host.md, seam 4): a pending
-- sign-in keeps its PKCE verifier, sealed for that sign-in alone, and the
-- nonce its id_token must carry back; a session keeps its id_token,
-- sealed, as the hint the provider's logout takes. NULL on every row from
-- before, and on WorkOS's.
ALTER TABLE logins ADD COLUMN verifier TEXT;
ALTER TABLE logins ADD COLUMN nonce TEXT;
ALTER TABLE sessions ADD COLUMN id_token TEXT;
