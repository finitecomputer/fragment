-- An org's content governance (docs/cloudflare-v1.md, decisions 69 to
-- 71; fragment_core::governance), apart from its billing's org_members:
-- a seat gives none of it, and a seatless person may hold it. A person's
-- row is theirs by their own yes to an admin's place; one who steps down
-- keeps it, an admin no more.
CREATE TABLE org_content (
  org TEXT NOT NULL,
  person TEXT NOT NULL,
  admin INTEGER NOT NULL CHECK (admin IN (0, 1)),
  since INTEGER NOT NULL,
  PRIMARY KEY (org, person)
);
CREATE INDEX org_content_person ON org_content (person);
-- A content admin's place offered to an email, no one's until whoever
-- signs in with it accepts it, on the grounds it was made on (an admin of
-- the org's, who pays for an org whose content has had no one, or an
-- operator's for an org with no admin), which must still hold then. It is
-- answered once, withdrawn, or expires.
CREATE TABLE org_content_offers (
  id TEXT PRIMARY KEY,
  org TEXT NOT NULL,
  email TEXT NOT NULL,
  grounds TEXT NOT NULL CHECK (grounds IN ('admin', 'payer', 'operator')),
  offered_by TEXT NOT NULL,
  offered_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  answer TEXT CHECK (answer IN ('accepted', 'declined', 'withdrawn', 'expired')),
  answered_by TEXT,
  answered_at INTEGER
);
CREATE UNIQUE INDEX org_content_offers_waiting ON org_content_offers (org, email) WHERE answer IS NULL;
CREATE INDEX org_content_offers_email ON org_content_offers (email) WHERE answer IS NULL;
-- What was done to an org's content and by whom, append-only, each entry
-- in the turn of its change under its request id: a client's, prefixed by
-- its kind (`offer:`), or the platform's own (`answer:`, `withdraw:`,
-- `step-down:`, `wipe:`), which no client's takes. A replay finds its
-- first. Its details name offers and people by id, never an email.
CREATE TABLE org_governance (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  org TEXT NOT NULL,
  request TEXT NOT NULL,
  at INTEGER NOT NULL,
  actor TEXT NOT NULL,
  action TEXT NOT NULL,
  subject TEXT,
  detail TEXT NOT NULL,
  UNIQUE (org, request)
);
