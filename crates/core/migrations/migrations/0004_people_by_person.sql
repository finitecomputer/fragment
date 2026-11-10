-- Who each person here signs in as (F4): their first notices read binds
-- them by it, once, roster or not, so a person's offers are found by who
-- they are here.
CREATE INDEX IF NOT EXISTS people_person ON people (person);
