-- Computer, step 6: an agent's grants are its agent fragment's (step 5,
-- fragment_core::grants), so the computer's old rows go: `agents.connections`
-- and `agent_accounts`. A row its fragment has not taken yet (a computer
-- asleep since step 5, or never asked) is kept first, in `meta`
-- `grants_owed`: a JSON object, agent fragment → `{identity, connections,
-- accounts}`, for that fragment to take at its next ask, never lost. A row
-- taken (`["-"]`) is not kept, nor one that says only the default (every
-- provider, no account picked), which a fragment holding none answers
-- anyway. Nothing owed, no row.
INSERT INTO meta (key, value)
SELECT 'grants_owed', json_group_object(a.fragment, json_object('identity', a.identity, 'connections', a.connections, 'accounts', json((SELECT json_group_object(provider, account) FROM agent_accounts WHERE agent = a.fragment))))
FROM agents a
WHERE a.connections != '["-"]' AND (a.connections != 'null' OR EXISTS (SELECT 1 FROM agent_accounts WHERE agent = a.fragment));
DELETE FROM meta WHERE key = 'grants_owed' AND value = '{}';
DROP TABLE agent_accounts;
ALTER TABLE agents DROP COLUMN connections;
