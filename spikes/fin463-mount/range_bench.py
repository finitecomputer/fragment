# FIN-463 spike, the fallback: a staged zip read by HTTP ranges through a
# Worker-gated host (spike.fragment.internal, one person's prefix), with
# Python's zipfile over a seekable range-reading file object.
#   range_bench.py <what,...>   what: list, big, rand, par
import hashlib, http.client, io, json, random, sys, threading, time, zipfile
from concurrent.futures import ThreadPoolExecutor

HOST = "spike.fragment.internal"
WHAT = sys.argv[1].split(",")
MiB = 1 << 20
stats = {"requests": 0, "bytes": 0}
lock = threading.Lock()
local = threading.local()


def conn():
    c = getattr(local, "c", None)
    if c is None:
        c = local.c = http.client.HTTPConnection(HOST, 80, timeout=300)
    return c


def get(path, start=None, end=None):
    headers = {} if start is None else {"range": f"bytes={start}-{end}"}
    for attempt in range(3):
        try:
            c = conn()
            c.request("GET", path, headers=headers)
            r = c.getresponse()
            data = r.read()
            break
        except (http.client.HTTPException, OSError):
            local.c = None
            if attempt == 2:
                raise
    if r.status not in (200, 206):
        raise IOError(f"GET {path} {headers}: {r.status} {data[:200]!r}")
    with lock:
        stats["requests"] += 1
        stats["bytes"] += len(data)
    return data


def head_size(path):
    c = conn()
    c.request("HEAD", path)
    r = c.getresponse()
    r.read()
    with lock:
        stats["requests"] += 1
    return int(r.headers["content-length"])


class RangeFile(io.RawIOBase):
    """A read-only, seekable file over HTTP ranges, with a small block cache."""

    def __init__(self, path, block=MiB, blocks=64):
        self.path, self.block, self.max_blocks = path, block, blocks
        self.size = head_size(path)
        self.pos = 0
        self.cache = {}

    def readable(self):
        return True

    def seekable(self):
        return True

    def tell(self):
        return self.pos

    def seek(self, off, whence=0):
        self.pos = off if whence == 0 else self.pos + off if whence == 1 else self.size + off
        return self.pos

    def _block(self, i):
        b = self.cache.get(i)
        if b is None:
            start = i * self.block
            b = get(self.path, start, min(start + self.block, self.size) - 1)
            if len(self.cache) >= self.max_blocks:
                self.cache.pop(next(iter(self.cache)))
            self.cache[i] = b
        return b

    def read(self, n=-1):
        if n is None or n < 0:
            n = self.size - self.pos
        n = min(n, self.size - self.pos)
        if n <= 0:
            return b""
        # a big read goes as one range, past the cache
        if n >= 4 * self.block:
            data = get(self.path, self.pos, self.pos + n - 1)
            self.pos += len(data)
            return data
        out = bytearray()
        while len(out) < n:
            i, o = divmod(self.pos, self.block)
            b = self._block(i)
            take = b[o:o + n - len(out)]
            out += take
            self.pos += len(take)
        return bytes(out)

    def readinto(self, buf):
        data = self.read(len(buf))
        buf[: len(data)] = data
        return len(data)


def out(**kw):
    print(json.dumps(kw), flush=True)


manifest = json.loads(get("/o/manifest.json"))

if "list" in WHAT:
    t0 = time.time()
    f = RangeFile("/o/staged.zip")
    z = zipfile.ZipFile(f)
    names = z.namelist()
    dt = time.time() - t0
    out(step="open zip + list (central directory)", entries=len(names), s=round(dt, 2), requests=stats["requests"], bytes=stats["bytes"])
    r0 = dict(stats)
    picks = random.Random(7).sample([n for n in names if n.startswith("small/")], 50)
    t0 = time.time()
    got = sum(len(z.read(n)) for n in picks)
    out(step="read 50 random small members", s=round(time.time() - t0, 2), bytes=got, requests=stats["requests"] - r0["requests"])

if "big" in WHAT:
    f = RangeFile("/o/staged.zip")
    z = zipfile.ZipFile(f)
    info = z.getinfo("big/blob.bin")
    want = manifest["tree/big/blob.bin"]["sha256"]
    h = hashlib.sha256()
    r0 = dict(stats)
    t0 = time.time()
    n = 0
    with z.open(info) as m:
        while True:
            c = m.read(16 * MiB)
            if not c:
                break
            h.update(c)
            n += len(c)
    dt = time.time() - t0
    out(step="stream 1.5 GiB member via zipfile, 16 MiB ranges", bytes=n, s=round(dt, 1), MBps=round(n / dt / 1e6, 1), sha_ok=h.hexdigest() == want, requests=stats["requests"] - r0["requests"])

if "rand" in WHAT:
    f = RangeFile("/o/staged.zip")
    z = zipfile.ZipFile(f)
    info = z.getinfo("big/blob.bin")
    # the member's data begins after its local header
    f.seek(info.header_offset)
    hdr = f.read(30)
    data_at = info.header_offset + 30 + int.from_bytes(hdr[26:28], "little") + int.from_bytes(hdr[28:30], "little")
    rnd = random.Random(463)
    lat = []
    for _ in range(30):
        off = data_at + rnd.randrange(0, info.file_size - MiB)
        t0 = time.time()
        b = get("/o/staged.zip", off, off + MiB - 1)
        lat.append(time.time() - t0)
        assert len(b) == MiB
    lat.sort()
    out(step="30 random 1 MiB range reads in the member", p50_ms=round(lat[15] * 1000), max_ms=round(lat[-1] * 1000), total_s=round(sum(lat), 2))

if "par" in WHAT:
    size = manifest["tree/big/blob.bin"]["size"]
    part = 64 * MiB
    for workers in (4, 8):
        t0 = time.time()
        with ThreadPoolExecutor(workers) as ex:
            n = sum(ex.map(lambda i: len(get("/o/tree/big/blob.bin", i, min(i + part, size) - 1)), range(0, size, part)))
        dt = time.time() - t0
        out(step=f"blob.bin in 64 MiB ranges, {workers} at once", bytes=n, s=round(dt, 1), MBps=round(n / dt / 1e6, 1))
    t0 = time.time()
    c = http.client.HTTPConnection(HOST, 80, timeout=300)
    c.request("GET", "/o/tree/big/blob.bin")
    r = c.getresponse()
    n = 0
    while True:
        b = r.read(8 * MiB)
        if not b:
            break
        n += len(b)
    dt = time.time() - t0
    out(step="blob.bin as one GET", bytes=n, s=round(dt, 1), MBps=round(n / dt / 1e6, 1))
print("DONE", flush=True)
