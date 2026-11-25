-- Computer, step 10: its disk (docs/computers.md, "A disk"; FIN-744), for
-- a computer made on one: a single row, made as the computer is. `/data`
-- is then an FPAC volume of `volume_bytes`, its objects in the bucket
-- under `disks/<its id's hex>/`. Its head (`fragment_core::computer::fpac`)
-- as typed columns, moved only by the head's rules: the life that posted
-- it (`head_epoch`, null before its first), the checkpoint it opens from
-- (`head_ck_*`, null before its first checkpoint) and whether that life
-- slept cleanly; `back_*`, the head before the newest life posted its
-- first, what a start falls back to past a head that will not mount (null
-- before its second life); `slept_unshipped`, the bytes its last sleep
-- had not shipped as its stop began. A wipe drops it with the rest.
CREATE TABLE disk (
  id INTEGER PRIMARY KEY CHECK (id = 0),
  volume_bytes INTEGER NOT NULL,
  head_epoch INTEGER,
  head_ck_epoch INTEGER,
  head_ck_seq INTEGER,
  head_ck_sum TEXT,
  head_clean INTEGER NOT NULL DEFAULT 0,
  back_epoch INTEGER,
  back_ck_epoch INTEGER,
  back_ck_seq INTEGER,
  back_ck_sum TEXT,
  back_clean INTEGER NOT NULL DEFAULT 0,
  slept_unshipped INTEGER
);
