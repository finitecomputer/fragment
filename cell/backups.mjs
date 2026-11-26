// A computer's backups (its saves' records), checked end to end, and
// moved again when their bytes came damaged or storage failed them
// (entry.mjs's `ContainerHost` and `DirectoryBackupGateway`). Kept apart
// from entry.mjs so Node can test it.
//
// The Sandbox SDK's shim hashes each archive as it makes it (a record's
// `sha256` is its producer's own) and streams it out through the egress
// intercept in parts R2 checks for length alone; a restore streams it back
// in, and the shim checks its size and SHA-256 before it swaps anything in.
// Bytes on either way can come the right length with the wrong contents
// (FIN-775). So (FIN-784):
// - a backup is kept once R2's own copy hashes to its record: one damaged
//   on its way out is deleted and backed up again. A check that cannot be
//   made (storage failed its read, or the save's time ran out) keeps the
//   record unchecked, as before it: a save that fits its bounds is never
//   failed for want of its check, so no computer sleeps unsaved for it;
// - a restore whose bytes came damaged (the SDK's BACKUP_INTEGRITY, a
//   corrupt archive to a wrong SHA-256) is asked again unless R2's own copy
//   is found not to hash to its record, on two reads that agree on what it
//   holds instead: only then is a save unusable. A check that cannot be
//   made condemns nothing.
// Damage before the shim's hash (in the container's own reading of its
// files) is out of their reach.

// Tries of one directory's backup in all while R2's copy of it comes out
// damaged, and of one record's restore while its bytes come in damaged and
// R2's copy is whole: each moves a whole archive again, within the save's
// or the start's own bound (computer.rs `BACKUPS_MS`, `RESTORE_MS`).
export const SAVE_TRIES = 3;
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

// One directory's backup, kept once R2's copy is its record's. `backup()`
// makes a fresh archive and answers its record; `stored(record)` reads
// what R2 holds of it; `forget(record)` deletes a copy that is not it;
// `again(tried, why)` is told of each backed up again, `unchecked(record,
// why)` of one kept unread. Throws once `SAVE_TRIES` came out damaged (the
// save fails, and its lifecycle tries it again), and whatever `backup`
// throws.
export async function backupChecked({ backup, stored, forget, again = () => {}, unchecked = () => {} }) {
  for (let tried = 1; ; tried++) {
    const record = await backup();
    let held;
    try {
      held = await stored(record);
    } catch (e) {
      unchecked(record, String(e?.message ?? e));
      return record;
    }
    if (holds(held, record)) return record;
    const why = against(held, record);
    await forget(record);
    if (tried >= SAVE_TRIES) throw new Error(`the backup of ${record.dir} came out damaged ${tried} times: ${why}`);
    again(tried, why);
  }
}

// One record's restore. `restore()` answers null once it is in place, or
// why its bytes came damaged; `stored()` reads what R2 holds of `record`;
// `again(tried, why)` is told of each asked again. Answers null once
// restored, or `{unusable}` when R2's own copy is not its record: the save
// never will restore. R2's copy is read after the first try that came
// damaged, and again after the next while no read was made. Throws once
// `RESTORE_TRIES` came in damaged while R2's copy was not found bad (a
// failed start, which may pass: the save is kept), and whatever `restore`
// throws.
export async function restoreChecked({ record, restore, stored, again = () => {} }) {
  let read = false;
  for (let tried = 1; ; tried++) {
    let damaged = await restore();
    if (damaged === null) return null;
    if (!read) {
      try {
        const bad = await badCopy(record, stored);
        if (bad !== null) return { unusable: `${damaged}; ${bad}` };
        read = true;
      } catch (e) {
        damaged = `${damaged}; R2's copy unread: ${e?.message ?? e}`;
      }
    }
    if (tried >= RESTORE_TRIES) throw new Error(`the restore of ${record.dir} came in damaged ${tried} times, R2's copy not found bad: ${damaged}`);
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
// once, never asked again (its backup is, whole, above).

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
