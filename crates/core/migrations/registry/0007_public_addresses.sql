-- A public origin belongs to one app identity forever, even after it
-- ends. No person or credential is stored here. One address per app.
CREATE TABLE public_addresses (
  label TEXT PRIMARY KEY,
  fragment TEXT NOT NULL,
  npub TEXT NOT NULL UNIQUE
);
