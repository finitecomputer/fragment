-- Each shared (operator) key's hourly cap, as its owner set it
-- (fragment_core::key_hour). No row: the default, `KEY_HOUR_CAP`.
CREATE TABLE key_caps (provider TEXT PRIMARY KEY, micros INTEGER NOT NULL);
