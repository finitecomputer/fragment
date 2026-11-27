# Error responses

What an error response from the cell (the Cloudflare Worker in `cell/`)
tells whoever asked, and what it keeps back. Written for the CASA
assessment (FIN-587).

## The shape

Every refusal is one JSON body, `{"error": <code>, "message": <sentence>}`,
with the HTTP status its code names (`crates/proto/src/lib.rs`,
`ErrorCode`). Some refusals also carry `reason` and `retry_after_ms`.
There are no HTML error pages with stack traces, and no framework error
pages. A few refusals a browser navigates to are HTML pages instead (a
fragment's sign-in or access page, `auth::refused`; sign-in's own,
`auth.rs`): they say the same sentences, HTML-escaped, and never a
redacted code's message. A sign-in's callback that WorkOS sent back with
an `error` names that OAuth code (`access_denied`, when it is shaped like
one: `public_error::oauth_error`). It says the redirect's
`error_description` too, escaped, without control characters and at most
300 characters, only when the redirect's `state` is the one this
browser's own sign-in holds (`public_error::oauth_description`): then the
provider of that sign-in wrote it (the identity door's explanations,
"the sign-in provider has not verified the email …"; WorkOS's), as OAuth
means it, for the person. A link no sign-in here began brings its
maker's words, so those go to the logs alone. The identity door's
`the sign-in provider said: …` passes on its upstream provider's own
description, which the door cuts to 200 characters and the same rule
binds: only a redirect of the door's, back to the browser that began it,
can bring it. A panic in the Worker gets Cloudflare's generic error page, which
shows no detail.

## The platform's own failures say nothing about why

`host_failed` (500) means the platform itself failed.
`registry_unavailable` (503) means the identity registry did not answer,
and its message wraps the registry's own refusal (a storage error's
words among them), so it is redacted the same way, as "the identity
registry did not answer; try again shortly, …" and a reference. Its message is
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

- `fetch`, the one door every answer of the cell's leaves by, on every
  host: it says any refusal still marked as an object's (below) as the
  edges within do, an early return's among them;
- `route`, and the platform's host (`route_inner`), including a
  Durable Object's refusal that a platform route passes on as it came;
- a fragment's host (`serve`), including every refusal the fragment
  object itself answers a site request with (a storage failure reading
  its facts, say), not only a 401 or 403 (`serve::REFUSAL`);
- a computer's host (`computer::serve_host`), including the computer
  object's own refusal on `/p/<port>/…` (its sessions' storage, its
  port's runtime);
- a computer's egress (`ComputerEgress::handle`, its guest's own door,
  which is not `fetch`): the computer object's refusal on
  `/api/computer/keepalive` is said as at any edge, and no answer leaves
  there with the mark.

The edge tells the platform's refusal from an app's answer by a mark:
every refusal one of the platform's objects answers carries
`x-fragment-platform-refusal` (`error::PLATFORM_REFUSAL`, set by
`CellError::response`). Only a marked JSON answer at a redacted code's
status is read and redacted (`public_error::rereads`), and the mark is
dropped from every answer that leaves the Worker. The mark cannot be
forged: a foreign answer is stripped of it, in any case, where it enters
the platform (`error::foreign`, `public_error::forged`): a fragment app's
answer (`js.rs`, the facet's `fetch`), a computer image's on a port
(`computer.rs`, `port`), and code.storage's passed to a guest. So an app
cannot have its words logged as a platform failure under a request's
reference, and no guest or browser ever reads the mark.

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
  - the embeddings model behind a fragment's search index
    (`cell/src/searchable.rs`): "the embeddings model refused (<status>)";
  - WorkOS: connected accounts, a connection's state, disconnect
    (`cell/src/keys.rs`), a connection's token (`cell/src/computer.rs`),
    and the sign-in exchange (`cell/src/registry/signin.rs`). A 400 there
    now says "this sign-in was used already or is too old: start the sign-in
    again";
  - Stripe (`cell/src/stripe.rs`): our request path names Stripe ids, so
    it is logged too, and the refusal says "Stripe refused the call
    (<status>)". Its timeout, an over-long answer and one that does not
    decode say so with no path or id ("Stripe did not answer").
  - a model call that got no answer (`cell/src/models.rs`, `unanswered`):
    the Workers AI binding's exception, or a fetch's failure, names the
    runtime's internals, so it is logged and the refusal says only "the
    model did not answer". A fragment's search, which embeds through
    this, passes on only that.
  - The rest of the 502s that still quote a vendor's or the runtime's
    text are tracked in FIN-654.
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
- **A computer's egress** (its guest's calls out through the cell): a
  call to the platform's API or a model re-enters the cell's own router
  (`crate::route`), so a platform failure there is redacted as at any
  edge before the guest reads it, and so is the computer object's refusal
  on `/api/computer/keepalive`. A refusal the egress handler makes itself
  (`ComputerEgress::handle`: signing, a route it has no answer for, an
  internal call's failure it wraps) keeps its whole message, unmarked:
  only that computer's guest (its owner's agent) reads it.
- **404s** echo the path that was asked for (`no route /x`), which the
  asker already knows.
