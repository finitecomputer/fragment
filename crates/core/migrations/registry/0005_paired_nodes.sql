-- People's own sandcastle nodes (bring your own computer, experimental:
-- docs/self-host.md, seam 2; cell/src/registry/nodes.rs keeps the rows,
-- fragment_core::pairing decides). `pairings`: a node waiting for its
-- person, by its user code, its device code only as its SHA-256.
-- `paired_nodes`: each person's nodes, their secrets sealed for the node's
-- id (`PairedNode:<id>`); a revoked node keeps its row and loses its
-- secret. `pair_misses`: the wrong codes a person tried. `node_prefs`:
-- the node a person chose for their new computers.
CREATE TABLE pairings (
  user_code TEXT PRIMARY KEY, device_hash TEXT NOT NULL UNIQUE, name TEXT NOT NULL, arch TEXT NOT NULL,
  created_at INTEGER NOT NULL, expires_at INTEGER NOT NULL, interval_s INTEGER NOT NULL, polled_at INTEGER, node TEXT);
CREATE INDEX pairings_created ON pairings (created_at);
CREATE TABLE paired_nodes (
  id TEXT PRIMARY KEY, owner TEXT NOT NULL, name TEXT NOT NULL, arch TEXT NOT NULL, paired_at INTEGER NOT NULL,
  sealed TEXT, revoked_at INTEGER);
CREATE INDEX paired_nodes_owner ON paired_nodes (owner);
CREATE TABLE pair_misses (identity TEXT NOT NULL, at INTEGER NOT NULL);
CREATE INDEX pair_misses_identity ON pair_misses (identity, at);
CREATE TABLE node_prefs (owner TEXT PRIMARY KEY, node TEXT NOT NULL);
