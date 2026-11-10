-- The access a person picked as they connected, at a row whose read only
-- the platform keeps (its read access names methods: a provider with no
-- read-only scope, Salesforce's), its newest connect known to have
-- imported: `read` or `write` (fragment_core::connection_signin::
-- access_held). None kept is read only.
CREATE TABLE connection_access (
  issuer TEXT NOT NULL, subject TEXT NOT NULL, provider TEXT NOT NULL,
  access TEXT NOT NULL, at INTEGER NOT NULL,
  PRIMARY KEY (issuer, subject, provider));
