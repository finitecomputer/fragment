# Read-only deployment smoke check

```sh
cargo xtask smoke --url http://127.0.0.1:8790 --expected-release dev --json
```

Choose the platform URL explicitly. There is no default host, config file,
credential lookup, sign-in, or vendor request. The example requires an already
running local platform; the smoke command does not start one. Use an HTTP(S)
origin or its `/healthz` URL, without userinfo, query, fragment, or another path.

The command sends `HEAD /healthz` and requires HTTP 200 plus exactly one valid
`x-fragment-deploy` header (the existing deployment contract in `docs/api.md`).
`--expected-release` optionally requires an exact match, including case and any
dirty suffix; for a clean deployed commit, use `git rev-parse --short=12 HEAD`.
It never follows redirects or retries a failed check. A redirect to sign-in or
another host fails. TLS verification and the environment's proxy settings remain
enabled. `Cache-Control: no-cache` asks intermediaries to revalidate.

`--timeout-ms` bounds the request (1–60000 ms, default 10000); the connect timeout
is the smaller of that limit and 3000 ms. Compilation is outside this timeout.
No response body is read. Raw URLs, transport errors, unrelated response headers,
and malformed release values are omitted from both human and JSON output.

Exit 0 means the check passed. Exit 1 means invalid arguments or a failed check.
`--json` writes one JSON object to stdout, including on argument errors; a failure
also writes a fixed diagnostic to stderr. `--help` prints usage without a request.
A successful example is:

```json
{"schema_version":1,"scope":"health_and_deployment_identity_only","passed":true,"code":"passed","http_status":200,"release":"dev","expected_release":"dev"}
```

The schema always includes these fields; unavailable values are `null`. Consumers
should check `schema_version` and `passed`. Failure codes are `missing_url`,
`invalid_url`, `invalid_arguments`, `invalid_expected_release`, `invalid_timeout`,
`client_failed`, `request_failed`, `request_timeout`, `http_error`, `missing_release`,
`invalid_release`, and `release_mismatch`. An HTTP failure includes its status;
a mismatch includes the validated actual and expected release IDs.

Passing proves only that this request received the platform health and identity
contract. It does not verify sign-in, chat, a model, connected accounts, billing,
computer startup, persistence, or agreement between Worker and Durable Object
releases. A valid header is not an authenticated identity proof. This is one
observation, not a readiness certification or an uptime monitor.

## Quick verification without a deployment

With the repository's Rust dependencies built, this takes under two minutes:

```sh
cargo test --locked -p xtask smoke
cargo run --locked --quiet -p xtask -- smoke --help
```

Expect 6 unit tests and 5 CLI integration tests to pass, and help text ending
with “Passing checks health and deployment identity only, not a product journey.”
A cold Rust build can take longer. All platform responses in these tests are
synthetic: unit fixtures cover validation and result classification, and a local
TCP fixture exercises the real binary and HTTP client, HEAD path, exact match,
mismatch, missing identity, redirects, stalled response timeout, malformed HTTP,
JSON output and exit codes. It uses literal loopback addresses and preserves proxy
settings. No real platform, browser, container, Google, or paid provider is tested.
