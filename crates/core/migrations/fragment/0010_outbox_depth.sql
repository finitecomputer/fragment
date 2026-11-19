-- Fragment, step 10: a record's deliveries carry the hop depth its own
-- triggers run at (channels.rs `published`), so a fragment that follows
-- the channel (subscriptions.rs, a follow) runs its triggers there one
-- chain on, and fragments following one another stop at HOP_DEPTH_MAX.
-- A row written before reads as 0: no follow was made before it.
ALTER TABLE delivery_outbox ADD COLUMN depth INTEGER NOT NULL DEFAULT 0;
