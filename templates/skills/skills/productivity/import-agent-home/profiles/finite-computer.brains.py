# A team brain from the old Finite, brought together one move at a time
# (finite-computer.md, "Record it" and "Once you are in the real one").
#
#   start <old id> <name file> <brain folder> <staged folder> [<folder left out>...]
#     takes out of the brain's first push each folder left out, and the
#     assets their notes name, with versions or without; with them, it
#     records in the brain's imported-from-finite.json which folders it
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
import hashlib, json, os, re, shutil, sys, tempfile

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


def carried(path):
    # A path `fragment sync` carries (cli/src/sync/scan.rs `syncable`, and
    # proto's `valid_repo_path`): one it skips would stay in this folder
    # alone, and look settled here.
    if not clean(path) or len(path.encode()) > 300 or path[1:2] == ":" or path.split("/")[0] == "node_modules":
        return False
    return not any(
        name[0] == "." or name[-1] == "~" or name[:2] == "~$" or name.endswith(".swp")
        or ".conflict-" in name or ".fragment-partial" in name
        for name in path.split("/")
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


def whole(path, write):
    # Written whole beside it, under a dot name of its own that never syncs,
    # then moved over it: a link there is replaced, never followed, and a
    # write that fails leaves what was there.
    fd, part = tempfile.mkstemp(dir=os.path.dirname(path) or ".", prefix=".", suffix=".fragment-partial")
    try:
        with os.fdopen(fd, "wb") as f:
            os.fchmod(f.fileno(), 0o644)
            write(f)
        os.replace(part, path)
    except OSError:
        os.unlink(part)
        raise


def put(src_root, dst_root, path):
    # The copy's file onto the brain's, never through a link: whether it went.
    src, dst = at(src_root, path), at(dst_root, path)
    if sha(src_root, path) is None or dst is None or blocked(dst_root, path):
        return False
    try:
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        with open(src, "rb") as f:
            whole(dst, lambda out: shutil.copyfileobj(f, out))
        return True
    except OSError:
        return False  # a file where its folder goes, or a full disk


def files_in(root):
    # Every file under `root`, by its path there, but in dot folders.
    for top, folders_here, names in os.walk(root):
        folders_here[:] = [name for name in folders_here if not name.startswith(".")]
        for name in names:
            yield os.path.relpath(os.path.join(top, name), root).replace(os.sep, "/")


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
            # A YAML value, read only in the forms it can't be misread in:
            # plain up to a comment, or quoted with no escape in it. Any
            # other names nothing, so its asset stops `start`.
            value = line[len("resource:"):].strip()
            read = re.fullmatch(r"""'([^']*)'(?:\s+#.*)?|"([^"\\]*)"(?:\s+#.*)?|([^'"\s].*?)(?:\s+#.*)?""", value)
            name = next((g for g in read.groups() if g is not None), "") if read else ""
            asset = path.split("/", 1)[0] + "/" + name
            if name.startswith("raw/assets/") and clean(asset):
                found.append(asset)
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
    try:
        whole(path, lambda f: f.write(json.dumps(value, indent=1, sort_keys=True).encode()))
    except OSError as e:
        stop(f"{path} can't be written ({e}): push nothing")


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


usage = (
    "usage: start <old id> <name file> <brain folder> <staged folder> [<folder left out>...]"
    " | copied <old id> <name file> <staged folder> | catch-up <old id> <name file> <brain folder> <copy folder>"
)
if len(sys.argv) < 2 or not (
    (sys.argv[1] == "start" and len(sys.argv) >= 6) or (sys.argv[1], len(sys.argv)) in (("catch-up", 6), ("copied", 5))
):
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
    versions = True
except (TypeError, KeyError, ValueError, AttributeError):
    if mode != "start":
        sys.exit("NO VERSIONS: this copy has no page versions; compare by hand")
    # `start` takes a folder left out of the push without them too.
    versions, sequence, folders, listed = False, 0, {}, {}
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
    staged, left_out = other, sys.argv[6:]
    exact = named.get("exact") if isinstance(named, dict) else None
    for f in left_out:
        if not clean(f) or not os.path.isdir(at(staged, f) or ""):
            stop(f"{f} is no folder of {staged}: name each folder left out as `ls` names it")

    def out(rel):
        # In a folder its owner left out, or that folder itself.
        return any(rel == f or rel.startswith(f + "/") for f in left_out)

    here = {p: h for p in listed if not out(p) and (h := sha(work, p)) is not None}
    # The folders it shares: each the old agent could read, with a page
    # here (or, with no pages, a folder here), and not left out.
    held = {folder_of(p, folders) for p in here}
    empty = set(folders) - {folder_of(p, folders) for p in listed}
    shared = {
        f: {"id": id_, "sequence": sequence}
        for f, (id_, read) in folders.items()
        if read and not out(f) and (f in held or (f in empty and os.path.isdir(at(work, f) or "")))
    }

    def outside(rel):
        # In a folder it doesn't share: one left out, or one the old agent
        # couldn't read (which only versions name).
        return out(rel) or folder_of(rel, folders) not in (None, *shared)

    pages, refused = {}, []
    for path, h in sorted(here.items()):
        id_, revision, old_sha = listed[path]
        if outside(path):
            continue
        if not carried(path):
            refused.append(path)  # sync never carries it
            continue
        # Staged as other text than its revision's: an edit the old brain
        # never got. Nothing is recorded as written, so any later change
        # to it in the old brain is for a person.
        exact_text = path in exact if isinstance(exact, list) else sha(staged, path) == old_sha
        pages[path] = {"id": id_, "revision": revision, "written": h if exact_text else None}
    new = {"from": "finite.computer", "old": old, "folders": shared, "pages": pages} if versions else MISSING
    if record is not MISSING and record != new:
        stop(f"{RECORD} is there already, and isn't this brain's start: push nothing")
    # Out of the push goes all a folder it doesn't share holds, with or
    # without versions: its files, and the assets its notes name, here or
    # in the copy (they sit in its parent wiki's `raw/assets/`). One a page
    # it shares names too stays out for a person (`ambiguous`). While a
    # folder is out, an asset no note names stops it: whose it is can't be
    # told.
    mine = list(files_in(work))
    gone = [(work, rel) for rel in mine if outside(rel)] + [(staged, rel) for rel in files_in(staged) if outside(rel)]
    theirs = {a for root, rel in gone for a in assets(root, rel)}
    kept = {a for rel in mine if not outside(rel) and carried(rel) for a in assets(work, rel)}
    held_out = {a for a in theirs if a not in listed and sha(work, a) is not None}
    stray = sorted(
        rel for rel in mine
        if gone and rel.split("/")[1:3] == ["raw", "assets"] and rel.count("/") > 2
        and not outside(rel) and rel not in kept | theirs and rel not in listed
    )
    if stray:
        stop(f"no page it shares names {', '.join(stray)}: push nothing, and ask whose each is")
    removed = sorted({rel for rel in mine if outside(rel)} | held_out)
    # What is left of a folder (empty folders, dot folders) never syncs.
    try:
        for rel in removed:
            os.unlink(os.path.join(work, *rel.split("/")))  # a link itself, never what it names
    except OSError as e:
        stop(f"{e.filename} can't be taken out of the push ({e.strerror}): push nothing")
    if versions:
        save(record_path, new)
    print(json.dumps({
        "versions": versions, "recorded": len(pages), "shared": sorted(shared),
        "not shared": sorted(set(folders) - set(shared) | set(left_out)),
        "removed": removed, "ambiguous": sorted(held_out & kept), "refused": refused,
    }))
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
        if not carried(asset) or blocked(copy_dir, asset) or blocked(work, asset):
            return None  # one sync never carries, or a link or a folder there
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
    if not carried(path) or at(copy_dir, path) is None or at(work, path) is None:
        held["refused"].append(path)  # climbs out, goes through a link, or never syncs
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
        # Never to an older revision: what the brain took stays the base.
        heal = path in exact and (
            base["revision"] < revision or base["revision"] == revision and base["written"] is None
            if base else seen is None or not older(folder)
        )
        if plan is None:
            held["both"].append(path)
        else:
            take.append(("same", path, entry if heal else None, plan))
        continue
    if path not in exact:
        held["unsynced"].append(path)  # an edit the old brain never got
        continue
    if base is not None and (revision < base["revision"] or revision == base["revision"] and base["written"] is not None):
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
for rel in files_in(copy_dir):
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
