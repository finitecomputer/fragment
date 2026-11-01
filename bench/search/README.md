# search-bench

Scores the engines Fragment's search could run on, over one corpus of
markdown and one file of labelled questions. Neither lives in this repo:
a real corpus is someone's brain. Only `fixtures/synthetic/` (an invented
company's wiki) is committed, for the tests.

Its own Cargo workspace, as `images/` is, so LanceDB's dependency tree
stays out of the root build.

## What it compares

Every engine indexes the same sections: the brain's cut
(`templates/brain/applib/sections.mjs`, ported in `src/corpus.rs`) of the
same pages (the brain's `indexable` rule), with the same vectors.

| system | what it is |
|---|---|
| brain today | the brain's search as deployed: FTS5, every query word as a prefix (AND), its BM25 weights, and its 12 MiB index cap (pages past it are not indexed) |
| fbrain lexical / hybrid | Finite Brain's search (finite-brain-cli `search.rs`): FTS5 OR of exact tokens, its weights, the top 50 ranked again by its own BM25 over the corpus; hybrid adds an exact float32 cosine scan (top 50), fused by RRF (k = 60) |
| DO baseline lexical / hybrid | what a fragment could do in its own Durable Object: FTS5 OR of exact tokens with the brain's weights; hybrid adds an int8 vector scan (one byte a dimension, a scale per vector) streamed row by row from SQLite, fused by RRF |
| vectors alone | the exact float32 and int8 scans by themselves |
| LanceDB | LanceDB OSS (`lancedb` 0.40): native BM25F (`CombinedFields`) over title, heading, context and body with the brain's weights, an IVF_FLAT cosine index, and its own hybrid query (RRF, k = 60) |

Scores are over pages: a result is a section, judged by its page; a page's
second section in the top 10 earns nothing. Recall@10, nDCG@10 (gains
2^g − 1) and Success@10 (any relevant page in the top 10) are means over
the questions with an answer; questions without one are counted, not
scored. Latency is each question's median of `--runs` warm searches, then
p50/p95 over questions, without the question's embedding (reported apart:
every hybrid pays it). Freshness edits one page (a new paragraph with a
word no page has), then deletes it, and asks each engine at once whether
its next search sees the change, by that word and by the new section's
vector. LanceDB is asked again after `optimize`.

## Running it

Embeddings come from an OpenAI-compatible `/v1/embeddings` on this
machine: the harness refuses any other host. For example, BGE-M3 (also
Workers AI's `@cf/baai/bge-m3`) on CPU through Hugging Face's TEI:

```sh
docker run -d --name search-bench-tei --memory 10g -p 127.0.0.1:18091:80 \
  -v <models dir>:/data ghcr.io/huggingface/text-embeddings-inference:cpu-1.9.4 \
  --model-id BAAI/bge-m3 --max-batch-tokens 2048 --auto-truncate
cargo build --release
target/release/search-bench embed --corpus <dir> --work <dir> \
  --embeddings-url http://127.0.0.1:18091/v1/embeddings --embeddings-model BAAI/bge-m3
target/release/search-bench run --corpus <dir> --questions <file.jsonl> --work <dir> \
  --embeddings-url http://127.0.0.1:18091/v1/embeddings --embeddings-model BAAI/bge-m3 \
  [--runs 5] [--decoys <n>] [--report <file.json>]
```

`embed` fills the cache (`<work>/embeddings.sqlite3`, by model and input),
so `run` embeds only the questions. `--decoys <n>` adds n made-up sections
with random vectors (`src/decoy.rs`) to time the engines at a larger size.
`--hashed-embeddings <dims>` replaces the model with hashed words, for the
tests alone.

A question file has one JSON object a line:

```json
{"id": "s01", "kind": "paraphrase", "q": "how do we roll back a broken firmware deploy",
 "relevant": {"engineering/deploys.md": 2, "engineering/incidents/2026-09-arm-stall.md": 1}}
```

`relevant` grades pages 2 (answers it) or 1 (helps); empty, the corpus has
no answer. Scores are also given by `kind`.

What `run` prints and `--report` writes are aggregates. The work directory
holds the indexes, the embedding cache and `details.jsonl` (each
question's top results and the pages no judgment covers: the pool to judge
next), which name the corpus's pages: keep it where the corpus is kept.

## The quality gate for a question across fragments (FIN-238)

`across` asks whether searching a brain split into child brains, one per
wiki, finds what one index over the whole brain finds (decision 5: the
federated answer's nDCG@10 within 5% of the union's, and no worse than
Finite Brain's hybrid):

```sh
target/release/search-bench across --corpus <dir> --questions <file.jsonl> --work <dir> \
  --embeddings-url http://127.0.0.1:18091/v1/embeddings --embeddings-model nomic-ai/nomic-embed-text-v1.5 \
  --query-prefix "search_query: " --document-prefix "search_document: " --window 8192 [--report <file.json>]
```

It runs what ships (`src/across.rs`), not the prototypes above:
- **Chunks:** each page is written as the brain writes it (`wire::Upsert`)
  and cut by `fragment_core::searchable::chunked`, at the slot's window
  and document prefix, so each chunk embeds what production embeds.
- **Each fragment's index:** the cell's SearchIndex, step by step. That's
  the same FTS5 table, tokenizer and field weights; BM25's best 100
  chunks; the sign codes against the question over all chunks, the best
  `candidates` reranked by int8, the best 100; and `fuse`.
- **Federated:** the corpus cut by top-level folder, one index each, each
  asked for `asked_limit(10)`, merged by `fragment_core::across::merge`
  (the route's code).
- **Union:** one index over the whole corpus, built for the test.
- **fbrain hybrid:** its lexical search over sections, fused with an exact
  float32 scan (a section by its best chunk).

It prints and writes aggregates, and the verdict. Each question's top
pages by system go to `<work>/across-details.jsonl`: the pool to judge,
which names the corpus's pages.

`cargo test --release` runs the whole pipeline on the synthetic fixtures.
