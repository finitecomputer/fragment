-- Fragment, step 16: the ask an agent's words were said on (FIN-615, PR
-- 16a; docs/api.md, "Who asked: mandates"): the mandate its post carried,
-- as it came, so an agent of the same owner's that the post asks here
-- carries that ask on (`fragment ask`). A record from before has none: an
-- ask in it goes on unproven, as before.
ALTER TABLE records ADD COLUMN carried TEXT;
