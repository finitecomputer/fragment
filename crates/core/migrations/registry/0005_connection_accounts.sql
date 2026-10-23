-- Several accounts at one connection (a row's `accounts_max` above 1):
-- what a pending connect is for, `connect`, `add` (another account) or
-- `reconnect` (the account `account` names, by its Pipes id), which its
-- callback imports as (fragment_core::connection_accounts::Intent). Rows
-- from before are connects.
ALTER TABLE connection_signins ADD COLUMN intent TEXT NOT NULL DEFAULT 'connect';
ALTER TABLE connection_signins ADD COLUMN account TEXT;
-- A disconnect of one of several accounts, fenced by that account alone
-- (its identifier: the ID token's `sub` its import set) until a connect
-- of it is known to have imported again, so the person's other accounts
-- at the provider stay connected.
CREATE TABLE connection_account_fences (
  issuer TEXT NOT NULL, subject TEXT NOT NULL, provider TEXT NOT NULL,
  identifier TEXT NOT NULL, fenced_at INTEGER NOT NULL,
  PRIMARY KEY (issuer, subject, provider, identifier));
