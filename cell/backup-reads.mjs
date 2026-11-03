// A backup's ranged reads, asked again when storage failed them (a
// computer's restore: entry.mjs's `DirectoryBackupGateway`). The Sandbox
// SDK's gateway answers 502 when R2 throws on a read, before any byte goes
// out, and its shim fails the whole restore on any answer but 206, asking
// nothing again: on production one R2 "internal error, please try again
// (10001)" failed a computer's start (2026-10-30). A read is a GET with no
// body, so asking it again is safe; a part's upload is a body streamed
// once, never asked again. Kept apart from entry.mjs so Node can test it.

// Tries of one read in all, and the pause before the second (doubled
// before each after it): 1.75 s of pauses at most, well inside a restore's
// own bound (computer.rs, `RESTORE_MS`).
export const READ_TRIES = 4;
export const READ_PAUSE_MS = 250;

// `read()`'s answer, asked again after a pause while storage failed it (a
// 5xx), up to `READ_TRIES` in all; any other answer, or the last, is the
// answer. Each one dropped has its body cancelled. `again(tried, status)`
// is told of each read asked again; `sleep(ms)` waits.
export async function readAgain(read, again = () => {}, sleep = (ms) => new Promise((r) => setTimeout(r, ms))) {
  for (let tried = 1; ; tried++) {
    const answer = await read();
    if (answer.status < 500 || tried >= READ_TRIES) return answer;
    await answer.body?.cancel();
    again(tried, answer.status);
    await sleep(READ_PAUSE_MS * 2 ** (tried - 1));
  }
}
