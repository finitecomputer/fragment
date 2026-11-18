---
name: disk-space-finite
description: "Your computer's disk holds 20 GB. Use this before a big download, dataset or build, when a write fails with \"No space left on device\", when df shows the disk nearly full, or when your owner says their computer is almost full: make room by moving big files you aren't using into storage (fragment offload), safely, and bring them back when needed."
---

# Room on your disk

Your computer's disk holds 20 GB for everything under `/data`: your work,
your home and their tools. A full disk stops your computer saving your
work, so keep room on it.

## Check

```
df -h /
du -xh --max-depth=3 /data/work 2>/dev/null | sort -h | tail -20
```

Past about 80% used (4 GB or less left), make room before anything big.
Your owner is told then too, and may ask you to.

## Make room, in this order

1. **Delete what tools make again.** Caches and dependencies:
   `pip cache purge`, `uv cache clean`, `npm cache clean --force`, and a
   project's `node_modules/`, `.venv/` or build folder you can build
   again. Nothing is lost.
2. **Move big files you aren't using to storage**: downloads, datasets,
   videos, archives, finished outputs. They stay your owner's and come
   back byte for byte.
3. **Ask your owner** before deleting anything else of theirs.

Never move or delete:

- anything under `/data/hermes`: your memory, skills, sessions and
  settings, which you need to run (`fragment offload` refuses them);
- a browser profile, keys, `.env` or other credential files;
- a file a running program is still writing.

## Storage

One members-only fragment of your owner's per agent holds what you move.
Make it once, named for you; `fragment list` shows its full name later:

```
fragment create <your name>-storage --visibility members
```

Then move a file or a folder, named by its whole path:

```
fragment offload /data/work/downloads/talk.mp4 --into <storage>
fragment offload /data/work/datasets/census --into <storage>
fragment deploy <storage>
```

Deploy once, after your first move: its page then lists what is there,
for your owner too.

`offload` copies each file into storage, reads it back and checks it, and
only then removes it here, leaving a note in its place
(`talk.mp4.offloaded.md`) that says where it went. If it fails, nothing
was removed: read why, fix it, and run it again. Cut off, run it again and
it finishes. A disk with no room at all can't take the note: delete a
cache first (step 1). A folder holding links, or more than 10,000 files,
is refused: move its folders one at a time, or delete it if tools make it
again.

To bring something back, once there is room for it:

```
fragment offload --back /data/work/downloads/talk.mp4.offloaded.md
```

Storage keeps its copy after it comes back. If `fragment offload` is not
a command on this computer, it needs its update: tell your owner to
restart it from Settings.

## Tell your owner

Always tell your owner, in plain words, what you moved and how much room
it made, never a path or a command: "I moved 3 GB of old downloads and
datasets into your storage, so your computer has room again. I can bring
any of them back." Storage is billed to them like any app's files, a few
cents a month per GB.
