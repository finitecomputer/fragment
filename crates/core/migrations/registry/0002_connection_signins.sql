-- A pending connection is spent before any external token exchange. The
-- browser holds its nonce; storage holds only its hash and sealed verifier.
CREATE TABLE connection_signins (
  state TEXT PRIMARY KEY, session TEXT NOT NULL, provider TEXT NOT NULL,
  issuer TEXT NOT NULL, subject TEXT NOT NULL, config TEXT NOT NULL,
  sealed TEXT NOT NULL, expires_at INTEGER NOT NULL);
CREATE INDEX connection_signins_session ON connection_signins (session);
CREATE INDEX connection_signins_expires ON connection_signins (expires_at);
