# The usage ledger

Every cost someone causes is metered into one ledger per payer
(docs/cloudflare-v1.md, decisions 24 to 27, 36, 37, 62 and 66 to 68).
The state machine is `crates/core/src/ledger.rs` (an org's,
`ledger/org.rs`), the prices `crates/core/src/price/`, the Durable
Object `cell/src/ledger.rs`, a fragment's meters `cell/src/meter.rs`. The
routes are docs/api.md, Ledger; seats and Stripe are docs/billing.md.

## Money

- **Integer micro-dollars, UTC months.** A row's charge is
  `ceil(list × (1 + fee) × (1 + margin))`, exact in 128 bits and rounded
  up once per row.
- **Idempotent by id.** Every mutation (a grant, a plan, a seat, an
  overdraft, a cap, a budget, a meter batch) carries its own id. The same
  id again answers as the first did and changes nothing; the same id with
  another body is refused (`conflicting_body`); a refusal is not
  remembered, so it may be tried again. A row is charged once by its
  reference, whatever batch carries it. References are one namespace,
  each source its prefix. References and batches are forgotten after
  `REF_KEEP_MS`, and a row older than that is refused as stale, so a
  forgotten one is never charged twice. Commands are never forgotten.
- **Charged after, never held** (decision 62). Nothing is held before a
  paid call; its usage is charged after its answer, past zero and past
  the overdraft. A call that failed before any usage, or reported none,
  is charged nothing (`ledger.unknown-usage`).
- **Plans** (decision 25): `guest`, for someone who never had a seat;
  `seat` (`SEAT_INCLUDED` a month); `seat_always_on`
  (`SEAT_ALWAYS_ON_INCLUDED`, its awake time not metered), the one seat
  sold. Included credit arrives each UTC month while the seat is
  `active` and expires at the month's end; a seat that becomes active or
  a plan that grows mid-month gets the difference once, and nothing is
  taken back when one shrinks. Purchased credit stays until spent. A
  charge draws included credit first; included credit that arrives while
  the person owes pays the debt first.
- **Seat state** (`active`, `past_due`, `canceled`) has one writer, the
  registry, in its own order (`SetSeat.seq`; an older one is refused as
  `StaleSeat`). `past_due` keeps this month's credit and gets no new
  month's. `canceled` stops agents; fragments serve and take writes on
  what is left.
- **Guests pay for nothing.** A guest's ledger refuses their paid calls
  and meter batches (`guest_payer`), and a guest makes no fragment
  (`guest_creates`), both 403. Fragments shared with them that they edit
  bill their owners.
- **Caps** (decision 26). Each fragment has a monthly cap on its owner's
  ledger (`CAP_DEFAULT` until the owner sets one), counting every charge
  in that fragment, the owner's own agents' included. At it, the
  fragment is closed to AI steps and agent turns by anyone but its owner
  and the owner's agents. A call that started under it is charged past
  it. A cap never stops writes or wakes.
- **Zero** (decision 27): at a balance of zero or less, agents stop: no
  turns, wakes or AI steps. **Read-only**: at minus the overdraft
  (`OVERDRAFT_DEFAULT` unless an operator sets another) the payer's
  fragments refuse writes, they make no fragment, and their cron and
  triggers record each run `blocked`, until the balance is above zero.
- **Standing** is `standing_of(plan, seat, balance, read_only)`: `ok`,
  `agents_stopped` or `read_only`, with why (`guest`, `seat_canceled`,
  `no_credit`, `overdrawn`). A refusal for credit, read-only, a cap or a
  budget is 402 `budget_used_up`.

## An org's ledger

Decisions 66 to 68. Named `org:<id>`, it holds the credit operators
grant it (orgs pay out of band) and nothing of a person's: no plan, seat,
included credit or overdraft (zero, so its fragments are read-only at a
balance of zero). It pays for its fragments alone: a plan, seat or
overdraft set on it, and an agent's turn, a wake or a computer's row
asked of it, are refused (`persons_only`, 403). An
operator sets its monthly budget (zero until set): once its fragments'
charges this month reach it, their AI steps are refused
(`budget_reached`) until a larger one or the next month; writes and
hosting go on. Every AI step in its fragments meets the fragment's cap,
whoever starts it: an org's fragment has no owner who passes it.

## Who pays

The caller decides who pays a row and sends it to that payer's ledger
(decision 36). Each source keeps its rows in an outbox and sends them on
the ledger queue under a batch id of its own until the ledger has them.

| What | Usage | Reference | Payer |
|---|---|---|---|
| An agent's model call (the route, a decision, a transcription, embeddings) | `tokens` | `aig:` | the agent's owner |
| An operator key's call the provider answered (under 500) | `key` | `key:<computer>:` | the agent's owner |
| A computer's awake time | `awake` | `awake:<computer>:` | its owner |
| A job's AI step | `tokens`, `neurons` | `step:<step's key>` | the fragment's owner |
| A fragment's requests, dynamic workers and storage | `requests`, `dynamic_workers`, `storage` | `req:`, `dw:`, `store:` | the fragment's owner |
| A preview card's shot, stale or failed too | `browser` | `card:` | the fragment's owner |
| An icon's making or upload | `tokens`, `neurons`, `images` | `icon:` | the fragment's owner |
| An export's archive | `storage`, `requests` | `export:<id>:` | the exporting person |

A fragment's owner is a person or an org. A person's agent bills that
person in an org's fragment too (decision 67). A step's key is the same
in every attempt, so a step tried again after its call was paid is
charged once.

Not metered: a person's own keys (decisions 37 and 60: their provider
bills them); a computer's time kept for its failed saves (the platform's:
docs/computers.md); a computer's saves, an app facet's database and git
storage (the debt ledger).

## Usage

What a person's agents used of each provider (M16; Paul, 2026-11-18:
"one place for what my agents used this month") is the ledger's too: each
agent's calls a provider answered through a computer's swap, by month,
and what they cost. A connection's and an own key's are counted at $0
(their provider bills the person); an operator key's at its charge as
the swap priced it. It is a display, not money: what charges is the
meter, above. `fragment_core::ledger::uses` is its state machine, the
ledger's `uses` table its store, `GET /api/ledger/uses[/{YYYY-MM}]`
(Settings' Connections) its read.

- **Off the path, best effort.** A swap sends its count (`/used`) to its
  agent's owner's ledger after its answer, in `wait_until`, one for every
  provider it named; nothing waits on it, and a count lost is lost (Paul:
  "accounting should be best effort, not sprawl into interfering and
  delaying everything that happens"). A count writes only the `uses`
  table, never the head, so the gate's answers never wait on it.
- **Bounded.** Thirteen months are kept (`MONTHS_KEPT`; an older one is
  never read back, and a count, a hand-over or the ledger's daily sweep
  forgets it), and a month takes at most `ROWS_MAX` agent and provider
  pairs (a person's agents at each provider); a pair past that is logged
  (`ledger.uses-full`), and one held counts on.
- **Moved, never dropped.** A computer counted its agents' uses itself
  before. Its step 7 keeps each month it held as owed, and it hands each
  to its owner's ledger as its owner reads its view (`/take-uses`, under
  `<computer>:<month>`). The ledger takes a month once by that id,
  adding it to what it counted since: the same body again adds nothing,
  another under it is refused. The debt ledger says when this goes.

## The price book

The book is the core's defaults (`price/defaults.rs`, each price with its
source, and the margin, `MARGIN_BP_DEFAULT`), the catalog's operator-key
prices and the model slots' rows (`Config::book`; the debt ledger, "The
price book is the core's defaults"). A ledger keeps no book: every
call into it is its own release's (docs/MODEL.md, Calls between
releases), so a batch is priced with the book of the release applying
it, and each book a ledger priced with is kept once as a command,
`book:<digest>`, for the audit. The book's version (`priceBook`) is
informational. The fee is AI Gateway's on credits, so it applies to
`tokens` and `neurons` only.

- **A video's seconds** (crate::media's video model, a third party's in
  Cloudflare's catalog, paid by Unified Billing): `tokens` of a row per
  resolution (`alibaba/hh1-t2v@720P`, `@1080P`), its output one
  millisecond of video, priced per million at the catalog's price a
  second ($0.14, $0.28), with the fee. Charged when the model answers
  with its video, at the seconds asked, as the catalog bills them. No
  usage kind of its own: every release's ledger prices tokens, and one
  without the row charges the video nothing (decision 62).

- **An operator key's call is charged what its vendor said it cost**
  (FIN-616). Its row's `charges` (`catalog::Charge`; the book's for its
  name, `price::DEFAULT_CHARGES`, each with its source) name the calls
  each is for and where its vendor's answer says what the call used: xAI's
  ticks (`xai:tick`, 10^10 a dollar, on a POST), ElevenLabs' characters
  (`character-cost`), Firecrawl's credits (a scrape's and a v2 search's),
  fal's units (each model priced apart: Klein's alone so far), X's posts
  (`meta.result_count`); or one call, at an estimate, where its answers
  say nothing (Perplexity's `call`, Browser Use's `session`, ElevenLabs'
  `music`). The swap reads a header as the answer came and a JSON pointer
  in the pipe's one pass over the answer (a body read whole, or an event
  stream's last event naming it), never holding the answer back. The
  first charge that is for the call and whose number its answer says is
  the call's usage, `<provider>:<unit>`; else the row's own price a call,
  if it names one; else nothing, logged `pipe.uncharged` (Google Places:
  kept uncharged, 2026-11-20). Only a 2xx is charged: a call its vendor
  refused made nothing. A status poll that repeats a running total (a
  crawl's), or a read that says a cost again (a stored response, a
  video's poll), is for no charge. A release whose book lacks a unit
  refuses its rows as unpriced, charging nothing; an older release's
  rows are priced as it priced them (`price::LEGACY_KEYS`, the debt
  ledger).

- **OpenRouter's price** (decision 61; a decision for Paul): a slot on
  OpenRouter is priced at the ceiling it routes under
  (`body.provider.max_price`), every input token at its prompt price,
  never less than OpenRouter could bill, grossed up to OpenRouter's fee
  (`OPENROUTER_FEE_BP`). A slot without one is refused
  (`Upstream::ceiling`). Settling from OpenRouter's own `usage.cost` is
  in the debt ledger.
- **A slot's own price**: a slot on any other OpenAI-compatible API may
  name its model's list price (`price`), which takes the fee and margin
  as every token row does (`Upstream::own_price`).
- **Finite Private's price** (decision 61; a decision for Paul): Workers
  AI's GLM-5.3 Flash's ($0.15 in, $0.50 out a million tokens), kept after
  Workers AI retired that model on 2026-11-20. It reports no cached
  tokens, so its prompts are priced uncached.
- **Deep's price** (decision 65): a slot like any. A delegate's calls
  bill its agent's owner, under no fragment's cap.

A model call that fell back is charged once, at the fallback's usage and
prices. Usage a release's book cannot price is charged nothing and
logged `ledger.unpriced` (#509).

## A quote

`POST /api/ledger/quote` (FIN-448; docs/api.md, Ledger) prices a planned
job before it starts, so an agent can say what it will cost and what is
left (Paul, 2026-11-08: "it will probably cost about $3 and you have $40
left this month, okay to proceed?"), and reads the balance rather than
guessing after a refusal. Each item is a row's usage times its calls,
charged by `ledger::row_charge`, the function a ledger's own rows are
charged by, with the book the answering release charges with: so it
prices every key and unit the book has (`Catalog::key_prices`), the always-on
seat's awake time at nothing, and a name the book does not price is
unknown, never guessed. Its payer is the one a charge resolves to (an
agent's owner). It is as accurate as the units it is given: a unit its
vendor's answer counts (speech's characters, Grok's ticks:
`Catalog::counted`) must be said, a model's tokens are an estimate,
and a call that falls back is quoted on its own model. It holds and
keeps nothing: the job is still charged after, never held (decision 62).

## The gate

Before a spend starts, its payer's ledger is asked `may_spend` (an
agent's turn, an AI step, a wake, a write, a create), and when someone
else spends in a fragment, its owner's ledger is asked `fragment_open`
(its cap), at the same time; if both refuse, the payer's refusal is
the one said. Both go through one cached question: each isolate keeps
each answer, a refusal too, for `STANDING_CACHE_MS`. A ledger answers
by reading alone and writes nothing, as it answers its status (F14); a
ledger's first call, whatever it asks, makes it. Only the ledger's
own refusal stops a spend. A ledger that does not answer, or answers
otherwise, lets the spend start (`ledger.gate-unknown`) and is asked
again next time.

**An agent's paid call waits on no ledger** (2026-11-01: "prioritize
user experience over nickel-and-diming"; `ledger::may_beside`):
its model calls and its calls on an operator key alike go through the
pipe (`cell/src/pipe.rs`), one gate and one charge. A fresh
answer answers it at once, a refusal refusing it before its vendor call.
With none, it starts at once and the ledger is asked beside it, the
answer kept for the next call. Only a question last heard refused,
longer ago than the window, is asked first, for at most `GATE_WAIT_MS`
(the payer may have topped up), and so is one being asked beside another
call now, each call for at most as long: a burst on a cold gate starts
one call, and the rest take the answer heard beside it, or go when the
ledger answers none of them. An ask beside a call holds nothing of
the call's: a call started runs to its end, a stream to its `[DONE]`,
and is charged as any call is. So a payer out of credit, or past a
fragment's cap, may make one call more in each isolate that has not
heard them (paid by the platform, charged to their ledger), and the
next is refused. A refusal said from the cache says the refusal's
standard words.

**Every other spend asks first** (`ledger::may`): a ledger with no
fresh answer is asked once, for at most `GATE_WAIT_MS`. So a payer out
of credit may overspend by what starts within the minute, and one who
topped up waits up to a minute.

- An agent's paid call (its chat, vision, decision, transcription and
  embeddings, a search's embeddings, and an operator key's call through
  its computer's swap) asks `agent_turn` of its payer, beside it.
- A job's AI step asks `ai_step` of the fragment's owner, capped when
  the run's principal is neither the owner nor an agent of theirs.
- A fragment's write asks `write` of its owner (402 past the overdraft).

A paid credit pack becomes `GrantCredit {id: "pack:<Checkout's id>", by:
"stripe"}` on the ledger of the person its metadata names (decision 55);
the registry pushes `SetPlan` and `SetSeat` (docs/billing.md, "What a
seat does").
