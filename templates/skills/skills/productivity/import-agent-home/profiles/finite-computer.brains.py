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
#     lists for a person everything it can't settle by itself.
#
# Versions are the old brain's own (the name file's `versions`, FIN-773),
# never two copies' bytes compared. Nothing is ever deleted or written
# over a change made here. It prints one line of JSON; `settled` true means
# the brain holds everything the copy does, so the copy may go.
import hashlib, json, os, shutil, stat, sys

RECORD = "imported-from-finite.json"


def stop(why):
    sys.exit(f"STOP: {why}")


def clean(path):
    return (
        isinstance(path, str)
        and 0 < len(path.encode()) <= 4096
        and not any(c < " " or c == "\x7f" or c == "\\" for c in path)
        and all(name not in ("", ".", "..") for name in path.split("/"))
    )


def inside(root, path):
    # `root`/`path` when no part of it under `root` is a link: else None.
    full = root
    for name in path.split("/"):
        full = os.path.join(full, name)
        try:
            if stat.S_ISLNK(os.lstat(full).st_mode):
                return None
        except FileNotFoundError:
            break
        except OSError:
            return None
    return os.path.join(root, *path.split("/"))


def sha(root, path):
    full = inside(root, path)
    try:
        if full is None or not stat.S_ISREG(os.lstat(full).st_mode):
            return None
        with open(full, "rb") as f:
            return hashlib.sha256(f.read()).hexdigest()
    except OSError:
        return None


def occupied(root, path):
    # Something at `path` that isn't a plain file (a folder, say).
    full = inside(root, path)
    try:
        return full is None or not stat.S_ISREG(os.lstat(full).st_mode)
    except FileNotFoundError:
        return False
    except OSError:
        return True


def copy(src_root, dst_root, path):
    src, dst = inside(src_root, path), inside(dst_root, path)
    if src is None or dst is None or occupied(dst_root, path):
        return False
    os.makedirs(os.path.dirname(dst), exist_ok=True)
    shutil.copyfile(src, dst, follow_symlinks=False)
    return True


def assets(root, path):
    # The `resource: raw/assets/<file>` a note's frontmatter names, by its
    # path in the brain (under the note's wiki, its top folder).
    full = inside(root, path)
    try:
        if full is None or not stat.S_ISREG(os.lstat(full).st_mode):
            return []
        with open(full, errors="replace") as f:
            lines = f.read(1 << 20).split("\n")
    except OSError:
        return []
    if not lines or lines[0].strip() != "---":
        return []
    found = []
    for line in lines[1:]:
        if line.strip() == "---":
            break
        if line.startswith("resource:"):
            name = line[len("resource:"):].strip().strip("\"'")
            wiki = path.split("/", 1)[0]
            if name.startswith("raw/assets/") and clean(name):
                found.append(f"{wiki}/{name}")
    return found


MISSING = object()


def read_json(path, what):
    # A plain file's JSON, never one through a link; MISSING when absent.
    try:
        mode = os.lstat(path).st_mode
    except FileNotFoundError:
        return MISSING
    except OSError:
        stop(f"{what} can't be read: {path}")
    if not stat.S_ISREG(mode):
        stop(f"{what} isn't a plain file: {path}")
    try:
        with open(path) as f:
            return json.load(f)
    except (OSError, ValueError):
        stop(f"{what} can't be read as JSON: {path}")


def write_json(path, value):
    # Written whole beside it under a dot name (which never syncs), then
    # moved over it: a link there is replaced, never followed.
    folder, base = os.path.split(path)
    partial = os.path.join(folder, "." + base + ".partial")
    try:
        os.unlink(partial)
    except FileNotFoundError:
        pass
    fd = os.open(partial, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o644)
    with os.fdopen(fd, "w") as f:
        json.dump(value, f, indent=1, sort_keys=True)
    os.replace(partial, path)


usage = "usage: start|catch-up <old id> <name file> <brain folder> <folder> | copied <old id> <name file> <staged folder>"
if len(sys.argv) < 2 or (sys.argv[1], len(sys.argv)) not in (("start", 6), ("catch-up", 6), ("copied", 5)):
    sys.exit(usage)
mode, old, name_file = sys.argv[1:4]
named = read_json(name_file, "the name file")
versions = named.get("versions") if isinstance(named, dict) else None


def text(value):
    if not isinstance(value, str):
        raise TypeError
    return value


def ident(brain, id_):
    # Who a page or folder is in the old brain: its brain (null for the
    # tree's own) and its id there. A path can change hands; this can't.
    if brain is not None:
        text(brain)
    return [brain, text(id_)]


try:
    sequence = int(versions["sequence"])
    # path -> (identity, read)
    folders = {text(f["path"]): (ident(f["brain"], f["id"]), bool(f["read"])) for f in versions["folders"]}
    # path -> (identity, revision, sha256)
    listed = {
        text(p["path"]): (ident(p["brain"], p["object"]), int(p["revision"]), text(p["sha256"]))
        for p in versions["pages"]
    }
except (TypeError, KeyError, ValueError):
    sys.exit("NO VERSIONS: this copy has no page versions; compare by hand")

if mode == "copied":
    staged = sys.argv[4]
    named["exact"] = sorted(p for p, (_, _, h) in listed.items() if clean(p) and sha(staged, p) == h)
    write_json(name_file, named)
    print(json.dumps({"pages": len(listed), "exact": len(named["exact"])}))
    sys.exit(0)
work, other = sys.argv[4:6]


def folder_of(path, known):
    # The deepest folder `path` is in (a child folder is under its parent).
    best = None
    for folder in known:
        if path.startswith(folder + "/") and (best is None or len(folder) > len(best)):
            best = folder
    return best


record_path = os.path.join(work, RECORD)
record = read_json(record_path, RECORD)
if record is not MISSING:
    try:
        assert isinstance(record, dict) and record["from"] == "finite.computer"
        text(record["old"])
        int(record["sequence"])
        assert all(clean(f) and ident(*i) for f, i in record["folders"].items())
        for path, seen in record["pages"].items():
            assert clean(path)
            ident(*seen["id"]), int(seen["revision"])
            if seen["written"] is not None:
                text(seen["written"])
    except (AssertionError, TypeError, KeyError, ValueError, AttributeError):
        stop(f"{RECORD} is not one this file wrote: change nothing")
    if record["old"] != old:
        stop(f"{RECORD} is another brain's ({record['old']}), not {old}")

if mode == "start":
    # The folders the brain shares: each the old agent could read with a
    # page here (or, with no pages, a folder here). One left out stays out.
    # From a reference copy, `copied` checked its staged pages already.
    staged, exact = other, named.get("exact")
    pages = {}
    for path, (id_, revision, old_sha) in listed.items():
        here = sha(work, path) if clean(path) else None
        if here is None:
            continue
        # A staged page that isn't its revision's text holds an edit the old
        # brain never got: recorded with nothing written, so any later
        # change to it is for a person.
        exact_text = path in exact if isinstance(exact, list) else sha(staged, path) == old_sha
        pages[path] = {"id": id_, "revision": revision, "sha256": old_sha, "written": here if exact_text else None}
    with_pages = {folder_of(p, folders) for p in pages}
    shared = {
        f: id_ for f, (id_, read) in sorted(folders.items())
        if read and clean(f) and (f in with_pages or (
            not any(folder_of(p, folders) == f for p in listed)
            and inside(work, f) and os.path.isdir(inside(work, f))
        ))
    }
    if record is not MISSING:
        same = record["folders"] == shared and all(
            sha(work, p) == seen["written"] or seen["written"] is None
            for p, seen in record["pages"].items()
        )
        if not same:
            stop(f"{RECORD} is there already, and isn't this brain's start: push nothing")
        print(json.dumps({"recorded": len(record["pages"]), "shared folders": sorted(shared), "again": True}))
        sys.exit(0)
    record = {"from": "finite.computer", "old": old, "sequence": sequence, "folders": shared, "pages": pages}
    write_json(record_path, record)
    left = sorted(f for f in folders if f not in shared)
    print(json.dumps({"recorded": len(pages), "shared folders": sorted(shared), "not shared": left}))
    sys.exit(0)

if record is MISSING:
    sys.exit(f"NO RECORD: no {RECORD} here; compare by hand")
exact = named.get("exact")
if not isinstance(exact, list):
    sys.exit("NO VERSIONS: this copy was kept without its pages checked (`copied`); compare by hand")
exact = set(exact)
copy_dir = other
shared = record["folders"]
known = set(folders) | set(shared)


def shared_here(folder):
    # Shared, and the same folder in this copy, not another at its path.
    return folder in shared and folder in folders and folders[folder][0] == shared[folder]


def asset_plan(names):
    # (fine, to copy): not fine when one is unreachable, in neither, or
    # different here; never one written over.
    plan = []
    for asset in names:
        there, here = sha(copy_dir, asset), sha(work, asset)
        if not shared_here(folder_of(asset, known) or ""):
            return False, []  # in a folder the brain doesn't share
        if occupied(copy_dir, asset) or occupied(work, asset):
            return False, []  # a link or a folder there
        if here is None and there is None:
            return False, []
        if here is not None and there is not None and here != there:
            return False, []
        if here is None:
            plan.append(asset)
    return True, plan


older = sequence < int(record["sequence"])
out = {k: [] for k in ("added", "updated", "assets", "both", "gone", "unsynced", "missing", "older", "refused", "extra")}
unshared, healed, taken = {}, 0, []
for path, (id_, revision, old_sha) in sorted(listed.items()):
    if not clean(path) or inside(copy_dir, path) is None or inside(work, path) is None:
        out["refused"].append(path)  # climbs out, or through a link
        continue
    folder = folder_of(path, known) or ""
    if not shared_here(folder):
        unshared[folder] = unshared.get(folder, 0) + 1
        continue
    there = sha(copy_dir, path)
    if there is None:
        out["missing"].append(path)  # listed, but not in this copy
        continue
    if occupied(work, path):
        out["both"].append(path)  # a folder or such where the page goes
        continue
    seen, here = record["pages"].get(path), sha(work, path)
    fine, plan = asset_plan(assets(copy_dir, path))
    if here == there:
        # Already here, byte for byte: settled once its assets are, and
        # recorded if it wasn't.
        if not fine:
            out["both"].append(path)
            continue
        taken.append(("same", path, id_, revision, old_sha, plan))
        if path in exact and (
            seen is None
            or (seen["id"] == id_ and seen["revision"] < revision)
            or (seen["id"] != id_ and not older)
        ):
            record["pages"][path] = {"id": id_, "revision": revision, "sha256": old_sha, "written": here}
            healed += 1
        continue
    if path not in exact:
        out["unsynced"].append(path)  # an edit the old brain never got
        continue
    if seen is not None and seen["id"] == id_ and revision <= seen["revision"]:
        continue  # the brain has this version, or a newer one
    if older:
        out["older"].append(path)  # this copy is older than the brain's
        continue
    if seen is None and here is None:
        kind = "added"
    elif seen is not None and seen["id"] == id_ and here is not None and here == seen["written"]:
        kind = "updated"  # newer there, unchanged here
    else:
        out["both"].append(path)  # changed or deleted here, made here, or another page
        continue
    if not fine:
        out["both"].append(path)  # its asset differs here, or can't come
        continue
    taken.append((kind, path, id_, revision, old_sha, plan))

for kind, path, id_, revision, old_sha, plan in taken:
    for asset in plan:
        if sha(work, asset) is None and copy(copy_dir, work, asset):
            out["assets"].append(asset)
    if kind == "same":
        continue
    if copy(copy_dir, work, path):
        record["pages"][path] = {"id": id_, "revision": revision, "sha256": old_sha, "written": sha(work, path)}
        out[kind].append(path)
    else:
        out["refused"].append(path)

# Files the copy holds that no listed page is: kept for a person, unless
# an asset the brain holds the same.
for top, dirs, files in os.walk(copy_dir):
    for name in files:
        rel = os.path.relpath(os.path.join(top, name), copy_dir).replace(os.sep, "/")
        if rel in listed:
            continue
        if "/raw/assets/" in "/" + rel and sha(copy_dir, rel) is not None and sha(copy_dir, rel) == sha(work, rel):
            continue
        out["extra"].append(rel)
out["extra"].sort()

if not older:
    # Pages the old brain deleted since: listed, while they're here.
    out["gone"] = sorted(
        p for p in record["pages"]
        if p not in listed and shared_here(folder_of(p, known) or "")
        and folders[folder_of(p, known)][1] and sha(work, p) is not None
    )
record["sequence"] = max(int(record["sequence"]), sequence)
write_json(record_path, record)
unread = sorted(f for f, (_, read) in folders.items() if not read)
report = os.path.normpath(work) + ".catch-up.json"
held = ("both", "gone", "unsynced", "missing", "older", "refused", "extra")
settled = not unshared and not any(out[k] for k in held)
write_json(report, dict(out, unshared=unshared, unread=unread, settled=settled))
print(json.dumps(
    {k: len(v) for k, v in out.items()}
    | {"healed": healed, "recorded": len(record["pages"]), "not shared here": sorted(unshared),
       "unread folders": unread, "settled": settled, "list": report}
))
