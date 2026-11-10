-- When a person's comp for the move (a month's seat, and its credit) was
-- given (F5): once a person, so a later visit gives none again.
ALTER TABLE people ADD COLUMN comped_ms INTEGER;
