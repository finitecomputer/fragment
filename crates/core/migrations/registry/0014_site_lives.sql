-- Each name's lives as the registry counts them (crates/core/src/
-- site_signin.rs): a create counts its name's next life in the turn
-- that forgets the name's sign-in rows. A member's mint holds only for
-- the life it was asked about, and a delete forgets only its own life's
-- rows. One row a name a create has begun; nothing of anyone.
CREATE TABLE site_lives (
  fragment TEXT PRIMARY KEY,
  life INTEGER NOT NULL
);
