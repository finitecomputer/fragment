// A computer's restores, checked end to end, and asked again when their
// bytes came damaged or storage failed them (entry.mjs's `ContainerHost`
// and `DirectoryBackupGateway`). Kept apart from entry.mjs so Node can
// test it.
//
// The Sandbox SDK's shim hashes each archive as it makes it (a record's
// `sha256` is its producer's own) and streams it out through the egress
// intercept in parts R2 checks for length alone; a restore streams it back
// in, and the shim checks its size and SHA-256 before it swaps anything in.
// Bytes on either way can come the right length with the wrong contents
// (FIN-775). So (FIN-784) each record a save takes is read back from R2
// and kept once R2's copy hashes to it (computer.rs `save`,
// fragment_core::computer::after_check), and a restore whose bytes came
// damaged (the SDK's BACKUP_INTEGRITY, a corrupt archive to a wrong
// SHA-256) is asked again unless R2's own copy is found not to hash to its
// record, on two reads that agree on what it holds instead: only then is a
// save unusable. A check that cannot be made condemns nothing. Damage
// before the shim's hash (in the container's own reading of its files) is
// out of reach.

// Tries of one record's restore in all while its bytes come in damaged and
// R2's copy is not found bad: each moves a whole archive again, within the
// start's own bound (computer.rs `RESTORE_MS`).
export const RESTORE_TRIES = 3;

// Whether `held` (what a read of R2's copy found: `{size, sha256}`, or
// null when it is gone) is `record`'s archive.
export function holds(held, record) {
  return held !== null && held.size === record.size && held.sha256 === record.sha256;
}

function against(held, record) {
  const found = held === null ? "is gone" : `holds ${held.size} bytes of SHA-256 ${held.sha256}`;
  return `R2's copy ${found}, not its record's ${record.size} bytes of SHA-256 ${record.sha256}`;
}

// One record's restore. `restore()` answers null once it is in place, or
// why its bytes came damaged; `stored()` reads what R2 holds of `record`;
// `again(tried, why)` is told of each asked again. R2's copy is read beside
// the try after one that came damaged, never before it, so a check never
// takes the time a try needs, and its verdict is awaited only once that try
// comes damaged too, or runs out of time (`Overdue`). Answers null once
// restored, or `{unusable}` when R2's own copy is not its record: the save
// never will restore. Throws once `RESTORE_TRIES` came in damaged while
// R2's copy was not found bad (a failed start, which may pass: the save is
// kept), and whatever `restore` throws.
export async function restoreChecked({ record, restore, stored, again = () => {} }) {
  // a read of R2's copy under way: settles to {bad}, {unread} or {}, never rejects
  let checking = null;
  // R2's copy was read and not found bad
  let whole = false;
  for (let tried = 1; ; tried++) {
    let damaged;
    try {
      damaged = await restore();
    } catch (e) {
      // a try cut off by its time still hears a copy found bad beside it
      const verdict = e?.name === "Overdue" ? await checking : null;
      if (verdict?.bad) return { unusable: `${e.message}; ${verdict.bad}` };
      throw e;
    }
    if (damaged === null) return null;
    if (checking !== null) {
      const verdict = await checking;
      checking = null;
      if (verdict.bad) return { unusable: `${damaged}; ${verdict.bad}` };
      if (verdict.unread) damaged = `${damaged}; R2's copy unread: ${verdict.unread}`;
      else whole = true;
    }
    if (tried >= RESTORE_TRIES) throw new Error(`the restore of ${record.dir} came in damaged ${tried} times, R2's copy not found bad: ${damaged}`);
    if (!whole) {
      checking = badCopy(record, stored).then(
        (bad) => (bad === null ? {} : { bad }),
        (e) => ({ unread: String(e?.message ?? e) }),
      );
    }
    again(tried, damaged);
  }
}

// Why R2's copy of `record` is not it, or null unless two reads agree on
// what it holds instead: a read damaged on its own way never condemns it.
async function badCopy(record, stored) {
  const first = await stored();
  if (holds(first, record)) return null;
  const second = await stored();
  const same = first === null ? second === null : second !== null && second.size === first.size && second.sha256 === first.sha256;
  return same ? against(first, record) : null;
}

// A restore's ranged reads, asked again when storage failed them. The
// SDK's gateway answers 502 when R2 throws on a read, before any byte goes
// out, and its shim fails the whole restore on any answer but 206, asking
// nothing again: on production one R2 "internal error, please try again
// (10001)" failed a computer's start (2026-10-30). A read is a GET with no
// body, so asking it again is safe; a part's upload is a body streamed
// once, never asked again (its backup is, whole: computer.rs `save`).

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
