# A team's brain from the old Finite, brought together one move at a time
# (FIN-774; finite-computer.md, "Brains, as brains"). Stdlib only; reads and
# writes only the folders it is given.
#
#   start <old id> <name file> <brain folder>
#       record what a brain just made from a copy holds of the old one, in
#       its imported-from-finite.json, and the old folders left out of it
#       (a folder only some should read): a catch-up never brings those
#   catch-up <old id> <name file> <brain folder> <copy folder>
#       bring a later copy's newer pages in: a page the old brain changed
#       since and nobody changed here is updated, a new one added; one
#       changed on both sides is left for a person, and one the old brain
#       dropped is listed, never deleted
#
# Versions are the old brain's own (the name file's `versions`, which the
# mover writes from the working tree's record: FIN-773), never the bytes of
# a copy, whose asset notes an import rewrites. Prints counts; the paths go
# to <brain folder>.catch-up.json beside it.

import hashlib, json, os, shutil, sys
mode, old, name_file, work = sys.argv[1:5]
copy = sys.argv[5] if mode == "catch-up" else None
RECORD = "imported-from-finite.json"


def sha(path):
    try:
        with open(path, "rb") as f:
            return hashlib.sha256(f.read()).hexdigest()
    except (FileNotFoundError, IsADirectoryError, NotADirectoryError):
        return None


try:
    with open(name_file) as f:
        versions = json.load(f).get("versions")
except (OSError, ValueError, AttributeError):
    versions = None
if not versions:
    sys.exit("NO VERSIONS: this stage has no page versions; compare by hand")
record_path = os.path.join(work, RECORD)
try:
    with open(record_path) as f:
        record = json.load(f)
except FileNotFoundError:
    record = None
if mode == "start":
    if record is not None:
        sys.exit(f"STOP: {RECORD} is there already; this brain was started before")
    left = sorted(f["path"] for f in versions["folders"] if not os.path.isdir(os.path.join(work, f["path"])))
    record = {"from": "finite.computer", "old": old, "sequence": 0, "pages": {}, "left": left}
elif record is None:
    sys.exit(f"NO RECORD: no {RECORD} here; compare by hand")
if record.get("old") != old:
    sys.exit(f"STOP: {RECORD} is another brain's ({record.get('old')}), not {old}")

pages = record["pages"]
read = {f["path"] for f in versions["folders"] if f["read"]}
unread = sorted(f["path"] for f in versions["folders"] if not f["read"])
listed = {p["path"] for p in versions["pages"]}
out = {k: [] for k in ("added", "updated", "both", "gone", "assets")}
left = set(record.get("left", []))
for page in versions["pages"]:
    path, revision, old_sha = page["path"], page["revision"], page["sha256"]
    here_path = os.path.join(work, path)
    here = sha(here_path)
    if mode == "start":
        if here is not None:
            pages[path] = {"revision": revision, "sha256": old_sha, "written": here}
        continue
    if path.split("/", 1)[0] in left:
        continue  # a folder this brain leaves out
    there_path = os.path.join(copy, path)
    if sha(there_path) is None:
        continue  # not in this copy
    seen = pages.get(path)
    if seen is not None and revision <= seen["revision"]:
        continue  # the brain has this version, or a newer one
    if seen is None and here is None:
        kind = "added"
    elif seen is not None and here == seen["written"]:
        kind = "updated"  # newer there, unchanged here
    else:
        out["both"].append(path)  # changed here too, or made here: a person decides
        continue
    os.makedirs(os.path.dirname(here_path), exist_ok=True)
    shutil.copyfile(there_path, here_path)
    pages[path] = {"revision": revision, "sha256": old_sha, "written": sha(here_path)}
    out[kind].append(path)

if mode == "catch-up":
    # The asset each page taken names (`resource: raw/assets/<file>`), from
    # the copy, unless the brain has it already: never one written over.
    for path in out["added"] + out["updated"]:
        wiki = path.split("/", 1)[0]
        with open(os.path.join(work, path), errors="replace") as f:
            for line in f:
                name = line.partition("resource:")[2].strip().strip("\"'")
                if line.startswith("resource:") and name.startswith("raw/assets/") and ".." not in name:
                    asset = os.path.join(wiki, name)
                    there, here = os.path.join(copy, asset), os.path.join(work, asset)
                    if os.path.isfile(there) and not os.path.exists(here):
                        os.makedirs(os.path.dirname(here), exist_ok=True)
                        shutil.copyfile(there, here)
                        out["assets"].append(asset)
    if versions["sequence"] > record["sequence"]:
        out["gone"] = sorted(
            p for p in pages if p not in listed and p.split("/", 1)[0] in read
        )
record["sequence"] = max(record["sequence"], versions["sequence"])
partial = os.path.join(work, "." + RECORD + ".partial")  # a dot name never syncs
with open(partial, "w") as f:
    json.dump(record, f, indent=1, sort_keys=True)
os.replace(partial, record_path)
report = os.path.normpath(work) + ".catch-up.json"
with open(report, "w") as f:
    json.dump(dict(out, unread=unread, left=sorted(left)), f, indent=1)
print(json.dumps({k: len(v) for k, v in out.items()} | {"recorded": len(pages), "unread folders": unread, "left out here": sorted(left), "list": report}))
