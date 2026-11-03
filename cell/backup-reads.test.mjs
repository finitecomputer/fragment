// backup-reads.mjs: a backup's ranged read that storage failed (a 5xx) is
// asked again after a growing pause, a bounded number of times; any other
// answer is the answer at once. Valid (a failure, then the read), invalid
// (answers never asked again), the bound (it gives up, answering the
// last), and each dropped answer's body cancelled.
import { test } from "node:test";
import assert from "node:assert/strict";
import { READ_PAUSE_MS, READ_TRIES, readAgain } from "./backup-reads.mjs";

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
