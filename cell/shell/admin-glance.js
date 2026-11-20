// The glance in words (admin.js): how old its logs' numbers are, and its
// trouble rows: what each kind of trouble is, and for a failed save, why,
// as the glance read its newest line (`GET /api/admin/glance`, `why`: the
// platform's words, short, a person's paths left out), with when, and a
// short label when the words are of one of a few kinds.

const TROUBLE = { wont_wake: "won't wake", saves_failing: "a save failed", disk_filling: "disk nearly full" };

// A failed save's kinds, by the words that say each: the disk's own (the
// OS's ENOSPC), the bound on a backup (cell/entry.mjs `Overdue`), its
// container gone (cell/src/computer.rs `hold`, or a start that is no
// longer the running one), an upload to storage (the Sandbox SDK's).
const KINDS = [
  [/no space left|ENOSPC/i, "disk full"],
  [/no answer within/, "timed out"],
  [/container is not running|is not the running one/, "container gone"],
  [/upload/i, "upload failed"],
];

/** What a trouble row says: its kind in words, and for a failed save
 * whose reason was read, that reason after its label and when (`when`
 * formats a time); `why` is null otherwise. */
export function troubleWhat(t, when) {
  const what = TROUBLE[t.kind] || t.kind;
  if (!t.why) return { what, why: null };
  const label = KINDS.find(([words]) => words.test(t.why.text))?.[1];
  return { what, why: [label, when(t.why.at), t.why.text].filter(Boolean).join(" · ") };
}

/** How long before `now` the logs' numbers at `at` were read (both ms,
 * the cell's clock), as the glance says it: "as of 3 minutes ago". */
export function ago(at, now) {
  const minutes = Math.floor(Math.max(0, now - at) / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return minutes === 1 ? "a minute ago" : `${minutes} minutes ago`;
  const hours = Math.floor(minutes / 60);
  return hours === 1 ? "an hour ago" : `${hours} hours ago`;
}
