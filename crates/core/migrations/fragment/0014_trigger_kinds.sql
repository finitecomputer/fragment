-- Fragment, step 14: a channel trigger may start runs only for some kinds
-- of record (FIN-747: a connector's mirror, its agent's questions on
-- `work` and not each step of its turns). What a trigger's `kinds`
-- declares, as installed from live, beside its other declarations, a
-- table of its own as a channel's `newest` is: a JSON array of names,
-- by the trigger's index (cell/src/plane.rs `channel_triggers`).
CREATE TABLE IF NOT EXISTS code_trigger_kinds (idx INTEGER PRIMARY KEY, kinds TEXT NOT NULL);
