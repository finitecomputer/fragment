-- Registry, step 9: an org's content takes in the people its fragments
-- admit, not its admins alone (docs/cloudflare-v1.md, decisions 69 to 71;
-- fragment_core::governance), each at an epoch an offboarding raises,
-- active until then, and when they last left (`left_at`): what an org's
-- fragment granted them under an older epoch, or invited them to before
-- they left, opens nothing. The table is made again with them; a row from
-- before is an admin's (or one who stepped down), in at 1.
CREATE TABLE org_content_epochs (
  org TEXT NOT NULL,
  person TEXT NOT NULL,
  epoch INTEGER NOT NULL CHECK (epoch >= 1),
  active INTEGER NOT NULL CHECK (active IN (0, 1)),
  admin INTEGER NOT NULL CHECK (admin IN (0, 1) AND (admin = 0 OR active = 1)),
  since INTEGER NOT NULL,
  left_at INTEGER,
  PRIMARY KEY (org, person)
);
INSERT INTO org_content_epochs (org, person, epoch, active, admin, since, left_at) SELECT org, person, 1, 1, admin, since, NULL FROM org_content;
DROP TABLE org_content;
ALTER TABLE org_content_epochs RENAME TO org_content;
CREATE INDEX org_content_person ON org_content (person);
