# A team brain from the old Finite, brought together one move at a time
# (finite-computer.md, "Record it" and "Once you are in the real one").
#
#   start <old id> <name file> <brain folder> <staged folder>
#     records, in the brain's imported-from-finite.json, which folders it
#     shares and which version of each of their pages it holds.
#   copied <old id> <copy's name file> <staged folder>
#     marks in a reference copy's name file which pages were staged as
#     their revision's text, before its asset notes are rewritten.
#   catch-up <old id> <copy's name file> <brain folder> <copy folder>
#     brings in what the copy holds newer, in those folders only, and
#     prints what it leaves for a person.
#
# The catch-up decides each page from three versions: the record's (the
# base: what the brain took last), the copy's, and the brain's now. Changed
# only in the old brain: taken. Changed only here: kept. Changed on both:
# listed. Deleted in the old brain: listed, never deleted. "Changed in the
# old brain" is by its own revisions (the name file's `versions`, FIN-773),
# never two copies' bytes: an import rewrites asset notes.
#
# The base is a file of the brain's because an agent can't read the
# brain's history yet: `fragment` reads `main` as it is now, never a file
# at an earlier commit nor `main`'s commit messages (FIN-780). Once it
# can, the import's own commit is the base, and the file goes (technical
# debt ledger, "A team brain's catch-up keeps its base in a file").
import hashlib, json, os, shutil, sys

RECORD = "imported-from-finite.json"
MISSING = object()


def stop(why):
    sys.exit(f"STOP: {why}")


def need(ok):
    if not ok:
        raise ValueError


def clean(path):
    # A relative path of `/`-separated names, none empty, `.` or `..`.
    return (
        isinstance(path, str)
        and 0 < len(path.encode()) <= 4096
        and not any(c < " " or c in "\x7f\\" for c in path)
        and all(name not in ("", ".", "..") for name in path.split("/"))
    )


def at(root, path):
    # `root`/`path`, or None when a part of it is a link.
    full = os.path.join(root, *path.split("/"))
    real = os.path.join(os.path.realpath(root), *path.split("/"))
    return full if os.path.realpath(full) == real else None


def sha(root, path):
    # A plain file's SHA-256, or None.
    full = at(root, path)
    try:
        if full is None or not os.path.isfile(full):
            return None
        with open(full, "rb") as f:
            return hashlib.sha256(f.read()).hexdigest()
    except OSError:
        return None


def blocked(root, path):
    # A link there, or something that isn't a plain file (a folder, say).
    full = at(root, path)
    return full is None or (os.path.lexists(full) and not os.path.isfile(full))


def put(src_root, dst_root, path):
    # The copy's file onto the brain's, never through a link: whether it went.
    src, dst = at(src_root, path), at(dst_root, path)
    if sha(src_root, path) is None or dst is None or blocked(dst_root, path):
        return False
    try:
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        shutil.copyfile(src, dst)
        return True
    except OSError:
        return False  # a file where its folder goes, say


def assets(root, path):
    # The `resource: raw/assets/<file>` a note's frontmatter names, by its
    # path in the brain (under the note's wiki, its top folder).
    full = at(root, path)
    try:
        with open(full, errors="replace") as f:
            lines = f.read(1 << 20).split("\n")
    except (OSError, TypeError):
        return []
    if lines[0].strip() != "---":
        return []
    found = []
    for line in lines[1:]:
        if line.strip() == "---":
            break
        if line.startswith("resource:"):
            name = line[len("resource:"):].strip().strip("\"'")
            if name.startswith("raw/assets/") and clean(name):
                found.append(path.split("/", 1)[0] + "/" + name)
    return found


def load(path, what):
    # A plain file's JSON, never one through a link; MISSING when absent.
    if not os.path.lexists(path):
        return MISSING
    if os.path.islink(path) or not os.path.isfile(path):
        stop(f"{what} isn't a plain file: {path}")
    try:
        with open(path) as f:
            return json.load(f)
    except (OSError, ValueError):
        stop(f"{what} can't be read as JSON: {path}")


def save(path, value):
    # Written whole beside it under a dot name (which never syncs), then
    # moved over it: a link there is replaced, never followed.
    part = os.path.join(os.path.dirname(path), "." + os.path.basename(path) + ".partial")
    if os.path.lexists(part):
        os.unlink(part)
    with os.fdopen(os.open(part, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o644), "w") as f:
        json.dump(value, f, indent=1, sort_keys=True)
    os.replace(part, path)


def ident(value):
    # Who a page or folder is in the old brain: its brain (null for the
    # tree's own) and its id there. A path can change hands; this can't.
    need(isinstance(value, list) and len(value) == 2)
    need((value[0] is None or isinstance(value[0], str)) and isinstance(value[1], str))
    return value


def count(n):
    need(type(n) is int and n >= 0)
    return n


def folder_of(path, known):
    # The deepest of `known` that `path` is in (a child folder is under its parent).
    return max((f for f in known if path.startswith(f + "/")), key=len, default=None)


usage = "usage: start|catch-up <old id> <name file> <brain folder> <folder> | copied <old id> <name file> <staged folder>"
if len(sys.argv) < 2 or (sys.argv[1], len(sys.argv)) not in (("start", 6), ("catch-up", 6), ("copied", 5)):
    sys.exit(usage)
mode, old, name_file = sys.argv[1:4]
named = load(name_file, "the name file")
try:
    v = named["versions"]
    sequence = count(v["sequence"])
    # path -> (identity, read)
    folders = {f["path"]: (ident([f["brain"], f["id"]]), f["read"] is True) for f in v["folders"]}
    # path -> (identity, revision, sha256)
    listed = {p["path"]: (ident([p["brain"], p["object"]]), count(p["revision"]), p["sha256"]) for p in v["pages"]}
    need(all(clean(f) for f in folders) and all(isinstance(p, str) for p in listed))
except (TypeError, KeyError, ValueError, AttributeError):
    sys.exit("NO VERSIONS: this copy has no page versions; compare by hand")
unread = sorted(f for f, (_, read) in folders.items() if not read)

if mode == "copied":
    staged = sys.argv[4]
    named["exact"] = sorted(p for p, (_, _, h) in listed.items() if clean(p) and sha(staged, p) == h)
    save(name_file, named)
    print(json.dumps({"pages": len(listed), "exact": len(named["exact"]), "unread": unread}))
    sys.exit(0)

work, other = sys.argv[4:6]
record_path = os.path.join(work, RECORD)
record = load(record_path, RECORD)
if record is not MISSING:
    try:
        need(isinstance(record, dict) and record["from"] == "finite.computer" and isinstance(record["old"], str))
        for f, seen in record["folders"].items():
            need(clean(f)), ident(seen["id"]), count(seen["sequence"])
        for p, seen in record["pages"].items():
            need(clean(p) and (seen["written"] is None or isinstance(seen["written"], str)))
            ident(seen["id"]), count(seen["revision"])
    except (TypeError, KeyError, ValueError, AttributeError):
        stop(f"{RECORD} is not one this file wrote: change nothing")
    if record["old"] != old:
        stop(f"{RECORD} is another brain's ({record['old']}), not {old}")

if mode == "start":
    # From a reference copy, `copied` checked its staged pages already.
    staged, exact = other, named.get("exact")
    pages = {}
    for path, (id_, revision, old_sha) in listed.items():
        here = sha(work, path) if clean(path) else None
        if here is not None:
            # Staged as other text than its revision's: an edit the old brain
            # never got. Nothing is recorded as written, so any later change
            # to it in the old brain is for a person.
            exact_text = path in exact if isinstance(exact, list) else sha(staged, path) == old_sha
            pages[path] = {"id": id_, "revision": revision, "written": here if exact_text else None}
    # The folders it shares: each the old agent could read, with a page
    # here (or, with no pages, a folder here). One left out stays out.
    held = {folder_of(p, folders) for p in pages}
    empty = set(folders) - {folder_of(p, folders) for p in listed}
    shared = {
        f: {"id": id_, "sequence": sequence}
        for f, (id_, read) in folders.items()
        if read and (f in held or (f in empty and os.path.isdir(at(work, f) or "")))
    }
    new = {"from": "finite.computer", "old": old, "folders": shared, "pages": pages}
    if record is not MISSING and record != new:
        stop(f"{RECORD} is there already, and isn't this brain's start: push nothing")
    save(record_path, new)
    print(json.dumps({"recorded": len(pages), "shared": sorted(shared), "not shared": sorted(set(folders) - set(shared))}))
    sys.exit(0)

if record is MISSING:
    sys.exit(f"NO RECORD: no {RECORD} here; compare by hand")
exact = named.get("exact")
if not isinstance(exact, list):
    sys.exit("NO VERSIONS: this copy was kept without its pages checked (`copied`); compare by hand")
exact = {p for p in exact if isinstance(p, str)}
copy_dir, shared = other, record["folders"]
known = set(folders) | set(shared)


def ours(folder):
    # Shared, the same folder in this copy (not another at its path), and
    # one this copy could read.
    return folder in shared and folder in folders and folders[folder] == (shared[folder]["id"], True)


def older(folder):
    # The brain took this folder from a copy newer than this one.
    return sequence < shared[folder]["sequence"]


def asset_plan(names):
    # The assets to bring with a note, or None when one can't come or
    # differs here: none is ever written over.
    plan = []
    for asset in names:
        there, here = sha(copy_dir, asset), sha(work, asset)
        if blocked(copy_dir, asset) or blocked(work, asset):
            return None  # a link or a folder there
        if there is None and here is None or None not in (there, here) and there != here:
            return None  # in neither, or different here
        if here is None:
            if not ours(folder_of(asset, known)):
                return None  # it would come into a folder this copy doesn't bring in
            plan.append(asset)
    return plan


took = {"added": 0, "updated": 0, "assets": 0}
held = {k: [] for k in ("both", "gone", "unsynced", "missing", "older", "extra", "refused")}
unshared, take = set(), []
for path, (id_, revision, _) in sorted(listed.items()):
    if not clean(path) or at(copy_dir, path) is None or at(work, path) is None:
        held["refused"].append(path)  # climbs out, or through a link
        continue
    folder = folder_of(path, known)
    if not ours(folder):
        unshared.add(folder or "")
        continue
    there = sha(copy_dir, path)
    if there is None:
        held["missing"].append(path)  # listed, and not in this copy
        continue
    seen, here = record["pages"].get(path), sha(work, path)
    base = seen if seen is not None and seen["id"] == id_ else None
    plan = asset_plan(assets(copy_dir, path))
    entry = {"id": id_, "revision": revision, "written": there}
    if here == there:
        # Here already, byte for byte: settled once its assets are. Recorded
        # when the record lost it, or holds an older version.
        heal = path in exact and (base["revision"] < revision if base else seen is None or not older(folder))
        if plan is None:
            held["both"].append(path)
        else:
            take.append(("same", path, entry if heal else None, plan))
        continue
    if path not in exact:
        held["unsynced"].append(path)  # an edit the old brain never got
        continue
    if base is not None and revision <= base["revision"]:
        continue  # not changed in the old brain since: what is here stays
    if older(folder):
        held["older"].append(path)
        continue
    if blocked(work, path):
        held["both"].append(path)  # a folder, say, where the page goes
        continue
    if seen is None and here is None:
        kind = "added"
    elif base is not None and here is not None and here == base["written"]:
        kind = "updated"  # changed in the old brain, and not here
    else:
        held["both"].append(path)  # changed or deleted here, made here, or another page
        continue
    if plan is None:
        held["both"].append(path)  # its asset differs here, or can't come
        continue
    take.append((kind, path, entry, plan))

for kind, path, entry, plan in take:
    brought = [a for a in plan if sha(work, a) is None]
    if not all(put(copy_dir, work, a) for a in brought) or (kind != "same" and not put(copy_dir, work, path)):
        held["both"].append(path)  # something here in the way
        continue
    took["assets"] += len(brought)
    if kind != "same":
        took[kind] += 1
    if entry is not None:
        record["pages"][path] = entry

# Files the copy holds that no listed page is: kept for a person, unless
# an asset the brain holds the same.
for top, _, files in os.walk(copy_dir):
    for name in files:
        rel = os.path.relpath(os.path.join(top, name), copy_dir).replace(os.sep, "/")
        asset = "/raw/assets/" in "/" + rel and sha(copy_dir, rel) is not None and sha(copy_dir, rel) == sha(work, rel)
        if rel not in listed and not asset:
            held["extra"].append(rel)
# Pages the old brain deleted since, by a copy no older: listed, while they're here.
held["gone"] = sorted(
    p for p in record["pages"]
    if p not in listed and ours(folder_of(p, known)) and not older(folder_of(p, known)) and sha(work, p) is not None
)
for f in shared:
    if ours(f):
        shared[f]["sequence"] = max(shared[f]["sequence"], sequence)
save(record_path, record)
settled = not unshared and not any(held.values())
for listed_here in held.values():
    listed_here.sort()
print(json.dumps(dict(took, **held, unshared=sorted(unshared), unread=unread, settled=settled), indent=1))
