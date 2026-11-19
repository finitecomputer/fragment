-- Ledger, step 2: what a person's agents used of each provider, by month
-- (fragment_core::ledger::uses; M16, moved from each computer's `uses`):
-- each agent's calls to each provider a month (`Month`'s index) and what
-- they were charged. The hand-overs it took of a computer's rows from
-- before are kept in `meta` (`uses_taken:<id>`, the body's digest).
CREATE TABLE uses (month INTEGER NOT NULL, agent TEXT NOT NULL, provider TEXT NOT NULL, calls INTEGER NOT NULL, micros INTEGER NOT NULL, PRIMARY KEY (month, agent, provider));
