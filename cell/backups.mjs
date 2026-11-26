// A computer's restores, asked again when their bytes came damaged or
// storage failed them (entry.mjs's `ContainerHost` and
// `DirectoryBackupGateway`). Kept apart from entry.mjs so Node can test it.
//
// The Sandbox SDK's shim hashes each archive as it makes it (a record's
// `sha256` is its producer's own); a restore streams it back in, and the
// shim checks its size and SHA-256 before it swaps anything in, reporting
// any mismatch (a corrupt archive to a wrong SHA-256) as BACKUP_INTEGRITY.
// Bytes on their way in can come the right length with the wrong contents
// (FIN-775), so (FIN-784) a restore whose bytes came damaged is asked again
// at once, rather than its save found unusable at the first: damage on the
// way in is rare enough that `RESTORE_TRIES` damaged in a row say the copy
// itself is bad, and the restore then ends as it always did, unusable.

// Tries of one record's restore in all while its bytes come in damaged,
// each moving its whole archive again within the start's own bound
// (computer.rs `RESTORE_MS`).
export const RESTORE_TRIES = 3;

// One record's restore. `restore()` answers null once it is in place, or
// why its bytes came damaged; `again(tried, why)` is told of each asked
// again. Answers null once restored, or `{unusable}` once `RESTORE_TRIES`
// came damaged, or once a try after a damaged one failed otherwise (its
// time ran out, say): as the first damaged one always ended. Throws
// whatever a first try throws but damage.
export async function restoreChecked({ restore, again = () => {} }) {
  let damaged = null;
  for (let tried = 1; ; tried++) {
    let why;
    try {
      why = await restore();
    } catch (e) {
      if (damaged === null) throw e;
      return { unusable: `${damaged}; asked again, it failed: ${e?.message ?? e}` };
    }
    if (why === null) return null;
    damaged = why;
    if (tried >= RESTORE_TRIES) return { unusable: `${why} (${tried} tries, each damaged)` };
    again(tried, why);
  }
}

// A restore's ranged reads, asked again when storage failed them. The
// SDK's gateway answers 502 when R2 throws on a read, before any byte goes
// out, and its shim fails the whole restore on any answer but 206, asking
// nothing again: on production one R2 "internal error, please try again
// (10001)" failed a computer's start (2026-10-30). A read is a GET with no
// body, so asking it again is safe; a part's upload is a body streamed
// once, never asked again (its save fails, and is tried again whole).

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
