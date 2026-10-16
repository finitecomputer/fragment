# Exact SaaS hostnames with external DNS

`cargo xtask deploy --config <outside-repo-file> --branch fragments`
can route public hosts in `lwn.lol` through a different Cloudflare zone,
without moving `lwn.lol` or its registration. This is an optional branch
deployment mode. Existing same-zone and standalone configurations keep
their behavior.

Add this object to an ordinary deployment config (secret fields still
name Secrets Store entries, never values):

```json
{
  "zone": "ratemymdp.com",
  "saas": {
    "public_suffix": "lwn.lol",
    "fallback_origin": "service.ratemymdp.com",
    "hostnames": [
      "fragments.lwn.lol",
      "midnight-conservatory--k3x9--fragments.lwn.lol",
      "0123456789abcdef01234567--computer--fragments.lwn.lol"
    ]
  }
}
```

`k3x9` is an illustrative platform-generated name suffix, not a username.
Upstream removed usernames (decision 47): create each app through the platform
and use its returned full name, such as `midnight-conservatory--k3x9`, to
provision its exact hostname. Legacy `<label>--<username>` hosts are not aliases.
List every intended app host and computer host. A computer's ID is
`computer:<24 lower-case hex>`; use the ID returned by the computers API
to provision `<24 hex>--computer--fragments.lwn.lol`. Its screen and port
ticket URLs use that exact origin. Adding the route grants no access:
the runtime still requires the computer owner's signed request or its
existing ticket/session authorization. A computers-enabled deployment's
app-only hostname list cannot serve its computers' screen or port URLs.
Provision each intended computer's exact hostname when its ID is known;
do not substitute an agent fragment's name or a wildcard.

With branch `fragments`, resources
are named `fragment-fragments`,
`fragment-jobs-fragments`, `fragment-blobs-fragments`, and the three queues
`fragment-deliveries-fragments`, `fragment-deliveries-dead-fragments` and
`fragment-ledger-fragments`; code.storage repositories start `fragments--`.
The branch name determines resource names and the public label mark.

Inspect the exact plan without credentials, builds or vendor calls:

```sh
cargo xtask deploy --config <outside-repo-file> --branch fragments --dry-run
```

The renderer uses `ratemymdp.com` only for route ownership and the provider
DNS record. Platform URLs, sign-in callbacks, canonical app URLs and hosted
test targets use `lwn.lol`. Cookies keep the runtime's existing host-only
behavior. Public DNS and SaaS certificate provisioning are separate steps;
the deploy command does not claim to provision or validate either.

## Provider setup

1. Verify existing Cloudflare account access, Workers Paid and the dynamic
   Worker/facets, Workflow, Queues, R2 and Secrets Store capabilities. Resolve
   any new billing, legal or persistent-access approval before enabling it.
2. Enable Cloudflare for SaaS on the provider zone. Verify its remaining
   hostname allowance; current documentation includes 100 on self-service
   plans, with additional hosts priced separately. The configuration caps
   its list at 100; that is not a billing cap or an account-usage check.
3. Create the **originless**, proxied `service.ratemymdp.com AAAA 100::`
   record with no other origin address, and set it as the SaaS fallback
   origin. The optional DNS token can create a missing record; it does not
   set the SaaS fallback origin or repair an existing record.
4. Provision each exact public hostname through Cloudflare for SaaS, using
   its returned ownership/certificate validation requirements. At Vercel,
   create each matching `lwn.lol` CNAME to `service.ratemymdp.com`, preserving
   unrelated records. Do not request an Enterprise wildcard certificate.
5. Deploy the Worker. Every route is `<exact-public-host>/*` in the provider
   zone. No `*/*`, provider-apex route or wildcard app route is generated.
   Check the account for pre-existing broad routes before launch; they are
   outside this renderer's control. The fallback host itself exposes no
   platform API. The Worker keeps workers.dev and preview URLs disabled.
6. Configure WorkOS callback `https://fragments.lwn.lol/auth/callback`.
   Keep signup disabled, use the real client ID (not an environment ID),
   and leave new users on `guest`. Grant only the intended owner a seat
   through an explicitly configured operator after real sign-in. People and
   operators use npubs on current upstream; do not reuse the old `id:` identities.

The public suffix shares a registrable domain with the platform. Preserve
the runtime's existing cookie and Fetch Metadata isolation checks. Never
set a parent-domain session cookie to work around login issues.

## Acceptance

Wait for hostname and SSL status to become active, then verify actual HTTPS
and the deployment revision at `/healthz`. Test real AuthKit sign-in,
callback and logout; navigate each app, connect its WebSocket, inspect
host-only cookies, and check outsider and cross-app denials. For each
provisioned computer, open its screen through an owner-issued port ticket
and confirm an unauthenticated request is refused. Probe unrelated
provider hosts to confirm they do not serve platform endpoints. Source
tests and successful provisioning alone are not live acceptance.

The hosted e2e CLI now targets the public suffix. Its normal tests create
random `e2e-*` app names, which a fixed thirteen-host list does **not**
provision. Run those suites on a separate test deployment with their own
hostname provisioning. Do not broaden live routes or enable test levers
to make that suite pass. Live demo acceptance uses real invited identities.

Checked 2026-10-08 against Cloudflare's
[Worker fallback origin guide](https://developers.cloudflare.com/cloudflare-for-platforms/cloudflare-for-saas/start/advanced-settings/worker-as-origin/)
and [SaaS plans](https://developers.cloudflare.com/cloudflare-for-platforms/cloudflare-for-saas/plans/).
