// cell/shell/admin-glance.js: the glance's trouble rows in words. A failed
// save whose reason the glance read says it, after a short label when its
// words are of one of a few kinds, and when; a reason of no known kind
// stands alone; another kind of trouble, or a failed save with no reason
// read (an older cell, or its lines past those read), says only its kind.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { troubleWhat } from "../shell/admin-glance.js";

const AT = Date.UTC(2026, 10, 18, 14, 2);
const when = (ms) => new Date(ms).toISOString().slice(0, 16).replace("T", " ");
const failed = (text) => ({ computer: "computer:17c33cdf88c6d1d8f58b7a0f", kind: "saves_failing", lines: 3, npub: null, email: null, why: { at: AT, text } });

test("a failed save's reason of a known kind: its label, when, then the reason as read", () => {
  const cases = [
    ["backup '/data': No space left on device (os error 28)", "disk full"],
    ["the backup of /data: no answer within 727000 ms", "timed out"],
    ["its container is not running", "container gone"],
    ["start 4 is not the running one", "container gone"],
    ["backup '/data': part upload failed with status 500", "upload failed"],
  ];
  for (const [text, label] of cases) {
    assert.deepEqual(troubleWhat(failed(text), when), { what: "a save failed", why: `${label} · 2026-11-18 14:02 · ${text}` });
  }
});

test("a reason of no known kind stands alone, after when", () => {
  assert.deepEqual(troubleWhat(failed("backup '/data': open /data/…: Permission denied (os error 13)"), when), {
    what: "a save failed",
    why: "2026-11-18 14:02 · backup '/data': open /data/…: Permission denied (os error 13)",
  });
});

test("no reason read, or another kind: its kind alone", () => {
  const { why: _, ...unread } = failed("x");
  assert.deepEqual(troubleWhat(unread, when), { what: "a save failed", why: null });
  assert.deepEqual(troubleWhat({ ...unread, kind: "wont_wake" }, when), { what: "won't wake", why: null });
  assert.deepEqual(troubleWhat({ ...unread, kind: "disk_filling" }, when), { what: "disk nearly full", why: null });
  // a kind a newer cell names, as it is
  assert.deepEqual(troubleWhat({ ...unread, kind: "something_new" }, when), { what: "something_new", why: null });
});
