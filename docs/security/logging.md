# Logging (FIN-586)

What Finite logs, where it goes, how long it is kept, and how secrets,
tokens and Google data are kept out.

## Where logs go

| Log | What writes it | Where it is kept | How long |
|---|---|---|---|
| Worker logs | the cell and the admin Worker: one JSON line per event (`console_log!`, `console_error!`; lesson 14 in docs/cloudflare-v1.md) and Cloudflare's own record of each request | Cloudflare Workers Logs (Workers Observability), turned on by `cargo xtask deploy` | Cloudflare's retention for the account's plan (To confirm). The privacy policy says "about a week". |
| Container logs | the bridge on each computer: one JSON line per event on stderr (`images/bridge/src/log.rs`) | the container's output, which Cloudflare's container runtime collects (To confirm) | To confirm |
| The operators' log | every operator write: comps, trial codes, pins, ledger commands, migration actions | the Registry's `admin_log` table (`cell/src/registry/admin.rs`, `log_admin`), read at `GET /api/admin/log` | the newest `ADMIN_LOG_MAX` rows |
| A fragment's audit trail | members, invites, grants, revokes, pin moves, face changes | the fragment's `events` channel (docs/MODEL.md, "One log: channels") | `AUDIT_RETENTION_MS` and `AUDIT_KEPT` (`crates/proto/src/lib.rs`) |
| An org's governance log | each change to an org's content admins, offers and grants | the Registry's `org_governance` (decisions 69 to 71) | kept with the org |
| A wipe's record | that a person was wiped, by whom | the Registry's `wipes` (docs/MODEL.md) | kept |

Not logged, by design:

- **Traces are off.** A Workers trace records each fetch's URL and query,
  and a job's fetch can carry a secret filled into its URL. `cargo
  xtask deploy` pins `observability.traces` off for the cell and the
  admin Worker (`xtask/src/deploy.rs`, `observability`; #814; tested in
  the same file).
- **AI Gateway logs are off.** A deploy refuses the `default` gateway,
  which logs (`xtask/src/deploy.rs`, `ai_gateway`; decision 23).
- **Model calls keep only usage.** The model intercept keeps a call's
  final usage, never its request or stream (docs/cloudflare-v1.md,
  lesson 7). Its log line (`model.settled`, `cell/src/models.rs`) has the
  reference, tier, model id, token counts and timing.

## Who can read them

- Worker logs: anyone with access to the Cloudflare account's Workers
  Observability (production-access.md). The operators' glance in
  `/admin` reads them with a key that has Workers Observability Read
  and nothing else (docs/secrets.md, "Where each lives").
- The operators' log: operators only (docs/api.md, "Operators: the
  admin").
- A fragment's `events`: its members, by role.

## What is kept out

The rule is docs/secrets.md's first paragraph: a secret is never in a
log. How the code holds to it:

- **Secrets are read in one place**, `cell/src/keys.rs`, and never
  logged there. App code and agents never hold a deployment key
  (docs/secrets.md, "How code reaches a secret").
- **Sealed values never leave their object.** A Vault value is never
  named by an answer, list, log, event or wipe report
  (docs/secrets.md, "A person's Vault"). A computer handing its old keys
  to the Vault logs only providers and counts.
- **Filled secrets are named back as placeholders.** A job fetch's
  failure message names each filled value as `{{NAME}}`, in each
  encoding a vendor might echo it in (docs/secrets.md; `redact`).
- **Session tokens are hashes.** The registry stores SHA-256 hashes; the
  tokens live only in HttpOnly cookies (docs/MODEL.md).
- **Connection tokens never reach the cell's logs.** They go from the
  provider straight to WorkOS Pipes, and the computer's swap asks Pipes
  for each request without keeping or logging it
  (docs/pipes-browser-binding.md; `cell/intercepts.mjs` writes no log
  line).
- **The bridge logs ids and sizes, never a message's text**
  (`images/bridge/src/log.rs`, module comment).

### Review of the cell's log lines (2026-11-23)

This review read every `console_log!`, `console_error!` and `console_warn!` in
`cell/src` (261 lines), the `console.*` calls in `cell/*.mjs`, and the
bridge's `ev!` lines with error or reason fields. Most lines carry ids
(npubs, fragment names, computer ids, Stripe ids), counts, timings and
the platform's own error messages.

No line logs a token, a key, a session or a Vault value. These lines
can carry text that came from someone else:

| Line | What it logs | Why it matters |
|---|---|---|
| `cell/src/icon.rs:208` (`icon.subject-failed`) | the first 300 bytes of a model provider's error body | a provider's error could echo part of the prompt (a fragment's name and description) |
| `cell/src/searchable.rs:500` (`search.query-unembedded`), `cell/src/across.rs:92` (`search.across-unembedded`) | the embeddings provider's refusal body (bounded by `bounded_text`) | a provider's error could echo the person's search query |
| `cell/src/models.rs:452` (`model.fallback`) | the reason a model call fell back | depends on the provider's error text |
| bridge `error` and `why` fields (e.g. `images/bridge/src/driver.rs`, `post.failed`, `attachment.unattached`) | `e.to_string()` of a failed call; an attachment's file name | an API error's text; a file name a person chose |
| many `e.message` fields | the platform's own error messages | these come from `CellError`; none found that builds a message from a secret |

These are filed as FIN-633. None of them logs Google API
content: the cell never sees Google's answers, which go from the egress
straight back to the computer.

### Request URLs

Cloudflare's own record of a request includes its URL (To confirm which
fields Workers Logs keeps). Three platform URLs carry a value in their
query:

- a share link, `?view=<token>` (decision 48; docs/api.md): long-lived,
  rotatable by the owner, read-only;
- `__signin?token=<redemption>`: single use, 60 seconds, one fragment;
- the OAuth callbacks' `?code=&state=`: single use, bound to the
  browser's cookie and PKCE.

The last two are worthless once used. A logged share-link token would
let someone with log access view that fragment until the owner rotates
the link. Checking this is FIN-634.

## Security events

FIN-586 asks that sign-ins, connection grants and revocations, and
operator actions are logged.

| Event | Logged today | Where |
|---|---|---|
| Operator actions | yes | the operators' log (above); a wipe in `wipes` |
| Grants and revokes of fragment access | yes | the fragment's `events` |
| Org content-admin changes | yes | `org_governance` |
| Sign-in | partly | the Registry keeps each subject's last sign-in time (`subjects.signed_in_at`); no log line per sign-in, beyond Cloudflare's request record |
| Sign-out, session revoked | partly | the Registry deletes the session; no log line on success |
| Connection granted or revoked | partly | only failures are logged (`cell/src/connections.rs`: `connection-sign-in` unfinished, `connection-disconnect` not revoked); success is not |

Filed as FIN-632.

## Gaps

- No log line for a successful sign-in, sign-out, connection grant or
  connection revocation (FIN-632).
- A few lines log text from model providers or API errors (table above;
  FIN-633).
- A share-link token, or request headers, may reach Workers Logs
  (FIN-634).
- Log retention and container log collection are not written down from
  Cloudflare's settings.

## To confirm

- Alex: the Workers Logs retention on the production account's plan,
  and whether it matches the privacy policy's "about a week".
- Alex: which request fields Workers Logs keeps (URL with query,
  headers such as `Cookie` or `Authorization`). If headers are kept,
  that is a finding.
- Alex: whether container stdout and stderr reach Workers Logs, and
  whether Hermes (third-party, on the computer) writes message content
  to its own log files in `/data`, which saves then keep.
- Alex: who holds access to Workers Observability in the Cloudflare
  account (production-access.md).
