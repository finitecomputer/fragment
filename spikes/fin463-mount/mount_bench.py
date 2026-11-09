# FIN-463 spike: what reading a staged tree through the FUSE mount is like.
#   mount_bench.py <mount> <what,...>   what: list, big, zip, rand, ro
import hashlib, json, os, random, shutil, subprocess, sys, threading, time, zipfile

M = sys.argv[1]
WHAT = sys.argv[2].split(",")
MiB = 1 << 20


def out(**kw):
    print(json.dumps(kw), flush=True)


def disk_watch(stop, peak):
    base = shutil.disk_usage("/").used
    peak["base"] = base
    while not stop.is_set():
        peak["max"] = max(peak.get("max", 0), shutil.disk_usage("/").used - base)
        time.sleep(0.5)


def run(cmd):
    t0 = time.time()
    p = subprocess.run(["sh", "-c", cmd], capture_output=True, text=True)
    return round(time.time() - t0, 2), p.returncode, (p.stdout + p.stderr).strip()[-300:]


manifest = json.load(open(os.path.join(M, "manifest.json")))

if "list" in WHAT:
    for label in ("cold", "warm"):
        out(step=f"find names ({label})", result=run(f"find {M}/tree/small -type f | wc -l"))
        out(step=f"ls -lR stat each ({label})", result=run(f"ls -lR {M}/tree/small | grep -c '^-'"))
    out(step="du -s", result=run(f"du -sk {M}/tree/small"))

if "big" in WHAT:
    want = manifest["tree/big/blob.bin"]
    stop, peak = threading.Event(), {}
    th = threading.Thread(target=disk_watch, args=(stop, peak), daemon=True)
    th.start()
    h = hashlib.sha256()
    t0 = time.time()
    n = 0
    first = None
    with open(os.path.join(M, "tree/big/blob.bin"), "rb", buffering=0) as f:
        while True:
            c = f.read(8 * MiB)
            if first is None:
                first = round(time.time() - t0, 3)
            if not c:
                break
            h.update(c)
            n += len(c)
    dt = time.time() - t0
    stop.set()
    th.join()
    out(step="read blob.bin sequentially", bytes=n, s=round(dt, 1), MBps=round(n / dt / 1e6, 1), first_read_s=first, sha_ok=h.hexdigest() == want["sha256"], disk_grew_peak_MB=round(peak.get("max", 0) / 1e6), disk_after_MB=round((shutil.disk_usage("/").used - peak["base"]) / 1e6))
    out(step="df after big", df=run("df -h / /tmp | tail -2")[2])

if "rand" in WHAT:
    size = manifest["tree/big/blob.bin"]["size"]
    rnd = random.Random(463)
    lat = []
    with open(os.path.join(M, "tree/big/blob.bin"), "rb", buffering=0) as f:
        fd = f.fileno()
        for _ in range(30):
            off = rnd.randrange(0, size - MiB)
            t0 = time.time()
            b = os.pread(fd, MiB, off)
            lat.append(time.time() - t0)
            assert len(b) == MiB
    lat.sort()
    out(step="30 random 1 MiB preads in blob.bin", p50_ms=round(lat[15] * 1000), max_ms=round(lat[-1] * 1000), total_s=round(sum(lat), 2))

if "zip" in WHAT:
    t0 = time.time()
    z = zipfile.ZipFile(os.path.join(M, "staged.zip"))
    names = z.namelist()
    t_open = time.time() - t0
    rnd = random.Random(7)
    picks = rnd.sample([n for n in names if n.startswith("small/")], 50)
    t0 = time.time()
    got = sum(len(z.read(n)) for n in picks)
    t_small = time.time() - t0
    info = z.getinfo("big/blob.bin")
    t0 = time.time()
    with z.open(info) as f:
        f.seek(info.file_size // 2)
        mid = f.read(4 * MiB)
    t_seek = time.time() - t0
    out(step="zipfile on the mount", entries=len(names), open_and_list_s=round(t_open, 2), read_50_small_s=round(t_small, 2), small_bytes=got, seek_mid_big_read_4MiB_s=round(t_seek, 2), mid_bytes=len(mid))

if "ro" in WHAT:
    for cmd in (f"touch {M}/new.txt", f"echo x >> {M}/tree/small/d00/f00000.txt", f"rm {M}/manifest.json", f"mkdir {M}/newdir", f"mv {M}/manifest.json {M}/m2.json"):
        out(step="write attempt", cmd=cmd.replace(M, "<mount>"), result=run(cmd))
    out(step="mount line", result=run(f"grep ' {M} ' /proc/self/mounts"))
print("DONE", flush=True)
