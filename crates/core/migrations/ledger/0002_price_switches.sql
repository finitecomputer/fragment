-- Ledger, step 2: the price switches a ledger took, one row each, by its
-- book's version (`Ledger::switch_prices`): book 9 doubled every charge
-- and every credit (2026-11-04). The row is the record that it took it,
-- so a restart or a replay takes it no more. A ledger at this step is one
-- an older release refuses (crates/core/src/migrate.rs), so none reads
-- its doubled credit in the old amounts, or rewrites its head.
CREATE TABLE switches (version INTEGER PRIMARY KEY, at_ms INTEGER NOT NULL);
