# FIN-463 spike: seeds synthetic data through the spike store
# (spike.fragment.internal, armed writable at one prefix): 10,000 small
# files, one 1.5 GiB blob, and staged.zip holding both, plus a manifest.
import hashlib, http.client, json, os, random, sys, time, zipfile
from concurrent.futures import ThreadPoolExecutor

HOST = "spike.fragment.internal"
SMALL = int(os.environ.get("SMALL", "10000"))
BIG = int(os.environ.get("BIG", str(1536 * 1024 * 1024)))
MiB = 1 << 20
POOL = os.urandom(64 * MiB)


def block(i):
    off = (i * 1048573 * 7) % (len(POOL) - MiB)
    return POOL[off:off + MiB]


def blob_chunks(n):
    i = 0
    left = n
    while left > 0:
        b = block(i)[: min(MiB, left)]
        yield b
        left -= len(b)
        i += 1


def put(path, body, length):
    c = http.client.HTTPConnection(HOST, 80, timeout=600)
    c.request("PUT", "/o/" + path, body=body, headers={"content-length": str(length)})
    r = c.getresponse()
    data = r.read()
    c.close()
    if r.status != 200:
        raise RuntimeError(f"PUT {path}: {r.status} {data[:200]!r}")
    return json.loads(data)


def small_file(i):
    rnd = random.Random(i)
    n = rnd.randint(256, 4096)
    text = (f"file {i} " + "x" * 64 + "\n") * (n // 74 + 1)
    return f"tree/small/d{i // 100:02d}/f{i:05d}.txt", text.encode()[:n]


def main():
    manifest = {}
    t0 = time.time()
    with ThreadPoolExecutor(32) as ex:
        sizes = list(ex.map(lambda i: (lambda k, b: put(k, b, len(b))["size"])(*small_file(i)), range(SMALL)))
    dt = time.time() - t0
    print(json.dumps({"small": SMALL, "bytes": sum(sizes), "s": round(dt, 1), "per_s": round(SMALL / dt)}), flush=True)

    h = hashlib.sha256()

    def hashed():
        for c in blob_chunks(BIG):
            h.update(c)
            yield c

    t0 = time.time()
    put("tree/big/blob.bin", hashed(), BIG)
    dt = time.time() - t0
    manifest["tree/big/blob.bin"] = {"size": BIG, "sha256": h.hexdigest()}
    print(json.dumps({"blob": BIG, "s": round(dt, 1), "MBps": round(BIG / dt / 1e6, 1)}), flush=True)

    t0 = time.time()
    zpath = "/tmp/staged.zip"
    with zipfile.ZipFile(zpath, "w", compression=zipfile.ZIP_DEFLATED, allowZip64=True) as z:
        for i in range(SMALL):
            k, b = small_file(i)
            z.writestr(k[len("tree/"):], b)
        zi = zipfile.ZipInfo("big/blob.bin", date_time=(2026, 11, 9, 0, 0, 0))
        zi.compress_type = zipfile.ZIP_STORED
        with z.open(zi, "w", force_zip64=True) as w:
            for c in blob_chunks(BIG):
                w.write(c)
    zsize = os.path.getsize(zpath)
    print(json.dumps({"zip_built": zsize, "s": round(time.time() - t0, 1)}), flush=True)

    zh = hashlib.sha256()
    t0 = time.time()

    def zchunks():
        with open(zpath, "rb") as f:
            while True:
                c = f.read(4 * MiB)
                if not c:
                    break
                zh.update(c)
                yield c

    put("staged.zip", zchunks(), zsize)
    dt = time.time() - t0
    manifest["staged.zip"] = {"size": zsize, "sha256": zh.hexdigest()}
    print(json.dumps({"zip": zsize, "s": round(dt, 1), "MBps": round(zsize / dt / 1e6, 1)}), flush=True)
    os.remove(zpath)
    m = json.dumps(manifest).encode()
    put("manifest.json", m, len(m))
    print("DONE", json.dumps(manifest), flush=True)


main()
