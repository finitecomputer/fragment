-- An org's content governance (docs/cloudflare-v1.md, decisions 69 to
-- 71; fragment_core::governance), apart from its billing's org_members:
-- a seat gives none of it, and a seatless person may hold it. A person's
-- place is in by their own yes; an offboarding raises its epoch, makes it
-- inactive and marks when (`left_at`), so what was granted under the old
-- epoch, or offered before it, opens nothing.
CREATE TABLE org_content (
  org TEXT NOT NULL,
  person TEXT NOT NULL,
  epoch INTEGER NOT NULL CHECK (epoch >= 1),
  active INTEGER NOT NULL CHECK (active IN (0, 1)),
  admin INTEGER NOT NULL CHECK (admin IN (0, 1) AND (admin = 0 OR active = 1)),
  since INTEGER NOT NULL,
  left_at INTEGER,
  PRIMARY KEY (org, person)
);
CREATE INDEX org_content_person ON org_content (person);
-- A content admin's place offered to an email, no one's until whoever
-- signs in with it answers it, once; `request` is its offerer's id for
-- it, so their replay finds it.
CREATE TABLE org_content_offers (
  id TEXT PRIMARY KEY,
  org TEXT NOT NULL,
  email TEXT NOT NULL,
  request TEXT NOT NULL,
  offered_by TEXT NOT NULL,
  offered_at INTEGER NOT NULL,
  answer TEXT CHECK (answer IN ('accepted', 'declined')),
  answered_by TEXT,
  answered_at INTEGER,
  UNIQUE (org, request)
);
CREATE UNIQUE INDEX org_content_offers_waiting ON org_content_offers (org, email) WHERE answer IS NULL;
CREATE INDEX org_content_offers_email ON org_content_offers (email) WHERE answer IS NULL;
-- What was done to an org's content, by whom, append-only: every offer,
-- answer, removal and offboarding, under its request id (a replay finds
-- its first). A person's wipe keeps the entries that name them.
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
