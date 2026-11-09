-- Each shared (operator) key's charges this last hour, by five-minute slot
-- (fragment_core::key_hour): its owner's cap on the key is checked against
-- them. Older slots are deleted as new ones are written.
CREATE TABLE key_hour (provider TEXT NOT NULL, slot INTEGER NOT NULL, micros INTEGER NOT NULL, PRIMARY KEY (provider, slot));
