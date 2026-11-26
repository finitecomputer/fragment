// backups.mjs: a computer's backups, checked end to end (FIN-784).
// - A backup is kept only once R2's copy hashes to its record: one damaged
//   on its way out is deleted and backed up again, a bounded number of
//   times.
// - A restore whose bytes came damaged is asked again once R2's copy is not
//   found bad, a bounded number of times, and never condemns a whole copy:
//   a save is unusable only when two reads of R2's copy agree it is not its
//   record.
// - A backup's ranged read that storage failed (a 5xx) is asked again after
//   a growing pause, a bounded number of times; any other answer is the
//   answer at once.
// Valid (whole at once), invalid (damaged, then whole; a copy really bad),
// the bounds (each gives up), and replay (a read damaged on its own way, or
// a second call, condemns nothing).
import { test } from "node:test";
import assert from "node:assert/strict";
import { READ_PAUSE_MS, READ_TRIES, RESTORE_TRIES, SAVE_TRIES, backupChecked, readAgain, restoreChecked } from "./backups.mjs";

const SHA = "a".repeat(64);
const OTHER = "b".repeat(64);
const record = (n) => ({ id: `id-${n}`, dir: "/data", size: 100, sha256: SHA, format: "directory-backup/v1" });

// A save's backups of one directory, whose copies in R2 hold `copies` in
// turn (null: gone; `{size, sha256}` otherwise; "whole": its record's).
function saves(copies) {
  const seen = { backups: 0, read: [], forgot: [], again: [] };
  const run = () =>
    backupChecked({
      backup: async () => record(++seen.backups),
      stored: async (r) => {
        seen.read.push(r.id);
        const copy = copies[Math.min(seen.read.length - 1, copies.length - 1)];
        return copy === "whole" ? { size: r.size, sha256: r.sha256 } : copy;
      },
      forget: async (r) => void seen.forgot.push(r.id),
      again: (tried, why) => seen.again.push([tried, why]),
    });
  return { seen, run };
}

test("a backup whose copy in R2 is its record is kept, read once", async () => {
  const { seen, run } = saves(["whole"]);
  assert.deepEqual(await run(), record(1));
  assert.deepEqual([seen.backups, seen.read, seen.forgot, seen.again], [1, ["id-1"], [], []]);
});

test("a backup that came out damaged is deleted and backed up again, and the whole one kept", async () => {
  const { seen, run } = saves([{ size: 100, sha256: OTHER }, "whole"]);
  assert.deepEqual(await run(), record(2), "the second backup's record, a fresh archive");
  assert.deepEqual([seen.backups, seen.forgot], [2, ["id-1"]]);
  assert.equal(seen.again.length, 1);
  assert.match(seen.again[0][1], /R2's copy holds 100 bytes of SHA-256 b{64}, not its record's 100 bytes of SHA-256 a{64}/);
});

test("a backup whose copy is gone or another size is backed up again too", async () => {
  for (const copy of [null, { size: 99, sha256: SHA }]) {
    const { seen, run } = saves([copy, "whole"]);
    assert.deepEqual(await run(), record(2), JSON.stringify(copy));
    assert.deepEqual(seen.forgot, ["id-1"]);
  }
});

test("a backup that keeps coming out damaged is tried a bounded number of times, each copy deleted, and fails", async () => {
  const { seen, run } = saves([{ size: 100, sha256: OTHER }]);
  await assert.rejects(run(), new RegExp(`the backup of /data came out damaged ${SAVE_TRIES} times: R2's copy holds`));
  assert.ok(SAVE_TRIES >= 3, "damage in transit is tried again at least twice");
  assert.equal(seen.backups, SAVE_TRIES);
  assert.deepEqual(seen.forgot, Array.from({ length: SAVE_TRIES }, (_, i) => `id-${i + 1}`), "no damaged copy is left as a record");
  assert.equal(seen.again.length, SAVE_TRIES - 1);
});

test("a backup that fails is not tried again here, nor its copy read: its save fails as before", async () => {
  let read = 0;
  const failing = backupChecked({
    backup: async () => {
      throw new Error("BACKUP_TRANSFER: a part upload failed");
    },
    stored: async () => void read++,
    forget: async () => {},
  });
  await assert.rejects(failing, /BACKUP_TRANSFER/);
  assert.equal(read, 0);
});

// A start's restores of one record: `tries` answers each restore in turn
// (null: in place; a string: why its bytes came damaged), and `copies` what
// each read of R2's copy holds, as above.
function restores(tries, copies) {
  const seen = { restores: 0, reads: 0, again: [] };
  const r = record(1);
  const run = () =>
    restoreChecked({
      record: r,
      restore: async () => tries[Math.min(seen.restores++, tries.length - 1)],
      stored: async () => {
        const copy = copies[Math.min(seen.reads++, copies.length - 1)];
        return copy === "whole" ? { size: r.size, sha256: r.sha256 } : copy;
      },
      again: (tried, why) => seen.again.push([tried, why]),
    });
  return { seen, run };
}

const DAMAGED = "BACKUP_INTEGRITY: the backup is corrupt: Unknown frame descriptor";

test("a restore that is in place reads nothing more: nothing is added to a wake", async () => {
  const { seen, run } = restores([null], ["whole"]);
  assert.equal(await run(), null);
  assert.deepEqual([seen.restores, seen.reads], [1, 0]);
});

test("a restore whose bytes came damaged, R2's copy whole, is asked again and restored, never unusable", async () => {
  const { seen, run } = restores([DAMAGED, null], ["whole"]);
  assert.equal(await run(), null);
  assert.deepEqual([seen.restores, seen.reads, seen.again], [2, 1, [[1, DAMAGED]]]);
});

test("a save whose copy in R2 is not its record, on two reads that agree, is unusable at once", async () => {
  const { seen, run } = restores([DAMAGED, null], [{ size: 100, sha256: OTHER }]);
  const answer = await run();
  assert.match(answer.unusable, /^BACKUP_INTEGRITY: the backup is corrupt: Unknown frame descriptor; R2's copy holds 100 bytes of SHA-256 b{64}, not its record's/);
  assert.deepEqual([seen.restores, seen.reads], [1, 2], "not asked again: it never will restore");
  const gone = restores([DAMAGED], [null]);
  assert.match((await gone.run()).unusable, /R2's copy is gone/);
});

test("a read of R2's copy damaged on its own way condemns nothing: the restore is asked again", async () => {
  // the first read disagrees with its record, the second with the first
  for (const copies of [[{ size: 100, sha256: OTHER }, "whole"], [{ size: 100, sha256: OTHER }, { size: 100, sha256: "c".repeat(64) }], [{ size: 100, sha256: OTHER }, null], [null, { size: 100, sha256: OTHER }]]) {
    const { seen, run } = restores([DAMAGED, null], copies);
    assert.equal(await run(), null, JSON.stringify(copies));
    assert.deepEqual([seen.restores, seen.reads], [2, 2]);
  }
});

test("a restore that keeps coming in damaged is asked a bounded number of times and fails, its save kept", async () => {
  const { seen, run } = restores([DAMAGED], ["whole"]);
  await assert.rejects(run(), new RegExp(`the restore of /data came in damaged ${RESTORE_TRIES} times, R2's copy not found bad: BACKUP_INTEGRITY`));
  assert.ok(RESTORE_TRIES >= 3, "damage in transit is asked again at least twice");
  assert.equal(seen.restores, RESTORE_TRIES);
  assert.equal(seen.reads, 1, "R2's copy is read once a restore, not once a try");
  // the next start asks afresh, and restores
  const next = restores([null], ["whole"]);
  assert.equal(await next.run(), null);
});

test("a failure that is not damage is thrown at once, untried again and nothing read", async () => {
  let reads = 0;
  const failing = restoreChecked({
    record: record(1),
    restore: async () => {
      throw new Error("BACKUP_TRANSFER: gateway connection failed");
    },
    stored: async () => void reads++,
  });
  await assert.rejects(failing, /BACKUP_TRANSFER/);
  assert.equal(reads, 0);
});

// A read answering `statuses` in turn, counting its calls and the bodies
// cancelled; the pauses it is made to wait, kept.
function reads(statuses) {
  const seen = { calls: 0, cancelled: 0, pauses: [], again: [] };
  const read = async () => {
    const status = statuses[Math.min(seen.calls, statuses.length - 1)];
    seen.calls++;
    const body = new ReadableStream({ cancel: () => void seen.cancelled++ });
    return new Response(status === 204 ? null : body, { status });
  };
  const again = (tried, status) => seen.again.push([tried, status]);
  const sleep = async (ms) => void seen.pauses.push(ms);
  return { seen, run: () => readAgain(read, again, sleep) };
}

test("a read storage failed once is asked again, after a pause", async () => {
  const { seen, run } = reads([502, 206]);
  const answer = await run();
  assert.equal(answer.status, 206);
  assert.deepEqual([seen.calls, seen.cancelled, seen.pauses, seen.again], [2, 1, [READ_PAUSE_MS], [[1, 502]]]);
});

test("an answer that is not storage's failure is the answer at once", async () => {
  for (const status of [206, 403, 404, 411, 416]) {
    const { seen, run } = reads([status]);
    assert.equal((await run()).status, status);
    assert.deepEqual([seen.calls, seen.cancelled, seen.pauses], [1, 0, []], `${status}`);
  }
});

test("a read that keeps failing is asked a bounded number of times, the last answer kept", async () => {
  const { seen, run } = reads([502, 503, 500, 502, 206]);
  const answer = await run();
  assert.equal(answer.status, 502, "the last try's answer, not a later one");
  assert.equal(seen.calls, READ_TRIES);
  assert.equal(seen.cancelled, READ_TRIES - 1, "each answer dropped is cancelled, the last one kept");
  assert.deepEqual(seen.pauses, Array.from({ length: READ_TRIES - 1 }, (_, i) => READ_PAUSE_MS * 2 ** i));
  assert.ok(seen.pauses.reduce((a, b) => a + b, 0) <= 2_000, "its pauses stay well inside a restore's bound");
});
