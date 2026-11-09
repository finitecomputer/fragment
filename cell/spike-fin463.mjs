// SPIKE (FIN-463), never merged: can a computer mount a staged agent's R2
// prefix read-only over FUSE, outside /data, and what is it like?
//
// Everything here is reached only with the branch's test secret
// (`x-fragment-test-secret`), and only on the spike branch's deployment.
//
//   POST /api/test/spike/computer {computer, op, ...}: an op on that
//     computer's container (`spikeComputer`): exec, mount, inspect,
//     unmount, store, running.
//   POST /api/test/spike/r2 {op: "count"|"empty", prefix}: the branch
//     bucket's keys under a prefix, counted or deleted (the teardown's).
//
// Two ways a computer reads a staged tree:
//   1. FUSE: the Sandbox SDK's `S3Mount` (sandbox-shim runs s3fs in the
//      container, aimed at `s3-<route>.sandbox.internal`, an intercepted
//      host). The SDK's own `S3Gateway` signs each request with an R2 S3
//      access key; preview's token cannot mint one, so `SpikeS3Gateway`
//      answers the same S3 subset from the R2 binding instead, with the
//      same policy: one bucket, one key prefix, reads only.
//   2. HTTP ranges: `SpikeStore`, an intercepted host
//      (`spike.fragment.internal`) serving one prefix's objects with
//      Range, and (for seeding the spike only) PUT.
import { WorkerEntrypoint } from "cloudflare:workers";
import { S3Mount } from "@cloudflare/sandbox";

const SECRET_HEADER = "x-fragment-test-secret";

function admitted(request, env) {
  const given = request.headers.get(SECRET_HEADER) || "";
  const want = String(env.FRAGMENT_TEST_SECRET || "").trim();
  if (!want || given.length !== want.length) return false;
  let diff = 0;
  for (let i = 0; i < want.length; i++) diff |= want.charCodeAt(i) ^ given.charCodeAt(i);
  return diff === 0;
}

const json = (value, status = 200) => new Response(JSON.stringify(value), { status, headers: { "content-type": "application/json" } });

// The Worker's spike routes (entry.mjs sends `/api/test/spike/*` here).
export async function spikeFetch(request, env) {
  if (!admitted(request, env) || request.method !== "POST") return new Response("no route", { status: 404 });
  const path = new URL(request.url).pathname;
  const body = await request.json().catch(() => ({}));
  try {
    if (path === "/api/test/spike/computer") {
      if (!/^computer:[0-9a-f]{24}$/.test(String(body.computer))) return json({ error: "name a computer" }, 400);
      const stub = env.COMPUTER.getByName(body.computer);
      return json(await stub.spike(body.op, body));
    }
    if (path === "/api/test/spike/r2") {
      const prefix = String(body.prefix ?? "");
      if (body.op === "count") return json(await countPrefix(env.BLOBS, prefix));
      if (body.op === "empty") return json(await emptyPrefix(env.BLOBS, prefix));
      return json({ error: "op is count or empty" }, 400);
    }
  } catch (e) {
    return json({ error: String((e && e.stack) || e) }, 500);
  }
  return new Response("no route", { status: 404 });
}

async function countPrefix(bucket, prefix) {
  let cursor;
  let objects = 0;
  let bytes = 0;
  for (;;) {
    const page = await bucket.list({ prefix, cursor, limit: 1000 });
    for (const o of page.objects) {
      objects++;
      bytes += o.size;
    }
    if (!page.truncated) break;
    cursor = page.cursor;
  }
  return { prefix, objects, bytes };
}

async function emptyPrefix(bucket, prefix) {
  let deleted = 0;
  const t0 = Date.now();
  // aborted multipart uploads are not listed; they expire on their own
  for (;;) {
    const page = await bucket.list({ prefix, limit: 1000 });
    if (page.objects.length === 0) break;
    await bucket.delete(page.objects.map((o) => o.key));
    deleted += page.objects.length;
    if (Date.now() - t0 > 20_000) return { prefix, deleted, more: true };
  }
  return { prefix, deleted, more: false };
}

// Reads a stream's last `keep` bytes.
async function tail(stream, keep) {
  if (!stream) return "";
  let last = new Uint8Array(0);
  for await (const chunk of stream) {
    const both = new Uint8Array(last.length + chunk.length);
    both.set(last);
    both.set(chunk, last.length);
    last = both.slice(-keep);
  }
  return new TextDecoder().decode(last);
}

const SPIKE_BUCKET = "spike";
const SPIKE_ENDPOINT = "https://r2-binding.invalid/";

// An op on a computer's container (entry.mjs's Computer `spike`).
export async function spikeComputer(ctx, op, args) {
  const c = ctx.container;
  const t0 = Date.now();
  const took = (value) => ({ ...value, ms: Date.now() - t0 });
  switch (op) {
    case "running":
      return took({ running: c.running });
    case "exec": {
      const ms = Math.min(Number(args.ms) || 60_000, 90_000);
      const kill = new AbortController();
      const timer = setTimeout(() => kill.abort(), ms);
      try {
        const proc = await c.exec(args.argv, { stderr: "combined", signal: kill.signal });
        const output = await tail(proc.stdout, Number(args.keep) || 16384);
        return took({ exitCode: await proc.exitCode, output });
      } finally {
        clearTimeout(timer);
      }
    }
    case "mount": {
      const mount = new S3Mount(c, ctx.exports.SpikeS3Gateway);
      await mount.mount({
        mountPath: args.mountPath,
        source: {
          type: "s3",
          endpoint: SPIKE_ENDPOINT,
          region: "auto",
          bucket: SPIKE_BUCKET,
          // unused: SpikeS3Gateway reads through the R2 binding
          credentials: { type: "static", accessKeyId: "unused", secretAccessKey: "unused" },
        },
        keyPrefix: args.keyPrefix,
        access: args.access || "read-only",
        s3fsOptions: args.s3fsOptions,
      });
      return took({ mounted: args.mountPath });
    }
    case "inspect":
      return took({ inspection: await new S3Mount(c, ctx.exports.SpikeS3Gateway).inspect(args.mountPath) });
    case "unmount":
      await new S3Mount(c, ctx.exports.SpikeS3Gateway).unmount(args.mountPath);
      return took({ unmounted: args.mountPath });
    case "store":
      // the fallback: one prefix by HTTP ranges, at spike.fragment.internal
      await c.interceptOutboundHttp("spike.fragment.internal", ctx.exports.SpikeStore({ props: { prefix: args.prefix, writable: !!args.writable } }));
      return took({ store: args.prefix, writable: !!args.writable });
    default:
      return took({ error: `no op ${op}` });
  }
}

// --- HTTP ranges ---------------------------------------------------------

function parseRange(header, size) {
  if (!header) return null;
  const m = /^bytes=(\d*)-(\d*)$/.exec(header.trim());
  if (!m || (m[1] === "" && m[2] === "")) return { bad: true };
  if (m[1] === "") {
    const n = Math.min(Number(m[2]), size);
    return { offset: size - n, length: n };
  }
  const start = Number(m[1]);
  if (start >= size) return { bad: true };
  const end = m[2] === "" ? size - 1 : Math.min(Number(m[2]), size - 1);
  if (end < start) return { bad: true };
  return { offset: start, length: end - start + 1 };
}

// An object's answer for GET or HEAD, with Range, as S3 answers it.
async function objectAnswer(bucket, key, request, extraHeaders = {}) {
  const head = await bucket.head(key);
  if (!head) return null;
  const size = head.size;
  const range = parseRange(request.headers.get("range"), size);
  const headers = new Headers({
    "accept-ranges": "bytes",
    etag: head.httpEtag,
    "last-modified": head.uploaded.toUTCString(),
    "content-type": head.httpMetadata?.contentType || "application/octet-stream",
    ...extraHeaders,
  });
  if (range && range.bad) {
    headers.set("content-range", `bytes */${size}`);
    return new Response(null, { status: 416, headers });
  }
  const isHead = request.method === "HEAD";
  if (range) {
    headers.set("content-range", `bytes ${range.offset}-${range.offset + range.length - 1}/${size}`);
    headers.set("content-length", String(range.length));
    if (isHead) return new Response(null, { status: 206, headers });
    const obj = await bucket.get(key, { range: { offset: range.offset, length: range.length } });
    if (!obj) return null;
    return new Response(obj.body, { status: 206, headers });
  }
  headers.set("content-length", String(size));
  if (isHead) return new Response(null, { status: 200, headers });
  const obj = await bucket.get(key);
  if (!obj) return null;
  return new Response(obj.body, { status: 200, headers });
}

// The fallback's store: one prefix of the branch bucket, by HTTP ranges.
//   GET|HEAD /o/<key>   the object (Range honoured)
//   GET /list?after=…   a page of keys and sizes, JSON
//   PUT /o/<key>        (spike seeding only, `writable`) the body, stored
export class SpikeStore extends WorkerEntrypoint {
  async fetch(request) {
    const { prefix, writable } = this.ctx.props;
    const url = new URL(request.url);
    const bucket = this.env.BLOBS;
    if (url.pathname.startsWith("/o/")) {
      const rel = decodeURIComponent(url.pathname.slice(3));
      if (!rel || rel.split("/").some((s) => s === "" || s === "." || s === "..")) return new Response("bad key", { status: 400 });
      const key = prefix + rel;
      if (request.method === "GET" || request.method === "HEAD") {
        return (await objectAnswer(bucket, key, request)) || new Response("not found", { status: 404 });
      }
      if (request.method === "PUT" && writable) {
        const put = await bucket.put(key, request.body);
        return json({ key: rel, size: put.size });
      }
      return new Response("read only", { status: 405 });
    }
    if (url.pathname === "/list" && request.method === "GET") {
      const page = await bucket.list({ prefix: prefix + (url.searchParams.get("prefix") || ""), cursor: url.searchParams.get("cursor") || undefined, limit: 1000 });
      return json({ objects: page.objects.map((o) => [o.key.slice(prefix.length), o.size]), cursor: page.truncated ? page.cursor : null });
    }
    return new Response("no route", { status: 404 });
  }
}

// --- the S3 subset s3fs reads, from the R2 binding ----------------------

const INSPECTION_VERSION_HEADER = "x-sandbox-s3-gateway-version";
const INSPECTION_RESULT_HEADER = "x-sandbox-s3-inspection-result";
const INSPECTION_DETAIL_HEADER = "x-sandbox-s3-inspection-detail";

function inspection(result, detail, status) {
  return new Response(null, {
    status,
    headers: { [INSPECTION_VERSION_HEADER]: "1", [INSPECTION_RESULT_HEADER]: result, [INSPECTION_DETAIL_HEADER]: encodeURIComponent(detail.slice(0, 1024)) },
  });
}

const xmlEscape = (s) => String(s).replace(/[<>&'"]/g, (c) => ({ "<": "&lt;", ">": "&gt;", "&": "&amp;", "'": "&apos;", '"': "&quot;" })[c]);
const s3Error = (status, code, message) =>
  new Response(`<?xml version="1.0" encoding="UTF-8"?><Error><Code>${code}</Code><Message>${xmlEscape(message)}</Message></Error>`, { status, headers: { "content-type": "application/xml" } });

const LIST_PARAMS = new Set(["delimiter", "encoding-type", "marker", "max-keys", "prefix", "list-type", "continuation-token", "fetch-owner", "start-after"]);

export class SpikeS3Gateway extends WorkerEntrypoint {
  async fetch(request) {
    const props = this.ctx.props;
    if (props.protocolVersion !== 1) return inspection("gateway-protocol", "gateway protocol is incompatible", 500);
    if (props.mode === "deny") return inspection("rejected-access", "mount route has been revoked", 403);
    const url = new URL(request.url);
    if (url.hostname !== `s3-${props.routeId}.sandbox.internal`) return inspection("rejected-access", "request host does not match the mount route", 403);
    const method = request.method.toUpperCase();
    // reads only, whatever the mount's access says: a staged tree is never written
    if (method !== "GET" && method !== "HEAD") return inspection("rejected-access", "the mount is read-only", 403);
    const sep = url.pathname.indexOf("/", 1);
    const bucketName = decodeURIComponent(sep === -1 ? url.pathname.slice(1) : url.pathname.slice(1, sep));
    const key = sep === -1 ? "" : decodeURIComponent(url.pathname.slice(sep + 1));
    if (bucketName !== props.source.bucket) return inspection("rejected-access", "request is outside the mounted bucket", 403);
    const prefix = props.keyPrefix || "";
    const bucket = this.env.BLOBS;
    if (key === "") {
      if (method === "HEAD") return new Response(null, { status: 200 });
      const p = url.searchParams;
      if (p.has("location")) return new Response(`<?xml version="1.0" encoding="UTF-8"?><LocationConstraint xmlns="http://s3.amazonaws.com/doc/2006-03-01/">auto</LocationConstraint>`, { headers: { "content-type": "application/xml" } });
      for (const name of p.keys()) if (!LIST_PARAMS.has(name)) return inspection("rejected-access", "request is not a supported s3fs operation", 403);
      const listPrefix = p.get("prefix") ?? "";
      if (prefix && !(listPrefix === prefix || listPrefix.startsWith(prefix))) return inspection("rejected-access", "list request is outside the mounted key prefix", 403);
      if (request.headers.get("user-agent") === "sandbox-shim/1" && p.get("list-type") === "2" && p.get("max-keys") === "1" && listPrefix === (props.keyPrefix ?? "")) {
        return inspection("usable", "R2 binding", 200);
      }
      return this.list(bucket, p, listPrefix);
    }
    if (prefix && !(key === prefix.slice(0, -1) || key.startsWith(prefix))) return inspection("rejected-access", "request is outside the mounted key prefix", 403);
    if ([...url.searchParams.keys()].length) return inspection("rejected-access", "request is not a supported s3fs operation", 403);
    const answer = await objectAnswer(bucket, key, request);
    if (answer) return answer;
    return method === "HEAD" ? new Response(null, { status: 404 }) : s3Error(404, "NoSuchKey", "The specified key does not exist.");
  }

  async list(bucket, p, listPrefix) {
    const v2 = p.get("list-type") === "2";
    const delimiter = p.get("delimiter") || undefined;
    const maxKeys = Math.min(Math.max(Number(p.get("max-keys") ?? 1000) || 0, 0), 1000);
    const urlEncode = p.get("encoding-type") === "url";
    const enc = (s) => xmlEscape(urlEncode ? encodeURIComponent(s).replace(/%2F/g, "/") : s);
    const token = v2 ? p.get("continuation-token") : null;
    const startAfter = v2 ? p.get("start-after") : p.get("marker");
    const opts = { prefix: listPrefix, limit: Math.max(maxKeys, 1) };
    if (delimiter) opts.delimiter = delimiter;
    if (token) opts.cursor = token;
    else if (startAfter) opts.startAfter = startAfter;
    const page = maxKeys === 0 ? { objects: [], delimitedPrefixes: [], truncated: false } : await bucket.list(opts);
    const prefixes = page.delimitedPrefixes || [];
    let xml = `<?xml version="1.0" encoding="UTF-8"?><ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Name>${xmlEscape(p.get("bucket") || SPIKE_BUCKET)}</Name><Prefix>${enc(listPrefix)}</Prefix><MaxKeys>${maxKeys}</MaxKeys>`;
    if (delimiter) xml += `<Delimiter>${enc(delimiter)}</Delimiter>`;
    if (urlEncode) xml += `<EncodingType>url</EncodingType>`;
    xml += `<IsTruncated>${page.truncated ? "true" : "false"}</IsTruncated>`;
    if (v2) {
      xml += `<KeyCount>${page.objects.length + prefixes.length}</KeyCount>`;
      if (token) xml += `<ContinuationToken>${xmlEscape(token)}</ContinuationToken>`;
      if (page.truncated) xml += `<NextContinuationToken>${xmlEscape(page.cursor)}</NextContinuationToken>`;
    } else {
      xml += `<Marker>${enc(startAfter || "")}</Marker>`;
      if (page.truncated) {
        // V1 pages by key: the last key or prefix this page named
        const last = [...page.objects.map((o) => o.key), ...prefixes].sort().pop();
        if (last) xml += `<NextMarker>${enc(last)}</NextMarker>`;
      }
    }
    for (const o of page.objects) {
      xml += `<Contents><Key>${enc(o.key)}</Key><LastModified>${o.uploaded.toISOString()}</LastModified><ETag>${xmlEscape(o.httpEtag)}</ETag><Size>${o.size}</Size><StorageClass>STANDARD</StorageClass></Contents>`;
    }
    for (const cp of prefixes) xml += `<CommonPrefixes><Prefix>${enc(cp)}</Prefix></CommonPrefixes>`;
    xml += `</ListBucketResult>`;
    return new Response(xml, { headers: { "content-type": "application/xml" } });
  }
}
