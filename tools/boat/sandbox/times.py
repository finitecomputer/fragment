#!/usr/bin/env python3
"""times.py save|restore: the folder and symlink times a Boat restore loses.

Cargo takes a crate as built when nothing it watches is newer than its last
build. crates/templates/build.rs watches whole directories (templates/,
cell/shell), and cargo's scan of a directory counts the mtime of every
folder and symlink in it, not only of files. A Boat restore brings files
back with their times, but:

- symlinks come back with the time they were made again (never restored);
- folder times come back only on the real disk, which replaces the lazy
  view of /home/user about 3 s after the hydration-done marker (until then
  a folder shows the time it was made again, so start.sh waits
  for the switch), and a folder that anything wrote into during hydration
  keeps its fresh time ("skipped_dirty" in /var/lib/ascii-lazy/daemon.out).

Either makes every fresh copy rebuild fragment-templates and the cell.
`save` (on the golden right before the snapshot, and on a kept sandbox
before it stops) records each folder's and symlink's mtime in the repo, build output and
.git aside; `restore` (start.sh, after hydration) puts back any
that differ. Whole seconds, as Boat restores files: flooring keeps every
"not newer than the build" relation the saved tree had.
"""
import json
import os
import sys
import time

REPO = os.path.expanduser("~/Projects/fragment")
RECORD = os.path.expanduser("~/.boat-keep/fragment-times.json")
SKIP = {".git", "target"}  # build output and git's own files: not sources
NS = 1_000_000_000
LIMIT = 200_000  # entries; the repo has about 1,000
PASSES = 30  # 2 s apart


def entries():
    """(relative path, lstat) of every folder and symlink under REPO."""
    found = []
    for root, dirs, files in os.walk(REPO):
        dirs[:] = [d for d in dirs if d not in SKIP]
        for name in dirs + files:
            path = os.path.join(root, name)
            st = os.lstat(path)
            if os.path.islink(path) or os.path.isdir(path):
                found.append((os.path.relpath(path, REPO), st))
        if len(found) > LIMIT:
            sys.exit(f"fragment-times: more than {LIMIT} folders and links under {REPO}")
    found.append((".", os.lstat(REPO)))
    return found


def save():
    record = {rel: st.st_mtime_ns for rel, st in entries()}
    tmp = RECORD + ".tmp"
    with open(tmp, "w") as f:
        json.dump(record, f, separators=(",", ":"))
    os.replace(tmp, RECORD)
    print(f"fragment-times: saved {len(record)} folder and link times")


def restore_once(record):
    fixed = 0
    for rel, st in entries():
        saved = record.get(rel)
        if saved is None:
            continue  # new since the save: its fresh time is the truth
        want = saved // NS * NS
        if st.st_mtime_ns != want:
            os.utime(os.path.join(REPO, rel), ns=(st.st_atime_ns, want), follow_symlinks=False)
            fixed += 1
    return fixed


def restore():
    try:
        with open(RECORD) as f:
            record = json.load(f)
    except FileNotFoundError:
        print(f"fragment-times: no {RECORD} (a golden saved before it): times left as restored")
        return
    # a second pass, 2 s on, must find nothing: nothing else moves them
    passes = []
    for _ in range(PASSES):
        passes.append(restore_once(record))
        if len(passes) > 1 and passes[-1] == 0:
            break
        time.sleep(2)
    else:
        sys.exit(f"fragment-times: times still moving after {PASSES} passes: {passes}")
    print(f"fragment-times: {len(record)} folder and link times; fixed per pass: {passes}")


if __name__ == "__main__":
    commands = {"save": save, "restore": restore}
    if len(sys.argv) != 2 or sys.argv[1] not in commands:
        sys.exit("usage: times.py save|restore")
    commands[sys.argv[1]]()
