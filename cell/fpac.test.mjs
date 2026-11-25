// fpac.mjs: the disk route answers FPAC's contract (finitecomputer/fpac,
// docs/contract.md at v0.1.0): its conformance cases 1-12, the route's own
// (13-19, the head's rules, are fragment_core::computer::fpac's, tested
// there), against a stand-in bucket that checks a put's SHA-256 and its
// condition as R2 does. Valid (open from the props, gets whole and ranged,
// a put created, the same put again the same), invalid (a name outside
// the grammar, another life's epoch, a body past its limit or unnamed, a
// range we do not serve or past the end), and replay (another writer's
// bytes conflict, naming the stored sum). A head post goes to the DO's
// rules, its answer passed back.
import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { fpacRoute } from "./fpac.mjs";

// workerd's: a stream of a length named up front
globalThis.FixedLengthStream = class {
  constructor(length) {
    const t = new TransformStream();
    this.readable = t.readable;
    this.writable = t.writable;
    this.length = length;
  }
};

const sha = (bytes) => createHash("sha256").update(bytes).digest();

// R2 as the route uses it: a put checks its SHA-256 and `if-none-match: *`
function bucket() {
  const objects = new Map();
  return {
    objects,
    async get(key, opts = {}) {
      const o = objects.get(key);
      if (!o) return null;
      let bytes = o;
      if (opts.range) {
        if (opts.range.offset > o.length) throw new Error("range starts past the end");
        bytes = o.subarray(opts.range.offset, opts.range.offset + opts.range.length);
      }
      return { size: o.length, checksums: { sha256: sha(o).buffer }, body: new Response(bytes).body };
    },
    async head(key) {
      const o = objects.get(key);
      return o ? { size: o.length, checksums: { sha256: sha(o).buffer } } : null;
    },
    async put(key, readable, { sha256, onlyIf }) {
      const bytes = new Uint8Array(await new Response(readable).arrayBuffer());
      if (sha(bytes).toString("hex") !== sha256) throw new Error("the SHA-256 checksum does not match");
      if (onlyIf.get("if-none-match") === "*" && objects.has(key)) return null;
      objects.set(key, bytes);
      return { key };
    },
  };
}

const RULES = {
  // the grammar as fragment_core::computer::fpac::object_epoch has it
  nameEpoch: (n) => {
    const m = /^e([1-9][0-9]{0,19})\/[a-z][a-z0-9.-]*$/.exec(n);
    return m && n.length <= 96 ? Number(m[1]) : null;
  },
  objectBytesMax: 1024,
  headLineMax: 200,
};
const DISK = { prefix: "disks/17c33cdf88c6d1d8f58b7a0f/", name: "17c33cdf88c6d1d8f58b7a0f", epoch: 7, volume: 20_971_520_000, head: "6 6 2 " + "ab".repeat(32) + " clean" };
const call = (b, method, path, { body, headers = {} } = {}, posted = async () => ({ answer: "taken" })) =>
  fpacRoute(new Request(`http://fpac.fragment.internal${path}`, { method, body, headers, duplex: "half" }), b, DISK, RULES, posted);
const putOf = (b, path, bytes, sum = sha(bytes).toString("hex")) => call(b, "PUT", path, { body: bytes, headers: { "x-fpac-sha256": sum, "content-length": String(bytes.length) } });

test("open names the disk, this life's epoch, its volume and its head, from the props", async () => {
  const r = await call(bucket(), "GET", "/open");
  assert.equal(r.status, 200);
  assert.deepEqual([r.headers.get("x-fpac-disk"), r.headers.get("x-fpac-epoch"), r.headers.get("x-fpac-volume-bytes"), r.headers.get("x-fpac-head")], [DISK.name, "7", "20971520000", DISK.head]);
  const fresh = await fpacRoute(new Request("http://fpac.fragment.internal/open"), bucket(), { ...DISK, head: null }, RULES, async () => ({}));
  assert.equal(fresh.headers.get("x-fpac-head"), "none", "a new disk's head is none");
});

test("a put is created, the same put again is the same, other bytes conflict naming the stored sum", async () => {
  const b = bucket();
  const bytes = new TextEncoder().encode("segment one");
  const r = await putOf(b, "/o/e7/s1", bytes);
  assert.deepEqual([r.status, r.headers.get("x-fpac-sha256")], [201, sha(bytes).toString("hex")]);
  assert.ok(b.objects.has(`${DISK.prefix}e7/s1`), "under its disk's prefix");
  assert.equal((await putOf(b, "/o/e7/s1", bytes)).status, 200, "its own earlier put, whose answer was lost");
  const other = new TextEncoder().encode("another writer");
  const c = await putOf(b, "/o/e7/s1", other);
  assert.deepEqual([c.status, c.headers.get("x-fpac-sha256")], [409, sha(bytes).toString("hex")]);
  const empty = await putOf(b, "/o/e7/s2", new Uint8Array(0));
  assert.equal(empty.status, 201, "an empty object round-trips");
  assert.equal((await call(b, "GET", "/o/e7/s2")).status, 200);
});

test("gets stream the whole object with its sum, or exactly a range, to its end at most", async () => {
  const b = bucket();
  const bytes = new TextEncoder().encode("0123456789");
  await putOf(b, "/o/e7/s1", bytes);
  const whole = await call(b, "GET", "/o/e7/s1");
  assert.deepEqual([whole.status, whole.headers.get("x-fpac-sha256"), await whole.text()], [200, sha(bytes).toString("hex"), "0123456789"]);
  const part = await call(b, "GET", "/o/e7/s1", { headers: { range: "bytes=2-4" } });
  assert.deepEqual([part.status, part.headers.get("content-range"), await part.text()], [206, "bytes 2-4/10", "234"]);
  const tail = await call(b, "GET", "/o/e7/s1", { headers: { range: "bytes=8-100" } });
  assert.deepEqual([tail.status, await tail.text()], [206, "89"], "a range past the end answers the tail");
  assert.equal((await call(b, "GET", "/o/e7/s1", { headers: { range: "bytes=10-12" } })).status, 416, "one starting past the end is refused");
  assert.equal((await call(b, "GET", "/o/e7/s1", { headers: { range: "bytes=4-2" } })).status, 400);
  assert.equal((await call(b, "GET", "/o/e7/s1", { headers: { range: "bytes=-2" } })).status, 400, "only one form of range");
  assert.equal((await call(b, "GET", "/o/e7/s9", { headers: { range: "bytes=0-1" } })).status, 404, "an absent name, ranged");
  assert.equal((await call(b, "GET", "/o/e7/s9")).status, 404, "an absent name, whole");
  assert.equal((await call(b, "GET", "/o/e6/s1")).status, 404, "an earlier life's name may be read");
});

test("a name outside the grammar, another life's epoch, a body past its limit or unnamed are refused", async () => {
  const b = bucket();
  const bytes = new TextEncoder().encode("x");
  for (const path of ["/o/e0/s1", "/o/e7/S1", "/o/x7/s1", "/o/e7/s1/x"]) {
    assert.equal((await call(b, "GET", path)).status, 400, path);
  }
  assert.equal((await call(b, "GET", "/o/../e7/s1")).status, 404, "a path that climbs out of /o/ is no route");
  assert.equal((await putOf(b, "/o/e6/s1", bytes)).status, 403, "a put into another life's epoch");
  assert.equal((await putOf(b, "/o/e7/s1", new Uint8Array(RULES.objectBytesMax + 1))).status, 413);
  assert.equal((await putOf(b, "/o/e7/s1", bytes, "nope")).status, 400, "a put names its SHA-256");
  assert.equal((await putOf(b, "/o/e7/s1", bytes, "00".repeat(32))).status, 400, "a body that is not its SHA-256");
  assert.equal(b.objects.size, 0, "nothing stored");
  assert.equal((await call(b, "DELETE", "/o/e7/s1")).status, 405, "no delete");
  assert.equal((await call(b, "GET", "/list")).status, 404, "no list");
});

test("a head post goes to the DO's rules with this life's epoch, its answer passed back", async () => {
  const seen = [];
  const posted = async (epoch, line) => {
    seen.push([epoch, line]);
    return line.endsWith("clean") ? { answer: "same" } : { refused: "an earlier life's head", status: 409 };
  };
  const line = "7 7 1 " + "cd".repeat(32);
  const same = await call(bucket(), "POST", "/head", { body: `${line} clean`, headers: { "content-length": String(line.length + 6) } }, posted);
  assert.deepEqual([same.status, same.headers.get("x-fpac-head-answer")], [200, "same"]);
  const refused = await call(bucket(), "POST", "/head", { body: line, headers: { "content-length": String(line.length) } }, posted);
  assert.deepEqual([refused.status, await refused.text()], [409, "an earlier life's head\n"]);
  const long = "9".repeat(RULES.headLineMax + 1);
  assert.equal((await call(bucket(), "POST", "/head", { body: long, headers: { "content-length": String(long.length) } }, posted)).status, 413);
  assert.deepEqual(seen.map(([e]) => e), [7, 7], "this life's epoch, from the props; the long one never reached it");
});
