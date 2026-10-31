# fragment-search

Fragment's search service: a fragment's sections, and their chunks'
vectors, in [LanceDB](https://lancedb.com) OSS. The data lives on object
storage (R2 on Cloudflare, any S3-compatible store self-hosted) or on a
disk. One image serves both deployments; only its storage URI and keys
change. Why LanceDB is measured in `bench/search` (#771).

The platform calls it. The service knows no fragment, person or model:
- a **scope** is a table's name, which the platform makes from a
  fragment's identity, incarnation and read scope;
- a **space** names the vectors' model, dimensions and version.

Authorization, embedding and metering stay in the cell. The service is
the index the cell asks.

## The API

Every path but `GET /healthz` takes the deployment's token as
`Authorization: Bearer <token>`. Bodies and answers are JSON. A refusal is
`{error, message}`, with the status code below.

| call | body → answer |
| --- | --- |
| `POST /v1/{scope}/upsert` | `{source, revision, replaces?, title?, at?, fields?, space, sections}` → `{source, revision, written, chunks, version}` |
| `POST /v1/{scope}/delete` | `{source}` → `{source, deleted}` |
| `POST /v1/{scope}/known` | `{hashes}` → `{known}`: the hashes it holds a vector of |
| `GET /v1/{scope}/sources?after=&limit=` | → `{sources: [{source, revision, at, chunks}], next}`, ascending by source |
| `GET /v1/{scope}/status` | → `{exists, chunks, sources, bytes, space, semantic, version}` |
| `POST /v1/{scope}/search` | `{q?, vector?, space?, limit?, mode?, fields?, after?, before?}` → `{results}` |
| `DELETE /v1/{scope}` | → `{dropped}` |

A scope is 1 to 128 of `a-z`, `0-9`, `-` and `_`, starting with a letter
or digit.

**Upsert** replaces one source whole, in one commit. A search sees its
old revision or its new one, never part of each.
- `sections` is `[{id, heading?, context?, at?, location?, chunks}]`.
- Each chunk is `{text, hash, vector?}`:
  - `hash` is the text's sha256 (64 lowercase hex);
  - `vector` is little-endian float32s, base64. Without one, the table's
    own vector of that hash is used (`known` says which it holds); a hash
    it has none of is 409 `missing_vectors`.
- A section's vectors are its chunks'. The platform cuts a section into
  chunks its embedding model reads whole, and a search finds the section
  by its best chunk.
- The same `(source, revision)` again writes nothing (`written: false`).
- `replaces` is a compare-and-set: the write happens only if the
  source's revision now is `replaces` (`""` means it has none), else
  409 `conflict` with `current`.
- `space` (`{model, dimensions, version?}`, or `null` for a table of
  words alone) is pinned by the table's first write. A write or search in
  another space is 409 `space_changed`: drop the table and write it
  again. Vectors of two spaces are never compared.
- `fields` is up to 4 `name: value` filters (a source's kind, say).
- `sections: []` empties the source and keeps its revision: its replay
  writes nothing, and a later write `replaces` it. `delete` removes the
  source and its revision.

**Search** finds:
- sections whose chunks hold the words: BM25F over title, heading,
  context and text, weighted 4, 3, 2 and 1;
- sections whose chunks are near the vector, by cosine.

Each leg brings 100 chunks. Sections are ranked within each leg by their
best chunk and fused by reciprocal rank (k = 60). `mode` is `hybrid` (the
default), `lexical` or `semantic`. A hit is:

```
{rank, source, section, revision, title, heading, context, at, location,
 snippet, score, signals}
```

`snippet` is at most 300 bytes of its best chunk, around the words;
`signals` says which legs found it. The newest writes are searched before
they are indexed (the unindexed tail).

**Limits** (src/api.rs):

| limit | value |
| --- | --- |
| a source | 2,000 sections; 8,000 chunks an upsert |
| a section | 32 chunks of 32 KiB |
| fields | 4 |
| a question | 1 KiB, 1 to 50 results |
| a sources page | 1,000 |
| a body | 64 MiB |
| a table | 2,000,000 chunks (507 `full` past it) |

## Storage, and what leaves it

Writes to one scope take turns. One service should hold a scope: the
platform routes each scope to one instance, and reads never wait.

**Deleted bytes are purged.** A write that removed rows (a delete, or a
source's old revision) starts maintenance: passes 75 seconds apart, each
holding the table's writes, until a pass leaves one version deleting no
row. A pass:
1. rewrites every file holding a deleted row without it (Lance's own
   threshold, a tenth of a file, would keep a few rows' bytes for good);
2. rebuilds the text indexes from the rows left, after a removal (an
   index keeps a removed row's words otherwise), else folds new rows in;
3. prunes the versions older than a minute, with every file only they
   held.

So deleted text leaves the store's files within about three minutes. A
pass that fails is retried, waiting twice as long each time (at most 15
minutes). A write during maintenance gets a pass after it. The store
itself records what is left to do (more than one version, or a deletion
not yet rewritten): after a restart, the service opens every table and
finishes it. `tests/service.rs` checks the files themselves after a
delete of a twentieth of a file, a delete between passes, a restart and
a failed pass. Index files hold words encoded, so for them the check is
the rebuild: an index built after a removal never held the removed row.

**A first write** makes the table and its text indexes. If it is cut off
between them, nothing was acknowledged; its retry, or the table's next
open, finishes the indexes before the table is searched.

**Indexes:**
- Every 32 writes, the indexes are brought up to date.
- Past 100,000 chunks, a table's vectors get an IVF_FLAT index by cosine.
  Below that, a search compares them all, which is exact and takes a few
  milliseconds (bench/search).

On S3 or R2, commits are conditional puts (`If-None-Match`), so no lock
service is needed.

## Running it

The environment:

| variable | what it is |
| --- | --- |
| `SEARCH_STORAGE` | a directory, or `s3://<bucket>/<prefix>` |
| `SEARCH_STORAGE_<OPTION>` | each of the store's options, its name lowercased (`SEARCH_STORAGE_AWS_ENDPOINT` → `aws_endpoint`) |
| `SEARCH_TOKEN` | the bearer token, at least 32 bytes |
| `SEARCH_LISTEN` | `0.0.0.0:8080` unless named |
| `<any of these>_FILE` | the value read from that file instead (a secret mounted in, as Docker's are): `SEARCH_TOKEN_FILE`, `SEARCH_STORAGE_AWS_SECRET_ACCESS_KEY_FILE` |

### Self-hosted, beside celld

1. Make a token: `openssl rand -hex 32`, kept where the deployment keeps
   its secrets.
2. Choose a store.
   - On one machine, a directory on a persistent disk:
     ```sh
     docker run -d --name fragment-search --restart unless-stopped \
       -p 127.0.0.1:8080:8080 -v /var/lib/fragment-search:/data \
       -e SEARCH_STORAGE=/data -e SEARCH_TOKEN=<token> fragment-search
     ```
     The image runs as `nobody` (65534), so `chown 65534 /var/lib/fragment-search`.
   - On the company's S3-compatible store (a bucket made beforehand):
     ```sh
     docker run -d --name fragment-search --restart unless-stopped -p 127.0.0.1:8080:8080 \
       -e SEARCH_STORAGE=s3://<bucket>/fragment-search -e SEARCH_TOKEN=<token> \
       -e SEARCH_STORAGE_AWS_ENDPOINT=https://s3.internal.example \
       -e SEARCH_STORAGE_AWS_REGION=<region> \
       -e SEARCH_STORAGE_AWS_ACCESS_KEY_ID=<id> -e SEARCH_STORAGE_AWS_SECRET_ACCESS_KEY=<secret> \
       fragment-search
     ```
     Add `SEARCH_STORAGE_ALLOW_HTTP=true` only for an endpoint on plain
     http on the machine itself.
3. Point the platform at it: its search URL and token, in the deployment's
   config (the cell's capability).
4. Its embeddings come from the deployment's embeddings slot. On a
   self-hosted deployment, that is an OpenAI-compatible model of its own
   (docs/api.md, Models), such as Hugging Face's TEI with BGE-M3. No call
   leaves the network.
5. Check it: `curl -s 127.0.0.1:8080/healthz`.

Moving a deployment's search:
- **Copy** its prefix (manifests, data and indexes) to the new store while
  no write is made, then point the platform at it.
- **Or rebuild:** start empty and let each fragment's source write its
  sources again, as a source writes its index anyway (the Session
  Archive's import reconciles by `sources`).
- **A new embedding model** is a new space. Its tables are dropped and
  written again; vectors are never carried across spaces.

### On Cloudflare

The platform runs this image in Containers, its store an R2 bucket
through R2's S3 API:
- `SEARCH_STORAGE=s3://<bucket>/search`
- `SEARCH_STORAGE_AWS_ENDPOINT=https://<account>.r2.cloudflarestorage.com`
- `SEARCH_STORAGE_AWS_REGION=auto`
- an R2 API token's key pair

The cell's capability makes the scopes, checks who may search, embeds,
meters, and routes each scope to one instance.

## Tests

```sh
cargo test --release
```

This covers limits, fusion and snippets, then the service on a directory:
- writes searched by words and by meaning;
- a replay that writes nothing, and a stale write refused;
- vectors reused by hash and never mixed across spaces;
- a restart that reads what was written, and sources paging in order;
- deleted text gone from the files, and a drop;
- the token required.

`on_an_object_store` runs the same against S3:

```sh
SEARCH_TEST_STORAGE=s3://<bucket>/test SEARCH_TEST_STORAGE_AWS_ENDPOINT=… \
SEARCH_TEST_STORAGE_AWS_ACCESS_KEY_ID=… SEARCH_TEST_STORAGE_AWS_SECRET_ACCESS_KEY=… \
SEARCH_TEST_STORAGE_AWS_REGION=… cargo test --release --test service on_an_object_store -- --ignored --nocapture
```

Each key may be a file instead (`SEARCH_TEST_STORAGE_AWS_SECRET_ACCESS_KEY_FILE=<path>`, mode 600), so it is
never on a command line. The test prints how long each step took.

It has passed against SeaweedFS's S3 gateway; R2's turn comes with the
capability (it is a paid store).
