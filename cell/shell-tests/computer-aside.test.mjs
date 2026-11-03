// cell/shell/computer.js's `setAsideSaid`: what the person is told of a
// folder a start found no room for (the `set_aside` notice), in plain words:
// which folder, the day it was saved, its size, and that it is kept. Every
// folder set aside is told, oldest first; one it does not know is a folder.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { ASIDE_TITLE, setAsideSaid } from "../shell/computer.js";

const at = Date.UTC(2026, 10, 3, 12);
const day = new Date(at).toLocaleDateString([], { month: "short", day: "numeric" });

test("a work folder that didn't fit: its day, its size, and that it is kept", () => {
  assert.equal(
    setAsideSaid([{ record: "r1", dir: "/data/work", bytes: 5_887_397_516, save: 594, savedAt: at, at: at + 1 }]),
    `Your work folder from ${day} (5.9 GB) didn't fit, so your computer started without it. It's kept safe.`,
  );
  assert.equal(ASIDE_TITLE, "A folder didn't fit on your computer");
});

test("each folder set aside is told, oldest first; small ones in MB, an unknown one as a folder", () => {
  const said = setAsideSaid([
    { record: "r1", dir: "/data", bytes: 420_000_000, save: 1, savedAt: at, at },
    { record: "r2", dir: "/data/work", bytes: 12_000, save: 2, savedAt: at, at },
    { record: "r3", dir: "/elsewhere", bytes: 2_500_000_000, save: 3, savedAt: at, at },
  ]);
  assert.equal(
    said,
    `Your computer's own files from ${day} (420 MB) didn't fit, so your computer started without them. They're kept safe. ` +
      `Your work folder from ${day} (1 MB) didn't fit, so your computer started without it. It's kept safe. ` +
      `Your folder from ${day} (2.5 GB) didn't fit, so your computer started without it. It's kept safe.`,
  );
});
