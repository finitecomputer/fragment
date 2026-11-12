# fragment self-hosted

Self-hosting runs master's cell on other implementations of the interfaces
it is written against. It lands on master seam by seam: each is switched
on by configuration, and master's Cloudflare path is unchanged. Where this
disagrees with `docs/cloudflare-v1.md`, that doc wins.

## The rule: one cell, interfaces, placement as configuration

The cell is written against Cloudflare's runtime APIs (Durable Objects,
the Worker Loader and facets, Workflows, Queues, R2, `ctx.container`) and
protocols for what sits outside a runtime (OpenAI's chat completions, the
code-store contract, the DevTools Protocol, Web Push). Self-hosting
supplies other implementations of the same interfaces. The cell never
asks "am I self-hosted?": it reads what it has from its configuration and
its bindings. A mode flag is a design bug.

## 12. Secrets: the store's bindings, stood in for

The cell reads each deployment secret as `await env.<BINDING>.get()`
(cell/src/keys.rs, through workers-rs's `SecretStore`). A self-hosted
runtime has no Secrets Store (celld's deploy refuses
`secrets_store_secrets`), so `cell/secrets.mjs` gives the cell stand-ins
with the same face: every class in `cell/entry.mjs` that runs the
platform's code takes its env through `withSecrets`, which, when the
Worker variable `FRAGMENT_SECRETS` names the deployment's secrets (the
pairs a deploy writes into `secrets_store_secrets`, and a backend), puts
an object with an async `get()` in each binding's place. Its class is
named `Fetcher`, as workerd's is, since workers-rs takes a store binding
by that name. Without `FRAGMENT_SECRETS` (Cloudflare, `wrangler dev`) the
env is as it came; the Rust is the same on both and never knows which it
read.

- **`vars`:** the Worker variable of the binding's own name, in
  `.dev.vars` (`devstack::Secrets::Shim`): a convenience for dev and the
  e2e alone. The shim refuses it on any fleet that is not local
  (`FRAGMENT_EGRESS_LOCAL=allow`).
- A `FRAGMENT_SECRETS` that names no backend is refused at the first
  request, loudly; nothing falls back to a backend it did not name.
- People's own secrets are not here: they stay in master's Vault Durable
  Object, sealed under keys derived from the host secret, as on
  Cloudflare.
- The e2e checks the stand-ins on wrangler: `FRAGMENT_E2E_SECRETS=vars`
  runs its node with them (its local config binds no store).
