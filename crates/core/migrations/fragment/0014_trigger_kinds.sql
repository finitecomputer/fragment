-- Fragment, step 14: a channel trigger may start runs only for some kinds
-- of record (FIN-747: a connector's mirror, its agent's questions on
-- `work` and not each step of its turns). What a trigger's `kinds`
-- declares, as installed from live, beside its other declarations: a row
-- each, by the trigger's index and its place in the manifest's list, which
-- a record's trigger matches its body's `kind` against (cell/src/plane.rs
-- `channel_triggers`).
CREATE TABLE IF NOT EXISTS code_trigger_kinds (
  idx INTEGER NOT NULL, n INTEGER NOT NULL, kind TEXT NOT NULL, PRIMARY KEY (idx, n), UNIQUE (idx, kind));
