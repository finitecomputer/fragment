# The usage ledger

Status: **built** (docs/cloudflare-v1.md, decisions 24–28 and
36–37; spike S4). The pure core is `crates/core/src/ledger.rs` (the state
machine) and `crates/core/src/price.rs` (the price book), with the public
wire types in `crates/proto/src/ledger.rs`. The cell runs it: the
`Ledger` Durable Object (`cell/src/ledger.rs`), the model route
(`cell/src/models.rs`), AI steps (`cell/src/ai.rs`), and each fragment's
meters and write gate (`cell/src/meter.rs`); docs/api.md (Models, Ledger)
is the wire contract. It replaced the OpenRouter-backed ledger and
`crates/core/src/budget.rs` (a hard cut).

## The model

- **One ledger per payer.** A payer is a person, or an org (its ledger
  named `org:<id>`; decision 66). The caller decides who pays a row and
  sends it to that payer's ledger, which keys on nothing else (decision
  36):
  - an agent's model calls, operator-key calls and compute bill the
    agent's owner;
  - a fragment's hosting (requests, storage, dynamic workers,
    screenshots) and its AI steps bill the fragment's owner: a person,
    or an org (its fragment's rows go to `org:<id>`, an agent's turns
    there to the agent's own owner).
- **An org's ledger** (decisions 66 to 68; `fragment_core::ledger::org`)
  holds the credit the org bought (its own $25 packs) or an operator
  gave it, and nothing else: no plan, seat, included credit or default
  plan. Its overdraft is zero, so its fragments are read-only at a
  balance of zero. It pays for no agent turn, no wake and no computer
  (`persons_only`, 403), and refuses a plan, a seat or an overdraft the
  same way. Its billing admins set a monthly budget (`OrgBudget`, zero
  until set): once its fragments' charges this month reach it, their AI
  steps are refused (`budget_reached`) until a larger one or the next
  month; writes and hosting go on. Every AI step in its fragments meets
  the fragment's cap too, whoever starts it: an org's fragment has no
  owner who passes it. Its terms are never in its head, which an older
  release's sweep reads and rewrites with only the fields it knows: that
  it is an org's is its ledger's name (`meta.payer`, kept as its first
  call named it), and its budget a row of its own (`meta.org_budget`), so
  a rollback loses neither. A person's head is written as before.
- **Guests pay for nothing.** A guest's ledger refuses their paid calls
  at the gate and meter batches (`guest_payer`). A guest owns nothing billable, so a
  guest makes no fragment either (`guest_creates`, 403: "guests can't
  create fragments"; Paul, 2026-10-03): every create asks its maker's
  ledger first (`may_spend(create)`). A guest still edits the fragments
  shared with them, whose writes bill those fragments' owners.
- **Plans** (decision 25): `guest`; `seat_always_on` ($200 a month, $100
  of credit included, an always-on computer whose awake time is not
  metered), the one seat sold; `seat` ($50 of credit included, a
  computer that sleeps), which is no longer sold (Paul, 2026-10-15) and
  comes only from an operator's comp or a deployment's `default_plan`.
  Stripe charges the seat itself; the ledger only knows the credit. More
  credit is the $25 pack (Where Stripe plugs in, below).
- **Seat state** comes from a hook: `active`, `past_due`, `canceled`.
  - A seat gets its included credit only while `active`.
  - `past_due` keeps this month's credit. The next month's arrives when
    the seat is active again.
  - `canceled` stops agents. Fragments keep serving and writing on
    whatever credit is left.
  - Its one writer is the registry (docs/billing.md), whose pushes may
    arrive out of order, so each carries `seq` (the registry's order). A
    change no newer than the last applied is refused as stale
    (`StaleSeat {current}`, the order it is at) and changes nothing; the
    registry orders its next push past it.
- **Credit.**
  - Included credit arrives at the start of each UTC month (lazily: the
    first mutation of the month, and every read, see it). It expires at
    the month's end. Skipped months grant nothing.
  - A seat that becomes active, or a plan that grows, mid-month gets the
    difference, once. Nothing is taken back when a plan shrinks.
  - Purchased credit (grants) stays until spent.
  - A charge draws included credit first, then purchased.
  - Included credit that arrives while the person owes pays the debt
    first. So a debt is paid once, and a negative purchased balance
    means the included credit is gone.
- **Caps** (decision 26). Each fragment has a monthly cap on its owner's
  ledger, $5 until the owner sets one. Its spend is every charge on that
  ledger in that fragment this month. At the cap the fragment is closed
  to AI steps and agent turns by anyone but its owner (and the owner's
  agents acting for them), as the gate reads it. A call that started
  under it is charged past it; only a fragment already at it is closed.
  Caps never stop writes or wakes. A late row counts in its own month.
- **Zero** (decision 27). At a balance of zero or less, agents stop: no
  turns, no wakes, no AI steps (`agents_stopped`, `no_credit`).
  Fragments keep serving and taking writes.
- **Overdraft and read-only.** At a balance of minus the overdraft ($2
  unless an operator sets another) the person's fragments go read-only.
  They stay read-only until the balance is above zero again. An operator
  who changes the overdraft decides it afresh. Read-only, the person
  makes no new fragment, and their fragments' cron and triggers start no
  runs: each is recorded `blocked`, saying why (Paul, 2026-10-03).
- **Meters always record.** Usage that happened is charged, past zero
  and past the overdraft: nothing is held before a paid call (decision
  62), so a call that started before zero is charged in full after it.
- **Standing** is a pure function, `standing_of(plan, seat, balance,
  read_only)`: `ok`, `agents_stopped {why}` or `read_only {why}`, with
  `why` one of `guest`, `seat_canceled`, `no_credit`, `overdrawn`.

## The price book

A price is micro-dollars per a fixed count of its unit, so Cloudflare's
own prices are exact integers. A row's charge is
`ceil(list × (1 + fee) × (1 + margin))`, computed exactly in 128 bits and
rounded up once per row. The fee is AI Gateway's 5% on credits, so it
applies to AI (tokens and neurons) only. The margin is the operator's,
50% by default. Each row also keeps its list price (for reconciling with
the gateway's log `cost`) and its cost basis (list plus fee).

| Meter (`Usage`) | Unit | Default list price | Source |
|---|---|---|---|
| `tokens` | tokens per model: input (uncached), cached input, cache write, output | per million: Flash $0.15 / $0.03 / $0.15 / $0.50; any model through OpenRouter (Finite's tiers', GLM-5.3 Flash: `z-ai/glm-5.3-flash`) at its slot's ceiling in the config, `max_price`'s prompt for every input token and its completion for output (Finite's $0.15 / $0.15 / $0.15 / $0.50), grossed up for OpenRouter's 5.5% fee (to $0.150715 / $0.150715 / $0.150715 / $0.502381), GLM-5.3 Flash's also kept as a row of the code's for its usage queued past a switch; a model on any other OpenAI-compatible API at the list price its slot names (`price`; the private tier's fallback, Tinfoil's DeepSeek V4.1 Flash, $0.65 / $0.13 / $0.65 / $1.45); Finite Private's GLM-5.3 Flash (`glm-5-3-flash`) the same as Flash's; GLM-5.3 $1.40 / $0.26 / $1.40 / $4.40; DeepSeek V4 Flash (the route's default fallback) $0.44 / $0.014 / $0.44 / $1.32; Opus 5.5 $4 / $0.20 / $5 / $20; the Clef family (decisions; media counted as input tokens, nothing out, no cache: every input token at its input price) Clef-flash $0.038, Clef $0.24, Clef-omni $0.15 | Workers AI catalog (`/ai/models/search`; DeepSeek's checked against its neurons on 2026-10-09); the AI model catalog page for Opus (S4); OpenRouter's is the `max_price` each slot routes under, read from the deployment's config (deploy/example.jsonc; its fee, openrouter.ai/docs/faq, read 2026-10-15), "OpenRouter's price" below, a decision for Paul; Finite Private publishes no price (finite-mono meters a key in weighted tokens, never dollars), so it is at Workers AI's Flash's, decision 61 (a decision for Paul); Workers AI's pricing page and its [Clef-flash](https://developers.cloudflare.com/workers-ai/models/clef-flash/), [Clef](https://developers.cloudflare.com/workers-ai/models/clef/) and [Clef-omni](https://developers.cloudflare.com/workers-ai/models/clef-omni/) pages (2026-10-14) |
| `neurons` | thousandths of a neuron | $0.011 per thousand neurons; an image (FLUX.1 [schnell]) is 4.80 neurons a 512×512 tile and 9.60 a step (`fragment_core::media`); a transcription (Whisper large-v3-turbo) is 46.63 neurons a minute of audio, charged at the length Whisper heard (`fragment_core::transcribe`) | Workers AI pricing (its image rows for FLUX.1 [schnell], its audio row for whisper-large-v3-turbo, read 2026-10-07); S4 matched it to tokens on every call |
| `awake` | ms, per instance type | `2vcpu-6gib`: $0.064224 an hour | Containers pricing: 6 GiB memory and a 12 GB disk provisioned, plus 5% of 2 vCPU (CPU is billed on active use, which the Computer DO cannot see) |
| `storage` | byte-hours, by class | per GB-month (10^9 bytes × 720 h): R2 $0.015, SQLite $0.20, git $0.015 | R2 and Durable Objects pricing; code.storage publishes no price to us, so git is at R2's |
| `requests` | requests | $0.45 per million | Workers Standard $0.30 plus the Durable Object request $0.15 |
| `dynamic_workers` | unique dynamic workers per UTC day | $0.002 each | Dynamic Workers pricing |
| `browser` | ms of Browser Rendering | $0.09 an hour | Browser Run pricing (developers.cloudflare.com/browser-run/pricing, updated 2026-04-21): $0.09 per browser hour past the 10 a month Workers Paid includes |
| `images` | unique transformations | $0.50 per thousand | Cloudflare Images pricing |
| `key` | calls the provider answered | per call at list: Firecrawl $0.01, FAL $0.006, Browser Use $0.02/12, Perplexity $0.005, Google Places $0.035, xAI $0.12, X API $0.05, ElevenLabs $0.15 (`micros` per `per` calls), or the price a catalog row names | each vendor's pricing page (`DEFAULT_KEYS` in `price.rs`, each with its source and what an estimate assumes); the book's `keys` are the deployment's catalog's operator rows |

Checked against S4's real numbers: a $0.004 Flash turn is charged
$0.0063; the gateway's own `cost` equals our list price on every call;
neurons and tokens agree within half a micro-dollar on the spike's 17
calls. The defaults are consts in `price.rs`, each with its source. The
operator's book comes from the deploy's configuration, built and
checked once per isolate (`Config::book`), and a ledger keeps none of
its own (decision 62): every call into a ledger comes from its own
release (the deploy-skew guard: docs/MODEL.md, Calls between releases),
so each batch is priced with the book of the release applying it. A
key's row added or its price changed, a change to the defaults, and a
configuration rolled back each reach every ledger with the deploy (#451:
on 2026-10-20 operator keys' rows were added without a higher version,
which a ledger then needed, and every ledger refused their calls as
unpriced). A row is charged at the book of its time, its version kept
beside it, and each book a ledger priced with is kept once as a command,
`book:<digest>`, for the audit. A head kept before names the book it
charged with: read past. A head is written with the book of the release
writing it (`Ledger::written`), which none since reads, so a release
rolled back to one that kept its own (and requires it) reads every head
this one writes. The book's version is informational (a
ledger's status shows it, `priceBook`): the higher of the
configuration's `price_book_version` and the code's
(`DEFAULT_BOOK_VERSION`, 7 since OpenRouter's GLM-5.3 Flash was priced
on 2026-10-15, after 6 for the Clef family, 5 for Finite Private's
GLM-5.3 Flash, 4 for Hermes' tool keys and 3 for DeepSeek V4 Flash).

**OpenRouter's price** (decision 61; a decision for Paul): Finite's
tiers run on GLM-5.3 Flash through OpenRouter, which routes each call to
one of its providers under the slot's `max_price` ($0.15 a million
tokens in, $0.50 out) and bills that provider's price, marked up by
nothing, from credits bought with a 5.5% fee (5% in crypto). The book
charges the ceiling, never less than OpenRouter could bill, and takes it
from the config: a slot whose `body` names `provider.max_price` is its
model's row (`fragment_core::models::Upstream::ceiling`, built into the
release's book with the deploy: `Models::priced`), so any model id
OpenRouter serves runs priced, and moving a tier to another model, or
back, is a change to the config alone (Paul, 2026-10-22). A slot on
OpenRouter without a `max_price` of `prompt` and `completion` (dollars,
0 to 1,000 a million tokens) is refused, by the deploy and the cell;
two slots running one id name one ceiling. A usage is priced by the
release that takes it (a model call's from its queue, an AI step's from
its fragment's outbox), which may run the tiers on another model by
then: GLM-5.3 Flash's row stays in the code, at the ceiling its slots
named, so its usage queued past the switch is charged as it was; any
other model's charges nothing there and logs `ledger.unpriced` (decision
62; carrying each usage's price with it is #509).
- **Every input token at $0.15**, cached and written ones too: the
  endpoints' cache reads ($0.0087 to $0.099 on 2026-10-15) are no part of
  `max_price`, so none bounds them.
- **OpenRouter's fee, not AI Gateway's.** The book's credits fee is one
  rate for all AI (5%), so the row's list is the ceiling × 1.055 / 1.05,
  rounded up once (`fragment_core::price::OPENROUTER_FEE_BP`): its cost
  basis is the ceiling plus 5.5%, and the margin goes on that. A million
  tokens in charge $0.237377, a million out $0.791251.
- **What it costs a person.** Workers AI's Flash's for output, and five
  times its for a cached prefix: a call of 30,000 tokens in (25,000 of
  them cached) and 600 out charges $0.007597, against $0.002835 on
  Workers AI's Flash. Settling from OpenRouter's own `usage.cost` would
  charge what each call cost; it is in the debt ledger.

**A slot's own price** (Paul, 2026-10-22: a model is a config's
choice): a slot on an OpenAI-compatible API that is not OpenRouter may
name its model's list price there (`price`: dollars a million tokens of
input, cached input and output), and that is its model's row, over any
the code keeps for that id (`fragment_core::models::Upstream::own_price`,
built into the release's book as a ceiling is: `Models::priced`). The
row takes the book's credits fee and margin as every token row does,
though such a vendor bills a card with no fee: the 5% is a cost the
platform does not bear, so a person pays that much more than the list
plus margin, never less. A cache write is at the input price (none of
these vendors bills one). Tinfoil's DeepSeek V4.1 Flash, the private
tier's fallback: a call of 30,000 tokens in (25,000 of them cached) and
600 out charges $0.011608, against $0.002835 on Finite Private's or
Workers AI's GLM-5.3 Flash, which the fallback answers only when they
fail.

**Deep's price** (decision 65). A tier's Deep is a slot like any: priced
as its model's row (Tinfoil's list for Private's, its ceiling for
Standard's on OpenRouter), its reasoning tokens counted in its output, a
call at most its slot's `max_tokens` (32,768 for Deep: $0.0475 of
Tinfoil's output at list). A delegate's calls carry its agent's name, so
they bill the agent's owner, as its turns do, under no fragment's cap; a
person's own Deep (their OpenRouter) is their account's.

**Finite Private's price** (decision 61; a decision for Paul): its list
price is Workers AI's GLM-5.3 Flash's, and its rows take the credits fee
and the margin as every token row does, though no gateway is between: a
call is charged the same whichever of the two answered it, and the cheap
tier costs a person what it did. Two differences remain:
- **Its cached tokens.** SGLang behind its limiter reports none (no
  `prompt_tokens_details`), so its prompts are priced uncached, where
  Workers AI prices a cached prefix at a fifth. An agent's long, mostly
  repeated prompt costs its owner more on Finite Private until it reports
  them (part of the ask to finite-mono).
- **What it costs Finite** is Tinfoil's GPU time, not tokens: the margin
  is a guess at that, not a measure.

A model call that fell back (docs/api.md, Models) is charged once,
from the fallback's usage at the fallback's prices: DeepSeek V4 Flash,
the default fallback, is dearer than the cheap tier's GLM-5.3 Flash; on
Finite's own deployment the fallback, Workers AI's GLM-5.3 Flash, is
cheaper than the tiers' OpenRouter row (and at one price with Finite
Private's).

## The state machine

Pure: no I/O, no clock (time is an argument). The state has two parts:

- **The head** (`Ledger`): the plan, the seat, the balance's parts, the
  read-only latch and the overdraft. Small and bounded; kept whole. The
  price book is the caller's release's (above), passed to `meter`. A
  head kept before decision 62 names the reservations it held too: as it
  loads, its holds are retired (`retire_holds`), never charged (what
  they held was never drawn), and each one's reservation entry, still
  held, is ended as released (its time kept, so a replay of it stays
  refused), in the transaction that writes the head back with `holds`
  empty. So a release rolled back to one before decision 62 (which
  requires the map) reads every head this one writes, and finds no hold
  and no held entry for its sweep.
- **The store** (`trait Store`): what grows with use, by id. References
  (`Entry`), batches, commands, each fragment's spend by month, and caps.
  The Durable Object keeps these in SQLite tables; `Memory` keeps them in
  maps for tests.

A mutation decides before it writes, so a refusal writes nothing. Every
mutation first brings the ledger to its time's month.

| Mutation | Idempotent by | Answer |
|---|---|---|
| `grant(GrantCredit)` | its `id` (Stripe's payment id, or the operator's) | `()` |
| `set_plan(SetPlan)` | its `id` | `()` |
| `set_seat(SetSeat)` | its `id`; ordered by `seq` (one no newer than the last applied is refused, `StaleSeat`) | `()` |
| `set_overdraft(SetOverdraft)` | its `id` | `()` |
| `set_cap(SetFragmentCap)` | its `id` | `()` |
| `meter(Meter, book)` | the batch's id; each row by its `ref`; the book kept once as a command, `book:<digest>` | `{charged, rows_new, rows_before, refused: [{index, why}]}` |
| `sweep(now)` | (time) | `{forgotten}` |

- **Replays.** The same id again answers as the first did and changes
  nothing. The same id with another body is refused
  (`conflicting_body`). A refusal is not remembered, so the same id may
  be tried again (a step after a top-up).
- **A row is charged once by its reference**, whatever batch carries
  it: a paid call's row under its call's reference (`aig:`, `key:`), an
  AI step's under the step's key (`step:`), so a step tried again after
  its call was paid is charged once (bug 2).
- **References are one namespace.** Sources prefix theirs (`aig:`,
  `step:`, `awake:`), so two sources never collide. An entry the
  releases before decision 62 kept (a reservation, held, settled or
  released) reads as one from before (`Entry::Before`): never charged
  again, a row under its reference is another body, and it is forgotten
  by its time as any is.
- **Sweeps** run from the Durable Object's alarm at `next_sweep_ms`,
  daily: references and batches older than 90 days are forgotten, and a
  row from before that is refused as `stale`, so a forgotten reference
  is never charged again. Commands are never forgotten.
- **Reads** (`standing`, `gate`, `fragment_open`, `may_spend`, `status`)
  roll a copy of the head to their own month and change nothing. A
  spend is an agent's turn, an AI step, a wake, a write, or a create (a
  fragment made, a write to one of the maker's own: refused for a guest
  as `guest_creates`, otherwise as a write is).
  `may_spend(spend, fragment, by_owner)` is the whole question when the
  payer owns the fragment (or there is none). When another person owns
  it, that owner's ledger also answers `fragment_open`.

Every refusal is a typed `Refused`, with `code()` (the platform's
`ErrorCode`: every money refusal is 402 `budget_used_up`) and `message()`
for people.

## The gate

Before a spend starts, its payer's ledger is asked whether it may
(`may_spend`, and a fragment's owner `fragment_open` when someone else
pays in it), through one cached question, `ledger::may` (cell/src/ledger.rs;
its cache is `fragment_core::ledger::gate`), never on every call
(decision 62; Paul, 2026-10-21: "billing should be settled best effort not
interrupt every request"):

- **Cached per isolate.** Each isolate (the router's, which makes model
  calls and the swap's operator-key calls, and each Fragment object's,
  which takes writes) keeps each answer for `STANDING_CACHE_MS` (a
  minute), keyed by the person and the question (the spend, the
  fragment, whether its owner spends). A refusal is kept as an
  allowance is. At most `GATE_ENTRIES_MAX` (1,024) answers: past it the
  stale go first, then the oldest.
- **Asked once, briefly.** With no fresh answer, the ledger is asked once,
  for at most `GATE_WAIT_MS` (1.5 s: a few times a Durable Object call's
  round trip, a cold start's included).
- **Fail open.** A ledger that does not answer in time, or answers with
  anything but its own refusal (an outage, a deploy going out), lets the
  spend start, logs `ledger.gate-unknown`, and is not kept, so the next
  spend asks again. The one deliberate refusal is the ledger's: a person
  out of credit, read-only, a guest, or a capped caller past a fragment's
  cap.
- **What it costs.** A person's standing reaches an isolate within a
  minute: one out of credit spends on for up to a minute more (each call
  charged after it, as every meter is: usage that happened is charged),
  and one who topped up waits up to a minute for their agents. Paul: "'overspend by
  whatever is in flight during the cache window' is a small price to pay
  for 1. simplifying the code, 2. not breaking agents, 3. lowering
  latency."
- **Who asks.** A job's AI step asks `may_spend(ai_step)` of its
  fragment's owner, with the fragment when its cap applies (the Fragment
  object's isolate: cell/src/ai.rs `gate`). An agent's model call (the router's isolate: the model
  route, and a computer's model intercept through it) asks
  `may_spend(agent_turn)` of the agent's owner, and `fragment_open` of a
  fragment someone else owns that it spends in for someone else
  (cell/src/models.rs `gate`); so does an operator key's call through the
  swap (the router's isolate too: cell/src/computer.rs `egress_swap`). A fragment's writes ask
  `may_spend(write)` of its owner (cell/src/meter.rs `read_only`): 402
  past the overdraft. A test fleet's `forget-standing` lever (docs/api.md)
  makes an isolate's gate forget every answer, as its ledger's
  `forget-gate` does the router's; its ledger's `slow {ms}` makes it
  answer late, which no call waits on past the gate's wait (the e2e's
  `ledger` section).
- **Test fleets** cap an e2e person's paid calls (`paid-calls {max}`):
  once their ledger has charged `max` (each an entry under
  `aig:`, `key:` or `step:`), its gate refuses an agent's turn or an AI
  step as at zero credit, saying so; the cap is approximate by what
  starts within the gate's minute.

## Limits

| Limit | Value | Why |
|---|---|---|
| `BATCH_ROWS_MAX` | 1,000 rows | one request and one transaction, far under the body limit |
| `GRANT_MAX` | $10,000 | more is a typo |
| `CAP_MAX` | $10,000 | more is no cap |
| `OVERDRAFT_MAX` | $1,000 | someone trusted with more is someone to invoice |
| `org::BUDGET_MAX` | $100,000 a month | an org's budget: more is a typo |
| `price::CHARGE_MAX` | $100,000 a row | no call or sample costs that; a row that would is a bug upstream |
| `price::QUANTITY_MAX` | 10^15 per quantity | keeps the 128-bit math from overflowing |
| `ID_MAX_BYTES` | 256, printable ASCII | ids and names |
| `REF_KEEP_MS` | 90 days | the audit retention; sources retry within hours |
| `CLOCK_SKEW_MS` | 5 minutes | a row's source runs on its own clock |

## The Ledger Durable Object

One `Ledger` DO per payer, named by a person's identity or `org:<id>`
(cell/src/ledger.rs).
Each route is one `transactionSync` over the head and the store (a
`js.rs` helper: workers-rs 0.8.5 has none). The bodies are the core's
types (inner) or proto's (public); answers are JSON of the answer types,
and refusals are `ErrorBody {error: code(), message: message()}` with the
typed `refused` beside it, so a caller tells a read-only owner from a
guest. A statement that fails marks the store at fault, and the route
rolls back whole. A new person's ledger starts as the core makes it (a
guest), then takes `FRAGMENT_DEFAULT_PLAN` under `plan:default`; an
org's starts as an org's (`Ledger::new_org`), by its name.

Tables:

- `head`: the head (`Ledger`), kept whole as one JSON row: small and
  bounded, checked as it is read back;
- `entries (ref PRIMARY KEY, at_ms, held, entry)` (`held` written 0: a
  column of the releases that held), `batches (id PRIMARY KEY,
  digest, answer, at_ms)`, `commands (id PRIMARY KEY, command, at_ms)`;
- `spend (month, fragment, micros, PRIMARY KEY (month, fragment))`,
  `caps (fragment PRIMARY KEY, micros)`.

Inner routes (platform code only, through the DO's stub):

| Route | Body | Answer |
|---|---|---|
| `POST /meter` | `Meter` | `Metered` |
| `POST /may-spend` | `{spend, fragment, byOwner}` | `{}` or the refusal |
| `POST /fragment-open` | `{fragment}` | `{}` or `cap_reached` |
| `POST /grant`, `/plan`, `/seat`, `/overdraft`, `/cap` | proto's `GrantCredit`, `SetPlan`, `SetSeat`, `SetOverdraft`, `SetFragmentCap` | `{}` |
| `POST /status` | `{}` | proto's `LedgerStatus` (a person's) |
| `POST /org-status`, `/org-budget` | `{}`, proto's `OrgBudget` | proto's `OrgLedgerStatus`, `{}` (an org's) |
| `POST /test` | `{op: clock {offsetMs} \| sweep \| entries {prefix} \| totals \| book \| paid-calls {max?} \| legacy-seat {seat, seq} \| schema}` | test fleets only |

A batch is priced with the configured book of the DO's release
(`Config::book`: the core's defaults and the configuration's operator
keys' prices; the debt ledger), and a status names its version
(`priceBook`); the `book` lever answers that book. Its alarm runs
`sweep` at `next_sweep_ms`.

Public routes (a hard cut of `/api/budget`; docs/api.md, Ledger):

- `GET /api/ledger` (the person; an agent reads its owner's) →
  `LedgerStatus`; `fragment ledger [--json]`.
- `PUT /api/f/<name>/cap {id, micros}` (the fragment's owner; kept as
  `cap:<fragment>:<id>`); `fragment cap <name> <usd>|default`.
- Operators: `POST /api/ledger/<user>/grant|plan|overdraft` with
  proto's bodies, their ids kept as `<kind>:<id>`; `fragment ledger
  grant <user> <usd> --why <text>`. A seat's state is the registry's
  alone: operators comp or end seats there (`/api/admin/seats`,
  `fragment operator comp|uncomp`; docs/billing.md), and the ledger's
  `seat` command went (2026-10-16). `<user>` may be `org:<org id>` for a
  grant of an org's own credit (an org that is there; its ledger takes
  no plan and no overdraft).
- An org's (decision 66): `GET /api/orgs/<org>/ledger` →
  `OrgLedgerStatus` and `PUT /api/orgs/<org>/budget {id, micros}` (its
  billing admins; kept as `budget:<id>`), `POST
  /api/orgs/<org>/billing/pack` (who pays for it; docs/billing.md).

## How each meter reaches it

Each source keeps an outbox in its own SQLite and flushes one batch per
payer through the ledger Queue, under a batch id of its own
(`<source>:<seq>`). A crash between the ledger's apply and the outbox's
mark sends the same batch again, which answers as before.

- **Model calls** (the computer's model intercept, lesson 7). Before the
  call it asks the gate (above); nothing is held. It streams, takes only
  the last, cumulative usage (S4), and is charged with it after its
  answer, at the stream's `[DONE]` (not when the connection ends: the
  gateway has held one open ~150 s past it), as one row under a
  reference of its own (`aig:<hex>`), a batch of its own under that
  reference on the ledger's queue (cell/src/meter.rs `charge`), which
  asks again until the ledger has it. A call that failed before any token
  is charged nothing. A call that reported no usage (its stream broke) is
  charged nothing, said aloud (`ledger.unknown-usage`): billing never
  stops an agent (decision 62). A Workers AI
  call's gateway log id rides along for reconciliation; the gateway's
  per-user spend rule is only a backstop. An OpenRouter or Finite
  Private call has neither: OpenRouter bills the platform's one key from
  its credit (its own credit limit the backstop), finite-mono meters its
  key against its window, and this ledger alone meters each person. An
  OpenRouter stream's usage comes on its last chunk, which keeps a
  choice: an OpenAI-compatible API's usage is read wherever it comes
  (`fragment_core::models::Stream::on`). The payer is the
  agent's owner; the fragment is the agent's. An agent's decision
  (`/v1/decide`) and transcription are charged the same way, after their
  answers, at the input tokens or the audio's length they reported.
- **AI steps** (a job's `ai.*`, voice input). Before the call the step
  asks the gate (above) of the fragment's owner, `may_spend(ai_step)`,
  capped (with the fragment's cap) when the run's principal is neither
  the owner nor an agent of theirs; nothing is held. What the call bought
  is kept in the Fragment DO beside the step (by its key,
  `<fragment>@<incarnation>/run/<run>/step/<index>`, the same in every
  attempt), together with its charge: one row in the fragment's outbox,
  `step:<key>`, which the ledger charges once whatever attempt or replay
  carries it, and its price on the run (`charges`, priced with the
  configured book), in one transaction. A retried step that finds what it
  kept reuses it and never calls the model again (bug 2). A receipt the
  release before kept is never charged again: that release reserved its
  call under an attempt's reference, which it settled, its sweep charges,
  or the next release forgives. A step that fails for good is
  charged nothing: nothing was held (bug 3 went with the holds).
  A text step is charged its tokens (streamed, from its last usage; its
  tools, tool calls and their results are tokens like any other, so the
  price book is unchanged); an image is charged `neurons` from the JPEG
  it got (its tiles, read from its header) and its steps. The payer is the
  fragment's owner.
  A decision (`job.ai.decide`, the Clef family: docs/api.md, Decisions)
  is charged its reported `usage.input_tokens` (media included) with no
  output charge: Clef scores every question in one pass over one context,
  its media and questions counted in it, and a request past it fails.
  More than the context is charged as reported (it happened); missing or
  malformed usage is charged nothing, said aloud (`ledger.unknown-usage`).
  A malformed paid answer is kept as a rejection with its charge, so a
  retry or replay reuses it, including after a restart. At the default
  book, 300 input tokens cost 18 micro-dollars on Clef-flash
  (ceil($0.038/M × 300 × 1.05 × 1.5)), 114 on Clef and 71 on Clef-omni;
  a whole context, 1,471 (Clef-flash's 24,576 tokens), 24,773 (Clef's
  65,536) or 15,120 (Clef-omni's 64,000).
- **Operator keys** (decisions 37 and 62): gated and charged the same
  way as model calls. Before the call, the swap asks the gate of the
  agent's owner (`may_spend(agent_turn)`): only a refusal stops it, and a
  ledger that does not answer refuses nothing. Once the provider answered
  (under 500), each operator key's call is one `key` row at its price
  (`key:<computer>:<12 hex>`, as the agent, naming the computer) on its
  owner's ledger, sent on the ledger's queue after the answer, and the
  computer counts it with its charge, priced with the configured book
  (docs/computers.md). A call the provider did not answer is charged
  nothing.
- **Not metered: a person's own keys** (decisions 37 and 60). Their own
  key's calls, an agent's model calls on their own OpenRouter account
  among them, are paid by their account at the provider. The computer
  counts them by agent and month (`uses`, charge 0), and nothing reaches
  the ledger: no charge, no platform fee (Paul,
  2026-10-08).
- **Compute** (the Computer DO). It meters each awake interval at sleep,
  and every few minutes while awake, as `awake {instance, ms}` rows
  (`awake:<computer>:<from ms>`). The payer is the computer's owner. The
  ledger waives an always-on seat's awake time; no wake starts when
  `may_spend(wake)` refuses. The time a computer is kept for its failed
  saves (from the start of a sleep's try whose save failed, a hung save's
  minutes included, to `computers.unsaved_max_ms` after that failure, and
  the try under way then to its failure) is never metered: the
  platform's, not its owner's (Paul, 2026-10-08; docs/computers.md, "What
  its owner is told").
- **Storage** (the Fragment DO's alarm, daily). It samples the fragment's
  own SQLite and its blobs, and meters `bytes × hours since the last
  sample` (`store:<fragment>@<incarnation>:<class>:<at>`) to the
  fragment's owner. Its app facet's database and its git repository are
  not sampled (the debt ledger). Nor are a computer's saves of `/data`
  in R2 (its newest three, `fragment_core::computer::SAVES_KEPT`;
  docs/computers.md, "Saves and what a wake restores"): the Computer DO
  meters only awake time and operator keys (`Usage::Awake`, `Usage::Key`
  in cell/src/computer.rs), so they are the platform's for now (the debt
  ledger, "A computer's saves are not metered").
- **Requests** (the Fragment DO, which sees each one the router hands
  it): a row per minute (`req:<fragment>@<incarnation>:<minute>`) to the
  fragment's owner, closed once the minute has passed.
- **Dynamic workers** (the Fragment DO): one row the first time a code
  version loads on a UTC day (`dw:<fragment>@<incarnation>:<version>:<day>`).

The fragment's three meters share its outbox (`meter_rows`), flushed by
its alarm one batch at a time (at most 200 rows, under the queue's 128 KB
message) as `frag:<fragment>@<incarnation>:<n>`; the queue's consumer
applies it and tells the fragment, which forgets it, and one not
acknowledged within five minutes is sent again. A guest's ledger refuses
the batch, so a guest's fragments are billed nothing.

- **Preview cards** (decision 31; docs/api.md, Cards): each try at a
  deploy's card, or at one an editor asked for again, is a `browser` row
  in the fragment's outbox
  (`card:<fragment>@<incarnation>:<live, 12 hex>:<wanted at>:<try>`),
  its session's time from acquire to close (a close that was lost adds
  the session's 10 s inactivity timeout), to the fragment's owner. A
  stale shot and a failed one are billed too: their browser time was
  spent. A guest's or a read-only owner's deploys are not shot, and an
  ask for a shot again is refused (402). An uploaded card is a blob,
  stored as any is.
- **Icons** (docs/api.md, Icons): a making's calls, each a row in the
  fragment's outbox as it answers
  (`icon:<fragment>@<incarnation>:<op>:<call>`, `op` when the making or
  upload started and a nonce of its own), to its owner: the
  subject (`tokens`, GLM-5.3 Flash), each drawing (`neurons`, FLUX.2
  [klein] 4B, what Workers AI said it cost, 104.2 when it says none: its
  pricing page's 26.05 a 512×512 output tile, four times, as it said in
  the spike), each check (`tokens`, Clef-flash), and each Images
  transformation (`images`: a cutout, a fit, a flattening; an upload
  fitted). A drawing refused after its call was bought. The owner's ledger
  is asked first, as for a job's AI step: capped (with the fragment's
  cap) when the one asking is neither its owner nor an agent of theirs.
  An upload is a write: refused while the owner is read-only, before
  anything is fitted.
- **Images** (the deploy path): `images` rows to the fragment's owner,
  when a transformation is made.
- **An export** (a person's list, export.rs; docs/api.md, Export): one
  batch to its person, `export:<id>`, as its archive is deleted (seven days
  after it was ready, at a newer export, or as a failed one's parts go):
  `export:<id>:storage`, its archive's bytes from ready to deleted as R2
  byte-hours, and `export:<id>:requests`, its R2 operations and its
  downloads. Asked of the ledger directly, as a computer's awake time is,
  until it answers (a guest's refuses it: billed nothing). Its reads of
  fragments are theirs, each a request metered to its owner.

## Where Stripe plugs in

- **Credit:** a paid credit pack's Checkout becomes `GrantCredit {id:
  "pack:<Checkout's id>", by: "stripe"}` (docs/billing.md, decision 55)
  on the ledger its metadata named when it was opened: its person's, or
  for an org's own pack (`fragment_payer: org:<id>`, no person), the
  org's. Its id may come again any time (its webhook, its return);
  commands are never forgotten.
- **A seat's state:** the registry pushes `SetPlan` (the seat's kind)
  and `SetSeat` (active while the seat is good, else canceled) in its own
  `seq`, from the seat and its org's subscription as fetched from Stripe
  (docs/billing.md, "What a seat does"). It is the seat's one writer.

## Assumptions (Paul's to confirm)

- `past_due` keeps agents running on this month's credit, and gets no new
  month's credit until active again.
- A canceled seat keeps paying for its fragments' hosting from what is
  left; `guest` is for people who never had a seat.
- Mid-month upgrades grant the difference at once; downgrades take
  nothing back.
- A debt is paid from the next month's included credit first.
- A cap counts every charge in the fragment on its owner's ledger,
  the owner's own agents' included. Only others are stopped by it.
- An agent's own usage cap (decision 14) is not built: the same
  mechanism, keyed by the agent's fragment, would carry it.
- A paid call that reports no usage is charged nothing, said aloud
  (`ledger.unknown-usage`; decision 62, the debt ledger).
- The default instance rate assumes a 12 GB disk and 5% CPU; git storage
  is priced at R2's until code.storage's price is known.
- A card's shot is a Browser Session, which Cloudflare also bills by
  concurrent browsers ($2.00 a month for each past 10, the month's
  average of each day's peak). That is a deployment-wide charge no one
  shot causes, so it is not metered per person; the margin carries it
  (each fragment has at most one shot out: its alarm takes it).
