-- Computer, step 7: what its agents used of each provider is its owner's
-- ledger's (fragment_core::ledger::uses), so the computer's `uses` goes.
-- Each month it holds is kept first, in `meta` `uses_owed:<month>` (the
-- month's index): its rows as the ledger takes them (`TakeUses.rows`, a
-- JSON list of `{agent, provider, calls, micros}`), for the ledger to take
-- once, never lost. A month is at most a computer's cap of rows
-- (`uses::OWED_ROWS_MAX`, about 200 bytes each), so each value stays far
-- under a row's size limit. Nothing held, no row.
INSERT INTO meta (key, value)
SELECT 'uses_owed:' || month, json_group_array(json_object('agent', agent, 'provider', provider, 'calls', calls, 'micros', micros))
FROM uses
GROUP BY month;
DROP TABLE uses;
