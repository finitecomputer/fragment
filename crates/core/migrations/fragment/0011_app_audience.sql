-- Fragment, step 11: the audience a fragment approved for each app member
-- (decision 72): each person, with what they do to the app (`viewer` sees
-- it, `editor` edits it), as the app answered at its join. An app this
-- fragment holds short of anyone on its list (a removal, a lowered role)
-- is paused, naming that person (`member_apps.paused`), until it is not.
CREATE TABLE IF NOT EXISTS app_audience (app TEXT NOT NULL, person TEXT NOT NULL, power TEXT NOT NULL, PRIMARY KEY (app, person));
ALTER TABLE member_apps ADD COLUMN paused TEXT;
