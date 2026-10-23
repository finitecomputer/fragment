-- An invite waiting on an email waits for its person's yes (docs/api.md,
-- Invites by email): the role it grants, what it called the fragment as
-- it was sent, and whose day its mail counted in (its sharer: a person,
-- an agent's owner), for the list of them its person answers from.
-- NULL on rows from before: invites mailed as "sign in to open it", which
-- a sign-in as their email still meets.
ALTER TABLE invites_waiting ADD COLUMN role TEXT;
ALTER TABLE invites_waiting ADD COLUMN title TEXT;
ALTER TABLE invites_waiting ADD COLUMN sharer TEXT;
