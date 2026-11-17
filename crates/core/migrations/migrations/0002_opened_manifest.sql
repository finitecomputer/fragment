-- The manifest a stage's complete wrote, kept with its upload for the new
-- agent's open (F7): the object answers it, so no read of STAGE does.
ALTER TABLE uploads ADD COLUMN manifest TEXT;
