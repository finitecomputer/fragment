# Browser-bound Pipes connections

`POST /api/connections/:provider/authorize` is a platform-browser operation.
A signed key cannot start it. A connection needs `connection_oauth` in its
catalog row; without it authorization fails closed. There is no fallback to
WorkOS's `/data-integrations/:provider/authorize` URL.

The Registry creates a random state and S256 PKCE verifier, seals the verifier,
and keeps the state hash with the initiating platform-session hash, provider,
WorkOS issuer/subject, configuration hash and ten-minute expiry. The answer
sets a host-only HttpOnly, SameSite=Lax `fragment_connection` cookie
(`__Host-` and Secure on HTTPS). Starting again replaces that browser's cookie.
At most eight attempts per session and the fleet's pending-sign-in limit in
total are retained. Expired attempts are swept; wiping the person removes them.

The provider returns to the platform's own
`/api/connections/:provider/callback`. `auth::trip_bound` proves possession of
the initiating browser's cookie. The Registry requires the *same live session*,
provider and unexpired transaction and atomically consumes it **before** any
external exchange. Another browser, person or session cannot consume it. The
provider exchanges its one-time code with the saved PKCE verifier and client
credentials. Fragment requires a complete bearer grant (access token, refresh
token, bounded expiry and all requested scopes), rechecks the live session and
WorkOS subject, then imports it into that subject's Pipes account. Tokens are
never returned to the browser or stored by Fragment; WorkOS stores and refreshes
them. No response-body diagnostics, redirects or automatic network retries are
used on the credential-bearing requests.

The documented import is `POST /user_management/users/:user_id/connected_accounts/:slug`.
Only an explicit 409 permits a `PUT` update of that same user's existing account.
An uncertain result consumes the attempt; reconnect rather than replaying it.
This deliberately supports confidential clients using `client_secret_post`,
authorization code, S256, and a space-delimited scope response. Other provider
protocols require an explicit implementation, not an unbound fallback.

## Evidence and scope

- [Google's web-server OAuth flow](https://developers.google.com/identity/protocols/oauth2/web-server)
  documents state validation, the callback and code exchange.
- [Google's OAuth client](https://github.com/googleapis/google-auth-library-nodejs/blob/main/src/auth/oauth2client.ts)
  exposes S256/code_verifier with the same confidential-client exchange.
- [WorkOS connected-account reference](https://workos.com/docs/reference/pipes/connected-account)
  documents import, refresh-token/access-token/expiry combinations, 409 on an
  existing account, and update. A refresh token without the configured matching
  OAuth client will not make refresh work; the deployment must use the same
  client as its WorkOS integration.
- The published `@workos-inc/widgets` 1.20.1 Pipes implementation requests
  `requireHandoff: true` and exchanges handoff/redirect tokens with a popup.
  Its private GraphQL completion contract is not the public REST contract.
  This change does not infer a vendor guarantee from that implementation or
  call its private API.

The local `connections` e2e lane uses two users and separate browser cookie
sets. It verifies copied URLs, missing/wrong-kind cookies, another user's and
another same-user session, replay, expiry, superseded starts, PKCE code
substitution, restart and reauthorization against real workerd and local
provider/Pipes fakes. Protocol tests reject malformed/partial grants and
configuration overriding binding parameters. Local tests establish our ordering
and attribution, not the live vendor configuration or refresh behavior.

## Rollout still required; this change does none of it

Paul reviews and deploys. Keep the staging provider disabled until the code and
configuration are reviewed. No new GCP IAM grant is required to implement or
test this PR locally.

The repository's normal release path is `.github/workflows/ci.yml`: PRs test;
a green `master` push deploys the GitHub `preview` environment with
`cargo xtask deploy --pinned-only --branch preview`; the `production` branch
uses the separate `production` environment without that branch flag. Deployment
requires green `check` and `e2e`. Production promotion is a later, separate
decision, after staging validation. No separate environment or manual deploy
is needed for this fix.

| Setting | Staging | Production | Reusable? |
| --- | --- | --- | --- |
| GitHub deployment environment / source branch | `preview` / `master` | `production` / `production` | Mechanism only; keep environment restrictions separate |
| Platform origin | Existing `preview` environment's `DEPLOY_HOST` and deployment config | Existing `production` environment's `DEPLOY_HOST` and deployment config | No; verify actual targets before merging/promoting |
| WorkOS environment and user issuer | Existing staging environment `environment_01KSB8XM8RSN80NM31YVYBAY20` | Existing production environment, verify its identifier | No cross-environment credentials or user IDs |
| Google project / OAuth client | `finitestaging228527660165` / existing `Finite Pipes Staging` client | Existing production project/client, verify; do not duplicate or rotate | Separate client IDs and secrets |
| Google authorized redirect URI | `https://<staging-platform>/api/connections/google/callback` | `https://<production-platform>/api/connections/google/callback` | Path is shared; origins are separate |
| OAuth endpoints and compatibility scopes | Reviewed endpoints/scopes below | Same protocol and reviewed scope set | Yes, subject to production consent-screen review |
| Cloudflare binding | `CONNECTION_SECRET_GOOGLE`, existing staging client's secret in its store | Same binding name, production client's secret in its store | Binding name only; not secret values |
| Deployment catalog | `connection_oauth` + store-name `key` in preview `DEPLOY_CONFIG` | Equivalent row in production `DEPLOY_CONFIG` | Shape only; client ID/store names/origin must be checked per environment |
| WorkOS provider | Existing staging Google integration, currently Disabled | Existing production integration, status to verify | Keep enablement separate and gated |
| Canonical privacy-policy / terms URLs | **Pending placeholders**; not a blocker for the controlled External/Testing staging work | **Pending placeholders**; do not submit unfinished policies for production verification | Reuse the eventual approved canonical URLs; none are invented or published here |

The existing WorkOS API/client bindings and Cloudflare deployment credentials
need no rotation for this feature. The new OAuth client-secret binding is an
additional consumer of the existing corresponding Google client, not a request
to generate new credentials. Parent coordinates exact account configuration;
none of these secret values were read or printed here.

1. Add the exact staging platform callback
   `https://<staging-platform>/api/connections/google/callback` to the **same**
   Google OAuth client configured in the staging WorkOS integration. The existing
   WorkOS callback alone does not serve this flow. Do not point staging at a
   production client or callback.
2. Bind that client's secret through Cloudflare Secrets Store as
   `CONNECTION_SECRET_GOOGLE`. In the deployment provider row, `key` is its
   **store name**, never the value. Add `connection_oauth` with `client_id`,
   `authorize: https://accounts.google.com/o/oauth2/v2/auth?access_type=offline&prompt=consent`,
   `exchange: https://oauth2.googleapis.com/token`, and the reviewed compatibility
   scopes. Endpoint/client/scope settings are public configuration; secrets are
   read only by `cell/src/keys.rs`.
3. Review prior accounts and pending legacy authorization URLs in any environment
   that used the old flow. This patch cannot invalidate URLs already issued by
   WorkOS or undo existing provider grants. Resolve those before enabling use;
   deleting a Pipes account does **not** revoke the underlying Google grant.
4. Separately authorize provider enablement and a controlled staging grant.
   Start with the sole approved test account, isolated browser profiles and a
   dedicated Fragment test user. Verify a copied link cannot import or expose
   credentials, then the original profile can complete, replay cannot exchange,
   and the imported account belongs to the initiating WorkOS user. Inspect only
   status/attribution, never token values. Separately authorize a minimal API
   read and refresh test. A genuine two-Google-user test requires another
   explicitly approved test account; do not use someone else's account.
5. Revoke the test grant at Google and remove the test connected account after
   validation. Production requires Paul's separate rollout decision.

For deployment verification, require CI's `/healthz` `x-fragment-deploy` header
to match the merged commit, and verify that a connection with no OAuth config
refuses authorization without redirecting or importing. Then use only the
separately approved staging test above. Provider status remaining Disabled is
not evidence that the new callback/import works live.

Rollback: first disable new connection starts by removing both `connection_oauth`
and its provider-row `key` from that environment's catalog through the normal
deployment path. An orphan `key` is rejected by deployment validation. Preserve
Registry schema step 2 in any rollback build: deploying a pre-step worker onto
the upgraded Registry is refused by the schema guard and can break sign-in.
Use a reviewed forward fix or revert of feature code retaining the migration,
not an unexamined promotion of an older SHA. Credential revocation or deletion,
if needed, is a separately authorized account operation.

As of 2026-10-20, the user's staging OAuth client and compatibility scopes are
configured and the WorkOS Google provider remains **Disabled**. No live grant,
token import, refresh test, provider enablement or deployment was performed for
this change. Production, Places and billing remain outside this work.
