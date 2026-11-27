// fpac.mjs: a computer's disk route (docs/computers.md, "A disk"): what
// its guest's FPAC daemon calls at fpac.fragment.internal, by FPAC's
// contract (finitecomputer/fpac, docs/contract.md, at the v0.1.0 the
// images pin). Apart from entry.mjs, which imports what only workerd has,
// so node's runner tries it against a stand-in bucket (fpac.test.mjs).
//
// The guest sends no credential and names no disk: `disk`, from the
// intercept's props as its start set them, is the only disk it reaches and
// its epoch the only life it writes under. Bytes stream through: a get is
// R2's body handed on, a put the request's body into R2, never read here.
// The head is the Computer DO's, moved only by the head's rules
// (`fragment_core::computer::fpac::decide`): `postHead` carries a post to
// it. Beside FPAC's contract, the platform's own `POST /unusable {why, blank, unmountable}`:
// its boot's word that the head it was handed will not open, carried by
// `postUnusable` with this life's epoch and that head, so the start it
// fails is this one alone. `rules` are the Rust side's, one source of each: the name's grammar
// (`nameEpoch`) and the bounds.

const RANGE = /^bytes=(\d{1,15})-(\d{1,15})$/;
const SHA256_HEX = /^[0-9a-f]{64}$/;
// a boot's report that its head will not open, at most
const UNUSABLE_MAX_BYTES = 4096;

const hex = (buf) => [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, "0")).join("");

function text(status, body, headers = {}) {
  return new Response(body, { status, headers: { "content-type": "text/plain", ...headers } });
}

// The body of a small post, at most `max` bytes, read as it comes: one that
// passes it is refused before more is read, whatever it says of its length
// (a stream need name none). Null past `max`.
async function smallBody(request, max) {
  const declared = request.headers.get("content-length");
  if (declared !== null && !(Number(declared) <= max)) return null;
  if (!request.body) return "";
  const reader = request.body.getReader();
  const chunks = [];
  let n = 0;
  // bounded: it stops once `max` is passed
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    n += value.byteLength;
    if (n > max) {
      reader.cancel().catch(() => {});
      return null;
    }
    chunks.push(value);
  }
  const all = new Uint8Array(n);
  let at = 0;
  for (const c of chunks) {
    all.set(c, at);
    at += c.byteLength;
  }
  return new TextDecoder().decode(all);
}

// Answers the guest's call `request` for `disk` ({ prefix, name, epoch,
// volume, head }) from `bucket` (R2: ranged get, put-if-absent, head).
export async function fpacRoute(request, bucket, disk, rules, postHead, postUnusable) {
  const url = new URL(request.url);
  const route = `${request.method} ${url.pathname}`;
  // a test fleet's outage of its storage (the lever `fpac-refuse-at-stop`,
  // set in its props at a stop's signal): every call refused, as R2 down
  // answers, until then; FPAC tries again (FIN-783)
  if (disk.refuseUntil && Date.now() < disk.refuseUntil) {
    await request.body?.cancel();
    return text(503, "the disk's storage is unreachable (the test lever's outage)\n");
  }
  if (route === "GET /open") {
    // what the platform gave the route at the start: no storage read
    return text(200, "", { "x-fpac-disk": disk.name, "x-fpac-epoch": String(disk.epoch), "x-fpac-volume-bytes": String(disk.volume), "x-fpac-head": disk.head || "none" });
  }
  if (route === "POST /head") {
    const line = await smallBody(request, rules.headLineMax);
    if (line === null) return text(413, "a head's line past its limit\n");
    const a = await postHead(disk.epoch, line);
    if (a.answer) return text(200, "", { "x-fpac-head-answer": a.answer });
    return text(a.status || 409, `${a.refused || "refused"}\n`);
  }
  if (route === "POST /unusable") {
    const body = await smallBody(request, UNUSABLE_MAX_BYTES);
    if (body === null) return text(413, "a report past its limit\n");
    let said;
    try {
      said = JSON.parse(body);
    } catch {
      said = {};
    }
    const why = said && said.why;
    if (typeof why !== "string" || why.length > UNUSABLE_MAX_BYTES) return text(400, "say why ({why})\n");
    // what its boot found: its device blank (nothing of it ever shipped), or
    // its filesystem unmountable (a format cut short); the platform decides
    // whether it starts as a new disk (`fragment_core::computer::fpac::fallback`)
    const a = await postUnusable(disk.epoch, disk.head || null, why, said.blank === true, said.unmountable === true);
    return new Response(JSON.stringify(a), { status: 200, headers: { "content-type": "application/json" } });
  }
  const name = url.pathname.startsWith("/o/") ? url.pathname.slice(3) : null;
  if (name === null) return text(404, "no such route\n");
  const epoch = rules.nameEpoch(name);
  if (epoch === null || epoch === undefined) return text(400, "a name outside the grammar\n");
  const key = `${disk.prefix}${name}`;
  if (request.method === "GET") return get(bucket, key, request.headers.get("range"));
  if (request.method === "PUT") {
    // only this life writes, and only under its own epoch
    if (epoch !== disk.epoch) return text(403, "a put names another life's epoch\n");
    return put(bucket, key, request, rules.objectBytesMax);
  }
  return text(405, "GET or PUT\n");
}

// A whole object, its SHA-256 named (the client checks the bytes against
// it), or one range of it: the only form `bytes=<first>-<last>`, served to
// the object's end at most, one starting past the end refused.
async function get(bucket, key, range) {
  if (range === null) {
    const o = await bucket.get(key);
    if (o === null) return text(404, "absent\n");
    const sum = o.checksums && o.checksums.sha256;
    if (!sum) return text(500, "an object with no checksum\n");
    return new Response(o.body, { status: 200, headers: { "content-length": String(o.size), "x-fpac-sha256": hex(sum) } });
  }
  const r = RANGE.exec(range);
  if (!r || Number(r[2]) < Number(r[1])) return text(400, "a range we do not serve\n");
  const first = Number(r[1]);
  let o;
  try {
    o = await bucket.get(key, { range: { offset: first, length: Number(r[2]) - first + 1 } });
  } catch (e) {
    // R2 refuses a range that starts past the end; any other failure may
    // pass, and must never read as the object's own answer (a 4xx the
    // daemon takes for good, and then the disk for unusable)
    const why = String((e && e.message) || e);
    let there;
    try {
      there = await bucket.head(key);
    } catch {
      return text(503, `the range: ${why}\n`);
    }
    if (there === null) return text(404, "absent\n");
    return text(first >= there.size ? 416 : 503, `the range: ${why}\n`);
  }
  if (o === null) return text(404, "absent\n");
  if (first >= o.size && o.size > 0) return text(416, "the range starts past the end\n");
  const length = Math.max(0, Math.min(Number(r[2]), o.size - 1) - first + 1);
  return new Response(o.body, { status: 206, headers: { "content-length": String(length), "content-range": `bytes ${first}-${first + length - 1}/${o.size}` } });
}

// Put-if-absent. R2 checks the body against the SHA-256 the put names and
// refuses the put when the name is taken; then the stored object's
// checksum tells our own earlier put (200: its answer was lost) from
// another writer's (409: the daemon is fenced).
async function put(bucket, key, request, objectBytesMax) {
  const claimed = request.headers.get("x-fpac-sha256") || "";
  if (!SHA256_HEX.test(claimed)) return text(400, "a put names its SHA-256\n");
  const length = Number(request.headers.get("content-length"));
  if (!Number.isSafeInteger(length) || length < 0) return text(411, "a put names its length\n");
  if (length > objectBytesMax) return text(413, "an object past its limit\n");
  // a stream of the length the request named, so R2 takes it as one
  const fixed = new FixedLengthStream(length);
  // not awaited: a put refused by its condition may leave the body unread
  (request.body || new Response(new Uint8Array(0)).body).pipeTo(fixed.writable).catch(() => {});
  let stored;
  try {
    stored = await bucket.put(key, fixed.readable, { sha256: claimed, onlyIf: new Headers({ "if-none-match": "*" }) });
  } catch (e) {
    const why = String((e && e.message) || e);
    return text(/sha|checksum|digest/i.test(why) ? 400 : 503, `the put: ${why}\n`);
  }
  if (stored !== null) return text(201, "", { "x-fpac-sha256": claimed });
  const there = await bucket.head(key);
  if (there === null || !there.checksums || !there.checksums.sha256) return text(503, "the name was taken, then gone\n");
  const sum = hex(there.checksums.sha256);
  return text(sum === claimed ? 200 : 409, "", { "x-fpac-sha256": sum });
}
