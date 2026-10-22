-- A person's disconnect of their account at a provider, kept until a
-- connect of theirs is known to have imported again: while it stands, no
-- read or swap takes Pipes' word that the account is connected (an import
-- whose outcome was unknown may land at Pipes after the disconnect).
CREATE TABLE connection_fences (
  issuer TEXT NOT NULL, subject TEXT NOT NULL, provider TEXT NOT NULL,
  fenced_at INTEGER NOT NULL, PRIMARY KEY (issuer, subject, provider));
