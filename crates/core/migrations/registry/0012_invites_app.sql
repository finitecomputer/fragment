-- An invite waiting on an email may ask in an app of its person's
-- (decision 72): the fragment's name of that app. Its person is a member
-- of the fragment already, so its mailed link asks them to accept it
-- before it opens the fragment, as for anyone not yet in. NULL: a
-- person's invite.
ALTER TABLE invites_waiting ADD COLUMN app TEXT;
