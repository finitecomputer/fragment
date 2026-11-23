# Who can reach production and user data (FIN-593)

**Draft — needs Paul's approval.** This lists the ways into production
that the repo defines. Who holds each account is not in the repo; those
names are under "To confirm".

## Ways in

| Access | What it reaches | How it is granted | Source |
|---|---|---|---|
| Cloudflare account (production) | the Workers, Durable Objects, R2, Secrets Store (names, and setting values; to confirm against the account's roles that the store's API gives no value back, though code bound to a secret reads it), Workers Logs, containers, DNS | Cloudflare account membership; roles to confirm | decisions 1, 3; docs/secrets.md |
| GitHub `production` environment | the deploy job's secrets (`CLOUDFLARE_API_TOKEN`, `DEPLOY_REPO_TOKEN`, `CLOUDFLARE_ACCOUNT_ID`), and the preview registry's token and account id (`PREVIEW_REGISTRY_TOKEN`, `PREVIEW_ACCOUNT_ID`), which only the manual image workflow's copy into production's registry uses | set by Paul | docs/secrets.md, "Files"; `.github/workflows/ci.yml` (`deploy`); `.github/workflows/image.yml` (`copy`) |
| Pushing `production` | deploys to production | restricted by a GitHub ruleset to one bypass actor (to confirm against the live ruleset; auditor evidence provided privately) | secure-development.md |
| Merging to `master` | deploys to preview, not production | anyone with write on the repo, through a PR | secure-development.md |
| finitecomputer/fragment-deploy | the deploy configs: names and paths, never secret values | GitHub repo access; CI reads it with a read-only token | docs/secrets.md |
| Operators | `/admin`: people's emails, seats, sign-in times, counts of what they own and share, the operators' log; comps, credit, pins, starting a computer from a save; and, with an operator key, a wipe | the deployment's `operators` list of npubs, set in the deploy config | docs/api.md, "Operators: the admin", "Operators: wiping a person" |
| WorkOS (production environment) | sign-in users, and Pipes, which holds connected accounts' tokens | WorkOS team membership | decisions 3, 22 |
| Stripe | customers and subscriptions | Stripe account membership | docs/billing.md |
| code.storage | every fragment's git | the org key in the Secrets Store; code.storage account membership | docs/secrets.md |
| Google Cloud | the OAuth client for Google connections | Google Cloud project membership | docs/pipes-browser-binding.md |
| OpenRouter, tool vendors | the platform's own keys and usage at each | vendor account membership | decision 61; docs/computers.md |

## What operators can and cannot see

- The operators' API answers emails, seats, last sign-in and counts. It
  has no route that reads a person's chats, files or secrets (docs/api.md,
  "Operators: the admin").
- `/admin` answers only an operator's platform session; to anyone else it
  is a 404. Its API decides again on every call.
- Most operator writes are in the operators' log; an object wake is
  not, and a wipe is in `wipes` (logging.md; FIN-651).
- No operator route reads a sealed secret. That is a property of the
  routes, not of the encryption: Secrets Store values are returned to
  the Worker code bound to them, and a sealed value opens for any
  Worker code that holds the host secret and names its scope
  (secrets-and-encryption.md, "What Finite seals itself").
- A person's agents act for that person, never for an operator.

## What platform access could reach

Someone with Cloudflare account access could deploy code, and code can
read any Durable Object's storage, read every bound secret and open
every sealed value. So Cloudflare account access is, in
effect, access to all user content. The same holds for code.storage's
org key (every fragment's git) and WorkOS (connected accounts' tokens,
through Pipes). These are the accounts to keep smallest.

## Adding and removing someone (proposed)

1. Paul approves the access in writing (a Linear issue).
2. The account owner adds the person with the least role that does the
   job, and MFA on (FIN-579).
3. The issue records what was granted and when.
4. To remove: take the person out of each account in the table, remove
   their npub from `operators` and deploy, and rotate any secret they
   could read (docs/secrets.md, "Rotating a key").
5. Review the whole list every quarter.

## To confirm

- Paul: who holds each account in the table, and with what role
  (Cloudflare, GitHub org admins and repo writers, the GitHub
  `production` environment, fragment-deploy, WorkOS, Stripe,
  code.storage, Google Cloud, OpenRouter, tool vendors).
- Paul: who is listed in production's `operators`, and who holds each
  operator key file.
- Paul: whether the deploying machine's files (DNS token, Stripe key)
  live only on Paul's machine.
- Alex: whether MFA is enforced on each account (FIN-579).
- Paul: whether the add, remove and quarterly review steps above become
  the policy.
