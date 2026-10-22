-- The Checkouts that hold a person's trial place, a JSON list of their
-- attempt ids (fragment_core::org::hold_taken): a Checkout that never is
-- gives back only its own part, and the place goes once none is left.
-- NULL on rows from before: an attempt that never gives it back.
ALTER TABLE trial_uses ADD COLUMN attempts TEXT;
