# Error responses

What an error response from the cell (the Cloudflare Worker in `cell/`)
tells whoever asked, and what it keeps back. Written for the CASA
assessment (FIN-587).

## The shape

Every refusal is one JSON body, `{"error": <code>, "message": <sentence>}`,
with the HTTP status its code names (`crates/proto/src/lib.rs`,
`ErrorCode`). Some refusals also carry `reason` and `retry_after_ms`.
There are no HTML error pages with stack traces, and no framework error
pages. A panic in the Worker gets Cloudflare's generic error page, which
shows no detail.

## The platform's own failures say nothing about why

`host_failed` (500) means the platform itself failed. Its message is
written for operators, and can hold a JavaScript error's text, a D1 or
Durable Object error, a binding's name, or an internal id. Past the cell's
edge it says only:

> something went wrong on our side; try again, and if it keeps happening,
> tell us this reference (reference `<ray>`)

The reference is the request's Cloudflare `Cf-Ray` when Cloudflare set a
well-formed one, so an operator can find the request in Cloudflare's logs.
Otherwise the cell makes up a random one. The whole message goes to the
Worker's logs as one line, `{"refused": "host_failed", "reference": …,
"message": …}`, and nowhere else.

The pure rule and its tests are in `crates/core/src/public_error.rs`. The
cell applies it at every edge where a refusal leaves for someone outside
(`cell/src/error.rs`, `public_response` and `public_answer`):

- `fetch`, `route`, and the platform's host (`route_inner`), including a
  Durable Object's refusal that a platform route passes on as it came;
- a fragment's host (`serve`), including the fragment object's own refusal
  (`serve::REFUSAL`);
- a computer's host (`computer::serve_host`).

Inside the platform, when one of its own objects answers the router, a
refusal keeps its whole message. The router needs it to decide and to log.
It is cut only at the edge.

## Every other refusal is for the person who asked

Messages for the other codes are written for the person or program that
asked: what was wrong and what to do (`invalid_request`, `forbidden`,
`not_found`, `rate_limited`, `budget_used_up`, and so on). They name only
what the asker sent or may already see.

- `upstream_failed` (502) says which service failed and its HTTP status.
  For a model call it also says whether the call was charged. Callers
  rely on this.
- **A vendor's own words are never in a message.** Their answer body or
  `message` may name the vendor's internals, an id of ours, or a path. They
  are logged where the call is made (`error::vendor_said`, at most 300
  characters) and the refusal names only the vendor and the status:
  - code.storage (`cell/src/cs.rs`, `upstream`);
  - WorkOS: connected accounts, a connection's state, disconnect
    (`cell/src/keys.rs`), a connection's token (`cell/src/computer.rs`),
    and the sign-in exchange (`cell/src/registry/signin.rs`). A 400 there
    now says "this sign-in was used already or is too old: start again";
  - Stripe (`cell/src/stripe.rs`): our request path names Stripe ids, so
    it is logged too, and the refusal says "Stripe refused the call
    (<status>)".
- `schema_refused` (500) names the fragment and why its stored data and
  the deployed code disagree. Its owner needs this to act on, and it names
  nothing beyond their fragment.

## Deliberate exceptions

- **A fragment app's own answers** pass as the app sent them, on the
  app's own origin. That includes an `app_failed` (422) message that the
  app's own code threw. It is the app author's code and their message to
  send.
- **The model route** (`/api/models/…`, OpenAI-compatible) passes a model
  provider's error answer through as the provider sent it. That is part
  of the OpenAI-compatible contract agents are built on. FIN-633 tracks
  keeping that text out of log lines.
- **A computer's egress** (its guest's calls out through the cell) keeps
  the whole message. Only that computer's own agent reads it.
- **404s** echo the path that was asked for (`no route /x`), which the
  asker already knows.
