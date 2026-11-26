// backups.mjs: a computer's restores, checked end to end (FIN-784).
// - A restore whose bytes came damaged is asked again, a bounded number of
//   times, and never condemns a whole copy: a save is unusable only when
//   two reads of R2's copy agree it is not its record.
// - R2's copy is read beside the try after a damaged one, never before it:
//   a try never waits for the check, and the check's verdict is awaited
//   only once that try comes damaged too, or runs out of time.
// - A backup's ranged read that storage failed (a 5xx) is asked again after
//   a growing pause, a bounded number of times; any other answer is the
//   answer at once.
// Valid (restored at once), invalid (damaged, then restored; a copy really
// bad), the bounds (each gives up), and replay (a read damaged on its own
// way, a check that fails, a second start).
// (A save's records are checked by the Rust side: fragment_core's
// computer::checks, and the e2e's computer-damage.)
import { test } from "node:test";
import assert from "node:assert/strict";
import { READ_PAUSE_MS, READ_TRIES, RESTORE_TRIES, STORED_TRIES, readAgain, restoreChecked, verdictOf } from "./backups.mjs";

const SHA = "a".repeat(64);
const OTHER = "b".repeat(64);
const r = { id: "id-1", dir: "/data", size: 100, sha256: SHA, format: "directory-backup/v1" };
const whole = { size: r.size, sha256: r.sha256 };
const bad = { size: 100, sha256: OTHER };
const DAMAGED = "BACKUP_INTEGRITY: the backup is corrupt: Data corruption detected";

// A start's restores of `r`: `tries` answers each try in turn, 5 ms after
// it is asked (null: in place; a string: why its bytes came damaged; an
// Error: thrown), and `copies` each read of R2's copy at once (an Error:
// thrown; "never": a read that never answers; `{late: copy}`: that copy,
// 40 ms after it is asked).
function restores(tries, copies, ahead = false) {
  const seen = { restores: 0, reads: 0, again: [] };
  const later = (ms) => new Promise((r) => setTimeout(r, ms));
  const stored = async () => {
    const copy = copies[Math.min(seen.reads++, copies.length - 1)];
    if (copy === "never") return new Promise(() => {});
    if (copy?.late !== undefined) return later(40).then(() => copy.late);
    if (copy instanceof Error) throw copy;
    return copy;
  };
  // a start after a failed one: its verdict begun before any restore of it
  const run = () =>
    restoreChecked({
      record: r,
      ahead: ahead ? verdictOf(r, stored) : null,
      restore: async () => {
        const t = tries[Math.min(seen.restores++, tries.length - 1)];
        await later(5);
        if (t instanceof Error) throw t;
        return t;
      },
      stored,
      again: (tried, why) => seen.again.push([tried, why]),
    });
  return { seen, run };
}

test("a restore that is in place reads nothing more: nothing is added to a wake", async () => {
  const { seen, run } = restores([null], [whole]);
  assert.equal(await run(), null);
  assert.deepEqual([seen.restores, seen.reads], [1, 0]);
});

test("no try waits for R2's copy: a restore whose bytes came damaged is asked again at once, and again, and restored", async () => {
  // its check never answers: neither the second try nor the third is held for it
  const { seen, run } = restores([DAMAGED, DAMAGED, null], ["never"]);
  assert.equal(await run(), null);
  assert.deepEqual([seen.restores, seen.reads, seen.again.map(([t]) => t)], [3, 1, [1, 2]]);
});

test("a save whose copy in R2 is not its record, on two reads that agree, is unusable once a try beside them comes damaged", async () => {
  const { seen, run } = restores([DAMAGED], [bad]);
  const answer = await run();
  assert.match(answer.unusable, /^BACKUP_INTEGRITY: the backup is corrupt: Data corruption detected; R2's copy holds 100 bytes of SHA-256 b{64}, not its record's 100 bytes of SHA-256 a{64}$/);
  assert.deepEqual([seen.restores, seen.reads], [2, 2], "not asked a third time: it never will restore");
  const gone = restores([DAMAGED], [null]);
  assert.match((await gone.run()).unusable, /R2's copy is gone/);
  // its reads slower than the tries: the last try waits for them
  const slow = restores([DAMAGED], [{ late: bad }]);
  assert.match((await slow.run()).unusable, /R2's copy holds 100 bytes/);
  assert.equal(slow.seen.restores, RESTORE_TRIES);
});

test("a copy found bad is heard whatever else a try failed by: its time, or a transfer, the read under way awaited", async () => {
  const overdue = Object.assign(new Error("the restore of /data: no answer within 1 ms"), { name: "Overdue" });
  const late = restores([DAMAGED, overdue], [{ late: bad }]);
  assert.match((await late.run()).unusable, /^the restore of \/data: no answer within 1 ms; R2's copy holds 100 bytes/);
  const transfer = new Error("BACKUP_TRANSFER: gateway connection failed");
  const known = restores([DAMAGED, transfer], [bad]);
  assert.match((await known.run()).unusable, /^BACKUP_TRANSFER: gateway connection failed; R2's copy holds 100 bytes/);
  // its reads still under way as the transfer failed: awaited, so the start goes back at once
  const pending = restores([DAMAGED, transfer], [{ late: bad }]);
  assert.match((await pending.run()).unusable, /^BACKUP_TRANSFER: gateway connection failed; R2's copy holds 100 bytes/);
  // a copy not found bad: the failure as it came
  await assert.rejects(restores([DAMAGED, transfer], [{ late: whole }]).run(), /BACKUP_TRANSFER/);
  await assert.rejects(restores([DAMAGED, overdue], [whole]).run(), /no answer within 1 ms/);
});

test("a read of R2's copy damaged on its own way condemns nothing: the restore is asked again", async () => {
  // the first read disagrees with its record, the second with the first
  for (const copies of [[bad, whole], [bad, { size: 100, sha256: "c".repeat(64) }], [bad, null], [null, bad]]) {
    const { seen, run } = restores([DAMAGED, DAMAGED, null], copies);
    assert.equal(await run(), null, JSON.stringify(copies));
    assert.deepEqual([seen.restores, seen.reads], [3, 2], "read once a start, while the copy was not found bad");
  }
});

test("a read of R2's copy that fails is made again at once, a bounded number of times, the first mismatch kept", async () => {
  const fails = new Error("R2 internal error, please try again (10001)");
  // bad, then a read that fails once, then bad again: unusable beside the second try, no time lost
  const once = restores([DAMAGED], [bad, fails, bad]);
  assert.match((await once.run()).unusable, /R2's copy holds 100 bytes of SHA-256 b{64}/);
  assert.deepEqual([once.seen.restores, once.seen.reads], [2, 3]);
  assert.ok(STORED_TRIES >= 3);
});

test("a check of R2's copy that keeps failing condemns nothing, and is made again beside the next try", async () => {
  const fails = new Error("R2 internal error, please try again (10001)");
  const unread = Array.from({ length: STORED_TRIES }, () => fails);
  const { seen, run } = restores([DAMAGED, DAMAGED, null], [...unread, whole]);
  assert.equal(await run(), null);
  assert.equal(seen.reads, STORED_TRIES + 1);
  assert.match(seen.again[1][1], /^BACKUP_INTEGRITY: .*; R2's copy unread: R2 internal error/);
  // unread beside the second try, read bad beside the third: unusable then
  const later = restores([DAMAGED], [...unread, bad]);
  assert.match((await later.run()).unusable, /R2's copy holds 100 bytes of SHA-256 b{64}/);
  assert.equal(later.seen.restores, 3);
});

test("a start after a failed one reads R2's copy from its restores' start: a copy bad is found though no try is left to read it after", async () => {
  // its first try came damaged late in a long restore, and the next ran out of time
  const overdue = Object.assign(new Error("the restore of /data: no answer within 1 ms"), { name: "Overdue" });
  const late = restores([DAMAGED, overdue], [bad], true);
  assert.match((await late.run()).unusable, /R2's copy holds 100 bytes of SHA-256 b{64}/);
  assert.deepEqual([late.seen.restores, late.seen.reads], [1, 2], "found at its first try");
  // a copy whole: restored at its first try, the read beside it unused, never waited for
  const fine = restores([null], ["never"], true);
  assert.equal(await fine.run(), null);
  assert.deepEqual([fine.seen.restores, fine.seen.reads], [1, 1]);
  // a first start reads nothing beside its first try
  const first = restores([null], [whole]);
  assert.equal(await first.run(), null);
  assert.equal(first.seen.reads, 0);
  // its reads slower than every try (restores before it took the time): the verdict is awaited at the end
  const slow = restores([DAMAGED, overdue], [{ late: bad }], true);
  assert.match((await slow.run()).unusable, /^the restore of \/data: no answer within 1 ms; R2's copy holds 100 bytes/);
});

test("a restore that keeps coming in damaged is asked a bounded number of times and fails, its save kept", async () => {
  const { seen, run } = restores([DAMAGED], [whole]);
  await assert.rejects(run(), new RegExp(`the restore of /data came in damaged ${RESTORE_TRIES} times, R2's copy not found bad: BACKUP_INTEGRITY`));
  assert.ok(RESTORE_TRIES >= 3, "damage in transit is asked again at least twice");
  assert.equal(seen.restores, RESTORE_TRIES);
  assert.equal(seen.reads, 1, "R2's copy is read once a start once found whole, not once a try");
  // the next start asks afresh, and restores
  const next = restores([null], [whole]);
  assert.equal(await next.run(), null);
});

test("a failure that is not damage is thrown at once, untried again and nothing read", async () => {
  const { seen, run } = restores([new Error("BACKUP_TRANSFER: gateway connection failed")], [whole]);
  await assert.rejects(run(), /BACKUP_TRANSFER/);
  assert.deepEqual([seen.restores, seen.reads], [1, 0]);
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
