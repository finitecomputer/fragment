-- Ledger, step 3: what its person's agents' calls on shared keys cost in
-- the last hour, in five-minute slots (fragment_core::ledger::uses,
-- `SPEND_SLOT_MS`; FIN-616): the hourly key pause's window, kept here for
-- the notice that replaced it. Slots older than the hour are forgotten as
-- each count is made.
CREATE TABLE spend_slots (slot INTEGER PRIMARY KEY, micros INTEGER NOT NULL);
