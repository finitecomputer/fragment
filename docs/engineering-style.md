# Engineering Style

Finite borrows Tiger Style — TigerBeetle's engineering discipline — where it
fits our risk profile: safety and performance are design decisions, not
afterthoughts, and velocity comes from doing things right the first time.

Sources:

- TigerStyle: https://github.com/tigerbeetle/tigerbeetle/blob/main/docs/TIGER_STYLE.md
- Paired assertions: https://tigerbeetle.com/blog/2023-12-27-it-takes-two-to-contract
- Joran Dirk Greef on TigerStyle, Wookash Podcast (2026-09): https://www.youtube.com/watch?v=NjiIGm1988U

Sections 1–7 are the shared Finite contract for every component. Section 8 is
the Finite Chat addendum; other components keep their own addenda in their
own docs.

## 1. Posture

- Safety is architecture, not bricks. There is no safe language; memory
  safety is table stakes. Do not ask of a language what only design can
  solve: design for faulty components, lying disks, and lost messages, then
  repair end-to-end.
- Spend time today to save time tomorrow. Time compounds: an hour of design
  or one explicit limit beats weeks of production firefighting. Doing it
  right the first time is the fast path, not the slow one.
- Zero unledgered technical debt. We do it right the first time; what we
  ship is solid, and momentum is the reward. Shortcuts are allowed only
  through the debt ledger, each with an observed source, risk, first proof,
  and delete condition. A shortcut without a delete condition is unfinished
  design, not debt.
- Be uncrashable within explicit limits, and crash when you should. The
  industry has this backwards: it crashes when it shouldn't (unchecked
  allocation) and keeps running when it shouldn't (assertions compiled out
  of production). Wear the seatbelt on the racetrack: assertions stay on in
  release builds.

## 2. Safety

- Simple, explicit control flow only. No recursion in protocol, storage,
  sync, or state-machine code; state machines are iterative and visibly
  bounded. In validation code, prefer explicit branches over clever `Option`
  or iterator chaining — the branch structure should show the safety
  property being enforced.
- Put a limit on everything: loops, queues, batches, payloads, fanout,
  retries. Most loops get an asserted upper bound; hitting it means a bug,
  and a crash with a stack trace beats a deadlock. Where progress matters,
  assert a lower bound too. If a loop is intentionally unbounded because it
  consumes a bounded source, say why beside the loop.
- Explicitly sized domain types at boundaries (`u32`/`u64` for protocol
  numbers, counters); `usize` stays inside local indexing.
- Assertions detect programmer errors; handled errors cover operating
  errors. Expected bad input, timeouts, and conflicts are values the caller
  can match. Contradictions — state that should be impossible — are
  corruption: crash or raise a corruption error; never continue when
  continuing would make state untrustworthy.
- Average two invariant checks per nontrivial function: one near ingress for
  the assumptions consumed, one near egress for the state or value produced.
  Pair important assertions: check data before writing it, again after
  reading it back. Split compound assertions so each names one exact
  invariant. Pure encode/decode helpers may rely on type exhaustiveness,
  but must reject impossible external values explicitly.
- Types and assertions are complements: types check the compile-time
  structure, assertions check the runtime flow of logic and state through
  it. Use both. Assert the positive space (the contract holds) and set
  tripwires in the negative space (breach) — you cannot enumerate every gap
  in the contract, but you can cheaply assert "we are never here." Also
  assert compile-time relationships between constants; those are design
  checks that run before the program does.
- State invariants positively: `if value_is_valid { ... } else { ... }`
  beats negated forms.
- Handle every error. Typed error enums at crate boundaries so callers match
  errors without parsing strings; no `anyhow` in protocol, engine, or store
  code; no hiding database or corruption errors behind `Option`.
- Declare variables at the smallest useful scope; compute values close to
  where they are used.
- Keep the build at the strictest practical warning setting
  (`cargo clippy --all-targets -- -D warnings`), from day one.
- Run at your own pace. Never do irreversible work directly in reaction to
  external events: validate, persist, then interpret from your own ordered
  state. This keeps control flow yours, keeps work bounded, and lets you
  batch.
- Always say why. Comments are sentences of prose explaining surprising
  constraints, explicit limits, schema choices, and security-relevant
  branches. Pass important options explicitly at call sites; library
  defaults are latent bugs waiting to change.

## 3. Correct state

- Authoritative server state lives in schema, constraints, and transactions.
  No JSON blobs for state the server must query, lock, constrain, or
  recover. JSON is for wire DTOs, encrypted payloads, and bounded typed
  replay values such as idempotency responses.
- Separate the control plane from the data plane. That boundary is where
  batching lives, and batching is what lets assertion safety and performance
  coexist.
- Treat cache invalidation as a protocol decision: any derived cache names
  its source of truth, invalidation trigger, and stale-read behavior.
- Give independently ordered projections independent sequence authority: a
  late bootstrap, duplicate, or replay must not regress a newer value with
  an older one.
- Selection, display order, timestamps, and identifier order never confer
  authority to rewrite durable state. Ambiguous state fails closed, without
  mutation.
- Durable protocol convergence — bounded sync, replay of already-authorized
  events after reconnect — is ordinary state-machine work, not a migration
  or repair hack. Conversely, never dress a production hypothesis up as a
  migration: read-only evidence, reproduce, prove the mutation on synthetic
  state, explicit authorization, named backup and rollback boundary.

## 4. Performance

- Sketch before you build: back-of-the-envelope estimates over the four
  resources (network, disk, memory, CPU) and two characteristics (bandwidth,
  latency). Sketches are cheap and land within 90% of the achievable
  maximum. The best time to get the 1000x wins is the design phase —
  exactly when you cannot profile.
- Optimize the slowest resource first — network, disk, memory, CPU — after
  adjusting for how often each is used.
- Respect the sequential portion. Amdahl's law: the contended, unsplittable
  part of the workload dominates throughput no matter how much you
  parallelize the rest. A single core left alone routinely beats a cluster
  reassembling contended state across a network. Don't shard the hot
  contended entity; batch it.
- Batch to amortize: one transaction per mutation, many items per message,
  and never hold a lock or open transaction across the network.
- Copies are not free. Memory bandwidth is the scarce resource; each copy
  thrashes caches and halves effective per-core throughput. Prefer borrowed
  slices, bounded buffers, zero-copy / bring-your-own-buffer interfaces.
  Allocate scratch near the boundary, not deep in helpers; bound result
  sets so vectors cannot grow with history.
- Integers, not floats, for counts and money. Numbers, not strings, at
  persistence boundaries.
- Extract visible hot loops into standalone functions with primitive
  arguments, so the compiler keeps them in registers and the reader spots
  redundant work.
- Keep a sniff test for the whole system. Finite Chat's: a laptop SQLite
  dev server handles hundreds of small appends per second, a Postgres
  canary design plausibly thousands — otherwise assume accidental
  complexity until proven otherwise.

## 5. Developer experience

- Get the nouns and verbs right; great names show you understand the
  domain. Prefer nouns over participles so a name works as a document
  header. Don't overload one name with context-dependent meanings.
- Units and qualifiers last, most significant word first: `latency_ms_max`,
  so `latency_ms_min` sorts beside it. Give related names equal length so
  they align (`source`/`target`, not `src`/`dest`).
- Say why in comments; say how in tests (a preamble stating goal and
  method). Write commit messages that inform and delight: a PR description
  is invisible to `git blame`; the commit message is forever.
- Keep signatures dimension-poor: few parameters, simple returns, few
  failure branches — dimensionality propagates virally through call chains.
- Keep functions to a screen and centralize control flow: the parent
  function owns branching and state mutation ("push `if`s up and `for`s
  down"); helpers validate, compute, or persist exactly one change, and
  leaf helpers stay pure.
- Audit every dependency before adding it; each is supply-chain surface and
  locked-in design. Prefer the standard library and existing workspace
  crates. New scripts are Rust binaries or tests, not shell or Python,
  unless a non-Rust tool is clearly better.
- Great APIs have limits, and the limits are documented — that is what
  makes them a joy. Make limits operator-tunable where useful (CLI flags,
  like cgroups in your own software).

## 6. Testing

- Test both spaces. Every mutation gets a valid path and an invalid/replay
  path. Every idempotent mutation gets success replay, rejected replay, and
  conflicting-body tests. Every storage invariant gets a restart test. Test
  valid data, invalid data, and data becoming invalid — the valid/invalid
  boundary is where the bugs live.
- Make the system logical, then simulate it deterministically. Keep the
  core pure logic; let the physical world in only through narrow gates —
  storage, network-as-message-passing, clock — and shim those in tests.
  Determinism turns multi-year heisenbugs into same-day fixes: fast-forward
  to the failure, rewind to the cause.
- Scope simulation honestly: fuzz components with clear interfaces;
  deterministically simulate the whole-system protocol. Don't shim POSIX or
  TCP — narrow your own gate instead, and fuzz the gate.
- A fuzzer proves the presence of bugs, never their absence. Build the
  mental model first, encode it as assertions, explain it in comments, then
  let simulation hunt for what the model missed.
- Prove the product, not a harness. E2E tests drive the real server the way
  real clients would. When a product-shaped path is blocked, name the
  blocker precisely; don't quietly substitute a fake.

## 7. Process

- Prefer hard cuts over compatibility shadow paths while pre-release: no
  duplicate old/new APIs, no launch/test-only shims, no migrations to
  preserve pre-release dev state. Rewrite callers and tests; reset dev
  state only through the one documented reset command that deletes whole
  explicit roots. Hard-cut the domain model before building product proof
  on it — never wrap a harness around a known-wrong shape.
- No automatic telemetry or diagnostic upload to compensate for weak
  debugging; ship explicit local diagnostics export instead.
- In planning and grilling sessions, ask the user for product semantics,
  user-visible behavior, privacy posture, and irreversible tradeoffs.
  Engineers own flags, path fences, test mechanics, and guardrails, unless
  those change product behavior.

## 8. Finite Chat addendum

- Every room state mutation has positive and negative/replay tests. Rejected
  mutations admitted under an idempotency key are durable state: persist
  them intentionally.
- Keep fake-MLS tests honest: mark the fake gate; keep server-side
  invariants separate from crypto truth.
- Linked-device history imports as one atomic room snapshot: chunks may
  stage and retry, but nothing becomes visible until the complete manifest
  validates and commits. A source that cannot prove complete retained
  plaintext fails closed; it never substitutes a representative subset.
- Control plane (room creation, KeyPackages, Welcomes, link sessions,
  repair, idempotency) decides and persists; data plane (encrypted
  messages, sync) stays opaque.
- Assert boundary. Handled errors: wrong epoch, stale KeyPackage, duplicate
  idempotency key with different body, missing Welcome, room needing
  repair. Assertions/corruption: `last_seq` disagreeing with persisted log
  rows, membership key not matching the stored device, a Welcome ack with
  no corresponding membership interval, a persisted idempotency response
  kind disagreeing with its operation.
- `debug_assert!` only for checks too costly for release; use `assert!` or
  `StoreError::CorruptState` for contradictions so the seatbelt stays on in
  production.
- Allocation: request/session scratch at the cycle boundary; explicit sync
  result limits; borrowed slices in hot helpers; WASM-facing allocations
  count against the API budget and are documented.
- Performance targets: room commit = one transaction, one log row, small
  membership delta, zero plaintext inspection; message append = one
  transaction over room head, log entry, idempotency record, push outbox;
  sync = bounded page read by `(room_id, seq)`. Optimize in order: network,
  disk, memory, CPU.
- Product E2E is simulator-first; repeat the matrix on a physical phone
  before first users. Offline tests preserve runtime identity and the
  configured server URL, then toggle reachability; wrong-server-URL cases
  belong to diagnostics, not the offline-send matrix.
