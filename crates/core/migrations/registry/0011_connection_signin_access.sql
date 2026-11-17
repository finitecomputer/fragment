-- What a pending connect asks its provider for: the access its person
-- picked, `read` (read only) or `write` (read and write), each its row's
-- scopes (`connection_oauth.access`), which its callback reads the grant
-- against (fragment_core::connection_signin::access_of). A row from before
-- names none: its row's configuration has changed since, so its callback
-- finishes nothing whatever this says, and read only is the lesser.
ALTER TABLE connection_signins ADD COLUMN access TEXT NOT NULL DEFAULT 'read';
