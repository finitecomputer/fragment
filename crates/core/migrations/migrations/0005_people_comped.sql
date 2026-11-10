-- The move's comp for a person (F5): when its seat ends, fixed at the
-- first attempt so a retry never asks for a later end; and when it was
-- given, after which no visit gives it again.
ALTER TABLE people ADD COLUMN comp_until_ms INTEGER;
ALTER TABLE people ADD COLUMN comped_ms INTEGER;
