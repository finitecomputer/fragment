# Incident response plan (FIN-591)

**Draft — needs Paul's approval.** This is a proposal. The tools it
names exist in the repo; the roles, timelines and contacts are under
"To confirm" until Paul approves them.

An incident is anything that exposes, changes or loses people's data, or
lets someone act as someone else: a leaked secret, a token in a log,
unauthorized access, a vulnerability being used, a vendor's breach that
touches our data.

## Roles

- **Incident lead:** the security owner, Alex Lewin (FIN-570), unless
  Paul names someone else for an incident.
- **Approver for irreversible steps:** Paul. Production deploys by hand,
  deleting secrets, DNS changes, teardown and wiping are Paul's
  (AGENTS.md, "Rules and approvals").

## 1. Find out

How an incident can reach us today:

- Reports to support@finite.computer (the privacy policy's contact).
- Master's CI: a red run names what failed (`master-red` in
  `.github/workflows/ci.yml`).
- Worker logs in Cloudflare Workers Observability, and the operators'
  glance in `/admin`, which reads them (logging.md).
- The operators' log of operator writes (`GET /api/admin/log`), and
  `wipes` for wipes. Not every operator act is logged: an object wake
  writes no row (logging.md; FIN-651).
- A fragment's `events` channel for its grants and revokes.
- Vendor notices (Cloudflare, WorkOS, Stripe, code.storage, OpenRouter,
  Google).
- The assessment lab's findings (FIN-600, FIN-601).

## 2. Record it

Open a Linear issue in Finite Engineering, labelled Security, at once.
Write down what was seen, when, and by whom. Keep the timeline in the
issue as it goes.

## 3. Contain

What the platform already lets us do, with its source:

| Need | Action | Source |
|---|---|---|
| A deployment secret leaked | set it again with `cargo xtask secret set`; in use within a minute, no deploy; then revoke the old one at its vendor | docs/secrets.md, "Rotating a key" |
| The host secret leaked | rotate it by name (`host_secret_previous`), deploy. This does not contain a leak by itself: while the old secret stays bound as `host_secret_previous`, values sealed under it still open and placeholders' tags made under it stay valid, and the Registry's sealed person keys are never resealed, so the old secret can't yet be dropped safely (secrets-and-encryption.md, "Key rotation"). To confirm: a full rotation procedure | docs/secrets.md, "Rotating the host secret" |
| An operator key leaked | remove its npub from `operators` and deploy; it works until then | docs/secrets.md, "Operator keys" |
| CI credentials leaked | Paul replaces them in the GitHub environment; revoke at Cloudflare or GitHub | docs/secrets.md, "Files" |
| A person's account taken over | a wipe ends the person's keys and sessions (docs/api.md, "Operators: wiping a person"); revoking a key ends its sessions (docs/MODEL.md) | `cell/src/wipe.rs` |
| A person's connection misused | the person disconnects in Settings: a best-effort revoke at the provider, then delete at Pipes (README.md, "Disconnecting"). The disconnect route acts only on the signed-in person's own accounts; there is no operator route to disconnect for them. Staff could only delete the account in WorkOS or revoke at the provider by hand (to confirm how) | docs/pipes-browser-binding.md; `cell/src/connections.rs` |
| A connector door misused | set the deployment's `connectors` to `[]`: no door is linked or redeemed and no relay opens | docs/computers.md, "Connector policy" |
| A bad release | a deploy whose release isn't verified tries to roll back by itself, only when there is an eligible release before (not a first deploy, not one whose schema or other changes can't be taken back) and the rollback itself can fail, leaving the deploy stuck. The admin Worker deploys after the cell; if it fails, the new cell stays live and only operators' routes are down until a deploy finishes it. Otherwise fix forward with a PR, or `wrangler rollback` within the rollout's rules | docs/MODEL.md, "Calls between releases"; `xtask/src/rollout.rs` (`back`, `eligible`); `xtask/src/deploy.rs` |
| A bad image | pin computers back to the image before | decision 19 |
| A shared vendor key misused | revoke it at the vendor; add the call to its catalog row's `deny` | docs/computers.md, "Calls an operator key never goes on" |

Keep evidence before changing anything that would erase it: export the
relevant logs from Workers Observability first.

## 4. Fix

Fix the cause with a PR, proven as any other (secure-development.md).
For a vulnerability, follow vulnerability-management.md.

## 5. Tell people

- **People affected:** by email, in plain words: what happened, what
  data, what we did, what they should do.
- **Google:** when Google user data is involved, as Google's API
  Services User Data Policy requires.
- **Others** as the law requires where the affected people live.

## 6. Learn

Within a week of closing, write what happened and what changes, in the
Linear issue. File each follow-up as its own issue.

## To confirm

- Paul: who the incident lead is, and the backup when Alex is away.
- Paul: whether there is an on-call rotation or out-of-hours contact,
  and how a report to support@finite.computer reaches the lead.
- Paul: the timeline for telling affected people and Google after an
  incident is confirmed (Google's policy and the applicable laws set the
  maximum; we need our own number).
- Paul: who sends the notices and from which address.
- Alex: how Workers Logs are exported and kept as evidence beyond their
  retention.
- Alex: whether any monitoring or alerting watches production beyond
  master's CI and the operators' glance (none is in the repo).
- Paul: whether a lawyer or insurer must be told, and how.
