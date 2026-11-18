-- Fragment, step 13: an app of another member's joins on two yeses
-- (decision 72): this fragment's owner invites it by name, and its owner,
-- a member here, accepts. Its invite waits on its owner's email, as any
-- invite does, naming the app.
ALTER TABLE invites ADD COLUMN app TEXT;
