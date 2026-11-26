// backups.mjs: a computer's restores (FIN-784).
// - A restore whose bytes came damaged is asked again at once, a bounded
//   number of times; one that comes in whole is restored, its save kept.
// - `RESTORE_TRIES` damaged in a row, or a try after a damaged one that
//   fails otherwise, end as a damaged restore always did: unusable.
// - A backup's ranged read that storage failed (a 5xx) is asked again after
//   a growing pause, a bounded number of times; any other answer is the
//   answer at once.
// Valid (restored at once, or at a later try), invalid (damaged each time),
// the bounds, and replay (a second start asks afresh).
import { test } from "node:test";
import assert from "node:assert/strict";
import { READ_PAUSE_MS, READ_TRIES, RESTORE_TRIES, readAgain, restoreChecked } from "./backups.mjs";

const DAMAGED = "BACKUP_INTEGRITY: the backup is corrupt: Data corruption detected";

// A start's restores of one record: `tries` answers each try in turn (null:
// in place; a string: why its bytes came damaged; an Error: thrown).
function restores(tries) {
  const seen = { restores: 0, again: [] };
  const run = () =>
    restoreChecked({
      restore: async () => {
        const t = tries[Math.min(seen.restores++, tries.length - 1)];
        if (t instanceof Error) throw t;
        return t;
      },
      again: (tried, why) => seen.again.push([tried, why]),
    });
  return { seen, run };
}

test("a restore that is in place is the restore, asked once", async () => {
  const { seen, run } = restores([null]);
  assert.equal(await run(), null);
  assert.deepEqual([seen.restores, seen.again], [1, []]);
});

test("a restore whose bytes came damaged is asked again at once and restored, its save kept", async () => {
  const { seen, run } = restores([DAMAGED, null]);
  assert.equal(await run(), null, "never unusable");
  assert.deepEqual([seen.restores, seen.again], [2, [[1, DAMAGED]]]);
  const twice = restores([DAMAGED, DAMAGED, null]);
  assert.equal(await twice.run(), null);
  assert.equal(twice.seen.restores, 3);
});

test("a restore that comes in damaged each time is asked a bounded number of times, then unusable, as it always ended", async () => {
  assert.ok(RESTORE_TRIES >= 3, "damage on the way in is asked again twice at least");
  const { seen, run } = restores([DAMAGED]);
  assert.deepEqual(await run(), { unusable: `${DAMAGED} (${RESTORE_TRIES} tries, each damaged)` });
  assert.equal(seen.restores, RESTORE_TRIES);
  assert.equal(seen.again.length, RESTORE_TRIES - 1);
  // the next start asks afresh
  const next = restores([null]);
  assert.equal(await next.run(), null);
});

test("a try after a damaged one that fails otherwise ends unusable, as the damaged one would have", async () => {
  const overdue = Object.assign(new Error("the restore of /data: no answer within 1 ms"), { name: "Overdue" });
  for (const e of [overdue, new Error("BACKUP_TRANSFER: gateway connection failed")]) {
    const { seen, run } = restores([DAMAGED, e]);
    assert.deepEqual(await run(), { unusable: `${DAMAGED}; asked again, it failed: ${e.message}` });
    assert.equal(seen.restores, 2);
  }
});

test("a first try that fails otherwise is thrown, as before: nothing asked again", async () => {
  const { seen, run } = restores([new Error("BACKUP_TRANSFER: gateway connection failed")]);
  await assert.rejects(run(), /BACKUP_TRANSFER/);
  assert.equal(seen.restores, 1);
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
