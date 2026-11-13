-- The services a pending connect picked (its row's `connection_oauth.
-- services`), a JSON list of their ids, whose scopes its callback reads the
-- grant against (fragment_core::connection_signin::scopes_asked). NULL:
-- every one, as a connect of a row of no services, or of before, asks.
ALTER TABLE connection_signins ADD COLUMN services TEXT;
