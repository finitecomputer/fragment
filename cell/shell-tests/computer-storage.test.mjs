// cell/shell/computer.js's Storage (docs/computers.md, "Room to restore"):
// what its owner's view says of its disk, as Settings → Computer → Storage
// draws it: its sizes as its notices say them, each kind's share, Other for
// what the kinds leave out, its biggest files, and nothing before a read.
// Run by `cargo xtask check` on the pinned Node (xtask/src/js_syntax.rs).

import { test } from "node:test";
import assert from "node:assert/strict";
import { storageParts, storageSize } from "../shell/computer.js";

test("sizes as its notices say them: GB to a tenth, under one whole MB, at least 1", () => {
  assert.deepEqual([2_389_566_592, 2_349_999_999, 1_000_000_000, 999_499_999, 400_000, 0].map(storageSize), ["2.4 GB", "2.3 GB", "1.0 GB", "999 MB", "1 MB", "1 MB"]);
});

test("before its first read after a save: nothing to draw", () => {
  assert.equal(storageParts(null), null);
  assert.equal(storageParts({}), null);
  assert.equal(storageParts({ disk: { total: 20e9, free: 9e9, room: 8e9, at: 1 } }), null, "a start's read alone: no /data size yet");
});

test("its kinds, Other for the rest, the room left, its biggest files and when", () => {
  const c = {
    disk: { total: 20_000_002_048, free: 9e9, data: 14e9, room: 2.6e9, filling: true, at: 10 },
    sizes: {
      at: 12,
      folders: [],
      files: [{ path: "work/juniper/downloads/talk.mp4", bytes: 3e9 }, { path: "big.bin", bytes: 2e9 }],
      kinds: [{ kind: "projects", bytes: 8e9 }, { kind: "downloads", bytes: 3e9 }, { kind: "agents", bytes: 1e9 }, { kind: "nonsense", bytes: 5e9 }, { kind: "caches", bytes: 0 }],
    },
  };
  const p = storageParts(c);
  assert.deepEqual(p.kinds.map((k) => [k.kind, k.label, k.bytes]), [["projects", "Projects", 8e9], ["downloads", "Downloads", 3e9], ["agents", "Agents", 1e9], ["other", "Other", 2e9]], "a kind it does not know, or empty, is left out; the rest is Other");
  assert.deepEqual([p.used, p.left, p.filling, p.at], [14e9, 2.6e9, true, 12]);
  assert.deepEqual(p.files, [{ path: "work/juniper/downloads/talk.mp4", name: "talk.mp4", folder: "work/juniper/downloads", bytes: 3e9 }, { path: "big.bin", name: "big.bin", folder: "", bytes: 2e9 }]);
});

test("no summary yet: its size alone, all of it Other; kinds past its size widen it", () => {
  const bare = storageParts({ disk: { total: 20e9, free: 9e9, data: 4e9, room: 12e9, at: 5 } });
  assert.deepEqual([bare.kinds, bare.files, bare.at, bare.filling], [[{ kind: "other", label: "Other", bytes: 4e9 }], [], 5, false]);
  const past = storageParts({ disk: { total: 20e9, free: 9e9, data: 4e9, room: 12e9, at: 5 }, sizes: { at: 6, folders: [], files: [], kinds: [{ kind: "projects", bytes: 5e9 }] } });
  assert.deepEqual([past.used, past.kinds.length], [5e9, 1], "never a negative Other");
  const files = Array.from({ length: 9 }, (_, i) => ({ path: `f${i}`, bytes: 9 - i }));
  assert.equal(storageParts({ disk: { data: 1, room: 1 }, sizes: { at: 1, folders: [], files, kinds: [] } }).files.length, 5, "the five biggest");
});
