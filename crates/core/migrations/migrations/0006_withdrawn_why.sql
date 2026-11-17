-- The will-not-move list (staff's withdrawals): why staff withdrew a move,
-- and whether the last roster dropped it, so taking a withdrawal back
-- (`include`) leaves the roster's own withdrawn. A row the roster
-- withdrew before this step was dropped by it.
ALTER TABLE moves ADD COLUMN why TEXT;
ALTER TABLE moves ADD COLUMN dropped INTEGER NOT NULL DEFAULT 0;
UPDATE moves SET dropped = 1 WHERE state = 'withdrawn' AND by_staff = 0;
