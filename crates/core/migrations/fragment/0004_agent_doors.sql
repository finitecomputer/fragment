-- Independent keys and latest link tickets, with the live SimpleX state preserved.
CREATE TABLE agent_doors (
  door TEXT PRIMARY KEY, key TEXT, nonce TEXT, expires_at INTEGER);
INSERT INTO agent_doors (door, key)
SELECT 'simplex', substr(value, 9) FROM meta WHERE key = 'door' AND substr(value, 1, 8) = 'simplex ';
INSERT INTO agent_doors (door, nonce, expires_at)
SELECT 'simplex', substr(value, 9, 32), CAST(substr(value, 42) AS INTEGER)
FROM meta WHERE key = 'door_ticket' AND substr(value, 1, 8) = 'simplex '
ON CONFLICT(door) DO UPDATE SET nonce = excluded.nonce, expires_at = excluded.expires_at;
